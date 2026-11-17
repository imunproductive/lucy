//! Match-authoritative damage, healing, deaths and respawn timing.
//!
//! The body scripts supply shot events. The match supplies the command and
//! pose at consumption, and owns identity-gated delivery. Hit selection,
//! damage, and respawn order follow ow174/game/combat.py: the targets are the
//! room's players' bodies, then the training bots (`Combat._targets`), and a
//! shooter's enemies are the targets of another team, or everyone else in a
//! free-for-all (`Combat._enemies`). A player's pools live in his body's
//! script, so a hit on a player is kept here as a [`PlayerHit`] for the match
//! to apply to that body (`Combat._damage`, `_died`).
//!
//! Client research (1.74): health is component 51; an op-2 update of a
//! pool's field changes the bar on every client that has the body. A body
//! whose health pools (type 0) add up to 0 with a maximum above 0 is dead
//! (0x7FF789ADBF00): the client then sets flag 0x2000 on its movement state
//! (0x7FF7896A4F9A) and the 3P animation takes the dead pose 004.015
//! (0x7FF789B24889); flag 0x2000 in the records does the same. A dead body's
//! statescript takes no presses (as the client's LogicalButton
//! 0x7FF78AAAC570). The game mode entity carries the client UI graph 0CB6,
//! whose client-only entries make the kill feed 0DEA and the notices 1FC0;
//! every full frame of that entity has to list 0CB6, or the client destroys
//! it. The kill feed line: the dead body's 01CF takes 003E.025 (killer 0012,
//! crit 0095, loadout 03E6, assists 03E7), pulses its ClientOnlyPulser st108,
//! and every client's 01CF sends 0765.025 to the mode entity's 0DEA. Targets
//! are rewound to the command's view time (0x7FF7894BEE40 / 0x7FF7894BEEC0).
//! The client ignores a destroyed entity id for 30 s, so a respawned bot gets
//! a new id. Unverified (OW1 rule; the client has no damage code): armour
//! stops 5 per hit, at most half.

use std::{
    collections::{BTreeSet, HashMap},
    fs, io,
    path::Path,
    sync::Arc,
    time::Duration,
};

use crate::{
    bot_plate::BotPlate,
    bot_script::SeenRocket,
    bots::{BOT_ENTITY, BOT_RECORD_DELAY, Bot},
    hitscan::{HitscanShot, Impact, TargetHistory, TargetPose, ViewFrame},
    projectile_flight::{FlightEnd, ProjectileFlight},
    projectiles::{
        BioticField, FieldDeliver, FieldIds, FiredProjectile, ProjectileEvent, SeenIds, ShotKey,
    },
    replication::WorldDelivery,
    scene::{bot_controller_entity, bot_controller_link, bot_create, bot_filter},
    scene_assets::PreparedScene,
};
use orpheus_content::{heroes::Eyes, stats::PlayerStats};
use orpheus_core::guid::parse_guid;
use orpheus_physics::static_ray::{Vec3, segment_distance};
use orpheus_script::{
    bot::{DAMAGE_AMOUNT, DAMAGE_DIRECTION, DAMAGE_SOURCE, DAMAGE_TAGS, DAMAGED, DIED},
    mode::EVERY_ENTITY,
    vitals::{Death, Vitals},
    vm::{GameMessage, HealthChange, ShotEvent, TargetCandidate},
    weapons::{AllyHits, DamageTags, HitDefinition, ProjectileBallistics},
    wire::{Value, Variables},
};
use orpheus_wire::{
    combat_messages::CombatNotice,
    commands::Command,
    entity::{EntityUpdate, Operation},
};

/// A hero waits this long in a mode whose graph sends no respawn time
/// (combat.py `RESPAWN`, the PvP modes' 10 s; [`RespawnTimes`] holds every
/// mode's whose graph sends one).
pub const RESPAWN: Duration = Duration::from_secs(10);
/// Between respawn attempts while every body id of the player is still
/// reserved (combat.py `RESPAWN_RETRY`).
pub const RESPAWN_RETRY: Duration = Duration::from_millis(250);
/// The match's deferred messages between bodies per tick (match.py
/// `MAX_BODY_MESSAGES`).
pub const MAX_BODY_MESSAGES: usize = 256;
/// At most about a second of retained combat frames per update (combat.py
/// `COMBAT_CATCH_UP`).
pub const COMBAT_CATCH_UP: u32 = 64;
/// The movement-state flag of a dead body: the 3P animation's dead pose
/// (combat.py `DEAD`).
pub const DEAD: u32 = 0x2000;
/// 003E.025's parameters, by the low 16 bits of their 026 identifiers
/// (01CF rid4, 0C18 rid6; combat.py `KILLER`, `CRIT`, `LOADOUT`, `ASSISTS`).
/// The tags go under 003D, [`DAMAGE_TAGS`], as in the damage message.
const KILLER: u32 = 0x0012;
/// 00BD, the body credited with the kill (01CF #40): its client lights the
/// kill feed line up (rid55 `03EE`: the local view is #392 or #40).
const CREDITED: u32 = 0x00bd;
const CRIT: u32 = 0x0095;
const LOADOUT: u32 = 0x03e6;
const ASSISTS: u32 = 0x03e7;

/// Each mode's respawn time for heroes (combat.py `respawn_seconds`): the
/// seconds its mode graph sends the player in 06FD.025, derived into
/// `game_respawns_174.json` (0F4A and 0EC2 send 10, the Practice Range's
/// 13C0 sends 3).
#[derive(Debug, Default)]
pub struct RespawnTimes(HashMap<u64, Duration>);

impl RespawnTimes {
    pub fn load(path: impl AsRef<Path>) -> io::Result<Self> {
        Self::from_json(&fs::read_to_string(path)?)
    }

    pub fn from_json(text: &str) -> io::Result<Self> {
        let raw: HashMap<String, f64> = serde_json::from_str(text).map_err(invalid)?;
        raw.into_iter()
            .map(|(mode, seconds)| {
                let mode = parse_guid(&mode).ok_or_else(|| invalid("bad respawn mode GUID"))?;
                let seconds = Duration::try_from_secs_f64(seconds).map_err(invalid)?;
                Ok((mode, seconds))
            })
            .collect::<io::Result<_>>()
            .map(Self)
    }

    /// The mode's time, or [`RESPAWN`] when its graph sends none.
    pub fn for_mode(&self, mode: u64) -> Duration {
        self.sent(mode).unwrap_or(RESPAWN)
    }

    /// The time the mode's graph sends in 06FD.025, if it sends one.
    pub fn sent(&self, mode: u64) -> Option<Duration> {
        self.0.get(&mode).copied()
    }
}

/// What a player's body script needs of the scene besides its graphs: its
/// eyes and the Biotic Field.
#[derive(Clone, Debug)]
pub struct BodyCombat {
    pub eyes: Eyes,
    pub field: Option<BioticField>,
}

fn invalid(error: impl std::fmt::Display) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error.to_string())
}

/// The "died" message a dead body's script takes (combat.py
/// `_death_parameters`): the killer, the body credited with the kill,
/// whether the blow was critical, its loadout, its damage tags and the
/// assists (none). The dead body's `01CF` (a bot's `0C18`) shares #392,
/// #40, #510, #1821, #9300 and #9296 with every client, whose client-only
/// plug posts the kill feed line (`0765.025`) from them: killer `03E9`, icon
/// `03EB` (the loadout), headshot `03EF` (crit) and `08AF` (the tags hold
/// `4FFB`). Python and upstream sent neither 00BD nor 003D. Assumption: the
/// credited body is the killer itself (no turret or pet kills anyone
/// here). Unsent and unknown: 006B, 0142 and 03E8 (`03ED`, 0DEA #9297).
pub fn death_message(
    killer: u32,
    critical: bool,
    loadout: Option<u64>,
    tags: DamageTags,
) -> (u64, Variables) {
    (
        DIED,
        Variables::from([
            (KILLER, Value::Entity(killer)),
            (CREDITED, Value::Entity(killer)),
            (DAMAGE_TAGS, tags.to_value()),
            (CRIT, Value::Bool(critical)),
            (LOADOUT, loadout.map_or(Value::Null, Value::Asset)),
            (ASSISTS, Value::Array(Vec::new())),
        ]),
    )
}

/// The damage message (`0011.025`, [`DAMAGED`]) a body's script takes for a
/// hit it survived: the damage's tags, who dealt it, how much and the way it
/// travels ([`damage_direction`]). Neither Python nor upstream sends it; no
/// graph does either, so the server does, as for the death message. The
/// Training Bot's flinch and pain graphs take it (`0E8C`, `0C17`), and
/// every hero body's: its pain line (`01CA` or `14B1`, after `004B` rid 63
/// rates the hurt as #10417e), its hit flinch and knockback (`02C5`) and
/// its "recently hurt" flag (`01CF` rid 171). Assumptions: the amount is
/// the damage dealt; a killing hit sends none (the death message goes
/// instead).
pub fn damage_message(
    source: u32,
    dealt: f64,
    tags: DamageTags,
    direction: Option<Vec3>,
) -> (u64, Variables) {
    (
        DAMAGED,
        Variables::from([
            (DAMAGE_TAGS, tags.to_value()),
            (DAMAGE_SOURCE, Value::Entity(source)),
            (DAMAGE_AMOUNT, Value::Float(dealt)),
            (
                DAMAGE_DIRECTION,
                direction.map_or(Value::Null, Value::Vector),
            ),
        ]),
    )
}

/// The way a hit from `from` to `to` travels, as a unit vector; none when
/// the two points meet. Assumption: the damage message's direction runs
/// from the attacker to the body (`0E8C` and `02C5` turn it against the
/// body's facing for the flinch).
pub fn damage_direction(from: Vec3, to: Vec3) -> Option<Vec3> {
    let offset: Vec3 = std::array::from_fn(|axis| to[axis] - from[axis]);
    let length = offset.iter().map(|part| part * part).sum::<f64>().sqrt();
    (length > f64::EPSILON).then(|| offset.map(|part| part / length))
}

/// Who fired: a player's body or a bot entity, and his team (combat.py
/// `_source_entity` and the shooter's `team`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Shooter {
    pub source: u32,
    pub team: u8,
    pub bot: bool,
}

#[derive(Clone, Copy)]
pub struct PosedHitscan {
    pub source: u32,
    pub origin: [f64; 3],
    pub command: Command,
    /// The command's aim, turned by the volley's spread when it has one
    /// (`weapon_spread::shot_direction`).
    pub direction: [f64; 3],
    pub shot_frame: u32,
    pub reach: f64,
    pub hit: HitDefinition,
    /// The volley's ray profile (`orpheus_script::weapons::ray_profile`).
    pub ray_profile: Option<u64>,
}

pub enum PosedShot {
    Hitscan(PosedHitscan),
    /// Boxed: by far the largest.
    Projectile(Box<PosedProjectile>),
    /// A health change the body's graphs made (a ModifyHealth state, as
    /// quick melee's `0043` st4): [`Combat::modify_health`].
    Health(HealthChange),
}

pub struct PosedProjectile {
    pub shot: ShotEvent,
    pub ballistics: ProjectileBallistics,
    pub origin: [f64; 3],
    pub direction: [f64; 3],
    /// Whether the owner's client shoots it under its body's lease, so the
    /// server confirms and releases it (projectiles.py `fired`: a predicted
    /// volley with a lease id); else only its damage flies (combat.py
    /// `_shoot`: a `Rocket` without a shot).
    pub leased: bool,
    /// A networked volley the owner's client doesn't shoot itself
    /// (`m_45271E8C` 0: Biotic Grenade `0B97` st0), of one projectile that
    /// leaves nothing: the server flies it and shows it to every client,
    /// its shooter's included (`projectiles::ServerCopies`). A bot's never
    /// is; its rockets go to the clients on their own path.
    pub server_shown: bool,
    /// The entity it leaves where it lands (projectiles.py `fired`: the
    /// Biotic Field 03DC, Torbjörn's turret), if any.
    pub spawn: Option<u64>,
    /// The Biotic Field it leaves, when the spawn is the field (boxed: a
    /// rare shot).
    pub field: Option<Box<BioticField>>,
    /// The shooter's feet when it fired (projectiles.py `Shot.where`).
    pub feet: [f64; 3],
    /// The volley state's count of volleys, as the other clients' shot
    /// record names it (projectiles.py `_show_others`: `state.volleys`).
    pub volleys: u8,
    /// The projectiles a shot of the volley fires (`m_numProjectilesPerShot`):
    /// a leased one's owner client shot each under its own id, and each is
    /// confirmed and released.
    pub projectiles: u8,
    /// The directions the server flies the shot's projectiles in for their
    /// damage: one per projectile when the volley lays them out in rings
    /// (`STU_613BDDB0`, `weapon_spread::ring_directions`), else `direction`
    /// alone (combat.py `_shoot` flew one whatever the count). A bot's
    /// flies along `direction` alone (combat.py's NPC path).
    pub pellets: Vec<[f64; 3]>,
}

#[derive(Clone, Copy)]
pub struct ShotContext {
    pub quanta: u32,
    pub tick: u32,
    pub now: Duration,
}

/// A training bot's death (combat.py `_bot_died`): the match runs its death
/// script with these.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BotDeath {
    pub bot: u32,
    pub killer: u32,
    pub critical: bool,
    pub loadout: Option<u64>,
    /// The killing hit's damage tags.
    pub tags: DamageTags,
    pub tick: u32,
}

/// A hit a training bot took and survived: its script's damage message
/// (`0011.025`, [`orpheus_script::bot::DAMAGED`]) carries these.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BotDamage {
    /// Who dealt it.
    pub source: u32,
    pub dealt: f64,
    pub tags: DamageTags,
    /// The world tick of the hit.
    pub tick: u32,
}

pub struct BotHit {
    pub notices: [Option<CombatNotice>; 2],
    pub health: EntityUpdate,
    pub bot: u32,
    /// The bot's pools added up after the hit.
    pub remaining: f64,
    pub death: Option<BotDeath>,
    /// The damage of a hit it survived; none for a heal or a kill.
    pub damage: Option<BotDamage>,
    /// What a heal gave the bot, for the healer's Healing Done and his
    /// ultimate; 0 for a damaging hit.
    pub healed: f64,
}

/// What the environment deals a body it kills: more than any hero's pools
/// (the kill volumes of other maps deal 9000 or 9999,
/// `STUECDamageAreaInstanceData`).
pub const ENVIRONMENT_DAMAGE: f64 = 9999.0;

/// A hit on a player's body, for the match to apply to his body's pools
/// (combat.py `_damage` for a human victim).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlayerHit {
    pub body: u32,
    pub shooter: Shooter,
    pub amount: f64,
    /// Critical as the death message carries it (a head hit of a weapon with
    /// a critical multiplier).
    pub critical: bool,
    pub loadout: Option<u64>,
    /// The 25001 frame: the shot's frame, or the world tick of a flight.
    pub frame: u32,
    /// The world tick of the death if this kills; None: the match's tick.
    pub world_tick: Option<u32>,
    /// An ally's heal of `amount` (combat.py `_heal`), not damage.
    pub heal: bool,
    /// The health change's damage tags, for the body's damage message
    /// ([`damage_message`]).
    pub tags: DamageTags,
    /// The way the damage travels, for the damage message: from the
    /// shooter's feet to the body's, as the match last placed them, or from
    /// his own explosion to his body; none when that isn't known.
    pub direction: Option<Vec3>,
}

#[derive(Clone)]
pub struct BotReplacement {
    pub retired: u32,
    pub created: EntityUpdate,
    /// The bot's stand-in controller's link to the new body (`scene.rs`
    /// `bot_controller_link`); `None` sends none.
    pub link: Option<EntityUpdate>,
}

/// A player's body as a target this tick.
#[derive(Clone, Debug, PartialEq)]
pub struct PlayerTarget {
    pub body: u32,
    pub team: u8,
    pub pose: TargetPose,
    pub dead: bool,
    /// Its pools are below their maximum ([`Vitals::hurt`]): a hit that
    /// strikes hurt allies strikes it.
    pub hurt: bool,
    /// Its body definition's target tags, if the data gives them
    /// ([`PreparedScene::body_tags`]).
    pub tags: Option<Arc<BTreeSet<u64>>>,
}

struct Target {
    vitals: Vitals,
    history: TargetHistory,
    respawn_at: Option<Duration>,
}

impl Target {
    fn new(health: f64, replacement: Option<u32>) -> Self {
        Self {
            vitals: Vitals::new(health, 0.0, 0.0),
            history: TargetHistory::new(replacement),
            respawn_at: None,
        }
    }
}

/// A player's current body and where it was (combat.py `history`, with the
/// replacement tick of `record_replacement`).
struct PlayerSlot {
    target: PlayerTarget,
    history: TargetHistory,
}

/// A bot's projectile in flight (combat.py `Rocket` with `last_tick`).
struct BotRocket {
    id: u64,
    shooter: Shooter,
    flight: ProjectileFlight,
}

/// One slot per map spawner, independent of the network ID of the current bot.
pub struct Combat {
    targets: Vec<Target>,
    players: Vec<PlayerSlot>,
    rockets: Vec<BotRocket>,
    player_hits: Vec<PlayerHit>,
    next_bot: u32,
    /// The last update's tick (combat.py `Combat.tick`).
    tick: u32,
    /// The ids of the projectiles the other clients see and of the Biotic
    /// Fields (projectiles.py `Projectiles.next_seen`, `next_field`).
    seen_ids: SeenIds,
    field_ids: FieldIds,
    /// The last bot rocket's id, and the ids of those that landed since
    /// the match last looked.
    next_rocket: u64,
    landed: Vec<u64>,
    /// The auras the hits put on bodies since the match last looked.
    auras: Vec<AuraHit>,
}

/// An aura a hit put on a body ([`Combat::take_auras`]): the match puts it
/// on the body's script (`vm/auras.rs` in `orpheus_script`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AuraHit {
    pub body: u32,
    /// The aura (`053` GUID).
    pub aura: u64,
    pub instigator: u32,
}

impl Combat {
    pub fn new(scene: &PreparedScene) -> Self {
        Self {
            targets: scene
                .bots()
                .iter()
                .map(|bot| Target::new(bot.spawn.health, None))
                .collect(),
            players: Vec::new(),
            rockets: Vec::new(),
            player_hits: Vec::new(),
            next_bot: BOT_ENTITY + scene.bots().len() as u32,
            tick: 0,
            seen_ids: SeenIds::new(),
            field_ids: FieldIds::default(),
            next_rocket: 0,
            landed: Vec::new(),
            auras: Vec::new(),
        }
    }

    /// combat.py `update`'s retained frames: the first combat frame this
    /// update at `tick` covers, at most COMBAT_CATCH_UP of them after the
    /// last update's.
    pub fn catch_up(&mut self, tick: u32) -> u32 {
        let first = self
            .tick
            .saturating_add(1)
            .max(tick.saturating_sub(COMBAT_CATCH_UP - 1));
        self.tick = tick;
        first
    }

    /// Keep where every body was this tick (combat.py `update`'s history):
    /// the players' current bodies, a new body with a history of its own
    /// that refuses shots aimed before it existed, and the live bots.
    pub fn record_poses(&mut self, scene: &PreparedScene, players: &[PlayerTarget], tick: u32) {
        let mut previous = std::mem::take(&mut self.players);
        self.players = players
            .iter()
            .map(|target| {
                let mut history = previous
                    .iter()
                    .position(|slot| slot.target.body == target.body)
                    .map_or_else(
                        || TargetHistory::new(Some(tick)),
                        |index| previous.swap_remove(index).history,
                    );
                history.record(tick, target.pose.clone());
                PlayerSlot {
                    target: target.clone(),
                    history,
                }
            })
            .collect();
        for (target, bot) in self.targets.iter_mut().zip(scene.bots()) {
            if !bot.dead {
                target.history.record(tick, scene.bot_target(bot));
            }
        }
    }

    /// The bodies the statescript's target searches can find this tick
    /// (`TrackTargets`): the players' bodies as recorded, then the bots, in
    /// slot order; no teams in a free-for-all. A player's body is his
    /// controller's (its id less the low byte), a bot's its stand-in
    /// controller's (`scene::bot_controller_entity`), an AI's (component
    /// 92: the bot's AI definition possesses it).
    pub fn target_candidates(&self, scene: &PreparedScene) -> Vec<TargetCandidate> {
        let free_for_all = scene.map().free_for_all;
        let candidate =
            |entity, team, dead, controller, ai, pose: &TargetPose, tags| TargetCandidate {
                entity,
                team: (!free_for_all).then_some(team),
                dead,
                controller,
                ai,
                feet: pose.feet,
                height: pose.height,
                radius: pose.radius,
                yaw: pose.yaw,
                tags,
            };
        self.players
            .iter()
            .map(|slot| {
                let target = &slot.target;
                let controller = Some(target.body & !0xff);
                candidate(
                    target.body,
                    target.team,
                    target.dead,
                    controller,
                    false,
                    &target.pose,
                    target.tags.clone(),
                )
            })
            .chain(scene.bots().iter().enumerate().map(|(slot, bot)| {
                let controller = bot_controller_entity(slot).ok();
                let pose = scene.bot_target(bot);
                candidate(
                    bot.entity,
                    bot.spawn.team,
                    bot.dead,
                    controller,
                    true,
                    &pose,
                    scene.body_tags(bot.spawn.body),
                )
            }))
            .collect()
    }

    /// The pools of the bot in `slot` now.
    pub fn bot_vitals(&self, slot: usize) -> Option<&Vitals> {
        self.targets.get(slot).map(|target| &target.vitals)
    }

    /// The hits on players' bodies since the last call, in order.
    pub fn take_player_hits(&mut self) -> Vec<PlayerHit> {
        std::mem::take(&mut self.player_hits)
    }

    /// The environment kills the player's body at `tick` (`map_areas`
    /// module notes: it fell onto the map's catch floor): a hit on it of
    /// [`ENVIRONMENT_DAMAGE`] that goes the way of any other, so the
    /// victim gets "YOU DIED" (25004), his body the death message and his
    /// board a death, and he respawns after the mode's time. Assumption:
    /// the body is its own killer, as in a self-inflicted explosion.
    pub fn environment_kill(&mut self, body: u32, team: u8, tick: u32) {
        self.player_hits.push(PlayerHit {
            body,
            shooter: Shooter {
                source: body,
                team,
                bot: false,
            },
            amount: ENVIRONMENT_DAMAGE,
            critical: false,
            loadout: None,
            frame: tick,
            world_tick: Some(tick),
            heal: false,
            tags: DamageTags::NONE,
            direction: None,
        });
    }

    /// The bodies a shooter's hit can strike (combat.py `_enemies`, and
    /// `_shot_targets` for a hit that strikes allies, leafall903/ORPHEUS
    /// (MIT) commit c239a84): other players' living bodies of another team
    /// (any in a free-for-all) and of his team as the hit strikes allies
    /// ([`strikes_ally`]), as `pose` places each. Bots come after them.
    fn player_candidates<'a>(
        &'a self,
        shooter: Shooter,
        free_for_all: bool,
        allies: AllyHits,
        pose: impl Fn(&PlayerSlot) -> Option<TargetPose> + 'a,
    ) -> impl Iterator<Item = (u32, TargetPose)> + 'a {
        self.players.iter().filter_map(move |slot| {
            let target = &slot.target;
            let enemy = free_for_all || target.team != shooter.team;
            (target.body != shooter.source
                && (enemy || strikes_ally(allies, target.hurt))
                && !target.dead)
                .then(|| Some((target.body, pose(slot)?)))
                .flatten()
        })
    }

    pub fn hitscan(
        &mut self,
        scene: &mut PreparedScene,
        shot: PosedHitscan,
        shooter: Shooter,
        context: ShotContext,
        visible: impl Fn(u32) -> bool,
    ) -> io::Result<Option<BotHit>> {
        let view = ViewFrame::from_command(&shot.command, context.quanta);
        let ray = HitscanShot::along(shot.origin, shot.direction, shot.reach, shot.hit);
        self.fire_hitscan(
            scene,
            (&ray, shot.ray_profile),
            shooter,
            view,
            (shot.shot_frame, None),
            context,
            visible,
        )
    }

    /// combat.py `_shoot` for a firing bot's hitscan volley: from `origin`
    /// along `direction`, at the poses of this tick, meeting the map as its
    /// volley's ray profile says.
    #[allow(clippy::too_many_arguments)]
    pub fn bot_hitscan(
        &mut self,
        scene: &mut PreparedScene,
        shooter: Shooter,
        origin: Vec3,
        direction: Vec3,
        reach: f64,
        hit: HitDefinition,
        ray_profile: Option<u64>,
        context: ShotContext,
    ) -> io::Result<Option<BotHit>> {
        let ray = HitscanShot::along(origin, direction, reach, hit);
        self.fire_hitscan(
            scene,
            (&ray, ray_profile),
            shooter,
            ViewFrame::at(context.tick),
            (context.tick, Some(context.tick)),
            context,
            |_| true,
        )
    }

    /// A shot along `ray` meets the map as its volley's ray profile says
    /// ([`PreparedScene::hitscan_wall`]).
    #[allow(clippy::too_many_arguments)]
    fn fire_hitscan(
        &mut self,
        scene: &mut PreparedScene,
        (ray, ray_profile): (&HitscanShot, Option<u64>),
        shooter: Shooter,
        view: ViewFrame,
        (shot_frame, world_tick): (u32, Option<u32>),
        context: ShotContext,
        visible: impl Fn(u32) -> bool,
    ) -> io::Result<Option<BotHit>> {
        let free_for_all = scene.map().free_for_all;
        let allies = ray.hit.allies;
        let players: Vec<_> = self
            .player_candidates(shooter, free_for_all, allies, |slot| {
                slot.history.at(view, slot.target.pose.clone())
            })
            .collect();
        let bots = scene
            .bots()
            .iter()
            .zip(&self.targets)
            .filter_map(|(bot, target)| {
                if !eligible_bot(bot, target, shooter, free_for_all, allies) || !visible(bot.entity)
                {
                    return None;
                }
                target
                    .history
                    .at(view, scene.bot_target(bot))
                    .map(|pose| (bot.entity, pose))
            });
        let Some(impact) = ray
            .resolve(
                players.into_iter().chain(bots),
                |origin, direction, reach| {
                    scene.hitscan_wall(origin, direction, reach, ray_profile)
                },
            )
            .map_err(io::Error::other)?
        else {
            return Ok(None);
        };
        Ok(self.apply_impact(scene, shooter, shot_frame, world_tick, context, &impact))
    }

    /// A health change a body's graph made ([`HealthChange`]: quick melee
    /// `0043` st4 strikes what its search found, Biotic Grenade's auras
    /// heal or damage the body they are on), applied as a shot's impact
    /// is: the damage message, the stats, a death and its respawn follow.
    /// Its instigator is the shooter, a player's body or a bot as the match
    /// last placed them; another instigator changes nothing. The server's
    /// native rule decides who takes it (assumption, as for the shots:
    /// combat.py `_enemies`): damage strikes an enemy of the instigator
    /// (anyone in a free-for-all) and the instigator's own body (the enemy
    /// spawn room `0C8E` st45), a heal an ally or itself. A bot takes it
    /// only once its create reached the instigator's client (`visible`, as
    /// for a shot); the dead take nothing. Unlike the shots (and Python's
    /// `_melee`), quick melee's search saw the bodies where the match placed
    /// them this tick, not as the shooter's client showed them. A change
    /// that carries an aura puts it on the body it struck
    /// ([`Self::take_auras`]).
    pub fn modify_health(
        &mut self,
        scene: &mut PreparedScene,
        change: &HealthChange,
        context: ShotContext,
        visible: impl Fn(u32) -> bool,
    ) -> Option<BotHit> {
        if !(change.amount.is_finite() && change.amount > 0.0) {
            return None;
        }
        let shooter = self.shooter(scene, change.instigator)?;
        let free_for_all = scene.map().free_for_all;
        let target = change.target;
        let aura = change.aura.map(|aura| AuraHit {
            body: target,
            aura,
            instigator: change.instigator,
        });
        if let Some(slot) = self
            .players
            .iter()
            .find(|slot| slot.target.body == target && !slot.target.dead)
        {
            let own = target == shooter.source;
            let teammate = !own && !free_for_all && slot.target.team == shooter.team;
            let takes = if change.heal {
                own || teammate
            } else {
                !teammate
            };
            if !takes {
                return None;
            }
            let direction = (!own)
                .then(|| self.feet(scene, shooter.source))
                .flatten()
                .and_then(|from| damage_direction(from, slot.target.pose.feet));
            self.player_hits.push(PlayerHit {
                body: target,
                shooter,
                amount: change.amount,
                critical: false,
                loadout: change.loadout,
                frame: change.frame,
                world_tick: None,
                heal: change.heal,
                tags: change.tags,
                direction,
            });
            self.auras.extend(aura);
            return None;
        }
        if !visible(target) {
            return None;
        }
        let impact = Impact {
            entity: target,
            distance: 0.0,
            damage: if change.heal { 0.0 } else { change.amount },
            critical: false,
            loadout: change.loadout,
            tags: change.tags,
            heal: if change.heal { change.amount } else { 0.0 },
        };
        let hit = self.apply_bot_impact(scene, shooter, change.frame, None, context, &impact);
        if hit.is_some() {
            self.auras.extend(aura);
        }
        hit
    }

    /// The shooter `entity` is: a player's body as the match last placed
    /// it, or a living bot.
    fn shooter(&self, scene: &PreparedScene, entity: u32) -> Option<Shooter> {
        if let Some(slot) = self.players.iter().find(|slot| slot.target.body == entity) {
            return Some(Shooter {
                source: entity,
                team: slot.target.team,
                bot: false,
            });
        }
        scene
            .bots()
            .iter()
            .find(|bot| bot.entity == entity && !bot.dead)
            .map(|bot| Shooter {
                source: entity,
                team: bot.spawn.team,
                bot: true,
            })
    }

    /// The auras the hits put on bodies since the last call, in order.
    pub fn take_auras(&mut self) -> Vec<AuraHit> {
        std::mem::take(&mut self.auras)
    }

    pub fn step_projectile(
        &mut self,
        scene: &mut PreparedScene,
        shooter: Shooter,
        flight: &mut ProjectileFlight,
        context: ShotContext,
        hits: &mut Vec<BotHit>,
    ) -> io::Result<Option<FlightEnd>> {
        let free_for_all = scene.map().free_for_all;
        let allies = flight.direct().map_or(AllyHits::None, |hit| hit.allies);
        let mut candidates: Vec<_> = self
            .player_candidates(shooter, free_for_all, allies, |slot| {
                Some(slot.target.pose.clone())
            })
            .collect();
        let first_bot = candidates.len();
        candidates.extend(
            scene
                .bots()
                .iter()
                .zip(&self.targets)
                .filter(|(bot, target)| eligible_bot(bot, target, shooter, free_for_all, allies))
                .map(|(bot, _)| (bot.entity, scene.bot_target(bot))),
        );
        let ray_profile = flight.ray_profile();
        let step = flight
            .advance(context.tick, &candidates, |origin, direction, reach| {
                scene.hitscan_wall(origin, direction, reach, ray_profile)
            })
            .map_err(io::Error::other)?;
        hits.extend(step.impacts.iter().filter_map(|impact| {
            self.apply_impact(
                scene,
                shooter,
                context.tick,
                Some(context.tick),
                context,
                impact,
            )
        }));
        // Its explosion pushes the bots it can reach and hurt, as it pushes
        // its shooter (`ProjectileFlight::blast_push`), the live ones after
        // its damage ([`PreparedScene::knock_bot`]): not an ally it heals,
        // nor an invulnerable one, as the shooter takes no push where he
        // takes no damage (`WorldDelivery::can_be_damaged`). Beyond
        // Python, which moved no bot. Other players' bodies get no push yet.
        if step.end.as_ref().is_some_and(|end| end.exploded) {
            for (entity, pose) in &candidates[first_bot..] {
                if !bot_hurtable(scene, *entity, shooter, free_for_all) {
                    continue;
                }
                if let Some(push) = flight.blast_push(pose) {
                    scene.knock_bot(*entity, push, context.tick);
                }
            }
            // Its aura (Biotic Grenade's `0267`) comes on every living body
            // its rings reach, the shooter's allies and himself included;
            // the aura's graph tells ally from enemy.
            let living_players = self
                .players
                .iter()
                .filter(|slot| !slot.target.dead)
                .map(|slot| (slot.target.body, slot.target.pose.clone()));
            let living_bots = scene
                .bots()
                .iter()
                .filter(|bot| !bot.dead)
                .map(|bot| (bot.entity, scene.bot_target(bot)));
            let reached: Vec<_> = living_players
                .chain(living_bots)
                .filter_map(|(body, pose)| {
                    flight.blast_aura(&pose).map(|aura| AuraHit {
                        body,
                        aura,
                        instigator: shooter.source,
                    })
                })
                .collect();
            self.auras.extend(reached);
        }
        // A player's explosion also reaches his own body (beyond Python,
        // `ProjectileFlight::own_blast`): its damage, never a heal, comes
        // after the others' (no ultimate charge for it: `apply_player_hits`),
        // and its push goes back with the end for his mover.
        let own = step
            .end
            .as_ref()
            .filter(|_| !shooter.bot)
            .and_then(|_| {
                self.players
                    .iter()
                    .find(|slot| slot.target.body == shooter.source && !slot.target.dead)
            })
            .and_then(|slot| flight.own_blast(&slot.target.pose));
        if let Some(own) = own.filter(|own| own.damage > 0.0) {
            self.player_hits.push(PlayerHit {
                body: shooter.source,
                shooter,
                amount: own.damage,
                critical: false,
                loadout: own.loadout,
                frame: context.tick,
                world_tick: Some(context.tick),
                heal: false,
                tags: own.tags,
                direction: Some(own.direction),
            });
        }
        Ok(step.end.map(|end| FlightEnd {
            knockback: own.and_then(|own| own.knockback),
            ..end
        }))
    }

    /// combat.py `_shoot` for a firing bot's projectile volley: a rocket
    /// flown by the room from this tick on (`Rocket(..., last_tick=tick)`).
    /// Its id names it when it lands (`take_landed`); none: nothing flies.
    pub fn fire_bot_projectile(
        &mut self,
        shooter: Shooter,
        projectile: PosedProjectile,
        tick: u32,
    ) -> Option<u64> {
        if !projectile.ballistics.damaging() {
            return None;
        }
        let flight = ProjectileFlight::new(
            projectile.ballistics,
            projectile.origin,
            projectile.direction,
            Some(tick),
        )?;
        self.next_rocket += 1;
        let id = self.next_rocket;
        self.rockets.push(BotRocket {
            id,
            shooter,
            flight,
        });
        Some(id)
    }

    /// Fly the bots' rockets for this tick (combat.py `_fly`).
    pub fn step_bot_projectiles(
        &mut self,
        scene: &mut PreparedScene,
        context: ShotContext,
    ) -> io::Result<Vec<BotHit>> {
        let mut hits = Vec::new();
        let mut rockets = std::mem::take(&mut self.rockets);
        let mut failed = None;
        rockets.retain_mut(|rocket| {
            let keep = match self.step_projectile(
                scene,
                rocket.shooter,
                &mut rocket.flight,
                context,
                &mut hits,
            ) {
                Ok(end) => end.is_none(),
                Err(error) => {
                    failed.get_or_insert(error);
                    false
                }
            };
            if !keep {
                self.landed.push(rocket.id);
            }
            keep
        });
        rockets.append(&mut self.rockets);
        self.rockets = rockets;
        failed.map_or(Ok(hits), Err)
    }

    /// combat.py `_damage`: a bot's pools change here; a player's hit waits
    /// for the match. An ally of the shooter (the hits that strike allies
    /// alone reach one) takes the impact's heal instead (combat.py
    /// `_heal`, leafall903/ORPHEUS (MIT) commit c239a84), and no damage.
    fn apply_impact(
        &mut self,
        scene: &mut PreparedScene,
        shooter: Shooter,
        shot_frame: u32,
        world_tick: Option<u32>,
        context: ShotContext,
        impact: &Impact,
    ) -> Option<BotHit> {
        let free_for_all = scene.map().free_for_all;
        if let Some(slot) = self
            .players
            .iter()
            .find(|slot| slot.target.body == impact.entity && !slot.target.dead)
        {
            let heal = !free_for_all && slot.target.team == shooter.team;
            let amount = if heal { impact.heal } else { impact.damage };
            let direction = self
                .feet(scene, shooter.source)
                .and_then(|from| damage_direction(from, slot.target.pose.feet));
            if amount > 0.0 {
                self.player_hits.push(PlayerHit {
                    body: impact.entity,
                    shooter,
                    amount,
                    critical: impact.critical && !heal,
                    loadout: impact.loadout,
                    frame: shot_frame,
                    world_tick,
                    heal,
                    tags: impact.tags,
                    direction,
                });
            }
            return None;
        }
        self.apply_bot_impact(scene, shooter, shot_frame, world_tick, context, impact)
    }

    /// Where a shooter's feet were as the match last placed them: a
    /// player's body as recorded this tick, else a bot.
    fn feet(&self, scene: &PreparedScene, entity: u32) -> Option<Vec3> {
        self.players
            .iter()
            .find(|slot| slot.target.body == entity)
            .map(|slot| slot.target.pose.feet)
            .or_else(|| {
                scene
                    .bots()
                    .iter()
                    .find(|bot| bot.entity == entity)
                    .map(|bot| bot.position)
            })
    }

    fn apply_bot_impact(
        &mut self,
        scene: &mut PreparedScene,
        shooter: Shooter,
        shot_frame: u32,
        world_tick: Option<u32>,
        context: ShotContext,
        impact: &Impact,
    ) -> Option<BotHit> {
        // A Training Bot is back after the mode's respawn time, as a hero
        // (`game_respawns`: 3 s in the practice modes).
        let respawn = scene.respawn_time();
        let free_for_all = scene.map().free_for_all;
        // Its statuses and modifiers (`vm/statuses.rs`): an invulnerable
        // bot takes no damage, and its healing modifiers scale a heal
        // (Biotic Grenade's anti-heal and heal boost).
        let effects = scene.bot_scripts().effects(impact.entity);
        let invulnerable = effects.is_some_and(|effects| effects.invulnerable());
        let healing = effects.map_or(1.0, |effects| effects.healing_scale(&impact.tags));
        let plate = Arc::clone(scene.bot_plate());
        let (bot, target) = scene
            .bots_mut()
            .iter_mut()
            .zip(&mut self.targets)
            .find(|(bot, _)| !bot.dead && bot.entity == impact.entity)?;
        if !free_for_all && bot.spawn.team == shooter.team {
            let given = target.vitals.heal(impact.heal * healing);
            return (given > 0.0).then(|| healed_bot(bot, target, given, &plate));
        }
        if invulnerable {
            return None;
        }
        let death_tick = world_tick.unwrap_or(context.tick);
        let result = target.vitals.damage(
            impact.damage,
            Death {
                time: context.now.as_secs_f64(),
                frame: death_tick,
                killer: shooter.source,
            },
        );
        if result.dealt == 0.0 {
            return None;
        }
        let mut components = vec![plate.pools(&target.vitals)];
        if result.killed {
            bot.died();
            target.respawn_at = Some(context.now.saturating_add(respawn));
            // Its filter bits say it is dead (`scene` module notes); its
            // team was checked at its create.
            components.extend(bot_filter(bot).ok());
        }
        let health = EntityUpdate {
            entity: bot.entity,
            operation: Some(Operation::Update(components)),
            ..EntityUpdate::default()
        };
        target.vitals.clear_changed();
        // A bot's hit tells no one (combat.py `_damage`: only a player
        // shooter gets 25001 and 25005).
        let player = !shooter.bot;
        Some(BotHit {
            notices: [
                player.then_some(CombatNotice::Hit {
                    target: bot.entity,
                    dealt: result.dealt,
                    critical: impact.critical,
                    killed: result.killed,
                    shot_frame,
                }),
                (player && result.killed).then_some(CombatNotice::Elimination {
                    killer: shooter.source,
                    victim: bot.entity,
                }),
            ],
            health,
            bot: bot.entity,
            remaining: target.vitals.total(),
            healed: 0.0,
            death: result.killed.then_some(BotDeath {
                bot: bot.entity,
                killer: shooter.source,
                critical: impact.critical,
                loadout: impact.loadout,
                tags: impact.tags,
                tick: death_tick,
            }),
            damage: (!result.killed).then_some(BotDamage {
                source: shooter.source,
                dealt: result.dealt,
                tags: impact.tags,
                tick: death_tick,
            }),
        })
    }

    pub fn respawn_due(
        &mut self,
        scene: &mut PreparedScene,
        tick: u32,
        now: Duration,
    ) -> io::Result<Vec<BotReplacement>> {
        let mut prepared = Vec::new();
        let mut next_bot = self.next_bot;
        for (slot, (bot, target)) in scene.bots().iter().zip(&self.targets).enumerate() {
            if !target.respawn_at.is_some_and(|deadline| deadline <= now) {
                continue;
            }
            let fresh = bot.respawned(next_bot, tick.saturating_add(BOT_RECORD_DELAY));
            next_bot = next_bot
                .checked_add(1)
                .ok_or_else(|| io::Error::other("bot entity IDs exhausted"))?;
            let created = bot_create(&fresh, scene.bot_plate())?;
            let link = Some(bot_controller_link(&fresh, slot)?);
            prepared.push((
                slot,
                fresh,
                BotReplacement {
                    retired: bot.entity,
                    created,
                    link,
                },
            ));
        }
        self.next_bot = next_bot;
        Ok(prepared
            .into_iter()
            .map(|(slot, fresh, replacement)| {
                self.targets[slot] = Target::new(fresh.spawn.health, Some(tick));
                scene.bots_mut()[slot] = fresh;
                replacement
            })
            .collect())
    }

    /// combat.py `heal_area`: the healer's own body and his teammates'
    /// (only his own in a free-for-all), players' and bots' alike, whose
    /// capsule comes within `radius` of `centre` (unverified: to the
    /// capsule; a bot keeps its capsule here though shots meet its hit
    /// shapes). The players' bodies come back for the match to heal; the
    /// bots heal here, each hit with what it took.
    pub fn heal_area(
        &mut self,
        scene: &mut PreparedScene,
        healer: Shooter,
        centre: Vec3,
        radius: f64,
        amount: f64,
    ) -> (Vec<u32>, Vec<BotHit>) {
        let free_for_all = scene.map().free_for_all;
        let reaches = |pose: &TargetPose| {
            let [a, b] = pose.segment();
            segment_distance(centre, a, b) - pose.radius <= radius
        };
        let players = self
            .players
            .iter()
            .map(|slot| &slot.target)
            .filter(|target| {
                let own = target.body & !0xff == healer.source & !0xff;
                let ally = !free_for_all && target.team == healer.team;
                (own || ally) && !target.dead && reaches(&target.pose)
            })
            .map(|target| target.body)
            .collect();
        let mut bots = Vec::new();
        for slot in 0..self.targets.len() {
            let bot = &scene.bots()[slot];
            let own = bot.entity == healer.source;
            let ally = !free_for_all && bot.spawn.team == healer.team;
            if bot.dead || !(own || ally) || !reaches(&scene.bot_target(bot)) {
                continue;
            }
            let target = &mut self.targets[slot];
            let given = target.vitals.heal(amount);
            if given > 0.0 {
                bots.push(healed_bot(bot, target, given, scene.bot_plate()));
            }
        }
        (players, bots)
    }
}

/// A bot's heal of `given`: its new pools for every client, and no notice
/// (combat.py `_heal` sends none).
fn healed_bot(bot: &Bot, target: &mut Target, given: f64, plate: &BotPlate) -> BotHit {
    let health = EntityUpdate {
        entity: bot.entity,
        operation: Some(Operation::Update(vec![plate.pools(&target.vitals)])),
        ..EntityUpdate::default()
    };
    target.vitals.clear_changed();
    BotHit {
        notices: [None, None],
        health,
        bot: bot.entity,
        remaining: target.vitals.total(),
        death: None,
        healed: given,
        damage: None,
    }
}

/// One player of a room for the match's side of combat: his controller
/// entity, his peer's world if his game is connected, and his board.
pub struct CombatPlayer<'w> {
    pub entity: u32,
    pub world: Option<&'w mut WorldDelivery>,
    pub stats: &'w mut PlayerStats,
}

fn player_of(players: &[CombatPlayer<'_>], body: u32) -> Option<usize> {
    players
        .iter()
        .position(|player| player.entity == body & !0xff)
}

/// The hits on players' bodies (combat.py `_damage`, `_tell_hit`,
/// `_died`, `_kill_notice`): the victim's pools take each; a player who hit
/// another gets 25001, Hero Damage Done and ultimate charge for it (his own
/// explosion's damage to himself gives none of them: beyond Python, which
/// never hurt the shooter; self damage gives no charge, game-rules.md 154,
/// and isn't damage done or a hit to show, assumption); a kill counts
/// Deaths for the victim, Eliminations and 25005 for another player who
/// killed him (the scene's board stats), and gives the victim 25004 ("YOU
/// WERE KILLED BY", or "YOU DIED" for his own blow), and the killer's
/// multikills count it ([`crate::announcer`]). A heal of an ally
/// (combat.py `_heal`, leafall903/ORPHEUS (MIT) commit c239a84) tells no
/// one: a player healer's Healing Done and ultimate count what the pools
/// took.
/// Returns each player's notices, by slot.
pub fn apply_player_hits(
    scene: &PreparedScene,
    hits: Vec<PlayerHit>,
    players: &mut [CombatPlayer<'_>],
    tick: u32,
    now: Duration,
) -> Vec<Vec<CombatNotice>> {
    let board = *scene.board_stats();
    let mut notices = vec![Vec::new(); players.len()];
    for hit in hits {
        let Some(victim) = players.iter().position(|player| {
            player
                .world
                .as_deref()
                .is_some_and(|world| world.current_body() == Some(hit.body))
        }) else {
            continue;
        };
        if hit.heal {
            let given = players[victim].world.as_deref_mut().map_or(0.0, |world| {
                world.heal_body(scene, hit.body, hit.amount, &hit.tags)
            });
            if !hit.shooter.bot {
                credit_healing(scene, players, hit.shooter.source, given);
            }
            continue;
        }
        let Some(damage) = players[victim]
            .world
            .as_deref_mut()
            .and_then(|world| world.damage_body(scene, &hit, tick, now))
        else {
            continue;
        };
        let shooter = (!hit.shooter.bot)
            .then(|| player_of(players, hit.shooter.source))
            .flatten();
        if let Some(shooter) = shooter.filter(|&shooter| shooter != victim) {
            notices[shooter].push(CombatNotice::Hit {
                target: hit.body,
                dealt: damage.dealt,
                critical: hit.critical,
                killed: damage.killed,
                shot_frame: hit.frame,
            });
            players[shooter].stats.add(board.hero_damage, damage.dealt);
            if let Some(world) = players[shooter].world.as_deref_mut() {
                world.gain_ult(damage.dealt);
            }
        }
        if !damage.killed {
            continue;
        }
        players[victim].stats.add(board.deaths, 1.0);
        let own_blow = shooter == Some(victim);
        if let Some(killer) = shooter.filter(|_| !own_blow) {
            players[killer].stats.add(board.eliminations, 1.0);
            let victim_body = players[victim]
                .world
                .as_deref()
                .and_then(|world| world.current_hero())
                .and_then(|hero| scene.heroes().get(hero))
                .map(|hero| hero.body);
            if let Some(world) = players[killer].world.as_deref_mut() {
                world.eliminated(
                    &scene.shared_graphs(),
                    tick,
                    hit.body,
                    victim_body,
                    &hit.tags,
                );
            }
            notices[killer].push(CombatNotice::Elimination {
                killer: hit.shooter.source,
                victim: hit.body,
            });
        }
        notices[victim].push(CombatNotice::Death {
            source: hit.shooter.source,
            killed_by: !own_blow,
        });
    }
    notices
}

/// combat.py `heal_area` for players' bodies: each heals, and the healer's
/// Healing Done counts what they took, with what the bots took (`bots_healed`);
/// his ultimate charges for it, his own healing included (unverified).
pub fn heal_players(
    scene: &PreparedScene,
    healer: u32,
    bodies: &[u32],
    amount: f64,
    bots_healed: f64,
    players: &mut [CombatPlayer<'_>],
) {
    let mut healed = bots_healed;
    for &body in bodies {
        for player in players.iter_mut() {
            if let Some(world) = player.world.as_deref_mut() {
                if world.current_body() == Some(body) {
                    healed += world.heal_body(scene, body, amount, &DamageTags::NONE);
                }
            }
        }
    }
    credit_healing(scene, players, healer, healed);
}

/// The healer's Healing Done counts `healed` and his ultimate charges for
/// it (combat.py `_heal`), when he is a player of the room.
fn credit_healing(
    scene: &PreparedScene,
    players: &mut [CombatPlayer<'_>],
    healer: u32,
    healed: f64,
) {
    if let Some(healer) = player_of(players, healer).filter(|_| healed > 0.0) {
        players[healer]
            .stats
            .add(scene.board_stats().healing, healed);
        if let Some(world) = players[healer].world.as_deref_mut() {
            world.gain_ult(healed);
        }
    }
}

/// match.py `queue_body_message` and `deliver_body_messages`: the messages
/// the players' bodies sent go to the bodies they name in the room, at most
/// MAX_BODY_MESSAGES a tick; a message to a body nobody plays takes no place
/// in the queue. A message to every entity ([`EVERY_ENTITY`]: `0686`'s
/// `0307` after Need Healing) goes to every other player's body; the
/// sender's own instances took it when it was sent. A message a body sends
/// while it takes one goes in the next batch.
pub fn deliver_body_messages(worlds: &mut [&mut WorldDelivery]) {
    let mut messages: Vec<_> = worlds
        .iter_mut()
        .flat_map(|world| world.take_body_messages())
        .collect();
    messages.retain(|message| {
        message.target == EVERY_ENTITY
            || worlds
                .iter()
                .any(|world| world.takes_body_message(message.target))
    });
    if messages.len() > MAX_BODY_MESSAGES {
        eprintln!(
            "match body message queue reached {MAX_BODY_MESSAGES}; {} dropped",
            messages.len() - MAX_BODY_MESSAGES
        );
    }
    for message in messages.into_iter().take(MAX_BODY_MESSAGES) {
        if message.target == EVERY_ENTITY {
            for world in worlds.iter_mut() {
                if let Some(body) = world.current_body().filter(|&body| body != message.sender) {
                    world.receive_body_message(&GameMessage {
                        target: body,
                        ..message.clone()
                    });
                }
            }
            continue;
        }
        for world in worlds.iter_mut() {
            if world.receive_body_message(&message) {
                break;
            }
        }
    }
}

/// A Biotic Field's heal this tick (projectiles.py `_field`: `heal_area`):
/// the healer's index among the room's worlds, his body, the heal sphere's
/// centre and radius, and the amount.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FieldHeal {
    pub slot: usize,
    pub source: u32,
    pub centre: Vec3,
    pub radius: f64,
    pub amount: f64,
}

/// projectiles.py `_show_others` for the projectiles a player fired: every
/// other client of the room that has his body's statescript gets its own
/// copy, numbered from the room's `next_seen`, in slot order. `worlds` are
/// the room's, by slot.
pub fn show_fired(
    combat: &mut Combat,
    fired: &[FiredProjectile],
    worlds: &mut [Option<&mut WorldDelivery>],
    tick: u32,
) {
    for projectile in fired {
        for world in worlds.iter_mut().flatten() {
            world.show_projectile(projectile, tick, || combat.seen_ids.next());
        }
    }
}

/// projectiles.py `_show_others` for the firing bots' rockets: every client
/// of the room that has the bot's statescript stream gets its own copy,
/// numbered from the room's `next_seen`, in slot order.
pub fn show_bot_rockets(
    combat: &mut Combat,
    rockets: &[SeenRocket],
    worlds: &mut [Option<&mut WorldDelivery>],
    tick: u32,
) {
    for rocket in rockets {
        for world in worlds.iter_mut().flatten() {
            world.show_bot_rocket(rocket, tick, || combat.seen_ids.next());
        }
    }
}

/// projectiles.py `flown` for the bots' rockets that landed this tick:
/// their copies on every client of the room finish.
pub fn finish_bot_rockets(
    combat: &mut Combat,
    worlds: &mut [Option<&mut WorldDelivery>],
    tick: u32,
) {
    for rocket in std::mem::take(&mut combat.landed) {
        for world in worlds.iter_mut().flatten() {
            world.finish_bot_rocket(rocket, tick);
        }
    }
}

/// projectiles.py `Projectiles.update` for a room, once every player's
/// shots stepped (`WorldDelivery::advance_projectiles`), by slot: an ended
/// shot finishes its copies on the other clients (`_end`); a field's heal
/// comes back for the match; its frames go again when it fades; every
/// client in the world gets its entity once (`_deliver_field`: the owner's
/// parented to each of his canisters that arrived, the others' at the
/// shooter's feet); the fields whose shot no longer runs go (`_end`); then
/// each client's copies of the others' projectiles advance
/// (`_ObservedProjectile.advance`).
pub fn deliver_projectiles(
    scene: &PreparedScene,
    combat: &mut Combat,
    worlds: &mut [Option<&mut WorldDelivery>],
    tick: u32,
) -> Vec<FieldHeal> {
    let live: Vec<ShotKey> = worlds
        .iter()
        .flatten()
        .flat_map(|world| world.live_fields())
        .collect();
    let mut events = Vec::new();
    for (slot, world) in worlds.iter_mut().enumerate() {
        if let Some(world) = world {
            let (owner, skin) = (world.owner(), world.skin());
            events.extend(
                world
                    .take_projectile_events()
                    .into_iter()
                    .map(|(body, event)| (slot, owner, skin, body, event)),
            );
        }
    }
    let mut heals = Vec::new();
    for (slot, owner, skin, body, event) in events {
        match event {
            ProjectileEvent::Ended { token, explode } => {
                for world in worlds.iter_mut().flatten() {
                    world.finish_projectile(body, token, tick, explode);
                }
            }
            ProjectileEvent::ServerEnded { token, exploded } => {
                for world in worlds.iter_mut().flatten() {
                    world.finish_server_projectile(body, token, tick, exploded);
                }
            }
            ProjectileEvent::FieldHeal {
                centre,
                radius,
                amount,
                ..
            } => heals.push(FieldHeal {
                slot,
                source: body.entity,
                centre,
                radius,
                amount,
            }),
            ProjectileEvent::FieldFaded { token } => {
                for world in worlds.iter_mut().flatten() {
                    world.fade_field((body, token));
                }
            }
            ProjectileEvent::FieldDeliver {
                token,
                feet,
                parents,
                faded,
            } => {
                let Some(frames) = scene.field_frames() else {
                    continue;
                };
                let canisters: Vec<_> = parents.into_iter().map(Some).collect();
                for world in worlds.iter_mut().flatten() {
                    let parents: &[Option<u32>] = if world.owner() == owner {
                        &canisters
                    } else {
                        &[None]
                    };
                    let field = FieldDeliver {
                        shot: (body, token),
                        parents,
                        feet,
                        skin,
                        faded,
                        frames,
                    };
                    world.deliver_field(&field, &mut combat.field_ids);
                }
            }
        }
    }
    for world in worlds.iter_mut().flatten() {
        world.end_fields(&live);
        world.advance_observed(tick);
    }
    heals
}

/// A living bot other than the shooter, of another team (any in a
/// free-for-all) or of his as the hit strikes allies.
fn eligible_bot(
    bot: &Bot,
    target: &Target,
    shooter: Shooter,
    free_for_all: bool,
    allies: AllyHits,
) -> bool {
    !bot.dead
        && bot.entity != shooter.source
        && (free_for_all
            || bot.spawn.team != shooter.team
            || strikes_ally(allies, target.vitals.hurt()))
}

/// Whether `shooter`'s hit can hurt the bot `entity` (`apply_bot_impact`'s
/// gate): alive, an enemy (anyone in a free-for-all) and holding no
/// invulnerable status.
fn bot_hurtable(scene: &PreparedScene, entity: u32, shooter: Shooter, free_for_all: bool) -> bool {
    scene.bots().iter().any(|bot| {
        bot.entity == entity && !bot.dead && (free_for_all || bot.spawn.team != shooter.team)
    }) && !scene
        .bot_scripts()
        .effects(entity)
        .is_some_and(|effects| effects.invulnerable())
}

/// Whether a hit that strikes `allies` strikes an ally that is `hurt` or
/// not (combat.py `_shot_targets`: the living allies below their maximum,
/// as the Biotic Rifle's filter asks).
fn strikes_ally(allies: AllyHits, hurt: bool) -> bool {
    match allies {
        AllyHits::None => false,
        AllyHits::Hurt => hurt,
        AllyHits::All => true,
    }
}

#[cfg(test)]
mod tests;
