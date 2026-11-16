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
//! by assumption) or a fault sends nothing; its gaps are logged once.
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
    placed::MapEntityWorld,
    snapshot::{ProjectedSnapshot, SnapshotEncoder, View},
    stream::{ScriptChunk, Stream, StreamError},
    vm::{TargetCandidate, Unknowns},
};

use crate::map_areas::PlacedScript;

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
    world: Option<MapEntityWorld>,
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
    ) -> Self {
        Self {
            graphs,
            codecs: CodecTable::client(),
            mode,
            mode_random,
            placed,
            world: None,
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

    /// Start the scripts at the first call, then advance them to `tick`,
    /// and project the complete ones' frames.
    pub fn advance(&mut self, tick: u32) {
        match &mut self.world {
            Some(world) => {
                world.advance(tick);
            }
            None => {
                self.world = Some(MapEntityWorld::start(
                    Arc::clone(&self.graphs),
                    self.mode,
                    self.mode_random,
                    Unknowns::for_build(),
                    self.placed.iter().map(|placed| placed.placement()),
                    tick,
                ));
            }
        }
        // Placed scripts send no messages the match routes yet.
        let world = self.world.as_mut().expect("started above");
        world.take_messages();
        if !self.gaps_logged {
            self.gaps_logged = true;
            let gaps = world.gaps();
            if !gaps.is_empty() {
                eprintln!(
                    "{} of {} placed scripts have gaps and send no frames: {gaps:?}",
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
    use orpheus_content::maps::GameMap;

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

    /// The Practice Range's 26 server placeables: 22 have graphs; the
    /// spawn room `0D62` (126), the six barriers `00F6`, the three spawn
    /// doors `0D32` (358, 479, 480) and the six spawn markers `01CE` (137,
    /// 150, 243, 415, 482, 590: they give themselves their team,
    /// `STU_0F647E55`) run whole and have frames, as do the four whose
    /// graphs `0216`/`0217` end at once. The console `265F` and pad `26F8`
    /// still meet gaps; they send nothing. Each door's frame changes once
    /// in its first 300 frames (its alias states, see `0D32`; which wait
    /// ends first is not pinned here).
    #[test]
    #[ignore = "requires the derived 104319 data"]
    fn the_range_placeables_run_their_graphs_and_have_frames() {
        let assets = SceneAssets::load(orpheus_testkit::runtime_dir()).unwrap();
        let mut scene = assets.prepare(GameMap::practice_range()).unwrap();
        assert_eq!(scene.server_placeables().len(), 26);
        for tick in 100..400 {
            scene.advance_placed(tick);
        }
        let world = scene.placed_scripts().world().unwrap();
        assert_eq!(world.entities().len(), 22);
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
        assert!(gaps.keys().all(|(graph, _)| *graph != 0x01ce), "{gaps:?}");
    }
}
