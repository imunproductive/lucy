//! The training bots' statescript: each bot's body script on the server
//! clock, its weapons and its death, and what every client gets of it.
//!
//! The room side follows `ow174/game/match.py` (`_prepare_bot_script`,
//! `_pending_bot_scripts`) and `ow174/game/combat.py` (`_bot_weapons`,
//! `_bot_died`, `_advance_bot_death`, `_respawn_bot`); the script itself is
//! [`BotBody`] (`ow174/game/script/bot.py`). The client side follows
//! `ow174/game/server.py` (`_BotScriptDelivery`, `queue_bot_live`,
//! `queue_bot_death`, `queue_bot_death_update`, `_flush_bot_frames`,
//! `_project_bot_snapshot`, `_flush_bot_nameplates`, `_queue_bot_snapshot`).

use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque},
    sync::Arc,
};

use crate::{
    bot_plate::{BotPlate, INPUTS, PlateVariable},
    bots::BOT_EYE,
    bots::Bot,
    combat::{
        BotDamage, BotDeath, PosedProjectile, damage_direction, damage_message, death_message,
    },
    projectiles::{
        DELIVERY_GRACE, ObservedOut, ObservedProjectile, predicted_sync, projectiles_per_shot,
        spawned_definitions,
    },
    replication::kill_feed_fresh,
    voice_lines::{VoiceLine, VoiceState, is_death_cry},
};
use orpheus_content::{
    maps::{GameMap, PRACTICE_RANGE_MAP, PRACTICE_RANGE_MODE},
    voice_sets::VoiceCatalog,
};
use orpheus_script::{
    bot::{BotBody, BotError, BotSpawn as ScriptSpawn, RANGE_BOT, practice_nameplate},
    codecs::CodecTable,
    graph::GraphStore,
    snapshot::{ProjectedSnapshot, SnapshotEncoder, SnapshotError, View},
    stream::{BodyStream, ChangeKey, DeltaKind, ScriptChunk},
    timeline::FRAME_MS,
    vm::{
        AiMoveRequest, AuraChange, BodyEffects, HealthChange, ShotEvent, Snapshot, TargetCandidate,
        Unknowns,
    },
    weapons::{HitDefinition, ProjectileBallistics, authored_number, ray_profile},
    wire::Value,
};
use orpheus_wire::entity::{EntityUpdate, MAX_PAYLOAD, frame};

/// How long a dead bot's death script runs: `int(BOT_RESPAWN / TICK)`
/// frames, 3 s of 16 ms frames rounded down (combat.py `_advance_bot_death`).
const DEATH_LIFETIME_FRAMES: u32 = 187;
/// At most about a second of missed frames replays the firing bots' weapons
/// (combat.py `COMBAT_CATCH_UP`).
const COMBAT_CATCH_UP: u32 = 64;
/// The metres a hitscan volley reaches when it names none (combat.py `RANGE`,
/// unverified).
const RANGE: f64 = 200.0;

/// A bot's observer projection at one frame. In the Practice Range it also
/// carries the same frame without the practice nameplate, which a client
/// gets until its mode UI (0CB6, whose #14206 the client-only 0D24 reads)
/// has arrived (server.py `_project_bot_snapshot`).
#[derive(Debug)]
pub struct BotFrame {
    full: ProjectedSnapshot,
    plain: Option<Plain>,
}

#[derive(Debug)]
struct Plain {
    child: u16,
    descriptor: (u32, Option<(u16, u16)>),
    stripped: ProjectedSnapshot,
}

/// What the room's bot scripts produced since the clients last looked.
#[derive(Clone, Debug)]
pub enum BotEvent {
    /// A bot's changed live frame: a firing bot's (combat.py
    /// `queue_bot_live`), the nameplate a hit enabled, or the bot's
    /// reaction to a hit.
    Live { entity: u32, frame: Arc<BotFrame> },
    /// The bot died; its script's immediate change, if any
    /// (`queue_bot_death`).
    Death {
        entity: u32,
        frame: Option<Arc<BotFrame>>,
    },
    /// A later frame of the death script (`queue_bot_death_update`).
    DeathUpdate { entity: u32, frame: Arc<BotFrame> },
}

/// A firing bot's shot: the match flies it against the players and the
/// other team's bots (combat.py `_shoot` for an NPC shooter).
pub struct BotShot {
    pub source: u32,
    pub team: u8,
    /// The world tick it was fired in.
    pub tick: u32,
    pub kind: BotShotKind,
    /// The other clients see it (projectiles.py `fired`: `_show_others`).
    pub observed: bool,
    /// The bot's live frame after the advance that fired it, else its
    /// latest (combat.py `_bot_weapons`: `snapshot or script.latest_live`).
    pub frame: Arc<BotFrame>,
}

// A few shots a tick: boxing the projectile buys nothing.
#[allow(clippy::large_enum_variant)]
pub enum BotShotKind {
    Projectile(PosedProjectile),
    Hitscan {
        origin: [f64; 3],
        direction: [f64; 3],
        reach: f64,
        hit: HitDefinition,
        /// The volley's ray profile (`orpheus_script::weapons::ray_profile`).
        ray_profile: Option<u64>,
        shot_frame: u32,
    },
}

struct BotScript {
    entity: u32,
    body: BotBody,
    /// The lifetime's first projection: later ones keep its exclusions.
    initial: ProjectedSnapshot,
    latest_live: Arc<BotFrame>,
    /// The last projection handed out (`BotScript._produced`).
    produced: ProjectedSnapshot,
    /// The world tick of the death and the killer's entity
    /// (`Vitals.death_tick`, `Vitals.killer`).
    death: Option<(u32, u32)>,
    /// The lifetime's voice lines ([`crate::voice_lines`]).
    voice: VoiceState,
    /// What each variable the live nameplate tool set had before it (none:
    /// never set), to put back when the tool lets it go.
    plate: BTreeMap<PlateVariable, Option<Value>>,
}

/// Every training bot's script in one room, one slot per spawner.
pub struct BotScripts {
    graphs: Option<Arc<GraphStore>>,
    codecs: Arc<CodecTable>,
    unknowns: Unknowns,
    mode: u64,
    practice: bool,
    slots: Vec<Option<BotScript>>,
    /// The entity each slot's script was last prepared for.
    prepared: Vec<Option<u32>>,
    pending: VecDeque<usize>,
    events: Vec<BotEvent>,
    /// The last world tick the weapons ran (combat.py `Combat.tick`).
    weapons_tick: u32,
    /// The bodies the target searches can find, as the match last placed
    /// them; a new script starts with them.
    targets: Vec<TargetCandidate>,
    /// The bodies' voice sets.
    voice_sets: Arc<VoiceCatalog>,
    /// The lines the bots said since the match last looked.
    voices: Vec<(VoiceLine, [f64; 3])>,
    /// The health changes the bots' graphs made (their auras' heals and
    /// damage) since the match last looked.
    health_changes: Vec<HealthChange>,
}

impl BotScripts {
    /// The bots whose body has a graph in the bot bundle wait for their
    /// script (match.py `_pending_bot_scripts`).
    pub fn new(
        graphs: Option<Arc<GraphStore>>,
        codecs: Arc<CodecTable>,
        unknowns: Unknowns,
        voice_sets: Arc<VoiceCatalog>,
        map: &GameMap,
        bots: &[Bot],
    ) -> Self {
        let pending = graphs.as_ref().map_or_else(VecDeque::new, |graphs| {
            bots.iter()
                .enumerate()
                .filter(|(_, bot)| graphs.entity_body(bot.spawn.body).is_some())
                .map(|(slot, _)| slot)
                .collect()
        });
        Self {
            graphs,
            codecs,
            unknowns,
            mode: map.mode_guid,
            practice: map.map_guid == PRACTICE_RANGE_MAP && map.mode_guid == PRACTICE_RANGE_MODE,
            slots: bots.iter().map(|_| None).collect(),
            prepared: vec![None; bots.len()],
            pending,
            events: Vec::new(),
            weapons_tick: 0,
            targets: Vec::new(),
            voice_sets,
            voices: Vec::new(),
            health_changes: Vec::new(),
        }
    }

    /// Every bot that has a script has had it prepared (match.py: the world
    /// waits for `_pending_bot_scripts`).
    pub fn ready(&self) -> bool {
        self.pending.is_empty()
    }

    /// The number of bots still waiting for their script.
    pub fn pending(&self) -> usize {
        self.pending.len()
    }

    pub fn running(&self) -> usize {
        self.slots.iter().flatten().count()
    }

    /// The script of the bot in `slot` as of its last run, while it is
    /// that bot's (`entity`).
    pub(crate) fn snapshot(&self, slot: usize, entity: u32) -> Option<&Snapshot> {
        let script = self.slots.get(slot)?.as_ref()?;
        (script.entity == entity)
            .then(|| script.body.snapshot())
            .flatten()
    }

    /// One script per update, within the loading window and off the
    /// world-create burst (match.py `update`).
    pub(crate) fn prepare_next(&mut self, bots: &[Bot], tick: u32, plate: &BotPlate) {
        if let Some(slot) = self.pending.pop_front() {
            self.prepare(slot, &bots[slot], tick, plate);
        }
    }

    /// The bodies every script's target searches can find from now on.
    pub(crate) fn set_target_candidates(&mut self, candidates: &[TargetCandidate]) {
        self.targets.clear();
        self.targets.extend_from_slice(candidates);
        for script in self.slots.iter_mut().flatten() {
            script.body.set_target_candidates(candidates);
        }
    }

    /// A respawned bot gets a new script under its new entity
    /// (combat.py `_respawn_bot`).
    pub(crate) fn prepare_respawned(&mut self, bots: &[Bot], tick: u32, plate: &BotPlate) {
        let Some(graphs) = &self.graphs else {
            return;
        };
        let due: Vec<usize> = bots
            .iter()
            .enumerate()
            .filter(|&(slot, bot)| {
                self.prepared[slot].is_some_and(|entity| entity != bot.entity)
                    && graphs.entity_body(bot.spawn.body).is_some()
            })
            .map(|(slot, _)| slot)
            .collect();
        for slot in due {
            self.prepare(slot, &bots[slot], tick, plate);
        }
    }

    /// match.py `_prepare_bot_script`: a firing bot of the Practice Range
    /// starts its firing behavior at its spawner, a bot whose spawner lists
    /// the patrol its patrol (`Bot::patrol`). The live nameplate tool's
    /// variables are set before the first frame. A bot made again after a
    /// death puts itself together ([`ScriptSpawn::respawned`]).
    fn prepare(&mut self, slot: usize, bot: &Bot, tick: u32, plate: &BotPlate) {
        let respawned = self.prepared[slot].is_some_and(|entity| entity != bot.entity);
        self.prepared[slot] = Some(bot.entity);
        self.slots[slot] = None;
        let Some(graphs) = &self.graphs else {
            return;
        };
        let spawn = ScriptSpawn {
            body: bot.spawn.body,
            hero: bot.spawn.hero,
            name: &bot.spawn.name,
            entity: bot.entity,
            frame: tick,
            mode: self.mode,
            health: bot.spawn.health,
            firing_position: (bot.spawn.firing && self.practice).then_some(bot.position),
            patrol: bot.patrol(),
            respawned,
        };
        let mut originals = BTreeMap::new();
        let started = BotBody::start(Arc::clone(graphs), spawn, self.unknowns)
            .map_err(|error| error.to_string())
            .and_then(|mut body| {
                tune(&mut body, &mut originals, plate).map_err(|error| error.to_string())?;
                body.set_target_candidates(&self.targets);
                let snapshot = body.snapshot().ok_or("the script halted")?;
                let initial = body
                    .encoder(graphs, &self.codecs)
                    .project(snapshot, View::Observer)
                    .map_err(|error| error.to_string())?;
                let frame = self
                    .frame(&body, snapshot, &initial)
                    .map_err(|error| error.to_string())?;
                Ok(BotScript {
                    entity: bot.entity,
                    produced: frame.full.clone(),
                    latest_live: Arc::new(frame),
                    initial,
                    body,
                    death: None,
                    voice: VoiceState::default(),
                    plate: originals,
                })
            });
        match started {
            Ok(script) => self.slots[slot] = Some(script),
            Err(reason) => eprintln!(
                "{}'s statescript could not start ({reason})",
                bot.spawn.name
            ),
        }
    }

    /// The live nameplate tool changed its variables: each live bot's
    /// script takes them now, and its changed frame goes to the clients.
    pub(crate) fn apply_plate(&mut self, bots: &[Bot], plate: &BotPlate) {
        for (slot, bot) in bots.iter().enumerate() {
            let Some(script) = self.slots[slot]
                .as_mut()
                .filter(|script| !bot.dead && script.entity == bot.entity)
            else {
                continue;
            };
            let result = tune(&mut script.body, &mut script.plate, plate)
                .map_err(|error| error.to_string())
                .and_then(|()| self.publish_live(slot));
            if let Err(reason) = result {
                eprintln!(
                    "bot {:08X}'s body script failed on the plate tool ({reason})",
                    bot.entity
                );
                self.slots[slot] = None;
            }
        }
    }

    /// The live nameplate tool's game message to every live bot's
    /// nameplate instances; their changed frames go to the clients. The
    /// number of bots that took it.
    pub(crate) fn message_plates(&mut self, bots: &[Bot], message: u64) -> usize {
        let mut taken = 0;
        for (slot, bot) in bots.iter().enumerate() {
            let Some(script) = self.slots[slot]
                .as_mut()
                .filter(|script| !bot.dead && script.entity == bot.entity)
            else {
                continue;
            };
            let result = script
                .body
                .message_nameplates(message)
                .map_err(|error| error.to_string())
                .and_then(|took| {
                    taken += usize::from(took);
                    if took {
                        self.publish_live(slot)
                    } else {
                        Ok(())
                    }
                });
            if let Err(reason) = result {
                eprintln!(
                    "bot {:08X}'s body script failed on the plate tool ({reason})",
                    bot.entity
                );
                self.slots[slot] = None;
            }
        }
        taken
    }

    /// What a live bot's plate variables read now: [`INPUTS`] and every
    /// other one the plate sets, each with whether the plate sets it. None
    /// without its script.
    pub(crate) fn plate_variables(
        &self,
        entity: u32,
        plate: &BotPlate,
    ) -> Option<Vec<(PlateVariable, Option<Value>, bool)>> {
        let script = self.script(entity)?;
        let mut variables: Vec<PlateVariable> = INPUTS.iter().map(|input| input.variable).collect();
        variables.extend(
            plate
                .variables()
                .map(|(variable, _)| variable)
                .filter(|variable| INPUTS.iter().all(|input| input.variable != *variable)),
        );
        Some(
            variables
                .into_iter()
                .map(|variable| {
                    let value = match variable {
                        PlateVariable::Entity(id) => script.body.resolved(false, id),
                        PlateVariable::Nameplate(id) => script.body.resolved(true, id),
                    };
                    let tuned = plate.variables().any(|(set, _)| set == variable);
                    (variable, value, tuned)
                })
                .collect(),
        )
    }

    fn frame(
        &self,
        body: &BotBody,
        snapshot: &Snapshot,
        initial: &ProjectedSnapshot,
    ) -> Result<BotFrame, SnapshotError> {
        let graphs = self.graphs.as_deref().expect("a running bot script");
        project_frame(
            graphs,
            &self.codecs,
            self.unknowns,
            self.practice,
            body.behaviors(),
            snapshot,
            initial,
        )
    }

    /// combat.py `_bot_weapons`: each firing bot's script advances on the
    /// world clock, frame by frame across a gap of missed ticks; its volley
    /// reports become shots and its changed frames go to the clients.
    pub(crate) fn weapons(&mut self, bots: &[Bot], tick: u32) -> Vec<BotShot> {
        let firing = bots
            .iter()
            .zip(&self.slots)
            .any(|(bot, script)| bot.spawn.firing && !bot.dead && script.is_some());
        let mut first = tick;
        if self.weapons_tick != 0 && tick > self.weapons_tick + 1 && firing {
            first = (self.weapons_tick + 1).max((tick + 1).saturating_sub(COMBAT_CATCH_UP));
        }
        self.weapons_tick = tick;
        let mut shots = Vec::new();
        for at in first..=tick {
            for (slot, bot) in bots.iter().enumerate() {
                if !bot.spawn.firing || bot.dead {
                    continue;
                }
                if let Err(reason) = self.fire(slot, bot, at, &mut shots) {
                    eprintln!("{}'s weapon script failed ({reason})", bot.spawn.name);
                    self.slots[slot] = None;
                }
            }
        }
        shots
    }

    fn fire(
        &mut self,
        slot: usize,
        bot: &Bot,
        at: u32,
        shots: &mut Vec<BotShot>,
    ) -> Result<(), String> {
        let Some(script) = self.slots[slot]
            .as_mut()
            .filter(|script| script.body.frame() <= at)
        else {
            return Ok(());
        };
        script.body.set_movement_sample(bot.movement_sample());
        if !script
            .body
            .advance_live(at, Some(bot.position))
            .map_err(|error| error.to_string())?
        {
            return Ok(());
        }
        let mut fired = Vec::new();
        for shot in script.body.take_shots() {
            if let Some(found) = shot_kind(&mut script.body, &shot, bot) {
                fired.push(found);
            }
        }
        let script = self.slots[slot].as_ref().expect("a running bot script");
        let frame = script
            .body
            .snapshot()
            .ok_or_else(|| "the script halted".to_string())
            .and_then(|snapshot| {
                self.frame(&script.body, snapshot, &script.initial)
                    .map_err(|error| error.to_string())
            });
        let script = self.slots[slot].as_mut().expect("a running bot script");
        let changed = match frame {
            Ok(frame) if frame.full != script.produced => {
                script.produced = frame.full.clone();
                script.latest_live = Arc::new(frame);
                true
            }
            Ok(_) => false,
            Err(reason) => return Err(reason),
        };
        shots.extend(fired.into_iter().map(|(kind, observed)| BotShot {
            source: bot.entity,
            team: bot.spawn.team,
            tick: at,
            kind,
            observed,
            frame: Arc::clone(&script.latest_live),
        }));
        if changed {
            self.events.push(BotEvent::Live {
                entity: script.entity,
                frame: Arc::clone(&script.latest_live),
            });
        }
        Ok(())
    }

    /// combat.py `_bot_died`'s script part: the death message 003E.025
    /// ([`death_message`]); every client registers the death, with the
    /// script's immediate change if it made one. `death.tick` is the world
    /// tick of the death.
    pub(crate) fn died(&mut self, death: &BotDeath) {
        let BotDeath {
            bot: entity,
            killer,
            tick,
            ..
        } = *death;
        let Some(slot) = self.slot(entity) else {
            return;
        };
        let script = self.slots[slot].as_mut().expect("a running bot script");
        script.death = Some((tick, killer));
        let (_, parameters) = death_message(killer, death.critical, death.loadout, death.tags);
        let frame = tick.max(script.body.frame());
        let result = script
            .body
            .die(frame, parameters)
            .map_err(|error| error.to_string())
            .and_then(|took| {
                if !took {
                    return Ok(None);
                }
                let script = self.slots[slot].as_ref().expect("a running bot script");
                let snapshot = script.body.snapshot().ok_or("the script halted")?;
                let frame = self
                    .frame(&script.body, snapshot, &script.initial)
                    .map_err(|error| error.to_string())?;
                Ok((frame.full != script.produced).then_some(frame))
            });
        match result {
            Ok(frame) => {
                let script = self.slots[slot].as_mut().expect("a running bot script");
                let frame = frame.map(|frame| {
                    script.produced = frame.full.clone();
                    Arc::new(frame)
                });
                self.events.push(BotEvent::Death { entity, frame });
            }
            Err(reason) => {
                eprintln!("bot {entity:08X}'s body script failed on 003E ({reason})");
                self.slots[slot] = None;
            }
        }
    }

    /// combat.py `_advance_bot_death`: a dead bot's script runs its due
    /// events within the death's lifetime; each changed frame goes out.
    pub(crate) fn advance_deaths(&mut self, bots: &[Bot], tick: u32) {
        for (slot, bot) in bots.iter().enumerate() {
            if !bot.dead {
                continue;
            }
            let Some(script) = self.slots[slot]
                .as_mut()
                .filter(|script| script.body.death_frame().is_some())
            else {
                continue;
            };
            let graphs = self.graphs.as_deref().expect("a running bot script");
            let BotScript {
                entity,
                body,
                initial,
                produced,
                ..
            } = script;
            let mut failure = None;
            let mut frames = Vec::new();
            let result = body.advance_death(tick, DEATH_LIFETIME_FRAMES, |_, snapshot| {
                if failure.is_some() {
                    return;
                }
                match project_frame(
                    graphs,
                    &self.codecs,
                    self.unknowns,
                    self.practice,
                    &[],
                    snapshot,
                    initial,
                ) {
                    Ok(frame) if frame.full != *produced => {
                        *produced = frame.full.clone();
                        frames.push(Arc::new(frame));
                    }
                    Ok(_) => {}
                    Err(error) => failure = Some(error.to_string()),
                }
            });
            let entity = *entity;
            self.events.extend(
                frames
                    .into_iter()
                    .map(|frame| BotEvent::DeathUpdate { entity, frame }),
            );
            if let Some(reason) = result.err().map(|error| error.to_string()).or(failure) {
                eprintln!("bot {entity:08X}'s death script timer failed ({reason})");
                self.slots[slot] = None;
            }
        }
    }

    /// combat.py `_sync_body_health`: the script's #253 follows the bot's
    /// pools after a hit that didn't kill it.
    pub(crate) fn set_health(&mut self, entity: u32, health: f64) {
        let Some(slot) = self.slot(entity) else {
            return;
        };
        let script = self.slots[slot].as_mut().expect("a running bot script");
        if let Err(error) = script.body.set_health(health) {
            eprintln!("bot {entity:08X}'s body script failed on its health ({error})");
            self.slots[slot] = None;
        }
    }

    /// A hit the bot survived reaches its script: `0011.025` with the
    /// damage's tags, source, amount and direction
    /// ([`crate::combat::damage_message`]); the changed frame (its flinch)
    /// goes to the clients as a live frame. The direction runs from the
    /// attacker's feet to the bot's, as the match last placed them
    /// (assumption; none when the attacker isn't found).
    pub(crate) fn hurt(&mut self, entity: u32, damage: &BotDamage, bots: &[Bot]) {
        let Some(slot) = self.slot(entity) else {
            return;
        };
        let feet = |entity: u32| {
            self.targets
                .iter()
                .find(|candidate| candidate.entity == entity)
                .map(|candidate| candidate.feet)
                .or_else(|| {
                    bots.iter()
                        .find(|bot| bot.entity == entity)
                        .map(|bot| bot.position)
                })
        };
        let direction =
            feet(damage.source).and_then(|from| damage_direction(from, bots[slot].position));
        let (_, parameters) = damage_message(damage.source, damage.dealt, damage.tags, direction);
        let script = self.slots[slot].as_mut().expect("a running bot script");
        let result = script
            .body
            .take_damage(damage.tick, parameters)
            .map_err(|error| error.to_string())
            .and_then(|took| {
                if took {
                    self.publish_live(slot)
                } else {
                    Ok(())
                }
            });
        if let Err(reason) = result {
            eprintln!("bot {entity:08X}'s body script failed on its damage ({reason})");
            self.slots[slot] = None;
        }
    }

    /// An aura comes on the live bot or goes off it (`vm/auras.rs` in
    /// `orpheus_script`: Biotic Grenade's `0267`, an ApplyAura's) at world
    /// tick `tick`; its changed frame goes to the clients as a live frame.
    pub(crate) fn change_aura(&mut self, change: &AuraChange, tick: u32) {
        let entity = change.target();
        let Some(slot) = self.slot(entity) else {
            return;
        };
        let script = self.slots[slot].as_mut().expect("a running bot script");
        let result = script
            .body
            .change_aura(tick, change)
            .map_err(|error| error.to_string())
            .and_then(|took| {
                let script = self.slots[slot].as_mut().expect("a running bot script");
                self.health_changes
                    .extend(script.body.take_health_changes());
                if took {
                    self.publish_live(slot)
                } else {
                    Ok(())
                }
            });
        if let Err(reason) = result {
            eprintln!("bot {entity:08X}'s body script failed on an aura ({reason})");
            self.slots[slot] = None;
        }
    }

    /// The health changes the bots' graphs made since the last call, in
    /// order.
    pub fn take_health_changes(&mut self) -> Vec<HealthChange> {
        std::mem::take(&mut self.health_changes)
    }

    /// The aura changes the bots' ApplyAura states made since the last
    /// call, in order.
    pub fn take_aura_changes(&mut self) -> Vec<AuraChange> {
        self.slots
            .iter_mut()
            .flatten()
            .flat_map(|script| script.body.take_aura_changes())
            .collect()
    }

    /// The statuses and combat modifiers the live bot's states hold.
    pub fn effects(&self, entity: u32) -> Option<&BodyEffects> {
        let script = self.slots.get(self.slot(entity)?)?.as_ref()?;
        Some(&script.body.world().effects)
    }

    /// The live bots' graph events due by world tick `tick` (a hit's flinch
    /// and pain waits): a bot the weapons didn't run this tick runs the
    /// tick when something is due, and its changed frame goes out. Then the
    /// criteria every bot raised become its voice lines
    /// ([`crate::voice_lines`]).
    pub(crate) fn advance_live(&mut self, bots: &[Bot], tick: u32) {
        for (slot, bot) in bots.iter().enumerate() {
            let Some(script) = self.slots[slot].as_mut() else {
                continue;
            };
            // A changed movement sample runs the frame too: its sampler
            // follows the body each frame (a knocked bot's landing).
            let resampled = script.body.set_movement_sample(bot.movement_sample());
            if !bot.dead && script.body.frame() < tick && (resampled || script.body.due_by(tick)) {
                let result = script
                    .body
                    .advance_live(tick, Some(bot.position))
                    .map_err(|error| error.to_string())
                    .and_then(|ran| if ran { self.publish_live(slot) } else { Ok(()) });
                if let Err(reason) = result {
                    eprintln!("bot {:08X}'s body script failed ({reason})", bot.entity);
                    self.slots[slot] = None;
                    continue;
                }
            }
            let script = self.slots[slot].as_mut().expect("a running bot script");
            self.health_changes
                .extend(script.body.take_health_changes());
            for criteria in script.body.take_criteria() {
                // A dead bot says only its death cries: `0C17`'s `003E`
                // branch (rid8) raises `05FB` for a killing hit tagged
                // `0644`, `1C92` for `074E`, 0.05 s after the death.
                if bot.dead && !is_death_cry(criteria.identifier) {
                    continue;
                }
                let set = self.voice_sets.for_body(bot.spawn.body);
                if let Some(voice) = script.voice.answer(set, script.entity, criteria, tick) {
                    self.voices.push((voice, bot.position));
                }
            }
        }
    }

    /// The point the bot in `slot` (while its script is `entity`'s) is
    /// asked to walk to now: its patrol's movement state's
    /// ([`BotBody::move_goal`]).
    pub(crate) fn move_goal(&self, slot: usize, entity: u32) -> Option<AiMoveRequest> {
        let script = self.slots.get(slot)?.as_ref()?;
        (script.entity == entity)
            .then(|| script.body.move_goal())
            .flatten()
    }

    /// The walker ended the bot's walk to `goal` at world tick `tick`; the
    /// script takes it when it next runs ([`BotBody::move_arrived`]).
    pub(crate) fn move_arrived(
        &mut self,
        slot: usize,
        entity: u32,
        goal: AiMoveRequest,
        tick: u32,
    ) {
        let Some(script) = self.slots[slot]
            .as_mut()
            .filter(|script| script.entity == entity)
        else {
            return;
        };
        if let Err(error) = script.body.move_arrived(goal, tick) {
            eprintln!("bot {entity:08X}'s body script failed on its walk's end ({error})");
            self.slots[slot] = None;
        }
    }

    /// The slot's current frame as its latest live one, sent when it
    /// changed.
    fn publish_live(&mut self, slot: usize) -> Result<(), String> {
        let script = self.slots[slot].as_ref().expect("a running bot script");
        let snapshot = script.body.snapshot().ok_or("the script halted")?;
        let frame = self
            .frame(&script.body, snapshot, &script.initial)
            .map_err(|error| error.to_string())?;
        let script = self.slots[slot].as_mut().expect("a running bot script");
        if frame.full != script.produced {
            script.produced = frame.full.clone();
            script.latest_live = Arc::new(frame);
            self.events.push(BotEvent::Live {
                entity: script.entity,
                frame: Arc::clone(&script.latest_live),
            });
        }
        Ok(())
    }

    /// The lines the bots said since the last call, each with where its
    /// bot stood.
    pub fn take_voices(&mut self) -> Vec<(VoiceLine, [f64; 3])> {
        std::mem::take(&mut self.voices)
    }

    pub fn take_events(&mut self) -> Vec<BotEvent> {
        std::mem::take(&mut self.events)
    }

    fn slot(&self, entity: u32) -> Option<usize> {
        self.slots.iter().position(|script| {
            script
                .as_ref()
                .is_some_and(|script| script.entity == entity)
        })
    }

    fn script(&self, entity: u32) -> Option<&BotScript> {
        self.slot(entity).and_then(|slot| self.slots[slot].as_ref())
    }

    /// The current lifetime's latest live frame (`BotScript.latest_live`).
    fn latest(&self, entity: u32) -> Option<&Arc<BotFrame>> {
        self.script(entity).map(|script| &script.latest_live)
    }

    /// The world tick of the bot's death and its killer.
    fn death(&self, entity: u32) -> Option<(u32, u32)> {
        self.script(entity).and_then(|script| script.death)
    }

    /// Whether the bot's script holds entity variable [`RANGE_BOT`].
    fn range_bot(&self, entity: u32) -> bool {
        self.script(entity)
            .and_then(|script| script.body.snapshot())
            .and_then(|snapshot| snapshot.variables.get(&RANGE_BOT))
            .is_some_and(orpheus_script::expr::truthy)
    }

    fn encoder(&self) -> Option<SnapshotEncoder<'_>> {
        Some(
            SnapshotEncoder::new(self.graphs.as_deref()?, &self.codecs)
                .with_unknowns(self.unknowns),
        )
    }
}

/// Set the live nameplate tool's variables on a bot's script, noting what
/// each had before the tool first set it, and put back what one the tool
/// let go had (null for one the bot never had: the VM can't unset it).
fn tune(
    body: &mut BotBody,
    originals: &mut BTreeMap<PlateVariable, Option<Value>>,
    plate: &BotPlate,
) -> Result<(), BotError> {
    let set = |body: &mut BotBody, variable, value| match variable {
        PlateVariable::Entity(id) => body.set_entity_variable(id, value),
        PlateVariable::Nameplate(id) => body.set_nameplate_variable(id, value),
    };
    for (variable, value) in plate.variables() {
        originals.entry(variable).or_insert_with(|| {
            match variable {
                PlateVariable::Entity(id) => body.entity_variable(id),
                PlateVariable::Nameplate(id) => body.nameplate_variable(id),
            }
            .cloned()
        });
        set(body, variable, value.clone())?;
    }
    let dropped: Vec<PlateVariable> = originals
        .keys()
        .copied()
        .filter(|&variable| plate.variables().all(|(set, _)| set != variable))
        .collect();
    for variable in dropped {
        let had = originals.remove(&variable).flatten();
        set(body, variable, had.unwrap_or(Value::Null))?;
    }
    Ok(())
}

/// The observer projection of one frame, and in the Practice Range the same
/// without the practice nameplate. Both keep the lifetime's first exclusions.
fn project_frame(
    graphs: &GraphStore,
    codecs: &CodecTable,
    unknowns: Unknowns,
    practice: bool,
    behaviors: &[u16],
    snapshot: &Snapshot,
    initial: &ProjectedSnapshot,
) -> Result<BotFrame, SnapshotError> {
    let encoder = SnapshotEncoder::new(graphs, codecs)
        .with_unknowns(unknowns)
        .hiding(behaviors);
    let full = encoder.project_next(snapshot, initial)?;
    let plain = match practice_nameplate(snapshot).filter(|_| practice) {
        Some(child) => {
            let instance = &snapshot.instances[&child];
            let mut hidden = behaviors.to_vec();
            hidden.push(child);
            let stripped = SnapshotEncoder::new(graphs, codecs)
                .with_unknowns(unknowns)
                .hiding(&hidden)
                .project_next(snapshot, initial)?;
            Some(Plain {
                child,
                descriptor: (instance.graph, instance.parent),
                stripped,
            })
        }
        None => None,
    };
    Ok(BotFrame { full, plain })
}

/// combat.py `_shoot` for an NPC: from the bot's eye along its yaw, level
/// (the server body pose until an animated muzzle is available). A volley
/// with a projectile speed flies; one without is hitscan. Its direct hit
/// scores no critical: bots never headshot (rule 123, as `ai.rs` aims at
/// the chest). Assumption: whether the game's damage path leaves the
/// critical out for an AI instigator, or the AI only never aims at the
/// head, isn't known. The Training Bot's rocket (`0BA3` node 178) names no
/// critical multiplier, which would read as 2, and flown level from the
/// eye it meets a player's head.
fn shot_kind(body: &mut BotBody, shot: &ShotEvent, bot: &Bot) -> Option<(BotShotKind, bool)> {
    let [x, y, z] = bot.position;
    let origin = [x, y + BOT_EYE, z];
    let yaw = bot.facing();
    let direction = [yaw.sin(), 0.0, yaw.cos()];
    let volleys = body.shot_volleys(shot);
    body.with_shot(shot, |node, mut evaluate| {
        let speed = authored_number(node.field("m_projectileSpeed"), 0.0, &mut evaluate).ok()?;
        if speed != 0.0 {
            let mut ballistics =
                ProjectileBallistics::from_authored(node, &mut evaluate).ok()??;
            if let Some(direct) = &mut ballistics.direct {
                direct.critical = 1.0;
            }
            // projectiles.py `fired`: the clients see a predicted volley's
            // ballistic projectile; the NPC path has no spawned ones.
            let observed = ballistics.definition.is_some()
                && predicted_sync(node, &mut evaluate).unwrap_or(false)
                && spawned_definitions(node).is_empty();
            return Some((
                BotShotKind::Projectile(PosedProjectile {
                    shot: *shot,
                    ballistics,
                    origin,
                    direction,
                    spawn: None,
                    leased: false,
                    server_shown: false,
                    field: None,
                    feet: bot.position,
                    volleys,
                    // combat.py `_shoot`: the NPC path flies one whatever
                    // the count, and leases none.
                    projectiles: projectiles_per_shot(node, &mut evaluate).unwrap_or(1),
                    pellets: vec![direction],
                }),
                observed,
            ));
        }
        let mut hit =
            HitDefinition::from_authored(node.field("m_6396149F")?, &mut evaluate).ok()??;
        hit.critical = 1.0;
        if hit.amount <= 0.0 {
            return None;
        }
        let reach = authored_number(node.field("m_FEC435C6"), RANGE, &mut evaluate).ok()?;
        Some((
            BotShotKind::Hitscan {
                origin,
                direction,
                reach: if reach == 0.0 { RANGE } else { reach },
                hit,
                ray_profile: ray_profile(node, &mut evaluate),
                shot_frame: shot.frame,
            },
            false,
        ))
    })
    .flatten()
}

/// A frame as one client takes it: the whole frame, or the one without the
/// practice nameplate.
#[derive(Clone)]
struct Chosen {
    frame: Arc<BotFrame>,
    stripped: bool,
}

impl Chosen {
    /// server.py `_project_bot_snapshot`: before the mode UI arrived the
    /// nameplate stays out; after, a frame that follows a known one keeps
    /// it only when the known one already had the same nameplate.
    fn new(frame: Arc<BotFrame>, mode_ready: bool, known: Option<&ProjectedSnapshot>) -> Self {
        let stripped = frame.plain.as_ref().is_some_and(|plain| {
            if mode_ready && known.is_none() {
                return false;
            }
            !(mode_ready
                && known.and_then(|known| known.instance(plain.child)) == Some(plain.descriptor))
        });
        Self { frame, stripped }
    }

    /// Project again with a known frame; a frame already without its
    /// nameplate stays so.
    fn again(&self, mode_ready: bool, known: Option<&ProjectedSnapshot>) -> Self {
        if self.stripped {
            return self.clone();
        }
        Self::new(Arc::clone(&self.frame), mode_ready, known)
    }

    fn projected(&self) -> &ProjectedSnapshot {
        match &self.frame.plain {
            Some(plain) if self.stripped => &plain.stripped,
            _ => &self.frame.full,
        }
    }
}

/// One bot lifetime's statescript on one client (server.py
/// `_BotScriptDelivery`).
struct BotView {
    stream: BodyStream<ProjectedSnapshot>,
    /// The newest live frame waiting behind an unacknowledged one.
    live: Option<Arc<BotFrame>>,
    /// None until this client registered the death.
    death_frames: Option<VecDeque<Chosen>>,
    /// The latest terminal frame, for a deferred kill feed pulse.
    death_snapshot: Option<Chosen>,
    /// A standalone nameplate chunk awaiting its ACK.
    nameplate: Option<u32>,
    /// Chunks replaced before their ACK: neither current nor retried.
    cancelled: BTreeSet<u32>,
    create_acked: bool,
    dirty: bool,
    /// The bot's rocket copies whose launch still needs their volley in
    /// this client's frames, in firing order (`volleys`).
    volleys: VecDeque<u64>,
    /// The bot's destroy waits for this client (server.py
    /// `_body_destroy_wait`).
    destroy_wait: bool,
}

/// What a client's view of the bots needs from its world this update.
pub(crate) struct BotViewContext<'a> {
    pub tick: u32,
    pub mode_ready: bool,
    /// Whether the client can name this entity in a kill feed line
    /// (server.py `combat_identity_ready`).
    pub identity: &'a dyn Fn(u32) -> bool,
    /// The bots whose create this client holds, current lifetimes only.
    pub live: &'a [u32],
}

/// Every bot's statescript on one client.
#[derive(Default)]
pub(crate) struct BotViews {
    views: HashMap<u32, BotView>,
    /// Lifetimes whose first frame couldn't go to this client.
    refused: HashSet<u32>,
    mode_ready: bool,
    /// The firing bots' rockets this client is shown, in firing order.
    seen: Vec<SeenCopy>,
    /// Rocket entities whose destroy has not arrived, by their copy.
    seen_destroys: HashMap<u32, u64>,
    seen_serial: u64,
}

impl BotViews {
    pub(crate) fn clear(&mut self) {
        *self = Self::default();
    }

    /// The bot's create went away with its lifetime.
    pub(crate) fn forget(&mut self, entity: u32) {
        if let Some(mut view) = self.views.remove(&entity) {
            view.stream.retire();
        }
        self.refused.remove(&entity);
    }

    pub(crate) fn create_arrived(&mut self, entity: u32) {
        if let Some(view) = self.views.get_mut(&entity) {
            view.stream.create_arrived();
            view.create_acked = true;
        }
    }

    /// The bot's destroy waits for this client's frames (server.py
    /// `_hold_body_destroy`).
    pub(crate) fn hold_destroy(&mut self, entity: u32) {
        if let Some(view) = self.views.get_mut(&entity) {
            view.destroy_wait = true;
        }
    }

    pub(crate) fn stream(&self, entity: u32) -> Option<&BodyStream<ProjectedSnapshot>> {
        self.views.get(&entity).map(|view| &view.stream)
    }

    pub(crate) fn current(&self, entity: u32, last: u32) -> bool {
        self.views
            .get(&entity)
            .is_some_and(|view| !view.cancelled.contains(&last))
    }

    pub(crate) fn ready(&self, entity: u32, last: u32) -> bool {
        self.views
            .get(&entity)
            .is_some_and(|view| view.stream.ready_to_send(last))
    }

    pub(crate) fn sent(&mut self, entity: u32, last: u32) {
        if let Some(view) = self.views.get_mut(&entity) {
            view.stream.sent(last);
        }
    }

    /// The chunk's datagram arrived or was lost; true: send it again.
    pub(crate) fn delivered(&mut self, entity: u32, last: u32, arrived: bool) -> bool {
        let Some(view) = self.views.get_mut(&entity) else {
            return false;
        };
        view.cancelled.remove(&last);
        if !arrived {
            return view.stream.lost(last).retry;
        }
        view.stream.arrived(last);
        if view.nameplate == Some(last) {
            view.nameplate = None;
        }
        view.dirty = true;
        false
    }

    /// The bot's frames still owed to this client hold back its destroy
    /// (server.py `_body_destroy_pending`).
    pub(crate) fn pending(&self, entity: u32, health_pending: bool) -> bool {
        self.views.get(&entity).is_some_and(|view| {
            view.create_acked
                && ((view.stream.pending().next().is_some() && view.nameplate.is_none())
                    || view.live.is_some()
                    || view
                        .death_frames
                        .as_ref()
                        .is_some_and(|frames| !frames.is_empty())
                    || (view.death_frames.is_some() && health_pending))
        })
    }

    /// Register new lifetimes, take the room's events, flush what they and
    /// this client's ACKs made due, then the practice nameplates. Returns
    /// the chunks to queue, in order.
    pub(crate) fn update(
        &mut self,
        scripts: &BotScripts,
        events: &[BotEvent],
        context: &BotViewContext<'_>,
    ) -> Vec<ScriptChunk> {
        let mut out = Vec::new();
        let Some(encoder) = scripts.encoder() else {
            return out;
        };
        let mut flush = Flush {
            scripts,
            seen: &self.seen,
            encoder,
            context,
            out: &mut out,
        };
        for &entity in context.live {
            if !self.views.contains_key(&entity) && !self.refused.contains(&entity) {
                match flush.register(entity) {
                    Some(view) => {
                        self.views.insert(entity, view);
                    }
                    None if scripts.latest(entity).is_some() => {
                        self.refused.insert(entity);
                    }
                    None => {}
                }
            }
        }
        for event in events {
            match event {
                BotEvent::Live { entity, frame } => {
                    if let Some(view) = self.views.get_mut(entity) {
                        if view.death_frames.is_none() {
                            view.live = Some(Arc::clone(frame));
                            flush.frames(*entity, view, false);
                        }
                    }
                }
                BotEvent::Death { entity, frame } => {
                    if let Some(view) = self.views.get_mut(entity) {
                        if view.death_frames.is_none() {
                            view.death_frames = Some(
                                frame
                                    .iter()
                                    .map(|frame| {
                                        Chosen::new(Arc::clone(frame), context.mode_ready, None)
                                    })
                                    .collect(),
                            );
                            flush.death_now(*entity, view);
                        }
                    }
                }
                BotEvent::DeathUpdate { entity, frame } => {
                    if let Some(view) = self.views.get_mut(entity) {
                        if let Some(frames) = &view.death_frames {
                            // A later timer keeps the presenter's projection
                            // from the original death.
                            let known = frames
                                .back()
                                .or(view.death_snapshot.as_ref())
                                .map(Chosen::projected)
                                .or_else(|| view.stream.base().map(|(_, base)| base))
                                .or_else(|| view.stream.pending().next().map(|(_, s, _)| s));
                            let chosen = Chosen::new(Arc::clone(frame), context.mode_ready, known);
                            view.death_frames
                                .as_mut()
                                .expect("a registered death")
                                .push_back(chosen);
                            flush.frames(*entity, view, false);
                        }
                    }
                }
            }
        }
        // server.py `_flush_combat_ui`: the mode UI's arrival lets the
        // registered deaths' kill feed through.
        let mode_arrived = context.mode_ready && !self.mode_ready;
        self.mode_ready = context.mode_ready;
        for (&entity, view) in &mut self.views {
            if view.dirty || (mode_arrived && view.death_frames.is_some()) {
                view.dirty = false;
                flush.frames(entity, view, false);
            }
        }
        if context.mode_ready && scripts.practice {
            for (&entity, view) in &mut self.views {
                flush.nameplate(entity, view);
            }
        }
        out
    }
}

struct Flush<'a, 'o> {
    scripts: &'a BotScripts,
    /// This client's bot rocket copies.
    seen: &'a [SeenCopy],
    encoder: SnapshotEncoder<'a>,
    context: &'a BotViewContext<'a>,
    out: &'o mut Vec<ScriptChunk>,
}

impl Flush<'_, '_> {
    /// server.py `_track_body_create`: the lifetime's full frame from its
    /// latest live frame, sent once the create arrived.
    fn register(&mut self, entity: u32) -> Option<BotView> {
        let latest = self.scripts.latest(entity)?;
        let chosen = Chosen::new(Arc::clone(latest), self.context.mode_ready, None);
        let mut stream = BodyStream::new(entity);
        let chunk = self
            .encoder
            .queue_full(&mut stream, chosen.projected())
            .map_err(|error| error.to_string())
            .and_then(|chunk| {
                let packet = frame(u32::MAX, u32::MAX, &[], &[], &[&chunk.update])
                    .map_err(|error| error.to_string())?;
                if packet.len() > MAX_PAYLOAD {
                    return Err("its first frame exceeds the game datagram budget".into());
                }
                Ok(chunk)
            });
        match chunk {
            Ok(chunk) => {
                self.out.push(chunk);
                Some(BotView {
                    stream,
                    live: None,
                    death_frames: None,
                    death_snapshot: None,
                    nameplate: None,
                    cancelled: BTreeSet::new(),
                    create_acked: false,
                    dirty: false,
                    volleys: VecDeque::new(),
                    destroy_wait: false,
                })
            }
            Err(reason) => {
                eprintln!("bot {entity:08X}'s statescript can't go to a client ({reason})");
                None
            }
        }
    }

    /// server.py `_flush_bot_frames`: one cumulative update at a time, from
    /// the acknowledged baseline; the live frame first, then the death's.
    fn frames(&mut self, entity: u32, view: &mut BotView, replace_nameplate: bool) {
        if view.stream.base().is_none() {
            return;
        }
        if view.stream.pending().next().is_some()
            && (view.nameplate.is_none() || !replace_nameplate)
        {
            return;
        }
        if !view.volleys.is_empty() {
            self.volley(entity, view);
            return;
        }
        if let Some(live) = view.live.take() {
            let chosen = Chosen::new(live, self.context.mode_ready, None);
            if self.queue(entity, view, chosen.projected().clone(), false) {
                return;
            }
        }
        if view
            .death_frames
            .as_ref()
            .is_some_and(|frames| !frames.is_empty())
        {
            while let Some(chosen) = view.death_frames.as_mut().and_then(VecDeque::pop_front) {
                view.death_snapshot = Some(chosen.clone());
                if self.death(entity, view, &chosen, None) {
                    return;
                }
            }
        } else if let Some(chosen) = view.death_snapshot.clone() {
            // Rechecking feed eligibility must not install a new presenter
            // on a corpse.
            let base = view.stream.base().map(|(_, base)| base.clone());
            self.death(entity, view, &chosen, base.as_ref());
        }
    }

    /// A death's first frame goes out at once, even while an earlier frame
    /// waits for its acknowledgement (server.py `_flush_bot_frames` waited,
    /// so a bot that died within a round trip of a hit it survived kept its
    /// health bar until that hit's frame arrived: 0D24 hides the bar on the
    /// dead flag #478, st38 `!#478e`). It is a cumulative update from the
    /// acknowledged frame, as the other clients' body frames are
    /// (`others.rs`), and the death's state holds every live change, so a
    /// live frame still waiting goes with it. A rocket copy whose launch
    /// still needs its volley keeps that volley in the death's frames until
    /// the launch (`hold_volleys`); before, the death waited for the launch,
    /// a round trip and more, with the bar up at 0 health.
    fn death_now(&mut self, entity: u32, view: &mut BotView) {
        if view.stream.base().is_none() {
            return;
        }
        view.live = None;
        while let Some(chosen) = view.death_frames.as_mut().and_then(VecDeque::pop_front) {
            view.death_snapshot = Some(chosen.clone());
            if self.death(entity, view, &chosen, None) {
                return;
            }
        }
    }

    /// server.py `_flush_bot_frames` while copies of the bot's rockets wait
    /// for their launch: the first copy's volley activation stays in the
    /// frames until the acknowledged baseline holds it; each later copy's
    /// volley keeps its acknowledged state, and everything else comes from
    /// the latest frame (without taking the live one).
    fn volley(&mut self, entity: u32, view: &mut BotView) {
        let Some(first) = self.first_volley(view) else {
            return;
        };
        let Some((_, acknowledged)) = view.stream.base() else {
            return;
        };
        // Keep this activation installed until its launch is acknowledged.
        if first.volley_arrived(acknowledged) {
            return;
        }
        let base = match view.nameplate {
            None => acknowledged,
            Some(old) => match view.stream.pending().find(|&(last, _, _)| last == old) {
                Some((_, snapshot, _)) => snapshot,
                None => acknowledged,
            },
        };
        let mut latest = match &view.live {
            Some(live) => Chosen::new(Arc::clone(live), self.context.mode_ready, None)
                .projected()
                .clone(),
            None => base.clone(),
        };
        if self.hold_volleys(view, &mut latest) {
            self.queue(entity, view, latest, false);
        }
    }

    /// The first rocket copy whose launch waits for its volley; none when
    /// projectile cleanup cancelled it, which happens before the stream
    /// advances.
    fn first_volley(&self, view: &BotView) -> Option<&ObservedProjectile> {
        let seen = self.seen;
        let first = &view
            .volleys
            .front()
            .and_then(|&serial| seen.iter().find(|copy| copy.serial == serial))?
            .projectile;
        (!view.destroy_wait && self.context.tick < first.deadline()).then_some(first)
    }

    /// Put the volleys of the rocket copies still waiting for their launch
    /// into `frame` (server.py `_flush_bot_frames`): the first copy's
    /// activation, which the launch needs in the acknowledged frame, each
    /// later copy's volley as acknowledged. False, `frame` untouched, when
    /// no copy waits or `frame` lost the first copy's instance (the old
    /// activation expires without recreating a removed instance).
    fn hold_volleys(&self, view: &BotView, frame: &mut ProjectedSnapshot) -> bool {
        let Some(first) = self.first_volley(view) else {
            return false;
        };
        let Some((_, acknowledged)) = view.stream.base() else {
            return false;
        };
        let (instance, state) = first.volley_key();
        if !frame.has_instance(instance) {
            return false;
        }
        let seen = self.seen;
        for copy in view
            .volleys
            .iter()
            .filter_map(|&serial| seen.iter().find(|copy| copy.serial == serial))
        {
            let (instance, state) = copy.projectile.volley_key();
            frame.replace_state(
                instance,
                state,
                acknowledged.state(instance, state).cloned(),
            );
        }
        frame.replace_state(instance, state, first.held().cloned());
        true
    }

    /// server.py `_queue_bot_death_snapshot`: a new kill feed pulse waits
    /// for the client's mode UI, the death's freshness and both names.
    fn death(
        &mut self,
        entity: u32,
        view: &mut BotView,
        chosen: &Chosen,
        known: Option<&ProjectedSnapshot>,
    ) -> bool {
        let allowed = self.kill_feed_allowed(entity);
        let mut projected = chosen
            .again(self.context.mode_ready, known)
            .projected()
            .clone()
            .gate_kill_feed(self.encoder.graphs(), &view.stream, allowed);
        // A death while a rocket copy waits for its launch (`death_now`).
        self.hold_volleys(view, &mut projected);
        self.queue(entity, view, projected, false)
    }

    /// combat.py `kill_feed_allowed`. A kill between two bots that both
    /// hold [`RANGE_BOT`] draws no line, as the bot's death graph `0C18`
    /// sends its kill feed message only when the victim or the killer lacks
    /// it (read from data upstream, kjs-port `a9096f5d`, `kill-feed.md`; the
    /// original client drew no line for such kills).
    fn kill_feed_allowed(&self, entity: u32) -> bool {
        let Some((tick, killer)) = self.scripts.death(entity) else {
            return false;
        };
        self.context.mode_ready
            && kill_feed_fresh(self.context.tick, tick)
            && (self.context.identity)(entity)
            && (self.context.identity)(killer)
            && !(self.scripts.range_bot(entity) && self.scripts.range_bot(killer))
    }

    /// server.py `_flush_bot_nameplates` for one bot: the nameplate alone
    /// on the acknowledged baseline, once its parent is there.
    fn nameplate(&mut self, entity: u32, view: &mut BotView) {
        if view.nameplate.is_some() {
            // The match logic queued this frame's health and combat updates.
            self.frames(entity, view, true);
            return;
        }
        let Some((_, base)) = view.stream.base() else {
            return;
        };
        if view.stream.pending().next().is_some()
            || view.live.is_some()
            || !view.volleys.is_empty()
            || view.death_frames.is_some()
        {
            return;
        }
        let Some(latest) = self.scripts.latest(entity) else {
            return;
        };
        let Some(plain) = &latest.plain else {
            return;
        };
        if base.instance(plain.child).is_some() {
            return;
        }
        let Some((parent, _)) = plain.descriptor.1 else {
            return;
        };
        if base.instance(parent) != latest.full.instance(parent) {
            return;
        }
        // The acknowledged frame owns everything outside the presenter.
        if let Some(snapshot) = base.with_instance_of(&latest.full, plain.child) {
            self.queue(entity, view, snapshot, true);
        }
    }

    /// server.py `_queue_bot_snapshot`: a resent cumulative update from the
    /// acknowledged baseline. A standalone nameplate chunk can be replaced by
    /// one that includes it.
    fn queue(
        &mut self,
        entity: u32,
        view: &mut BotView,
        projected: ProjectedSnapshot,
        mut nameplate_only: bool,
    ) -> bool {
        let old = view.nameplate;
        if let Some(old) = old {
            let queued = view
                .stream
                .pending()
                .find(|&(last, _, _)| last == old)
                .map(|(_, snapshot, _)| snapshot);
            if queued == Some(&projected) {
                return false; // only the queued presenter would be repeated
            }
        }
        let chunk = match self.encoder.queue_delta(
            &mut view.stream,
            &projected,
            DeltaKind::Observer { resend: true },
            FRAME_MS as u32,
        ) {
            Ok(Some(chunk)) => chunk,
            Ok(None) => return false,
            Err(error) => {
                eprintln!("bot {entity:08X}'s frames for a client failed ({error})");
                return false;
            }
        };
        if let Some(old) = old {
            let child = self
                .scripts
                .latest(entity)
                .and_then(|latest| latest.plain.as_ref())
                .map(|plain| plain.child);
            nameplate_only = child.is_some_and(|child| {
                view.stream
                    .pending()
                    .find(|&(last, _, _)| last == chunk.last)
                    .is_some_and(|(_, _, keys)| {
                        keys.iter().all(|&key| key_instance(key) == Some(child))
                    })
            });
            if !view.stream.discard_unsent(old) {
                view.cancelled.insert(old);
            }
        }
        view.nameplate = nameplate_only.then_some(chunk.last);
        self.out.push(chunk);
        true
    }
}

/// The instance a change belongs to; none for a frame action.
fn key_instance(key: ChangeKey) -> Option<u16> {
    match key {
        ChangeKey::Instance(instance)
        | ChangeKey::Variable(instance, _)
        | ChangeKey::State(instance, _)
        | ChangeKey::VolleyIntroduction(instance, _) => Some(instance),
        ChangeKey::Action { .. } => None,
    }
}

/// A firing bot's rocket the clients see (projectiles.py `fired` for an
/// NPC, then `_show_others`).
#[derive(Clone, Debug)]
pub struct SeenRocket {
    /// The room's rocket it follows (combat.py `Rocket.shot`).
    pub rocket: u64,
    pub source: u32,
    pub shot: ShotEvent,
    pub definition: u64,
    pub volleys: u16,
    pub origin: [f64; 3],
    pub direction: [f64; 3],
    /// The ticks it flies at most (`Shot.bound`).
    pub bound: u32,
    /// The bot's frame it was fired from (`snapshot`).
    pub frame: Arc<BotFrame>,
}

impl SeenRocket {
    /// The copy of a fired rocket, if the shot is one the clients see.
    pub fn of(rocket: u64, shot: &BotShot) -> Option<Self> {
        let BotShotKind::Projectile(projectile) = &shot.kind else {
            return None;
        };
        let definition = projectile.ballistics.definition.filter(|_| shot.observed)?;
        Some(Self {
            rocket,
            source: shot.source,
            shot: projectile.shot,
            definition,
            volleys: u16::from(projectile.volleys),
            origin: projectile.origin,
            direction: projectile.direction,
            bound: (projectile.ballistics.lifetime * 1000.0 / FRAME_MS as f64).ceil() as u32,
            frame: Arc::clone(&shot.frame),
        })
    }
}

/// A record of a bot rocket's copy on one client: its create and the bot's
/// shot record, current while the copy lives; its destroy until it arrived.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SeenDelivery {
    Create { serial: u64 },
    Launch { serial: u64 },
    Destroy { serial: u64 },
}

/// One bot rocket on one client (`_ObservedProjectile` with `npc=True`).
struct SeenCopy {
    serial: u64,
    rocket: u64,
    projectile: ObservedProjectile,
}

impl BotViews {
    /// projectiles.py `_show_others` for a bot's rocket: when this client
    /// has the bot's statescript stream (server.py `bot_stream`, even
    /// before its baseline), an entity of the rocket's definition under the
    /// map, launched in the bot's channel 2 once its create arrived.
    pub(crate) fn show_rocket(
        &mut self,
        rocket: &SeenRocket,
        tick: u32,
        next_id: impl FnOnce() -> u32,
    ) -> Vec<(EntityUpdate, SeenDelivery)> {
        let Some(view) = self.views.get(&rocket.source) else {
            return Vec::new();
        };
        let has_base = view.stream.base().is_some();
        let deadline = tick
            .saturating_add(rocket.bound)
            .saturating_add(DELIVERY_GRACE);
        let Ok(mut projectile) = ObservedProjectile::new(
            rocket.source,
            next_id(),
            rocket.definition,
            &rocket.shot,
            rocket.volleys,
            rocket.origin.map(|value| value as f32),
            rocket.direction,
            deadline,
        ) else {
            return Vec::new();
        };
        self.seen_serial += 1;
        let serial = self.seen_serial;
        let create = projectile.open(has_base);
        // server.py `retain_bot_volley`: the firing frame's activation of
        // the shot's volley stays in this client's frames until the launch
        // arrives.
        if projectile.hold(&rocket.frame.full) {
            if let Some(view) = self.views.get_mut(&rocket.source) {
                view.volleys.push_back(serial);
            }
        }
        self.seen.push(SeenCopy {
            serial,
            rocket: rocket.rocket,
            projectile,
        });
        create
            .into_iter()
            .filter_map(|out| match out {
                ObservedOut::Create(update) => Some((update, SeenDelivery::Create { serial })),
                _ => None,
            })
            .collect()
    }

    /// The rocket landed (projectiles.py `flown` for an NPC shot): its copies
    /// keep DELIVERY_GRACE ticks for their launch.
    pub(crate) fn finish_rocket(&mut self, rocket: u64, tick: u32) {
        for copy in self.seen.iter_mut().filter(|copy| copy.rocket == rocket) {
            copy.projectile.finish(tick, false, true);
        }
    }

    /// `_ObservedProjectile.advance` for this client's bot rockets: the
    /// create once the bot's stream has a baseline, the launch once the
    /// create arrived and the acknowledged baseline holds the volley, the
    /// destroy when the rocket landed, its bot's lifetime ended or the
    /// deadline passed.
    pub(crate) fn advance_rockets(&mut self, tick: u32) -> Vec<(EntityUpdate, SeenDelivery)> {
        let mut records = Vec::new();
        let mut seen = std::mem::take(&mut self.seen);
        seen.retain_mut(|copy| {
            let view = self.views.get(&copy.projectile.body());
            let base = view
                .and_then(|view| view.stream.base())
                .map(|(_, base)| base);
            let volley = base.is_some_and(|base| copy.projectile.volley_arrived(base));
            let serial = copy.serial;
            let mut keep = true;
            for out in copy
                .projectile
                .advance(tick, view.is_some(), base.is_some(), volley)
            {
                match out {
                    ObservedOut::Create(update) => {
                        records.push((update, SeenDelivery::Create { serial }));
                    }
                    ObservedOut::Launch(update) => {
                        records.push((update, SeenDelivery::Launch { serial }));
                    }
                    // Dropping the copy withdraws its queued launch.
                    ObservedOut::CancelLaunch => {}
                    ObservedOut::Destroy(update) => {
                        self.seen_destroys.insert(update.entity, serial);
                        records.push((update, SeenDelivery::Destroy { serial }));
                    }
                    ObservedOut::Done => keep = false,
                }
            }
            // `_release_volley`: the launch arrived or the copy ended; the
            // first held volley's release lets the bot's frames go on.
            if (!keep || copy.projectile.launched()) && copy.projectile.release() {
                if let Some(view) = self.views.get_mut(&copy.projectile.body()) {
                    let first = view.volleys.front() == Some(&serial);
                    view.volleys.retain(|&held| held != serial);
                    if first {
                        view.dirty = true;
                    }
                }
            }
            keep
        });
        self.seen = seen;
        records
    }

    pub(crate) fn seen_current(&self, delivery: SeenDelivery, entity: u32) -> bool {
        match delivery {
            SeenDelivery::Create { serial } | SeenDelivery::Launch { serial } => {
                self.seen.iter().any(|copy| copy.serial == serial)
            }
            SeenDelivery::Destroy { serial } => self.seen_destroys.get(&entity) == Some(&serial),
        }
    }

    /// The record's datagram arrived or was lost; true: send it again.
    pub(crate) fn seen_delivered(
        &mut self,
        delivery: SeenDelivery,
        entity: u32,
        arrived: bool,
    ) -> bool {
        if !arrived {
            return self.seen_current(delivery, entity);
        }
        match delivery {
            SeenDelivery::Create { serial } | SeenDelivery::Launch { serial } => {
                if let Some(copy) = self.seen.iter_mut().find(|copy| copy.serial == serial) {
                    if matches!(delivery, SeenDelivery::Create { .. }) {
                        copy.projectile.creation_arrived();
                    } else {
                        copy.projectile.launch_arrived();
                    }
                }
            }
            SeenDelivery::Destroy { .. } => {
                self.seen_destroys.remove(&entity);
            }
        }
        false
    }
}

#[cfg(test)]
mod tests;
