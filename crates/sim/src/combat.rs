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
    bot_script::SeenRocket,
    bots::{BOT_ENTITY, BOT_RECORD_DELAY, Bot},
    hitscan::{HitscanShot, Impact, TargetHistory, TargetPose, ViewFrame},
    projectile_flight::{FlightEnd, ProjectileFlight},
    projectiles::{
        FIELD_ENTITY, FieldDeliver, FieldIds, FiredProjectile, ProjectileEvent, SeenIds, ShotKey,
    },
    replication::WorldDelivery,
    scene::{bot_controller_entity, bot_controller_link, bot_create, bot_filter},
    scene_assets::PreparedScene,
    spawned::{ServerFlight, SpawnedEntities},
};
use lucy_content::{heroes::Eyes, stats::PlayerStats};
use lucy_core::guid::parse_guid;
use lucy_physics::static_ray::Vec3;
use lucy_script::{
    ai_definitions::AiControl,
    bot::{DAMAGE_AMOUNT, DAMAGE_DIRECTION, DAMAGE_SOURCE, DAMAGE_TAGS, DAMAGED, DIED},
    ultimate::Dealt,
    vitals::{Death, Vitals},
    vm::{
        ASSIST_DEFENSIVE, ASSIST_OFFENSIVE, AssistLink, AuraChange, BodyEffects, CombatMod,
        DAMAGE_DEALT, DAMAGE_TAKEN, GameMessage, HEALING_DEALT, HEALING_TAKEN, HEALTH_PACK,
        Hardpoints, HealthChange, SeenBody, ShotEvent, TargetCandidate, TrackedStat, combat_scale,
    },
    weapons::{AllyHits, DamageTags, HitDefinition, ProjectileBallistics},
    wire::{Value, Variables},
};
use lucy_wire::{
    combat_messages::CombatNotice,
    commands::Command,
    entity::{EntityUpdate, GAME_MODE_ENTITY, Operation},
};

/// Stand-in respawn wait in a mode whose graph sends no respawn time
/// (combat.py `RESPAWN`), for a body no mode body script counts the respawn
/// of (`crate::replication::WorldDelivery::respawn_due`); [`RespawnTimes`]
/// holds the time of every mode whose graph sends one.
pub const RESPAWN: Duration = Duration::from_secs(10);
/// Between respawn attempts while every body id of the player is still
/// reserved (combat.py `RESPAWN_RETRY`).
pub const RESPAWN_RETRY: Duration = Duration::from_millis(250);
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

/// `003E`'s death camera parameters, which only the death layer `01CB`
/// (rid5) reads: the killer's controller `07A9` (#21709), whether the
/// death takes a set camera `0234` (#4608e; `01D8`'s Play of the Game
/// scoring skips such a death), that camera's point `0233` (#4609) and
/// whether it frames the body instead `0232` (#4610)
/// ([`DeathCamera`]).
const KILLER_CONTROLLER: u32 = 0x07a9;
const SET_CAMERA: u32 = 0x0234;
const SET_CAMERA_POINT: u32 = 0x0233;
const SET_CAMERA_ON_BODY: u32 = 0x0232;

/// What a player's body's death layer `01CB` needs of its death besides
/// the kill feed's fields (`refs/reports/DeathCam.md`, "Day (2026-11-19),
/// DayDeathParams").
///
/// - `07A9` is the possessor of the killing body (its controller; an AI
///   hero's AI entity), as the graphs that set #21709 themselves read it
///   (`STU_F7262B36(#40)`: `0C72`, `0B99`, `0EE1`) and as the client's
///   `STU_00707E1E` takes it (it reads the entity's component 44, which
///   only a possessor has). `01CB` frames that controller's possessed body
///   when the killer's body is gone (#21711, client-only st115/st120/st123)
///   and tells it of the kill (rid224 `17D9` to `STU_0AC15F78(#21709)`).
///   A self-kill (an environment kill too) names the body's own. 0: the
///   world lists no such body.
/// - `0234` false: no graph and no client code sends `003E` (`owsearch`:
///   no use of `003E.025` or of the four parameter GUIDs), so which deaths
///   retail gave the set camera (`01CB` st13: st14 at the body + 1 m, or
///   st30 20 m above `0233`) is unknown; a kill and a self-kill take
///   `01CB`'s own cameras (st36 at the killer, st14/st25 at the body).
/// - `0233` the body's position at the death [assumption: the point st30
///   looks down at], and `0232` false; both unread while `0234` is false.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DeathCamera {
    pub killer_controller: u32,
    pub position: Vec3,
}

impl DeathCamera {
    fn parameters(self) -> [(u32, Value); 4] {
        [
            (KILLER_CONTROLLER, Value::Entity(self.killer_controller)),
            (SET_CAMERA, Value::Bool(false)),
            (SET_CAMERA_POINT, Value::Vector(self.position)),
            (SET_CAMERA_ON_BODY, Value::Bool(false)),
        ]
    }
}

/// Each mode's respawn time for heroes (combat.py `respawn_seconds`): the
/// seconds its mode graph sends the player in 06FD.025, derived into
/// `game_respawns_174.json` (0F4A and 0EC2 send 10, the Practice Range's
/// 13C0 sends 3). Every body takes them in `06FD` for its `0C8E`, which
/// counts the respawn; only the stand-ins wait them out in Rust: the
/// Training Bots, and bodies without a counting `0C8E`.
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
/// eyes.
#[derive(Clone, Debug)]
pub struct BodyCombat {
    pub eyes: Eyes,
}

fn invalid(error: impl std::fmt::Display) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error.to_string())
}

/// The "died" message a dead body's script takes (combat.py
/// `_death_parameters`): the killer, the body credited with the kill,
/// whether the blow was critical, its loadout, its damage tags and the
/// assists: the bodies of the players who earned an assist on the kill
/// ([`kill_credit`]). The dead body's `01CF` (a bot's `0C18`) shares #392,
/// #40, #510, #1821, #9300 and #9296 with every client, whose client-only
/// plug posts the kill feed line (`0765.025`) from them: killer `03E9`, icon
/// `03EB` (the loadout), headshot `03EF` (crit), assists `03EC` (`0DEA`
/// rid50 draws each one's hero icon; `01CF` rid55 sends none in a
/// free-for-all, team 4) and `08AF` (the tags hold `4FFB`). Python and
/// upstream sent neither 00BD nor 003D, and no assists. Assumption: the
/// credited body is the killer itself (no turret or pet kills anyone
/// here). Unsent and unknown: 006B, 0142 and 03E8 (`03ED`, 0DEA #9297).
/// A player's body's message carries its death layer's parameters too
/// (`camera`, [`DeathCamera`]); a bot's `0C18` reads none of them.
pub fn death_message(
    killer: u32,
    critical: bool,
    loadout: Option<u64>,
    tags: DamageTags,
    assists: &[u32],
    camera: Option<DeathCamera>,
) -> (u64, Variables) {
    let mut parameters = Variables::from([
        (KILLER, Value::Entity(killer)),
        (CREDITED, Value::Entity(killer)),
        (DAMAGE_TAGS, tags.to_value()),
        (CRIT, Value::Bool(critical)),
        (LOADOUT, loadout.map_or(Value::Null, Value::Asset)),
        (
            ASSISTS,
            Value::Array(assists.iter().copied().map(Value::Entity).collect()),
        ),
    ]);
    parameters.extend(camera.into_iter().flat_map(DeathCamera::parameters));
    (DIED, parameters)
}

/// The damage message (`0011.025`, [`DAMAGED`]) a body's script takes for a
/// hit: the damage's tags, who dealt it, how much and the way it travels
/// ([`damage_direction`]). Neither Python nor upstream sends it; no graph
/// does either, so the server does, as for the death message. The
/// Training Bot's flinch and pain graphs take it (`0E8C`, `0C17`), and
/// every hero body's: its pain line (`01CA` or `14B1`, after `004B` rid 63
/// rates the hurt as #10417e), its hit flinch and knockback (`02C5`) and
/// its "recently hurt" flag (`01CF` rid 171). The clients see what the
/// graphs keep of it in their shared variables: the attacker (`01CF`
/// #392), the way (`02C5` #1018), the amount (`08B6` #476) and the tags
/// (`1688` #510). A killing hit's goes before the death message, with the
/// kill's fields ([`hit_messages`]). Assumption: the amount is the damage
/// dealt.
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

/// The messages a player's body takes for `hit`, with `tags` (the hit's,
/// plus any its damage modifiers added, `vm/statuses.rs`), which dealt
/// `dealt` and, for a killing hit, the assisters' bodies (`kill`; none:
/// the body lives; [`WorldDelivery::send_hit_messages`]), in order: its
/// damage message ([`damage_message`]), then for a killing hit its death
/// message ([`death_message`], with the death layer's `camera`). A
/// killing hit's damage message also carries the kill's fields (the
/// credited body `00BD`, crit `0095`, loadout `03E6`, assists `03E7`), as
/// retail's damage event does: Echo's `1544` rid54 reads them from
/// `0011.025` alone, not `003E.025`, and posts the kill feed line
/// (`0765.025`) from them when a native flag (#33688e, no graph writes
/// it) marks the blow as lethal (`refs/reports/PlayerHurt.md`, "Killing
/// hit").
pub fn hit_messages(
    hit: &PlayerHit,
    tags: DamageTags,
    dealt: f64,
    kill: Option<&[u32]>,
    camera: Option<DeathCamera>,
) -> Vec<(u64, Variables)> {
    let source = hit.shooter.source;
    let death = |tags, camera| {
        kill.map(|assists| death_message(source, hit.critical, hit.loadout, tags, assists, camera))
    };
    let (message, mut parameters) = damage_message(source, dealt, tags, hit.direction);
    parameters.extend(death(tags, None).into_iter().flat_map(|(_, kill)| kill));
    let mut messages = vec![(message, parameters)];
    messages.extend(death(tags, camera));
    messages
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
    /// The volley's ray profile (`lucy_script::weapons::ray_profile`).
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
    /// It leaves the Biotic Field (the spawn is `03DC`; its own shot
    /// keeps the field until the field's graph ends it).
    pub field: bool,
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
    /// What the killing hit dealt, for its damage message (`0011.025`).
    pub dealt: f64,
    pub tick: u32,
}

/// A hit a training bot took and survived: its script's damage message
/// (`0011.025`, [`lucy_script::bot::DAMAGED`]) carries these.
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
    /// The health change's tags (the hit's, or the heal's), for the
    /// shooter's damage dealt or healing done (`lucy_script::ultimate`).
    pub tags: DamageTags,
    /// The health change's source besides the shooter
    /// ([`HealthChange::source`]: a Biotic Orb), which takes the damage
    /// dealt or healing done too.
    pub source: Option<u32>,
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
    /// The environment's kill ([`Combat::environment_kill`]): the match
    /// credits it to whoever last knocked the body ([`KnockCredit`]).
    pub environment: bool,
    /// Where the damage came from, for the victim's damage direction arc
    /// (25000, [`CombatNotice::Hurt`]): another's feet as the match last
    /// placed them; none for his own explosion (assumption: one's own
    /// blow shows no arc) and when that isn't known.
    pub from: Option<Vec3>,
    /// The health change's source besides the shooter
    /// ([`HealthChange::source`]: a Biotic Orb), which takes the damage
    /// dealt or healing done too.
    pub source: Option<u32>,
    /// A health pack's heal (the change of `0046` st0, [`HEALTH_PACK`]):
    /// the Workshop's Event Was Health Pack.
    pub health_pack: bool,
}

/// An explosion's push on another player's body ([`Combat::take_player_pushes`]),
/// for the match to hand to his mover ([`WorldDelivery::knock`]): the
/// victim's client predicts none (it runs the explosion-ring knockback
/// `0x7FF789A433F0` only on its own body, for an explosion its own body's
/// script made: `0x7FF789A3BB10`), so it learns the push from the owner
/// corrections, as the shooter learns the part of his own push his client
/// didn't predict.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlayerPush {
    pub body: u32,
    /// Metres a second added to the body's velocity.
    pub push: Vec3,
    /// Whose explosion it was: the knock an environment kill credits
    /// ([`KnockCredit`]).
    pub by: Shooter,
    /// The ability that pushed (the Workshop's Event Ability).
    pub loadout: Option<u64>,
}

/// What a hit or a push did between the room's players
/// ([`apply_player_hits`], [`apply_player_pushes`]), for the Workshop's
/// player events (`crate::workshop::WorkshopRun::player_events`). Each
/// player is his entity ([`CombatPlayer::entity`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PlayerEvent {
    /// `dealt` damage to `victim`'s body. `attacker`: the player whose hit
    /// it was, the one an environment kill is credited to
    /// ([`KnockCredit`]), or the victim himself for his own blow and a fall
    /// nobody knocked him into ([`Combat::environment_kill`]); none for a
    /// bot's or a placeable's. `direction`: [`PlayerHit::direction`].
    /// `loadout`: the ability that dealt it ([`PlayerHit::loadout`]), for
    /// the Workshop's Event Ability.
    Damage {
        attacker: Option<u32>,
        victim: u32,
        dealt: f64,
        critical: bool,
        environment: bool,
        direction: Option<Vec3>,
        killed: bool,
        loadout: Option<u64>,
    },
    /// `victim`'s death counts as `player`'s elimination: his final blow,
    /// or his share of the kill ([`kill_credit`]).
    Elimination {
        player: u32,
        victim: u32,
        final_blow: bool,
    },
    /// `healee`'s pools took `healed` (above 0) of a heal. `healer`: the
    /// player whose heal it was; none for a bot's or a placeable's (a
    /// health pack's, `health_pack`). `loadout`: the healing ability.
    Heal {
        healer: Option<u32>,
        healee: u32,
        healed: f64,
        health_pack: bool,
        loadout: Option<u64>,
    },
    /// `attacker`'s explosion pushed `victim`'s body by `push` (metres a
    /// second), with the ability `loadout`.
    Knockback {
        attacker: u32,
        victim: u32,
        push: Vec3,
        loadout: Option<u64>,
    },
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
    /// Its pools' health and their maximum, every kind together; none:
    /// unknown. A hit that strikes hurt allies strikes it while hurt
    /// ([`Self::hurt`]); the graphs' searches read it
    /// ([`TargetCandidate::health`]).
    pub health: Option<[f64; 2]>,
    /// Its body definition's target tags, if the data gives them
    /// ([`PreparedScene::body_tags`]).
    pub tags: Option<Arc<BTreeSet<u64>>>,
    /// Its body definition's model hardpoints, if the data gives them
    /// ([`PreparedScene::body_hardpoints`]).
    pub hardpoints: Option<Arc<Hardpoints>>,
    /// An AI hero's AI entity, which possesses the body; none for a
    /// human's.
    pub ai: Option<AiControl>,
    /// The modifiers of the damage and healing the body deals that its
    /// graphs hold now (`lucy_script::vm::statuses`: Nano Boost's
    /// `0B9F` st19), for its hits.
    pub dealt: Vec<CombatMod>,
}

impl PlayerTarget {
    /// Its pools are below their maximum, as a statescript expression
    /// compares them ([`Vitals::hurt`]).
    pub fn hurt(&self) -> bool {
        self.health
            .is_some_and(|[now, most]| now < most - lucy_script::expr::EPSILON)
    }
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

/// Where a player's client showed the bodies (`TrackTargets` `m_1C2A9945`,
/// `lucy_script::vm::World::seen_bodies`): rewound to a command's view
/// by the history the server's shots rewind with ([`TargetHistory::at`]).
/// Assumption: a search sees them at the view of the command its script
/// runs, as that command's shots do (the client's view time for a search,
/// `0x7FF789CBB340`/`0x7FF789CC1AA0`, was not read past the player
/// lookup).
pub struct TargetRewind<'a> {
    combat: &'a Combat,
    scene: &'a PreparedScene,
    quanta: u32,
}

impl TargetRewind<'_> {
    /// Each body [`Combat::target_candidates`] lists, where `command`'s
    /// view had it, into `seen`; one that did not exist then is left out.
    pub fn seen(&self, command: &Command, seen: &mut Vec<SeenBody>) {
        let view = ViewFrame::from_command(command, self.quanta);
        let place = |entity, pose: TargetPose| SeenBody {
            entity,
            feet: pose.feet,
            height: pose.height,
            radius: pose.radius,
            yaw: pose.yaw,
        };
        seen.clear();
        seen.extend(self.combat.players.iter().filter_map(|slot| {
            let pose = slot.history.at(view, slot.target.pose.clone())?;
            Some(place(slot.target.body, pose))
        }));
        let bots = self.scene.bots().iter().zip(&self.combat.targets);
        seen.extend(bots.filter_map(|(bot, target)| {
            let pose = target.history.at(view, self.scene.bot_target(bot))?;
            Some(place(bot.entity, pose))
        }));
    }
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
    player_pushes: Vec<PlayerPush>,
    next_bot: u32,
    /// The ids of the projectiles the other clients see and of the Biotic
    /// Fields (projectiles.py `Projectiles.next_seen`, `next_field`).
    seen_ids: SeenIds,
    field_ids: FieldIds,
    /// The Biotic Fields and the server projectiles whose entity starts
    /// graphs (the Biotic Orbs) the room runs on their graphs.
    spawned: SpawnedEntities,
    /// The last bot rocket's id, and the ids of those that landed since
    /// the match last looked.
    next_rocket: u64,
    landed: Vec<u64>,
    /// The auras the hits put on bodies since the match last looked.
    auras: Vec<AuraHit>,
    /// The stats the room counted natively since the match last looked
    /// ([`Self::take_tracked_stats`]).
    counted: Vec<TrackedStat>,
    /// The notices of the changes the shooters' dealt modifiers and the
    /// bots' taken ones made since the match last looked
    /// ([`Self::take_messages`]).
    modified: Vec<GameMessage>,
}

/// An aura a hit put on a body ([`Combat::take_auras`]): the match puts it
/// on the body's script (`vm/auras.rs` in `lucy_script`).
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
            player_pushes: Vec::new(),
            next_bot: BOT_ENTITY + scene.bots().len() as u32,
            seen_ids: SeenIds::new(),
            field_ids: FieldIds::default(),
            spawned: SpawnedEntities::new(
                scene.shared_graphs(),
                scene
                    .spawn_graphs()
                    .get(&FIELD_ENTITY)
                    .cloned()
                    .unwrap_or_default(),
                Arc::clone(scene.spawn_graphs()),
                scene.map().mode_guid,
                scene.map().ruleset,
                scene.mode_random(),
                Some(scene.map_rays()),
            ),
            next_rocket: 0,
            landed: Vec::new(),
            auras: Vec::new(),
            counted: Vec::new(),
            modified: Vec::new(),
        }
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
    /// controller's (its id less the low byte); an AI hero's is his AI
    /// entity's, an AI (component 92, which its definition declares), and
    /// his player's, whose controller the clients see possessing it; a
    /// bot's is its stand-in controller's (`scene::bot_controller_entity`),
    /// an AI too (component 92: the bot's AI definition possesses it). A
    /// player's body lists the Biotic Fields its controller owns.
    pub fn target_candidates(&self, scene: &PreparedScene) -> Vec<TargetCandidate> {
        let free_for_all = scene.map().free_for_all;
        let definitions = scene.graphs().ai_definitions();
        let candidate =
            |entity, team, dead, controller, ai, pose: &TargetPose, tags, health| TargetCandidate {
                entity,
                team: (!free_for_all).then_some(team),
                dead,
                controller,
                player: None,
                ai,
                ai_control: None,
                feet: pose.feet,
                height: pose.height,
                radius: pose.radius,
                yaw: pose.yaw,
                tags,
                health,
                spawned: Vec::new(),
                hardpoints: None,
            };
        self.players
            .iter()
            .map(|slot| {
                let target = &slot.target;
                let player = target.body & !0xff;
                let controller = target.ai.map_or(player, |ai| ai.entity);
                TargetCandidate {
                    player: Some(player),
                    ai_control: target.ai,
                    spawned: self.spawned.owned_by(target.body),
                    hardpoints: target.hardpoints.clone(),
                    ..candidate(
                        target.body,
                        target.team,
                        target.dead,
                        Some(controller),
                        target.ai.is_some_and(|ai| ai.has_component_92(definitions)),
                        &target.pose,
                        target.tags.clone(),
                        target.health,
                    )
                }
            })
            .chain(scene.bots().iter().enumerate().map(|(slot, bot)| {
                let controller = bot_controller_entity(slot).ok();
                let pose = scene.bot_target(bot);
                let vitals = self.targets.get(slot).map(|target| &target.vitals);
                TargetCandidate {
                    hardpoints: scene.body_hardpoints(bot.spawn.body),
                    ..candidate(
                        bot.entity,
                        bot.spawn.team,
                        bot.dead,
                        controller,
                        true,
                        &pose,
                        scene.body_tags(bot.spawn.body),
                        vitals.map(|vitals| [vitals.total(), vitals.maximum()]),
                    )
                }
            }))
            .collect()
    }

    /// The bodies of [`Self::target_candidates`] as a player's client
    /// showed them at his commands' views, for his body's target searches
    /// that rewind ([`TargetRewind`]).
    pub fn rewind<'a>(&'a self, scene: &'a PreparedScene, quanta: u32) -> TargetRewind<'a> {
        TargetRewind {
            combat: self,
            scene,
            quanta,
        }
    }

    /// The pools of the bot in `slot` now.
    pub fn bot_vitals(&self, slot: usize) -> Option<&Vitals> {
        self.targets.get(slot).map(|target| &target.vitals)
    }

    /// The hits on players' bodies since the last call, in order.
    pub fn take_player_hits(&mut self) -> Vec<PlayerHit> {
        std::mem::take(&mut self.player_hits)
    }

    /// The pushes explosions gave players' bodies since the last call, in
    /// order.
    pub fn take_player_pushes(&mut self) -> Vec<PlayerPush> {
        std::mem::take(&mut self.player_pushes)
    }

    /// The environment kills the player's body at `tick` (`map_areas`
    /// module notes: it fell onto the map's catch floor, into a kill volume
    /// or a lethal damage area): a hit on it of [`ENVIRONMENT_DAMAGE`] that
    /// goes the way of any other, so the victim gets 25004, his body the
    /// death message and his board a death, and he respawns after the
    /// mode's time. The match credits it to whoever last knocked the body
    /// ([`KnockCredit`], [`apply_player_hits`]); without one the body is its
    /// own killer ("YOU DIED"), as in a self-inflicted explosion.
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
            environment: true,
            from: None,
            source: None,
            health_pack: false,
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
                && (enemy || strikes_ally(allies, target.hurt()))
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
    /// ([`PreparedScene::hitscan_wall`]), and the bodies only when the
    /// profile's rays test their hit shapes
    /// ([`PreparedScene::ray_meets_bodies`]).
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
        let bodies = scene.ray_meets_bodies(ray_profile);
        let players: Vec<_> = if bodies {
            self.player_candidates(shooter, free_for_all, allies, |slot| {
                slot.history.at(view, slot.target.pose.clone())
            })
            .collect()
        } else {
            Vec::new()
        };
        let bots = scene
            .bots()
            .iter()
            .zip(&self.targets)
            .filter_map(|(bot, target)| {
                if !bodies
                    || !eligible_bot(bot, target, shooter, free_for_all, allies)
                    || !visible(bot.entity)
                {
                    return None;
                }
                target
                    .history
                    .at(view, scene.bot_target(bot))
                    .map(|pose| (bot.entity, pose))
            });
        let traced = tracing::enabled!(target: "lucy::hits", tracing::Level::DEBUG);
        if traced {
            trace_shot(
                shooter,
                shot_frame,
                view,
                ray,
                players.iter().cloned().chain(bots.clone()),
            );
        }
        let impact = ray
            .resolve(
                players.into_iter().chain(bots),
                |origin, direction, reach| {
                    scene.hitscan_wall(origin, direction, reach, ray_profile)
                },
            )
            .map_err(io::Error::other)?;
        if traced {
            tracing::debug!(
                target: "lucy::hits",
                "shot of {:08X} at frame {shot_frame}: {}",
                shooter.source,
                impact.map_or_else(
                    || "missed".to_owned(),
                    |impact| format!(
                        "struck {:08X} at {:.3} m{}",
                        impact.entity,
                        impact.distance,
                        if impact.critical { ", critical" } else { "" }
                    )
                )
            );
        }
        let Some(impact) = impact else {
            self.count_shot(shooter, None);
            return Ok(None);
        };
        let struck = self
            .enemy_of(scene, shooter, impact.entity)
            .then_some(impact.critical);
        self.count_shot(shooter, struck);
        Ok(self.apply_impact(
            scene, shooter, shot_frame, world_tick, context, &impact, None,
        ))
    }

    /// A health change a body's graph made ([`HealthChange`]: quick melee
    /// `0043` st4 strikes what its search found, Biotic Grenade's auras
    /// heal or damage the body they are on), applied as a shot's impact
    /// is: the damage message, the stats, a death and its respawn follow.
    /// Its instigator is the shooter, a player's body or a bot as the match
    /// last placed them, or a server placeable of the map (the spawn room
    /// `0D62` that `0C8E` st13's heal names, `#40`); another instigator
    /// changes nothing. The server's native rule decides who takes it
    /// (assumption, as for the shots: combat.py `_enemies`): damage strikes
    /// an enemy of the instigator (anyone in a free-for-all) and the
    /// instigator's own body (the enemy spawn room `0C8E` st45), a heal an
    /// ally or itself; a placeable's change strikes the player's body its
    /// graph named, whatever his team (the graph chose him), and no bot
    /// (the bots run no mode body graph that names one). A bot takes it
    /// only once its create reached the instigator's client (`visible`, as
    /// for a shot); the dead take nothing. As for the shots, quick melee's
    /// search found the bodies as the shooter's client showed them
    /// ([`TargetRewind`]). A change
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
        // A server placeable, or the mode entity (the Workshop's Kill
        // `1482` without a killer names none, so its instigator falls back
        // to the mode entity): the environment's, as a placeable's.
        let placeable = || {
            change.instigator == GAME_MODE_ENTITY
                || scene
                    .server_placeables()
                    .iter()
                    .any(|placed| placed.entity() == change.instigator)
        };
        let (shooter, environment) = match self.shooter(scene, change.instigator) {
            Some(shooter) => (shooter, false),
            None if placeable() => (
                Shooter {
                    source: change.instigator,
                    team: u8::MAX,
                    bot: false,
                },
                true,
            ),
            None => return None,
        };
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
            let takes = environment
                || if change.heal {
                    own || teammate
                } else {
                    !teammate
                };
            if !takes {
                return None;
            }
            let from = (!own).then(|| self.feet(scene, shooter.source)).flatten();
            let direction = from.and_then(|from| damage_direction(from, slot.target.pose.feet));
            let (amount, tags) = self.dealt(
                scene,
                shooter,
                target,
                change.heal,
                change.amount,
                change.tags,
                None,
            );
            self.player_hits.push(PlayerHit {
                body: target,
                shooter,
                amount,
                critical: false,
                loadout: change.loadout,
                frame: change.frame,
                world_tick: None,
                heal: change.heal,
                tags,
                direction,
                environment: false,
                from,
                source: change.source,
                health_pack: change.heal && change.site.graph == HEALTH_PACK,
            });
            // Its push (Soundwave's), as an explosion's on another player
            // ([`PlayerPush`]): the owner corrections carry it to his
            // client, which predicts none for another's hit.
            if let Some(push) = change.knockback.filter(|_| !change.heal && !own) {
                self.player_pushes.push(PlayerPush {
                    body: target,
                    push,
                    by: shooter,
                    loadout: change.loadout,
                });
            }
            self.auras.extend(aura);
            return None;
        }
        if environment || !visible(target) {
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
        let mut hit =
            self.apply_bot_impact(scene, shooter, change.frame, None, context, &impact, None)?;
        hit.source = change.source;
        self.auras.extend(aura);
        if let Some(push) = change.knockback.filter(|_| !change.heal) {
            scene.knock_bot(target, push, context.tick);
        }
        Some(hit)
    }

    /// The shooter `entity` is: a player's body as the match last placed
    /// it (his player entity, the controller, standing for it: the
    /// Workshop's Heal `137C` names the healer player as its instigator,
    /// `STU_77B0987E(#18739[1])`, where Damage `1483` takes his body with
    /// `STU_CF348C08`), or a living bot.
    fn shooter(&self, scene: &PreparedScene, entity: u32) -> Option<Shooter> {
        if let Some(slot) = self
            .players
            .iter()
            .find(|slot| slot.target.body == entity || slot.target.body & !0xff == entity)
        {
            return Some(Shooter {
                source: slot.target.body,
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

    /// Who the players' graphs named their bodies' environment kills
    /// credit since the last call (`STU_545AB3B8`: the Rocket Punch
    /// victim's `0D07` names the puncher before its wall slam), kept as
    /// each body's knock credit ([`WorldDelivery::credit_environment`]):
    /// a player's body or a living bot ([`Self::shooter`]); an entity that
    /// is neither, or none, credits nobody.
    pub fn apply_environment_credits(
        &self,
        scene: &PreparedScene,
        players: &mut [CombatPlayer<'_>],
        now: Duration,
    ) {
        for world in players
            .iter_mut()
            .filter_map(|player| player.world.as_deref_mut())
        {
            for credit in world.take_environment_credits() {
                let by = credit.by.and_then(|entity| self.shooter(scene, entity));
                world.credit_environment(credit.body, by, now);
            }
        }
    }

    /// A hit of `amount` with `tags` on `target` as the shooter's dealt
    /// modifiers make it (`lucy_script::vm::statuses`): a heal scaled
    /// by those of the healing he gives, damage by those of the damage he
    /// deals, as he holds them at the hit ([`Self::dealt_modifiers`]) or,
    /// for a projectile's hit, as `launched` kept them at its launch; with
    /// the tags they added. Their notices wait for the match
    /// ([`Self::take_messages`]).
    #[allow(clippy::too_many_arguments)]
    fn dealt(
        &mut self,
        scene: &PreparedScene,
        shooter: Shooter,
        target: u32,
        heal: bool,
        amount: f64,
        mut tags: DamageTags,
        launched: Option<&[CombatMod]>,
    ) -> (f64, DamageTags) {
        let kind = if heal { HEALING_DEALT } else { DAMAGE_DEALT };
        // A dealt kind has no party filter (`m_5E6F1B8B` is set nowhere).
        let scaled = match launched {
            Some(launched) => combat_scale(launched, kind, &mut tags, &[]),
            None => combat_scale(self.dealt_modifiers(scene, shooter), kind, &mut tags, &[]),
        };
        self.modified.extend(scaled.notices(amount, target));
        (scaled.apply(amount), tags)
    }

    /// The modifiers the shooter holds now: a bot's from its script, a
    /// player's as [`PlayerTarget::dealt`] has them.
    fn dealt_modifiers<'a>(
        &'a self,
        scene: &'a PreparedScene,
        shooter: Shooter,
    ) -> impl Iterator<Item = &'a CombatMod> {
        let source = shooter.source;
        let bot = shooter
            .bot
            .then(|| scene.bot_scripts().effects(source))
            .flatten()
            .into_iter()
            .flat_map(BodyEffects::modifiers);
        let player = (!shooter.bot)
            .then(|| self.players.iter().find(|slot| slot.target.body == source))
            .flatten()
            .into_iter()
            .flat_map(|slot| &slot.target.dealt);
        bot.chain(player)
    }

    /// The notices of the changes modifiers made to the hits since the
    /// last call (`lucy_script::vm::MODIFIED`), each to the entity its
    /// modifier credits ([`crate::messages`]): the shooters' dealt ones
    /// and the bots' taken ones (a player's taken ones go with his body's
    /// messages, `WorldDelivery::take_messages`).
    pub fn take_messages(&mut self) -> Vec<GameMessage> {
        std::mem::take(&mut self.modified)
    }

    /// The auras the hits put on bodies since the last call, in order.
    pub fn take_auras(&mut self) -> Vec<AuraHit> {
        std::mem::take(&mut self.auras)
    }

    /// The auras the room's spawned entities (Biotic Fields and Orbs) put
    /// on bodies or took off since the last call (`crate::spawned`), for
    /// the bodies' scripts.
    pub fn take_field_auras(&mut self) -> Vec<AuraChange> {
        self.spawned.take_auras()
    }

    /// The stats the room counted since the last call, for the players
    /// they name: what its Biotic Fields' graphs counted (`0258` rid14
    /// "Biotic Fields Deployed", for its owner's player), and what the
    /// server counts itself, as the retail server did (no graph names
    /// them; `refs/reports/StatCredit.md`): a player's damage to the
    /// training bots, his heals of them and the bots he eliminated
    /// ([`Self::apply_bot_impact`]), and his weapons' shots
    /// ([`Self::count_shot`]).
    pub fn take_tracked_stats(&mut self) -> Vec<TrackedStat> {
        let mut stats = self.spawned.take_tracked_stats();
        stats.append(&mut self.counted);
        stats
    }

    /// `amount` of each of `stats` for the player whose body (or entity)
    /// `player` is.
    fn count(&mut self, player: u32, stats: Vec<u64>, amount: f64) {
        self.counted.push(TrackedStat {
            stats,
            amount,
            targets: vec![player],
        });
    }

    /// One shot of a player's volley: a hitscan ray (a pellet of a hitscan
    /// shotgun each), or a projectile once its flight ended. It counts
    /// Shots Fired (`0030`, stat event 9), and Shots Hit (`0031`, event 10)
    /// when it struck an enemy (`struck`: whether that hit was critical,
    /// which also counts Critical Hits `03DE`, event 12), else Shots Missed
    /// (`05F2`, event 63). The board's Weapon Accuracy (`002F`) and Critical
    /// Hit Accuracy (`03E2`) are ratios over them (`game_stat_rules_174.json`).
    /// Assumptions (no data or client code says): every damaging volley of
    /// the hero counts (its abilities' too), a projectile counts as a hit
    /// when it hurt an enemy directly or by its explosion, and a shot that
    /// struck only an ally (a heal) counts as missed. A bot's shots count
    /// nothing.
    fn count_shot(&mut self, shooter: Shooter, struck: Option<bool>) {
        if shooter.bot {
            return;
        }
        let mut stats = vec![SHOTS_FIRED];
        match struck {
            Some(critical) => {
                stats.push(SHOTS_HIT);
                if critical {
                    stats.push(CRITICAL_HITS);
                }
            }
            None => stats.push(SHOTS_MISSED),
        }
        self.count(shooter.source, stats, 1.0);
    }

    /// Whether `entity`, a player's body or a bot, is an enemy of
    /// `shooter`: of another team, or anyone else in a free-for-all.
    fn enemy_of(&self, scene: &PreparedScene, shooter: Shooter, entity: u32) -> bool {
        let free_for_all = scene.map().free_for_all;
        let team = self
            .players
            .iter()
            .find(|slot| slot.target.body == entity)
            .map(|slot| slot.target.team)
            .or_else(|| {
                scene
                    .bots()
                    .iter()
                    .find(|bot| bot.entity == entity)
                    .map(|bot| bot.spawn.team)
            });
        team.is_some_and(|team| entity != shooter.source && (free_for_all || team != shooter.team))
    }

    /// The room's spawned entities, which send and take game messages
    /// ([`crate::messages`]).
    pub fn spawned_mut(&mut self) -> &mut SpawnedEntities {
        &mut self.spawned
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
        // Its first step is its launch: it keeps its shooter's dealt
        // modifiers as they are now for all its hits.
        flight.launch(|| {
            self.dealt_modifiers(scene, shooter)
                .filter(|modifier| [DAMAGE_DEALT, HEALING_DEALT].contains(&modifier.kind))
                .cloned()
                .collect()
        });
        let ray_profile = flight.ray_profile();
        // A profile that doesn't test the bodies' hit shapes flies through
        // them (`PreparedScene::ray_meets_bodies`; an assumption: the
        // client's projectiles were not seen to query by their volley's
        // profile, refs report `WeaponRays.md` "Body masks").
        if !scene.ray_meets_bodies(ray_profile) {
            flight.pass_bodies();
        }
        let step = flight
            .advance(context.tick, &candidates, |origin, direction, reach| {
                scene.projectile_wall(origin, direction, reach, ray_profile)
            })
            .map_err(io::Error::other)?;
        for impact in &step.impacts {
            if self.enemy_of(scene, shooter, impact.entity) {
                flight.strike(impact.critical);
            }
            hits.extend(self.apply_impact(
                scene,
                shooter,
                context.tick,
                Some(context.tick),
                context,
                impact,
                flight.launch_modifiers(),
            ));
        }
        if step.end.is_some() && flight.damaging() {
            self.count_shot(shooter, flight.struck());
        }
        // Its explosion pushes the bodies it can reach and hurt, as it
        // pushes its shooter (`ProjectileFlight::blast_push`): the live bots
        // after its damage ([`PreparedScene::knock_bot`]), and other
        // players' bodies through their movers ([`PlayerPush`]; their
        // worlds refuse it to a body the hit killed or can't hurt,
        // [`WorldDelivery::knock`]): not an ally it heals, nor an
        // invulnerable body, as the shooter takes no push where he takes no
        // damage (`WorldDelivery::can_be_damaged`). Beyond Python, which
        // moved no one.
        if step.end.as_ref().is_some_and(|end| end.exploded) {
            for (entity, pose) in &candidates[..first_bot] {
                let enemy = self.players.iter().any(|slot| {
                    slot.target.body == *entity
                        && (free_for_all || slot.target.team != shooter.team)
                });
                if let Some(push) = flight.blast_push(pose).filter(|_| enemy) {
                    self.player_pushes.push(PlayerPush {
                        body: *entity,
                        push,
                        by: shooter,
                        loadout: flight.blast_loadout(),
                    });
                }
            }
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
            let launched = flight.launch_modifiers();
            let (amount, tags) = self.dealt(
                scene,
                shooter,
                shooter.source,
                false,
                own.damage,
                own.tags,
                launched,
            );
            self.player_hits.push(PlayerHit {
                body: shooter.source,
                shooter,
                amount,
                critical: false,
                loadout: own.loadout,
                frame: context.tick,
                world_tick: Some(context.tick),
                heal: false,
                tags,
                direction: Some(own.direction),
                environment: false,
                from: None,
                source: None,
                health_pack: false,
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
    /// `launched`: a projectile's dealt modifiers ([`Self::dealt`]).
    #[allow(clippy::too_many_arguments)]
    fn apply_impact(
        &mut self,
        scene: &mut PreparedScene,
        shooter: Shooter,
        shot_frame: u32,
        world_tick: Option<u32>,
        context: ShotContext,
        impact: &Impact,
        launched: Option<&[CombatMod]>,
    ) -> Option<BotHit> {
        let free_for_all = scene.map().free_for_all;
        if let Some(slot) = self
            .players
            .iter()
            .find(|slot| slot.target.body == impact.entity && !slot.target.dead)
        {
            let heal = !free_for_all && slot.target.team == shooter.team;
            let amount = if heal { impact.heal } else { impact.damage };
            let from = self.feet(scene, shooter.source);
            let direction = from.and_then(|from| damage_direction(from, slot.target.pose.feet));
            let (amount, tags) = self.dealt(
                scene,
                shooter,
                impact.entity,
                heal,
                amount,
                impact.tags,
                launched,
            );
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
                    tags,
                    direction,
                    environment: false,
                    from,
                    source: None,
                    health_pack: false,
                });
            }
            return None;
        }
        self.apply_bot_impact(
            scene, shooter, shot_frame, world_tick, context, impact, launched,
        )
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

    #[allow(clippy::too_many_arguments)]
    fn apply_bot_impact(
        &mut self,
        scene: &mut PreparedScene,
        shooter: Shooter,
        shot_frame: u32,
        world_tick: Option<u32>,
        context: ShotContext,
        impact: &Impact,
        launched: Option<&[CombatMod]>,
    ) -> Option<BotHit> {
        // A Training Bot is back after the mode's respawn time, as a hero
        // (`game_respawns`: 3 s in the practice modes).
        let respawn = scene.respawn_time();
        let board = *scene.board_stats();
        let free_for_all = scene.map().free_for_all;
        let heal = !free_for_all
            && scene
                .bots()
                .iter()
                .any(|bot| bot.entity == impact.entity && bot.spawn.team == shooter.team);
        // The shooter's dealt modifiers, then the bot's statuses and taken
        // modifiers (`vm/statuses.rs`): an invulnerable bot takes no
        // damage, and its graphs' modifiers (their party filters tested on
        // the shooter) scale what it takes (Biotic Grenade's anti-heal and
        // heal boost, Nano Boost's damage reduction).
        let amount = if heal { impact.heal } else { impact.damage };
        let (amount, mut tags) = self.dealt(
            scene,
            shooter,
            impact.entity,
            heal,
            amount,
            impact.tags,
            launched,
        );
        let invulnerable = scene
            .bot_scripts()
            .effects(impact.entity)
            .is_some_and(|effects| effects.invulnerable());
        let taken = if heal { HEALING_TAKEN } else { DAMAGE_TAKEN };
        let scaled =
            scene
                .bot_scripts_mut()
                .combat_scale(impact.entity, taken, &mut tags, shooter.source);
        self.modified.extend(scaled.notices(amount, shooter.source));
        let amount = scaled.apply(amount);
        let (bot, target) = scene
            .bots_mut()
            .iter_mut()
            .zip(&mut self.targets)
            .find(|(bot, _)| !bot.dead && bot.entity == impact.entity)?;
        if heal {
            let given = target.vitals.heal(amount);
            let hit = (given > 0.0).then(|| healed_bot(bot, target, given, tags));
            // A player healer's Healing Done counts what the pools took.
            if hit.is_some() && !shooter.bot {
                self.count(shooter.source, vec![board.healing], given);
            }
            return hit;
        }
        if invulnerable {
            return None;
        }
        let death_tick = world_tick.unwrap_or(context.tick);
        let result = target.vitals.damage(
            amount,
            Death {
                time: context.now.as_secs_f64(),
                frame: death_tick,
                killer: shooter.source,
            },
        );
        if result.dealt == 0.0 {
            return None;
        }
        let mut components = vec![target.vitals.component()];
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
        let hit = BotHit {
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
                    assist: false,
                }),
            ],
            health,
            bot: bot.entity,
            remaining: target.vitals.total(),
            healed: 0.0,
            tags,
            source: None,
            death: result.killed.then_some(BotDeath {
                bot: bot.entity,
                killer: shooter.source,
                critical: impact.critical,
                loadout: impact.loadout,
                tags,
                dealt: result.dealt,
                tick: death_tick,
            }),
            damage: (!result.killed).then_some(BotDamage {
                source: shooter.source,
                dealt: result.dealt,
                tags,
                tick: death_tick,
            }),
        };
        // A player's board counts the bot as it counts a player's body
        // (`apply_player_hits`): Hero Damage Done for what the pools took,
        // and for the kill Eliminations and Final Blows (the Training Bots
        // are heroes: `016B`, `016E`). A bot's death credits only its
        // killer: no hit log or assist states for bot victims.
        if player {
            self.count(shooter.source, vec![board.hero_damage], result.dealt);
            if result.killed {
                self.count(shooter.source, vec![board.eliminations, FINAL_BLOWS], 1.0);
            }
        }
        Some(hit)
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
            let created = bot_create(&fresh)?;
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
}

/// A bot's heal of `given`: its new pools for every client, and no notice
/// (combat.py `_heal` sends none).
fn healed_bot(bot: &Bot, target: &mut Target, given: f64, tags: DamageTags) -> BotHit {
    let health = EntityUpdate {
        entity: bot.entity,
        operation: Some(Operation::Update(vec![target.vitals.component()])),
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
        tags,
        source: None,
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

/// How long damage to a body counts toward the eliminations of its death:
/// every player who hurt it within this before it died is credited one
/// (the retail server's own rule: no graph or data names the window).
/// Assumption: 3 s, the rule players measured (Overwatch Wiki, "Common
/// Terms": "any player who dealt any damage to the eliminated enemy hero
/// within 3 seconds before they died will get an Elimination credit"); the
/// debuffs' assist credit outlasts them as long (`STU_0DB9E66B`
/// `m_43B917C6` 3 s), while `1901`'s attacker list keeps a pet's owner 5 s
/// (`refs/reports/StatCredit.md`).
pub const DAMAGE_CREDIT: Duration = Duration::from_secs(3);

/// Whether damage dealt at `at` counts toward a death at `now`
/// ([`DAMAGE_CREDIT`]).
pub fn credits_damage(at: Duration, now: Duration) -> bool {
    now <= at + DAMAGE_CREDIT
}

/// How long a knockback credits the environment kill of the body it
/// knocked ([`KnockCredit`]). The retail server's own rule; no graph or
/// data names it. Assumption: 10 s, as players measured it in OW1 (KarQ,
/// r/OverwatchUniversity 2019-08-24, "1 Environmental Kill Tip for Every
/// Hero": "Wait 10 seconds" is one of four ways to cancel a knockback's
/// credit; `refs/reports/StatCredit.md`).
pub const KNOCKBACK_CREDIT: Duration = Duration::from_secs(10);

/// Who an environment kill of a player's body credits: the last enemy
/// whose explosion knocked it ([`PlayerPush`]), for [`KNOCKBACK_CREDIT`]
/// after the knock, as the retail rule gives an environmental kill to the
/// last player who applied crowd control (`refs/reports/RulesNotes.md`
/// 146; damage alone doesn't). The other ways players measured to cancel
/// it (KarQ, as [`KNOCKBACK_CREDIT`]): a jump, a landing on the ground,
/// and the body's own knockback (its own explosion). Assumptions: a
/// landing cancels it only once the tick's environment kills are dealt
/// ([`Self::settle`]), as a fall ends by landing on the catch floor that
/// kills; the explosions are the only knockbacks the server applies to a
/// player's body.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct KnockCredit(Option<Knock>);

#[derive(Clone, Copy, Debug, PartialEq)]
struct Knock {
    body: u32,
    by: Shooter,
    at: Duration,
    landed: bool,
}

impl KnockCredit {
    /// Another's explosion (`by`) knocked `body` at `now`.
    pub fn knocked(&mut self, body: u32, by: Shooter, now: Duration) {
        self.0 = Some(Knock {
            body,
            by,
            at: now,
            landed: false,
        });
    }

    /// The body's own explosion pushed it.
    pub fn own_push(&mut self) {
        self.0 = None;
    }

    /// A graph named who an environment kill of `body` credits at `now`
    /// (`STU_545AB3B8`): `by` as if he knocked it, or nobody.
    pub fn named(&mut self, body: u32, by: Option<Shooter>, now: Duration) {
        match by {
            Some(by) => self.knocked(body, by, now),
            None => self.0 = None,
        }
    }

    /// The body's mover posed a frame that jumped or landed it.
    pub fn posed(&mut self, landed: bool, jumped: bool) {
        if jumped {
            self.0 = None;
        } else if let Some(knock) = self.0.as_mut().filter(|_| landed) {
            knock.landed = true;
        }
    }

    /// The tick's environment kills are dealt: a knock the body landed
    /// from credits no later one.
    pub fn settle(&mut self) {
        if self.0.is_some_and(|knock| knock.landed) {
            self.0 = None;
        }
    }

    /// Who the environment's kill of `body` at `now` credits.
    pub fn credit(&self, body: u32, now: Duration) -> Option<Shooter> {
        self.0
            .filter(|knock| knock.body == body && now <= knock.at + KNOCKBACK_CREDIT)
            .map(|knock| knock.by)
    }
}

/// Final Blows (`002B`): the career counter of the stat event 0
/// (`m_primaryEvent`), the hero board's "Final Blows".
pub(crate) const FINAL_BLOWS: u64 = 0x0860_0000_0000_002b;
/// Shots Fired (`0030`, stat event 9), Shots Hit (`0031`, event 10), Shots
/// Missed (`05F2`, event 63) and Critical Hits (`03DE`, event 12):
/// [`Combat::count_shot`].
const SHOTS_FIRED: u64 = 0x0860_0000_0000_0030;
const SHOTS_HIT: u64 = 0x0860_0000_0000_0031;
const SHOTS_MISSED: u64 = 0x0860_0000_0000_05f2;
const CRITICAL_HITS: u64 = 0x0860_0000_0000_03de;
/// Offensive Assists (`03D4`, stat event 47) and Defensive Assists
/// (`03DA`, event 48), on the support heroes' boards.
const OFFENSIVE_ASSISTS: u64 = 0x0860_0000_0000_03d4;
const DEFENSIVE_ASSISTS: u64 = 0x0860_0000_0000_03da;
/// Environmental Kills (`0362`, stat event 49) and Environmental Deaths
/// (`0365`, event 50): [`apply_player_hits`].
pub(crate) const ENVIRONMENTAL_KILLS: u64 = 0x0860_0000_0000_0362;
pub(crate) const ENVIRONMENTAL_DEATHS: u64 = 0x0860_0000_0000_0365;

/// An assist a player earned on a kill ([`kill_credit`]).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Assist {
    Offensive,
    Defensive,
}

/// Whom a player's death credits besides its killer ([`kill_credit`]),
/// by slot.
#[derive(Debug, Default, PartialEq)]
pub struct KillCredit {
    /// The players who hurt the victim within [`DAMAGE_CREDIT`] before the
    /// death: an elimination each.
    pub eliminations: Vec<usize>,
    /// The players whose assist states name the victim or one of its
    /// eliminators: an assist each.
    pub assists: Vec<(usize, Assist)>,
}

/// The credit of the death of `victim` (slot and body) killed by `killer`
/// (a player's slot; none: a bot, the environment or his own blow), the
/// retail server's native bookkeeping (no graph credits it):
///
/// - an elimination for every player among `attackers` (the sources of
///   the damage the victim took within [`DAMAGE_CREDIT`]) but the killer
///   and the victim;
/// - an assist for every player an assist state names as its assister
///   (`links`, every player's: `STU_0DB9E66B`, `vm/statuses.rs`): an
///   offensive one (kind 1) when the state's targets hold the victim, a
///   defensive one (kind 2) when they hold the body of a player credited
///   with the elimination (the killer or one in the list above: "the
///   players whose eliminations grant assist credit"). The killer and the
///   victim get none; a player with both kinds gets the defensive one (a
///   player's report, Blizzard forums 2022: "Zen healing someone who kills
///   a discorded target counts as defensive"); an assister may be credited
///   an elimination too. Assumptions, from the Workshop's Start Assist and
///   the kinds' targets (`refs/reports/StatCredit.md`).
///
/// `bodies` are the players' current bodies, by slot; `player_of` names a
/// body's (or player entity's) player.
pub fn kill_credit(
    (victim, victim_body): (usize, u32),
    killer: Option<usize>,
    attackers: &[u32],
    links: &[AssistLink],
    bodies: &[Option<u32>],
    player_of: impl Fn(u32) -> Option<usize>,
) -> KillCredit {
    let others = |slot: &usize| *slot != victim && Some(*slot) != killer;
    let mut eliminations: Vec<usize> = Vec::new();
    for slot in attackers.iter().filter_map(|&source| player_of(source)) {
        if others(&slot) && !eliminations.contains(&slot) {
            eliminations.push(slot);
        }
    }
    let eliminators: Vec<u32> = killer
        .iter()
        .chain(&eliminations)
        .filter_map(|&slot| bodies.get(slot).copied().flatten())
        .collect();
    let mut assists: Vec<(usize, Assist)> = Vec::new();
    for link in links {
        let earned = match link.kind {
            ASSIST_OFFENSIVE if link.targets.contains(&victim_body) => Assist::Offensive,
            ASSIST_DEFENSIVE
                if link
                    .targets
                    .iter()
                    .any(|target| eliminators.contains(target)) =>
            {
                Assist::Defensive
            }
            _ => continue,
        };
        for slot in link
            .assisters
            .iter()
            .filter_map(|&entity| player_of(entity))
        {
            if !others(&slot) {
                continue;
            }
            match assists.iter_mut().find(|(held, _)| *held == slot) {
                Some((_, kind)) if earned == Assist::Defensive => *kind = earned,
                Some(_) => {}
                None => assists.push((slot, earned)),
            }
        }
    }
    KillCredit {
        eliminations,
        assists,
    }
}

/// The hits on players' bodies (combat.py `_damage`, `_tell_hit`,
/// `_died`, `_kill_notice`): the victim's pools take each; a player who hit
/// another gets 25001, Hero Damage Done and ultimate charge for it (his own
/// explosion's damage to himself gives none of them: beyond Python, which
/// never hurt the shooter; self damage gives no charge, game-rules.md 154,
/// and isn't damage done or a hit to show, assumption). A kill counts
/// Deaths for the victim and gives him 25004 ("YOU WERE KILLED BY", or
/// "YOU DIED" for his own blow); another player who killed him gets
/// Eliminations, Final Blows, 25005 and `0093.025` (his body's
/// elimination line, his multikills: [`crate::announcer`]); the others the
/// death credits ([`kill_credit`]) get, for damage, Eliminations, 25005
/// and `0093.025` without the final blow flag, and for an assist its stat
/// (Offensive or Defensive Assists), 25005 as an assist ("ASSIST", unless
/// he has the elimination's) and a place in the death message's assists
/// (`03E7`, the kill feed's assist icons). The environment's kill
/// ([`PlayerHit::environment`]) is the kill of whoever last knocked the
/// body ([`KnockCredit`]), counted as damage of the health it had left
/// (`refs/reports/RulesNotes.md` 146: Hero Damage Done and ultimate charge,
/// no hit marker, assumption), and with an Environmental Kill for a player
/// who knocked it and an Environmental Death for the victim. A heal of an
/// ally (combat.py `_heal`, leafall903/ORPHEUS (MIT) commit c239a84) tells
/// no one: a player healer's Healing Done and ultimate count what the pools
/// took. Returns each player's notices, by slot, and what the hits did
/// between the players ([`PlayerEvent`]), in order.
pub fn apply_player_hits(
    scene: &PreparedScene,
    hits: Vec<PlayerHit>,
    players: &mut [CombatPlayer<'_>],
    tick: u32,
    now: Duration,
) -> (Vec<Vec<CombatNotice>>, Vec<PlayerEvent>) {
    let board = *scene.board_stats();
    let mut notices = vec![Vec::new(); players.len()];
    let mut events = Vec::new();
    for mut hit in hits {
        let Some(victim) = players.iter().position(|player| {
            player
                .world
                .as_deref()
                .is_some_and(|world| world.current_body() == Some(hit.body))
        }) else {
            continue;
        };
        let victim_body = players[victim]
            .world
            .as_deref()
            .and_then(|world| world.current_hero())
            .and_then(|hero| scene.heroes().get(hero))
            .map(|hero| hero.body);
        if hit.heal {
            let given = players[victim]
                .world
                .as_deref_mut()
                .map_or(0.0, |world| world.heal_body(scene, &hit));
            let healer = (!hit.shooter.bot)
                .then(|| player_of(players, hit.shooter.source))
                .flatten();
            if given > 0.0 {
                events.push(PlayerEvent::Heal {
                    healer: healer.map(|slot| players[slot].entity),
                    healee: players[victim].entity,
                    healed: given,
                    health_pack: hit.health_pack,
                    loadout: hit.loadout,
                });
            }
            if !hit.shooter.bot {
                let target = (hit.body, victim_body);
                credit_healing(
                    scene,
                    players,
                    hit.shooter.source,
                    target,
                    given,
                    &hit.tags,
                    hit.source,
                );
            }
            continue;
        }
        if hit.environment
            && let Some(by) = players[victim]
                .world
                .as_deref()
                .and_then(|world| world.knocked_by(hit.body, now))
        {
            hit.shooter = by;
        }
        let Some(damage) = players[victim]
            .world
            .as_deref_mut()
            .and_then(|world| world.apply_damage(scene, &hit, tick, now))
        else {
            continue;
        };
        let shooter = (!hit.shooter.bot)
            .then(|| player_of(players, hit.shooter.source))
            .flatten();
        let own_blow = shooter == Some(victim);
        let killer = shooter.filter(|_| !own_blow);
        events.push(PlayerEvent::Damage {
            attacker: shooter.map(|slot| players[slot].entity),
            victim: players[victim].entity,
            dealt: damage.dealt,
            critical: hit.critical,
            environment: hit.environment,
            direction: hit.direction,
            killed: damage.killed,
            loadout: hit.loadout,
        });
        let bodies: Vec<Option<u32>> = players
            .iter()
            .map(|player| {
                player
                    .world
                    .as_deref()
                    .and_then(WorldDelivery::current_body)
            })
            .collect();
        let credit = if damage.killed {
            death_credit(players, (victim, hit.body), killer, &bodies, now)
        } else {
            KillCredit::default()
        };
        let assisters: Vec<u32> = credit
            .assists
            .iter()
            .filter_map(|&(slot, _)| bodies[slot])
            .collect();
        if let Some(world) = players[victim].world.as_deref_mut() {
            world.send_hit_messages(&hit, &damage, &assisters);
        }
        // His body's `0B8B` decides the charge, his own blow's too (none:
        // rid28, the body hit must be hostile to him); its `01C7` plays the
        // hit's sound, the critical one for a critical hit.
        if let Some(world) = shooter.and_then(|shooter| players[shooter].world.as_deref_mut()) {
            let dealt = Dealt::Damage {
                critical: hit.critical,
            };
            world.dealt(
                &scene.shared_graphs(),
                hit.body,
                victim_body,
                damage.dealt,
                &hit.tags,
                dealt,
            );
            if let Some(source) = hit.source {
                world.source_dealt(source, hit.body, damage.dealt, &hit.tags, dealt);
            }
        }
        if let Some(shooter) = killer.filter(|_| !hit.environment) {
            notices[shooter].push(CombatNotice::Hit {
                target: hit.body,
                dealt: damage.dealt,
                critical: hit.critical,
                killed: damage.killed,
                shot_frame: hit.frame,
            });
        }
        if let Some(shooter) = killer {
            players[shooter].stats.add(board.hero_damage, damage.dealt);
        }
        if !damage.killed {
            continue;
        }
        players[victim].stats.add(board.deaths, 1.0);
        if hit.environment {
            players[victim].stats.add(ENVIRONMENTAL_DEATHS, 1.0);
            if let Some(slot) = killer {
                players[slot].stats.add(ENVIRONMENTAL_KILLS, 1.0);
            }
        }
        let eliminators = killer
            .map(|slot| (slot, true))
            .into_iter()
            .chain(credit.eliminations.iter().map(|&slot| (slot, false)));
        for (slot, final_blow) in eliminators {
            events.push(PlayerEvent::Elimination {
                player: players[slot].entity,
                victim: players[victim].entity,
                final_blow,
            });
            let player = &mut players[slot];
            player.stats.add(board.eliminations, 1.0);
            if final_blow {
                player.stats.add(FINAL_BLOWS, 1.0);
            }
            if let Some(world) = player.world.as_deref_mut() {
                world.eliminated(
                    &scene.shared_graphs(),
                    tick,
                    hit.body,
                    victim_body,
                    &hit.tags,
                    final_blow,
                );
            }
            let credited = if final_blow {
                Some(hit.shooter.source)
            } else {
                bodies[slot]
            };
            if let Some(credited) = credited {
                notices[slot].push(CombatNotice::Elimination {
                    killer: credited,
                    victim: hit.body,
                    assist: false,
                });
            }
        }
        for &(slot, kind) in &credit.assists {
            players[slot].stats.add(
                match kind {
                    Assist::Offensive => OFFENSIVE_ASSISTS,
                    Assist::Defensive => DEFENSIVE_ASSISTS,
                },
                1.0,
            );
            if let Some(body) = bodies[slot].filter(|_| !credit.eliminations.contains(&slot)) {
                notices[slot].push(CombatNotice::Elimination {
                    killer: body,
                    victim: hit.body,
                    assist: true,
                });
            }
        }
        notices[victim].push(CombatNotice::Death {
            source: hit.shooter.source,
            killed_by: !own_blow,
        });
    }
    (notices, events)
}

/// [`kill_credit`] of a player's death: the victim's recent attackers and
/// every player's assist credits, as their worlds hold them.
fn death_credit(
    players: &[CombatPlayer<'_>],
    victim: (usize, u32),
    killer: Option<usize>,
    bodies: &[Option<u32>],
    now: Duration,
) -> KillCredit {
    let attackers = players[victim.0]
        .world
        .as_deref()
        .map(|world| world.recent_attackers(victim.1, now))
        .unwrap_or_default();
    let links: Vec<AssistLink> = players
        .iter()
        .filter_map(|player| player.world.as_deref())
        .flat_map(WorldDelivery::assist_links)
        .collect();
    kill_credit(victim, killer, &attackers, &links, bodies, |entity| {
        player_of(players, entity)
    })
}

/// The explosions' pushes on players' bodies ([`Combat::take_player_pushes`]),
/// after their hits ([`apply_player_hits`]): each goes to the mover of the
/// player whose body it is ([`WorldDelivery::knock`]) at `now`, as the knock
/// an environment kill credits ([`KnockCredit`]). First, with the tick's
/// environment kills dealt, a knock its body landed from is forgotten
/// ([`KnockCredit::settle`]). Returns the pushes that moved a player's
/// body, as knockbacks between players ([`PlayerEvent::Knockback`]).
pub fn apply_player_pushes(
    pushes: Vec<PlayerPush>,
    players: &mut [CombatPlayer<'_>],
    now: Duration,
) -> Vec<PlayerEvent> {
    for world in players
        .iter_mut()
        .filter_map(|player| player.world.as_deref_mut())
    {
        world.settle_knock();
    }
    let mut events = Vec::new();
    for push in pushes {
        let Some(victim) = players.iter().position(|player| {
            player
                .world
                .as_deref()
                .is_some_and(|world| world.current_body() == Some(push.body))
        }) else {
            continue;
        };
        let attacker = (!push.by.bot)
            .then(|| player_of(players, push.by.source))
            .flatten()
            .map(|slot| players[slot].entity);
        let knocked = players[victim]
            .world
            .as_deref_mut()
            .is_some_and(|world| world.knock(push.body, push.push, push.by, now));
        if let Some(attacker) = attacker.filter(|_| knocked) {
            events.push(PlayerEvent::Knockback {
                attacker,
                victim: players[victim].entity,
                push: push.push,
                loadout: push.loadout,
            });
        }
    }
    events
}

/// The healer's Healing Done counts `healed` of `target` (its entity and
/// body definition), and his body takes it as healing done (combat.py
/// `_heal`), when he is a player of the room; so does the heal's `source`
/// besides him (a Biotic Orb, [`WorldDelivery::source_dealt`]).
fn credit_healing(
    scene: &PreparedScene,
    players: &mut [CombatPlayer<'_>],
    healer: u32,
    (target, target_body): (u32, Option<u64>),
    healed: f64,
    tags: &DamageTags,
    source: Option<u32>,
) {
    if let Some(healer) = player_of(players, healer).filter(|_| healed > 0.0) {
        players[healer]
            .stats
            .add(scene.board_stats().healing, healed);
        if let Some(world) = players[healer].world.as_deref_mut() {
            world.dealt(
                &scene.shared_graphs(),
                target,
                target_body,
                healed,
                tags,
                Dealt::Healing,
            );
            if let Some(source) = source {
                world.source_dealt(source, target, healed, tags, Dealt::Healing);
            }
        }
    }
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
/// shot finishes its copies on the other clients (`_end`); the spawned
/// entities run on their graphs (`crate::spawned`): the Biotic Fields whose
/// shot no longer runs end, the server projectiles whose entity starts
/// graphs (the Biotic Orbs) move with their flights or go with them, the
/// others advance, the fields that opened and the new such flights start,
/// a field its graph removed ends its shot and a projectile its graph
/// removed ends its flight (its copies finish); every client in the world
/// gets each running field's entity once (`_deliver_field`: the owner's
/// parented to each of his canisters that arrived, the others' at the
/// shooter's feet) and the field graph's frame whenever it changes; the
/// fields whose shot no longer runs go (`_end`); then each client's copies
/// of the others' projectiles advance (`_ObservedProjectile.advance`).
pub fn deliver_projectiles(
    scene: &PreparedScene,
    combat: &mut Combat,
    worlds: &mut [Option<&mut WorldDelivery>],
    tick: u32,
) {
    let mut events = Vec::new();
    for world in worlds.iter_mut().flatten() {
        let (owner, caster) = (world.owner(), (world.skin(), world.team_bit()));
        events.extend(
            world
                .take_projectile_events()
                .into_iter()
                .map(|(body, event)| (owner, caster, body, event)),
        );
    }
    let live: Vec<ShotKey> = worlds
        .iter()
        .flatten()
        .flat_map(|world| world.live_fields())
        .collect();
    combat.spawned.end_unless(&live);
    let mut opened = Vec::new();
    let mut delivers = Vec::new();
    for (owner, caster, body, event) in events {
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
            ProjectileEvent::FieldOpened { token, feet } => {
                opened.push(((body, token), combat.field_ids.next(), feet));
            }
            ProjectileEvent::FieldDeliver {
                token,
                feet,
                parents,
            } => delivers.push((owner, caster, (body, token), feet, parents)),
        }
    }
    let candidates = combat.target_candidates(scene);
    let flights: Vec<ServerFlight> = worlds
        .iter()
        .flatten()
        .flat_map(|world| world.server_flights())
        .collect();
    let seen_ids = &mut combat.seen_ids;
    combat
        .spawned
        .advance(tick, candidates, opened, &flights, || seen_ids.next());
    for shot in combat.spawned.take_ended() {
        for world in worlds.iter_mut().flatten() {
            world.end_field(shot);
        }
    }
    for (body, token) in combat.spawned.take_ended_flights() {
        for world in worlds.iter_mut().flatten() {
            world.end_server_flight(body, token);
        }
        for world in worlds.iter_mut().flatten() {
            world.finish_server_projectile(body, token, tick, None);
        }
    }
    for (shot, frame) in combat.spawned.flight_frames() {
        for world in worlds.iter_mut().flatten() {
            world.refresh_server_projectile(shot, frame);
        }
    }
    for (shot, scale) in combat.spawned.flight_speeds() {
        for world in worlds.iter_mut().flatten() {
            world.set_server_flight_speed(shot, scale);
        }
    }
    for (owner, (skin, team_bit), shot, feet, parents) in delivers {
        let Some(frame) = combat.spawned.frame(shot) else {
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
                shot,
                parents,
                feet,
                skin,
                team_bit,
                frame,
            };
            world.deliver_field(&field, &mut combat.field_ids);
            world.refresh_field(shot, frame);
        }
    }
    let live: Vec<ShotKey> = worlds
        .iter()
        .flatten()
        .flat_map(|world| world.live_fields())
        .collect();
    for world in worlds.iter_mut().flatten() {
        world.end_fields(&live);
        world.advance_observed(tick);
    }
}

/// The `lucy::hits` debug trace of a hitscan shot: its ray and view,
/// and each candidate body the ray's line passes within 2 m of, as the
/// shot meets it (rewound to the view): its feet and facing, and each
/// critical shape's middle, radius and how far the line passes from it.
fn trace_shot(
    shooter: Shooter,
    shot_frame: u32,
    view: ViewFrame,
    ray: &HitscanShot,
    candidates: impl Iterator<Item = (u32, TargetPose)>,
) {
    let [ox, oy, oz] = ray.origin;
    let [dx, dy, dz] = ray.direction;
    // How far the ray's line passes from a point.
    let off = |point: Vec3| {
        let p = [point[0] - ox, point[1] - oy, point[2] - oz];
        let along = (p[0] * dx + p[1] * dy + p[2] * dz).max(0.0);
        let q = [p[0] - dx * along, p[1] - dy * along, p[2] - dz * along];
        (q[0] * q[0] + q[1] * q[1] + q[2] * q[2]).sqrt()
    };
    let mut near = String::new();
    for (entity, pose) in candidates {
        let [x, y, z] = pose.feet;
        let middle = [x, y + pose.height * 0.5, z];
        if off(middle) > 2.0 + pose.height * 0.5 {
            continue;
        }
        near.push_str(&format!(
            "; {entity:08X} feet ({x:.3}, {y:.3}, {z:.3}) yaw {:.1}",
            pose.yaw.to_degrees()
        ));
        for (centre, radius) in pose.critical_shapes() {
            near.push_str(&format!(
                " crit ({:.3}, {:.3}, {:.3}) r {radius:.2} off {:.3}",
                centre[0],
                centre[1],
                centre[2],
                off(centre)
            ));
        }
    }
    tracing::debug!(
        target: "lucy::hits",
        "shot of {:08X} at frame {shot_frame} {view:?}: from ({ox:.3}, {oy:.3}, {oz:.3}) along ({dx:.4}, {dy:.4}, {dz:.4}){near}",
        shooter.source,
    );
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
