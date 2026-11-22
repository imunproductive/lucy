//! Per-peer world delivery, including the owning player's body lifetime.

use std::{
    borrow::Borrow,
    collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque},
    io,
    ops::Range,
    sync::Arc,
    time::Duration,
};

use crate::{
    body::{BodyCreate, BodyView, body_create, owner_pose},
    body_script::OwnerBodySpawn,
    bot_script::{BotEvent, BotViewContext, BotViews, SeenDelivery, SeenRocket},
    bots::BOT_RECORD_DELAY,
    combat::{
        BotReplacement, Combat, DEAD, DeathCamera, KnockCredit, PlayerHit, PlayerTarget,
        PosedProjectile, PosedShot, RESPAWN_RETRY, Shooter, TargetRewind, credits_damage,
        hit_messages,
    },
    controller::ControllerFrame,
    egress::{
        DeliveryReport, FrameRecords, GameEgress, PacketDelivery, RecordSource, SendError,
        SentFrame,
    },
    mode_script::ModeRunner,
    mode_view::{ModeFrame, ModeView},
    notices::{NOTICE_GAP, Switch},
    others::{
        ObservedScript, ObserverActivity, ObserverCache, OtherDelivery, Others, PlayerView,
        RemoteBody, RemoteDeath, ShowContext, ShowOutput, ViewerReadiness,
    },
    placed_script::{MadeView, PlacedFrame, PlacedViews},
    pools::FALLBACK_POOLS,
    projectile_flight::{FlightEnd, ProjectileFlight},
    projectiles::{
        FieldCopies, FieldDeliver, FieldDelivery, FiredProjectile, OwnerProjectiles,
        ProjectileDelivery, ProjectileEvent, ProjectileLeases, ProjectileToken, ServerCopies,
        ShotKey,
    },
    scene::{
        PlayerAppearance, Possession, SHIELDED, body_filter, filter_component, player_team_bit,
    },
    scene_assets::PreparedScene,
    spawned::ServerFlight,
    voice_lines::{VoiceLine, VoiceState, delay_ticks, is_death_cry},
    workshop::WorkshopView,
};
use lucy_content::{
    hero_settings::{HeroSetting, HeroSettings, SENT_SETTINGS, SettingDefault, script_settings},
    heroes::{ANA, DOOMFIST, Hero, SOLDIER, WIDOWMAKER},
    parameters::{ClientGameMessage, HeroSelectRequest, OPEN_OR_CLOSE, PICK_HERO, ParameterValue},
};
use lucy_core::tick::{MAX_LEASE_FRAME, TICK_PERIOD};
use lucy_physics::{
    contact::cache::CapsulePose,
    mover::correction::CorrectionHistory,
    mover::kinematics::{
        HOLD_INPUT_FRAMES, Poses, held, neutral, poses as kinematics_poses, repeated,
    },
    mover::modifiers::{Modifier, ModifierKey},
    mover::{MoveEnd, MoveKey, Mover},
};
use lucy_script::controller::{
    ControllerScript, HeroSelectRequest as SelectRequest, HeroSelectState, HeroSwitch,
};
use lucy_script::{
    ai_definitions::AiControl,
    graph::GraphStore,
    mode::EVERY_ENTITY,
    placed::{DummyChange, NextDummy},
    snapshot::{ProjectedSnapshot, ProjectionCache, SnapshotEncoder, View},
    stream::{BodyStream, ChangeKey, DeltaKind, ScriptChunk, Stream},
    vitals::{Death, Vitals},
    vm::{
        AssistLink, AuraChange, BodyEffects, DAMAGE_DEALT, DAMAGE_TAKEN, EnvironmentCredit,
        GameMessage, HEALING_DEALT, HEALING_TAKEN, Knockback, MessageEnd, Motion, MoveRequest,
        ParentWrite, Pickup, PlayRequest, RaisedCriteria, Revision, Scaled, ScopeChange,
        ScopeSource, Snapshot, TargetCandidate, TrackedStat, VoiceRequest, combat_scale,
    },
    weapons::{DamageTags, ProjectileBallistics},
    wire::{Value as ScriptValue, Variables},
};
use lucy_wire::{
    combat_messages::CombatNotice,
    commands::{Command, CommandQueue, FRAME_HALF, SUBFRAME, frame_distance},
    entity::{
        CLOCK_FASTER, CLOCK_SLOWER, CLOCK_STRONGER, Component as WorldComponent, Create,
        EntityUpdate, GAME_MODE_ENTITY, Movement, Operation, SkinTheme,
    },
    link::delivery,
    messages::Message,
};
use parts::{Part, PartOutcome, Parts};

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
// tick runs the oldest at once.
const MAX_BUFFERED: usize = 4;
// Retail's client shrank and grew its lead by dilating its 16 ms frame
// (15.2 ms, Ford) when the server asked (the ch3 clock flags,
// [`clock_flags`]). The server also keeps its buffer from its side: a
// buffer above the target for this many ticks in a row runs one command
// more, and one below it while the client sends waits a tick (assumption).
const DRAIN_TICKS: u32 = 20;
// A dry buffer waits this many ticks in a row for the client's next frame
// before it repeats his last input. A datagram a few frames late, or lost
// (the next carries every command since ack − 1), then runs its frames as
// the client made them; 4 frames (64 ms) is the buffer retail allowed
// before mid-2016. In the debugger runs of 2026-11-19 about half of the
// dry spells were shorter (OwnerPrediction.md "DayLatePress"); a longer
// silence is a client stall, and his body goes on with his last input for
// the others (retail), for `MAX_REPEATS` frames.
const MAX_WAIT: u32 = MAX_BUFFERED as u32;
// Past the wait a dry tick repeats the client's newest input for at most
// this many frames since his last datagram with commands, however far
// behind the frames run that input is; later dry ticks run a neutral frame
// (no button, no movement key, his aim), so a silent body still simulates
// (gravity, a fall, its timers) but stops walking and firing. Retail
// duplicated the last input to bridge the gap until the client, told to
// run faster, refilled the buffer (GDC 2017, Ford 30:50, GDC-Netcode.md
// §1); Ford gives no count. A stalling client (a loaded PC, 27 fps) goes
// silent for 150-450 ms at a time with his keys held, then sends every
// frame of the stall late, and falls 30 frames and more behind the frames
// run; the 4 repeats this rule had stopped his body in those stalls where
// his client walked on (MapEntities.md "Console slide divergence
// (MapAnim)", sessions c2, st1, st2). The wait and the repeats together
// hold his input 0.5 s, the mover's own horizon for a held input
// (`HOLD_INPUT_FRAMES`).
const MAX_REPEATS: u32 = HOLD_INPUT_FRAMES - MAX_WAIT;

/// The ch3 clock flags for a client whose command frames came `lead` frames
/// ahead of the server's last tick when they first arrived (the latest of
/// them since the last tick: `InputQueue::commands`). They move his clock
/// toward the lead that puts the next tick's command and one more
/// ([`BUFFER_TARGET`]) in hand before the next tick runs, `lead` 2: faster
/// with less, four times as much again when the next tick's own came late
/// (0 or less); slower with more, four times as much again two frames or
/// more over. A client that sends in bursts (a stalling PC, the debugger)
/// is sped up until even a burst's oldest new frame comes in time, as
/// retail's client dilated time when its input was lost (Ford 30:50); one
/// that leads by more than he needs is slowed.
///
/// Retail's server "measures how early or late each command frame arrives
/// and tells the client" (GDC-Netcode.md §1): the lead, not the buffer's
/// depth. The depth can't show it: a burst runs all but [`MAX_BUFFERED`]
/// at once and a dry tick waits, so a client 11 frames ahead kept 1-4
/// commands in hand and got "slower" or nothing, while every dry tick after
/// a stall asked for "faster × 4" (refs/reports/Latency.md §6). His clock
/// also jumps ahead on its own: at each spawn (his entity's create,
/// `0x7FF78969C360` → `0x7FF7896D1400`) and whenever the packet frame passes
/// it (the ch3 handler `0x7FF7896DADD0`; a frame over 150 ms advances it by
/// 150 ms only, `0x7FF789AC7140`), it restarts at the newest packet frame
/// plus his link's smoothed round trip in frames (`0x7FF7894A6960`,
/// connection +0x118, at most 16). Right after the map loads that round
/// trip still holds the loading's stalls, so his first spawn could start up
/// to 16 frames (256 ms) ahead (10.9 in one run), and the old flags pulled
/// him in by 0.2 frames a second. The client scales each frame's time by
/// `1 ∓ k` (k = 0.625 %) and by `1 ∓ 4k` again with [`CLOCK_STRONGER`]
/// (`0x7FF789D28B10`): 0.39 or 1.9 frames a second.
fn clock_flags(lead: i32) -> u8 {
    let target = BUFFER_TARGET as i32 + 1;
    match lead {
        lead if lead <= 0 => CLOCK_FASTER | CLOCK_STRONGER,
        lead if lead < target => CLOCK_FASTER,
        lead if lead > target + 1 => CLOCK_SLOWER | CLOCK_STRONGER,
        lead if lead > target => CLOCK_SLOWER,
        _ => 0,
    }
}

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
/// left from the wrong place. Widowmaker is corrected since her movement
/// state carries her Grappling Hook's pull (`005A` st1, a `MoveTo`): flag
/// 0x10 and the pull's record, with its air ticks as the lateral count
/// (`lucy_physics::mover::move_to`). Before (2026-11-19, `Grapple.md`
/// "DayGrapple6") each correction during a pull made her client's replay
/// begin its mover behaviour again (`0x7FF789B53CC0`, about 30 times in
/// one pull: checkpoint at the feet, lateral count 0) while the server's
/// run kept its own, so her client drifted from the server's body (0.12 to
/// 0.5 m) and was pulled back. A ledge pull whose navigation path her
/// client finds (`STU_1F5ED928`) would still pull her back. Doomfist is
/// corrected since 2026-11-21 (DoomfistMoves15, refs report Doomfist.md
/// §23): his abilities' movement matched his client's frame for frame, but
/// without corrections whatever moved his client off the server's body
/// stayed (a client stall, a slam leap his client applied twice after a
/// rollback, a corner contact turning a few millimetres into a metre);
/// Practice Range runs ended 0.7-1.5 m off uncorrected and at 0.000 m
/// corrected. His Rocket Punch's dash is server-only (`0D06` st42), so his
/// client follows it from these records. The other heroes stay out until
/// the server runs all of their movement (`docs/notes/behavior.md`, "Owner
/// corrections"); they get their body's records only in windows
/// ([`server_shows_owner_movement`]).
const CORRECTED_HEROES: [u64; 4] = [SOLDIER, ANA, WIDOWMAKER, DOOMFIST];

/// How long an uncorrected hero's owner still gets his body's records after
/// the movement that needed them (upstream `SERVER_MOTION_TAIL_US`,
/// leafall903/ORPHEUS (MIT) 6bb305e9): his client needs one to take the end
/// of the movement, and the state's end can still change it.
const RECORD_TAIL_MS: u32 = 250;

/// Whether a body's statescript moves it now in a way only the server's
/// records show its owner, so he gets them although his hero isn't
/// corrected: a `STU_4773DA56` registration is on (Reinhardt's `007D` st2
/// and `0083` st9, `01C9` st8, `0CCE` st1; Rocket Punch's `0D06` st67, now
/// corrected): his client then draws his first-person view from the
/// movement states it keeps of his body, which only the records bring up
/// to date (upstream 266b28fd, checked with the original client; without
/// records the view sat where the create left it). Reinhardt's Charge
/// (`007D`) needs no lead: its registration st2 is on through the whole
/// charge (#435, st18 under the charge's MovementMod st36).
pub(crate) fn server_shows_owner_movement(snapshot: &Snapshot, graphs: &GraphStore) -> bool {
    const REGISTRATION: &str = "STU_4773DA56";
    snapshot.instances.values().any(|instance| {
        let Some(graph) = graphs.graph(instance.graph) else {
            return false;
        };
        instance
            .states
            .iter()
            .filter(|(_, state)| state.active)
            .any(|(&raw, _)| {
                graph
                    .state(usize::from(raw))
                    .is_some_and(|node| node.class == REGISTRATION)
            })
    })
}

/// `0013.025`: the entity is revived (the bots' reset, [`lucy_script::bot::RESET`]).
const REVIVED: u64 = lucy_script::bot::RESET;
/// `0013.025`'s `00C6` (`01CA`'s #2227): on for a new life.
const REVIVED_RESPAWN: u32 = 0x00c6;
/// `0685.025`: the mode body script `0C8E` (rid2/rid78) tells the mode a
/// dead body's respawn count ended (`0221` its controller, `03D9` the
/// respawn camera's #22241).
pub const RESPAWN_READY: u64 = 0x0240_0000_0000_0685;
/// `002D.025` {`0029` an identifier}: the game's identifier, which `01C7`
/// rid32 keeps in #59 and tests for #1928e.
const GAME_IDENTIFIER: u64 = 0x0240_0000_0000_002d;
/// `080D.025` to a player's controller: the heroes he may pick, in
/// `0458.026` (`0C90` st65 → #10489).
const ALLOWED_HEROES: u64 = 0x0240_0000_0000_080d;
const ALLOWED_HEROES_LIST: u32 = 0x0458;
/// `06FD.025` held by the mode graph to every entity: the bodies' `0C8E`
/// st2 takes the respawn seconds (`00B7`) from it.
const RESPAWN_MESSAGE: u64 = 0x0240_0000_0000_06fd;

/// Whether a living body of team `other` blocks the moves of a body of
/// `team` (the server's mover then takes its capsule,
/// [`WorldDelivery::set_blockers`]): an enemy's does, an ally's does not,
/// and in a free-for-all everyone is an enemy, as combat counts them. Retail
/// OW1: "you can clip through other allies ... However, you cannot clip
/// through enemies" (Furious Paul's Overwatch mechanics guide, 2017);
/// upstream's server leaves allies out the same way (leafall903/ORPHEUS
/// (MIT) `mccree.py` `_npc_peer_contacts`). A dead body blocks nobody. The
/// client does the same: its mover's {category, mask} (`0x7FF789B463F0`)
/// takes mask 0x1499 (both team categories 8 and 16) and clears its own
/// team's bit when component 31's byte `+0x14DF` is 0 (which
/// `STU_6CC90892` clears while active) or its protected byte `+0x14DA`
/// says so; team index 4 (free-for-all) keeps both bits (PlayerHurt.md,
/// "Client tools pass 2 (2026-11-16)"). Which of the two bytes is set for
/// a player's mover in a match is not read.
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

    /// The body's target tags component (76), when its states added or
    /// suppressed a tag or stopped doing so since the last call.
    fn take_target_tags_update(&mut self) -> Option<WorldComponent> {
        None
    }

    fn projectile_leases(&self) -> Option<&ProjectileLeases> {
        None
    }

    fn movement_modifiers(&self) -> Option<&BTreeMap<ModifierKey, Modifier>> {
        None
    }

    /// Whether the body's statescript moves it in a way only the server's
    /// records show its owner now ([`server_shows_owner_movement`]).
    fn owner_needs_records(&self) -> bool {
        false
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

    /// An aura comes on the body or goes off it (`vm/auras.rs` in
    /// `lucy_script`): a hit's, or an ApplyAura's.
    fn change_aura(&mut self, _change: &AuraChange) -> io::Result<()> {
        Ok(())
    }

    /// The aura changes the body's ApplyAura states made since the last
    /// call, for the match to hand to their targets.
    fn take_aura_changes(&mut self) -> Vec<AuraChange> {
        Vec::new()
    }

    /// A scope instance opened or closed on the body (`vm/scopes.rs` in
    /// `lucy_script`; the Training Partner console's scope 12): the
    /// instance opened.
    fn change_scope(&mut self, _change: &ScopeChange) -> io::Result<Option<u16>> {
        Ok(None)
    }

    /// Another graph set the cooldown of the body's ability on logical
    /// button `button` (the Workshop's Set Ability Cooldown,
    /// `lucy_script::vm::Component::set_ability_cooldown`).
    fn set_ability_cooldown(&mut self, _button: i64, _value: f64) -> io::Result<()> {
        Ok(())
    }

    /// The cooldown left on the body's ability on logical button `button`
    /// (the Workshop's Ability Cooldown); 0 without one.
    fn ability_cooldown(&self, _button: i64) -> f64 {
        0.0
    }

    /// The body's entity variable `id` as its script holds it now, a
    /// stacked value included (for the controller's graphs that read it,
    /// `ControllerScript::sync_body_variables`); null without one.
    fn entity_variable(&self, _id: u32) -> ScriptValue {
        ScriptValue::Null
    }

    /// The scope instances the body's scope states opened or closed on
    /// their targets since the last call (the console's scope 197 from
    /// scope 12, on a re-picked dummy), for the match.
    fn take_scope_changes(&mut self) -> Vec<ScopeChange> {
        Vec::new()
    }

    /// The match opened the scope instance `instance` on `target` for the
    /// body's scope state `source`.
    fn scope_opened(&mut self, _source: ScopeSource, _target: u32, _instance: u16) {}

    /// The graphs the body's ability graphs played on other bodies since
    /// the last call (Rocket Punch's `0D07` on its victims), for the match
    /// to play in their scripts ([`Self::play_script`]).
    fn take_play_requests(&mut self) -> Vec<PlayRequest> {
        Vec::new()
    }

    /// Another body's graph played `request.graph` on this body: played
    /// in its script. False: it plays none.
    fn play_script(&mut self, _request: &PlayRequest) -> io::Result<bool> {
        Ok(false)
    }

    /// The knockbacks the body's graphs reported since the last call
    /// (`STU_AFBB59E9`, `lucy_script::vm::Knockback`).
    fn take_knockbacks(&mut self) -> Vec<Knockback> {
        Vec::new()
    }

    /// The variables the body's scope instances set in their scope states'
    /// instances since the last call (`STU_D49BBD03`), for the match.
    fn take_parent_writes(&mut self) -> Vec<ParentWrite> {
        Vec::new()
    }

    /// The player and body the next dummy bot the body's graphs make gets
    /// (the console's scope 12 on a re-pick); none: they make none.
    fn set_next_dummy(&mut self, _next: Option<NextDummy>) {}

    /// The dummy bots the body's graphs made or removed since the last
    /// call, for the match.
    fn take_dummy_changes(&mut self) -> Vec<DummyChange> {
        Vec::new()
    }

    /// The statuses and combat modifiers the body's states hold.
    fn effects(&self) -> Option<&BodyEffects> {
        None
    }

    /// What a health change of `kind` with `tags` on the body, with
    /// `other` its attacker or healer, becomes (`BodyEffects::scale`): the
    /// modifiers its states hold. A runtime that can't test a party filter
    /// on `other` leaves out the modifiers that have one.
    fn combat_scale(&mut self, kind: u64, tags: &mut DamageTags, _other: u32) -> Scaled {
        let own = self.effects().into_iter().flat_map(BodyEffects::modifiers);
        combat_scale(own, kind, tags, &[])
    }

    /// The assist credits the body's states hold now, the lingering ones
    /// included (`lucy_script::vm` [`AssistLink`]).
    fn assists(&self) -> Vec<AssistLink> {
        Vec::new()
    }

    /// A game message from another entity (`sender`; 0: none known), run
    /// now (match.py `deliver_body_messages`, combat.py `_died`). Returns
    /// how many entries took it.
    fn receive_message(
        &mut self,
        _sender: u32,
        _message: u64,
        _parameters: Variables,
    ) -> io::Result<usize> {
        Ok(0)
    }

    /// A persistent sender (`sender`) let its message go for the body: its
    /// receiving states that held it go back to idle, now.
    fn end_message(&mut self, _sender: u32, _message: u64) -> io::Result<()> {
        Ok(())
    }

    /// The game messages the body sent other entities since the last call.
    fn take_messages(&mut self) -> Vec<GameMessage> {
        Vec::new()
    }

    /// The ends of the body's persistent sends to other entities since the
    /// last call ([`crate::messages::deliver`]).
    fn take_message_ends(&mut self) -> Vec<MessageEnd> {
        Vec::new()
    }

    /// The body's persistent sends' new parameters since the last call
    /// ([`crate::messages::deliver_updates`]).
    fn take_message_updates(&mut self) -> Vec<GameMessage> {
        Vec::new()
    }

    /// The ends of the persistent sends the body's graphs hold now: what
    /// goes when its script stops.
    fn held_sends(&self) -> Vec<MessageEnd> {
        Vec::new()
    }

    /// Whether the body's graphs revived it since the last call (`0C8E`'s
    /// `STU_621AF478` on the Workshop's Resurrect).
    fn take_revive(&mut self) -> bool {
        false
    }

    /// The authored target tags of the body the player just eliminated,
    /// which the body's graphs test when they take `0093.025`.
    fn set_victim_tags(&mut self, _victim: u32, _tags: BTreeSet<u64>) {}

    /// The criteria the body's graphs raised on it or its controller since
    /// the last call ([`crate::voice_lines`]).
    fn take_criteria(&mut self) -> Vec<RaisedCriteria> {
        Vec::new()
    }

    /// The voice stimuli the body's server-only states asked it to say
    /// since the last call ([`crate::voice_lines`]).
    fn take_voices(&mut self) -> Vec<VoiceRequest> {
        Vec::new()
    }

    /// The stats the body's graphs counted since the last call (TrackStat,
    /// TrackStatTime): the match adds them to its players' stats.
    fn take_tracked_stats(&mut self) -> Vec<TrackedStat> {
        Vec::new()
    }

    /// Who the body's graphs named its environment kill credits since the
    /// last call (`STU_545AB3B8`).
    fn take_environment_credits(&mut self) -> Vec<EnvironmentCredit> {
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

    /// The bodies the body's target searches can find from now on
    /// (`Combat::target_candidates`).
    fn set_target_candidates(&mut self, _candidates: &[TargetCandidate]) {}

    /// The pickups the map's placeables hold now (the health packs), for
    /// the body's search (`0033` st11, `lucy_script` `vm/pickups.rs`).
    fn set_pickups(&mut self, _pickups: &[Pickup]) {}

    /// The map placeables' entity variables now, which the body's graphs
    /// read of other entities.
    fn set_map_variables(&mut self, _variables: &Arc<[(u32, Variables)]>) {}

    /// The death layer's parameters of the death message for a kill by the
    /// body `killer` ([`DeathCamera`]), as the body's world knows the
    /// killer and the body; none: the body runs no script that reads them.
    fn death_camera(&mut self, _killer: u32) -> Option<DeathCamera> {
        None
    }

    /// Where the player saw those bodies at `command`'s view, for the
    /// searches that rewind, before the body's script runs the command.
    fn see_targets(&mut self, _rewind: &TargetRewind<'_>, _command: &Command) {}

    /// Entity variables the server writes onto the body, which no graph
    /// writes from outside: the player's hero settings
    /// ([`hero_setting_variables`]) and the ultimate charge a respawn's new
    /// body keeps ([`WorldDelivery::respawn_due`]).
    fn set_entity_variables(&mut self, _variables: &Variables) -> io::Result<()> {
        Ok(())
    }

    /// Whether the body runs a mode body script that counts its respawn
    /// down and sends [`RESPAWN_READY`] to the mode when it ends (`0C8E`).
    fn counts_respawn(&self) -> bool {
        false
    }

    /// The match's overtime, which the body's mode script reads as the
    /// mode entity's #3392e.
    fn set_mode_overtime(&mut self, _overtime: bool) -> io::Result<()> {
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
    /// The server projectiles fired so far ([`ProjectileToken::server`]).
    server_shots: u64,
    /// The projectiles fired since the match last took them, for the other
    /// clients (projectiles.py `fired`: `_show_others`).
    fired: Vec<FiredProjectile>,
    /// The buttons of the last command traced (`lucy::owner` at debug).
    traced_buttons: u32,
    /// The active states (instance, graph, state, activation) at the last
    /// frame traced (`lucy::pose` at debug).
    traced_states: BTreeSet<(u16, u32, u16, u16)>,
    /// The last command the body ran with a pose: the frames of a gap
    /// repeat it ([`ScriptGap`]).
    input: Option<Command>,
    /// The last owner projection, made again only when the script changed
    /// or a pulse or wheel action in it ran out.
    projection: Option<ProjectionCache>,
    /// What the other clients get of the script, shared between them.
    observers: ObserverCache,
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
        let corrections = runtime.movement_modifiers().is_some().then(|| {
            if CORRECTED_HEROES.contains(&body.hero) {
                CorrectionHistory::new(body.id.entity, body.id.generation)
            } else {
                CorrectionHistory::windowed(body.id.entity, body.id.generation)
            }
        });
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
            server_shots: 0,
            fired: Vec::new(),
            traced_buttons: 0,
            traced_states: BTreeSet::new(),
            input: None,
            projection: None,
            observers: ObserverCache::default(),
        })
    }

    /// `lucy::pose` at debug: the body's movement after a command frame,
    /// and the states that began or ended in it (instance/graph/state#activation),
    /// to compare with the client's prediction frame by frame.
    fn trace_pose(&mut self, mover: &Mover, frame: u32) {
        if !tracing::enabled!(target: "lucy::pose", tracing::Level::DEBUG) {
            return;
        }
        let active: BTreeSet<_> = self
            .runtime
            .snapshot()
            .into_iter()
            .flat_map(|snapshot| {
                snapshot.instances.iter().flat_map(|(&id, instance)| {
                    instance
                        .states
                        .iter()
                        .filter(|(_, state)| state.active)
                        .map(move |(&raw, state)| (id, instance.graph, raw, state.activation))
                })
            })
            .collect();
        let name = |&(id, graph, raw, activation): &(u16, u32, u16, u16)| {
            format!("{id}/{graph:04X}/{raw}#{activation}")
        };
        let began: Vec<_> = active.difference(&self.traced_states).map(name).collect();
        let ended: Vec<_> = self.traced_states.difference(&active).map(name).collect();
        self.traced_states = active;
        let pose = mover.pose();
        // Widowmaker's Grappling Hook's outputs when a state changes: #59
        // (the switch), #431 (hook point), #433 (way), #434 (normal), #435
        // (hit).
        let hook: Vec<_> = (!began.is_empty() || !ended.is_empty())
            .then(|| self.runtime.snapshot())
            .flatten()
            .into_iter()
            .flat_map(|snapshot| snapshot.instances.values())
            .filter(|instance| instance.graph == 0x005a)
            .flat_map(|instance| {
                [59, 431, 433, 434, 435]
                    .into_iter()
                    .filter_map(|id| Some(format!("#{id}={:?}", instance.variables.get(&id)?)))
            })
            .collect();
        tracing::debug!(
            target: "lucy::pose",
            body = format_args!("{:08X}", self.id.entity),
            frame,
            position = ?pose.position,
            velocity = ?pose.velocity,
            flags = format_args!("{:#x}", pose.flags),
            began = ?began,
            ended = ?ended,
            hook = ?hook,
            "owner pose"
        );
    }

    /// One frame of the body's script, its searches that rewind seeing the
    /// bodies as `rewind` has them at the command's view. A dead body's
    /// reports go nowhere (combat.py `Combat.command`: the dead do not
    /// shoot).
    fn advance(
        &mut self,
        mover: &mut Mover,
        command: Command,
        step: BodyStep,
        rewind: Option<&TargetRewind<'_>>,
        dead: bool,
    ) -> io::Result<Vec<PosedShot>> {
        if command.frame > self.frame {
            if let Some(rewind) = rewind {
                self.runtime.see_targets(rewind, &command);
            }
            self.runtime.advance(command, mover, step)?;
            self.frame = command.frame;
            if step == BodyStep::Posed {
                self.input = Some(command);
            }
            if command.buttons != self.traced_buttons {
                self.traced_buttons = command.buttons;
                tracing::debug!(
                    target: "lucy::owner",
                    body = format_args!("{:08X}", self.id.entity),
                    frame = command.frame,
                    buttons = format_args!("{:#x}", command.buttons),
                    step = ?step,
                    volleys = %self.volleys(),
                    "command buttons changed"
                );
            }
            self.trace_pose(mover, command.frame);
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
            if self.runtime.owner_needs_records() {
                let tail = RECORD_TAIL_MS.div_ceil(self.runtime.frame_ms());
                history.open(command.frame, command.frame.saturating_add(tail));
            }
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
    /// projectiles of the shot is confirmed. A server projectile (Biotic
    /// Grenade, [`PosedProjectile::server_shown`]) flies and every client
    /// sees it, its shooter's included ([`ServerCopies`]). combat.py
    /// `_shoot`: another unleased one flies for its damage alone. A shot of
    /// pellet rings flies each pellet for its damage (upstream 7cb4018); a
    /// leased one's confirmation and release still follow one projectile
    /// along the aim, which then does no damage, so the wire is as without
    /// rings.
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
                    (true, _) => {
                        projectiles.admit_field(projectile.shot, definition, bound, projectile.feet)
                    }
                    (false, Some(_)) => {
                        projectiles.admit_spawning(projectile.shot, definition, bound)
                    }
                    (false, None) => projectiles.admit_volley(
                        projectile.shot,
                        definition,
                        bound,
                        projectile.projectiles,
                    ),
                }
                .map(|token| (token, definition))
            }
            (_, Some(definition)) if projectile.server_shown => {
                self.server_shots += 1;
                Some((ProjectileToken::server(self.server_shots), definition))
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
            tracing::debug!(target: "lucy::owner", base = base_last, "owner delta base");
            let projected = encoder
                .project_cached(snapshot, View::Owner, Some(base), &mut self.projection)
                .map_err(invalid)?;
            let gated;
            let snapshot = match kill_feed {
                Some(allowed) => {
                    gated =
                        projected
                            .clone()
                            .gate_kill_feed(encoder.graphs(), &self.stream, allowed);
                    &gated
                }
                None => projected,
            };
            // A correction goes only when something changed
            // (`SnapshotEncoder::queue_delta`).
            let Some(chunk) = encoder
                .queue_delta(&mut self.stream, snapshot, kind, self.runtime.frame_ms())
                .map_err(invalid)?
            else {
                return Ok(None);
            };
            (chunk, Some(kind))
        };
        let last = chunk.last;
        let replaced = kind.and_then(|kind| {
            self.unsent_delta.replace((last, kind)).map(|(last, _)| {
                self.stream.discard_unsent(last);
                last
            })
        });
        tracing::debug!(
            target: "lucy::owner",
            body = format_args!("{:08X}", self.id.entity),
            chunk = last,
            kind = ?kind,
            processed = self.frame,
            bytes = chunk.update.chunk.as_ref().map_or(0, |chunk| chunk.as_bytes().len()),
            replaced = ?replaced,
            volleys = %self.volleys(),
            keys = ?self.stream.pending().find(|&(number, ..)| number == last).map(|(.., keys)| keys),
            "owner frame queued"
        );
        let record = Record::new(
            chunk.update,
            Delivery::BodyScript {
                id: self.id,
                last,
                delta: kind.is_some(),
            },
        );
        Ok(Some((record, replaced)))
    }

    /// The body's weapon volleys for the `lucy::owner` trace: each active
    /// one as `instance/state #activation start=ms ammo0 var=value`.
    fn volleys(&self) -> String {
        use std::fmt::Write;

        let mut out = String::new();
        let Some(snapshot) = self.runtime.snapshot() else {
            return out;
        };
        for (id, instance) in &snapshot.instances {
            for (raw, state) in &instance.states {
                let lucy_script::vm::StateKind::WeaponVolley {
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

/// Whose graphs made an entity ([`WorldDelivery::update_made`]).
#[derive(Clone, Copy)]
enum MadeBy {
    Mode,
    Placeables,
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
    /// The body's target tags (component 76); a newer one supersedes it.
    Tags {
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
    /// A chunk of the game mode entity's frames (`mode_view.rs`).
    Mode {
        last: u32,
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
    /// The player's own copy of his server projectile (projectiles.rs
    /// `ServerCopies`).
    ServerSeen(SeenDelivery),
    /// A server placeable's full frame (placed_script.rs); a newer one
    /// supersedes it.
    Placed {
        last: u32,
    },
    /// The create (false) or destroy (true) of an entity the mode entity's
    /// graphs made ([`WorldDelivery::update_mode_entities`]), whose frames
    /// go as a placeable's ([`Self::Placed`]); or the destroy of a server
    /// placeable that went ([`WorldDelivery::update_placed`]).
    ModeEntity {
        destroy: bool,
    },
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
    /// The current body's filter word with or without [`SHIELDED`]
    /// (`WorldDelivery::shield_step`); a newer one supersedes it.
    Filter {
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
    /// The record's part, when its chunk goes in parts (`parts.rs`).
    part: Option<Part>,
}

impl Record {
    fn new(update: EntityUpdate, delivery: Delivery) -> Self {
        Self {
            update,
            delivery,
            part: None,
        }
    }

    pub(crate) fn baseline(update: EntityUpdate) -> Self {
        Self::new(update, Delivery::Baseline)
    }

    fn script(chunk: ScriptChunk, close: bool) -> Self {
        Self::new(
            chunk.update,
            Delivery::Script {
                last: chunk.last,
                close,
                resend: chunk.resend,
            },
        )
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
    click: Option<lucy_wire::commands::Click>,
    /// The last one-shot action of the late commands.
    action: u8,
}

/// A player's input, in command-frame order (GDC 2017, Ford 30:50; Reed
/// s.42). Commands wait in `buffer` until a tick runs them
/// ([`InputQueue::release`]): one a tick, the next of the client's frames.
/// Frames already run or admitted are dropped (the client's redundant
/// window resends them); later ones wait. A tick that finds no command
/// first waits for it, up to [`MAX_WAIT`] ticks in a row: the frame then
/// runs with its own input when it comes, as the client predicted it. A
/// longer silence runs the client's last input again for the next frame
/// each tick (retail duplicates the last input), so his body goes on for
/// the others, for at most [`MAX_REPEATS`] frames since his last datagram;
/// then each dry tick runs a neutral frame, his aim with nothing held, so a
/// silent body falls and its timers run but it neither walks nor fires.
/// When the real command of a frame run so comes, the presses it had and
/// the frame lacked go with the next command run, so no press is lost,
/// though it runs a few frames after the client's (a misprediction the
/// client's rollback shows as a volley begun twice,
/// refs/reports/OwnerPrediction.md "DayLatePress"). A game message keeps
/// its place among the commands: it runs at the boundary before the first
/// command released after it, the frame the next owner frame names.
#[derive(Default)]
struct InputQueue {
    work: VecDeque<InputWork>,
    /// Commands and messages not yet released, in arrival order.
    buffer: VecDeque<InputWork>,
    /// Input that runs as it comes, with no buffer: an AI player's, made on
    /// the server a tick at a time.
    immediate: bool,
    first_admitted: Option<u32>,
    /// The newest frame admitted: a client's command, a repeat or a
    /// neutral frame.
    admitted: Option<u32>,
    processed: Option<u32>,
    /// The client's newest command (late ones included): a dry tick past
    /// the wait repeats it.
    last: Option<Command>,
    /// The buttons of the last command released.
    ran: u32,
    /// Ticks in a row that found no command while the client sends for a
    /// live body: the waits, then the repeats and neutral frames.
    dry: u32,
    /// Frames repeated since the client's last datagram with commands, up
    /// to [`MAX_REPEATS`].
    repeats: u32,
    /// Ticks in a row with more than [`BUFFER_TARGET`] commands left waiting.
    surplus: u32,
    /// A command of the client came since the last tick.
    arrived: bool,
    late: LateInput,
    /// The client's lead: how early the command frames that came for the
    /// first time since the last release arrived, the latest of them (its
    /// frame less the server's last tick when it came), until a release
    /// takes it ([`clock_flags`]).
    arrival_lead: Option<i32>,
    /// The lead the clock flags follow: the last release's `arrival_lead`,
    /// kept while no new frame comes.
    lead: i32,
    /// The ch3 clock flags of the frames sent after the last release.
    clock: u8,
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
            // Runs as it comes: its buttons are the ones run
            // ([`Self::take_late`], which the Workshop's Is Button Held reads).
            let work = match work {
                InputWork::Command { command, pending } => InputWork::Command {
                    command: self.take_late(command),
                    pending,
                },
                work => work,
            };
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
        // Late (newest at or before `admitted`) or with frames the client
        // skipped (fewer commands than frames) after a stall.
        tracing::debug!(
            target: "lucy::input",
            tick,
            oldest = commands[0].frame,
            newest = commands[commands.len() - 1].frame,
            count = commands.len(),
            admitted = ?self.admitted,
            "commands arrived"
        );
        // How early each command frame arrives, the first time it does
        // (the redundant window resends older ones): the oldest new frame
        // came the latest ([`clock_flags`]). A datagram that waited, or a
        // burst after a stall, shows a small lead or none.
        let seen = self.last.map(|command| command.frame);
        if let Some(first) = commands.iter().find(|command| {
            seen.is_none_or(|seen| (1..FRAME_HALF).contains(&frame_distance(command.frame, seen)))
        }) {
            let lead = first.frame.wrapping_sub(tick) as i32;
            self.arrival_lead = Some(self.arrival_lead.map_or(lead, |earlier| earlier.min(lead)));
        }
        // The client sends: a dry tick may repeat his newest input again,
        // late ones included (he is behind the frames run, not silent).
        self.repeats = 0;
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
                target: "lucy::input",
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
    /// buffer grows by one). With no command in hand, the messages, and
    /// while the client sends for a live body (`body`) the dry tick waits
    /// for his next frame, up to [`MAX_WAIT`] in a row; each later one
    /// runs the next frame: his last input again, past [`MAX_REPEATS`]
    /// such frames since his last datagram a neutral one ([`neutral`]).
    /// Sets the clock flags the client's next frames get from his lead
    /// ([`clock_flags`]).
    fn release(&mut self, ticks: u32, body: bool) {
        self.clock = 0;
        if self.immediate || self.closing {
            return;
        }
        let arrived = std::mem::take(&mut self.arrived);
        let depth = self.depth();
        let sending = body && self.last.is_some();
        if let Some(lead) = self.arrival_lead.take() {
            self.lead = lead;
        }
        if sending {
            self.clock = clock_flags(self.lead);
        }
        if depth == 0 {
            self.surplus = 0;
            self.work.append(&mut self.buffer);
            if sending {
                if self.dry == 0 {
                    tracing::debug!(
                        target: "lucy::input",
                        frame = ?self.admitted.map(|frame| frame.wrapping_add(1)),
                        "input buffer dry: the body waits for the client's next command"
                    );
                }
                let waited = self.dry.max(MAX_WAIT);
                self.dry = self.dry.saturating_add(ticks);
                self.repeat(self.dry.saturating_sub(waited));
            }
            return;
        }
        if self.dry != 0 {
            tracing::debug!(
                target: "lucy::input",
                ticks = self.dry,
                depth,
                "input buffer refilled after dry ticks"
            );
            self.dry = 0;
        }
        if depth <= BUFFER_TARGET && arrived && ticks == 1 {
            tracing::debug!(
                target: "lucy::input",
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
                target: "lucy::input",
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

    /// A dry buffer past the wait: the next `count` frames, each the
    /// client's newest input held ([`held`], however many frames behind it
    /// is: a client behind the frames run still sends), or past
    /// [`MAX_REPEATS`] since his last datagram a neutral frame.
    fn repeat(&mut self, count: u32) {
        let Some(last) = self.last else {
            return;
        };
        for _ in 0..count {
            let Some(frame) = self
                .admitted
                .map(|frame| frame.wrapping_add(1))
                .filter(|&frame| frame <= MAX_LEASE_FRAME)
            else {
                return;
            };
            let command = if self.repeats < MAX_REPEATS {
                self.repeats += 1;
                let command = self.take_late(held(last, frame));
                tracing::debug!(
                    target: "lucy::input",
                    frame,
                    buttons = format_args!("{:#x}", command.buttons),
                    "input buffer dry: the last input repeated"
                );
                command
            } else {
                tracing::debug!(
                    target: "lucy::input",
                    frame,
                    "input buffer dry: a neutral frame"
                );
                self.take_late(neutral(last, frame))
            };
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
        self.dry = 0;
        self.surplus = 0;
        self.arrived = false;
        self.late = LateInput::default();
        self.clock = 0;
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
    /// A dummy bot (`STU_FC3921B1`, the Practice Range's Training Partner):
    /// the clients get his card without a "joined" or "left" line.
    pub dummy: bool,
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
            .map(|(slot, player)| PlayerView {
                dummy: player.dummy,
                ..match player.world.as_deref() {
                    Some(world) => {
                        world.player_view(&player.appearance, scene, |body| observed(slot, body))
                    }
                    None => PlayerView::absent(&player.appearance),
                }
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
            if let Err(error) = world.send_hero_select(scene, tick) {
                eprintln!("hero select frame for {:08X} failed ({error})", world.owner);
            }
            world.send_controller_ack(tick, now);
            cards
        })
        .collect()
}

/// What a controller frame shows of hero select: open, the pick, the skin
/// selector and the hero he plays ([`WorldDelivery::send_hero_select`]).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct SelectShown {
    open: bool,
    pick: Option<u64>,
    skins: bool,
    hero: Option<u64>,
}

pub struct WorldDelivery {
    owner: u32,
    queued: Vec<Arc<Record>>,
    /// The queued records in parts (`parts.rs`).
    parts: Parts,
    created: HashSet<u32>,
    bots: HashMap<u32, BotLifetime>,
    /// Each live bot's way onto this client's screen (`bot_shown`).
    bot_sights: HashMap<u32, BotSight>,
    health_updates: HashMap<u32, u64>,
    health_revision: u64,
    /// The target tag updates of the current body and the bots this client
    /// doesn't have yet, by entity: the revision of the newest.
    tag_updates: HashMap<u32, u64>,
    /// The revision of each live bot's target tags this client was given,
    /// in its create or an update ([`crate::bot_script::BotScripts::tags`]).
    bot_tags: HashMap<u32, u64>,
    started: Option<u32>,
    pub presentation_due: bool,
    controller: Stream,
    mode: ModeView,
    /// What the last controller frame showed of hero select; a change sends
    /// the next ([`Self::send_hero_select`]).
    select_shown: Option<SelectShown>,
    /// What the last controller frame carried, without a hero switch
    /// notice; a change of the script's projection sends the next
    /// ([`Self::controller_changed`]).
    controller_sent: Option<ProjectedSnapshot>,
    /// The controller snapshot's revision [`Self::controller_changed`] last
    /// projected.
    controller_seen: Option<Revision>,
    /// Hero select stays shown open after a pick while the heroes assemble
    /// (match.py `game_message`), though `288B` closes it (module notes of
    /// `crate::controller`): until the client closes it or the countdown
    /// ends.
    keep_open: bool,
    /// The messages the mode graph holds to every entity now
    /// ([`Self::set_mode_held`]): where it holds `06FD` a new body takes
    /// the respawn seconds from it.
    mode_held: Vec<u64>,
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
    /// The player's own copies of his server projectiles (Biotic Grenade).
    server_copies: ServerCopies,
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
    /// The server placeables' statescript on this client.
    placed_views: PlacedViews,
    /// The server placeables a mode graph removed
    /// ([`PreparedScene::remove_placeables`]): their creates not sent yet
    /// go no more.
    removed_placed: HashSet<u32>,
    /// The entities the mode entity's graphs made that this client is
    /// given (their create queued), and those whose destroy hasn't arrived.
    mode_entities: HashSet<u32>,
    mode_entity_destroys: HashSet<u32>,
    /// The entities the placeables' graphs made that this client is given
    /// (a health pack's visual), as [`Self::mode_entities`].
    placed_made: HashSet<u32>,
    /// The bodies the target searches can find, as the match last placed
    /// them; a new body starts with them.
    target_candidates: Vec<TargetCandidate>,
    /// The pickups the placeables held as the match last looked; a new
    /// body starts with them.
    pickups: Vec<Pickup>,
    /// The map placeables' entity variables now ([`Self::set_map_variables`]).
    map_variables: Arc<[(u32, Variables)]>,
    /// The current body's death and respawn (combat.py `Vitals.died_at`).
    death: Option<BodyDeath>,
    /// Who last hurt which of the player's bodies, and when: the player
    /// sources of damage, for the eliminations a death credits
    /// ([`Self::recent_attackers`]).
    attackers: Vec<Attack>,
    /// The last knock of the player's body, for the environment kill it
    /// credits ([`Self::knocked_by`]).
    knocks: KnockCredit,
    /// The pools of a body that runs no script (combat.py `Vitals` from
    /// `pools.body_pools`).
    plain_vitals: Option<Vitals>,
    /// Changes of the current body's pools, for the other clients.
    body_health: u64,
    /// The current body's target tags (component 76) with their revision,
    /// once its states changed them: its own client and the others get
    /// them (`ALLIED HEALTH BARS`: `0D24` reads the viewer's tags).
    body_tags: Option<(BodyId, u64, WorldComponent)>,
    tag_revision: u64,
    board: BoardDelivery,
    /// The PvP "assemble your team" countdown runs (match.py `assembling`):
    /// a pick keeps hero select open, which shows the skin selector.
    assembling: bool,
    /// The mode's own graph drives the match (`MatchFlow::follows_graph`):
    /// hero select and its skin selector are the graphs' own.
    mode_graph: bool,
    /// An AI player's world (`crate::ai`): no client receives it, so it
    /// views nobody, and its records arrive as soon as they are ready
    /// ([`Self::deliver_locally`]).
    headless: bool,
    /// An AI player's AI entity and the difficulty identifier it plays at
    /// (`lucy_script::ai_definitions`); none for a human's world. Each
    /// body he plays is possessed by the AI entity, made from that body's
    /// AI definition.
    ai: Option<(u32, u64)>,
    /// Whether the match is in overtime (game-rules.md 11): every body's
    /// mode script reads it as the mode entity's #3392e, and `0C8E` adds
    /// 3 s to a respawn for it.
    overtime: bool,
    /// The hero settings the player's client last sent (21604); every body
    /// he plays takes them. They are the client's, so a new world keeps them.
    hero_settings: HeroSettings,
    /// The voice of the current body (its entity): its stimuli's cooldowns
    /// and lines' turns ([`crate::voice_lines`]).
    voice: Option<(u32, VoiceState)>,
    /// Voice stimuli the body or the controller asked to say, waiting for
    /// their delay: the world tick each is due, and the body that says it.
    pending_voices: Vec<(u32, u32, VoiceRequest)>,
    /// Whether the living body stood in one of its team's spawn rooms when
    /// last measured ([`Self::area_step`]); `None` when it is dead or gone,
    /// or its team has no room on the map.
    spawn_room: Option<bool>,
    /// The spawn room flag (component 74 field 6) as last queued, and its
    /// revision; the player's create sends it set.
    spawn_flag: (bool, u64),
    /// Whether the current body's filter word (component 26) as last
    /// queued carries [`SHIELDED`], and its revision; a new body's create
    /// sends it without.
    shield: (bool, u64),
    /// A hero picked while the body was dead: the respawn plays it.
    respawn_hero: Option<(u64, SkinTheme)>,
    /// The hero the controller's graph forced on him (the Workshop's Start
    /// Forcing Player To Be Hero, `ControllerScript::forced_hero`), until
    /// [`Self::change_to_forced_hero`] makes its body.
    forced_hero: Option<u64>,
    /// A body placed at a spawn point and its facing (1/65536 turns): the
    /// view turn (25008) waits for its possession to arrive
    /// ([`Self::take_spawn_view`]).
    spawn_view: Option<(BodyId, i32)>,
    /// The hurt notices (25000) of the hits the current body took, for the
    /// match to send ([`Self::take_hurt_notices`]).
    hurt: Vec<CombatNotice>,
    /// The player's controller graph (`0C90`) in the VM, from the world's
    /// start: hero select (`288B`) takes his picks and opens, its snapshot
    /// is his controller frames, and it counts his multikills
    /// (`crate::announcer`). Whether a pulse waits for its controller frame.
    controller_script: Option<ControllerScript>,
    /// The pose the current body's last graph teleport left it in (a
    /// Workshop Teleport, `STU_CFCE55DC` in the controller's scope), when
    /// the body keeps no correction history, until a frame carries it to
    /// the owner as a correction ([`Self::apply_body_moves`]).
    teleported: Option<Movement>,
    /// The scope instances the controller's scope states opened or closed
    /// on entities other than his current body, for the match
    /// ([`Self::take_scope_changes`]).
    controller_scopes: Vec<ScopeChange>,
    multikill_due: bool,
    /// The notices of the changes his body's modifiers made to the hits on
    /// it (`lucy_script::vm::MODIFIED`), and what the sources of his
    /// body's health changes dealt ([`Self::source_dealt`]), to go with its
    /// messages.
    modified: Vec<GameMessage>,
    /// The ends of the persistent sends his bodies' scripts held as they
    /// stopped ([`Self::take_message_ends`]).
    retired_sends: Vec<MessageEnd>,
    /// Where his bodies come into the world instead of a spawn point, and
    /// their facing (1/65536 turns): a dummy bot's ([`Self::place_bodies_at`]).
    placement: Option<([f64; 3], i32)>,
}

/// The entity variables of a body's hero settings
/// (`lucy_content::hero_settings`): the setting identifier's index holds
/// the client's value, else the default. That covers the every-hero settings
/// the client sends (null at DEFAULT, which it sends as no entry) and the
/// settings the body's graphs read on the server (the data's default when
/// the client sent none, as it sends only settings saved away from their
/// DEFAULT). A value is encoded as the client
/// keeps it (`0x7FF789B7F0F0` compares a saved entry with the default so): a
/// float as the bits of an f64, a bool as 0 or 1, an identifier as its GUID.
pub fn hero_setting_variables(settings: &HeroSettings, hero: u64) -> Variables {
    let sent = SENT_SETTINGS
        .iter()
        .map(|&setting| (setting, SettingDefault::Identifier(None)));
    let read = script_settings(hero).map(|setting| (setting.setting, setting.default));
    sent.chain(read)
        .map(|(setting, default)| {
            let value = match (default, settings.value(hero, setting)) {
                (SettingDefault::Bool(_), Some(value)) => ScriptValue::Bool(value != 0),
                (SettingDefault::Bool(value), None) => ScriptValue::Bool(value),
                (SettingDefault::Float(_), Some(bits)) => ScriptValue::Float(f64::from_bits(bits)),
                (SettingDefault::Float(value), None) => ScriptValue::Float(f64::from(value)),
                (SettingDefault::Identifier(_), Some(0))
                | (SettingDefault::Identifier(None), None) => ScriptValue::Null,
                (SettingDefault::Identifier(_), Some(value))
                | (SettingDefault::Identifier(Some(value)), None) => ScriptValue::Asset(value),
            };
            (setting as u32, value)
        })
        .collect()
}

/// A body's death (combat.py `Vitals.death_tick`, `killer`) and when it
/// respawns.
///
/// A body whose mode body script counts the respawn (`0C8E`,
/// [`OwnerBodyRuntime::counts_respawn`]) respawns when that script sends
/// [`RESPAWN_READY`] to the mode. Stand-in (loose ends, "Respawn without
/// `0C8E`"): a body without such a script (Deathmatch, Team Deathmatch,
/// Bounty Hunter and the tutorial, whose body scripts send no `0685`, or a
/// body whose script failed) respawns the mode's `06FD` seconds after its
/// death ([`PreparedScene::respawn_time`]).
#[derive(Clone, Copy, Debug)]
struct BodyDeath {
    body: BodyId,
    killer: u32,
    tick: u32,
    died_at: Duration,
    /// When the respawn is due: from the script's `0685` on, or the next
    /// try while every body id is still reserved; none: not yet.
    due: Option<Duration>,
    /// Where a revival brings the player back (his graphs revived the
    /// body: the Workshop's Resurrect); none: the respawn's place.
    revive_at: Option<[f64; 3]>,
}

/// How a hit on the player's body ended (combat.py `_damage`): the damage
/// dealt, whether it killed the body, and the hit's tags as the body's
/// damage modifiers left them, for its messages
/// ([`WorldDelivery::send_hit_messages`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BodyDamage {
    pub dealt: f64,
    pub killed: bool,
    pub tags: DamageTags,
}

/// A hit that hurt one of the player's bodies ([`WorldDelivery::attackers`]).
#[derive(Clone, Copy, Debug)]
struct Attack {
    body: u32,
    source: u32,
    at: Duration,
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
            parts: Parts::default(),
            created: HashSet::new(),
            bots: HashMap::new(),
            bot_sights: HashMap::new(),
            health_updates: HashMap::new(),
            health_revision: 0,
            tag_updates: HashMap::new(),
            bot_tags: HashMap::new(),
            started: None,
            presentation_due: false,
            controller: Stream::new(owner),
            mode: ModeView::new(),
            select_shown: None,
            controller_sent: None,
            controller_seen: None,
            keep_open: false,
            mode_held: Vec::new(),
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
            server_copies: ServerCopies::default(),
            shots: Vec::new(),
            others: Others::default(),
            switch: None,
            switches: 0,
            notices: SwitchNotices::default(),
            retained_elsewhere: HashSet::new(),
            bot_views: BotViews::default(),
            placed_views: PlacedViews::default(),
            removed_placed: HashSet::new(),
            mode_entities: HashSet::new(),
            mode_entity_destroys: HashSet::new(),
            placed_made: HashSet::new(),
            target_candidates: Vec::new(),
            pickups: Vec::new(),
            map_variables: Arc::new([]),
            death: None,
            attackers: Vec::new(),
            knocks: KnockCredit::default(),
            plain_vitals: None,
            body_health: 0,
            body_tags: None,
            tag_revision: 0,
            board: BoardDelivery::default(),
            assembling: false,
            mode_graph: false,
            headless: false,
            ai: None,
            overtime: false,
            hero_settings: HeroSettings::default(),
            voice: None,
            pending_voices: Vec::new(),
            spawn_room: None,
            spawn_flag: (true, 0),
            shield: (false, 0),
            respawn_hero: None,
            forced_hero: None,
            spawn_view: None,
            hurt: Vec::new(),
            controller_script: None,
            teleported: None,
            controller_scopes: Vec::new(),
            multikill_due: false,
            modified: Vec::new(),
            retired_sends: Vec::new(),
            placement: None,
        }
    }

    /// The world of an AI player, whose body the server plays without a
    /// client (`crate::ai`): his AI entity `ai_entity` possesses each body
    /// he plays, at `difficulty` (a difficulty identifier,
    /// `lucy_script::ai_definitions::EASY` and up).
    pub fn headless(owner: u32, ai_entity: u32, difficulty: u64) -> Self {
        let mut world = Self {
            headless: true,
            ai: Some((ai_entity, difficulty)),
            ..Self::new(owner)
        };
        world.input.immediate = true;
        world
    }

    /// The AI that possesses this player's body of `hero`: none for a
    /// human's.
    fn ai_control(&self, graphs: &GraphStore, hero: u64) -> Option<AiControl> {
        let (entity, difficulty) = self.ai?;
        Some(AiControl {
            entity,
            definition: graphs.ai_definitions().definition(hero)?,
            difficulty,
        })
    }

    /// Whether this is an AI player's world.
    pub fn is_headless(&self) -> bool {
        self.headless
    }

    /// His bodies come into the world at `position` facing `yaw` (1/65536
    /// turns) rather than at his team's spawn point: a dummy bot stands
    /// where the graph that made him put him (`STU_FC3921B1`).
    pub fn place_bodies_at(&mut self, position: [f64; 3], yaw: i32) {
        self.placement = Some((position, yaw));
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
        self.parts.clear();
        self.created.clear();
        self.bots.clear();
        self.bot_sights.clear();
        self.health_updates.clear();
        self.tag_updates.clear();
        self.bot_tags.clear();
        self.started = None;
        self.presentation_due = false;
        self.mover = None;
        self.teleported = None;
        self.body_script = None;
        self.retired_sends.clear();
        self.script_failed = false;
        self.projectile_destroys.clear();
        self.projectile_events.clear();
        self.fields = FieldCopies::default();
        self.server_copies = ServerCopies::default();
        self.input.clear();
        self.shots.clear();
        self.death = None;
        self.plain_vitals = None;
        self.board = BoardDelivery::default();
        self.others.clear();
        self.bot_views.clear();
        self.placed_views.clear();
        self.removed_placed.clear();
        self.mode_entities.clear();
        self.mode_entity_destroys.clear();
        self.placed_made.clear();
        self.target_candidates.clear();
        self.pickups.clear();
        self.notices = SwitchNotices::default();
        self.spawn_room = None;
        self.spawn_view = None;
        self.hurt.clear();
        self.controller_script = None;
        self.select_shown = None;
        self.controller_sent = None;
        self.controller_seen = None;
        self.keep_open = false;
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
                let script = self
                    .body_script
                    .as_ref()
                    .filter(|script| script.id == body.id);
                let runtime = script.map(|script| &script.runtime);
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
                    tags: self
                        .body_tags
                        .as_ref()
                        .filter(|(id, ..)| *id == body.id)
                        .map(|(_, revision, tags)| (*revision, tags)),
                    death,
                    script: script.filter(|_| observed(body.id)).and_then(|script| {
                        let runtime = &script.runtime;
                        Some(ObservedScript::new(
                            runtime.encoder(),
                            runtime.snapshot()?,
                            || runtime.observer_activity(),
                            runtime.frame_ms(),
                            &script.observers,
                        ))
                    }),
                })
            });
        view
    }

    /// Queue what [`Others::show`] made for this client; its cards go to the
    /// reliable channel.
    fn apply_shown(&mut self, output: ShowOutput) -> Vec<Message> {
        for (update, delivery) in output.records {
            self.push(Record::new(update, Delivery::Other(delivery)));
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
        if self.notices.chunk == Some(self.controller.latest_data()) {
            self.queue_controller(scene, tick, None)?;
        } else {
            let switch = self.notices.waiting.pop_front().expect("waiting switch");
            self.queue_controller(scene, tick, Some(switch.kept))?;
            self.notices.chunk = Some(self.controller.latest_data());
        }
        self.notices.sent_at = Some(now);
        Ok(())
    }

    /// The player eliminated `victim` (of body definition `victim_body`)
    /// with a hit tagged `tags` at world tick `tick`: `0093.025` to his
    /// body, whose `01CA` (rid221) says his elimination line 1.2 s later
    /// ([`Self::take_voice_lines`]), and to his controller's `0215`
    /// ([`crate::announcer`]), unless `0215` rid15 leaves the victim out
    /// (his own body, or one tagged `06AD`/`031D`:
    /// [`crate::announcer::counted_victim`]). A pulse of its st21 sends a
    /// controller frame. A script fault stops the script; the controller's
    /// multikills stop with it.
    ///
    /// The body's copy carries `0015` the victim, `0012` the killer (his
    /// body), `0091` `final_blow` (off for an elimination credited for
    /// damage before another player's killing blow,
    /// [`crate::combat::kill_credit`]: `01CA` rid221 then says no line),
    /// and the killing hit's tags as `003D` and `0355` (`01CA` tests `0355`
    /// for the melee tag `0302`; assumption: both name the killing hit's
    /// tags). The body learns the victim's authored target tags first:
    /// `01CA` rid210 skips victims tagged `047F`, `003A` or `02CB` (on no
    /// hero or Training Bot body).
    pub fn eliminated(
        &mut self,
        graphs: &GraphStore,
        tick: u32,
        victim: u32,
        victim_body: Option<u64>,
        tags: &DamageTags,
        final_blow: bool,
    ) {
        let killer = self.body.map_or(self.owner, |body| body.id.entity);
        if victim == killer {
            return;
        }
        let failure = self
            .body_script
            .as_mut()
            .filter(|script| script.id.entity == killer)
            .and_then(|script| {
                let victim_tags = victim_body
                    .and_then(|body| graphs.bodies().get(&body))
                    .and_then(|body| body.target_tags.clone())
                    .unwrap_or_default();
                script.runtime.set_victim_tags(victim, victim_tags);
                let parameters = Variables::from([
                    (0x15, ScriptValue::Entity(victim)),
                    (0x12, ScriptValue::Entity(killer)),
                    (0x91, ScriptValue::Bool(final_blow)),
                    (0x3d, tags.to_value()),
                    (0x355, tags.to_value()),
                ]);
                script
                    .runtime
                    .receive_message(0, lucy_script::controller::ELIMINATION, parameters)
                    .err()
            });
        if let Some(error) = failure {
            self.drop_failed_script(&error.to_string());
        }
        if !crate::announcer::counted_victim(graphs, victim_body) {
            return;
        }
        let Some(script) = &mut self.controller_script else {
            return;
        };
        let before = script.multikill();
        match script.eliminated(tick, victim, killer, final_blow) {
            Ok(()) => self.multikill_due |= script.multikill() != before,
            Err(error) => {
                eprintln!("controller script of {:08X} stopped ({error})", self.owner);
                self.controller_script = None;
            }
        }
    }

    /// A multikill pulse goes out at once in a controller frame
    /// ([`crate::announcer`]).
    fn send_multikills(&mut self, scene: &PreparedScene, tick: u32) -> io::Result<()> {
        if !std::mem::take(&mut self.multikill_due)
            || self.headless
            || self.controller.latest_data() == 0
        {
            return Ok(());
        }
        self.queue_controller(scene, tick, None)
    }

    /// A controller frame once what it shows of hero select changed: the
    /// script's `288B` after a request or on its own (st95 closes the screen
    /// once the picked hero's body lives; st98 when the player may no longer
    /// pick), the hero he plays, the skin selector; or once anything else
    /// it carries changed ([`Self::controller_changed`]).
    fn send_hero_select(&mut self, scene: &PreparedScene, tick: u32) -> io::Result<()> {
        if self.headless || !self.can_handle_game_message() {
            return Ok(());
        }
        self.advance_controller(tick);
        let shown = self
            .select_input(scene.controller().practice())
            .map(|(shown, _)| shown);
        if shown.is_some() && (shown != self.select_shown || self.controller_changed(scene)) {
            self.queue_controller(scene, tick, None)?;
        }
        Ok(())
    }

    /// Whether the controller script's projection differs from the last
    /// controller frame's. The client restores its own controller to that
    /// frame at an owner rollback without running the controller's switches
    /// again, so a state the server's script began since (the Training
    /// Partner console's scope 126 st13 → st33, which hides the HUD while the
    /// picker is open) has to reach it: until then each rollback ended the
    /// client's own st33. Projected only when the snapshot's revision moved.
    fn controller_changed(&mut self, scene: &PreparedScene) -> bool {
        let Some(sent) = &self.controller_sent else {
            return false;
        };
        let Some((script, revision)) = self
            .controller_script
            .as_ref()
            .and_then(|script| Some((script, script.snapshot()?.revision())))
        else {
            return false;
        };
        if self.controller_seen == Some(revision) {
            return false;
        }
        self.controller_seen = Some(revision);
        let Some((_, input)) = self.select_input(scene.controller().practice()) else {
            return false;
        };
        scene
            .controller()
            .controller_projection(script, input)
            .is_ok_and(|projected| projected != *sent)
    }

    /// An owner chunk with only the command frame the server has of the
    /// player (`Stream::owner_ack`), every 100 ms once the controller's data
    /// has arrived, body or not. Without one the client's controller host
    /// keeps each prediction it journals for 40 frames, then rolls back and
    /// replays by itself (`0x7FF78991B200`); that rollback undid the begin
    /// of the client-made hero preview `2906`'s st2, so a preview made in
    /// hero select before the first body never went away
    /// (`refs/reports/ControllerWorld.md`, "the auto-picked preview").
    fn send_controller_ack(&mut self, tick: u32, now: Duration) {
        if self.headless || self.controller_script.is_none() {
            return;
        }
        let newest = self.command_ack(tick);
        match self.controller.owner_ack(now, Some(newest)) {
            Ok(Some(chunk)) => self.push(Record::script(chunk, false)),
            Ok(None) => {}
            Err(error) => eprintln!("controller ack of {:08X} not sent ({error})", self.owner),
        }
    }

    /// The controller script's world facts the server keeps (component 74
    /// field 6, the body's dead flag, the body's variables its graphs
    /// read), then its clock at `tick`. A fault stops it.
    fn advance_controller(&mut self, tick: u32) {
        let (may_pick, dead) = (self.may_change_hero(), self.death.is_some());
        let Some(script) = &mut self.controller_script else {
            return;
        };
        script.set_may_pick(may_pick);
        script.set_body_dead(dead);
        let body = self
            .body_script
            .as_ref()
            .filter(|body| self.body.is_some_and(|current| current.id == body.id));
        let synced = script.sync_body_variables(|id| {
            body.map_or(ScriptValue::Null, |body| body.runtime.entity_variable(id))
        });
        if let Err(error) = synced.and_then(|()| script.advance(tick).map(drop)) {
            eprintln!("controller script of {:08X} stopped ({error})", self.owner);
            self.controller_script = None;
        }
        self.after_controller();
    }

    /// What the controller's graphs did outside the controller, after each
    /// run: the scopes they opened or closed ([`Self::apply_controller_scopes`]),
    /// the body moves ([`Self::apply_body_moves`]) and the body's
    /// cooldowns ([`Self::apply_cooldowns`]).
    fn after_controller(&mut self) {
        self.apply_controller_scopes();
        self.apply_body_moves();
        self.apply_cooldowns();
    }

    /// The controller's graphs set cooldowns of the current body's
    /// abilities (a Workshop Set Ability Cooldown's scope, `STU_FFF26F80`):
    /// its script writes them at once, so the abilities and the owner's
    /// next frame have them.
    fn apply_cooldowns(&mut self) {
        let Some(script) = &mut self.controller_script else {
            return;
        };
        let cooldowns = script.take_cooldowns();
        for cooldown in cooldowns {
            if self.current_body() != Some(cooldown.body) {
                continue;
            }
            let written = self.body_script.as_mut().map(|script| {
                script
                    .runtime
                    .set_ability_cooldown(cooldown.button, cooldown.value)
            });
            if let Some(Err(error)) = written {
                self.drop_failed_script(&error.to_string());
            }
        }
    }

    /// The controller's scope states opened or closed scope instances: on
    /// his current body at once (the Workshop's Set … Enabled stacks
    /// `#9535e`, `#9538e`, `#9743e` … there, `1D42`/`1D43`/`1F30`), the
    /// opened instance going back to its scope state for its frame record;
    /// on another entity through the match ([`Self::take_scope_changes`]).
    /// A body whose script hasn't started yet takes none: the scope state
    /// asks again each tick until one opens.
    fn apply_controller_scopes(&mut self) {
        let Some(script) = &mut self.controller_script else {
            return;
        };
        let changes = script.take_scope_changes();
        for change in changes {
            let target = change.target();
            if self.current_body() != Some(target) {
                self.controller_scopes.push(change);
                continue;
            }
            if !self.takes_body_message(target) {
                continue;
            }
            let opened = self
                .body_script
                .as_mut()
                .map(|script| script.runtime.change_scope(&change));
            let instance = match opened {
                Some(Ok(Some(instance))) => instance,
                Some(Err(error)) => {
                    self.drop_failed_script(&error.to_string());
                    continue;
                }
                _ => continue,
            };
            if let (ScopeChange::Open { source, .. }, Some(script)) =
                (&change, &mut self.controller_script)
            {
                script.scope_opened(*source, target, instance);
            }
        }
    }

    /// The controller's graphs put the current body somewhere (a Workshop
    /// Teleport's scope, `STU_CFCE55DC`): the mover takes it now, facing
    /// the graph's direction when it gave one, and corrects the owner's
    /// client with it; its raised discontinuity counter makes the client
    /// take it without replaying its commands
    /// (`lucy_physics::mover::correction`). A body that keeps a
    /// correction history sends the poses of the frames after it
    /// ([`CorrectionHistory::discontinue`]); another sends this pose
    /// ([`Self::send_frame`]). Either waits for the body's create and for
    /// a packet of the pose's frame: the mover runs the client's commands,
    /// whose frames lead the server's. Retail's owner correction for it is
    /// Workshop.md's inference.
    fn apply_body_moves(&mut self) {
        let Some(script) = &mut self.controller_script else {
            return;
        };
        let moves = script.take_body_moves();
        let body = self.body.map(|body| body.id.entity);
        for body_move in moves
            .into_iter()
            .filter(|body_move| Some(body_move.body) == body)
        {
            let yaw = body_move
                .facing
                .map(|[x, _, z]| (x.atan2(z) * 65_536.0 / std::f64::consts::TAU).round() as i32);
            self.teleport_current(body_move.position, yaw);
        }
    }

    /// The current body's mover goes to `position` (facing `yaw` when
    /// given) and the owner's client is corrected to it
    /// ([`Self::apply_body_moves`]).
    fn teleport_current(&mut self, position: [f64; 3], yaw: Option<i32>) {
        let Some(mover) = self.mover.as_mut() else {
            return;
        };
        mover.teleport(position, yaw);
        let pose = mover.pose();
        let history = self.body_script.as_mut().and_then(|script| {
            let tail = RECORD_TAIL_MS.div_ceil(script.runtime.frame_ms());
            script.corrections.as_mut().map(|history| (history, tail))
        });
        match history {
            Some((history, tail)) => {
                history.discontinue(pose.frame.map_or(0, |frame| frame + 1), tail);
            }
            None => self.teleported = Some(pose),
        }
    }

    /// The client's hero select request to the controller script's `288B`
    /// at `tick`; its state after, none without a script.
    fn select_request(&mut self, tick: u32, request: SelectRequest) -> Option<HeroSelectState> {
        self.advance_controller(tick);
        let script = self.controller_script.as_mut()?;
        match script.hero_select(tick, request) {
            Ok(state) => state,
            Err(error) => {
                eprintln!("controller script of {:08X} stopped ({error})", self.owner);
                self.controller_script = None;
                None
            }
        }
    }

    /// What a controller frame shows of hero select now, and the frame's
    /// input besides the script (`crate::controller` module notes): shown
    /// open while `288B` has it open or [`Self::keep_open`]; the skin
    /// selector (`288B`'s #25377) while open as heroes assemble; the
    /// assemble title hidden in the Practice Range only. Not in the
    /// Practice Range: its mode graph `13C0` holds no `0692`, so `288B`
    /// st35 never sets #25377 there, and the client's st38 says Athena's
    /// hero select line (`0918` → `03F2` VoiceStimulus `0168.078`) each
    /// time #25377 turns on. Where the mode graph drives the match
    /// ([`Self::set_mode_graph`]) the skin selector is `288B`'s own: its
    /// st32-st35 set #25377 while the graph holds `0692` (`0F4A` st9).
    fn select_input(&self, practice: bool) -> Option<(SelectShown, ControllerFrame)> {
        let state = self.controller_script.as_ref()?.hero_select_state()?;
        let open = state.open || self.keep_open;
        let graph = self.mode_graph && !practice;
        let skins = !graph && open && self.assembling;
        let hero = self.body.map(|body| body.hero);
        Some((
            SelectShown {
                open,
                pick: state.pick,
                skins,
                hero,
            },
            ControllerFrame {
                keep_open: self.keep_open,
                skins: (!graph).then_some(skins),
                hide_assemble: practice,
                notice: None,
                hero,
            },
        ))
    }

    /// This player's controller and team, as another's controller script
    /// knows it; none without a script.
    pub fn controller_team(&self) -> Option<(u32, i64)> {
        let script = self.controller_script.as_ref()?;
        Some((script.entity(), script.team()?))
    }

    /// A message to the player's controller (or to every entity) at `tick`
    /// (`players`: the room's controllers and teams): his controller
    /// script takes it.
    fn receive_controller_message(
        &mut self,
        tick: u32,
        players: &[(u32, i64)],
        message: &GameMessage,
    ) {
        self.advance_controller(tick);
        let owner = self.owner;
        let Some(script) = &mut self.controller_script else {
            return;
        };
        script.set_players(
            players
                .iter()
                .copied()
                .filter(|&(controller, _)| controller != owner)
                .collect(),
        );
        if let Err(error) = script.receive_from(tick, message) {
            eprintln!("controller script of {owner:08X} stopped ({error})");
            self.controller_script = None;
            return;
        }
        if let Some(hero) = script.forced_hero() {
            self.forced_hero = Some(hero);
        }
    }

    /// Queues `record`: whole, or in parts when its chunk is too big for a
    /// datagram (`parts.rs`).
    fn push(&mut self, record: Record) {
        let entity = record.update.entity;
        match self.parts.split(record) {
            Ok(records) => self.queued.extend(records),
            Err(error) => eprintln!("a record of {entity:08X} not sent ({error})"),
        }
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
        self.bot_sights.retain(|&entity, sight| match *sight {
            BotSight::Sent(sequence) => match delivery(sequence, ack, ack_bits) {
                Some(true) => {
                    tracing::debug!(
                        target: "lucy::join",
                        entity = format_args!("{entity:08X}"),
                        sequence,
                        "bot's first movement record arrived"
                    );
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
        self.mode.arrived()
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
        self.push(Record::new(update, Delivery::Health { revision }));
    }

    /// The current body's or a live bot's target tags at `revision` for
    /// this client, superseding the ones not yet delivered; they wait for
    /// the entity's create.
    fn queue_tags(&mut self, entity: u32, revision: u64, tags: WorldComponent) {
        let live_bot = self.bots.get(&entity) == Some(&BotLifetime::Live);
        let live_body = self.body.is_some_and(|body| {
            body.id.entity == entity
                && self.bodies.get(&entity) == Some(&(body.id.generation, Lifetime::Live))
        });
        if !live_bot && !live_body {
            return;
        }
        self.tag_updates.insert(entity, revision);
        self.remove_stale();
        self.push(Record::new(
            EntityUpdate {
                entity,
                operation: Some(Operation::Update(vec![tags])),
                ..EntityUpdate::default()
            },
            Delivery::Tags { revision },
        ));
    }

    /// The server placeables' frames this client doesn't have yet, for the
    /// placeables whose create arrived (placed_script.rs). A removed
    /// placeable (a mode graph's `STU_A3DC6A25`, or a health pack's own
    /// `STU_523EE38D`) gets no more frames, and its create no more sends;
    /// a client that has it gets its destroy (the client runs no
    /// `STU_523EE38D`). Then the entities the placeables' graphs made (a
    /// health pack's visual), as the mode entity's ([`Self::update_made`]).
    pub fn update_placed(&mut self, scene: &PreparedScene) {
        if self.started.is_none() {
            return;
        }
        let mut removed = false;
        for &entity in scene.removed_placeables() {
            if self.removed_placed.insert(entity) {
                self.placed_views.forget(entity);
                removed = true;
                if self.created.remove(&entity) {
                    self.mode_entity_destroys.insert(entity);
                    self.push(Record::new(
                        EntityUpdate {
                            entity,
                            operation: Some(Operation::Destroy),
                            ..EntityUpdate::default()
                        },
                        Delivery::ModeEntity { destroy: true },
                    ));
                }
            }
        }
        if removed {
            self.remove_stale();
        }
        let created = |entity| self.created.contains(&entity);
        let chunks = match self
            .placed_views
            .update(scene.placed_scripts().frames(), created)
        {
            Ok(chunks) => chunks,
            Err(error) => {
                eprintln!("a placed entity's frame failed ({error})");
                return;
            }
        };
        for chunk in chunks {
            self.push(Record::new(
                chunk.update,
                Delivery::Placed { last: chunk.last },
            ));
        }
        let made: Vec<MadeView<'_>> = scene.placed_scripts().made().collect();
        self.update_made(MadeBy::Placeables, &made);
    }

    /// The entities the mode entity's graphs made (the Workshop's effects
    /// and icons, [`ModeRunner::created`]) on this client, once its world
    /// started ([`Self::update_made`]).
    pub fn update_mode_entities(&mut self, mode: &ModeRunner) {
        let _replicate = lucy_core::phase::enter(lucy_core::phase::Phase::Replicate);
        if !self.world_started() {
            return;
        }
        let made: Vec<MadeView<'_>> = mode
            .created()
            .map(|made| MadeView {
                entity: made.entity.entity,
                definition: made.entity.definition,
                position: made.entity.position,
                frame: made.frame(),
            })
            .collect();
        self.update_made(MadeBy::Mode, &made);
    }

    /// The entities a graph made, as they are now: a create for each new
    /// one (its definition where the graph put it), its frames once the
    /// create arrived and whenever they change (as a server placeable's,
    /// on the entity's own stream), a destroy for each that went.
    fn update_made(&mut self, by: MadeBy, made: &[MadeView<'_>]) {
        let known = match by {
            MadeBy::Mode => &self.mode_entities,
            MadeBy::Placeables => &self.placed_made,
        };
        let gone: Vec<u32> = known
            .iter()
            .copied()
            .filter(|&entity| !made.iter().any(|made| made.entity == entity))
            .collect();
        for entity in gone {
            match by {
                MadeBy::Mode => self.mode_entities.remove(&entity),
                MadeBy::Placeables => self.placed_made.remove(&entity),
            };
            self.created.remove(&entity);
            self.placed_views.forget(entity);
            self.mode_entity_destroys.insert(entity);
            self.remove_stale();
            self.push(Record::new(
                EntityUpdate {
                    entity,
                    operation: Some(Operation::Destroy),
                    ..EntityUpdate::default()
                },
                Delivery::ModeEntity { destroy: true },
            ));
        }
        for made in made {
            let new = match by {
                MadeBy::Mode => self.mode_entities.insert(made.entity),
                MadeBy::Placeables => self.placed_made.insert(made.entity),
            };
            if !new {
                continue;
            }
            self.mode_entity_destroys.remove(&made.entity);
            let mut create = Create::new(made.definition, Vec::new());
            create.position = made.position.map(|part| part as f32);
            self.push(Record::new(
                EntityUpdate {
                    entity: made.entity,
                    operation: Some(Operation::Create(Box::new(create))),
                    ..EntityUpdate::default()
                },
                Delivery::ModeEntity { destroy: false },
            ));
        }
        let created = |entity| self.created.contains(&entity);
        let frames = made
            .iter()
            .filter_map(|made| Some((made.entity, made.frame?)));
        let chunks = match self.placed_views.update(frames, created) {
            Ok(chunks) => chunks,
            Err(error) => {
                eprintln!("a made entity's frame failed ({error})");
                return;
            }
        };
        for chunk in chunks {
            self.push(Record::new(
                chunk.update,
                Delivery::Placed { last: chunk.last },
            ));
        }
    }

    /// The training bots' statescript on this client this tick: new bot
    /// lifetimes' first frames, the room's live and death frames, what this
    /// client's ACKs made due and the practice nameplates
    /// (server.py `queue_bot_live`, `queue_bot_death`, `_flush_bot_frames`,
    /// `_flush_bot_nameplates`); then each bot's target tags (component 76)
    /// that changed since this client was given them, as for a player's
    /// body.
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
            self.push(Record::new(
                chunk.update,
                Delivery::BotFrames { last: chunk.last },
            ));
        }
        for entity in live {
            let Some((revision, tags)) = scene.bot_scripts().tags(entity) else {
                continue;
            };
            if self.bot_tags.insert(entity, revision) != Some(revision) {
                self.queue_tags(entity, revision, tags.clone());
            }
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
        self.tag_updates.remove(&old);
        self.bot_tags.remove(&old);
        self.remove_stale();
        self.push(Record::new(
            EntityUpdate {
                entity: old,
                operation: Some(Operation::Destroy),
                ..EntityUpdate::default()
            },
            Delivery::BotDestroy,
        ));
        self.push(Record::new(
            replacement.created,
            Delivery::BotCreate { after: Some(old) },
        ));
        if let Some(link) = replacement.link {
            self.push(Record::new(link, Delivery::BotLink { body: new }));
        }
    }

    fn current(&self, record: &Record) -> bool {
        record.part.is_none_or(|part| self.parts.current(part)) && self.delivery_current(record)
    }

    /// Whether `record`'s delivery still has to reach the client, its
    /// parts aside.
    fn delivery_current(&self, record: &Record) -> bool {
        let entity = record.update.entity;
        match record.delivery {
            Delivery::Baseline => !self.removed_placed.contains(&entity),
            Delivery::Script { close: false, .. } => true,
            Delivery::BotCreate { .. } => self.bots.get(&entity) == Some(&BotLifetime::Live),
            Delivery::BotLink { body } => self.bots.get(&body) == Some(&BotLifetime::Live),
            Delivery::BotDestroy => self.bots.get(&entity) == Some(&BotLifetime::Retiring),
            Delivery::Health { revision } => self.health_updates.get(&entity) == Some(&revision),
            Delivery::Tags { revision } => self.tag_updates.get(&entity) == Some(&revision),
            Delivery::Board { revision } => self.board.revision == revision,
            Delivery::SpawnFlag { revision } => self.spawn_flag.1 == revision,
            Delivery::Filter { revision } => {
                self.shield.1 == revision && self.body.is_some_and(|body| body.id.entity == entity)
            }
            Delivery::Script {
                last, close: true, ..
            } => last == self.controller.latest_data(),
            Delivery::Mode { last, .. } => self.mode.current(last),
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
            Delivery::ServerSeen(seen) => self.server_copies.current(seen, entity),
            Delivery::Placed { last } => self.placed_views.current(entity, last),
            Delivery::ModeEntity { destroy: false } => {
                self.mode_entities.contains(&entity) || self.placed_made.contains(&entity)
            }
            Delivery::ModeEntity { destroy: true } => self.mode_entity_destroys.contains(&entity),
        }
    }

    pub fn acknowledge(&mut self, report: DeliveryReport<Arc<Record>>, now: Duration) {
        if self.started.is_none() {
            return;
        }
        let mut retry = Vec::new();
        let traced = tracing::enabled!(target: "lucy::join", tracing::Level::DEBUG);
        for packet in report.packets {
            for record in packet.records {
                if !self.current(&record) {
                    continue;
                }
                let (record, lost_part) = match record.part {
                    None => (record, None),
                    Some(part) => match self.parts.delivered(part, packet.arrived) {
                        PartOutcome::Waiting => continue,
                        PartOutcome::Arrived(first) => (first, None),
                        PartOutcome::Lost => (record, Some(part)),
                    },
                };
                if let Some(kind) = traced.then(|| self.join_kind(&record)).flatten() {
                    tracing::debug!(
                        target: "lucy::join",
                        sequence = packet.sequence,
                        arrived = packet.arrived,
                        entity = format_args!("{:08X}", record.update.entity),
                        kind,
                        "record acknowledged"
                    );
                }
                let retried = retry.len();
                match (packet.arrived, record.delivery) {
                    (arrived, Delivery::BodyScript { last, .. }) => {
                        let script = self.body_script.as_mut().expect("current body stream");
                        tracing::debug!(
                            target: "lucy::owner",
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
                    (arrived, Delivery::ServerSeen(seen)) => {
                        let entity = record.update.entity;
                        if self.server_copies.delivered(seen, entity, arrived) {
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
                    (arrived, Delivery::Mode { last }) => {
                        if self.mode.delivered(last, arrived) {
                            retry.push(record);
                        }
                    }
                    (false, Delivery::Script { resend: false, .. }) => {}
                    (false, _) => retry.push(record),
                    (true, Delivery::Baseline) => {
                        // A server placeable's create (`Operation::Placeable`)
                        // counts too: its statescript frames wait for it
                        // (`update_placed`).
                        if matches!(
                            record.update.operation,
                            Some(Operation::Create(_) | Operation::Placeable(..))
                        ) {
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
                        | Delivery::SpawnFlag { .. }
                        | Delivery::Filter { .. },
                    ) => {}
                    (true, Delivery::Health { .. }) => {
                        self.health_updates.remove(&record.update.entity);
                    }
                    (true, Delivery::Tags { .. }) => {
                        self.tag_updates.remove(&record.update.entity);
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
                            Ok(Some((update, field))) => {
                                self.push(Record::new(update, Delivery::Field(field)))
                            }
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
                    (true, Delivery::Placed { last }) => {
                        self.placed_views.arrived(record.update.entity, last);
                    }
                    (true, Delivery::ModeEntity { destroy: false }) => {
                        self.created.insert(record.update.entity);
                    }
                    (true, Delivery::ModeEntity { destroy: true }) => {
                        self.mode_entity_destroys.remove(&record.update.entity);
                    }
                    (true, Delivery::Script { last, .. }) => {
                        self.controller.arrived(last);
                    }
                }
                // A lost part that won't go again: nor will its record.
                if let Some(part) = lost_part.filter(|_| retry.len() == retried) {
                    self.parts.give_up(part);
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
        let mut parts = std::mem::take(&mut self.parts);
        parts.retain(|first| self.delivery_current(first));
        self.parts = parts;
    }

    /// The records a player's body, view and HUD depend on, named for the
    /// `lucy::join` trace.
    fn join_kind(&self, record: &Record) -> Option<&'static str> {
        Some(match record.delivery {
            Delivery::Baseline if matches!(record.update.operation, Some(Operation::Create(_))) => {
                "create"
            }
            Delivery::CreateBody(_) => "body create",
            Delivery::Possession(_) => "possession",
            Delivery::DestroyBody(_) => "body destroy",
            Delivery::BodyScript { delta: false, .. } => "owner full frame",
            Delivery::BotCreate { .. } => "bot create",
            Delivery::BotDestroy => "bot destroy",
            Delivery::Script { close: true, .. } => "controller frame (close)",
            Delivery::Script { .. } => "controller frame",
            Delivery::Mode { .. } => "mode frame",
            _ => return None,
        })
    }

    /// The queued records a datagram may carry now, in the order it takes
    /// them: queue order (a record follows its prerequisites), except the
    /// owner delta of the client's own body, which goes first once his
    /// possession arrived. It carries the command frame his prediction is
    /// checked at, and the volleys that frame introduced
    /// (`OwnerProjectiles::owner_sent`); a datagram over its budget drops
    /// its last records, and the next tick's delta replaces an unsent one at
    /// a later command frame. Queued last, it lost the datagram to the
    /// Range's once-a-second full mode frame (about 940 bytes): a volley
    /// shot at frame f was introduced at f + 1, so its ids of both frames
    /// were confirmed (Echo's Tri-Shot came as six bolts).
    pub(crate) fn ready(&self) -> Vec<Ready<'_>> {
        let body = self.body.map(|body| body.id);
        let mut created = body.is_none_or(|id| self.created.contains(&id.entity));
        let mut bound = body == self.bound;
        let mut destroyed = HashSet::new();
        let mut creating = HashSet::new();
        let holding = self.parts.holding(|first| self.delivery_current(first));
        let mut ready: Vec<_> = self
            .queued
            .iter()
            .enumerate()
            .filter_map(|(index, record)| {
                if record.part.is_some_and(|part| !self.parts.current(part))
                    || holding.holds(record)
                {
                    return None;
                }
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
                    Delivery::Health { .. } | Delivery::Tags { .. } | Delivery::Filter { .. }
                        if !self.identity_ready(record.update.entity) =>
                    {
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
                    // A bot's first frame goes in its create's datagram, after
                    // it (`BotViews::ready`).
                    Delivery::BotFrames { last }
                        if !self.bot_views.ready(
                            record.update.entity,
                            last,
                            creating.contains(&record.update.entity),
                        ) =>
                    {
                        return None;
                    }
                    Delivery::Other(other) if !self.others.ready(other) => return None,
                    Delivery::Mode { last, .. } if !self.mode.current(last) => return None,
                    _ => {}
                }
                Some(Ready { index, record })
            })
            .collect();
        let own = ready.iter().position(|item| {
            matches!(item.record.delivery,
                Delivery::BodyScript { id, delta: true, .. } if self.bound == Some(id))
        });
        if let Some(own) = own {
            ready[..=own].rotate_right(1);
        }
        ready
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
                Delivery::Mode { last, .. } => self.mode.sent(last),
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

    /// The queue indexes of the records a datagram carried, in queue order
    /// (as [`WorldDelivery::sent`] takes them; [`WorldDelivery::ready`] may
    /// put one first).
    pub(crate) fn sent_indices(ready: &[Ready<'_>], sent: &[RecordSource]) -> Vec<usize> {
        let mut indexes: Vec<_> = sent
            .iter()
            .filter_map(|source| match *source {
                RecordSource::Queued(index) => Some(ready[index].index),
                RecordSource::Movement(_) => None,
            })
            .collect();
        indexes.sort_unstable();
        indexes
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
        // A teleport's pose ([`Self::apply_body_moves`]) goes once its
        // body's create arrived, in a packet of its frame or later; a body
        // with correction history carries it in its own candidate.
        let teleport = if reliable_only {
            None
        } else {
            self.teleported
                .filter(|pose| pose.frame.is_none_or(|frame| frame <= tick))
                .zip(
                    self.body
                        .map(|body| body.id.entity)
                        .filter(|entity| self.created.contains(entity)),
                )
                .map(|(pose, entity)| EntityUpdate {
                    entity,
                    movement: Some(pose),
                    ..EntityUpdate::default()
                })
        };
        let script = self.body_script.as_ref();
        let history = script.and_then(|script| script.corrections.as_ref());
        let correction = if reliable_only {
            None
        } else {
            script.and_then(|script| {
                history?.candidate(tick, self.created.contains(&script.id.entity))
            })
        };
        let owner_index = correction
            .map(|correction| correction.update())
            .or(teleport)
            .map(|update| {
                let index = movements.len();
                movements.push(update);
                index
            });
        let queued = self.ready();
        let newest = self.command_ack(tick);
        let sent = egress.send_frame(
            FrameRecords {
                tick,
                clock: self.input.clock,
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
        let traced = tracing::enabled!(target: "lucy::join", tracing::Level::DEBUG);
        for &index in &indexes {
            let record = &self.queued[index];
            // The client takes this datagram's frame as the body's command
            // frame (`BodyStream::full_sent`).
            if let Delivery::BodyScript {
                id, delta: false, ..
            } = record.delivery
            {
                if let Some(script) = self.body_script.as_mut().filter(|script| script.id == id) {
                    script.stream.full_sent(tick);
                }
            }
            if let Some(kind) = traced.then(|| self.join_kind(record)).flatten() {
                tracing::debug!(
                    target: "lucy::join",
                    sequence = sent.sequence,
                    tick,
                    entity = format_args!("{:08X}", record.update.entity),
                    kind,
                    "record sent"
                );
            }
        }
        self.sent(&indexes);
        let carried =
            owner_index.is_some_and(|index| sent.records.contains(&RecordSource::Movement(index)));
        let history = self
            .body_script
            .as_mut()
            .and_then(|script| script.corrections.as_mut());
        match (correction, history) {
            (Some(correction), Some(history)) if carried => {
                history.sent(&correction);
            }
            (None, _) if carried => self.teleported = None,
            _ => {}
        }
        for source in &sent.records {
            let RecordSource::Movement(index) = *source else {
                continue;
            };
            let entity = movements[index].entity;
            if self.bots.get(&entity) == Some(&BotLifetime::Live)
                && !self.bot_sights.contains_key(&entity)
            {
                tracing::debug!(
                    target: "lucy::join",
                    entity = format_args!("{entity:08X}"),
                    sequence = sent.sequence,
                    tick,
                    "bot's first movement record sent"
                );
                self.bot_sights
                    .insert(entity, BotSight::Sent(sent.sequence));
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
        let tags = self.body_script.as_mut().and_then(|script| {
            let id = script.id;
            script
                .runtime
                .take_target_tags_update()
                .map(|tags| (id, tags))
        });
        if let Some((id, tags)) = tags {
            self.tag_revision += 1;
            self.queue_tags(id.entity, self.tag_revision, tags.clone());
            self.body_tags = Some((id, self.tag_revision, tags));
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
            // Its persistent sends go with it.
            self.retired_sends
                .extend(script.runtime.take_message_ends());
            self.retired_sends.extend(script.runtime.held_sends());
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
        // The bots' creates carry their target tags as they are now
        // ([`PreparedScene::initial_world`]).
        self.bot_tags.extend(scene.bots().iter().filter_map(|bot| {
            let (revision, _) = scene.bot_scripts().tags(bot.entity)?;
            Some((bot.entity, revision))
        }));
        for update in records {
            if self.bots.contains_key(&update.entity) {
                self.push(Record::new(update, Delivery::BotCreate { after: None }));
            } else {
                self.push(Record::baseline(update));
            }
        }
        self.started = Some(tick);
        // The player's controller graph runs from the world's start: hero
        // select, the multikills.
        self.controller_script = scene
            .controller()
            .start_script(
                scene.shared_graphs(),
                self.owner,
                viewer.team,
                scene.heroes(),
                tick,
            )
            .map_err(|error| {
                eprintln!(
                    "controller script of {:08X} can't start ({error})",
                    self.owner
                );
            })
            .ok();
        // A custom game's hero pool: `0C90` st65 takes `080D` {`0458`: the
        // heroes} into #10489, the list HeroSelect allows (`m_8B103ADE`;
        // the client refuses a hero outside it, `sub_7FF789CB0C70`), as
        // the Workshop's Set Player Allowed Heroes and `0F2F` send it.
        let pool = &scene.map().hero_pool;
        if !pool.is_empty() {
            let message = GameMessage {
                sender: GAME_MODE_ENTITY,
                target: self
                    .controller_script
                    .as_ref()
                    .map_or(0, ControllerScript::entity),
                message: ALLOWED_HEROES,
                parameters: Variables::from([(
                    ALLOWED_HEROES_LIST,
                    ScriptValue::Array(pool.iter().map(|&hero| ScriptValue::Asset(hero)).collect()),
                )]),
            };
            self.receive_controller_message(tick, &[], &message);
        }
        // Hero select opens on the mode's `0727`, which the room holds to
        // every entity (`crate::mode_script::held`): `0C90` st28 → `0738`
        // → `288B` st134 keeps it open while the player has no body.
        // The player's create sends the spawn room flag set.
        self.spawn_flag = (true, self.spawn_flag.1 + 1);
        Ok(())
    }

    /// The player's controller frame and the mode entity's first frame,
    /// `mode_frame`: the mode graph's now where it drives the mode entity
    /// (`crate::mode_script`), else [`PreparedScene::mode_ui`].
    pub fn present(
        &mut self,
        scene: &PreparedScene,
        mode_frame: ModeFrame<'_>,
        tick: u32,
    ) -> io::Result<()> {
        if self.controller.latest_data() == 0 && self.created.contains(&self.owner) {
            self.queue_controller(scene, tick, None)?;
        }
        // While heroes assemble the countdown frames are the mode entity's
        // first (combat.py `spawned`).
        if !self.mode.started() && !self.assembling {
            self.queue_mode_frame(mode_frame, tick)?;
        }
        self.presentation_due = self.controller.latest_data() == 0 || !self.mode.started();
        Ok(())
    }

    /// The hero of the player's body, `None` before his first pick.
    pub fn hero(&self) -> Option<u64> {
        self.body.map(|body| body.hero)
    }

    pub fn set_assembling(&mut self, assembling: bool) {
        self.assembling = assembling;
    }

    /// Whether the mode's own graph drives the match: its `0692` opens hero
    /// select while the heroes assemble and its end closes it (`288B`
    /// st32-st36), with no hand of the server's ([`Self::select_input`]).
    pub fn set_mode_graph(&mut self, mode_graph: bool) {
        self.mode_graph = mode_graph;
    }

    /// The messages the mode graph holds to every entity now
    /// (`ModeRunner::held`), before the world starts and each tick.
    pub fn set_mode_held(&mut self, held: &[GameMessage]) {
        self.mode_held.clear();
        self.mode_held
            .extend(held.iter().map(|message| message.message));
    }

    /// The match's overtime, for the current body's mode script and every
    /// later one (the mode entity's #3392e).
    pub fn set_overtime(&mut self, overtime: bool) {
        if self.overtime == overtime {
            return;
        }
        self.overtime = overtime;
        let failure = self
            .body_script
            .as_mut()
            .and_then(|script| script.runtime.set_mode_overtime(overtime).err());
        if let Some(error) = failure {
            self.drop_failed_script(&error.to_string());
        }
    }

    /// The game mode entity's frame now, such as the countdown: whole when
    /// hand-built, else what changed since the frame the client
    /// acknowledged (`mode_view.rs`); false when the client has no mode
    /// entity yet (server.py `queue_mode_frame`).
    pub fn queue_mode_frame(&mut self, frame: ModeFrame<'_>, tick: u32) -> io::Result<bool> {
        let _replicate = lucy_core::phase::enter(lucy_core::phase::Phase::Replicate);
        if !self.created.contains(&GAME_MODE_ENTITY) {
            return Ok(false);
        }
        if let Some(chunk) = self.mode.queue(frame, tick).map_err(invalid)? {
            let last = chunk.last;
            self.push(Record::new(chunk.update, Delivery::Mode { last }));
            self.remove_stale();
        }
        Ok(true)
    }

    /// The countdown is over: a player with a hero gets hero select closed,
    /// and the skin selector goes (match.py `_end_assemble`). The close goes
    /// to `288B` as the client's would (stand-in for the mode graph's end of
    /// the assembly, which doesn't run).
    pub fn end_assemble(&mut self, scene: &PreparedScene, tick: u32) -> io::Result<()> {
        self.assembling = false;
        self.keep_open = false;
        if !self.can_handle_game_message() {
            return Ok(());
        }
        if self.body.is_some() {
            self.select_request(tick, SelectRequest::OpenOrClose(false));
        }
        self.queue_controller(scene, tick, None)
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
        if !matches!(message.guid, PICK_HERO | OPEN_OR_CLOSE) {
            self.instance_message(tick, message);
            return Ok(());
        }
        let request = message.hero_select(scene.heroes());
        tracing::debug!(
            target: "lucy::join",
            tick,
            guid = format_args!("{:016X}", message.guid),
            request = ?request.map(|request| match request {
                HeroSelectRequest::Pick(hero) => format!("pick {:016X}", hero.map_or(0, |hero| hero.guid)),
                HeroSelectRequest::OpenOrClose(wanted) => format!("open/close {wanted:?}"),
            }),
            "game message"
        );
        // The controller graph's `288B` decides (rid24: a pick he may make
        // now, of a hero he may play; rid22: an open where he may pick, any
        // close; st95/st98 close the screen); its frames go out when what
        // they show changes ([`Self::send_hero_select`]). The server keeps
        // the role queue, the body, the skin and the dead player's wait.
        match request {
            Some(HeroSelectRequest::Pick(Some(hero))) => {
                // The card's role queue (not a graph's: `STU_1AAB4B4F` still
                // answers every hero).
                if viewer
                    .card
                    .role
                    .is_some_and(|role| role != 0 && hero.role_number() != u8::try_from(role).ok())
                {
                    return Ok(());
                }
                let open = self
                    .controller_script
                    .as_ref()
                    .and_then(ControllerScript::hero_select_state)
                    .is_some_and(|state| state.open);
                let picked = self.select_request(tick, SelectRequest::Pick(hero.guid));
                if picked.and_then(|state| state.pick) != Some(hero.guid) {
                    return Ok(());
                }
                // The screen stays open until the heroes are assembled, as
                // in retail (match.py `game_message`); `288B` closes it.
                self.keep_open |= self.assembling && open;
                let skin = viewer
                    .card
                    .skins
                    .get(&hero.guid)
                    .copied()
                    .unwrap_or_default();
                // A dead player's pick waits for his respawn: the new hero
                // comes back at the spawn after the delay (assumption: as in
                // retail, where a hero change while dead keeps the timer).
                if self.death.is_some() {
                    self.respawn_hero = Some((hero.guid, skin));
                    return Ok(());
                }
                // match.py `game_message`: the same hero and skin keep the
                // body, unless its script failed (`needs_body`).
                let same = self
                    .body
                    .is_some_and(|body| (body.hero, body.skin) == (hero.guid, skin));
                if (self.script_failed || !same)
                    && let Some(entity) = self.next_body(now)
                {
                    self.switch(scene, viewer, tick, quanta, entity, (hero, skin), true)?;
                }
            }
            Some(HeroSelectRequest::OpenOrClose(wanted)) => {
                let shown = self.select_shown.is_some_and(|shown| shown.open);
                let open = wanted.unwrap_or(!shown);
                self.keep_open &= open;
                self.select_request(tick, SelectRequest::OpenOrClose(open));
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
        if let Err(error) = script.runtime.receive_message(0, selection, parameters) {
            self.drop_failed_script(&error.to_string());
        }
    }

    /// The ch3 ack: the last of the client's own command frames run (before
    /// the first, the frame before the first admitted, at most `tick`). The
    /// client keeps its commands from ack − 1 on
    /// (`lucy_physics::mover::correction` notes), so it resends those a
    /// dry tick waits for. Repeats past the wait run beyond his newest; the
    /// ack stays at his newest, so he still sends the frames they stood in
    /// for (`InputQueue::reconcile`). It is never past the frame an owner
    /// frame names.
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
    /// one command a tick; none when none came (the body waits for it),
    /// past [`MAX_WAIT`] dry ticks a repeat of the last input, past
    /// [`MAX_REPEATS`] repeats a neutral frame.
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

    /// The view turn of a body placed at a spawn point (first spawn,
    /// respawn, new round), once the client has its possession: the client
    /// keeps its view on the player, not the body, and only 25006/25008/
    /// 25009 set it (`refs/reports/RespawnFacing.md`), so without it a
    /// respawn kept the view of the death. Taken once; a newer body drops
    /// it.
    pub fn take_spawn_view(&mut self) -> Option<CombatNotice> {
        let (id, yaw) = self.spawn_view?;
        if self.body.is_none_or(|body| body.id != id) {
            self.spawn_view = None;
            return None;
        }
        if self.bound != Some(id) {
            return None;
        }
        self.spawn_view = None;
        Some(CombatNotice::View { yaw })
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

    /// The pickups the map's placeables hold now (the health packs), for
    /// the body's search.
    pub fn set_pickups(&mut self, pickups: &[Pickup]) {
        self.pickups.clear();
        self.pickups.extend_from_slice(pickups);
        if let Some(script) = &mut self.body_script {
            script.runtime.set_pickups(pickups);
        }
    }

    /// The map placeables' entity variables now
    /// (`PlacedScripts::entity_variables`), which the body's graphs read
    /// (the capture area aura's `0F00` st8 asks its point's owner
    /// `#9901.id(99)`).
    pub fn set_map_variables(&mut self, variables: &Arc<[(u32, Variables)]>) {
        self.map_variables = Arc::clone(variables);
        if let Some(script) = &mut self.body_script {
            script.runtime.set_map_variables(variables);
        }
    }

    /// Tests: the player's controller's instances of scope `scope`
    /// (`vm/scopes.rs`), in order: the first is the one a client's 21616
    /// from it names (the Training Partner console's scope 126, where its
    /// picker sends `1213`).
    #[cfg(any(test, feature = "fixtures"))]
    pub fn controller_scope_instances(&self, scope: u32) -> Vec<u16> {
        let Some(snapshot) = self
            .controller_script
            .as_ref()
            .and_then(|script| script.snapshot())
        else {
            return Vec::new();
        };
        snapshot
            .instances
            .iter()
            .filter(|(_, instance)| instance.scope.as_ref().is_some_and(|link| link.id == scope))
            .map(|(&id, _)| id)
            .collect()
    }

    /// Tests: whether the last controller frame queued runs state `state`
    /// of the controller's instance of graph `graph` (its own or a scope
    /// instance on it).
    #[cfg(any(test, feature = "fixtures"))]
    pub fn controller_frame_state(&self, graph: u32, state: u16) -> bool {
        let (Some(sent), Some(snapshot)) = (
            &self.controller_sent,
            self.controller_script
                .as_ref()
                .and_then(ControllerScript::snapshot),
        ) else {
            return false;
        };
        snapshot
            .instances
            .iter()
            .any(|(&id, instance)| instance.graph == graph && sent.state(id, state).is_some())
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

    /// Tests: the targets of the active scope state `state` of the current
    /// body's instance of graph `graph`, each with the instance its frame
    /// record names there (`lucy_script` `vm/scopes.rs`).
    #[cfg(any(test, feature = "fixtures"))]
    pub fn body_scope_hits(&self, graph: u32, state: u16) -> Vec<(u32, Option<u16>)> {
        let Some(snapshot) = self
            .body_script
            .as_ref()
            .and_then(|script| script.runtime.snapshot())
        else {
            return Vec::new();
        };
        snapshot
            .instances
            .values()
            .filter(|instance| instance.graph == graph)
            .filter_map(|instance| instance.states.get(&state))
            .filter(|state| state.active)
            .flat_map(|state| state.scopes.iter().map(|hit| (hit.target, hit.instance)))
            .collect()
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
            .and_then(|script| script.runtime.set_entity_variables(&variables).err());
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
            pose: {
                let pose = mover.pose();
                scene.player_target(hero, pose.position, pose.yaw, mover.crouched())
            },
            dead: self.death.is_some(),
            health: vitals.map(|vitals| [vitals.total(), vitals.maximum()]),
            tags: scene.body_tags(hero.body),
            hardpoints: scene.body_hardpoints(hero.body),
            ai: self.ai_control(scene.graphs(), body.hero),
            dealt: self.body_effects().map_or_else(Vec::new, |effects| {
                effects
                    .modifiers()
                    .filter(|modifier| [DAMAGE_DEALT, HEALING_DEALT].contains(&modifier.kind))
                    .cloned()
                    .collect()
            }),
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

    /// combat.py `_damage` and `_died` for the player's current body, its
    /// messages included ([`Self::apply_damage`], then
    /// [`Self::send_hit_messages`] without assists).
    pub fn damage_body(
        &mut self,
        scene: &PreparedScene,
        hit: &PlayerHit,
        tick: u32,
        now: Duration,
    ) -> Option<BodyDamage> {
        let damage = self.apply_damage(scene, hit, tick, now)?;
        self.send_hit_messages(scene, hit, &damage, &[]);
        Some(damage)
    }

    /// combat.py `_damage` and `_died` for the player's current body: its
    /// pools take the hit, scaled by the damage-taken modifiers his graphs
    /// hold (`vm/statuses.rs`: Nano Boost's reduction; Deflect's ×0 for an
    /// attacker in front, its party filter tested on the hit's shooter),
    /// shields, armour, health, and its script's health follows; a killing
    /// hit makes it dead and sets its respawn after the mode's time. The
    /// hit's source is kept as the body's attacker
    /// ([`Self::recent_attackers`]). Its messages are
    /// [`Self::send_hit_messages`]'s.
    pub fn apply_damage(
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
        if !self.can_be_damaged() {
            return None;
        }
        let mut tags = hit.tags;
        let amount = self.taken_scale(DAMAGE_TAKEN, &mut tags, hit.shooter.source, hit.amount);
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
                let result = script.runtime.vitals_mut()?.damage(amount, source);
                if result.dealt > 0.0 {
                    failure = script.runtime.health_changed().err();
                }
                result
            }
            None => self
                .plain_vitals
                .get_or_insert_with(|| hero_vitals(scene, body.hero))
                .damage(amount, source),
        };
        if result.dealt <= 0.0 {
            return None;
        }
        self.body_health += 1;
        let attacker = hit.shooter.source;
        self.attackers.retain(|attack| {
            attack.body == body.id.entity
                && attack.source != attacker
                && credits_damage(attack.at, now)
        });
        self.attackers.push(Attack {
            body: body.id.entity,
            source: hit.shooter.source,
            at: now,
        });
        if result.killed {
            self.death = Some(BodyDeath {
                body: body.id,
                killer: hit.shooter.source,
                tick: death_tick,
                died_at: now,
                due: None,
                revive_at: None,
            });
        }
        if let Some(error) = failure {
            self.drop_failed_script(&error.to_string());
        }
        Some(BodyDamage {
            dealt: result.dealt,
            killed: result.killed,
            tags,
        })
    }

    /// The messages of a hit [`Self::apply_damage`] applied to the current
    /// body: the hurt notice (25000, [`CombatNotice::Hurt`]) for his
    /// client's damage direction arc when another dealt it from a known
    /// place ([`PlayerHit::from`]), then the body's script's: the damage
    /// message (`0011.025`), and for a killing hit the "died" message
    /// (003E.025, which pulses the kill feed line), both naming `assists`
    /// (`03E7`: the assisters' bodies, [`crate::combat::kill_credit`]).
    pub fn send_hit_messages(
        &mut self,
        scene: &PreparedScene,
        hit: &PlayerHit,
        damage: &BodyDamage,
        assists: &[u32],
    ) {
        if let Some(from) = hit.from.filter(|_| !self.headless) {
            self.hurt.push(CombatNotice::Hurt {
                victim: hit.body,
                attacker: hit.shooter.source,
                from: from.map(|axis| axis as f32),
                dealt: damage.dealt,
            });
        }
        let Some(script) = self
            .body_script
            .as_mut()
            .filter(|script| script.id.entity == hit.body)
        else {
            return;
        };
        let camera = damage
            .killed
            .then(|| script.runtime.death_camera(hit.shooter.source))
            .flatten();
        let messages = hit_messages(
            hit,
            damage.tags,
            damage.dealt,
            damage.killed.then_some(assists),
            camera,
            scene.hurt_feedback(),
        );
        let failure = messages.into_iter().find_map(|(message, parameters)| {
            script.runtime.receive_message(0, message, parameters).err()
        });
        if let Some(error) = failure {
            self.drop_failed_script(&error.to_string());
        }
    }

    /// The hurt notices of the hits since the last call, in order.
    pub fn take_hurt_notices(&mut self) -> Vec<CombatNotice> {
        std::mem::take(&mut self.hurt)
    }

    /// The players' sources (bodies) of the damage the body `body` took
    /// within [`crate::combat::DAMAGE_CREDIT`] before `now`, each once, in the order of
    /// their last hits.
    pub fn recent_attackers(&self, body: u32, now: Duration) -> Vec<u32> {
        self.attackers
            .iter()
            .filter(|attack| attack.body == body && credits_damage(attack.at, now))
            .map(|attack| attack.source)
            .collect()
    }

    /// The assist credits the current body's states hold now
    /// ([`OwnerBodyRuntime::assists`]).
    pub fn assist_links(&self) -> Vec<AssistLink> {
        let Some(body) = self.body else {
            return Vec::new();
        };
        self.body_script
            .as_ref()
            .filter(|script| script.id == body.id)
            .map(|script| script.runtime.assists())
            .unwrap_or_default()
    }

    /// combat.py `heal_area` for the player's living body, the heal `hit`
    /// scaled by the healing modifiers his graphs hold for a heal with its
    /// tags (Biotic Grenade's anti-heal and heal boost, `vm/statuses.rs`;
    /// party filters tested on the healer). Returns what the pools took.
    pub fn heal_body(&mut self, scene: &PreparedScene, hit: &PlayerHit) -> f64 {
        let Some(current) = self.body.filter(|current| current.id.entity == hit.body) else {
            return 0.0;
        };
        if self.death.is_some() {
            return 0.0;
        }
        let mut tags = hit.tags;
        let amount = self.taken_scale(HEALING_TAKEN, &mut tags, hit.shooter.source, hit.amount);
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

    /// The spawn rooms each tick (`map_areas` module notes): whether the
    /// living body stands in one of his team's; the client's spawn room
    /// flag (component 74 field 6) follows [`Self::may_change_hero`]. The
    /// room's heal and invulnerability are the mode body graph's (`0C8E`
    /// st11 and st13, on the room's `0723`: `crate::placed_script`).
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
        self.shield_step();
        let inside = self.may_change_hero();
        // The controller graph's `STU_E4C6AAB4` reads the same field 6.
        if let Some(script) = &mut self.controller_script {
            script.set_may_pick(inside);
        }
        if inside != self.spawn_flag.0 {
            let revision = self.spawn_flag.1 + 1;
            self.spawn_flag = (inside, revision);
            self.push(Record::new(
                crate::scene::spawn_room_flag(self.owner, inside),
                Delivery::SpawnFlag { revision },
            ));
            self.remove_stale();
        }
    }

    /// The body's filter word follows the statuses its graphs hold
    /// ([`SHIELDED`], `scene` module notes): the owner's client then
    /// predicts no explosion push on it, as it gets none from the server
    /// ([`Self::can_be_damaged`]). A dead body's word comes with its pools.
    fn shield_step(&mut self) {
        let Some(body) = self.body.filter(|_| self.death.is_none()) else {
            return;
        };
        let shielded = self.body_effects().is_some_and(BodyEffects::invulnerable);
        if shielded == self.shield.0 {
            return;
        }
        let revision = self.shield.1 + 1;
        self.shield = (shielded, revision);
        let bits = body_filter(body.team_bit, false) | if shielded { SHIELDED } else { 0 };
        self.push(Record::new(
            EntityUpdate {
                entity: body.id.entity,
                operation: Some(Operation::Update(vec![filter_component(bits)])),
                ..EntityUpdate::default()
            },
            Delivery::Filter { revision },
        ));
        self.remove_stale();
    }

    /// The statuses and combat modifiers the current body's states hold.
    fn body_effects(&self) -> Option<&BodyEffects> {
        let body = self.body?;
        self.body_script
            .as_ref()
            .filter(|script| script.id == body.id)
            .and_then(|script| script.runtime.effects())
    }

    /// A health change of `amount` of `kind` with `tags` on the current
    /// body, with `other` its attacker or healer, as its graphs' modifiers
    /// make it (`OwnerBodyRuntime::combat_scale`); their notices go out
    /// with the body's messages ([`Self::take_messages`]).
    fn taken_scale(&mut self, kind: u64, tags: &mut DamageTags, other: u32, amount: f64) -> f64 {
        let body = self.body;
        let scaled = match self
            .body_script
            .as_mut()
            .filter(|script| body.is_some_and(|body| script.id == body.id))
        {
            Some(script) => script.runtime.combat_scale(kind, tags, other),
            None => Scaled::NONE,
        };
        self.modified.extend(scaled.notices(amount, other));
        scaled.apply(amount)
    }

    /// Whether a hit can hurt the current body: no invulnerable status
    /// (`STU_9028E035`) held by his graphs (the spawn room's is `0C8E`
    /// st11). An explosion pushes only a body it can hurt (the user's game
    /// check, 2026-11-17: no push in the spawn room), so the knockback takes
    /// the same gate as the damage.
    fn can_be_damaged(&self) -> bool {
        !self.body_effects().is_some_and(BodyEffects::invulnerable)
    }

    /// Another's explosion pushes the player's living body `body`
    /// ([`crate::combat::PlayerPush`]) at `now`: `push` goes to his mover
    /// for its next posed frame, as his own explosion's does
    /// ([`Self::advance_projectiles`]), and the owner corrections carry it
    /// to his client, which predicts no push from another's explosion; `by`
    /// has knocked the body ([`Self::knocked_by`]). Not where it can't hurt
    /// him ([`Self::can_be_damaged`]: the spawn room, whose status bit 0x4
    /// also stops the client's own push, `scene` module notes) nor on a body
    /// the hit killed. Whether it pushed the body.
    pub fn knock(&mut self, body: u32, push: [f64; 3], by: Shooter, now: Duration) -> bool {
        if self.current_body() != Some(body) || self.death.is_some() || !self.can_be_damaged() {
            return false;
        }
        let Some(mover) = &mut self.mover else {
            return false;
        };
        mover.push(push);
        self.knocks.knocked(body, by, now);
        true
    }

    /// Who last knocked the player's body `body` so that its environment
    /// kill at `now` is theirs ([`KnockCredit`]).
    pub fn knocked_by(&self, body: u32, now: Duration) -> Option<Shooter> {
        self.knocks.credit(body, now)
    }

    /// The tick's environment kills are dealt ([`KnockCredit::settle`]).
    pub fn settle_knock(&mut self) {
        self.knocks.settle();
    }

    /// Who the player's body's graphs named its environment kill credits
    /// since the last call (`STU_545AB3B8`), for the match to resolve
    /// ([`crate::combat::Combat::apply_environment_credits`]).
    pub fn take_environment_credits(&mut self) -> Vec<EnvironmentCredit> {
        self.body_script
            .as_mut()
            .map_or_else(Vec::new, |script| script.runtime.take_environment_credits())
    }

    /// A graph named who an environment kill of the player's living body
    /// `body` credits at `now` ([`KnockCredit::named`]).
    pub fn credit_environment(&mut self, body: u32, by: Option<Shooter>, now: Duration) {
        if self.current_body() == Some(body) && self.death.is_none() {
            self.knocks.named(body, by, now);
        }
    }

    /// An aura comes on the player's living body (Biotic Grenade's
    /// explosion, an ApplyAura) or goes off it.
    pub fn change_aura(&mut self, change: &AuraChange) {
        if self.death.is_some() && matches!(change, AuraChange::Apply { .. }) {
            return;
        }
        let Some(body) = self.body else {
            return;
        };
        let failure = self
            .body_script
            .as_mut()
            .filter(|script| script.id == body.id)
            .and_then(|script| script.runtime.change_aura(change).err());
        if let Some(error) = failure {
            self.drop_failed_script(&error.to_string());
        }
    }

    /// A scope instance a scope state elsewhere (the Training Partner
    /// console's `265F`: scope 12 on the player's body, scope 126 on his
    /// controller) opens or closes on this player's body or controller at
    /// `tick` (`lucy_script` `vm/scopes.rs`): the instance opened. A
    /// failure stops that script.
    pub fn change_scope(&mut self, tick: u32, change: &ScopeChange) -> Option<u16> {
        let target = change.target();
        if self.takes_body_message(target) {
            let script = self.body_script.as_mut()?;
            return match script.runtime.change_scope(change) {
                Ok(opened) => opened,
                Err(error) => {
                    self.drop_failed_script(&error.to_string());
                    None
                }
            };
        }
        self.controller_script
            .as_ref()
            .filter(|script| script.entity() == target)?;
        self.advance_controller(tick);
        let script = self.controller_script.as_mut()?;
        let opened = match script.change_scope(tick, change) {
            Ok(opened) => opened,
            Err(error) => {
                eprintln!("controller script of {:08X} stopped ({error})", self.owner);
                self.controller_script = None;
                None
            }
        };
        self.after_controller();
        opened
    }

    /// The player as the Workshop's values see him
    /// ([`crate::workshop`]): none until his body is in the world.
    pub fn workshop_view(&self, scene: &PreparedScene) -> Option<WorkshopView> {
        self.started?;
        let body = self.body?;
        let mover = self.mover.as_ref()?;
        let hero = scene.heroes().get(body.hero)?;
        let pose = mover.pose();
        let target = scene.player_target(hero, pose.position, pose.yaw, mover.crouched());
        let script = self
            .body_script
            .as_ref()
            .filter(|script| script.id == body.id);
        let vitals = match script {
            Some(script) => script.runtime.vitals(),
            None => self.plain_vitals.as_ref(),
        };
        // The meter `0B8B` and Set Ultimate Charge (`1DAB`) keep: #342e
        // of the cost #338e.
        let ultimate_charge =
            script
                .and_then(|script| script.runtime.snapshot())
                .map_or(0.0, |snapshot| {
                    let read = |id| {
                        snapshot
                            .variables
                            .get(&id)
                            .map_or(0.0, lucy_script::expr::to_float)
                    };
                    let cost = read(lucy_script::ultimate::ULT_COST);
                    if cost > 0.0 {
                        100.0 * read(lucy_script::ultimate::ULT_CHARGE) / cost
                    } else {
                        0.0
                    }
                });
        let command = self.input.last;
        let [right, forward] = pose.throttles.map(|throttle| f64::from(throttle) / 127.0);
        Some(WorkshopView {
            feet: pose.position,
            eye: target.eye,
            facing: command.map_or([target.yaw.sin(), 0.0, target.yaw.cos()], |command| {
                crate::hitscan::aim_direction(&command)
            }),
            velocity: pose.velocity,
            throttle: [-right, 0.0, forward],
            buttons: self.input.ran,
            on_ground: mover.on_ground(),
            alive: self.death.is_none(),
            hero: body.hero,
            health: vitals.map_or(0.0, Vitals::total),
            max_health: vitals.map_or(0.0, Vitals::maximum),
            ultimate_charge,
        })
    }

    /// The cooldown left on the current body's ability on logical button
    /// `button`, as its script has it; 0 without a body script.
    pub fn ability_cooldown(&self, button: i64) -> f64 {
        self.body_script
            .as_ref()
            .filter(|script| Some(script.id) == self.body.map(|body| body.id))
            .map_or(0.0, |script| script.runtime.ability_cooldown(button))
    }

    /// Workshop player variable `index` of this player: his controller's
    /// entity variable (`lucy_script::expr::workshop_variable`), a
    /// chase's value included. None without a controller script.
    pub fn workshop_variable(&self, index: u8) -> Option<ScriptValue> {
        Some(
            self.controller_script
                .as_ref()?
                .workshop_variable(i64::from(index)),
        )
    }

    /// The controller's script at `tick`, before the Workshop's rules read
    /// his variables (a chase moves them with time).
    pub fn advance_controller_to(&mut self, tick: u32) {
        self.advance_controller(tick);
    }

    /// Set Workshop player variable `index` of this player. A failure stops
    /// the controller's script.
    pub fn set_workshop_variable(&mut self, index: u8, value: ScriptValue) {
        let Some(script) = &mut self.controller_script else {
            return;
        };
        if let Err(error) = script.set_workshop_variable(i64::from(index), value) {
            eprintln!("controller script of {:08X} stopped ({error})", self.owner);
            self.controller_script = None;
        }
        self.after_controller();
    }

    /// The mode entity played `request.graph` on this player's controller
    /// at `tick` (a Workshop Chase Variable's `1CCF`): whether his
    /// controller is its target. A failure stops the controller's script.
    pub fn play_script(&mut self, tick: u32, request: &PlayRequest) -> bool {
        if !self
            .controller_script
            .as_ref()
            .is_some_and(|script| script.entity() == request.target)
        {
            return false;
        }
        self.advance_controller(tick);
        let Some(script) = &mut self.controller_script else {
            return true;
        };
        if let Err(error) = script.play_script(request) {
            eprintln!("controller script of {:08X} stopped ({error})", self.owner);
            self.controller_script = None;
        }
        self.after_controller();
        true
    }

    /// The player and body the next dummy bot the current body's graphs
    /// make gets (the console's scope 12 there, on a re-pick).
    pub fn set_next_dummy(&mut self, next: Option<NextDummy>) {
        if let Some(script) = &mut self.body_script {
            script.runtime.set_next_dummy(next);
        }
    }

    /// The dummy bots the current body's graphs made or removed since the
    /// last call, for the match.
    pub fn take_dummy_changes(&mut self) -> Vec<DummyChange> {
        self.body_script
            .as_mut()
            .map_or_else(Vec::new, |script| script.runtime.take_dummy_changes())
    }

    /// The scope instances the current body's scope states opened or
    /// closed on their targets since the last call (the Training Partner
    /// console's scope 197 on a re-picked dummy), and the controller's on
    /// entities other than his body, for the match.
    pub fn take_scope_changes(&mut self) -> Vec<ScopeChange> {
        let mut changes = std::mem::take(&mut self.controller_scopes);
        if let Some(script) = &mut self.body_script {
            changes.extend(script.runtime.take_scope_changes());
        }
        changes
    }

    /// The match opened the scope instance `instance` on `target` for the
    /// scope state `source` of this player's current body or controller.
    pub fn scope_opened(&mut self, source: ScopeSource, target: u32, instance: u16) {
        if let Some(script) = self
            .controller_script
            .as_mut()
            .filter(|script| script.entity() == source.entity)
        {
            script.scope_opened(source, target, instance);
        } else if let Some(script) = &mut self.body_script {
            script.runtime.scope_opened(source, target, instance);
        }
    }

    /// The variables the current body's scope instances set in their scope
    /// states' instances since the last call (the console's `#940` on a
    /// re-pick), for the match.
    pub fn take_parent_writes(&mut self) -> Vec<ParentWrite> {
        self.body_script
            .as_mut()
            .map_or_else(Vec::new, |script| script.runtime.take_parent_writes())
    }

    /// The graphs the current body's ability graphs played on other
    /// bodies since the last call (Rocket Punch's `0D07`, Soundwave's
    /// `0ECA`), for the match to play there ([`Self::play_on_body`]).
    pub fn take_body_play_requests(&mut self) -> Vec<PlayRequest> {
        self.body_script
            .as_mut()
            .map_or_else(Vec::new, |script| script.runtime.take_play_requests())
    }

    /// Another body's ability graph played `request.graph` on this
    /// player's current body (`lucy_script` `vm/play.rs`
    /// `Plays::Others`): its body script plays it as the PlayScript would
    /// have, and its instance reaches every client in the body's frames,
    /// as the retail server's did (the client's own PlayScript on a
    /// networked instance runs only the base action, Workshop.md
    /// "WorkshopRuntime6"). Whether this body is its target; a failure
    /// stops the body's script.
    pub fn play_on_body(&mut self, request: &PlayRequest) -> bool {
        if !self.takes_body_message(request.target) {
            return false;
        }
        let Some(script) = self.body_script.as_mut() else {
            return false;
        };
        match script.runtime.play_script(request) {
            Ok(played) => played,
            Err(error) => {
                self.drop_failed_script(&error.to_string());
                false
            }
        }
    }

    /// The knockbacks the current body's graphs reported since the last
    /// call (`STU_AFBB59E9`), for the match's knockback credit.
    pub fn take_knockbacks(&mut self) -> Vec<Knockback> {
        self.body_script
            .as_mut()
            .map_or_else(Vec::new, |script| script.runtime.take_knockbacks())
    }

    /// A 21616 for the player's controller with a message other than hero
    /// select's: what the controller's instance named there sends (the
    /// action `STU_79A131B4`, 0x7FF789814C20, writes its host's entity and
    /// its own instance; the Training Partner picker's `1213` from the
    /// console's scope 126 there). Its parameters (`Parameters::rows`)
    /// go to the instance's entries; a parameter the client sends as an
    /// array of one value is that value, as the server's own sends carry
    /// it. A blob the server can't read is dropped.
    fn instance_message(&mut self, tick: u32, message: ClientGameMessage<'_>) {
        let Ok(instance) = u16::try_from(message.instance) else {
            return;
        };
        let Some(rows) = message.parameters.rows() else {
            tracing::debug!(
                target: "lucy::join",
                tick,
                guid = format_args!("{:016X}", message.guid),
                "game message with unread parameters"
            );
            return;
        };
        let parameters = rows
            .into_iter()
            .map(|(key, value)| (u32::from(key as u16), parameter_value(value)))
            .collect();
        self.advance_controller(tick);
        let owner = self.owner;
        let Some(script) = &mut self.controller_script else {
            return;
        };
        if let Err(error) = script.receive_in_instance(tick, instance, message.guid, parameters) {
            eprintln!("controller script of {owner:08X} stopped ({error})");
            self.controller_script = None;
        }
    }

    /// The aura changes the current body's ApplyAura states made, for the
    /// match to hand to their targets.
    pub fn take_body_aura_changes(&mut self) -> Vec<AuraChange> {
        if !self.world_started() {
            return Vec::new();
        }
        self.body_script
            .as_mut()
            .map_or_else(Vec::new, |script| script.runtime.take_aura_changes())
    }

    /// The player's living body dealt `amount` of damage (or healing, with
    /// `heal`) to `target`, a body of definition `target_body`, in a health
    /// change tagged `tags`: its body takes `0012.025` (or `0044.025`), as
    /// from the retail server, and its graph `0B8B` charges the ultimate
    /// for it (`lucy_script::ultimate`). The body learns the target's
    /// authored target tags first (`0B8B` rid45 tests `02ED`, rid50 `03C3`,
    /// rid85 `011E`).
    pub fn dealt(
        &mut self,
        graphs: &GraphStore,
        target: u32,
        target_body: Option<u64>,
        amount: f64,
        tags: &DamageTags,
        heal: bool,
    ) {
        if self.death.is_some() || amount <= 0.0 {
            return;
        }
        let Some(body) = self.body else {
            return;
        };
        let failure = self
            .body_script
            .as_mut()
            .filter(|script| script.id == body.id)
            .and_then(|script| {
                let target_tags = target_body
                    .and_then(|body| graphs.bodies().get(&body))
                    .and_then(|body| body.target_tags.clone())
                    .unwrap_or_default();
                script.runtime.set_victim_tags(target, target_tags);
                let (message, parameters) =
                    lucy_script::ultimate::dealt_message(target, amount, tags, heal);
                script
                    .runtime
                    .receive_message(body.id.entity, message, parameters)
                    .err()
            });
        if let Some(error) = failure {
            self.drop_failed_script(&error.to_string());
        }
    }

    /// The source of a health change the player's body instigated (its
    /// `m_7B93233C`: a Biotic Orb, whose auras `1101`/`1102` heal or hurt
    /// through the bodies' own graphs) dealt `amount` to `target`: it
    /// takes the same `0012.025` (or `0044.025`) as the body
    /// ([`Self::dealt`]), which the orb's `10FF`/`1100` rid15 counts
    /// against its pool, whether or not the body still lives. It goes with
    /// the body's messages ([`Self::take_messages`]), from the source
    /// itself (assumption: retail's sender isn't known; the orbs' entries
    /// don't filter it).
    pub fn source_dealt(
        &mut self,
        source: u32,
        target: u32,
        amount: f64,
        tags: &DamageTags,
        heal: bool,
    ) {
        if amount <= 0.0 {
            return;
        }
        let (message, parameters) =
            lucy_script::ultimate::dealt_message(target, amount, tags, heal);
        self.modified.push(GameMessage {
            sender: source,
            target: source,
            message,
            parameters,
        });
    }

    /// The hero the controller's graph forced on him
    /// ([`lucy_script::controller::ControllerScript::forced_hero`]):
    /// `0C90`'s StateHero plays #940e, and the server makes the body, as
    /// for a pick: a living body changes where it stands, a dead one's
    /// respawn plays it. The skin is his card's.
    pub fn change_to_forced_hero(
        &mut self,
        scene: &PreparedScene,
        viewer: &PlayerAppearance<'_>,
        tick: u32,
        now: Duration,
        quanta: u32,
    ) -> io::Result<()> {
        let Some(hero) = self
            .forced_hero
            .take()
            .and_then(|hero| scene.heroes().get(hero))
        else {
            return Ok(());
        };
        let skin = viewer
            .card
            .skins
            .get(&hero.guid)
            .copied()
            .unwrap_or_default();
        if self.death.is_some() {
            self.respawn_hero = Some((hero.guid, skin));
            return Ok(());
        }
        if self.body.is_some_and(|body| body.hero == hero.guid) {
            return Ok(());
        }
        match self.next_body(now) {
            Some(entity) => self.switch(scene, viewer, tick, quanta, entity, (hero, skin), true)?,
            // Every id of his body ring is still reserved: next tick.
            None => self.forced_hero = Some(hero.guid),
        }
        Ok(())
    }

    /// combat.py `_respawn`: once the body's respawn is due ([`BodyDeath`]:
    /// its `0C8E` sent `0685`), a dead player gets a new body of the same
    /// hero at his spawn point, as a hero switch does (match.py
    /// `switch_hero`), revived ([`Self::revived`]). While every id of his
    /// body ring is still reserved, he tries again RESPAWN_RETRY later.
    /// True when he respawned.
    ///
    /// Retail revives the dead body itself (`STU_621AF478`), whose entity
    /// variables stay, so the new body that stands for it starts with the
    /// dead one's ultimate charge #342e (before the revive message, so
    /// `0B8B` rid66's "spawn with the ultimate" ruleset key still fills
    /// it). A body of another hero (a pick while dead) starts from 0, as a
    /// hero change does: `0C90` st131 sends the new body `14F2.025` with
    /// the share #32319 the mode's #32271 keeps, none by default.
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
        let counting = self
            .body_script
            .as_ref()
            .is_some_and(|script| script.id == death.body && script.runtime.counts_respawn());
        let due = death.due.unwrap_or_else(|| {
            if counting {
                Duration::MAX
            } else {
                death.died_at.saturating_add(scene.respawn_time())
            }
        });
        if now < due {
            return Ok(false);
        }
        let Some(body) = self.body.filter(|body| body.id == death.body) else {
            self.death = None;
            return Ok(false);
        };
        let Some(entity) = self.next_body(now) else {
            if let Some(death) = &mut self.death {
                death.due = Some(now.saturating_add(RESPAWN_RETRY));
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
        let charge = self
            .body_script
            .as_ref()
            .filter(|script| guid == body.hero && script.id == body.id)
            .and_then(|script| script.runtime.snapshot())
            .and_then(|snapshot| {
                let charge = snapshot.variables.get(&lucy_script::ultimate::ULT_CHARGE)?;
                Some(Variables::from([(
                    lucy_script::ultimate::ULT_CHARGE,
                    charge.clone(),
                )]))
            });
        self.switch(scene, viewer, tick, quanta, entity, (hero, skin), false)?;
        if let Some(charge) = charge {
            let failure = self
                .body_script
                .as_mut()
                .and_then(|script| script.runtime.set_entity_variables(&charge).err());
            if let Some(error) = failure {
                self.drop_failed_script(&error.to_string());
            }
        }
        self.revived();
        if let Some(feet) = death.revive_at {
            self.teleport_current(feet, None);
        }
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
        self.revived();
        if select && self.can_handle_game_message() {
            // `288B` opens as on the client's H; it stays shown open while
            // the heroes assemble.
            self.select_request(tick, SelectRequest::OpenOrClose(true));
            self.keep_open = true;
            self.queue_controller(scene, tick, None)?;
        }
        Ok(true)
    }

    /// The new body of a respawn or a new round stands for retail's revived
    /// one (loose ends, "A respawn makes a new body"): it gets the revive
    /// message `0013.025` with `00C6` on. On it the hero's `01CA` (rid56)
    /// raises his respawn line `05D8` on his controller 1.5 s later (none
    /// in Elimination, `0020` or `0029`), which every hero's voice set
    /// answers with stimulus `002C` (once per 20 s). Assumptions: retail
    /// sends `0013` (no graph does; its 44 listeners reset ability state
    /// with `00C6` on, as for a new life) when it revives the body, with
    /// `00C6` on; the first spawn of a hero gets none.
    fn revived(&mut self) {
        let Some(script) = &mut self.body_script else {
            return;
        };
        if let Err(error) = script.runtime.receive_message(
            0,
            REVIVED,
            Variables::from([(REVIVED_RESPAWN, ScriptValue::Bool(true))]),
        ) {
            self.drop_failed_script(&error.to_string());
        }
    }

    /// The game messages the player's graphs sent other entities since the
    /// last call, for the match to route ([`crate::messages`]): his current
    /// body's (match.py `queue_body_message`), while other clients get the
    /// body (`_shown`), the notices of its modifiers' changes, then his
    /// controller's (`0C90` st5/st21/st70's `068E`, `070B`, `08BE` to
    /// every entity after a pick, `06FF` to the mode).
    pub fn take_messages(&mut self) -> Vec<GameMessage> {
        let mut messages = self.take_body_messages();
        messages.append(&mut self.modified);
        if let Some(controller) = &mut self.controller_script {
            messages.extend(controller.take_messages());
        }
        messages
    }

    /// The ends of his body's persistent sends to other entities since the
    /// last call, with those its stopped scripts held, for the match to
    /// route with his messages ([`crate::messages::deliver`]).
    pub fn take_message_ends(&mut self) -> Vec<MessageEnd> {
        let mut ends = std::mem::take(&mut self.retired_sends);
        if self.world_started()
            && let Some(script) = self.body_script.as_mut()
        {
            ends.extend(script.runtime.take_message_ends());
        }
        ends
    }

    /// His body's persistent sends' new parameters since the last call
    /// ([`crate::messages::deliver_updates`]).
    pub fn take_message_updates(&mut self) -> Vec<GameMessage> {
        if !self.world_started() {
            return Vec::new();
        }
        self.body_script
            .as_mut()
            .map_or_else(Vec::new, |script| script.runtime.take_message_updates())
    }

    fn take_body_messages(&mut self) -> Vec<GameMessage> {
        if !self.world_started() {
            return Vec::new();
        }
        let Some(script) = self.body_script.as_mut() else {
            return Vec::new();
        };
        let mut messages = script.runtime.take_messages();
        // The mode body script's `0685`: the dead body may respawn now.
        // Retail's mode graph takes it (`0C8F`, then `0687` to the
        // controller, whose `0C90` revives the body); the server respawns
        // the player itself (loose ends, "A respawn makes a new body").
        let sender = script.id;
        let revived = script.runtime.take_revive();
        messages.retain(|message| {
            let ready = message.message == RESPAWN_READY && message.target == GAME_MODE_ENTITY;
            if ready && let Some(death) = self.death.as_mut().filter(|death| death.body == sender) {
                death.due.get_or_insert(Duration::ZERO);
            }
            !ready
        });
        // Its graphs revived it (the Workshop's Resurrect, `0C8E` rid76):
        // back at once where the body lies. Retail revives the same body
        // (`STU_621AF478`); the server makes a new one there, as for any
        // respawn.
        let feet = self.mover.as_ref().map(|mover| mover.pose().position);
        if revived && let Some(death) = self.death.as_mut().filter(|death| death.body == sender) {
            death.due = Some(Duration::ZERO);
            death.revive_at = feet;
        }
        messages
    }

    /// The stats the player's graphs counted since the last call: his
    /// body's (TrackStat, TrackStatTime in its hero and ability graphs) and
    /// his controller's (`0215`, `0C90`), for the match's stat table.
    pub fn take_tracked_stats(&mut self) -> Vec<TrackedStat> {
        let mut stats = self
            .body_script
            .as_mut()
            .map_or_else(Vec::new, |script| script.runtime.take_tracked_stats());
        if let Some(controller) = &mut self.controller_script {
            stats.extend(controller.take_tracked_stats());
        }
        stats
    }

    /// The voice lines the current body says since the last call, by its
    /// hero's voice set ([`crate::voice_lines`]): for the criteria its
    /// graphs raised on it or its controller (the communication wheel's
    /// lines, the pain lines after a hit, the respawn line), and the voice
    /// stimuli its server-only states and its controller's named (the
    /// elimination lines, the line after a third kill), each once its delay
    /// is up. A dead body says only its death cries.
    pub fn take_voice_lines(&mut self, scene: &PreparedScene, tick: u32) -> Vec<VoiceLine> {
        if !self.world_started() {
            return Vec::new();
        }
        let Some(script) = self.body_script.as_mut() else {
            return Vec::new();
        };
        let criteria = script.runtime.take_criteria();
        let mut requests = script.runtime.take_voices();
        let id = script.id;
        // The controller's `0215` waits 1.6 s after a third kill before its
        // line (st3 → st28): its clock runs on with the match's.
        let stopped =
            self.controller_script
                .as_mut()
                .and_then(|controller| match controller.advance(tick) {
                    Ok(_) => {
                        requests.extend(controller.take_voices());
                        None
                    }
                    Err(error) => Some(error),
                });
        if let Some(error) = stopped {
            eprintln!("controller script of {:08X} stopped ({error})", self.owner);
            self.controller_script = None;
        }
        let Some(body) = self.body.filter(|body| body.id == id) else {
            return Vec::new();
        };
        let speaker = body.id.entity;
        for request in requests {
            let due = tick.saturating_add(delay_ticks(request.delay));
            self.pending_voices.push((due, speaker, request));
        }
        // A line waits for its body; another body's never comes.
        self.pending_voices
            .retain(|&(_, waiting, _)| waiting == speaker);
        let mut due = Vec::new();
        self.pending_voices.retain(|(at, _, request)| {
            let now = *at <= tick;
            if now {
                due.push(request.clone());
            }
            !now
        });
        if criteria.is_empty() && due.is_empty() {
            return Vec::new();
        }
        let dead = self.death.is_some();
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
        let mut lines: Vec<VoiceLine> = criteria
            .into_iter()
            .filter(|criteria| !dead || is_death_cry(criteria.identifier))
            .filter(|criteria| hurt.speaks(criteria.identifier))
            .filter_map(|criteria| voice.answer(set, speaker, criteria, tick))
            .collect();
        if !dead {
            lines.extend(due.into_iter().filter_map(|request| {
                voice.say(set, speaker, request.stimulus, request.audience, tick)
            }));
        }
        lines
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
    fn takes_body_message(&self, target: u32) -> bool {
        self.world_started() && self.current_body() == Some(target) && self.body_script.is_some()
    }

    /// Whether this player's graphs are entity `entity`'s: his current
    /// body's or his controller's.
    pub fn runs(&self, entity: u32) -> bool {
        self.takes_body_message(entity)
            || self
                .controller_script
                .as_ref()
                .is_some_and(|script| script.entity() == entity)
    }

    /// The entities whose graphs this player runs now ([`Self::runs`]).
    pub fn script_entities(&self) -> Vec<u32> {
        let body = self
            .current_body()
            .filter(|&body| self.takes_body_message(body));
        let controller = self
            .controller_script
            .as_ref()
            .map(ControllerScript::entity);
        body.into_iter().chain(controller).collect()
    }

    /// A game message at `tick` ([`crate::messages`]) to the player's body
    /// or controller, whichever it names, or to both but its sender
    /// ([`EVERY_ENTITY`]). `players`: the room's controllers and teams, as
    /// the controller graph's filters know them.
    pub fn receive_message(&mut self, tick: u32, players: &[(u32, i64)], message: &GameMessage) {
        let controller = self
            .controller_script
            .as_ref()
            .map(ControllerScript::entity);
        if message.target == EVERY_ENTITY {
            if let Some(body) = self.current_body().filter(|&body| body != message.sender) {
                self.receive_body_message(&GameMessage {
                    target: body,
                    ..message.clone()
                });
            }
            if controller.is_some_and(|controller| controller != message.sender) {
                self.receive_controller_message(tick, players, message);
            }
        } else if !self.receive_body_message(message) && controller == Some(message.target) {
            self.receive_controller_message(tick, players, message);
        }
    }

    /// A persistent sender let its message to the player's body or
    /// controller go.
    pub fn end_message(&mut self, end: &MessageEnd) {
        if self.end_body_message(end) {
            return;
        }
        let owner = self.owner;
        let Some(script) = self
            .controller_script
            .as_mut()
            .filter(|script| script.entity() == end.target)
        else {
            return;
        };
        if let Err(error) = script.end_message(end.sender, end.message) {
            eprintln!("controller script of {owner:08X} stopped ({error})");
            self.controller_script = None;
        }
    }

    /// A message to the current body (match.py `deliver_body_messages`):
    /// its entries run now; a failure stops the body's script. False when
    /// the message is not for this body.
    fn receive_body_message(&mut self, message: &GameMessage) -> bool {
        if !self.takes_body_message(message.target) {
            return false;
        }
        let Some(script) = &mut self.body_script else {
            return false;
        };
        if let Err(error) = script.runtime.receive_message(
            message.sender,
            message.message,
            message.parameters.clone(),
        ) {
            self.drop_failed_script(&error.to_string());
        }
        true
    }

    /// A persistent sender let its message go for the current body
    /// (`lucy_script` `World::end_message`): its receiving states that
    /// held it go back to idle now; a failure stops the body's script.
    /// False when the end is not for this body.
    fn end_body_message(&mut self, end: &MessageEnd) -> bool {
        if !self.takes_body_message(end.target) {
            return false;
        }
        let Some(script) = &mut self.body_script else {
            return false;
        };
        if let Err(error) = script.runtime.end_message(end.sender, end.message) {
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
        self.push(Record::new(
            EntityUpdate {
                entity: self.owner,
                operation: Some(Operation::Update(vec![values.clone()])),
                ..EntityUpdate::default()
            },
            Delivery::Board {
                revision: self.board.revision,
            },
        ));
        self.board.sent = Some(values);
        self.remove_stale();
    }

    /// projectiles.py `Projectiles.update` for the player's shots: their
    /// flights, confirmations and releases at `tick`. What they ask of the
    /// room waits for [`crate::combat::deliver_projectiles`]. An
    /// explosion's push on his own body ([`FlightEnd::knockback`]) goes to
    /// his mover for its next posed frame, and the owner corrections carry
    /// it to his client.
    pub fn advance_projectiles(
        &mut self,
        tick: u32,
        mut step: impl FnMut(u32, &mut ProjectileFlight) -> io::Result<Option<FlightEnd>>,
    ) -> io::Result<()> {
        // His own explosion pushes him only where it could hurt him
        // ([`Self::can_be_damaged`]), and cancels the credit of another's
        // knock ([`KnockCredit::own_push`]).
        let pushable = self.can_be_damaged();
        let Some(script) = &mut self.body_script else {
            return Ok(());
        };
        let mover = &mut self.mover;
        let knocks = &mut self.knocks;
        let id = script.id;
        let mut deliveries = Vec::new();
        let projectiles = &mut script.projectiles;
        let events = &mut self.projectile_events;
        script.flights.retain_mut(|(token, flight)| {
            let result = step(id.entity, flight);
            if let (
                Ok(Some(FlightEnd {
                    knockback: Some(knockback),
                    ..
                })),
                Some(mover),
            ) = (&result, mover.as_mut().filter(|_| pushable))
            {
                mover.push(*knockback);
                knocks.own_push();
            }
            if let Some(token) = token.filter(|token| token.is_server()) {
                let exploded = match &result {
                    Ok(None) => return true,
                    Ok(Some(end)) => end.exploded.then(|| end.point.map(|value| value as f32)),
                    Err(_) => None,
                };
                events.push((id, ProjectileEvent::ServerEnded { token, exploded }));
                return false;
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
        deliveries.extend(projectiles.advance(tick));
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
    /// projectile, when this client has his body's statescript; and this
    /// player's own server projectile ([`ServerCopies`]), when his body's
    /// script fired it.
    pub(crate) fn show_projectile(
        &mut self,
        fired: &FiredProjectile,
        tick: u32,
        next_id: impl FnOnce() -> u32,
    ) {
        if !self.world_started() {
            return;
        }
        let own = self
            .body_script
            .as_ref()
            .filter(|script| fired.token.is_server() && script.id == fired.body);
        if let Some(script) = own {
            let has_base = script.stream.base().is_some();
            for (update, delivery) in self.server_copies.show(fired, has_base, tick, next_id) {
                self.push(Record::new(update, Delivery::ServerSeen(delivery)));
            }
            return;
        }
        for (update, delivery) in self.others.show_projectile(fired, tick, next_id) {
            self.push(Record::new(update, Delivery::Other(delivery)));
        }
    }

    pub(crate) fn take_projectile_events(&mut self) -> Vec<(BodyId, ProjectileEvent)> {
        std::mem::take(&mut self.projectile_events)
    }

    /// The player's server projectiles in flight that name an entity
    /// definition, where each is now ([`crate::spawned`] runs the graphs
    /// of those whose entity starts any).
    pub(crate) fn server_flights(&self) -> Vec<ServerFlight> {
        let Some(script) = &self.body_script else {
            return Vec::new();
        };
        let hero = self
            .body
            .filter(|body| body.id == script.id)
            .map_or(0, |body| body.hero);
        script
            .flights
            .iter()
            .filter_map(|(token, flight)| {
                let token = token.filter(|token| token.is_server())?;
                Some(ServerFlight {
                    shot: (script.id, token),
                    hero,
                    definition: flight.definition()?,
                    position: flight.position(),
                })
            })
            .collect()
    }

    /// The server projectile `token` of `body` removed itself (its graph,
    /// [`crate::spawned`]): its flight ends here, where it is, without an
    /// explosion; the match finishes its copies.
    pub(crate) fn end_server_flight(&mut self, body: BodyId, token: ProjectileToken) {
        if let Some(script) = self.body_script.as_mut().filter(|script| script.id == body) {
            script.flights.retain(|(flown, _)| *flown != Some(token));
        }
    }

    /// The graphs of the server projectile `shot` give it this speed scale
    /// ([`crate::spawned`]): its flight flies at it from its next step.
    pub(crate) fn set_server_flight_speed(&mut self, (body, token): ShotKey, scale: f64) {
        if let Some(script) = self.body_script.as_mut().filter(|script| script.id == body) {
            for (_, flight) in script
                .flights
                .iter_mut()
                .filter(|(flown, _)| *flown == Some(token))
            {
                flight.set_speed_scale(scale);
            }
        }
    }

    /// The skin theme of the player's body (match.py `Player.skin`).
    pub(crate) fn skin(&self) -> SkinTheme {
        self.body.map(|body| body.skin).unwrap_or_default()
    }

    /// His body's team bit (`scene::player_team_bit`); none without a body.
    pub(crate) fn team_bit(&self) -> Option<u64> {
        self.body.map(|body| body.team_bit)
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

    /// A server projectile's flight ended (`ProjectileEvent::ServerEnded`):
    /// this client's copy of it, another player's or its own, finishes.
    pub(crate) fn finish_server_projectile(
        &mut self,
        body: BodyId,
        token: ProjectileToken,
        tick: u32,
        exploded: Option<[f32; 3]>,
    ) {
        self.others
            .finish_server_projectile(body, token, tick, exploded);
        self.server_copies.finish(body, token, tick, exploded);
    }

    /// The graphs of a server projectile's entity have a frame
    /// ([`crate::spawned`]: a Biotic Orb's): this client's copy of it,
    /// another player's or its own, sends it once its create arrived
    /// ([`Self::advance_observed`]).
    pub(crate) fn refresh_server_projectile(&mut self, shot: ShotKey, frame: &PlacedFrame) {
        self.others.set_projectile_frame(shot, frame);
        self.server_copies.set_frame(shot, frame);
    }

    /// `_ObservedProjectile.advance` for this client's copies.
    pub(crate) fn advance_observed(&mut self, tick: u32) {
        let records = self.others.advance_projectiles(tick);
        for (update, delivery) in records {
            self.push(Record::new(update, Delivery::Other(delivery)));
        }
        for (update, delivery) in self.bot_views.advance_rockets(tick) {
            self.push(Record::new(update, Delivery::BotSeen(delivery)));
        }
        let source = self
            .body_script
            .as_ref()
            .map(|script| (script.id, script.stream.base().map(|(_, base)| base)));
        for (update, delivery) in self.server_copies.advance(tick, source) {
            self.push(Record::new(update, Delivery::ServerSeen(delivery)));
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
            self.push(Record::new(update, Delivery::BotSeen(delivery)));
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
            self.push(Record::new(update, Delivery::Field(delivery)));
        }
    }

    /// The field graph's frame changed: the field's copies get it
    /// (`FieldArrived.send`).
    pub(crate) fn refresh_field(&mut self, shot: ShotKey, frame: &PlacedFrame) {
        match self.fields.refresh(shot, frame) {
            Ok(records) => {
                for (update, delivery) in records {
                    self.push(Record::new(update, Delivery::Field(delivery)));
                }
            }
            Err(error) => eprintln!("a Biotic Field frame failed ({error})"),
        }
    }

    /// The field's graph removed it: its shot ends (the canister goes)
    /// when it is this player's.
    pub(crate) fn end_field(&mut self, (body, token): ShotKey) {
        let Some(script) = self.body_script.as_mut().filter(|script| script.id == body) else {
            return;
        };
        let Some(projectiles) = &mut script.projectiles else {
            return;
        };
        let deliveries = projectiles.end_field(token);
        if let Err(error) = self.queue_projectiles(body, deliveries) {
            eprintln!("a Biotic Field's canister did not go ({error})");
        }
    }

    /// `_end`: the fields of the shots that no longer run go.
    pub(crate) fn end_fields(&mut self, live: &[ShotKey]) {
        let records = self.fields.end(|shot| live.contains(&shot));
        if records.is_empty() {
            return;
        }
        for (update, delivery) in records {
            self.push(Record::new(update, Delivery::Field(delivery)));
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
            self.push(Record::new(update, delivery));
        }
        self.remove_stale();
        Ok(())
    }

    /// One unit per physics pose, unposed VM frame, pre-body command, or queued game message.
    /// A fresh frame is committed only after its final pose has completed.
    /// `combat`: the room's, whose history the body's searches that rewind
    /// see the bodies by (none yet: they can't tell).
    pub fn advance_input(
        &mut self,
        scene: &PreparedScene,
        viewer: &PlayerAppearance<'_>,
        quanta: u32,
        budget: usize,
        combat: Option<&Combat>,
    ) -> io::Result<(usize, bool)> {
        let dt = f64::from(quanta) / 1_000_000.0;
        let rewind = combat.map(|combat| combat.rewind(scene, quanta));
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
                    let (used, complete) = self.advance_command(
                        scene,
                        command,
                        &mut pending,
                        dt,
                        remaining,
                        rewind.as_ref(),
                    )?;
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
        rewind: Option<&TargetRewind<'_>>,
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
                if let Err(error) = script.advance(mover, held, step, rewind, dead) {
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
                    if let Err(error) =
                        script.advance(mover, command, BodyStep::Posed, rewind, true)
                    {
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
        let quanta = (dt * 1_000_000.0).round() as u32;
        // A jump or a landing cancels another's knock ([`KnockCredit`]).
        let knocks = &mut self.knocks;
        let stepped =
            mover.step_pending_with(&mut pending.poses, budget - used, |mover, command| {
                // The idle-animated placeables' parts where this client had
                // them as it predicted the command.
                scene.pose_idle_for(&command, quanta);
                tracing::debug!(
                    target: "lucy::pose",
                    frame = command.frame,
                    forward = command.forward,
                    right = command.right,
                    yaw = command.yaw,
                    pitch = command.pitch,
                    buttons = format_args!("{:#x}", command.buttons),
                    extra = command.extra,
                    subframe = command.subframe(),
                    "command"
                );
                let Some(script) = script.as_mut().filter(|_| failed.is_none()) else {
                    mover.step_pose_to(rays, command, dt, None)?;
                    knocks.posed(mover.landed(), mover.jumped());
                    return Ok(());
                };
                // match.py `_step_body_pose`: one world-target state moves the
                // body; arbitration between several is unresolved, so none does.
                let requests = script.runtime.move_requests();
                let request = match requests[..] {
                    [request] => Some(request),
                    _ => None,
                };
                let key = request.map(|request| MoveKey {
                    instance: request.instance,
                    raw_state: request.raw_state,
                    activation: request.activation,
                });
                let outcome = mover.step_pose_to(
                    rays,
                    command,
                    dt,
                    key.zip(request.as_ref())
                        .map(|(key, request)| (key, &request.params)),
                )?;
                knocks.posed(mover.landed(), mover.jumped());
                let ended = request.zip(outcome).map_or(Ok(()), |(request, end)| {
                    let arrived = end == MoveEnd::Arrived;
                    script.runtime.finish_move(request, arrived, command.frame)
                });
                let advanced = ended
                    .and_then(|()| script.advance(mover, command, BodyStep::Posed, rewind, false));
                match advanced {
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

    /// A new body for the player (match.py `switch_hero`). A pick closes
    /// hero select with a full controller frame (unless the heroes are still
    /// assembling). A body placed at the spawn point (not a pick of a living
    /// body, which changes the hero where he stands) turns the view to the
    /// spawn's facing with 25008 after its possession
    /// ([`Self::take_spawn_view`]).
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
        let mover = match (here, self.placement) {
            (Some(here), _) => {
                scene.player_mover_at(hero.body, viewer.team, here.position, here.yaw)?
            }
            (None, Some((position, yaw))) => {
                scene.player_mover_at(hero.body, viewer.team, position, yaw)?
            }
            (None, None) => {
                scene.player_spawn_mover(hero.body, viewer.team, viewer.spawn, viewer.room)?
            }
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
        let mut script = scene
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
                    mode_random: scene.mode_random(),
                    ai: self.ai_control(scene.graphs(), hero.guid),
                    respawn_held: self.mode_held.contains(&RESPAWN_MESSAGE),
                },
            )
            .ok()
            .flatten()
            .and_then(|mut runtime| {
                runtime.set_target_candidates(&self.target_candidates);
                runtime.set_pickups(&self.pickups);
                runtime.set_map_variables(&self.map_variables);
                // The body starts with the defaults; what the client sent
                // replaces them.
                let settings = hero_setting_variables(&self.hero_settings, hero.guid);
                if settings != hero_setting_variables(&HeroSettings::default(), hero.guid) {
                    runtime.set_entity_variables(&settings).ok()?;
                }
                Some(runtime)
            })
            .and_then(|runtime| BodyScript::new(body, start_frame, runtime).ok());
        // The tags its states took as it started go in its create, as the
        // other clients' creates of it carry them (`Others::ensure_body`).
        let tags = script
            .as_mut()
            .and_then(|script| script.runtime.take_target_tags_update());
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
                tags: tags.clone(),
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
        let placed = here.is_none() && !self.headless;

        self.generation = generation;
        self.bodies.insert(entity, (generation, Lifetime::Live));
        self.created.remove(&entity);
        let old = self.body.replace(body);
        self.shield.0 = false;
        self.cancel_body_script();
        self.body_script = script;
        if let Some(tags) = tags {
            self.tag_revision += 1;
            self.body_tags = Some((id, self.tag_revision, tags));
        }
        // `01C7` rid32 sets #1928e on `002D.025` {`0029`}: unless that
        // identifier is `018D`, `0016` or `0018`, the body's `01CA` says
        // elimination lines and `0215` may count kill streaks. Retail's
        // server sends it (no graph does but `1225`); the identifier is its
        // `STU_ABB6C23E`, whose client evaluator writes 0, and 0 here
        // (assumption: the server's game identifier is none of the three).
        // A body made in overtime reads it from the start.
        let overtime = self.overtime;
        let failure = self.body_script.as_mut().and_then(|script| {
            overtime
                .then(|| script.runtime.set_mode_overtime(true))
                .transpose()
                .and_then(|_| {
                    script.runtime.receive_message(
                        0,
                        GAME_IDENTIFIER,
                        Variables::from([(0x29, ScriptValue::Asset(0))]),
                    )
                })
                .err()
        });
        if let Some(error) = failure {
            self.drop_failed_script(&error.to_string());
        }
        self.mover = Some(mover);
        self.teleported = None;
        self.bound = None;
        // combat.py `retire_player_body`: the old body's death and pools go
        // with it.
        self.death = None;
        self.plain_vitals = None;
        self.respawn_hero = None;
        self.spawn_room = None;
        self.spawn_view = placed.then_some((id, movement.yaw));
        if let Some(controller) = &mut self.controller_script {
            controller.possessed(entity, hero.guid);
        }
        // A switch from one hero to another, not the first pick: `037F.025`
        // to the player's controller script, whose `0C90` rid220 keeps it
        // for the chat line (`crate::notices`). Other clients take it from
        // this player's view.
        if let Some(old) = old.filter(|old| old.hero != hero.guid) {
            let kept = self
                .controller_script
                .as_mut()
                .map(|controller| controller.hero_switched(tick, old.hero, hero.guid, self.owner));
            match kept {
                Some(Ok(Some(kept))) => {
                    self.switches += 1;
                    let switch = Switch {
                        kept,
                        serial: self.switches,
                    };
                    self.switch = Some(switch);
                    if scene.switch_notices() && self.controller.latest_data() != 0 {
                        self.notices.waiting.push_back(switch);
                    }
                }
                Some(Err(error)) => {
                    eprintln!("controller script of {:08X} stopped ({error})", self.owner);
                    self.controller_script = None;
                }
                Some(Ok(None)) | None => {}
            }
        }
        self.push(Record::new(create, Delivery::CreateBody(id)));
        self.push(Record::new(
            EntityUpdate {
                entity: self.owner,
                operation: Some(Operation::Update(possession.components().into())),
                ..EntityUpdate::default()
            },
            Delivery::Possession(id),
        ));
        if let Some(old) = old {
            self.retire_body(old.id);
        }
        // A pick's frame names the new hero (#940e); `288B` closes the
        // screen a frame later, once the body lives (st95), in a frame of
        // its own ([`Self::send_hero_select`]).
        if pick {
            self.queue_controller(scene, tick, None)?;
        }
        tracing::debug!(
            target: "lucy::join",
            tick,
            body = format_args!("{entity:08X}"),
            hero = format_args!("{:016X}", hero.guid),
            pick,
            "body switch queued"
        );
        self.remove_stale();
        Ok(())
    }

    fn retire_body(&mut self, id: BodyId) {
        self.created.remove(&id.entity);
        self.health_updates.remove(&id.entity);
        self.tag_updates.remove(&id.entity);
        self.bodies
            .insert(id.entity, (id.generation, Lifetime::Retiring));
        self.push(Record::new(
            EntityUpdate {
                entity: id.entity,
                operation: Some(Operation::Destroy),
                ..EntityUpdate::default()
            },
            Delivery::DestroyBody(id),
        ));
    }

    /// A full controller frame projected from the controller script
    /// (`crate::controller`), with a hero switch notice's variables when
    /// `notice` is given: a separate event, even when the menu is already
    /// closed. Nothing without a script; a frame the projection can't write
    /// (a gap the script met, in a debug build) is reported and not sent.
    fn queue_controller(
        &mut self,
        scene: &PreparedScene,
        tick: u32,
        notice: Option<HeroSwitch>,
    ) -> io::Result<()> {
        self.advance_controller(tick);
        let Some((shown, input)) = self.select_input(scene.controller().practice()) else {
            return Ok(());
        };
        let command_frame = self.command_ack(tick);
        let Some(script) = &self.controller_script else {
            return Ok(());
        };
        self.select_shown = Some(shown);
        let controller = scene.controller();
        let projections = controller
            .controller_projection(script, input)
            .and_then(|plain| {
                let noticed = notice
                    .map(|notice| {
                        controller.controller_projection(
                            script,
                            ControllerFrame {
                                notice: Some(notice),
                                ..input
                            },
                        )
                    })
                    .transpose()?;
                Ok((plain, noticed))
            });
        let chunk = projections.and_then(|(plain, noticed)| {
            let chunk = self.controller.full_owner(command_frame, |cmfd| {
                controller.encode_controller(script, noticed.as_ref().unwrap_or(&plain), cmfd)
            })?;
            self.controller_sent = Some(plain);
            Ok(chunk)
        });
        let chunk = match chunk {
            Ok(chunk) => chunk,
            Err(error) => {
                eprintln!("controller frame of {:08X} not sent ({error})", self.owner);
                return Ok(());
            }
        };
        tracing::debug!(
            target: "lucy::join",
            tick,
            last = chunk.last,
            open = shown.open,
            pick = format_args!("{:016X}", shown.pick.unwrap_or_default()),
            hero = format_args!("{:016X}", shown.hero.unwrap_or_default()),
            notice = notice.is_some(),
            "controller frame queued"
        );
        let close = shown.hero.is_some() && !shown.open && notice.is_none();
        self.push(Record::script(chunk, close));
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

/// Hand a body's new dashes, rewinds and velocity actions to its mover;
/// they run from the next pose on (`lucy_physics::mover::motion`,
/// [`Mover::act_on_velocity`]). Players', AI heroes' and training bots'
/// bodies alike ([`crate::bots::Bot::start_motions`]).
pub(crate) fn start_motions(mover: &mut Mover, motions: Vec<Motion>, dt: f64) {
    for motion in motions {
        match motion {
            Motion::Dash { distance, seconds } => mover.dash(distance, seconds),
            Motion::Rewind { seconds, duration } => mover.rewind(seconds, duration, dt),
            Motion::EndRewind { finished } => mover.end_rewind(finished),
            Motion::Velocity(element) => mover.act_on_velocity(element),
        }
    }
}

fn invalid(error: impl std::fmt::Display) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error.to_string())
}

/// A 21616 parameter as the VM's value: an array of one value is that
/// value ([`WorldDelivery::instance_message`]).
fn parameter_value(value: ParameterValue) -> ScriptValue {
    match value {
        ParameterValue::Null => ScriptValue::Null,
        ParameterValue::Bool(flag) => ScriptValue::Bool(flag),
        ParameterValue::Integer(number) => ScriptValue::Integer(number),
        ParameterValue::Float(number) => ScriptValue::Float(f64::from(number)),
        ParameterValue::Asset(guid) => ScriptValue::Asset(guid),
        ParameterValue::Vector([x, y, z]) => {
            ScriptValue::Vector([f64::from(x), f64::from(y), f64::from(z)])
        }
        ParameterValue::Entity(entity) => ScriptValue::Entity(entity),
        ParameterValue::Array(mut items) if items.len() == 1 => {
            parameter_value(items.pop().expect("one item"))
        }
        ParameterValue::Array(items) => {
            ScriptValue::Array(items.into_iter().map(parameter_value).collect())
        }
    }
}

mod parts;
#[cfg(test)]
mod tests;
