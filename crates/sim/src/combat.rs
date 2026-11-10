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

use std::{io, time::Duration};

use crate::{
    bots::{BOT_ENTITY, BOT_RECORD_DELAY, BOT_RESPAWN_TIME, Bot},
    hitscan::{HitscanShot, Impact, TargetHistory, TargetPose, ViewFrame},
    projectile_flight::{FlightEnd, ProjectileFlight},
    replication::WorldDelivery,
    scene::bot_create,
    scene_assets::PreparedScene,
};
use orpheus_content::{
    heroes::SOLDIER,
    maps::PRACTICE_RANGE_MODE,
    stats::{DEATHS, ELIMINATIONS, HEALING, HERO_DAMAGE, PlayerStats},
};
use orpheus_physics::static_ray::{Vec3, segment_distance};
use orpheus_script::{
    bot::DIED,
    vitals::{Death, Vitals},
    vm::ShotEvent,
    weapons::{HitDefinition, ProjectileBallistics},
    wire::{Value, Variables},
};
use orpheus_wire::{
    combat_messages::CombatNotice,
    commands::Command,
    entity::{EntityUpdate, Operation},
};

/// A hero waits this long in the PvP modes (0F4A and 0EC2 send 06FD.025
/// {10}: DATA; combat.py `RESPAWN`).
pub const RESPAWN: Duration = Duration::from_secs(10);
/// A hero waits this long in the Practice Range (13C0 sends 06FD.025 {3}:
/// DATA; combat.py `PRACTICE_RESPAWN`).
pub const PRACTICE_RESPAWN: Duration = Duration::from_secs(3);
/// Between respawn attempts while every body id of the player is still
/// reserved (combat.py `RESPAWN_RETRY`).
pub const RESPAWN_RETRY: Duration = Duration::from_millis(250);
/// The match's deferred messages between bodies per tick (match.py
/// `MAX_BODY_MESSAGES`).
pub const MAX_BODY_MESSAGES: usize = 256;
/// The movement-state flag of a dead body: the 3P animation's dead pose
/// (combat.py `DEAD`).
pub const DEAD: u32 = 0x2000;
/// Quick melee: graph 0043's ModifyHealth state, metres ahead of the eye and
/// the sphere's radius (combat.py `MELEE`, DATA).
pub const MELEE: [(u32, u16, f64, f64); 1] = [(0x0043, 4, 1.5, 1.0)];
/// 003E.025's parameters, by the low 16 bits of their 026 identifiers
/// (01CF rid4; combat.py `KILLER`, `CRIT`, `LOADOUT`, `ASSISTS`).
const KILLER: u32 = 0x0012;
const CRIT: u32 = 0x0095;
const LOADOUT: u32 = 0x03e6;
const ASSISTS: u32 = 0x03e7;

/// The mode's respawn time for heroes (combat.py `respawn_seconds`: the
/// Practice Range controller's 3 s, else the PvP 10 s).
pub fn respawn_time(mode: u64) -> Duration {
    if mode == PRACTICE_RANGE_MODE {
        PRACTICE_RESPAWN
    } else {
        RESPAWN
    }
}

/// The "died" message a dead body's script takes (combat.py
/// `_death_parameters`): the killer, whether the blow was critical, its
/// loadout and the assists (none).
pub fn death_message(killer: u32, critical: bool, loadout: Option<u64>) -> (u64, Variables) {
    (
        DIED,
        Variables::from([
            (KILLER, Value::Entity(killer)),
            (CRIT, Value::Bool(critical)),
            (LOADOUT, loadout.map_or(Value::Null, Value::Asset)),
            (ASSISTS, Value::Array(Vec::new())),
        ]),
    )
}

pub(crate) fn eye_height(hero: u64, crouched: bool) -> f64 {
    match (crouched, hero) {
        (false, _) => 1.55,
        (true, SOLDIER) => 1.085,
        (true, _) => 1.15,
    }
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
    pub shot_frame: u32,
    pub reach: f64,
    pub hit: HitDefinition,
}

/// A quick melee hit (combat.py `_melee`): a sphere ahead of the eye along
/// the command's aim, once per activation of graph 0043's st4.
#[derive(Clone, Copy, Debug)]
pub struct PosedMelee {
    pub source: u32,
    pub centre: [f64; 3],
    pub radius: f64,
    pub amount: f64,
    pub loadout: Option<u64>,
    pub command: Command,
}

pub enum PosedShot {
    Hitscan(PosedHitscan),
    Projectile(PosedProjectile),
    Melee(PosedMelee),
}

pub struct PosedProjectile {
    pub shot: ShotEvent,
    pub ballistics: ProjectileBallistics,
    pub origin: [f64; 3],
    pub direction: [f64; 3],
    /// The entity it leaves where it lands (projectiles.py `fired`: the
    /// Biotic Field 03DC), if any.
    pub spawn: Option<u64>,
    /// The shooter's feet when it fired (projectiles.py `Shot.where`).
    pub feet: [f64; 3],
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
    pub tick: u32,
}

pub struct BotHit {
    pub notices: [Option<CombatNotice>; 2],
    pub health: EntityUpdate,
    pub bot: u32,
    /// The bot's pools added up after the hit.
    pub remaining: f64,
    pub death: Option<BotDeath>,
}

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
}

#[derive(Clone)]
pub struct BotReplacement {
    pub retired: u32,
    pub created: EntityUpdate,
}

/// A player's body as a target this tick.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlayerTarget {
    pub body: u32,
    pub team: u8,
    pub pose: TargetPose,
    pub dead: bool,
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
        }
    }

    /// Keep where every body was this tick (combat.py `update`'s history):
    /// the players' current bodies, a new body with a history of its own
    /// that refuses shots aimed before it existed, and the live bots.
    pub fn record_poses(&mut self, scene: &PreparedScene, players: &[PlayerTarget], tick: u32) {
        let mut previous = std::mem::take(&mut self.players);
        self.players = players
            .iter()
            .map(|&target| {
                let mut history = previous
                    .iter()
                    .position(|slot| slot.target.body == target.body)
                    .map_or_else(
                        || TargetHistory::new(Some(tick)),
                        |index| previous.swap_remove(index).history,
                    );
                history.record(tick, target.pose);
                PlayerSlot { target, history }
            })
            .collect();
        for (target, bot) in self.targets.iter_mut().zip(scene.bots()) {
            if !bot.dead {
                target.history.record(tick, scene.bot_target(bot));
            }
        }
    }

    /// The hits on players' bodies since the last call, in order.
    pub fn take_player_hits(&mut self) -> Vec<PlayerHit> {
        std::mem::take(&mut self.player_hits)
    }

    /// The enemies' bodies a shooter can hit (combat.py `_enemies`): other
    /// players' living bodies of another team (any in a free-for-all), as
    /// `pose` places each. Bots come after them.
    fn player_candidates<'a>(
        &'a self,
        shooter: Shooter,
        free_for_all: bool,
        pose: impl Fn(&PlayerSlot) -> Option<TargetPose> + 'a,
    ) -> impl Iterator<Item = (u32, TargetPose)> + 'a {
        self.players.iter().filter_map(move |slot| {
            let target = slot.target;
            (target.body != shooter.source
                && (free_for_all || target.team != shooter.team)
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
        let ray = HitscanShot::from_command(shot.origin, &shot.command, shot.reach, shot.hit);
        self.fire_hitscan(
            scene,
            &ray,
            shooter,
            view,
            (shot.shot_frame, None),
            context,
            visible,
        )
    }

    /// combat.py `_shoot` for a firing bot's hitscan volley: from `origin`
    /// along `direction`, at the poses of this tick.
    #[allow(clippy::too_many_arguments)]
    pub fn bot_hitscan(
        &mut self,
        scene: &mut PreparedScene,
        shooter: Shooter,
        origin: Vec3,
        direction: Vec3,
        reach: f64,
        hit: HitDefinition,
        context: ShotContext,
    ) -> io::Result<Option<BotHit>> {
        let ray = HitscanShot::along(origin, direction, reach, hit);
        self.fire_hitscan(
            scene,
            &ray,
            shooter,
            ViewFrame::at(context.tick),
            (context.tick, Some(context.tick)),
            context,
            |_| true,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn fire_hitscan(
        &mut self,
        scene: &mut PreparedScene,
        ray: &HitscanShot,
        shooter: Shooter,
        view: ViewFrame,
        (shot_frame, world_tick): (u32, Option<u32>),
        context: ShotContext,
        visible: impl Fn(u32) -> bool,
    ) -> io::Result<Option<BotHit>> {
        let free_for_all = scene.map().free_for_all;
        let players: Vec<_> = self
            .player_candidates(shooter, free_for_all, |slot| {
                slot.history.at(view, slot.target.pose)
            })
            .collect();
        let bots = scene
            .bots()
            .iter()
            .zip(&self.targets)
            .filter_map(|(bot, target)| {
                if !eligible_bot(bot, shooter, free_for_all) || !visible(bot.entity) {
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
                |origin, direction, reach| scene.hitscan_wall(origin, direction, reach),
            )
            .map_err(io::Error::other)?
        else {
            return Ok(None);
        };
        Ok(self.apply_impact(scene, shooter, shot_frame, world_tick, context, &impact))
    }

    /// combat.py `_melee`: every enemy whose capsule, as the shooter's client
    /// showed it, comes within the sphere takes the hit.
    pub fn melee(
        &mut self,
        scene: &mut PreparedScene,
        melee: PosedMelee,
        shooter: Shooter,
        context: ShotContext,
        visible: impl Fn(u32) -> bool,
    ) -> Vec<BotHit> {
        if melee.amount <= 0.0 {
            return Vec::new();
        }
        let view = ViewFrame::from_command(&melee.command, context.quanta);
        let free_for_all = scene.map().free_for_all;
        let reaches = |pose: TargetPose| {
            let [a, b] = pose.segment();
            segment_distance(melee.centre, a, b) - pose.radius <= melee.radius
        };
        let players: Vec<_> = self
            .player_candidates(shooter, free_for_all, |slot| {
                slot.history.at(view, slot.target.pose)
            })
            .filter(|&(_, pose)| reaches(pose))
            .collect();
        let bots: Vec<_> = scene
            .bots()
            .iter()
            .zip(&self.targets)
            .filter(|(bot, _)| eligible_bot(bot, shooter, free_for_all) && visible(bot.entity))
            .filter_map(|(bot, target)| {
                let pose = target.history.at(view, scene.bot_target(bot))?;
                reaches(pose).then_some(bot.entity)
            })
            .collect();
        let frame = melee.command.frame;
        players
            .into_iter()
            .map(|(entity, _)| entity)
            .chain(bots)
            .filter_map(|entity| {
                let impact = Impact {
                    entity,
                    distance: 0.0,
                    damage: melee.amount,
                    critical: false,
                    loadout: melee.loadout,
                };
                self.apply_impact(scene, shooter, frame, None, context, &impact)
            })
            .collect()
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
        let candidates: Vec<_> = self
            .player_candidates(shooter, free_for_all, |slot| Some(slot.target.pose))
            .chain(
                scene
                    .bots()
                    .iter()
                    .filter(|bot| eligible_bot(bot, shooter, free_for_all))
                    .map(|bot| (bot.entity, scene.bot_target(bot))),
            )
            .collect();
        let end = flight
            .advance(context.tick, &candidates, |origin, direction, reach| {
                scene.hitscan_wall(origin, direction, reach)
            })
            .map_err(io::Error::other)?;
        if let Some(end) = &end {
            hits.extend(end.impacts.iter().filter_map(|impact| {
                self.apply_impact(
                    scene,
                    shooter,
                    context.tick,
                    Some(context.tick),
                    context,
                    impact,
                )
            }));
        }
        Ok(end)
    }

    /// combat.py `_shoot` for a firing bot's projectile volley: a rocket
    /// flown by the room from this tick on (`Rocket(..., last_tick=tick)`).
    pub fn fire_bot_projectile(
        &mut self,
        shooter: Shooter,
        projectile: PosedProjectile,
        tick: u32,
    ) {
        let damaging = projectile
            .ballistics
            .direct
            .is_some_and(|hit| hit.amount > 0.0)
            || projectile
                .ballistics
                .blast
                .as_ref()
                .is_some_and(|blast| blast.hit.amount > 0.0);
        if !damaging {
            return;
        }
        if let Some(flight) = ProjectileFlight::new(
            projectile.ballistics,
            projectile.origin,
            projectile.direction,
            Some(tick),
        ) {
            self.rockets.push(BotRocket { shooter, flight });
        }
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
            match self.step_projectile(
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
            }
        });
        rockets.append(&mut self.rockets);
        self.rockets = rockets;
        failed.map_or(Ok(hits), Err)
    }

    /// combat.py `_damage`: a bot's pools change here; a player's hit waits
    /// for the match.
    fn apply_impact(
        &mut self,
        scene: &mut PreparedScene,
        shooter: Shooter,
        shot_frame: u32,
        world_tick: Option<u32>,
        context: ShotContext,
        impact: &Impact,
    ) -> Option<BotHit> {
        if self
            .players
            .iter()
            .any(|slot| slot.target.body == impact.entity && !slot.target.dead)
        {
            if impact.damage > 0.0 {
                self.player_hits.push(PlayerHit {
                    body: impact.entity,
                    shooter,
                    amount: impact.damage,
                    critical: impact.critical,
                    loadout: impact.loadout,
                    frame: shot_frame,
                    world_tick,
                });
            }
            return None;
        }
        self.apply_bot_impact(scene, shooter, shot_frame, world_tick, context, impact)
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
        let (bot, target) = scene
            .bots_mut()
            .iter_mut()
            .zip(&mut self.targets)
            .find(|(bot, _)| !bot.dead && bot.entity == impact.entity)?;
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
        if result.killed {
            bot.died(death_tick);
            target.respawn_at = Some(context.now.saturating_add(BOT_RESPAWN_TIME));
        }
        let health = EntityUpdate {
            entity: bot.entity,
            operation: Some(Operation::Update(vec![target.vitals.component()])),
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
            death: result.killed.then_some(BotDeath {
                bot: bot.entity,
                killer: shooter.source,
                critical: impact.critical,
                loadout: impact.loadout,
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
            let created = bot_create(&fresh, None)?;
            prepared.push((
                slot,
                fresh,
                BotReplacement {
                    retired: bot.entity,
                    created,
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
    /// capsule). The players' bodies come back for the match to heal; the
    /// bots heal here, with what they took.
    pub fn heal_area(
        &mut self,
        scene: &mut PreparedScene,
        healer: Shooter,
        centre: Vec3,
        radius: f64,
        amount: f64,
    ) -> (Vec<u32>, Vec<(BotHit, f64)>) {
        let free_for_all = scene.map().free_for_all;
        let reaches = |pose: TargetPose| {
            let [a, b] = pose.segment();
            segment_distance(centre, a, b) - pose.radius <= radius
        };
        let players = self
            .players
            .iter()
            .map(|slot| slot.target)
            .filter(|target| {
                let own = target.body & !0xff == healer.source & !0xff;
                let ally = !free_for_all && target.team == healer.team;
                (own || ally) && !target.dead && reaches(target.pose)
            })
            .map(|target| target.body)
            .collect();
        let mut bots = Vec::new();
        for slot in 0..self.targets.len() {
            let bot = &scene.bots()[slot];
            let own = bot.entity == healer.source;
            let ally = !free_for_all && bot.spawn.team == healer.team;
            if bot.dead || !(own || ally) || !reaches(scene.bot_target(bot)) {
                continue;
            }
            let target = &mut self.targets[slot];
            let given = target.vitals.heal(amount);
            if given <= 0.0 {
                continue;
            }
            let health = EntityUpdate {
                entity: bot.entity,
                operation: Some(Operation::Update(vec![target.vitals.component()])),
                ..EntityUpdate::default()
            };
            target.vitals.clear_changed();
            bots.push((
                BotHit {
                    notices: [None, None],
                    health,
                    bot: bot.entity,
                    remaining: target.vitals.total(),
                    death: None,
                },
                given,
            ));
        }
        (players, bots)
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
/// `_died`, `_kill_notice`): the victim's pools take each; a player
/// shooter gets 25001 and HERO_DAMAGE; a kill counts DEATHS for the victim,
/// ELIMINATIONS and 25005 for another player who killed him, and gives the
/// victim 25004 ("YOU WERE KILLED BY", or "YOU DIED" for his own blow).
/// Returns each player's notices, by slot.
pub fn apply_player_hits(
    scene: &PreparedScene,
    hits: Vec<PlayerHit>,
    players: &mut [CombatPlayer<'_>],
    tick: u32,
    now: Duration,
) -> Vec<Vec<CombatNotice>> {
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
        if let Some(shooter) = shooter {
            notices[shooter].push(CombatNotice::Hit {
                target: hit.body,
                dealt: damage.dealt,
                critical: hit.critical,
                killed: damage.killed,
                shot_frame: hit.frame,
            });
            players[shooter].stats.add(HERO_DAMAGE, damage.dealt);
        }
        if !damage.killed {
            continue;
        }
        players[victim].stats.add(DEATHS, 1.0);
        let own_blow = shooter == Some(victim);
        if let Some(killer) = shooter.filter(|_| !own_blow) {
            players[killer].stats.add(ELIMINATIONS, 1.0);
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
/// HEALING counts what they took, with what the bots took (`bots_healed`).
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
                    healed += world.heal_body(scene, body, amount);
                }
            }
        }
    }
    if let Some(healer) = player_of(players, healer).filter(|_| healed > 0.0) {
        players[healer].stats.add(HEALING, healed);
    }
}

/// match.py `deliver_body_messages`: the messages the players' bodies sent
/// go to the bodies they name, in the room, at most MAX_BODY_MESSAGES a
/// tick. A message a body sends while it takes one goes in the next batch.
pub fn deliver_body_messages(worlds: &mut [&mut WorldDelivery]) {
    let messages: Vec<_> = worlds
        .iter_mut()
        .flat_map(|world| world.take_body_messages())
        .collect();
    if messages.len() > MAX_BODY_MESSAGES {
        eprintln!(
            "match body message queue reached {MAX_BODY_MESSAGES}; {} dropped",
            messages.len() - MAX_BODY_MESSAGES
        );
    }
    for message in messages.into_iter().take(MAX_BODY_MESSAGES) {
        for world in worlds.iter_mut() {
            if world.receive_body_message(&message) {
                break;
            }
        }
    }
}

fn eligible_bot(bot: &Bot, shooter: Shooter, free_for_all: bool) -> bool {
    !bot.dead && bot.entity != shooter.source && (free_for_all || bot.spawn.team != shooter.team)
}

#[cfg(test)]
mod tests;
