//! The server-owned map placeables' scripts in a match, and what each
//! client gets of them.
//!
//! The room starts the scripts of the scene's server placeables
//! ([`PreparedScene::server_placeables`]) at its first tick in the
//! client's order (`orpheus_script::placed`), lets their target searches
//! find the players, and advances them every tick. A complete script's
//! observer frame goes to each client once that client holds the
//! placeable's create, and again whenever it changes, as a full frame on
//! the placeable's own script stream (the mode entity's pattern). A
//! script with gaps (classes the VM refuses in debug builds, or answers
//! by assumption) or a fault sends no frames; its gaps are logged once.
//! A script that hasn't faulted still sends its messages (the gap log
//! names what it assumed); the spawn rooms' are what the bodies need, and
//! their searches assume nothing (`orpheus_script` `vm/targets.rs`).
//!
//! The spawn rooms (`0D62`) wait for the mode graph's `0724`; the server
//! sends it in the mode graph's place to the rooms the mode holds now
//! ([`crate::map_areas::room_holds`]), and lets it go for a room it no
//! longer holds. A room then sends `0723` to its team's living heroes
//! inside (its target search and their area, `STU_73E1C5F0`), which their
//! mode body graph `0C8E` turns into the spawn room's invulnerability and
//! heal, and lets it go when they leave. The messages of running scripts
//! go to the match ([`PlacedScripts::take_messages`]), which routes them
//! with every other entity's ([`crate::messages`]); the scripts take the
//! messages sent to them or to every entity
//! ([`PlacedScripts::receive_message`]).
//!
//! [`PreparedScene::server_placeables`]: crate::scene_assets::PreparedScene::server_placeables

use std::{
    collections::{BTreeMap, HashMap},
    sync::Arc,
};

use orpheus_core::bits::BitWriter;
use orpheus_script::{
    codecs::CodecTable,
    graph::GraphStore,
    map_tags::MapTags,
    mode::EVERY_ENTITY,
    placed::{DummyChange, EntityVolumes, MapEntityWorld, NextDummy},
    snapshot::{ProjectedSnapshot, SnapshotEncoder, View},
    stream::{ScriptChunk, Stream, StreamError},
    vm::{GameMessage, MapRays, MessageEnd, TargetCandidate, Unknowns},
};
use orpheus_wire::entity::GAME_MODE_ENTITY;

use crate::map_areas::{PlacedScript, ROOM_MESSAGE, RoomVolumes, SpawnRoom, room_message};

/// A placeable's latest frame: its full frame, and a serial that counts
/// its changes.
#[derive(Clone, Debug)]
pub struct PlacedFrame {
    pub serial: u64,
    pub bits: Arc<BitWriter>,
}

/// The scripts of one match's server placeables.
pub struct PlacedScripts {
    graphs: Arc<GraphStore>,
    codecs: CodecTable,
    mode: u64,
    /// The match's mode entity value ([`crate::scene::mode_create`]).
    mode_random: f32,
    placed: Vec<Arc<PlacedScript>>,
    /// The map's tagged entities in the mode, for the tag queries.
    map_tags: Arc<MapTags>,
    /// The map's collision, for the scripts' ray casts.
    map_rays: Arc<dyn MapRays>,
    rooms: Arc<[SpawnRoom]>,
    world: Option<MapEntityWorld>,
    /// The rooms that hold the mode's `0724`, with the team it gave each.
    held: BTreeMap<u32, Option<u8>>,
    /// The complete scripts' messages and ends for the match to route.
    messages: Vec<GameMessage>,
    ends: Vec<MessageEnd>,
    /// The last projection and frame of each complete script.
    frames: BTreeMap<u32, (ProjectedSnapshot, PlacedFrame)>,
    /// The player and body the next dummy bot gets, as the match gave.
    next_dummy: Option<NextDummy>,
    /// The running scripts' dummy bot changes for the match to carry out.
    dummies: Vec<DummyChange>,
    gaps_logged: bool,
}

impl PlacedScripts {
    pub fn new(
        graphs: Arc<GraphStore>,
        mode: u64,
        mode_random: f32,
        placed: Vec<Arc<PlacedScript>>,
        map_tags: Arc<MapTags>,
        map_rays: Arc<dyn MapRays>,
        rooms: Arc<[SpawnRoom]>,
    ) -> Self {
        Self {
            graphs,
            codecs: CodecTable::client(),
            mode,
            mode_random,
            placed,
            map_tags,
            map_rays,
            rooms,
            world: None,
            held: BTreeMap::new(),
            messages: Vec::new(),
            ends: Vec::new(),
            frames: BTreeMap::new(),
            next_dummy: None,
            dummies: Vec::new(),
            gaps_logged: false,
        }
    }

    pub fn world(&self) -> Option<&MapEntityWorld> {
        self.world.as_ref()
    }

    /// The bodies the placeables' target searches find from now on.
    pub fn set_target_candidates(&mut self, candidates: &[TargetCandidate]) {
        if let Some(world) = &mut self.world {
            world.set_players(candidates.to_vec());
        }
    }

    /// Start the scripts at the first call, give the rooms the mode holds
    /// (`holds`, [`crate::map_areas::room_holds`]) the mode's `0724` and
    /// let it go for the others, then advance them to `tick`, keep the
    /// running ones' messages and project the complete ones' frames.
    pub fn advance(&mut self, tick: u32, holds: &[(u32, Option<u8>)]) {
        let world = match &mut self.world {
            Some(world) => {
                world.set_next_dummy(self.next_dummy);
                world.advance(tick);
                world
            }
            None => {
                let world = self.world.insert(MapEntityWorld::start(
                    Arc::clone(&self.graphs),
                    self.mode,
                    self.mode_random,
                    Unknowns::for_build(),
                    [],
                    Some(Arc::clone(&self.map_tags)),
                    Some(Arc::clone(&self.map_rays)),
                    Some(Arc::new(RoomVolumes(Arc::clone(&self.rooms))) as Arc<dyn EntityVolumes>),
                    tick,
                ));
                world.set_next_dummy(self.next_dummy);
                world.place(self.placed.iter().map(|placed| placed.placement()));
                world
            }
        };
        let gone: Vec<u32> = self
            .held
            .iter()
            .filter(|&(room, team)| !holds.contains(&(*room, *team)))
            .map(|(&room, _)| room)
            .collect();
        for room in gone {
            self.held.remove(&room);
            world.end_message(room, GAME_MODE_ENTITY, ROOM_MESSAGE);
        }
        for &(room, team) in holds {
            if self.held.contains_key(&room) {
                continue;
            }
            world.receive_message(room, GAME_MODE_ENTITY, ROOM_MESSAGE, &room_message(team));
            self.held.insert(room, team);
        }
        // A running script's messages count (the gap log lists what it
        // assumes); only complete scripts send frames.
        let running = |world: &MapEntityWorld, sender| {
            world.entity(sender).is_some_and(|placed| placed.running())
        };
        let messages = world.take_messages();
        self.messages.extend(
            messages
                .into_iter()
                .filter(|message| running(world, message.sender)),
        );
        let ends = world.take_message_ends();
        self.ends
            .extend(ends.into_iter().filter(|end| running(world, end.sender)));
        self.take_dummies();
        let world = self.world.as_mut().expect("started above");
        if !self.gaps_logged {
            self.gaps_logged = true;
            let gaps = world.gaps();
            if !gaps.is_empty() {
                eprintln!(
                    "{} of {} placed scripts have gaps and send no frames (their messages still go): {gaps:?}",
                    world
                        .entities()
                        .iter()
                        .filter(|placed| !placed.complete())
                        .count(),
                    world.entities().len()
                );
            }
        }
        let world = self.world.as_ref().expect("started above");
        let encoder =
            SnapshotEncoder::new(&self.graphs, &self.codecs).with_unknowns(world.unknowns());
        for placed in world.entities() {
            let snapshot = match placed.snapshot() {
                Some(snapshot) if placed.complete() => snapshot,
                _ => {
                    self.frames.remove(&placed.entity);
                    continue;
                }
            };
            let projected = match encoder.project(snapshot, View::Observer) {
                Ok(projected) => projected,
                Err(error) => {
                    eprintln!(
                        "placed entity {:08X}'s frame does not project ({error:?})",
                        placed.entity
                    );
                    self.frames.remove(&placed.entity);
                    continue;
                }
            };
            if self
                .frames
                .get(&placed.entity)
                .is_some_and(|(known, _)| *known == projected)
            {
                continue;
            }
            match encoder.full(&projected) {
                Ok(bits) => {
                    let serial = self
                        .frames
                        .get(&placed.entity)
                        .map_or(1, |(_, frame)| frame.serial + 1);
                    let frame = PlacedFrame {
                        serial,
                        bits: Arc::new(bits),
                    };
                    self.frames.insert(placed.entity, (projected, frame));
                }
                Err(error) => {
                    eprintln!(
                        "placed entity {:08X}'s frame does not encode ({error:?})",
                        placed.entity
                    );
                    self.frames.remove(&placed.entity);
                }
            }
        }
    }

    /// The running scripts' game messages and message ends since the last
    /// call, for the match to route ([`crate::messages`]).
    pub fn take_messages(&mut self) -> (Vec<GameMessage>, Vec<MessageEnd>) {
        (
            std::mem::take(&mut self.messages),
            std::mem::take(&mut self.ends),
        )
    }

    /// The player and first body the next dummy bot the scripts make gets
    /// (`STU_FC3921B1`, the Training Partner console `265F`); None: they
    /// can make none now. The match sets it before each run.
    pub fn set_next_dummy(&mut self, next: Option<NextDummy>) {
        self.next_dummy = next;
        if let Some(world) = &mut self.world {
            world.set_next_dummy(next);
        }
    }

    /// The dummy bots the running scripts made or removed since the last
    /// call, in order, for the match to make or remove.
    pub fn take_dummy_changes(&mut self) -> Vec<DummyChange> {
        self.take_dummies();
        std::mem::take(&mut self.dummies)
    }

    /// The world's dummy bot changes go to the match.
    fn take_dummies(&mut self) {
        let Some(world) = &mut self.world else {
            return;
        };
        let changes = world.take_dummy_changes();
        if changes
            .iter()
            .any(|change| matches!(change, DummyChange::Create { .. }))
        {
            // The bot took the player and body it was given.
            self.next_dummy = None;
        }
        self.dummies.extend(changes);
    }

    /// Whether a running script runs the placed entity `entity`.
    pub fn runs(&self, entity: u32) -> bool {
        self.world
            .as_ref()
            .and_then(|world| world.entity(entity))
            .is_some_and(|placed| placed.running())
    }

    /// A game message to the placed entity it names, or to every running
    /// script but its sender's ([`EVERY_ENTITY`]): their entries run now;
    /// what they send goes with the next tick's messages.
    pub fn receive_message(&mut self, message: &GameMessage) {
        let Some(world) = &mut self.world else {
            return;
        };
        if message.target == EVERY_ENTITY {
            world.receive_everywhere(message.sender, message.message, &message.parameters);
        } else {
            world.receive_message(
                message.target,
                message.sender,
                message.message,
                &message.parameters,
            );
        }
    }

    /// A persistent sender let its message to the placed entity
    /// `end.target` go.
    pub fn end_message(&mut self, end: &MessageEnd) {
        if let Some(world) = &mut self.world {
            world.end_message(end.target, end.sender, end.message);
        }
    }

    /// Each complete script's latest frame, by entity.
    pub fn frames(&self) -> impl Iterator<Item = (u32, &PlacedFrame)> {
        self.frames
            .iter()
            .map(|(&entity, (_, frame))| (entity, frame))
    }
}

/// One placeable's script stream on one client.
#[derive(Debug)]
struct PlacedView {
    stream: Stream,
    /// The serial of the frame last queued.
    serial: u64,
}

/// Every placeable's script stream on one client.
#[derive(Debug, Default)]
pub(crate) struct PlacedViews {
    views: HashMap<u32, PlacedView>,
}

impl PlacedViews {
    pub(crate) fn clear(&mut self) {
        self.views.clear();
    }

    /// The chunks of the frames this client doesn't have yet, for the
    /// placeables whose create it holds.
    pub(crate) fn update<'a>(
        &mut self,
        frames: impl Iterator<Item = (u32, &'a PlacedFrame)>,
        created: impl Fn(u32) -> bool,
    ) -> Result<Vec<ScriptChunk>, StreamError> {
        let mut out = Vec::new();
        for (entity, frame) in frames {
            if !created(entity) {
                continue;
            }
            let view = self.views.entry(entity).or_insert_with(|| PlacedView {
                stream: Stream::new(entity),
                serial: 0,
            });
            if view.serial == frame.serial {
                continue;
            }
            out.push(view.stream.full(&frame.bits)?);
            view.serial = frame.serial;
        }
        Ok(out)
    }

    /// A chunk is current while it holds the placeable's newest frame.
    pub(crate) fn current(&self, entity: u32, last: u32) -> bool {
        self.views
            .get(&entity)
            .is_some_and(|view| view.stream.latest_data() == last)
    }

    pub(crate) fn arrived(&mut self, entity: u32, last: u32) {
        if let Some(view) = self.views.get_mut(&entity) {
            view.stream.arrived(last);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        messages::{HeldReach, deliver_held},
        mode_script::ModeRunner,
        scene_assets::{PreparedScene, SceneAssets},
    };
    use orpheus_content::maps::{GameMap, SpawnChoice};
    use orpheus_script::wire::Variables;

    /// The Practice Range as the room runs it (`game_service.rs` `tick`,
    /// `update_flows`, `route_messages`): `13C0` from the room's first
    /// tick, the placed scripts with no stand-in `0724`, then the mode
    /// graph's held broadcasts to every placed script.
    struct Range {
        scene: PreparedScene,
        mode: ModeRunner,
        reach: HeldReach,
    }

    impl Range {
        fn start(assets: &SceneAssets, tick: u32) -> Self {
            let scene = assets.prepare(GameMap::practice_range()).unwrap();
            let mode = ModeRunner::start(&scene, tick, Vec::new());
            assert!(mode.graph_driven(), "{:?}", mode.gaps());
            Self {
                scene,
                mode,
                reach: HeldReach::default(),
            }
        }

        fn advance(&mut self, tick: u32) {
            self.scene.advance_placed(tick, &[]);
            self.mode.advance(tick, &Variables::new(), Vec::new());
            let held = self.mode.held().to_vec();
            let (_, placed) = self.scene.message_recipients(tick);
            deliver_held(&held, &mut self.reach, &mut [placed]);
        }
    }

    fn frame(serial: u64) -> PlacedFrame {
        let mut bits = BitWriter::new();
        bits.bits(serial, 8).unwrap();
        PlacedFrame {
            serial,
            bits: Arc::new(bits),
        }
    }

    /// A placeable's frame goes to a client only once its create arrived,
    /// once per change, and only its newest chunk stays current.
    #[test]
    fn a_placed_frame_waits_for_the_create_and_goes_once_per_change() {
        const DOOR: u32 = 0x8000_0166;
        let mut views = PlacedViews::default();
        let first = frame(1);
        assert!(
            views
                .update([(DOOR, &first)].into_iter(), |_| false)
                .unwrap()
                .is_empty()
        );
        let sent = views
            .update([(DOOR, &first)].into_iter(), |_| true)
            .unwrap();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].last, 1);
        assert!(views.current(DOOR, 1));
        assert!(
            views
                .update([(DOOR, &first)].into_iter(), |_| true)
                .unwrap()
                .is_empty()
        );
        let second = frame(2);
        let sent = views
            .update([(DOOR, &second)].into_iter(), |_| true)
            .unwrap();
        assert_eq!(sent[0].last, 2);
        assert!(!views.current(DOOR, 1));
        assert!(views.current(DOOR, 2));
    }

    /// The Practice Range's 26 server placeables, with `13C0` running: 22
    /// have graphs; the six
    /// barriers `00F6`, the three spawn doors `0D32` (358, 479, 480), the
    /// six spawn markers `01CE` (137, 150, 243, 415, 482, 590: they give
    /// themselves their team, `STU_0F647E55`, and take `13C0`'s `0724`,
    /// st2, whose st1 tags each marker `0035`) and the spawn room `0D62`
    /// (126, whose target searches `13C0`'s `0724` begins: every field
    /// they set is understood, `vm/targets.rs`) run whole and have frames,
    /// as do the four whose graphs `0216`/`0217` end at once and the pad
    /// `26F8` (586: the Training Partner setting, ruleset 29450, is on, so
    /// it keeps itself); the console `265F` still meets gaps and sends
    /// nothing. Each door's frame changes twice in its first 300 frames: as
    /// it takes `13C0`'s `0702` (st12, remote slot 7, writes its remote sync
    /// variables #8869 and #8870 from `03C5` and `03C4`) and by its alias
    /// states (see `0D32`; which wait ends first is not pinned here).
    #[test]
    #[ignore = "requires the derived 104319 data"]
    fn the_range_placeables_run_their_graphs_and_have_frames() {
        let assets = SceneAssets::load(orpheus_testkit::runtime_dir()).unwrap();
        let mut range = Range::start(&assets, 100);
        assert_eq!(range.scene.server_placeables().len(), 26);
        for tick in 100..400 {
            range.advance(tick);
        }
        let scene = &range.scene;
        let world = scene.placed_scripts().world().unwrap();
        assert_eq!(world.entities().len(), 22);
        let room = world.entity(0x8000_007E).unwrap();
        assert!(room.running() && room.complete(), "{:?}", world.gaps());
        let framed: Vec<u32> = scene
            .placed_scripts()
            .frames()
            .map(|(entity, _)| entity & 0x7FFF_FFFF)
            .collect();
        assert_eq!(
            framed,
            [
                126, 137, 150, 185, 188, 199, 204, 243, 340, 358, 398, 400, 415, 416, 424, 479,
                480, 482, 548, 586, 590
            ]
        );
        for door in [358, 479, 480] {
            let (_, frame) = scene
                .placed_scripts()
                .frames()
                .find(|(entity, _)| *entity == 0x8000_0000 | door)
                .unwrap();
            assert_eq!(frame.serial, 3, "door {door}");
        }
        let gaps = world.gaps();
        assert!(
            gaps.keys()
                .all(|(graph, _)| ![0x01ce, 0x0d62].contains(graph)),
            "{gaps:?}"
        );
    }

    /// The Practice Range's Training Partner console `265F` (267), with
    /// `13C0` running: at its
    /// start it finds its pad `3992` (586, tag `09B7`) and takes the pad's
    /// position and facing (`STU_71A87E92`/`STU_2200A66D`, rid6), reads
    /// the Training Partner setting (ruleset 29450 of the mode entity,
    /// true in `0060.0C6`), so the pad keeps itself (`26F8`), and asks for
    /// its default partner: it removes the dummy bot in red slot 0
    /// (`STU_C1561031`, rid34) and makes a Tracer (`0003.075`, rid36's
    /// #940) there (`STU_FC3921B1`, rid35), whose body goes to #29454.
    /// The facing is the pad's rotation (x y z w) `[0, 0.9528117, 0,
    /// 0.3035622]` turned +z, (2wy, 0, 1 − 2y²) = (0.578470, 0, −0.815704),
    /// as 1/65536 turns (`World::entity_facing`), within 1e-4.
    #[test]
    #[ignore = "requires the derived 104319 data"]
    fn the_range_console_makes_its_training_partner_on_its_pad() {
        const CONSOLE: u32 = 0x8000_010B;
        const PAD: u32 = 0x8000_024A;
        const TRACER: u64 = 0x02E0_0000_0000_0003;
        let next = NextDummy {
            player: 0xA000_0200,
            body: 0xA000_0201,
        };
        let assets = SceneAssets::load(orpheus_testkit::runtime_dir()).unwrap();
        let mut range = Range::start(&assets, 100);
        range.scene.set_next_dummy(Some(next));
        range.advance(100);
        let changes = range.scene.take_dummy_changes();
        let world = range.scene.placed_scripts().world().unwrap();
        assert!(world.entity(PAD).is_some(), "{:?}", world.gaps());
        let [
            DummyChange::Remove { team: 1, slot: 0 },
            DummyChange::Create { dummy, next: made },
        ] = changes.as_slice()
        else {
            panic!("{changes:?} {:?}", world.gaps());
        };
        assert_eq!(*made, next);
        assert_eq!((dummy.hero, dummy.team, dummy.slot), (TRACER, 1, 0));
        assert_eq!(dummy.position, [34.24001, 1.9917264, 22.972233]);
        let forward = [0.578_470, 0.0, -0.815_704];
        assert!(
            dummy
                .facing
                .iter()
                .zip(forward)
                .all(|(got, want)| (got - want).abs() < 1e-4),
            "{:?}",
            dummy.facing
        );
        let console = world.entity(CONSOLE).unwrap().snapshot().unwrap();
        let partner = console
            .instances
            .values()
            .find_map(|instance| instance.variables.get(&29454));
        assert_eq!(
            partner,
            Some(&orpheus_script::wire::Value::Entity(next.body))
        );
        // One bot a player and body: the next needs new ones.
        range.advance(101);
        assert!(range.scene.take_dummy_changes().is_empty());
    }

    /// The Practice Range's spawn room `0D62` (126) takes `13C0`'s `0724`
    /// (st9, held to every entity: invulnerable, heal and hero select, no
    /// team), the only `0724` there is,
    /// and sends `0723` with its three flags to a living blue hero whose
    /// feet are in its area (`STU_73E1C5F0`), not to a red one, a dead one
    /// or one outside; it lets it go when he walks out. Its two searches
    /// (st0, rid 0, and st3, rid 4) search every 12 frames (`m_DD792FF0`
    /// 3): on the ticks where `(tick + h) as i32 % 12 == 0`, `h` the FNV-1a
    /// hash of the bytes of `0x8000_007E ^ rid`: for st0 `0xA9DA_B4EB`,
    /// -1445284629 as i32 (-1445284629 + 105 = -12 × 120440377: ticks 105,
    /// 117, 129, …), for st3 `0xEA8B_B12F`, -359943889 (-359943889 + 97 =
    /// -12 × 29995316: ticks 97, 109, 121, …). The room's frame goes out
    /// from the first tick, and again when its searches change it.
    #[test]
    #[ignore = "requires the derived 104319 data"]
    fn the_range_spawn_room_holds_the_heroes_of_its_team_inside() {
        const ROOM: u32 = 0x8000_007E;
        const HELD: u64 = 0x0240_0000_0000_0723;
        let assets = SceneAssets::load(orpheus_testkit::runtime_dir()).unwrap();
        let map = GameMap::practice_range();
        let (spawn, _) = map.spawn_point_in(0, 0, SpawnChoice::First).unwrap();
        let mut range = Range::start(&assets, 100);
        let from_mode = GameMessage {
            sender: GAME_MODE_ENTITY,
            target: EVERY_ENTITY,
            message: ROOM_MESSAGE,
            parameters: room_message(None),
        };
        assert!(
            range.mode.held().contains(&from_mode),
            "{:?}",
            range.mode.held()
        );
        let body = |entity: u32, team: u8, dead: bool, feet: [f64; 3]| TargetCandidate {
            entity,
            team: Some(team),
            dead,
            controller: None,
            ai: false,
            ai_control: None,
            feet,
            height: 2.0,
            radius: 0.4,
            yaw: 0.0,
            tags: None,
            spawned: Vec::new(),
        };
        let outside = [spawn[0] + 60.0, spawn[1], spawn[2] + 60.0];
        let bodies = |at: [f64; 3]| {
            vec![
                body(0x0000_1001, 0, false, at),
                body(0x0000_1002, 1, false, spawn),
                body(0x0000_1003, 0, true, spawn),
            ]
        };
        let frame_serial = |scene: &PreparedScene| {
            scene
                .placed_scripts()
                .frames()
                .find(|(entity, _)| *entity == ROOM)
                .map(|(_, frame)| frame.serial)
        };
        range.advance(100);
        let world = range.scene.placed_scripts().world().unwrap();
        let room = world.entity(ROOM).unwrap();
        assert!(room.running() && room.complete(), "{:?}", world.gaps());
        let first = frame_serial(&range.scene).expect("the room's frame");
        // Each tick of `ticks`: the `0723`s sent (tick, target, parameters)
        // and let go (tick, target).
        let run = |range: &mut Range, ticks: std::ops::RangeInclusive<u32>| {
            let (mut sent, mut ended) = (Vec::new(), Vec::new());
            for tick in ticks {
                range.advance(tick);
                let (messages, ends) = range.scene.take_placed_messages();
                sent.extend(
                    messages
                        .iter()
                        .filter(|message| message.message == HELD)
                        .map(|message| {
                            assert_eq!(message.sender, ROOM);
                            (tick, message.target, message.parameters.len())
                        }),
                );
                ended.extend(ends.iter().map(|end| {
                    assert_eq!((end.sender, end.message), (ROOM, HELD));
                    (tick, end.target)
                }));
            }
            (sent, ended)
        };
        range.scene.set_placed_target_candidates(&bodies(spawn));
        assert_eq!(
            run(&mut range, 101..=120),
            (vec![(105, 0x0000_1001, 3)], vec![])
        );
        assert!(frame_serial(&range.scene).unwrap() > first);
        range.scene.set_placed_target_candidates(&bodies(outside));
        assert_eq!(
            run(&mut range, 121..=140),
            (vec![], vec![(129, 0x0000_1001)])
        );
    }
}
