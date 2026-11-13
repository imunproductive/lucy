//! Per-peer world delivery, including the owning player's body lifetime.

use std::{
    borrow::Borrow,
    collections::{BTreeMap, HashMap, HashSet, VecDeque},
    io,
    ops::Range,
    sync::Arc,
    time::Duration,
};

use crate::{
    announcer::Multikills,
    body::{BodyCreate, BodyView, body_create, owner_pose},
    body_script::OwnerBodySpawn,
    bot_script::{BotEvent, BotViewContext, BotViews, SeenDelivery, SeenRocket},
    bots::BOT_RECORD_DELAY,
    combat::{
        BotReplacement, DEAD, PlayerHit, PlayerTarget, PosedProjectile, PosedShot, RESPAWN_RETRY,
        damage_message, death_message,
    },
    controller::ControllerFrame,
    egress::{
        DeliveryReport, FrameRecords, GameEgress, PacketDelivery, RecordSource, SendError,
        SentFrame,
    },
    map_areas::SPAWN_HEALING,
    notices::{NOTICE_GAP, Switch},
    others::{
        ObservedScript, ObserverActivity, OtherDelivery, Others, PlayerView, RemoteBody,
        RemoteDeath, ShowContext, ShowOutput, ViewerReadiness,
    },
    pools::FALLBACK_POOLS,
    projectile_flight::{FlightEnd, ProjectileFlight},
    projectiles::{
        FieldCopies, FieldDeliver, FieldDelivery, FiredProjectile, OwnerProjectiles,
        ProjectileDelivery, ProjectileEvent, ProjectileLeases, ProjectileToken, ShotKey,
    },
    scene::{PlayerAppearance, Possession, body_filter, filter_component, player_team_bit},
    scene_assets::PreparedScene,
    voice_lines::{VoiceLine, VoiceState},
};
use orpheus_content::{
    hero_settings::{HeroSetting, HeroSettings, SCRIPT_SETTINGS},
    heroes::{ANA, Hero, SOLDIER},
    parameters::{ClientGameMessage, HeroSelectRequest},
};
use orpheus_core::tick::{MAX_LEASE_FRAME, TICK_PERIOD};
use orpheus_physics::{
    contact::cache::CapsulePose,
    mover::Mover,
    mover::correction::CorrectionHistory,
    mover::kinematics::{Poses, poses as kinematics_poses, repeated},
    mover::modifiers::{Modifier, ModifierKey},
};
use orpheus_script::{
    snapshot::{ProjectedSnapshot, SnapshotEncoder, View},
    stream::{BodyStream, ChangeKey, DeltaKind, ScriptChunk, Stream},
    vitals::{Death, Vitals},
    vm::{
        BodyEffects, GameMessage, Motion, MoveRequest, RaisedCriteria, Snapshot, TargetCandidate,
    },
    weapons::{DamageTags, ProjectileBallistics},
    wire::{Value as ScriptValue, Variables},
};
use orpheus_wire::{
    commands::{Command, CommandQueue, FRAME_HALF, SUBFRAME, frame_distance},
    entity::{
        Component as WorldComponent, EntityUpdate, GAME_MODE_ENTITY, MAX_PAYLOAD, Operation,
        SkinTheme, frame,
    },
    link::delivery,
    messages::Message,
};

pub(crate) const BODY_REUSE_DELAY: Duration = Duration::from_secs(30);
// Resource bound for this worker. The reference keeps 64 predicted states and
// 32 commands. A farther-ahead command can be retried once the clock catches up.
const MAX_INPUT_LEAD: u32 = 96;
// Several retransmit windows fit before a peer exhausts this queue. One final
// leave marker may follow the accepted prefix when the cap is reached.
const MAX_PENDING_INPUT: usize = 256;
// BodyScript.command retains this many intermediate VM frames before its
// requested frame. The first held frame runs even across a larger gap.
const MAX_SCRIPT_TAIL: u32 = 256;

// The server's input buffer (GDC 2017, Ford, "Overwatch Gameplay
// Architecture and Netcode" 30:50; Developer Update "Let's Talk Netcode"
// 2016, 11:47): each tick runs one command of a player, the next in frame
// order, from a small buffer that the client keeps fed. Retail's target
// for a steady stream was one command buffered: the client led by half the
// round trip plus one frame, so the command of the next tick is in hand
// when a tick runs its own, and a datagram up to a tick late runs on time.
const BUFFER_TARGET: usize = 1;
// Before mid-2016 retail let up to 4 commands wait (~48 ms). Beyond that a
// tick runs the oldest at once (assumption: retail's client slowed down
// instead, and we have no signal that tells ours to).
const MAX_BUFFERED: usize = 4;
// Retail's client shrank and grew the buffer by dilating its 16 ms frame
// (15.2 ms, Ford): one command in 20 frames. We have no field that asks it
// to (`docs/notes/behavior.md`, "Input timing"), so the server does it
// from its side: a buffer above the target for this many ticks in a row
// runs one command more, and one below it while the client sends waits a
// tick (assumption).
const DRAIN_TICKS: u32 = 20;
// A dry buffer repeats the client's last input for at most this many ticks
// in a row (assumption: a longer silence is not a late datagram, so the
// body waits for the client's next command as it did before the buffer).
const MAX_REPEATS: u32 = MAX_BUFFERED as u32;

/// correction.py `HEROES`: the heroes whose owner gets his body's state
/// (owner corrections): the ones whose movement the server's mover follows.
/// Python had Soldier: 76 alone. Ana is beyond it (2026-11-12): no ability
/// of hers moves her, and her scoped walk (`01C7` st49 takes the speed down
/// by `#23184`, which `0B94` st77 sets to 0.65 while the zoom `#169` is
/// full) runs at the reference's speeds on the reference's frames, the same
/// with and without her corrections. With corrections her scoped walk
/// stopped in the game while her owner frames sent `#23184` as its stacked
/// value with its binding to st77 (`#23184e + 0.65`), which her client
/// (inferred, not traced) counts twice: 1.3, a speed scalar below 0. They
/// send its own value now (`SnapshotEncoder` `project_variables`). Without corrections, any other difference between
/// her client's body and the server's lasted until she died, and her shots
/// left from the wrong place. Widowmaker stays out: the server can't run
/// her Grappling Hook (`005A` st1, a `MoveTo` to an entity and bone), so a
/// correction would pull her back. The other heroes stay out until the
/// server runs all of their movement (`docs/notes/behavior.md`, "Owner
/// corrections").
const CORRECTED_HEROES: [u64; 2] = [SOLDIER, ANA];

/// Whether a living body of team `other` blocks the moves of a body of
/// `team` (the server's mover then takes its capsule,
/// [`WorldDelivery::set_blockers`]): an enemy's does, an ally's does not,
/// and in a free-for-all everyone is an enemy, as combat counts them. Retail
/// OW1: "you can clip through other allies ... However, you cannot clip
/// through enemies" (Furious Paul's Overwatch mechanics guide, 2017);
/// upstream's server leaves allies out the same way (leafall903/ORPHEUS
/// (MIT) `mccree.py` `_npc_peer_contacts`). A dead body blocks nobody. The
/// client's own masks admit both team categories before its callers'
/// overrides (0x1499 and 0x1498, upstream `docs/MCCREE_ROLL.txt`), and a
/// state turns a mover's same-team contact off (`STU_6CC90892`, in
/// Junkenstein's graphs `0CBC` and `0F68` only): which override leaves a
/// player's allies out is not known.
pub fn blocks(free_for_all: bool, team: u8, other: u8) -> bool {
    free_for_all || team != other
}

/// The capsules that block the body of a player of `team` ([`blocks`]),
/// where they are now: the other players' living bodies (`others`, each
/// with his team) and the living training bots.
pub fn blockers<'a>(
    scene: &PreparedScene,
    team: u8,
    others: impl IntoIterator<Item = (u8, &'a WorldDelivery)>,
) -> Vec<CapsulePose> {
    let free_for_all = scene.map().free_for_all;
    let enemy = |other| blocks(free_for_all, team, other);
    let bots = scene
        .bot_capsules()
        .filter(|&(other, _)| enemy(other))
        .map(|(_, capsule)| capsule);
    others
        .into_iter()
        .filter(|&(other, _)| enemy(other))
        .filter_map(|(_, world)| world.body_capsule())
        .chain(bots)
        .collect()
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct BodyId {
    pub(crate) entity: u32,
    pub(crate) generation: u64,
}

#[derive(Clone, Copy)]
struct Body {
    id: BodyId,
    hero: u64,
    skin: SkinTheme,
    /// The filter bit of the player's team (`scene::player_team_bit`).
    team_bit: u64,
}

/// A running owner VM and the authored data used to encode its snapshots.
/// A supported modifier projection is required before the mover can correct
/// the owner's prediction, even if the current set of modifiers is empty.
pub trait OwnerBodyRuntime: Send {
    fn snapshot(&self) -> Option<&Snapshot>;
    fn encoder(&self) -> SnapshotEncoder<'_>;
    fn advance(&mut self, command: Command, mover: &Mover, step: BodyStep) -> io::Result<()>;
    fn frame_ms(&self) -> u32;

    fn vitals(&self) -> Option<&Vitals> {
        None
    }

    /// New health revisions supersede older ones, including any slot clears.
    fn take_health_update(&mut self) -> Option<WorldComponent> {
        None
    }

    fn projectile_leases(&self) -> Option<&ProjectileLeases> {
        None
    }

    fn movement_modifiers(&self) -> Option<&BTreeMap<ModifierKey, Modifier>> {
        None
    }

    fn take_shots(&mut self, _command: Option<Command>, _mover: &Mover) -> Vec<PosedShot> {
        Vec::new()
    }

    /// What other clients must get at once of this body's statescript
    /// (observers.py `_activity`). None: every send frame may carry a delta.
    fn observer_activity(&self) -> Option<ObserverActivity> {
        None
    }

    /// The body's pools, for damage and healing (combat.py `Vitals`).
    fn vitals_mut(&mut self) -> Option<&mut Vitals> {
        None
    }

    /// The pools changed outside the VM: the owner's health variables follow
    /// them, and a body with none left is dead to its own script
    /// (driver.py `BodyScript.set_health`).
    fn health_changed(&mut self) -> io::Result<()> {
        Ok(())
    }

    /// The hero dealt `points` of damage or healing: his ultimate charges
    /// for it.
    fn gain_ult(&mut self, _points: f64) -> io::Result<()> {
        Ok(())
    }

    /// An aura comes on the body (`vm/auras.rs` in `orpheus_script`),
    /// dealt by `instigator`.
    fn apply_aura(&mut self, _aura: u64, _instigator: u32) -> io::Result<()> {
        Ok(())
    }

    /// The statuses and combat modifiers the body's states hold.
    fn effects(&self) -> Option<&BodyEffects> {
        None
    }

    /// A game message from another entity, run now (match.py
    /// `deliver_body_messages`, combat.py `_died`). Returns how many entries
    /// took it.
    fn receive_message(&mut self, _message: u64, _parameters: Variables) -> io::Result<usize> {
        Ok(0)
    }

    /// The game messages the body sent other entities since the last call.
    fn take_messages(&mut self) -> Vec<GameMessage> {
        Vec::new()
    }

    /// The criteria the body's graphs raised on it since the last call
    /// ([`crate::voice_lines`]).
    fn take_criteria(&mut self) -> Vec<RaisedCriteria> {
        Vec::new()
    }

    /// The active `MoveTo` states' targets (match.py `_step_body_pose`).
    fn move_requests(&self) -> Vec<MoveRequest> {
        Vec::new()
    }

    /// A world-target move ended at this frame: the state finishes when the
    /// body arrived, else aborts (match.py `body_command`).
    fn finish_move(
        &mut self,
        _request: MoveRequest,
        _arrived: bool,
        _frame: u32,
    ) -> io::Result<()> {
        Ok(())
    }

    /// The dashes and rewinds the body's graphs began or ended since the
    /// last call, for the mover (Tracer's Blink and Recall).
    fn take_motions(&mut self) -> Vec<Motion> {
        Vec::new()
    }

    /// Begin the hero's spawn voice line (voicelines.py `VoiceLines.spawned`).
    fn begin_spawn_bark(&mut self, _now: Duration) -> io::Result<()> {
        Ok(())
    }

    /// End the spawn voice line once its time is up (voicelines.py
    /// `VoiceLines.expire`).
    fn expire_bark(&mut self, _now: Duration) -> io::Result<()> {
        Ok(())
    }

    /// The bodies the body's target searches can find from now on
    /// (`Combat::target_candidates`).
    fn set_target_candidates(&mut self, _candidates: &[TargetCandidate]) {}

    /// The player's hero settings that the body's graphs read, as entity
    /// variables ([`hero_setting_variables`]).
    fn set_hero_settings(&mut self, _variables: &Variables) -> io::Result<()> {
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BodyStep {
    Posed,
    UnposedBoundary,
    UnposedTimer,
}

struct BodyScript {
    id: BodyId,
    runtime: Box<dyn OwnerBodyRuntime>,
    stream: BodyStream<ProjectedSnapshot>,
    frame: u32,
    unsent_delta: Option<(u32, DeltaKind)>,
    corrections: Option<CorrectionHistory>,
    projectiles: Option<OwnerProjectiles>,
    /// Each projectile in flight, with its lease token when the owner's
    /// client shot it too (combat.py `Combat.rockets`).
    flights: Vec<(Option<ProjectileToken>, ProjectileFlight)>,
    /// The projectiles fired since the match last took them, for the other
    /// clients (projectiles.py `fired`: `_show_others`).
    fired: Vec<FiredProjectile>,
    /// The buttons of the last command traced (`orpheus::owner` at debug).
    traced_buttons: u32,
    /// The last command the body ran with a pose: the frames of a gap
    /// repeat it ([`ScriptGap`]).
    input: Option<Command>,
}

impl BodyScript {
    fn new(body: Body, frame: u32, runtime: Box<dyn OwnerBodyRuntime>) -> io::Result<Self> {
        let snapshot = runtime
            .snapshot()
            .ok_or_else(|| invalid("body VM is unavailable"))?;
        if snapshot.entity != body.id.entity {
            return Err(invalid("body VM has the wrong entity"));
        }
        runtime
            .encoder()
            .project(snapshot, View::Owner)
            .map_err(invalid)?;
        let corrections = (CORRECTED_HEROES.contains(&body.hero)
            && runtime.movement_modifiers().is_some())
        .then(|| CorrectionHistory::new(body.id.entity, body.id.generation));
        let projectiles = runtime
            .projectile_leases()
            .filter(|leases| !leases.blocks().is_empty())
            .map(|leases| OwnerProjectiles::new(body.id.entity, leases.clone()));
        Ok(Self {
            id: body.id,
            runtime,
            stream: BodyStream::new(body.id.entity),
            frame,
            unsent_delta: None,
            corrections,
            projectiles,
            flights: Vec::new(),
            fired: Vec::new(),
            traced_buttons: 0,
            input: None,
        })
    }

    /// One frame of the body's script. A dead body's reports go nowhere
    /// (combat.py `Combat.command`: the dead do not shoot).
    fn advance(
        &mut self,
        mover: &mut Mover,
        command: Command,
        step: BodyStep,
        dead: bool,
    ) -> io::Result<Vec<PosedShot>> {
        if command.frame > self.frame {
            self.runtime.advance(command, mover, step)?;
            self.frame = command.frame;
            if step == BodyStep::Posed {
                self.input = Some(command);
            }
            if command.buttons != self.traced_buttons {
                self.traced_buttons = command.buttons;
                tracing::debug!(
                    target: "orpheus::owner",
                    body = format_args!("{:08X}", self.id.entity),
                    frame = command.frame,
                    buttons = format_args!("{:#x}", command.buttons),
                    step = ?step,
                    volleys = %self.volleys(),
                    "command buttons changed"
                );
            }
        }
        if !self
            .runtime
            .snapshot()
            .is_some_and(Snapshot::initialization_complete)
        {
            return Err(invalid("body VM reached an unsupported state"));
        }
        if step == BodyStep::UnposedTimer {
            // Combat discards the neutral interval's reports at its boundary.
            // Drain interior reports early so a long gap cannot fill the VM queue.
            self.runtime.take_shots(None, mover);
            return Ok(Vec::new());
        }
        if let Some(modifiers) = self.runtime.movement_modifiers() {
            mover.track_modifiers(command.frame, modifiers);
        } else if self.corrections.is_some() {
            return Err(invalid("body VM lost its movement projection"));
        }
        if let Some(history) = self
            .corrections
            .as_mut()
            .filter(|_| step == BodyStep::Posed)
        {
            history.record(mover.pose());
        }
        let mut hits = Vec::new();
        let shots = self
            .runtime
            .take_shots((step == BodyStep::Posed).then_some(command), mover);
        if dead {
            return Ok(hits);
        }
        for shot in shots {
            match shot {
                PosedShot::Projectile(shot) => self.launch_projectile(*shot),
                hit => hits.push(hit),
            }
        }
        Ok(hits)
    }

    /// projectiles.py `fired` for a leased projectile: the Biotic Field
    /// canister leaves its field where it lands, another spawning volley
    /// (Torbjörn's turret, Zarya's Graviton Surge) its entity; any other
    /// flies, and the other clients see it (one copy, as Python, whatever
    /// the volley's count of projectiles a shot), while each of the owner's
    /// projectiles of the shot is confirmed. combat.py `_shoot`: an
    /// unleased one flies for its damage alone. A shot of pellet rings
    /// flies each pellet for its damage (upstream 7cb4018); a leased one's
    /// confirmation and release still follow one projectile along the aim,
    /// which then does no damage, so the wire is as without rings.
    fn launch_projectile(&mut self, projectile: PosedProjectile) {
        let bound = (projectile.ballistics.bound() / TICK_PERIOD.as_secs_f64()).ceil() as u32;
        let definition = projectile.ballistics.definition;
        let damaging = projectile.ballistics.damaging();
        let rings = projectile.pellets.len() > 1;
        let pellets: Vec<_> = if rings && damaging {
            projectile
                .pellets
                .iter()
                .filter_map(|&pellet| {
                    ProjectileFlight::new(
                        projectile.ballistics.clone(),
                        projectile.origin,
                        pellet,
                        None,
                    )
                })
                .collect()
        } else {
            Vec::new()
        };
        let tracked = if rings {
            ProjectileBallistics {
                direct: None,
                blast: None,
                ..projectile.ballistics
            }
        } else {
            projectile.ballistics
        };
        let Some(flight) =
            ProjectileFlight::new(tracked, projectile.origin, projectile.direction, None)
        else {
            return;
        };
        let token = match (&mut self.projectiles, definition) {
            (Some(projectiles), Some(definition)) if projectile.leased => {
                match (projectile.field, projectile.spawn) {
                    (Some(field), _) => projectiles.admit_field(
                        projectile.shot,
                        definition,
                        bound,
                        projectile.feet,
                        *field,
                    ),
                    (None, Some(_)) => {
                        projectiles.admit_spawning(projectile.shot, definition, bound)
                    }
                    (None, None) => projectiles.admit_volley(
                        projectile.shot,
                        definition,
                        bound,
                        projectile.projectiles,
                    ),
                }
                .map(|token| (token, definition))
            }
            _ => None,
        };
        self.flights
            .extend(pellets.into_iter().map(|pellet| (None, pellet)));
        if token.is_none() && (rings || !damaging) {
            return;
        }
        self.flights.push((token.map(|(token, _)| token), flight));
        if let Some((token, definition)) = token.filter(|_| projectile.spawn.is_none()) {
            self.fired.push(FiredProjectile {
                body: self.id,
                token,
                shot: projectile.shot,
                definition,
                volleys: u16::from(projectile.volleys),
                origin: projectile.origin,
                direction: projectile.direction,
                bound,
            });
        }
    }

    /// The next owner frame. While the body is dead, `kill_feed` says whether
    /// its kill feed pulse may go now (match.py `_body_frames`:
    /// `kill_feed_snapshot`).
    fn next_record(
        &mut self,
        created: bool,
        kill_feed: Option<bool>,
    ) -> io::Result<Option<(Record, Option<u32>)>> {
        if !created {
            return Ok(None);
        }
        let encoder = self.runtime.encoder();
        let snapshot = self
            .runtime
            .snapshot()
            .ok_or_else(|| invalid("body VM is unavailable"))?;
        let (chunk, kind) = if self.stream.last() == 0 {
            let snapshot = encoder.project(snapshot, View::Owner).map_err(invalid)?;
            (
                encoder
                    .queue_full(&mut self.stream, &snapshot)
                    .map_err(invalid)?,
                None,
            )
        } else {
            let replace_initial =
                matches!(self.unsent_delta, Some((_, DeltaKind::InitialOwner(_))));
            let Some(kind) = self
                .stream
                .owner_kind(self.frame, false, replace_initial)
                .map_err(invalid)?
            else {
                return Ok(None);
            };
            let (base_last, base) = self
                .stream
                .base()
                .expect("owner delta requires the baseline");
            tracing::debug!(target: "orpheus::owner", base = base_last, "owner delta base");
            let mut snapshot = encoder.project_next(snapshot, base).map_err(invalid)?;
            if let Some(allowed) = kill_feed {
                snapshot = snapshot.gate_kill_feed(encoder.graphs(), &self.stream, allowed);
            }
            let chunk = encoder
                .queue_delta(&mut self.stream, &snapshot, kind, self.runtime.frame_ms())
                .map_err(invalid)?
                .expect("owner deltas include a command frame");
            (chunk, Some(kind))
        };
        // An individual chunk that cannot fit alone needs fragmentation.
        // Fail this VM closed so it cannot hold up other entity records.
        let packet = frame(u32::MAX, u32::MAX, &[], &[], &[&chunk.update]).map_err(invalid)?;
        if packet.len() > MAX_PAYLOAD {
            return Err(invalid(
                "body script chunk exceeds the game datagram budget",
            ));
        }
        let last = chunk.last;
        let replaced = kind.and_then(|kind| {
            self.unsent_delta.replace((last, kind)).map(|(last, _)| {
                self.stream.discard_unsent(last);
                last
            })
        });
        tracing::debug!(
            target: "orpheus::owner",
            body = format_args!("{:08X}", self.id.entity),
            chunk = last,
            kind = ?kind,
            processed = self.frame,
            bytes = packet.len(),
            replaced = ?replaced,
            volleys = %self.volleys(),
            "owner frame queued"
        );
        let record = Record {
            update: chunk.update,
            delivery: Delivery::BodyScript {
                id: self.id,
                last,
                delta: kind.is_some(),
            },
        };
        Ok(Some((record, replaced)))
    }

    /// The body's weapon volleys for the `orpheus::owner` trace: each active
    /// one as `instance/state #activation start=ms ammo0 var=value`.
    fn volleys(&self) -> String {
        use std::fmt::Write;

        let mut out = String::new();
        let Some(snapshot) = self.runtime.snapshot() else {
            return out;
        };
        for (id, instance) in &snapshot.instances {
            for (raw, state) in &instance.states {
                let orpheus_script::vm::StateKind::WeaponVolley {
                    ammo0, ammo_var, ..
                } = state.kind
                else {
                    continue;
                };
                let value = ammo_var.and_then(|var| instance.variables.get(&var.id));
                let _ = write!(
                    out,
                    "{id}/{raw}{} #{} start={} ammo0={ammo0} var={value:?}; ",
                    if state.active { "" } else { "(ended)" },
                    state.activation,
                    state.start_ms,
                );
            }
        }
        out
    }

    fn sent(&mut self, last: u32) {
        if let Some((report, projectiles)) = self.stream.sent(last).zip(self.projectiles.as_mut()) {
            projectiles.owner_sent(
                last,
                report.command_frame,
                |instance, raw| {
                    report
                        .changes
                        .contains(&ChangeKey::State(instance, raw))
                        .then(|| report.snapshot.volley_identity(instance, raw))
                        .flatten()
                },
                |instance, raw| {
                    report
                        .changes
                        .contains(&ChangeKey::VolleyIntroduction(instance, raw))
                },
            );
        }
        if self.unsent_delta.is_some_and(|(queued, _)| queued == last) {
            self.unsent_delta = None;
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Lifetime {
    Live,
    Retiring,
    ReusableAt(Duration),
}

#[derive(Clone, Copy)]
enum Delivery {
    Baseline,
    BotCreate {
        after: Option<u32>,
    },
    Health {
        revision: u64,
    },
    /// The player's Tab board numbers (match.py `_send_board`); a newer one
    /// supersedes it.
    Board {
        revision: u64,
    },
    BotDestroy,
    CreateBody(BodyId),
    Possession(BodyId),
    DestroyBody(BodyId),
    BodyScript {
        id: BodyId,
        last: u32,
        delta: bool,
    },
    ProjectileConfirm {
        id: BodyId,
        token: ProjectileToken,
    },
    ProjectileDestroy {
        id: BodyId,
        token: ProjectileToken,
    },
    Script {
        last: u32,
        close: bool,
        resend: bool,
    },
    /// Another player's entity, body or statescript (others.rs).
    Other(OtherDelivery),
    /// A Biotic Field entity: its create, frames and destroy
    /// (projectiles.py `_deliver_field`, `FieldArrived`, `_end`).
    Field(FieldDelivery),
    /// A training bot's statescript chunk (bot_script.rs).
    BotFrames {
        last: u32,
    },
    /// A firing bot's rocket on this client (bot_script.rs `SeenDelivery`).
    BotSeen(SeenDelivery),
    /// A respawned bot's stand-in controller now controls `body`, its new
    /// body (`scene.rs` `bot_controller_link`): it goes with or after the
    /// body's create, and a newer respawn's link supersedes it.
    BotLink {
        body: u32,
    },
    /// The player's spawn room flag (component 74 field 6, `scene.rs`
    /// `spawn_room_flag`); a newer one supersedes it.
    SpawnFlag {
        revision: u64,
    },
}

/// How far a bot got on this client's screen (`WorldDelivery::bot_shown`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BotSight {
    /// Its first movement record went out in this datagram.
    Sent(u32),
    /// That datagram arrived; the commands of the datagram that said so
    /// were made before the client could draw it.
    Arrived,
    /// Drawn for commands after this frame (none: all of them).
    Shown(Option<u32>),
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum BotLifetime {
    Live,
    Retiring,
}

pub struct Record {
    update: EntityUpdate,
    delivery: Delivery,
}

impl Record {
    pub(crate) fn baseline(update: EntityUpdate) -> Self {
        Self {
            update,
            delivery: Delivery::Baseline,
        }
    }

    fn script(chunk: ScriptChunk, close: bool) -> Self {
        Self {
            update: chunk.update,
            delivery: Delivery::Script {
                last: chunk.last,
                close,
                resend: chunk.resend,
            },
        }
    }
}

pub(crate) struct Ready<'a> {
    index: usize,
    record: &'a Arc<Record>,
}

impl Ready<'_> {
    pub(crate) fn tracked(&self) -> Arc<Record> {
        Arc::clone(self.record)
    }
}

impl Borrow<EntityUpdate> for Ready<'_> {
    fn borrow(&self) -> &EntityUpdate {
        &self.record.update
    }
}

enum InputWork {
    Message {
        message: Message,
        tick: u32,
        received: Duration,
    },
    Skin {
        hero: u64,
        tick: u32,
        received: Duration,
    },
    Command {
        command: Command,
        pending: Option<PendingCommand>,
    },
    Leave,
}

/// The frames of the body's script before a command that its mover can't
/// pose: the first missing frame and the one before the command run the
/// body's last input held ([`repeated`]), the frames between only its
/// timers. A held button stays held across the gap: a neutral frame there
/// released and pressed it again, which the client never did (HitPredict,
/// item 1: a second shot, the ammo counter going up and down).
#[derive(Default)]
struct ScriptGap {
    first: Option<u32>,
    tail: Range<u32>,
}

impl ScriptGap {
    fn before(script_frame: u32, first_pose: u32) -> Self {
        let first_missing = script_frame.saturating_add(1);
        if first_missing >= first_pose {
            return Self::default();
        }
        let last_missing = first_pose - 1;
        Self {
            first: Some(first_missing),
            tail: (first_missing + 1).max(last_missing.saturating_sub(MAX_SCRIPT_TAIL))..first_pose,
        }
    }
}

impl Iterator for ScriptGap {
    type Item = (u32, BodyStep);

    fn next(&mut self) -> Option<Self::Item> {
        if let Some(frame) = self.first.take() {
            return Some((frame, BodyStep::UnposedBoundary));
        }
        let frame = self.tail.next()?;
        let step = if self.tail.is_empty() {
            BodyStep::UnposedBoundary
        } else {
            BodyStep::UnposedTimer
        };
        Some((frame, step))
    }
}

struct PendingCommand {
    poses: Poses,
    gap: ScriptGap,
}

impl PendingCommand {
    fn new(poses: Poses, script: Option<&BodyScript>) -> Self {
        let gap = script
            .zip(poses.next_frame())
            .map_or_else(ScriptGap::default, |(script, first_pose)| {
                ScriptGap::before(script.frame, first_pose)
            });
        Self { poses, gap }
    }
}

/// What a late command showed that the commands run in its place lacked
/// ([`InputQueue::reconcile`]); the next command run takes it.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct LateInput {
    /// Buttons pressed in the late commands.
    pressed: u32,
    /// Buttons released in the late commands.
    released: u32,
    /// The sub-frame click of the last late press.
    click: Option<orpheus_wire::commands::Click>,
    /// The last one-shot action of the late commands.
    action: u8,
}

/// A player's input, in command-frame order (GDC 2017, Ford 30:50; Reed
/// s.42). Commands wait in `buffer` until a tick runs them
/// ([`InputQueue::release`]): one a tick, the next of the client's frames.
/// Frames already run or admitted are dropped (the client's redundant
/// window resends them); later ones wait. A tick that finds no command
/// runs the client's last input again for the next frame (retail
/// duplicates the last input); when the real command of that frame comes,
/// the presses it had and the repeat lacked go with the next command run,
/// so no press is lost. A game message keeps its place among the commands:
/// it runs at the boundary before the first command released after it,
/// the frame the next owner frame names.
#[derive(Default)]
struct InputQueue {
    work: VecDeque<InputWork>,
    /// Commands and messages not yet released, in arrival order.
    buffer: VecDeque<InputWork>,
    /// Input that runs as it comes, with no buffer: an AI player's, made on
    /// the server a tick at a time.
    immediate: bool,
    first_admitted: Option<u32>,
    /// The newest frame admitted: a client's command or a repeat.
    admitted: Option<u32>,
    processed: Option<u32>,
    /// The client's newest command (late ones included): a dry tick
    /// repeats it.
    last: Option<Command>,
    /// The buttons of the last command released.
    ran: u32,
    /// Dry ticks in a row.
    repeats: u32,
    /// Ticks in a row with more than [`BUFFER_TARGET`] commands left waiting.
    surplus: u32,
    /// A command of the client came since the last tick.
    arrived: bool,
    late: LateInput,
    closing: bool,
}

impl InputQueue {
    fn pending(&self) -> usize {
        self.work.len() + self.buffer.len()
    }

    /// The commands in hand for the coming ticks.
    fn depth(&self) -> usize {
        self.buffer
            .iter()
            .filter(|work| matches!(work, InputWork::Command { .. }))
            .count()
    }

    fn push(&mut self, work: InputWork) -> bool {
        if self.closing {
            return false;
        }
        if self.pending() == MAX_PENDING_INPUT {
            self.close_after_prefix();
            return false;
        }
        if self.immediate {
            self.work.push_back(work);
        } else {
            self.buffer.push_back(work);
        }
        true
    }

    fn commands(&mut self, tick: u32, commands: &[Command]) {
        if self.closing || commands.is_empty() {
            return;
        }
        self.reconcile(commands);
        let (fresh, boundary) = command_prefix(self.admitted, tick, commands);
        for command in fresh {
            if !self.push(InputWork::Command {
                command,
                pending: None,
            }) {
                return;
            }
            self.first_admitted.get_or_insert(command.frame);
            self.admitted = Some(command.frame);
            self.last = Some(command);
            self.arrived = true;
        }
        if boundary.is_some() {
            self.close_after_prefix();
        }
    }

    /// The client's commands of frames a dry tick ran with its last input
    /// repeated. Each press and release they had (a button down that was
    /// up in the command before, or the reverse) is kept for the next
    /// command run; the newest becomes the input a dry tick repeats.
    fn reconcile(&mut self, commands: &[Command]) {
        let (Some(mut last), Some(admitted)) = (self.last, self.admitted) else {
            return;
        };
        for &late in commands {
            // Only frames past the client's newest command and up to the
            // newest admitted, the repeats: older ones ran or are duplicates.
            let after = frame_distance(late.frame, last.frame);
            if after == 0
                || after >= FRAME_HALF
                || frame_distance(admitted, late.frame) >= FRAME_HALF
            {
                continue;
            }
            let pressed = late.buttons & !last.buttons & !SUBFRAME;
            self.late.pressed |= pressed;
            self.late.released |= last.buttons & !late.buttons & !SUBFRAME;
            if pressed != 0 && late.click.is_some() {
                self.late.click = late.click;
            }
            if late.action != 0 {
                self.late.action = late.action;
            }
            tracing::debug!(
                target: "orpheus::input",
                frame = late.frame,
                buttons = format_args!("{:#x}", late.buttons),
                pressed = format_args!("{pressed:#x}"),
                "late command of a repeated frame"
            );
            last = late;
        }
        self.last = Some(last);
    }

    /// The input of `ticks` server ticks (one, more after ticks the clock
    /// skipped): the next command in frame order each, with the messages
    /// before it. More when more than [`MAX_BUFFERED`] would wait, or when
    /// more than [`BUFFER_TARGET`] waited for [`DRAIN_TICKS`] ticks. None
    /// when no more than the target is in hand while the client sends (the
    /// buffer grows by one). With no command in hand, the messages, then
    /// the client's last input repeated for the next frame while his body
    /// is in the world (`body`).
    fn release(&mut self, ticks: u32, body: bool) {
        if self.immediate || self.closing {
            return;
        }
        let arrived = std::mem::take(&mut self.arrived);
        let depth = self.depth();
        if depth == 0 {
            self.surplus = 0;
            self.work.extend(self.buffer.drain(..));
            self.repeat(ticks, body);
            return;
        }
        self.repeats = 0;
        if depth <= BUFFER_TARGET && arrived && ticks == 1 {
            tracing::debug!(
                target: "orpheus::input",
                depth,
                "input buffer grows: the command waits a tick"
            );
            return;
        }
        let mut count = (ticks as usize).max(depth.saturating_sub(MAX_BUFFERED));
        self.surplus = if depth.saturating_sub(count) > BUFFER_TARGET {
            self.surplus + 1
        } else {
            0
        };
        if self.surplus >= DRAIN_TICKS {
            self.surplus = 0;
            count += 1;
        }
        let count = count.min(depth);
        if count > 1 {
            tracing::debug!(
                target: "orpheus::input",
                depth,
                count,
                "input buffer catches up"
            );
        }
        let mut released = 0;
        while released < count {
            let work = match self.buffer.pop_front().expect("buffered command") {
                InputWork::Command { command, pending } => {
                    released += 1;
                    InputWork::Command {
                        command: self.take_late(command),
                        pending,
                    }
                }
                work => work,
            };
            self.work.push_back(work);
        }
    }

    /// A dry buffer: the client's last input again, for the next frames.
    fn repeat(&mut self, ticks: u32, body: bool) {
        let Some(last) = self.last.filter(|_| body) else {
            return;
        };
        for _ in 0..ticks.min(MAX_REPEATS.saturating_sub(self.repeats)) {
            let Some(frame) = self
                .admitted
                .map(|frame| frame.wrapping_add(1))
                .filter(|&frame| frame <= MAX_LEASE_FRAME)
            else {
                return;
            };
            let command = self.take_late(repeated(last, frame));
            tracing::debug!(
                target: "orpheus::input",
                frame,
                buttons = format_args!("{:#x}", command.buttons),
                "input buffer dry: the last input repeated"
            );
            self.repeats += 1;
            self.admitted = Some(frame);
            self.work.push_back(InputWork::Command {
                command,
                pending: None,
            });
        }
    }

    /// `command` with the late presses and releases
    /// ([`InputQueue::reconcile`]): a button the repeats held through a
    /// late release is up for this frame if this command holds it (the
    /// next presses it again); one they never pressed is down for this
    /// frame if this command doesn't hold it. The click and the action go
    /// with it. A press or release can come a few frames late, never twice
    /// or not at all.
    fn take_late(&mut self, mut command: Command) -> Command {
        let late = std::mem::take(&mut self.late);
        let release = late.released & self.ran & command.buttons;
        let press = late.pressed & !self.ran & !command.buttons;
        command.buttons = (command.buttons & !release) | press;
        if press != 0 && command.click.is_none() {
            if let Some(click) = late.click {
                command.click = Some(click);
                command.buttons |= SUBFRAME;
            }
        }
        if command.action == 0 {
            command.action = late.action;
        }
        self.ran = command.buttons;
        command
    }

    /// The client's newest command frame.
    fn newest_real(&self) -> Option<u32> {
        self.last.map(|last| last.frame)
    }

    fn close_after_prefix(&mut self) {
        while let Some(work) = self.buffer.pop_front() {
            let work = match work {
                InputWork::Command { command, pending } => InputWork::Command {
                    command: self.take_late(command),
                    pending,
                },
                work => work,
            };
            self.work.push_back(work);
        }
        self.work.push_back(InputWork::Leave);
        self.closing = true;
    }

    fn clear(&mut self) {
        self.work.clear();
        self.buffer.clear();
        self.admitted = self.processed;
        self.last = None;
        self.ran = 0;
        self.repeats = 0;
        self.surplus = 0;
        self.arrived = false;
        self.late = LateInput::default();
        self.closing = false;
    }
}

/// Hero switches waiting for their controller frame (notices.py `Notices`).
#[derive(Default)]
struct SwitchNotices {
    waiting: VecDeque<Switch>,
    /// The controller chunk of the last notice frame.
    chunk: Option<u32>,
    sent_at: Option<Duration>,
}

/// One player of a room for [`show_players`]: his place in the match and
/// his peer's world, if his game is connected.
pub struct RoomPlayer<'a, 'w> {
    pub appearance: PlayerAppearance<'a>,
    pub world: Option<&'w mut WorldDelivery>,
}

/// Bring every client of a room up to date on the other players
/// (match.py `update`: `_show_players`, `ensure_remote_body`,
/// `Observers.update`, `Notices.update`). Each player's view is built once
/// from his peer; each client's bookkeeping then follows the views. Returns
/// each player's 20308 messages for the reliable channel, by slot.
pub fn show_players(
    scene: &PreparedScene,
    players: &mut [RoomPlayer<'_, '_>],
    tick: u32,
    now: Duration,
) -> Vec<Vec<Message>> {
    let controller = scene.controller();
    let entry = move |hero| controller.team_entry_frame(hero);
    let context = ShowContext {
        map: scene.map(),
        pools: scene.pools(),
        entry: &entry,
        tick,
        switch_notices: scene.switch_notices(),
    };
    let mut taken: Vec<_> = players
        .iter_mut()
        .map(|player| {
            player
                .world
                .as_deref_mut()
                .filter(|world| world.world_started() && !world.headless)
                .map(|world| world.others.take())
        })
        .collect();
    let outputs: Vec<_> = {
        // A body is projected for observers only when another client of
        // the room takes its frames.
        let observed = |slot: usize, body: BodyId| {
            taken.iter().enumerate().any(|(viewer, others)| {
                viewer != slot && others.as_ref().is_some_and(|others| others.observes(body))
            })
        };
        let views: Vec<_> = players
            .iter()
            .enumerate()
            .map(|(slot, player)| match player.world.as_deref() {
                Some(world) => {
                    world.player_view(&player.appearance, scene, |body| observed(slot, body))
                }
                None => PlayerView::absent(&player.appearance),
            })
            .collect();
        // What each client can name in a kill feed line besides the other
        // players' bodies (combat.py `kill_feed_allowed`).
        let readiness: Vec<_> = players
            .iter()
            .map(|player| {
                player
                    .world
                    .as_deref()
                    .map_or_else(ViewerReadiness::default, WorldDelivery::readiness)
            })
            .collect();
        taken
            .iter_mut()
            .enumerate()
            .map(|(slot, others)| {
                Some(
                    others
                        .as_mut()?
                        .show(&context, &views, slot, &readiness[slot]),
                )
            })
            .collect()
    };
    players
        .iter_mut()
        .zip(taken)
        .zip(outputs)
        .map(|((player, others), output)| {
            let (Some(world), Some(others)) = (player.world.as_deref_mut(), others) else {
                return Vec::new();
            };
            world.others = others;
            let cards = match output {
                Some(Ok(output)) => world.apply_shown(output),
                _ => Vec::new(),
            };
            if let Err(error) = world.send_notices(scene, tick, now) {
                eprintln!(
                    "hero switch notice for {:08X} failed ({error})",
                    world.owner
                );
            }
            if let Err(error) = world.send_multikills(scene, tick) {
                eprintln!("multikill pulse for {:08X} failed ({error})", world.owner);
            }
            cards
        })
        .collect()
}

pub struct WorldDelivery {
    owner: u32,
    queued: Vec<Arc<Record>>,
    created: HashSet<u32>,
    bots: HashMap<u32, BotLifetime>,
    /// Each live bot's way onto this client's screen (`bot_shown`).
    bot_sights: HashMap<u32, BotSight>,
    health_updates: HashMap<u32, u64>,
    health_revision: u64,
    started: Option<u32>,
    pub presentation_due: bool,
    controller: Stream,
    mode: Stream,
    select_open: bool,
    body: Option<Body>,
    bound: Option<BodyId>,
    generation: u64,
    bodies: HashMap<u32, (u64, Lifetime)>,
    input: InputQueue,
    mover: Option<Mover>,
    /// The other bodies that block the moves of this player's body
    /// ([`WorldDelivery::set_blockers`]).
    blockers: Vec<CapsulePose>,
    body_script: Option<BodyScript>,
    /// The body's script stopped on a fault: a pick of the same hero gets a
    /// new body with a new script (match.py `body_script_failed`).
    script_failed: bool,
    projectile_destroys: HashMap<u32, (BodyId, ProjectileToken)>,
    /// What the player's projectiles ask of the room since the match last
    /// took it (projectiles.py `_end`, `_field`).
    projectile_events: Vec<(BodyId, ProjectileEvent)>,
    /// The Biotic Field entities this client has (projectiles.py
    /// `Shot.field_ids`).
    fields: FieldCopies,
    /// The hits and melees of the processed input, for the match.
    shots: Vec<PosedShot>,
    others: Others,
    /// The player's last hero switch, for the chat lines.
    switch: Option<Switch>,
    switches: u64,
    notices: SwitchNotices,
    /// Bodies of this player's ring that other clients still reference.
    retained_elsewhere: HashSet<u32>,
    /// The training bots' statescript on this client.
    bot_views: BotViews,
    /// The bodies the target searches can find, as the match last placed
    /// them; a new body starts with them.
    target_candidates: Vec<TargetCandidate>,
    /// The current body's death and respawn (combat.py `Vitals.died_at`).
    death: Option<BodyDeath>,
    /// The pools of a body that runs no script (combat.py `Vitals` from
    /// `pools.body_pools`).
    plain_vitals: Option<Vitals>,
    /// Changes of the current body's pools, for the other clients.
    body_health: u64,
    board: BoardDelivery,
    /// The PvP "assemble your team" countdown runs (match.py `assembling`):
    /// a pick keeps hero select open, which shows the skin selector.
    assembling: bool,
    /// An AI player's world (`crate::ai`): no client receives it, so it
    /// views nobody, and its records arrive as soon as they are ready
    /// ([`Self::deliver_locally`]).
    headless: bool,
    /// Time the match flow adds to a death's respawn now (overtime,
    /// game-rules.md 11).
    respawn_extra: Duration,
    /// The hero settings the player's client last sent (21604); every body
    /// he plays takes them. They are the client's, so a new world keeps them.
    hero_settings: HeroSettings,
    /// The voice of the current body (its entity): its stimuli's cooldowns
    /// and lines' turns ([`crate::voice_lines`]).
    voice: Option<(u32, VoiceState)>,
    /// Whether the living body stood in one of its team's spawn rooms when
    /// last measured ([`Self::area_step`]); `None` when it is dead or gone,
    /// or its team has no room on the map.
    spawn_room: Option<bool>,
    /// The spawn room flag (component 74 field 6) as last queued, and its
    /// revision; the player's create sends it set.
    spawn_flag: (bool, u64),
    /// A hero picked while the body was dead: the respawn plays it.
    respawn_hero: Option<(u64, SkinTheme)>,
    /// The player's multikills (`crate::announcer`), and whether a pulse
    /// waits for its controller frame.
    multikills: Multikills,
    multikill_due: bool,
}

/// The entity variables of the hero settings the client sends
/// (`orpheus_content::hero_settings`): the setting identifier's index holds
/// the chosen entry's identifier, or nothing (null) at DEFAULT, which the
/// client sends as no entry.
pub fn hero_setting_variables(settings: &HeroSettings, hero: u64) -> Variables {
    SCRIPT_SETTINGS
        .iter()
        .map(|&setting| {
            let value = match settings.value(hero, setting) {
                None | Some(0) => ScriptValue::Null,
                Some(value) => ScriptValue::Asset(value),
            };
            (setting as u32, value)
        })
        .collect()
}

/// A body's death (combat.py `Vitals.death_tick`, `killer`) and when it
/// respawns (`Combat.respawns`).
#[derive(Clone, Copy, Debug)]
struct BodyDeath {
    body: BodyId,
    killer: u32,
    tick: u32,
    respawn_at: Duration,
}

/// How a hit on the player's body ended (combat.py `_damage`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BodyDamage {
    pub dealt: f64,
    pub killed: bool,
}

/// The Tab board the client has (match.py `stats_sent`, `next_stats`).
#[derive(Default)]
struct BoardDelivery {
    sent: Option<WorldComponent>,
    next: Duration,
    revision: u64,
}

/// combat.py `kill_feed_allowed`: a death's pulse goes out first only
/// while it is fresh (`ClientOnlyPulser.FRESH_MS`, 500 ms of world ticks).
const KILL_FEED_FRESH_MS: u64 = 500;

/// match.py `STATS_EVERY`.
pub const STATS_EVERY: Duration = Duration::from_millis(500);

pub(crate) fn kill_feed_fresh(tick: u32, death_tick: u32) -> bool {
    tick >= death_tick
        && u64::from(tick - death_tick) * TICK_PERIOD.as_millis() as u64 <= KILL_FEED_FRESH_MS
}

impl WorldDelivery {
    pub fn new(owner: u32) -> Self {
        Self {
            owner,
            queued: Vec::new(),
            created: HashSet::new(),
            bots: HashMap::new(),
            bot_sights: HashMap::new(),
            health_updates: HashMap::new(),
            health_revision: 0,
            started: None,
            presentation_due: false,
            controller: Stream::new(owner),
            mode: Stream::new(GAME_MODE_ENTITY),
            select_open: true,
            body: None,
            bound: None,
            generation: 0,
            bodies: HashMap::new(),
            input: InputQueue::default(),
            mover: None,
            blockers: Vec::new(),
            body_script: None,
            script_failed: false,
            projectile_destroys: HashMap::new(),
            projectile_events: Vec::new(),
            fields: FieldCopies::default(),
            shots: Vec::new(),
            others: Others::default(),
            switch: None,
            switches: 0,
            notices: SwitchNotices::default(),
            retained_elsewhere: HashSet::new(),
            bot_views: BotViews::default(),
            target_candidates: Vec::new(),
            death: None,
            plain_vitals: None,
            body_health: 0,
            board: BoardDelivery::default(),
            assembling: false,
            headless: false,
            respawn_extra: Duration::ZERO,
            hero_settings: HeroSettings::default(),
            voice: None,
            spawn_room: None,
            spawn_flag: (true, 0),
            respawn_hero: None,
            multikills: Multikills::default(),
            multikill_due: false,
        }
    }

    /// The world of an AI player, whose body the server plays without a
    /// client (`crate::ai`).
    pub fn headless(owner: u32) -> Self {
        let mut world = Self {
            headless: true,
            ..Self::new(owner)
        };
        world.input.immediate = true;
        world
    }

    /// Whether this is an AI player's world.
    pub fn is_headless(&self) -> bool {
        self.headless
    }

    /// An AI player's send frame: what a client would get this frame
    /// arrives at once, so his body's lifetime, statescript and pools go on
    /// as a connected player's do. An arrival can make more records ready
    /// (a body's possession after its create), so it repeats while records
    /// are ready, a few times at most.
    pub fn deliver_locally(&mut self, tick: u32, now: Duration) {
        self.prepare_body_updates(tick);
        for _ in 0..4 {
            let ready = self.ready();
            if ready.is_empty() {
                break;
            }
            let records: Vec<Arc<Record>> = ready.iter().map(Ready::tracked).collect();
            let indexes: Vec<usize> = ready.iter().map(|ready| ready.index).collect();
            self.sent(&indexes);
            self.acknowledge(
                DeliveryReport {
                    requeued_reliable: 0,
                    packets: vec![PacketDelivery {
                        sequence: 0,
                        arrived: true,
                        records,
                    }],
                },
                now,
            );
        }
        // What a client would get arrives at once: every bot it has is drawn.
        for (&entity, &lifetime) in &self.bots {
            if lifetime == BotLifetime::Live && self.created.contains(&entity) {
                self.bot_sights.insert(entity, BotSight::Shown(None));
            }
        }
    }

    /// The player's body takes hero picks: his controller went out and
    /// his entity arrived (server.py `can_handle_game_message`).
    pub fn takes_game_messages(&self) -> bool {
        self.can_handle_game_message()
    }

    /// His living body's health as a share of its most.
    pub fn health_share(&self) -> Option<f64> {
        self.body?;
        let vitals = self
            .body_script
            .as_ref()
            .and_then(|script| script.runtime.vitals())
            .or(self.plain_vitals.as_ref())?;
        let most = vitals.maximum();
        (most > 0.0).then(|| vitals.total() / most)
    }

    pub fn clear(&mut self) {
        self.queued.clear();
        self.created.clear();
        self.bots.clear();
        self.bot_sights.clear();
        self.health_updates.clear();
        self.started = None;
        self.presentation_due = false;
        self.mover = None;
        self.body_script = None;
        self.script_failed = false;
        self.projectile_destroys.clear();
        self.projectile_events.clear();
        self.fields = FieldCopies::default();
        self.input.clear();
        self.shots.clear();
        self.death = None;
        self.plain_vitals = None;
        self.board = BoardDelivery::default();
        self.others.clear();
        self.bot_views.clear();
        self.target_candidates.clear();
        self.notices = SwitchNotices::default();
        self.spawn_room = None;
        self.multikills = Multikills::default();
        self.multikill_due = false;
    }

    /// This player as the other clients of the room see him this tick. His
    /// body's statescript is projected for observers when `observed`.
    fn player_view<'a>(
        &'a self,
        player: &PlayerAppearance<'a>,
        scene: &'a PreparedScene,
        observed: impl Fn(BodyId) -> bool,
    ) -> PlayerView<'a> {
        let mut view = PlayerView::absent(player);
        let Some(entered) = self.started else {
            return view;
        };
        view.entered = Some(entered);
        view.presented = self.controller.latest_data() != 0;
        view.switch = self.switch;
        view.body = self
            .body
            .zip(self.mover.as_ref())
            .and_then(|(body, mover)| {
                let runtime = self
                    .body_script
                    .as_ref()
                    .filter(|script| script.id == body.id)
                    .map(|script| &script.runtime);
                let death = self
                    .death
                    .filter(|death| death.body == body.id)
                    .map(|death| RemoteDeath {
                        tick: death.tick,
                        killer: death.killer,
                    });
                let mut pose = mover.pose();
                // match.py `Player.movement`: a dead body's records carry
                // combat.py `DEAD`, the 3P animation's dead pose.
                if death.is_some() {
                    pose.flags |= DEAD;
                }
                let hero = scene.heroes().get(body.hero)?;
                Some(RemoteBody {
                    id: body.id,
                    hero,
                    gravity: scene.mover(hero.body).gravity_scale,
                    skin: body.skin,
                    pose,
                    vitals: runtime
                        .and_then(|runtime| runtime.vitals())
                        .or(self.plain_vitals.as_ref()),
                    health: self.body_health,
                    death,
                    script: runtime.filter(|_| observed(body.id)).and_then(|runtime| {
                        let encoder = runtime.encoder();
                        let snapshot = runtime.snapshot()?;
                        let projected = encoder.project(snapshot, View::Observer).ok()?;
                        Some(ObservedScript {
                            encoder,
                            snapshot,
                            projected,
                            activity: runtime.observer_activity(),
                            frame_ms: runtime.frame_ms(),
                        })
                    }),
                })
            });
        view
    }

    /// Queue what [`Others::show`] made for this client; its cards go to the
    /// reliable channel.
    fn apply_shown(&mut self, output: ShowOutput) -> Vec<Message> {
        for (update, delivery) in output.records {
            self.push(Record {
                update,
                delivery: Delivery::Other(delivery),
            });
        }
        self.notices.waiting.extend(output.switches);
        self.remove_stale();
        output.cards
    }

    /// The bodies of `owner`'s ring this client may still reference
    /// (server.py `body_id_available`): a new body of his takes none of them.
    pub fn retained_bodies(&self, owner: u32, now: Duration) -> impl Iterator<Item = u32> + '_ {
        self.others.retained(owner, now)
    }

    /// notices.py `Notices.update`: a waiting switch goes in a controller
    /// frame of its own once the client has every controller frame sent
    /// before, NOTICE_GAP after the last one. After a notice frame, a plain
    /// frame goes first: it clears the three variables without a line.
    fn send_notices(&mut self, scene: &PreparedScene, tick: u32, now: Duration) -> io::Result<()> {
        if self.notices.waiting.is_empty() || !self.can_handle_game_message() {
            return Ok(());
        }
        if self.controller.data_in_flight()
            || self
                .notices
                .sent_at
                .is_some_and(|at| now < at.saturating_add(NOTICE_GAP))
        {
            return Ok(());
        }
        let (open, hero) = (self.select_open, self.body.map(|body| body.hero));
        if self.notices.chunk == Some(self.controller.latest_data()) {
            self.queue_controller(scene, tick, open, hero, None)?;
        } else {
            let switch = self.notices.waiting.pop_front().expect("waiting switch");
            self.queue_controller(scene, tick, open, hero, Some(&switch.presence()))?;
            self.notices.chunk = Some(self.controller.latest_data());
        }
        self.notices.sent_at = Some(now);
        Ok(())
    }

    /// The player eliminated an enemy at world tick `tick` (`0215`'s
    /// `0093.025`, [`crate::announcer`]).
    pub fn eliminated(&mut self, tick: u32) {
        if self.multikills.eliminated(tick) {
            self.multikill_due = true;
        }
    }

    /// A multikill pulse goes out at once in a controller frame, which
    /// carries `0215` from then on ([`crate::announcer`]).
    fn send_multikills(&mut self, scene: &PreparedScene, tick: u32) -> io::Result<()> {
        if !std::mem::take(&mut self.multikill_due)
            || self.headless
            || self.controller.latest_data() == 0
        {
            return Ok(());
        }
        let (open, hero) = (self.select_open, self.body.map(|body| body.hero));
        self.queue_controller(scene, tick, open, hero, None)
    }

    fn push(&mut self, record: Record) {
        self.queued.push(Arc::new(record));
    }

    pub fn world_started(&self) -> bool {
        self.started.is_some()
    }

    pub fn identity_ready(&self, entity: u32) -> bool {
        self.world_started() && self.created.contains(&entity)
    }

    /// Whether this client's command at `frame` was made with bot `entity`
    /// on its screen, so his shot or melee then can strike it: its create
    /// arrived, a datagram with its first movement record arrived (the
    /// client places a mover only from its movement records; Prometheus
    /// (0.8 beta, facts only): the movement-state history alone places it,
    /// not component 1), and the command came in a later datagram than the
    /// one that said so. Before, the server let a respawned bot be hit as
    /// soon as its create arrived, while the client showed nothing there.
    /// Beyond Python (server.py `combat_identity_ready` alone). A rewind
    /// of the targets to the shooter's view would place the bot by its
    /// history instead.
    pub fn bot_shown(&self, entity: u32, frame: u32) -> bool {
        self.identity_ready(entity)
            && matches!(
                self.bot_sights.get(&entity),
                Some(BotSight::Shown(from))
                    if from.is_none_or(|from| (frame.wrapping_sub(from) as i32) > 0)
            )
    }

    /// The client's ACK window of a datagram (`ack`, `ack_bits`), for the
    /// bots' first movement records ([`WorldDelivery::bot_shown`]): their
    /// datagram arrived, or was lost and the next record's counts.
    pub fn acknowledge_movement(&mut self, ack: u32, ack_bits: u64) {
        self.bot_sights.retain(|_, sight| match *sight {
            BotSight::Sent(sequence) => match delivery(sequence, ack, ack_bits) {
                Some(true) => {
                    *sight = BotSight::Arrived;
                    true
                }
                Some(false) => false,
                None => true,
            },
            _ => true,
        });
    }

    pub fn mode_ready(&self) -> bool {
        self.mode.delivered() != 0
    }

    pub fn health_pending(&self, entity: u32) -> bool {
        self.health_updates.contains_key(&entity)
    }

    pub fn queue_health(&mut self, update: EntityUpdate) {
        let entity = update.entity;
        let live_bot = self.bots.get(&entity) == Some(&BotLifetime::Live);
        let live_body = self.body.is_some_and(|body| {
            body.id.entity == entity
                && self.bodies.get(&entity) == Some(&(body.id.generation, Lifetime::Live))
        });
        if !live_bot && !live_body {
            return;
        }
        self.health_revision = self.health_revision.wrapping_add(1);
        let revision = self.health_revision;
        self.health_updates.insert(entity, revision);
        self.remove_stale();
        self.push(Record {
            update,
            delivery: Delivery::Health { revision },
        });
    }

    /// The training bots' statescript on this client this tick: new bot
    /// lifetimes' first frames, the room's live and death frames, what this
    /// client's ACKs made due and the practice nameplates
    /// (server.py `queue_bot_live`, `queue_bot_death`, `_flush_bot_frames`,
    /// `_flush_bot_nameplates`).
    pub fn update_bots(&mut self, scene: &PreparedScene, events: &[BotEvent], tick: u32) {
        if self.started.is_none() {
            return;
        }
        let live: Vec<u32> = self
            .bots
            .iter()
            .filter(|&(_, lifetime)| *lifetime == BotLifetime::Live)
            .map(|(&entity, _)| entity)
            .collect();
        let mut views = std::mem::take(&mut self.bot_views);
        let identity = |entity: u32| self.combat_identity_ready(entity);
        let context = BotViewContext {
            tick,
            mode_ready: self.mode_ready(),
            identity: &identity,
            live: &live,
        };
        let chunks = views.update(scene.bot_scripts(), events, &context);
        // A lifetime registered after its create already arrived (a fast
        // ACK between ticks) may send its first frame at once.
        for &entity in live.iter().filter(|&entity| self.created.contains(entity)) {
            views.create_arrived(entity);
        }
        self.bot_views = views;
        for chunk in chunks {
            self.push(Record {
                update: chunk.update,
                delivery: Delivery::BotFrames { last: chunk.last },
            });
        }
    }

    /// This bot's statescript frames still owed to this client: its
    /// replacement waits (server.py `_body_destroy_pending`).
    pub fn bot_frames_pending(&self, entity: u32) -> bool {
        self.created.contains(&entity)
            && self.bot_views.pending(entity, self.health_pending(entity))
    }

    /// The bot's replacement waits for this client (server.py
    /// `_hold_body_destroy`): its held rocket volleys stop advancing its
    /// frames.
    pub fn hold_bot_destroy(&mut self, entity: u32) {
        self.bot_views.hold_destroy(entity);
    }

    /// The client's statescript stream of the bot's current lifetime, even
    /// before its baseline arrived (server.py `bot_stream`).
    pub fn bot_stream(&self, entity: u32) -> Option<&BodyStream<ProjectedSnapshot>> {
        self.bot_views.stream(entity)
    }

    pub fn replace_bot(&mut self, replacement: BotReplacement) {
        let old = replacement.retired;
        let new = replacement.created.entity;
        if self.bots.get(&old) != Some(&BotLifetime::Live) || self.bots.contains_key(&new) {
            return;
        }
        self.bots.insert(old, BotLifetime::Retiring);
        self.bot_views.forget(old);
        self.bots.insert(new, BotLifetime::Live);
        self.created.remove(&old);
        self.bot_sights.remove(&old);
        self.health_updates.remove(&old);
        self.remove_stale();
        self.push(Record {
            update: EntityUpdate {
                entity: old,
                operation: Some(Operation::Destroy),
                ..EntityUpdate::default()
            },
            delivery: Delivery::BotDestroy,
        });
        self.push(Record {
            update: replacement.created,
            delivery: Delivery::BotCreate { after: Some(old) },
        });
        if let Some(link) = replacement.link {
            self.push(Record {
                update: link,
                delivery: Delivery::BotLink { body: new },
            });
        }
    }

    fn current(&self, record: &Record) -> bool {
        let entity = record.update.entity;
        match record.delivery {
            Delivery::Baseline | Delivery::Script { close: false, .. } => true,
            Delivery::BotCreate { .. } => self.bots.get(&entity) == Some(&BotLifetime::Live),
            Delivery::BotLink { body } => self.bots.get(&body) == Some(&BotLifetime::Live),
            Delivery::BotDestroy => self.bots.get(&entity) == Some(&BotLifetime::Retiring),
            Delivery::Health { revision } => self.health_updates.get(&entity) == Some(&revision),
            Delivery::Board { revision } => self.board.revision == revision,
            Delivery::SpawnFlag { revision } => self.spawn_flag.1 == revision,
            Delivery::Script {
                last, close: true, ..
            } => last == self.controller.latest_data(),
            Delivery::CreateBody(id) => {
                self.bodies.get(&id.entity) == Some(&(id.generation, Lifetime::Live))
            }
            Delivery::DestroyBody(id) => {
                self.bodies.get(&id.entity) == Some(&(id.generation, Lifetime::Retiring))
            }
            Delivery::Possession(id) => self.body.is_some_and(|body| body.id == id),
            Delivery::BodyScript { id, .. } => self
                .body_script
                .as_ref()
                .is_some_and(|script| script.id == id),
            Delivery::ProjectileConfirm { id, token } => self
                .body_script
                .as_ref()
                .filter(|script| script.id == id)
                .and_then(|script| script.projectiles.as_ref())
                .is_some_and(|projectiles| projectiles.confirmation_current(token, entity)),
            Delivery::ProjectileDestroy { id, token } => {
                self.projectile_destroys.get(&entity) == Some(&(id, token))
            }
            Delivery::Other(other) => self.others.current(other, entity),
            Delivery::Field(field) => self.fields.current(field, entity),
            Delivery::BotFrames { last } => self.bot_views.current(entity, last),
            Delivery::BotSeen(seen) => self.bot_views.seen_current(seen, entity),
        }
    }

    pub fn acknowledge(&mut self, report: DeliveryReport<Arc<Record>>, now: Duration) {
        if self.started.is_none() {
            return;
        }
        let mut retry = Vec::new();
        for packet in report.packets {
            for record in packet.records {
                if !self.current(&record) {
                    continue;
                }
                match (packet.arrived, record.delivery) {
                    (arrived, Delivery::BodyScript { last, .. }) => {
                        let script = self.body_script.as_mut().expect("current body stream");
                        tracing::debug!(
                            target: "orpheus::owner",
                            chunk = last,
                            arrived,
                            "owner frame acknowledged"
                        );
                        let delivery = if arrived {
                            script.stream.arrived(last)
                        } else {
                            script.stream.lost(last)
                        };
                        if let Some((projectiles, arrived)) =
                            script.projectiles.as_mut().zip(delivery.owner_report)
                        {
                            projectiles.owner_arrival(last, arrived);
                        }
                        if delivery.retry {
                            retry.push(record);
                        }
                    }
                    (arrived, Delivery::Other(other)) => {
                        let entity = record.update.entity;
                        if self.others.delivered(other, entity, arrived, now) {
                            retry.push(record);
                        }
                    }
                    (arrived, Delivery::BotSeen(seen)) => {
                        let entity = record.update.entity;
                        if self.bot_views.seen_delivered(seen, entity, arrived) {
                            retry.push(record);
                        }
                    }
                    (arrived, Delivery::BotFrames { last }) => {
                        let entity = record.update.entity;
                        if self.bot_views.delivered(entity, last, arrived) {
                            retry.push(record);
                        }
                    }
                    (false, Delivery::Script { resend: false, .. }) => {}
                    (false, _) => retry.push(record),
                    (true, Delivery::Baseline) => {
                        if matches!(record.update.operation, Some(Operation::Create(_))) {
                            self.created.insert(record.update.entity);
                        }
                    }
                    (true, Delivery::BotCreate { .. }) => {
                        self.created.insert(record.update.entity);
                        self.bot_views.create_arrived(record.update.entity);
                    }
                    (true, Delivery::BotDestroy) => {
                        self.bots.remove(&record.update.entity);
                    }
                    (
                        true,
                        Delivery::Board { .. }
                        | Delivery::BotLink { .. }
                        | Delivery::SpawnFlag { .. },
                    ) => {}
                    (true, Delivery::Health { .. }) => {
                        self.health_updates.remove(&record.update.entity);
                    }
                    (true, Delivery::CreateBody(id)) => {
                        self.created.insert(id.entity);
                        if let Some(script) =
                            self.body_script.as_mut().filter(|script| script.id == id)
                        {
                            script.stream.create_arrived();
                        }
                    }
                    (true, Delivery::Possession(id)) => self.bound = Some(id),
                    (true, Delivery::DestroyBody(id)) => {
                        self.bodies.insert(
                            id.entity,
                            (
                                id.generation,
                                Lifetime::ReusableAt(now.saturating_add(BODY_REUSE_DELAY)),
                            ),
                        );
                    }
                    (true, Delivery::ProjectileConfirm { token, .. }) => {
                        self.body_script
                            .as_mut()
                            .and_then(|script| script.projectiles.as_mut())
                            .expect("current projectile tracker")
                            .create_arrived(token, record.update.entity);
                    }
                    (true, Delivery::ProjectileDestroy { .. }) => {
                        self.projectile_destroys.remove(&record.update.entity);
                    }
                    (true, Delivery::Field(FieldDelivery::Create)) => {
                        // projectiles.py `FieldArrived`: the field's frame.
                        match self.fields.created(record.update.entity) {
                            Ok(Some((update, field))) => self.push(Record {
                                update,
                                delivery: Delivery::Field(field),
                            }),
                            Ok(None) => {}
                            Err(error) => eprintln!(
                                "Biotic Field {:08X}'s frame failed ({error})",
                                record.update.entity
                            ),
                        }
                    }
                    (true, Delivery::Field(FieldDelivery::Frame { last })) => {
                        self.fields.frame_arrived(record.update.entity, last);
                    }
                    (true, Delivery::Field(FieldDelivery::Destroy)) => {
                        self.fields.destroy_arrived(record.update.entity);
                    }
                    (true, Delivery::Script { last, .. }) if record.update.entity == self.owner => {
                        self.controller.arrived(last);
                    }
                    (true, Delivery::Script { last, .. }) => {
                        self.mode.arrived(last);
                    }
                }
            }
        }
        if !retry.is_empty() {
            retry.append(&mut self.queued);
            self.queued = retry;
            self.remove_stale();
        }
    }

    fn remove_stale(&mut self) {
        let mut queued = std::mem::take(&mut self.queued);
        queued.retain(|record| self.current(record));
        self.queued = queued;
    }

    pub(crate) fn ready(&self) -> Vec<Ready<'_>> {
        let body = self.body.map(|body| body.id);
        let mut created = body.is_none_or(|id| self.created.contains(&id.entity));
        let mut bound = body == self.bound;
        let mut destroyed = HashSet::new();
        let mut creating = HashSet::new();
        self.queued
            .iter()
            .enumerate()
            .filter_map(|(index, record)| {
                match record.delivery {
                    Delivery::BotDestroy => {
                        destroyed.insert(record.update.entity);
                    }
                    Delivery::BotCreate { after: Some(old) }
                        if self.bots.contains_key(&old) && !destroyed.contains(&old) =>
                    {
                        return None;
                    }
                    Delivery::BotCreate { .. } => {
                        creating.insert(record.update.entity);
                    }
                    Delivery::BotLink { body }
                        if !self.created.contains(&body) && !creating.contains(&body) =>
                    {
                        return None;
                    }
                    Delivery::Health { .. } if !self.identity_ready(record.update.entity) => {
                        return None;
                    }
                    Delivery::SpawnFlag { .. } if !self.created.contains(&self.owner) => {
                        return None;
                    }
                    Delivery::CreateBody(id) if Some(id) == body => created = true,
                    Delivery::Possession(_) if !created => return None,
                    Delivery::Possession(_) => bound = true,
                    Delivery::DestroyBody(_) | Delivery::Script { close: true, .. } if !bound => {
                        return None;
                    }
                    Delivery::BodyScript { id, last, delta }
                        if (delta && !bound)
                            || !self.body_script.as_ref().is_some_and(|script| {
                                script.id == id && script.stream.ready_to_send(last)
                            }) =>
                    {
                        return None;
                    }
                    Delivery::ProjectileConfirm { id, .. }
                        if self.bound != Some(id) || !self.created.contains(&id.entity) =>
                    {
                        // Even a fallback confirmation needs an acknowledged owner lease.
                        return None;
                    }
                    Delivery::BotFrames { last }
                        if !self.bot_views.ready(record.update.entity, last) =>
                    {
                        return None;
                    }
                    Delivery::Other(other) if !self.others.ready(other) => return None,
                    _ => {}
                }
                Some(Ready { index, record })
            })
            .collect()
    }

    /// Retain blocked records and remove only the exact records selected by the
    /// successful send, even when eligibility skipped an earlier queue entry.
    pub(crate) fn sent(&mut self, indexes: &[usize]) {
        if indexes.is_empty() {
            return;
        }
        for &index in indexes {
            match self.queued[index].delivery {
                Delivery::BodyScript { id, last, .. } => {
                    if let Some(script) = self.body_script.as_mut().filter(|script| script.id == id)
                    {
                        script.sent(last);
                    }
                }
                Delivery::Other(other) => self.others.sent(other),
                Delivery::BotFrames { last } => {
                    let entity = self.queued[index].update.entity;
                    self.bot_views.sent(entity, last);
                }
                _ => {}
            }
        }
        let mut selected = indexes.iter().copied().peekable();
        let mut index = 0;
        self.queued.retain(|_| {
            let keep = selected.next_if_eq(&index).is_none();
            index += 1;
            keep
        });
    }

    pub(crate) fn sent_indices(ready: &[Ready<'_>], sent: &[RecordSource]) -> Vec<usize> {
        sent.iter()
            .filter_map(|source| match *source {
                RecordSource::Queued(index) => Some(ready[index].index),
                RecordSource::Movement(_) => None,
            })
            .collect()
    }

    pub fn send_frame<E>(
        &mut self,
        tick: u32,
        mut movements: Vec<EntityUpdate>,
        reliable_only: bool,
        egress: &mut GameEgress<Arc<Record>>,
        send: impl FnOnce(Vec<u8>) -> Result<u32, E>,
    ) -> Result<SentFrame, SendError<E>> {
        if !reliable_only {
            self.prepare_body_updates(tick);
        }
        let script = self.body_script.as_ref();
        let history = script.and_then(|script| script.corrections.as_ref());
        let correction = if reliable_only {
            None
        } else {
            script.and_then(|script| {
                history?.candidate(tick, self.created.contains(&script.id.entity))
            })
        };
        let owner_index = correction.map(|correction| {
            let index = movements.len();
            movements.push(correction.update());
            index
        });
        let queued = self.ready();
        let newest = self.command_ack(tick);
        let sent = egress.send_frame(
            FrameRecords {
                tick,
                queued: &queued,
                movements: &movements,
                owner_correction: owner_index.is_some(),
                reliable_only,
            },
            |records| {
                let selected = owner_index
                    .filter(|&index| {
                        records
                            .iter()
                            .any(|record| std::ptr::eq(*record, &movements[index]))
                    })
                    .and(correction.as_ref());
                history.map_or(newest, |history| {
                    history.command_ack(tick, Some(newest), selected)
                })
            },
            |source| match source {
                RecordSource::Queued(index) => queued[index].tracked(),
                RecordSource::Movement(_) => unreachable!("movement records are not retried"),
            },
            send,
        )?;
        let indexes = Self::sent_indices(&queued, &sent.records);
        self.sent(&indexes);
        let selected = owner_index
            .zip(correction)
            .filter(|(index, _)| sent.records.contains(&RecordSource::Movement(*index)));
        let history = self
            .body_script
            .as_mut()
            .and_then(|script| script.corrections.as_mut());
        if let (Some((_, correction)), Some(history)) = (selected, history) {
            history.sent(&correction);
        }
        for source in &sent.records {
            let RecordSource::Movement(index) = *source else {
                continue;
            };
            let entity = movements[index].entity;
            if self.bots.get(&entity) == Some(&BotLifetime::Live) {
                self.bot_sights
                    .entry(entity)
                    .or_insert(BotSight::Sent(sent.sequence));
            }
        }
        Ok(sent)
    }

    fn prepare_body_updates(&mut self, tick: u32) {
        // A split input gap can update physics history immediately. The owner
        // VM and health publish only after the queued command completes.
        if matches!(
            self.input.work.front(),
            Some(InputWork::Command {
                pending: Some(_),
                ..
            })
        ) {
            return;
        }
        let health = match (&mut self.body_script, &mut self.plain_vitals) {
            (Some(script), _) => script.runtime.take_health_update().map(|health| {
                let dead = script.runtime.vitals().and_then(Vitals::death).is_some();
                (script.id.entity, health, dead)
            }),
            (None, Some(vitals)) if vitals.has_changed() => {
                vitals.clear_changed();
                self.body.map(|body| {
                    let dead = vitals.death().is_some();
                    (body.id.entity, vitals.cumulative_component(), dead)
                })
            }
            _ => None,
        };
        if let Some((entity, health, dead)) = health {
            let mut components = vec![health];
            // A death's pools come with the filter bits that say the body
            // is dead (`scene` module notes).
            if let Some(body) = self.body.filter(|body| dead && body.id.entity == entity) {
                components.push(filter_component(body_filter(body.team_bit, true)));
            }
            self.body_health += 1;
            self.queue_health(EntityUpdate {
                entity,
                operation: Some(Operation::Update(components)),
                ..EntityUpdate::default()
            });
        }
        let kill_feed = self
            .body_script
            .as_ref()
            .and_then(|script| self.death.filter(|death| death.body == script.id))
            .map(|death| self.kill_feed_allowed(death, tick));
        let Some(script) = &mut self.body_script else {
            return;
        };
        let created = self.created.contains(&script.id.entity);
        let (record, replaced) = match script.next_record(created, kill_feed) {
            Ok(Some(record)) => record,
            Ok(None) => return,
            Err(error) => {
                self.drop_failed_script(&error.to_string());
                return;
            }
        };
        if let Some(replaced) = replaced {
            let id = script.id;
            self.queued.retain(|queued| {
                !matches!(queued.delivery, Delivery::BodyScript { id: queued_id, last, .. }
                    if queued_id == id && last == replaced)
            });
        }
        self.push(record);
    }

    /// A body whose script failed goes on without it until a pick of its
    /// hero replaces it. Say so once, with the first gap the VM reported, so
    /// a playtest shows why.
    fn drop_failed_script(&mut self, error: &str) {
        if let Some(script) = &self.body_script {
            let first = script
                .runtime
                .snapshot()
                .and_then(|snapshot| snapshot.diagnostics.first())
                .map(|item| {
                    format!(
                        "graph {:04X} node {:?}: {:?}",
                        item.site.graph, item.site.node, item.issue
                    )
                });
            eprintln!(
                "body {:08X} lost its script ({error}); first gap: {}",
                script.id.entity,
                first.as_deref().unwrap_or("none")
            );
        }
        self.cancel_body_script();
        self.script_failed = true;
    }

    /// The body's script stops (match.py `stop_body_script`), not failed.
    fn cancel_body_script(&mut self) {
        self.script_failed = false;
        if let Some(mut script) = self.body_script.take() {
            script.stream.retire();
            if let Some(mover) = &mut self.mover {
                let frame = mover.pose().frame.unwrap_or(script.frame);
                mover.track_modifiers(frame, &BTreeMap::new());
            }
            if let Some(projectiles) = &mut script.projectiles {
                self.queue_projectiles(script.id, projectiles.retire())
                    .expect("projectile retirement has no release payload");
                self.projectile_events.extend(
                    projectiles
                        .take_events()
                        .into_iter()
                        .map(|event| (script.id, event)),
                );
            }
            self.remove_stale();
        }
    }

    pub fn start(
        &mut self,
        scene: &PreparedScene,
        viewer: &PlayerAppearance<'_>,
        tick: u32,
    ) -> io::Result<()> {
        let records = scene.initial_world(viewer)?;
        self.bots.extend(
            scene
                .bots()
                .iter()
                .map(|bot| (bot.entity, BotLifetime::Live)),
        );
        for update in records {
            if self.bots.contains_key(&update.entity) {
                self.push(Record {
                    update,
                    delivery: Delivery::BotCreate { after: None },
                });
            } else {
                self.push(Record::baseline(update));
            }
        }
        self.started = Some(tick);
        // The player's create sends the spawn room flag set.
        self.spawn_flag = (true, self.spawn_flag.1 + 1);
        Ok(())
    }

    pub fn present(&mut self, scene: &PreparedScene, tick: u32) -> io::Result<()> {
        if self.controller.latest_data() == 0 && self.created.contains(&self.owner) {
            self.queue_controller(scene, tick, true, None, None)?;
        }
        // While heroes assemble the countdown frames are the mode entity's
        // first (combat.py `spawned`).
        if self.mode.latest_data() == 0
            && !self.assembling
            && self.created.contains(&GAME_MODE_ENTITY)
        {
            let chunk = self.mode.full(scene.mode_ui()).map_err(invalid)?;
            self.push(Record::script(chunk, false));
        }
        self.presentation_due = self.controller.latest_data() == 0 || self.mode.latest_data() == 0;
        Ok(())
    }

    /// The hero of the player's body, `None` before his first pick.
    pub fn hero(&self) -> Option<u64> {
        self.body.map(|body| body.hero)
    }

    pub fn set_assembling(&mut self, assembling: bool) {
        self.assembling = assembling;
    }

    /// What the match flow adds to the respawn of a death from now on.
    pub fn set_respawn_extra(&mut self, extra: Duration) {
        self.respawn_extra = extra;
    }

    /// A full frame of the game mode entity, such as the countdown; false
    /// when the client has no mode entity yet (server.py `queue_mode_frame`).
    pub fn queue_mode_frame(&mut self, frame: &orpheus_core::bits::BitWriter) -> io::Result<bool> {
        if !self.created.contains(&GAME_MODE_ENTITY) {
            return Ok(false);
        }
        let chunk = self.mode.full(frame).map_err(invalid)?;
        self.push(Record::script(chunk, false));
        self.remove_stale();
        Ok(true)
    }

    /// The countdown is over: a player with a hero gets hero select closed,
    /// and the skin selector goes (match.py `_end_assemble`).
    pub fn end_assemble(&mut self, scene: &PreparedScene, tick: u32) -> io::Result<()> {
        self.assembling = false;
        if !self.can_handle_game_message() {
            return Ok(());
        }
        let hero = self.body.map(|body| body.hero);
        let open = self.select_open && hero.is_none();
        self.queue_controller(scene, tick, open, hero, None)
    }

    fn can_handle_game_message(&self) -> bool {
        self.started.is_some()
            && self.controller.latest_data() != 0
            && self.created.contains(&self.owner)
    }

    fn game_message(
        &mut self,
        scene: &PreparedScene,
        viewer: &PlayerAppearance<'_>,
        tick: u32,
        now: Duration,
        quanta: u32,
        message: &Message,
    ) -> io::Result<()> {
        if !self.can_handle_game_message() {
            return Ok(());
        }
        if message.id == crate::wheel::COMMUNICATION_ACTION {
            self.communication_action(message);
            return Ok(());
        }
        let Some(message) = ClientGameMessage::read(message) else {
            return Ok(());
        };
        if message.entity != self.owner {
            return Ok(());
        }
        match message.hero_select(scene.heroes()) {
            Some(HeroSelectRequest::Pick(Some(hero))) => {
                if viewer
                    .card
                    .role
                    .is_some_and(|role| role != 0 && hero.role_number() != u8::try_from(role).ok())
                {
                    return Ok(());
                }
                let skin = viewer
                    .card
                    .skins
                    .get(&hero.guid)
                    .copied()
                    .unwrap_or_default();
                let open = self.assembling && self.select_open;
                // A dead player's pick waits for his respawn: the new hero
                // comes back at the spawn after the delay (assumption: as in
                // retail, where a hero change while dead keeps the timer).
                if self.death.is_some() {
                    self.respawn_hero = Some((hero.guid, skin));
                    self.queue_controller(scene, tick, open, Some(hero.guid), None)?;
                    return Ok(());
                }
                // Only in a spawn room (game-rules.md 23); the client asks
                // only there, so a pick from outside crossed the room's
                // edge on its way: the screen closes on the current hero.
                if !self.may_change_hero() {
                    self.queue_controller(
                        scene,
                        tick,
                        false,
                        self.body.map(|body| body.hero),
                        None,
                    )?;
                    return Ok(());
                }
                // match.py `game_message`: the same hero and skin keep the
                // body, unless its script failed (`needs_body`).
                if !self.script_failed
                    && self
                        .body
                        .is_some_and(|body| (body.hero, body.skin) == (hero.guid, skin))
                {
                    // The screen stays open until the heroes are assembled,
                    // as in retail (match.py `game_message`).
                    if !self.assembling {
                        self.queue_controller(scene, tick, false, Some(hero.guid), None)?;
                    }
                } else if let Some(entity) = self.next_body(now) {
                    self.switch(scene, viewer, tick, quanta, entity, (hero, skin), true)?;
                }
            }
            Some(HeroSelectRequest::OpenOrClose(wanted)) => {
                // Hero select opens only where a pick is taken.
                let open = self.body.is_none()
                    || (self.may_change_hero() && wanted.unwrap_or(!self.select_open));
                self.queue_controller(scene, tick, open, self.body.map(|body| body.hero), None)?;
            }
            _ => {}
        }
        Ok(())
    }

    /// A communication wheel choice (21608) runs on the current body as
    /// game message `02A3` ([`crate::wheel`]; match.py
    /// `communication_action` in leafall903/ORPHEUS); a failure stops the
    /// body's script, as any other body message does.
    fn communication_action(&mut self, message: &Message) {
        let Some((selection, parameters)) = crate::wheel::selection(message) else {
            return;
        };
        if !self.world_started() {
            return;
        }
        let Some(script) = &mut self.body_script else {
            return;
        };
        if let Err(error) = script.runtime.receive_message(selection, parameters) {
            self.drop_failed_script(&error.to_string());
        }
    }

    /// The ch3 ack: the last of the client's own command frames run. A
    /// dry buffer's repeats run past them (`InputQueue::release`); the ack
    /// stays at his newest, so the client, which keeps its commands from
    /// ack − 1 on (`orpheus_physics::mover::correction` notes), still sends
    /// the ones the repeats stood in for (`InputQueue::reconcile`). It is
    /// never past the frame an owner frame names.
    pub fn command_ack(&self, tick: u32) -> u32 {
        let processed = self.input.processed.unwrap_or_else(|| {
            self.input
                .first_admitted
                .map_or(tick, |frame| tick.min(frame.saturating_sub(1)))
        });
        match self.input.newest_real() {
            Some(newest)
                if frame_distance(processed, newest) != 0
                    && frame_distance(processed, newest) < FRAME_HALF =>
            {
                newest
            }
            _ => processed,
        }
    }

    pub fn queue_game_message(&mut self, tick: u32, received: Duration, message: Message) {
        if self.can_handle_game_message() {
            self.input.push(InputWork::Message {
                message,
                tick,
                received,
            });
        }
    }

    pub fn queue_skin(&mut self, hero: u64, tick: u32, received: Duration) {
        if self.can_handle_game_message() {
            self.input.push(InputWork::Skin {
                hero,
                tick,
                received,
            });
        }
    }

    fn apply_skin(
        &mut self,
        scene: &PreparedScene,
        viewer: &PlayerAppearance<'_>,
        hero: u64,
        tick: u32,
        now: Duration,
        quanta: u32,
    ) -> io::Result<()> {
        let Some(body) = self.body.filter(|body| body.hero == hero) else {
            return Ok(());
        };
        let Some(&skin) = viewer
            .card
            .skins
            .get(&hero)
            .filter(|&&skin| skin != body.skin)
        else {
            return Ok(());
        };
        if let Some(entity) = self.next_body(now) {
            let hero = scene.heroes().get(hero).expect("active hero");
            self.switch(scene, viewer, tick, quanta, entity, (hero, skin), true)?;
        }
        Ok(())
    }

    pub fn queue_commands(&mut self, tick: u32, commands: &[Command]) {
        if self.started.is_some() {
            self.input.commands(tick, commands);
        }
        // The datagram that said a bot's first record arrived ends here.
        let newest = self.input.newest_real();
        for sight in self.bot_sights.values_mut() {
            if *sight == BotSight::Arrived {
                *sight = BotSight::Shown(newest);
            }
        }
    }

    /// Tests of what a command does, not of when: input runs as it comes,
    /// as an AI player's does.
    #[cfg(test)]
    pub(crate) fn unbuffered(mut self) -> Self {
        self.input.immediate = true;
        self
    }

    /// The input of `ticks` server ticks goes to work ([`InputQueue::release`]):
    /// one command a tick, a repeat of the last input when none came.
    pub fn release_input(&mut self, ticks: u32) {
        let body = self.mover.is_some();
        self.input.release(ticks, body);
    }

    pub fn has_input_work(&self) -> bool {
        !self.input.work.is_empty()
    }

    pub fn combat_body_active(&self) -> bool {
        self.body_script.is_some()
    }

    /// The hero of the player's current body.
    pub fn current_hero(&self) -> Option<u64> {
        self.body.map(|body| body.hero)
    }

    /// The hits and melees of the input processed since the last call.
    pub fn take_shots(&mut self) -> Vec<PosedShot> {
        std::mem::take(&mut self.shots)
    }

    /// The player's current body.
    pub fn current_body(&self) -> Option<u32> {
        self.body.map(|body| body.id.entity)
    }

    /// The bodies the target searches can find from now on
    /// (`Combat::target_candidates`): the current body's and the next.
    pub fn set_target_candidates(&mut self, candidates: &[TargetCandidate]) {
        self.target_candidates.clear();
        self.target_candidates.extend_from_slice(candidates);
        if let Some(script) = &mut self.body_script {
            script.runtime.set_target_candidates(candidates);
        }
    }

    /// Tests: whether state `state` of the current body's instance of graph
    /// `graph` is active.
    #[cfg(any(test, feature = "fixtures"))]
    pub fn body_state_active(&self, graph: u32, state: u16) -> bool {
        self.body_script
            .as_ref()
            .and_then(|script| script.runtime.snapshot())
            .is_some_and(|snapshot| {
                snapshot.instances.values().any(|instance| {
                    instance.graph == graph
                        && instance
                            .states
                            .get(&state)
                            .is_some_and(|state| state.active)
                })
            })
    }

    /// Tests: whether the client acknowledged an owner frame of the current
    /// body in which state `state` of its instance of graph `graph` runs
    /// (the owner stream's base), and the entity variable `variable` as
    /// that frame left it.
    #[cfg(any(test, feature = "fixtures"))]
    pub fn body_delivered(&self, graph: u32, state: u16, variable: u32) -> (bool, ScriptValue) {
        let Some((script, (_, base))) = self
            .body_script
            .as_ref()
            .and_then(|script| Some((script, script.stream.base()?)))
        else {
            return (false, ScriptValue::Null);
        };
        let running = script.runtime.snapshot().is_some_and(|snapshot| {
            snapshot
                .instances
                .iter()
                .any(|(&id, instance)| instance.graph == graph && base.state(id, state).is_some())
        });
        let value = base
            .entity_variable(variable)
            .cloned()
            .unwrap_or(ScriptValue::Null);
        (running, value)
    }

    /// The client's hero settings (21604, the whole list): kept for every
    /// body the player plays from now on, and given to the current one at
    /// once, whose graphs react to them (`01CF` st115/st116 watch ALLIED
    /// HEALTH BARS); a setting the list leaves out goes back to DEFAULT.
    pub fn update_hero_settings(&mut self, entries: &[HeroSetting]) {
        self.hero_settings.replace(entries);
        let Some(body) = self.body else {
            return;
        };
        let variables = hero_setting_variables(&self.hero_settings, body.hero);
        let failure = self
            .body_script
            .as_mut()
            .filter(|script| script.id == body.id)
            .and_then(|script| script.runtime.set_hero_settings(&variables).err());
        if let Some(error) = failure {
            self.drop_failed_script(&error.to_string());
        }
    }

    /// Whether the player's current body is dead (combat.py `is_dead`).
    pub fn body_dead(&self) -> bool {
        self.death.is_some()
    }

    /// Where the player's body stands while it is alive in the world (the
    /// match flow's objectives count it).
    pub fn body_position(&self) -> Option<[f64; 3]> {
        self.started?;
        self.body?;
        if self.death.is_some() {
            return None;
        }
        Some(self.mover.as_ref()?.pose().position)
    }

    /// The centre of the player's body's capsule while it is alive in the
    /// world (its mover data and stance), and how far it is above the feet:
    /// what the environment's death rules test (`map_areas` module notes).
    pub fn body_centre(&self) -> Option<([f64; 3], f64)> {
        let feet = self.body_position()?;
        let capsule = self.body_capsule()?;
        let [x, y, z] = capsule.position.map(f64::from);
        let centre = y + f64::from(capsule.top + capsule.radius) / 2.0;
        Some(([x, centre, z], centre - feet[1]))
    }

    /// The capsule of the player's body while it is alive in the world, as
    /// it blocks other bodies' moves ([`blocks`]).
    pub fn body_capsule(&self) -> Option<CapsulePose> {
        self.body_position()?;
        Some(self.mover.as_ref()?.capsule())
    }

    /// The capsules of the living bodies that block this player's body
    /// ([`blocks`]) where they are now, for the commands it runs next.
    pub fn set_blockers(&mut self, blockers: Vec<CapsulePose>) {
        self.blockers = blockers;
    }

    /// The player's body as a target this tick (combat.py `_targets`: a
    /// player in the world with a body), hurt when its pools are below
    /// their maximum.
    pub fn combat_target(&self, scene: &PreparedScene, team: u8) -> Option<PlayerTarget> {
        self.started?;
        let body = self.body?;
        let mover = self.mover.as_ref()?;
        let hero = scene.heroes().get(body.hero)?;
        let vitals = match self
            .body_script
            .as_ref()
            .filter(|script| script.id == body.id)
        {
            Some(script) => script.runtime.vitals(),
            None => self.plain_vitals.as_ref(),
        };
        Some(PlayerTarget {
            body: body.id.entity,
            team,
            pose: scene.player_target(hero, mover.pose().position, mover.crouched()),
            dead: self.death.is_some(),
            hurt: vitals.is_some_and(Vitals::hurt),
        })
    }

    /// Whether this client can name `entity` in a combat notice or a kill
    /// feed line (server.py `combat_identity_ready`): his own current body,
    /// another player's body once its possession arrived, a bot once its
    /// create arrived.
    pub fn combat_identity_ready(&self, entity: u32) -> bool {
        if !self.world_started() {
            return false;
        }
        if entity & !0xff == self.owner {
            return self.current_body() == Some(entity);
        }
        self.others
            .possession_confirmed(entity)
            .unwrap_or_else(|| self.created.contains(&entity))
    }

    fn readiness(&self) -> ViewerReadiness<'_> {
        ViewerReadiness {
            mode_ready: self.mode_ready(),
            own_body: self.current_body(),
            created: Some(&self.created),
        }
    }

    /// combat.py `kill_feed_allowed` for the own body: the mode UI arrived,
    /// the death is fresh and the client can name the body and its killer.
    fn kill_feed_allowed(&self, death: BodyDeath, tick: u32) -> bool {
        self.mode_ready()
            && kill_feed_fresh(tick, death.tick)
            && self.combat_identity_ready(death.body.entity)
            && self.combat_identity_ready(death.killer)
    }

    /// combat.py `_damage` and `_died` for the player's current body: its
    /// pools take the hit (shields, armour, health) and its script's health
    /// follows; a killing hit makes it dead, gives its script the "died"
    /// message (003E.025, which pulses the kill feed line) and sets its
    /// respawn after the mode's time.
    pub fn damage_body(
        &mut self,
        scene: &PreparedScene,
        hit: &PlayerHit,
        tick: u32,
        now: Duration,
    ) -> Option<BodyDamage> {
        let body = self.body.filter(|body| body.id.entity == hit.body)?;
        if self.death.is_some() || hit.amount.is_nan() || hit.amount <= 0.0 {
            return None;
        }
        // Invulnerable in his team's spawn room (`map_areas`: 0C8E `03D4`),
        // or while his graphs hold a status (`STU_9028E035`).
        if self.spawn_room == Some(true) || self.body_effects().is_some_and(BodyEffects::invulnerable)
        {
            return None;
        }
        let death_tick = hit.world_tick.unwrap_or(tick);
        let source = Death {
            time: now.as_secs_f64(),
            frame: death_tick,
            killer: hit.shooter.source,
        };
        let mut failure = None;
        let result = match self
            .body_script
            .as_mut()
            .filter(|script| script.id == body.id)
        {
            Some(script) => {
                let result = script.runtime.vitals_mut()?.damage(hit.amount, source);
                if result.dealt > 0.0 {
                    failure = script.runtime.health_changed().err();
                }
                result
            }
            None => self
                .plain_vitals
                .get_or_insert_with(|| hero_vitals(scene, body.hero))
                .damage(hit.amount, source),
        };
        if result.dealt <= 0.0 {
            return None;
        }
        self.body_health += 1;
        let script = self
            .body_script
            .as_mut()
            .filter(|script| script.id == body.id && failure.is_none());
        if result.killed {
            self.death = Some(BodyDeath {
                body: body.id,
                killer: hit.shooter.source,
                tick: death_tick,
                respawn_at: now
                    .saturating_add(scene.respawn_time())
                    .saturating_add(self.respawn_extra),
            });
            if let Some(script) = script {
                let (message, parameters) =
                    death_message(hit.shooter.source, hit.critical, hit.loadout, hit.tags);
                failure = script.runtime.receive_message(message, parameters).err();
            }
        } else if let Some(script) = script.filter(|_| scene.hurt_feedback().message) {
            // The live hurt feedback switch (`crate::hurt_feedback`): with
            // the flinch off the message goes without its tags.
            let tags = if scene.hurt_feedback().flinch {
                hit.tags
            } else {
                DamageTags::NONE
            };
            let (message, parameters) =
                damage_message(hit.shooter.source, result.dealt, tags, hit.direction);
            failure = script.runtime.receive_message(message, parameters).err();
        }
        if let Some(error) = failure {
            self.drop_failed_script(&error.to_string());
        }
        Some(BodyDamage {
            dealt: result.dealt,
            killed: result.killed,
        })
    }

    /// combat.py `heal_area` for the player's living body, scaled by the
    /// healing modifiers his graphs hold for a heal with `tags` (Biotic
    /// Grenade's anti-heal and heal boost, `vm/statuses.rs`). Returns what
    /// the pools took.
    pub fn heal_body(
        &mut self,
        scene: &PreparedScene,
        body: u32,
        amount: f64,
        tags: &DamageTags,
    ) -> f64 {
        let Some(current) = self.body.filter(|current| current.id.entity == body) else {
            return 0.0;
        };
        if self.death.is_some() {
            return 0.0;
        }
        let amount = amount * self.body_effects().map_or(1.0, |effects| effects.healing_scale(tags));
        let mut failure = None;
        let given = match self
            .body_script
            .as_mut()
            .filter(|script| script.id == current.id)
        {
            Some(script) => {
                let Some(vitals) = script.runtime.vitals_mut() else {
                    return 0.0;
                };
                let given = vitals.heal(amount);
                if given > 0.0 {
                    failure = script.runtime.health_changed().err();
                }
                given
            }
            None => self
                .plain_vitals
                .get_or_insert_with(|| hero_vitals(scene, current.hero))
                .heal(amount),
        };
        if given > 0.0 {
            self.body_health += 1;
        }
        if let Some(error) = failure {
            self.drop_failed_script(&error.to_string());
        }
        given
    }

    /// Whether the player may change heroes now: while dead (assumption:
    /// the dead change heroes as in retail, the new hero taking the
    /// respawn), before his first body, or while his body is not known to
    /// stand outside his team's spawn rooms (game-rules.md 23).
    pub fn may_change_hero(&self) -> bool {
        self.death.is_some() || self.body.is_none() || self.spawn_room != Some(false)
    }

    /// Whether his living body stood in one of his team's spawn rooms when
    /// last measured; `None` when dead, gone or the team has no room.
    pub fn in_spawn_room(&self) -> Option<bool> {
        self.spawn_room
    }

    /// The spawn rooms each tick (`map_areas` module notes, mode body graph
    /// `0C8E`): where the living body stands; inside, it heals
    /// [`SPAWN_HEALING`] a second (one tick's share, [`TICK_PERIOD`]) and
    /// takes no damage ([`Self::damage_body`]); the client's spawn room
    /// flag (component 74 field 6) follows [`Self::may_change_hero`].
    pub fn area_step(&mut self, scene: &PreparedScene, viewer: &PlayerAppearance<'_>) {
        if self.started.is_none() {
            return;
        }
        self.spawn_room = match (self.body, &self.mover, self.death) {
            (Some(_), Some(mover), None) => {
                scene.in_spawn_room(viewer.team, viewer.room, mover.pose().position)
            }
            _ => None,
        };
        if let (Some(true), Some(body)) = (self.spawn_room, self.body) {
            let amount = SPAWN_HEALING * TICK_PERIOD.as_secs_f64();
            self.heal_body(scene, body.id.entity, amount, &DamageTags::NONE);
        }
        let inside = self.may_change_hero();
        if inside != self.spawn_flag.0 {
            let revision = self.spawn_flag.1 + 1;
            self.spawn_flag = (inside, revision);
            self.push(Record {
                update: crate::scene::spawn_room_flag(self.owner, inside),
                delivery: Delivery::SpawnFlag { revision },
            });
            self.remove_stale();
        }
    }

    /// The statuses and combat modifiers the current body's states hold.
    fn body_effects(&self) -> Option<&BodyEffects> {
        let body = self.body?;
        self.body_script
            .as_ref()
            .filter(|script| script.id == body.id)
            .and_then(|script| script.runtime.effects())
    }

    /// An aura comes on the player's living body, dealt by `instigator`
    /// (Biotic Grenade's explosion).
    pub fn apply_aura(&mut self, aura: u64, instigator: u32) {
        if self.death.is_some() {
            return;
        }
        let Some(body) = self.body else {
            return;
        };
        let failure = self
            .body_script
            .as_mut()
            .filter(|script| script.id == body.id)
            .and_then(|script| script.runtime.apply_aura(aura, instigator).err());
        if let Some(error) = failure {
            self.drop_failed_script(&error.to_string());
        }
    }

    /// The player's living body dealt `points` of damage or healing: its
    /// ultimate charges for it.
    pub fn gain_ult(&mut self, points: f64) {
        if self.death.is_some() {
            return;
        }
        let Some(body) = self.body else {
            return;
        };
        let failure = self
            .body_script
            .as_mut()
            .filter(|script| script.id == body.id)
            .and_then(|script| script.runtime.gain_ult(points).err());
        if let Some(error) = failure {
            self.drop_failed_script(&error.to_string());
        }
    }

    /// combat.py `_respawn`: once the mode's time is up, a dead player gets
    /// a new body of the same hero at his spawn point, as a hero switch does
    /// (match.py `switch_hero`), and his spawn voice line. While every id of
    /// his body ring is still reserved, he tries again RESPAWN_RETRY later.
    /// True when he respawned.
    pub fn respawn_due(
        &mut self,
        scene: &PreparedScene,
        viewer: &PlayerAppearance<'_>,
        tick: u32,
        now: Duration,
        quanta: u32,
    ) -> io::Result<bool> {
        let Some(death) = self.death else {
            return Ok(false);
        };
        if now < death.respawn_at {
            return Ok(false);
        }
        let Some(body) = self.body.filter(|body| body.id == death.body) else {
            self.death = None;
            return Ok(false);
        };
        let Some(entity) = self.next_body(now) else {
            if let Some(death) = &mut self.death {
                death.respawn_at = now.saturating_add(RESPAWN_RETRY);
            }
            return Ok(false);
        };
        let (guid, skin) = self.respawn_hero.unwrap_or_else(|| {
            let skin = viewer
                .card
                .skins
                .get(&body.hero)
                .copied()
                .unwrap_or_default();
            (body.hero, skin)
        });
        let hero = scene
            .heroes()
            .get(guid)
            .ok_or_else(|| invalid("the dead body's hero is unknown"))?;
        self.switch(scene, viewer, tick, quanta, entity, (hero, skin), false)?;
        self.begin_spawn_bark(now);
        Ok(true)
    }

    /// A new round puts the player back in his team's spawn room: a new
    /// body of his hero there, dead or alive. With `select` hero select
    /// opens too, as before the second side of an Assault, Escort or Hybrid
    /// match (game-rules.md 2), and stays open while the heroes assemble. A
    /// player without a hero keeps waiting in hero select. True when he got
    /// a new body.
    pub fn spawn_again(
        &mut self,
        scene: &PreparedScene,
        viewer: &PlayerAppearance<'_>,
        tick: u32,
        now: Duration,
        quanta: u32,
        select: bool,
    ) -> io::Result<bool> {
        if select {
            self.assembling = true;
        }
        let Some(body) = self.body else {
            return Ok(false);
        };
        let Some(entity) = self.next_body(now) else {
            return Ok(false);
        };
        let (guid, skin) = self.respawn_hero.unwrap_or((body.hero, body.skin));
        let hero = scene
            .heroes()
            .get(guid)
            .ok_or_else(|| invalid("the body's hero is unknown"))?;
        self.switch(scene, viewer, tick, quanta, entity, (hero, skin), false)?;
        self.begin_spawn_bark(now);
        if select && self.can_handle_game_message() {
            self.queue_controller(scene, tick, true, Some(hero.guid), None)?;
        }
        Ok(true)
    }

    /// voicelines.py `VoiceLines.spawned` on the current body.
    fn begin_spawn_bark(&mut self, now: Duration) {
        if let Some(script) = &mut self.body_script {
            if let Err(error) = script.runtime.begin_spawn_bark(now) {
                eprintln!(
                    "{:08X}'s spawn voice line failed ({error})",
                    script.id.entity
                );
            }
        }
    }

    /// voicelines.py `VoiceLines.expire` on the current body.
    pub fn expire_bark(&mut self, now: Duration) {
        if let Some(script) = &mut self.body_script {
            if let Err(error) = script.runtime.expire_bark(now) {
                eprintln!(
                    "ending {:08X}'s spawn voice line failed ({error})",
                    script.id.entity
                );
            }
        }
    }

    /// The game messages the current body sent other entities (match.py
    /// `queue_body_message`), while other clients get the body (`_shown`).
    pub fn take_body_messages(&mut self) -> Vec<GameMessage> {
        if !self.world_started() {
            return Vec::new();
        }
        self.body_script
            .as_mut()
            .map_or_else(Vec::new, |script| script.runtime.take_messages())
    }

    /// The voice lines the current body says for the criteria its graphs
    /// raised since the last call (the communication wheel's lines and the
    /// pain lines after a hit), by its hero's voice set
    /// ([`crate::voice_lines`]).
    pub fn take_voice_lines(&mut self, scene: &PreparedScene, tick: u32) -> Vec<VoiceLine> {
        if !self.world_started() {
            return Vec::new();
        }
        let Some(script) = self.body_script.as_mut() else {
            return Vec::new();
        };
        let criteria = script.runtime.take_criteria();
        let id = script.id;
        let Some(body) = self.body.filter(|body| body.id == id) else {
            return Vec::new();
        };
        if criteria.is_empty() || self.death.is_some() {
            return Vec::new();
        }
        let speaker = body.id.entity;
        let set = scene
            .heroes()
            .get(body.hero)
            .and_then(|hero| scene.voice_sets().for_body(hero.body));
        if self
            .voice
            .as_ref()
            .is_none_or(|(entity, _)| *entity != speaker)
        {
            self.voice = Some((speaker, VoiceState::default()));
        }
        let (_, voice) = self.voice.as_mut().expect("the body's voice");
        let hurt = scene.hurt_feedback();
        criteria
            .into_iter()
            .filter(|criteria| hurt.speaks(criteria.identifier))
            .filter_map(|criteria| voice.answer(set, speaker, criteria, tick))
            .collect()
    }

    /// Where the player's body stands, alive or dead, while it is in the
    /// world: where he hears voice lines from.
    pub fn hearing_position(&self) -> Option<[f64; 3]> {
        self.started?;
        self.body?;
        Some(self.mover.as_ref()?.pose().position)
    }

    /// Whether this client's current body runs a script that takes messages
    /// sent to `target` (match.py `_message_player`).
    pub fn takes_body_message(&self, target: u32) -> bool {
        self.world_started() && self.current_body() == Some(target) && self.body_script.is_some()
    }

    /// A message to the current body (match.py `deliver_body_messages`):
    /// its entries run now; a failure stops the body's script. False when
    /// the message is not for this body.
    pub fn receive_body_message(&mut self, message: &GameMessage) -> bool {
        if !self.takes_body_message(message.target) {
            return false;
        }
        let Some(script) = &mut self.body_script else {
            return false;
        };
        if let Err(error) = script
            .runtime
            .receive_message(message.message, message.parameters.clone())
        {
            self.drop_failed_script(&error.to_string());
        }
        true
    }

    /// match.py `_send_board`: the player's board numbers again when they
    /// changed, at most every STATS_EVERY, once his controller went out.
    pub fn send_board(&mut self, now: Duration, board: impl FnOnce() -> WorldComponent) {
        if !self.can_handle_game_message() || now < self.board.next {
            return;
        }
        let values = board();
        if self.board.sent.as_ref() == Some(&values) {
            return;
        }
        self.board.next = now.saturating_add(STATS_EVERY);
        self.board.revision += 1;
        self.push(Record {
            update: EntityUpdate {
                entity: self.owner,
                operation: Some(Operation::Update(vec![values.clone()])),
                ..EntityUpdate::default()
            },
            delivery: Delivery::Board {
                revision: self.board.revision,
            },
        });
        self.board.sent = Some(values);
        self.remove_stale();
    }

    /// projectiles.py `Projectiles.update` for the player's shots: their
    /// flights, confirmations and releases at `tick`, whose retained combat
    /// frames start at `first_tick`. What they ask of the room waits for
    /// [`crate::combat::deliver_projectiles`]. An explosion's push on his
    /// own body ([`FlightEnd::knockback`]) goes to his mover for its next
    /// posed frame, and the owner corrections carry it to his client.
    pub fn advance_projectiles(
        &mut self,
        tick: u32,
        first_tick: u32,
        mut step: impl FnMut(u32, &mut ProjectileFlight) -> io::Result<Option<FlightEnd>>,
    ) -> io::Result<()> {
        let Some(script) = &mut self.body_script else {
            return Ok(());
        };
        let mover = &mut self.mover;
        let id = script.id;
        let mut deliveries = Vec::new();
        let projectiles = &mut script.projectiles;
        script.flights.retain_mut(|(token, flight)| {
            let result = step(id.entity, flight);
            if let (
                Ok(Some(FlightEnd {
                    knockback: Some(knockback),
                    ..
                })),
                Some(mover),
            ) = (&result, mover.as_mut())
            {
                mover.push(*knockback);
            }
            let (Some(token), Some(projectiles)) = (*token, projectiles.as_mut()) else {
                return matches!(result, Ok(None));
            };
            match result {
                Ok(None) => true,
                Ok(Some(end)) => {
                    projectiles.flight_ended(token, tick, end.exploded.then_some(end.point));
                    false
                }
                Err(_) => {
                    projectiles.cancel(token, &mut deliveries);
                    false
                }
            }
        });
        let Some(projectiles) = &mut script.projectiles else {
            return Ok(());
        };
        deliveries.extend(projectiles.advance(tick, first_tick));
        self.projectile_events.extend(
            projectiles
                .take_events()
                .into_iter()
                .map(|event| (id, event)),
        );
        self.queue_projectiles(id, deliveries)
    }

    /// The projectiles the player fired since the last call, for the other
    /// clients (projectiles.py `fired`: `_show_others`).
    pub fn take_fired(&mut self) -> Vec<FiredProjectile> {
        self.body_script
            .as_mut()
            .map_or_else(Vec::new, |script| std::mem::take(&mut script.fired))
    }

    /// projectiles.py `_show_others` for this client: another player's
    /// projectile, when this client has his body's statescript.
    pub(crate) fn show_projectile(
        &mut self,
        fired: &FiredProjectile,
        tick: u32,
        next_id: impl FnOnce() -> u32,
    ) {
        if !self.world_started() {
            return;
        }
        for (update, delivery) in self.others.show_projectile(fired, tick, next_id) {
            self.push(Record {
                update,
                delivery: Delivery::Other(delivery),
            });
        }
    }

    pub(crate) fn take_projectile_events(&mut self) -> Vec<(BodyId, ProjectileEvent)> {
        std::mem::take(&mut self.projectile_events)
    }

    /// The skin theme of the player's body (match.py `Player.skin`).
    pub(crate) fn skin(&self) -> SkinTheme {
        self.body.map(|body| body.skin).unwrap_or_default()
    }

    pub(crate) fn owner(&self) -> u32 {
        self.owner
    }

    /// The player's Biotic Fields that still run.
    pub(crate) fn live_fields(&self) -> Vec<ShotKey> {
        self.body_script
            .as_ref()
            .and_then(|script| Some((script.id, script.projectiles.as_ref()?)))
            .map_or_else(Vec::new, |(id, projectiles)| {
                projectiles.fields().map(|token| (id, token)).collect()
            })
    }

    /// `_ObservedProjectile.finish` for this client's copy of another
    /// player's shot.
    pub(crate) fn finish_projectile(
        &mut self,
        body: BodyId,
        token: ProjectileToken,
        tick: u32,
        explode: bool,
    ) {
        self.others.finish_projectile(body, token, tick, explode);
    }

    /// `_ObservedProjectile.advance` for this client's copies.
    pub(crate) fn advance_observed(&mut self, tick: u32) {
        let records = self.others.advance_projectiles(tick);
        for (update, delivery) in records {
            self.push(Record {
                update,
                delivery: Delivery::Other(delivery),
            });
        }
        for (update, delivery) in self.bot_views.advance_rockets(tick) {
            self.push(Record {
                update,
                delivery: Delivery::BotSeen(delivery),
            });
        }
        self.remove_stale();
    }

    /// projectiles.py `_show_others` for a firing bot's rocket.
    pub(crate) fn show_bot_rocket(
        &mut self,
        rocket: &SeenRocket,
        tick: u32,
        next_id: impl FnOnce() -> u32,
    ) {
        if !self.world_started() {
            return;
        }
        for (update, delivery) in self.bot_views.show_rocket(rocket, tick, next_id) {
            self.push(Record {
                update,
                delivery: Delivery::BotSeen(delivery),
            });
        }
    }

    /// projectiles.py `flown` for a bot's rocket that landed.
    pub(crate) fn finish_bot_rocket(&mut self, rocket: u64, tick: u32) {
        self.bot_views.finish_rocket(rocket, tick);
    }

    /// `_deliver_field` for this client, once it is in the world.
    pub(crate) fn deliver_field(
        &mut self,
        field: &FieldDeliver<'_>,
        ids: &mut crate::projectiles::FieldIds,
    ) {
        if !self.world_started() {
            return;
        }
        for (update, delivery) in self.fields.deliver(field, ids) {
            self.push(Record {
                update,
                delivery: Delivery::Field(delivery),
            });
        }
    }

    /// The field faded: its frames go again (`FieldArrived.send`).
    pub(crate) fn fade_field(&mut self, shot: ShotKey) {
        match self.fields.fade(shot) {
            Ok(records) => {
                for (update, delivery) in records {
                    self.push(Record {
                        update,
                        delivery: Delivery::Field(delivery),
                    });
                }
            }
            Err(error) => eprintln!("a faded Biotic Field frame failed ({error})"),
        }
    }

    /// `_end`: the fields of the shots that no longer run go.
    pub(crate) fn end_fields(&mut self, live: &[ShotKey]) {
        let records = self.fields.end(|shot| live.contains(&shot));
        if records.is_empty() {
            return;
        }
        for (update, delivery) in records {
            self.push(Record {
                update,
                delivery: Delivery::Field(delivery),
            });
        }
        self.remove_stale();
    }

    fn queue_projectiles(
        &mut self,
        id: BodyId,
        deliveries: Vec<ProjectileDelivery>,
    ) -> io::Result<()> {
        if deliveries.is_empty() {
            return Ok(());
        }
        for delivery in deliveries {
            let update = delivery.update().map_err(invalid)?;
            let delivery = match delivery {
                ProjectileDelivery::Confirm { token, .. } => {
                    self.projectile_destroys.remove(&update.entity);
                    Delivery::ProjectileConfirm { id, token }
                }
                ProjectileDelivery::Destroy { token, .. } => {
                    self.projectile_destroys.insert(update.entity, (id, token));
                    Delivery::ProjectileDestroy { id, token }
                }
            };
            self.push(Record { update, delivery });
        }
        self.remove_stale();
        Ok(())
    }

    /// One unit per physics pose, unposed VM frame, pre-body command, or queued game message.
    /// A fresh frame is committed only after its final pose has completed.
    pub fn advance_input(
        &mut self,
        scene: &PreparedScene,
        viewer: &PlayerAppearance<'_>,
        quanta: u32,
        budget: usize,
    ) -> io::Result<(usize, bool)> {
        let dt = f64::from(quanta) / 1_000_000.0;
        let mut remaining = budget;
        while remaining != 0 {
            let Some(work) = self.input.work.pop_front() else {
                break;
            };
            match work {
                InputWork::Message {
                    message,
                    tick,
                    received,
                } => {
                    let _ = self.game_message(scene, viewer, tick, received, quanta, &message);
                    remaining -= 1;
                }
                InputWork::Skin {
                    hero,
                    tick,
                    received,
                } => {
                    let _ = self.apply_skin(scene, viewer, hero, tick, received, quanta);
                    remaining -= 1;
                }
                InputWork::Command {
                    command,
                    mut pending,
                } => {
                    let (used, complete) =
                        self.advance_command(scene, command, &mut pending, dt, remaining)?;
                    remaining -= used;
                    if !complete {
                        self.input
                            .work
                            .push_front(InputWork::Command { command, pending });
                        break;
                    }
                    self.input.processed = Some(command.frame);
                }
                InputWork::Leave => return Ok((budget - remaining + 1, true)),
            }
        }
        Ok((budget - remaining, false))
    }

    fn advance_command(
        &mut self,
        scene: &PreparedScene,
        command: Command,
        pending: &mut Option<PendingCommand>,
        dt: f64,
        budget: usize,
    ) -> io::Result<(usize, bool)> {
        let Some(mover) = &mut self.mover else {
            return Ok((1, true));
        };
        // match.py `Player.take_commands`: a dead body lies where it fell; its
        // script still takes each command, from the command's own frame.
        let dead = self.death.is_some();
        let pending = pending.get_or_insert_with(|| {
            if dead {
                let gap = self
                    .body_script
                    .as_ref()
                    .map_or_else(ScriptGap::default, |script| {
                        ScriptGap::before(script.frame, command.frame)
                    });
                PendingCommand {
                    poses: kinematics_poses(None, command),
                    gap,
                }
            } else {
                PendingCommand::new(mover.pending_poses(command), self.body_script.as_ref())
            }
        });
        let mut used = 0;
        let mut failed = None;
        let script = &mut self.body_script;
        let shots = &mut self.shots;
        if let Some(script) = script {
            for (frame, step) in pending.gap.by_ref().take(budget) {
                used += 1;
                let held = script.input.map_or(
                    Command {
                        frame,
                        ..Command::default()
                    },
                    |input| repeated(input, frame),
                );
                if let Err(error) = script.advance(mover, held, step, dead) {
                    mover.track_modifiers(frame, &BTreeMap::new());
                    pending.gap = ScriptGap::default();
                    failed = Some(error.to_string());
                    break;
                }
            }
        }
        if dead {
            for command in pending.poses.by_ref().take(budget - used) {
                used += 1;
                if let Some(script) = script.as_mut().filter(|_| failed.is_none()) {
                    if let Err(error) = script.advance(mover, command, BodyStep::Posed, true) {
                        failed = Some(error.to_string());
                    }
                }
            }
            if let Some(error) = failed {
                self.drop_failed_script(&error);
            }
            return Ok((used, pending.poses.len() == 0));
        }
        mover.set_peers(&self.blockers);
        let rays = scene.movement_index();
        let stepped =
            mover.step_pending_with(&mut pending.poses, budget - used, |mover, command| {
                let Some(script) = script.as_mut().filter(|_| failed.is_none()) else {
                    mover.step_pose_to(rays, command, dt, None)?;
                    return Ok(());
                };
                // match.py `_step_body_pose`: one world-target state moves the
                // body; arbitration between several is unresolved, so none does.
                let requests = script.runtime.move_requests();
                let request = match requests[..] {
                    [request] => Some(request),
                    _ => None,
                };
                let outcome =
                    mover.step_pose_to(rays, command, dt, request.map(|r| (r.target, r.speed)))?;
                let ended = request.zip(outcome).map_or(Ok(()), |(request, arrived)| {
                    script.runtime.finish_move(request, arrived, command.frame)
                });
                match ended.and_then(|()| script.advance(mover, command, BodyStep::Posed, false)) {
                    Ok(hits) => {
                        shots.extend(hits);
                        start_motions(mover, script.runtime.take_motions(), dt);
                    }
                    Err(error) => {
                        mover.track_modifiers(command.frame, &BTreeMap::new());
                        failed = Some(error.to_string());
                    }
                }
                Ok(())
            });
        if let Some(error) = failed {
            self.drop_failed_script(&error);
        }
        used += stepped.map_err(invalid)?;
        Ok((used, pending.poses.len() == 0))
    }

    /// The next id of the player's body ring that neither this client nor
    /// any other client of the room still references (match.py
    /// `_next_body_id`).
    fn next_body(&self, now: Duration) -> Option<u32> {
        let suffix = self.body.map_or(0, |body| body.id.entity & 0xff);
        (0..255)
            .map(|offset| self.owner | ((suffix + offset) % 255 + 1))
            .find(|&entity| {
                !self.retained_elsewhere.contains(&entity)
                    && self.bodies.get(&entity).is_none_or(
                        |(_, lifetime)| matches!(lifetime, Lifetime::ReusableAt(at) if now >= *at),
                    )
            })
    }

    /// The bodies of this player's ring that other clients of the room still
    /// reference; [`Self::retained_bodies`] of their worlds.
    pub fn retain_elsewhere(&mut self, bodies: impl IntoIterator<Item = u32>) {
        self.retained_elsewhere.clear();
        self.retained_elsewhere.extend(bodies);
    }

    /// A new body for the player (match.py `switch_hero`), then a full
    /// controller frame: a pick closes hero select with it (unless the
    /// heroes are still assembling), and a body placed at the spawn point
    /// without a pick (a respawn, a new round) sends it with hero select as
    /// it was. That frame after the new body's possession is what the first
    /// spawn has and a respawn lacked in the game: the first spawn faced the
    /// spawn's way, a respawn (also one with a hero picked while dead, whose
    /// frame went at the pick, with the dead body) kept the view of the
    /// death (user's check, 2026-11-13). Assumption: the client sets its
    /// view to the possessed body's facing on that frame; it may need hero
    /// select closing instead (`refs/reports/RespawnFacing.md`).
    #[allow(clippy::too_many_arguments)]
    fn switch(
        &mut self,
        scene: &PreparedScene,
        viewer: &PlayerAppearance<'_>,
        tick: u32,
        quanta: u32,
        entity: u32,
        selection: (&Hero, SkinTheme),
        pick: bool,
    ) -> io::Result<()> {
        let (hero, skin) = selection;
        // A pick of a living body changes the hero where he stands, facing
        // the same way (no teleport: hero change in the spawn room); a
        // respawn or a first pick starts at the spawn point.
        let here = self
            .mover
            .as_ref()
            .filter(|_| pick && self.body.is_some() && self.death.is_none())
            .map(Mover::pose);
        let mover = match here {
            Some(here) => scene.player_mover_at(hero.body, viewer.team, here.position, here.yaw)?,
            None => scene.player_spawn_mover(hero.body, viewer.team, viewer.spawn, viewer.room)?,
        };
        let movement = mover.pose();
        let pose = owner_pose(movement);
        let generation = self
            .generation
            .checked_add(1)
            .ok_or_else(|| invalid("player body lifetime exhausted"))?;
        let id = BodyId { entity, generation };
        let team_bit = player_team_bit(scene.map(), viewer.team)?;
        let body = Body {
            id,
            hero: hero.guid,
            skin,
            team_bit,
        };
        // A new body is alive (`scene` module notes).
        let body_filter = body_filter(team_bit, false);
        let start_frame = self.input.processed.unwrap_or(tick);
        let script = scene
            .owner_body_runtime(
                hero,
                OwnerBodySpawn {
                    slot: u8::try_from(viewer.slot).map_err(invalid)?,
                    controller: self.owner,
                    entity,
                    body_filter,
                    frame: start_frame,
                    pose: movement,
                    quanta,
                },
            )
            .ok()
            .flatten()
            .and_then(|mut runtime| {
                runtime.set_target_candidates(&self.target_candidates);
                let settings = hero_setting_variables(&self.hero_settings, hero.guid);
                if settings.values().any(|value| *value != ScriptValue::Null) {
                    runtime.set_hero_settings(&settings).ok()?;
                }
                Some(runtime)
            })
            .and_then(|runtime| BodyScript::new(body, start_frame, runtime).ok());
        let create = body_create(
            BodyCreate {
                entity,
                hero,
                filter: body_filter,
                position: pose.position,
                yaw: pose.yaw,
                skin,
                health: script
                    .as_ref()
                    .and_then(|script| script.runtime.vitals())
                    .map(Vitals::component),
            },
            BodyView::Owner {
                pose: Some(pose),
                leases: script
                    .as_ref()
                    .and_then(|script| script.runtime.projectile_leases())
                    .map_or(&[], ProjectileLeases::blocks),
            },
            scene.pools(),
        );
        let possession = Possession {
            body: entity,
            hero: hero.guid,
            skin: (skin.guid != 0).then_some(skin.guid),
        };
        let placed = here.is_none() && !self.headless && self.can_handle_game_message();
        let open = if pick {
            self.assembling && self.select_open
        } else {
            self.select_open
        };
        let frame = (pick || placed)
            .then(|| self.controller_chunk(scene, tick, open, Some(hero.guid), None))
            .transpose()?;

        self.generation = generation;
        self.bodies.insert(entity, (generation, Lifetime::Live));
        self.created.remove(&entity);
        let old = self.body.replace(body);
        self.cancel_body_script();
        self.body_script = script;
        self.mover = Some(mover);
        self.bound = None;
        // combat.py `retire_player_body`: the old body's death and pools go
        // with it.
        self.death = None;
        self.plain_vitals = None;
        self.respawn_hero = None;
        self.spawn_room = None;
        if frame.is_some() {
            self.select_open = open;
        }
        // notices.py `hero_switched`: a switch from one hero to another, not
        // the first pick. Other clients take it from this player's view.
        if let Some(old) = old.filter(|old| old.hero != hero.guid) {
            self.switches += 1;
            let switch = Switch {
                old: old.hero,
                new: hero.guid,
                player: self.owner,
                serial: self.switches,
            };
            self.switch = Some(switch);
            if scene.switch_notices() && self.controller.latest_data() != 0 {
                self.notices.waiting.push_back(switch);
            }
        }
        self.push(Record {
            update: create,
            delivery: Delivery::CreateBody(id),
        });
        self.push(Record {
            update: EntityUpdate {
                entity: self.owner,
                operation: Some(Operation::Update(possession.components().into())),
                ..EntityUpdate::default()
            },
            delivery: Delivery::Possession(id),
        });
        if let Some(old) = old {
            self.retire_body(old.id);
        }
        if let Some(frame) = frame {
            self.push(frame);
        }
        self.remove_stale();
        Ok(())
    }

    fn retire_body(&mut self, id: BodyId) {
        self.created.remove(&id.entity);
        self.health_updates.remove(&id.entity);
        self.bodies
            .insert(id.entity, (id.generation, Lifetime::Retiring));
        self.push(Record {
            update: EntityUpdate {
                entity: id.entity,
                operation: Some(Operation::Destroy),
                ..EntityUpdate::default()
            },
            delivery: Delivery::DestroyBody(id),
        });
    }

    /// A full controller frame. A frame with a hero switch's presence bits is
    /// a separate event, even when the menu is already closed.
    fn controller_chunk(
        &mut self,
        scene: &PreparedScene,
        tick: u32,
        open: bool,
        hero: Option<u64>,
        presence: Option<&BTreeMap<usize, ScriptValue>>,
    ) -> io::Result<Record> {
        let command_frame = self.command_ack(tick);
        // The skin selector shows in the PvP modes while heroes are
        // assembled, in the Practice Range whenever the screen is open; only
        // the Practice Range hides the assemble title (match.py
        // `send_controller`).
        let practice = scene.controller().practice();
        let skins = open && (practice || self.assembling);
        let chunk = self
            .controller
            .full_owner(command_frame, |cmfd| {
                scene.controller().controller_frame(
                    cmfd,
                    ControllerFrame {
                        select_open: open,
                        skins,
                        hide_assemble: practice,
                        presence,
                        hero,
                        multikills: self.multikills.pulse(tick),
                    },
                )
            })
            .map_err(invalid)?;
        Ok(Record::script(
            chunk,
            hero.is_some() && !open && presence.is_none(),
        ))
    }

    fn queue_controller(
        &mut self,
        scene: &PreparedScene,
        tick: u32,
        open: bool,
        hero: Option<u64>,
        presence: Option<&BTreeMap<usize, ScriptValue>>,
    ) -> io::Result<()> {
        let record = self.controller_chunk(scene, tick, open, hero, presence)?;
        self.select_open = open;
        self.push(record);
        self.remove_stale();
        Ok(())
    }

    /// The movement records of this send frame: every other player's body
    /// from the frame after its create (match.py `remote_movements`), then
    /// the training bots once their creates had time to arrive.
    pub fn movement(&self, scene: &PreparedScene, tick: u32) -> Vec<EntityUpdate> {
        let mut records = self.others.movements().to_vec();
        if self
            .started
            .is_some_and(|started| tick.saturating_sub(started) >= BOT_RECORD_DELAY)
        {
            records.extend(
                scene
                    .bots()
                    .iter()
                    .filter(|bot| self.created.contains(&bot.entity) && bot.record_due(tick))
                    .map(|bot| EntityUpdate {
                        entity: bot.entity,
                        movement: Some(bot.movement(tick)),
                        ..EntityUpdate::default()
                    }),
            );
        }
        records
    }
}

fn command_prefix(
    anchor: Option<u32>,
    tick: u32,
    commands: &[Command],
) -> (Vec<Command>, Option<u32>) {
    // Classify the lease boundary independently of the resource window so a
    // fresh unencodable frame still closes its old link.
    let (mut fresh, boundary) = CommandQueue::new().before_limit(commands, MAX_LEASE_FRAME, anchor);
    let limit = tick.saturating_add(MAX_INPUT_LEAD);
    fresh.truncate(fresh.partition_point(|command| command.frame <= limit));
    (fresh, boundary)
}

/// A body's own pools when no script keeps them (combat.py `vitals_of`:
/// `pools.body_pools`).
fn hero_vitals(scene: &PreparedScene, hero: u64) -> Vitals {
    scene
        .pools()
        .hero_pools(hero)
        .unwrap_or(FALLBACK_POOLS)
        .vitals()
}

/// Hand the body's new dashes and rewinds to its mover; they run from the
/// next pose on (`orpheus_physics::mover::motion`).
fn start_motions(mover: &mut Mover, motions: Vec<Motion>, dt: f64) {
    for motion in motions {
        match motion {
            Motion::Dash { distance, seconds } => mover.dash(distance, seconds),
            Motion::Rewind { seconds, duration } => mover.rewind(seconds, duration, dt),
            Motion::EndRewind { finished } => mover.end_rewind(finished),
        }
    }
}

fn invalid(error: impl std::fmt::Display) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error.to_string())
}

#[cfg(test)]
mod tests;
