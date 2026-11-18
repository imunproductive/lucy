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
//! go to the match ([`PlacedScripts::take_messages`]), which hands them to
//! the bodies.
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
    placed::{EntityVolumes, MapEntityWorld},
    snapshot::{ProjectedSnapshot, SnapshotEncoder, View},
    stream::{ScriptChunk, Stream, StreamError},
    vm::{GameMessage, MessageEnd, TargetCandidate, Unknowns},
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
    rooms: Arc<[SpawnRoom]>,
    world: Option<MapEntityWorld>,
    /// The rooms that hold the mode's `0724`, with the team it gave each.
    held: BTreeMap<u32, Option<u8>>,
    /// The complete scripts' messages and ends for the match to route.
    messages: Vec<GameMessage>,
    ends: Vec<MessageEnd>,
    /// The last projection and frame of each complete script.
    frames: BTreeMap<u32, (ProjectedSnapshot, PlacedFrame)>,
    gaps_logged: bool,
}

impl PlacedScripts {
    pub fn new(
        graphs: Arc<GraphStore>,
        mode: u64,
        mode_random: f32,
        placed: Vec<Arc<PlacedScript>>,
        map_tags: Arc<MapTags>,
        rooms: Arc<[SpawnRoom]>,
    ) -> Self {
        Self {
            graphs,
            codecs: CodecTable::client(),
            mode,
            mode_random,
            placed,
            map_tags,
            rooms,
            world: None,
            held: BTreeMap::new(),
            messages: Vec::new(),
            ends: Vec::new(),
            frames: BTreeMap::new(),
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
                world.advance(tick);
                world
            }
            None => self.world.insert(MapEntityWorld::start(
                Arc::clone(&self.graphs),
                self.mode,
                self.mode_random,
                Unknowns::for_build(),
                self.placed.iter().map(|placed| placed.placement()),
                Some(Arc::clone(&self.map_tags)),
                Some(Arc::new(RoomVolumes(Arc::clone(&self.rooms))) as Arc<dyn EntityVolumes>),
                tick,
            )),
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
    /// call, for the match to hand to the bodies.
    pub fn take_messages(&mut self) -> (Vec<GameMessage>, Vec<MessageEnd>) {
        (
            std::mem::take(&mut self.messages),
            std::mem::take(&mut self.ends),
        )
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
    use crate::scene_assets::SceneAssets;
    use orpheus_content::maps::{GameMap, SpawnChoice};

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

    /// The Practice Range's 26 server placeables: 22 have graphs; the six
    /// barriers `00F6`, the three spawn doors `0D32` (358, 479, 480), the
    /// six spawn markers `01CE` (137, 150, 243, 415, 482, 590: they give
    /// themselves their team, `STU_0F647E55`) and the spawn room `0D62`
    /// (126, whose target searches the mode's `0724` begins: every field
    /// they set is understood, `vm/targets.rs`) run whole and have frames,
    /// as do the four whose graphs `0216`/`0217` end at once; the console
    /// `265F` and pad `26F8` still meet gaps and send nothing. Each door's
    /// frame changes once in its first 300 frames (its alias states, see
    /// `0D32`; which wait ends first is not pinned here).
    #[test]
    #[ignore = "requires the derived 104319 data"]
    fn the_range_placeables_run_their_graphs_and_have_frames() {
        let assets = SceneAssets::load(orpheus_testkit::runtime_dir()).unwrap();
        let mut scene = assets.prepare(GameMap::practice_range()).unwrap();
        assert_eq!(scene.server_placeables().len(), 26);
        for tick in 100..400 {
            scene.advance_placed(tick, |_| SpawnChoice::First);
        }
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
                480, 482, 548, 590
            ]
        );
        for door in [358, 479, 480] {
            let (_, frame) = scene
                .placed_scripts()
                .frames()
                .find(|(entity, _)| *entity == 0x8000_0000 | door)
                .unwrap();
            assert_eq!(frame.serial, 2, "door {door}");
        }
        let gaps = world.gaps();
        assert!(
            gaps.keys()
                .all(|(graph, _)| ![0x01ce, 0x0d62].contains(graph)),
            "{gaps:?}"
        );
    }

    /// The Practice Range's spawn room `0D62` (126) takes the mode's `0724`
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
        let mut scene = assets.prepare(map).unwrap();
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
        let frame_serial = |scene: &crate::scene_assets::PreparedScene| {
            scene
                .placed_scripts()
                .frames()
                .find(|(entity, _)| *entity == ROOM)
                .map(|(_, frame)| frame.serial)
        };
        scene.advance_placed(100, |_| SpawnChoice::First);
        let world = scene.placed_scripts().world().unwrap();
        let room = world.entity(ROOM).unwrap();
        assert!(room.running() && room.complete(), "{:?}", world.gaps());
        let first = frame_serial(&scene).expect("the room's frame");
        // Each tick of `ticks`: the `0723`s sent (tick, target, parameters)
        // and let go (tick, target).
        let run = |scene: &mut crate::scene_assets::PreparedScene,
                   ticks: std::ops::RangeInclusive<u32>| {
            let (mut sent, mut ended) = (Vec::new(), Vec::new());
            for tick in ticks {
                scene.advance_placed(tick, |_| SpawnChoice::First);
                let (messages, ends) = scene.take_placed_messages();
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
        scene.set_placed_target_candidates(&bodies(spawn));
        assert_eq!(
            run(&mut scene, 101..=120),
            (vec![(105, 0x0000_1001, 3)], vec![])
        );
        assert!(frame_serial(&scene).unwrap() > first);
        scene.set_placed_target_candidates(&bodies(outside));
        assert_eq!(
            run(&mut scene, 121..=140),
            (vec![], vec![(129, 0x0000_1001)])
        );
    }
}
