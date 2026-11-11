//! Each map's objective data from the install (`game_objectives_174.json`
//! `maps`, the `orpheus-assets` derivation `game_objectives`): its capture
//! points' areas and times, the time each objective adds, and its payload's
//! speed factor, waits and track. The match flow builds a map's objectives
//! from it (`match_flow.rs`, `objectives.rs`).
//!
//! The file is checked when it loads: data that doesn't fit (a box without
//! size, a track whose segments have no length) is an error, and so is a map
//! whose derivation failed once a flow plays on it ([`ObjectiveData::map`]).

use std::{collections::HashMap, fs, io, path::Path, sync::Arc};

use orpheus_content::maps::GameMap;
use serde::Deserialize;

use crate::{
    match_flow::Mode,
    mode_rules_data::FILE,
    objectives::{CaptureBox, Track},
};

/// Every map's objective data, by map GUID.
#[derive(Debug, Default)]
pub struct ObjectiveData {
    maps: HashMap<u64, Result<Arc<MapObjectives>, String>>,
}

/// One map's objective data. Entities are named by their flat placeable
/// index, as `MapObjective::placeable`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MapObjectives {
    pub points: Vec<PointData>,
    pub objectives: Vec<ObjectiveTimes>,
    pub payloads: Vec<PayloadData>,
}

/// A capture point (graph 0E8E): its base capture and decay seconds
/// (v12395, v12396) and its area's boxes.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct PointData {
    pub placeable: u32,
    pub position: [f64; 3],
    pub capture: f64,
    pub decay: f64,
    pub boxes: Vec<CaptureBox>,
}

/// The seconds an objective (graph 0F4C) adds to the clock when it becomes
/// the current one (0F4A uids 646-648): with Competitive rules in rounds 1
/// and 2 (v10552) and later (v10553), without them (v10803).
#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
pub struct ObjectiveTimes {
    pub placeable: u32,
    pub objective: u32,
    pub competitive: f64,
    pub competitive_extra: f64,
    pub quick_play: f64,
}

impl ObjectiveTimes {
    /// The seconds it adds: `competitive` is ruleset entry 9544
    /// ("Competitive Rules"), `extra` a round after the first two (v7901
    /// at least 2).
    pub fn seconds(&self, competitive: bool, extra: bool) -> f64 {
        match (competitive, extra) {
            (true, false) => self.competitive,
            (true, true) => self.competitive_extra,
            (false, _) => self.quick_play,
        }
    }
}

/// A payload (graph 1225): its speed factor (v7779), its two waits once
/// active (v2102, v2118), whether it starts with a run of its own (v1362),
/// and its track.
#[derive(Clone, Debug, PartialEq)]
pub struct PayloadData {
    pub placeable: u32,
    pub speed: f64,
    pub unlock: [f64; 2],
    pub auto_start: bool,
    pub track: Track,
}

/// A waypoint of a payload's track as derived: the cubic's a, b, c to the
/// next one (d is its position) and the segment's stored length, none on
/// the last; the barrier flag; the pause and run of its own of graph 121E.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct WaypointData {
    pub placeable: u32,
    pub position: [f64; 3],
    pub curve: Option<[[f64; 3]; 3]>,
    pub length: Option<f64>,
    pub barrier: bool,
    pub event: Option<WaypointEvent>,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
pub struct WaypointEvent {
    pub pause: f64,
    pub auto_next: bool,
}

#[derive(Deserialize)]
struct RawFile {
    maps: HashMap<String, RawMap>,
}

#[derive(Deserialize)]
struct RawMap {
    #[serde(default)]
    points: Vec<PointData>,
    #[serde(default)]
    objectives: Vec<ObjectiveTimes>,
    #[serde(default)]
    payloads: Vec<RawPayload>,
    error: Option<String>,
}

#[derive(Deserialize)]
struct RawPayload {
    placeable: u32,
    speed: f64,
    unlock: [f64; 2],
    auto_start: bool,
    track: Vec<WaypointData>,
}

impl ObjectiveData {
    /// Read the file from the runtime `data` folder.
    pub fn load(data: &Path) -> io::Result<Self> {
        let path = data.join(FILE);
        let bytes = fs::read(&path).map_err(|error| {
            io::Error::new(error.kind(), format!("{}: {error}", path.display()))
        })?;
        Self::read(&bytes)
    }

    pub fn read(bytes: &[u8]) -> io::Result<Self> {
        let file: RawFile = serde_json::from_slice(bytes).map_err(invalid)?;
        let mut maps = HashMap::new();
        for (key, raw) in file.maps {
            let guid = u64::from_str_radix(key.trim_start_matches("0x"), 16)
                .map_err(|_| invalid(format!("{FILE}: map key {key}")))?;
            let entry = match raw.error {
                Some(error) => Err(error),
                None => {
                    Ok(Arc::new(MapObjectives::check(raw).map_err(|error| {
                        invalid(format!("{FILE}: map {key}: {error}"))
                    })?))
                }
            };
            maps.insert(guid, entry);
        }
        Ok(Self { maps })
    }

    /// The objective data of the map; an error when its derivation failed
    /// and its mode plays for objectives. A map without objective data has
    /// none.
    pub fn map(&self, map: &GameMap) -> io::Result<Arc<MapObjectives>> {
        match self.maps.get(&map.map_guid) {
            None => Ok(Arc::default()),
            Some(Ok(objectives)) => Ok(Arc::clone(objectives)),
            Some(Err(error)) if Mode::of(map.mode_guid).is_some_and(Mode::has_objectives) => {
                Err(invalid(format!(
                    "{FILE}: map {:016X} ({}) has no objective data: {error}",
                    map.map_guid, map.name
                )))
            }
            Some(Err(_)) => Ok(Arc::default()),
        }
    }
}

impl MapObjectives {
    fn check(raw: RawMap) -> Result<Self, String> {
        for point in &raw.points {
            if !(point.capture > 0.0 && point.decay > 0.0) {
                return Err(format!(
                    "point {}: capture {} s, decay {} s",
                    point.placeable, point.capture, point.decay
                ));
            }
            if point.boxes.is_empty() {
                return Err(format!("point {} has no boxes", point.placeable));
            }
            for area in &point.boxes {
                area.check()
                    .map_err(|error| format!("point {}: {error}", point.placeable))?;
            }
        }
        let payloads = raw
            .payloads
            .into_iter()
            .map(|payload| {
                if !(payload.speed > 0.0 && payload.unlock.iter().all(|wait| *wait >= 0.0)) {
                    return Err(format!(
                        "payload {}: speed {}, waits {:?}",
                        payload.placeable, payload.speed, payload.unlock
                    ));
                }
                Ok(PayloadData {
                    placeable: payload.placeable,
                    speed: payload.speed,
                    unlock: payload.unlock,
                    auto_start: payload.auto_start,
                    track: Track::new(&payload.track)
                        .map_err(|error| format!("payload {}: {error}", payload.placeable))?,
                })
            })
            .collect::<Result<_, String>>()?;
        Ok(Self {
            points: raw.points,
            objectives: raw.objectives,
            payloads,
        })
    }

    pub fn point(&self, placeable: u32) -> Option<&PointData> {
        self.points
            .iter()
            .find(|point| point.placeable == placeable)
    }

    pub fn times(&self, placeable: u32) -> Option<&ObjectiveTimes> {
        self.objectives
            .iter()
            .find(|objective| objective.placeable == placeable)
    }

    pub fn payload(&self, placeable: u32) -> Option<&PayloadData> {
        self.payloads
            .iter()
            .find(|payload| payload.placeable == placeable)
    }
}

fn invalid(error: impl std::fmt::Display) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error.to_string())
}

#[cfg(test)]
mod tests {
    use orpheus_content::maps::{MapCatalog, ObjectiveKind};
    use orpheus_script::graph::GraphStore;

    use super::*;
    use crate::{
        match_flow::{ASSAULT, CONTROL, ELIMINATION, ESCORT, HYBRID, MatchFlow, Timers},
        mode_graph_values::ModeGraphValues,
        mode_rules_data::ModeRulesData,
        objectives::Area,
    };

    const ANUBIS: u64 = 0x0800_0000_0000_005b;
    const KINGS_ROW: u64 = 0x0800_0000_0000_00d4;
    const HANAMURA: u64 = 0x0800_0000_0000_0165;
    const DORADO: u64 = 0x0800_0000_0000_02c3;
    const NEPAL: u64 = 0x0800_0000_0000_04b7;
    const LIJIANG: u64 = 0x0800_0000_0000_0662;
    const BLACK_FOREST: u64 = 0x0800_0000_0000_070c;

    struct Data {
        objectives: ObjectiveData,
        catalog: MapCatalog,
    }

    fn data() -> Data {
        let data = orpheus_testkit::runtime_dir().join("data");
        Data {
            objectives: ObjectiveData::load(&data).unwrap(),
            catalog: MapCatalog::load(&data).unwrap(),
        }
    }

    impl Data {
        fn game_map(&self, map: u64, mode: u64) -> GameMap {
            self.catalog
                .game_map(map, mode, "", false, &[6, 6])
                .unwrap()
        }
    }

    /// The area of a point at the objective's height + 0.1 m, sampled on a
    /// 5 cm grid.
    fn footprint(point: &PointData) -> f64 {
        let area = Area::Boxes(point.boxes.clone());
        let [x, y, z] = point.position;
        let cell = 0.05;
        let reach = (20.0 / cell) as i32;
        let mut inside = 0;
        for i in -reach..=reach {
            for k in -reach..=reach {
                let at = [x + f64::from(i) * cell, y + 0.1, z + f64::from(k) * cell];
                inside += usize::from(area.contains(at));
            }
        }
        inside as f64 * cell * cell
    }

    /// The boxes are read with their rotation the right way round: the
    /// footprints match the refs report's (`CaptureAreas.md` §2: Anubis A
    /// 171.2 m², Hanamura B 271.9, Nepal B 275.6, Lijiang C 249.9, the
    /// last four boxes turned in 45° steps), and every point holds its
    /// objective's position.
    #[test]
    #[ignore = "requires the 1.74 game data (ORPHEUS_GAME_DIR)"]
    fn the_capture_areas_have_the_reported_footprints() {
        let data = data();
        for (map, mode, order, expected) in [
            (ANUBIS, ASSAULT, 0, 171.2),
            (HANAMURA, ASSAULT, 1, 271.9),
            (NEPAL, CONTROL, 1, 275.6),
            (LIJIANG, CONTROL, 2, 249.9),
        ] {
            let game = data.game_map(map, mode);
            let objectives = data.objectives.map(&game).unwrap();
            let objective = game
                .objectives
                .iter()
                .find(|objective| {
                    objective.order == order
                        && matches!(
                            objective.kind,
                            ObjectiveKind::Point | ObjectiveKind::ControlPoint
                        )
                })
                .unwrap();
            let point = objectives.point(objective.placeable).unwrap();
            let found = footprint(point);
            assert!(
                (found - expected).abs() < 0.01 * expected,
                "map {map:X} objective {order}: {found} m², expected {expected}"
            );
            for point in &objectives.points {
                assert!(Area::Boxes(point.boxes.clone()).contains(point.position));
            }
        }
    }

    /// King's Row's track: its stored length (185.0 m) and its curve
    /// (191.6 m, 1.0355 times longer: `Payloads.md` §2), from the payload's
    /// place through the checkpoints' entities.
    #[test]
    #[ignore = "requires the 1.74 game data (ORPHEUS_GAME_DIR)"]
    fn the_payload_follows_its_track_through_its_checkpoints() {
        let data = data();
        let game = data.game_map(KINGS_ROW, HYBRID);
        let objectives = data.objectives.map(&game).unwrap();
        let payload = game.objectives_of(ObjectiveKind::Payload).next().unwrap();
        let track = &objectives.payload(payload.placeable).unwrap().track;
        assert!((track.length() - 185.02).abs() < 0.01);
        let distance = |a: [f64; 3], b: [f64; 3]| -> f64 {
            (0..3)
                .map(|axis| (a[axis] - b[axis]).powi(2))
                .sum::<f64>()
                .sqrt()
        };
        let mut arc = 0.0;
        let mut previous = track.position(0.0);
        let mut at = 0.0;
        while at < track.length() {
            at = (at + 0.05).min(track.length());
            let point = track.position(at);
            arc += distance(previous, point);
            previous = point;
        }
        assert!((arc - 191.6).abs() < 0.2, "{arc}");
        assert!(distance(track.position(0.0), payload.position) < 0.15);
        for checkpoint in game.objectives_of(ObjectiveKind::Checkpoint) {
            let waypoint = track
                .waypoints()
                .iter()
                .find(|waypoint| waypoint.placeable == checkpoint.placeable)
                .unwrap();
            assert!(distance(track.position(waypoint.distance), checkpoint.position) < 0.01);
        }
    }

    /// The flows of the install's maps start with their objectives' times:
    /// Anubis 4:00 either way, King's Row and Dorado 5:00 without
    /// Competitive rules and 4:00 with them; Nepal's Control and Black
    /// Forest's Elimination find their points.
    #[test]
    #[ignore = "requires the 1.74 game data (ORPHEUS_GAME_DIR)"]
    fn the_install_maps_start_their_flows_with_their_times() {
        let data = data();
        let path = orpheus_testkit::runtime_dir().join("data");
        let graphs = GraphStore::load_runtime(&path, "game_graphs_174.json.gz").unwrap();
        let values = ModeGraphValues::read(&graphs).unwrap();
        let rules = ModeRulesData::load(&graphs, &path).unwrap();
        let flow = |map, mode, competitive| {
            let game = data.game_map(map, mode);
            let objectives = data.objectives.map(&game).unwrap();
            let rules = rules.rules(graphs.rulesets(), mode).unwrap();
            let mut flow = MatchFlow::new(
                &game,
                competitive,
                values,
                rules,
                objectives,
                Timers::default(),
            )
            .unwrap_or_else(|| panic!("map {map:X} mode {mode:X}"));
            flow.start();
            for _ in 0..80 * 60 {
                flow.advance(1.0 / 60.0, &[]);
                if flow.phase() == crate::match_flow::Phase::Round {
                    break;
                }
            }
            flow.clock()
        };
        for (map, mode, quick_play, competitive) in [
            (ANUBIS, ASSAULT, 240.0, 240.0),
            (KINGS_ROW, HYBRID, 300.0, 240.0),
            (DORADO, ESCORT, 300.0, 240.0),
        ] {
            assert!((flow(map, mode, false) - quick_play).abs() < 0.1, "{map:X}");
            assert!((flow(map, mode, true) - competitive).abs() < 0.1, "{map:X}");
        }
        flow(NEPAL, CONTROL, false);
        flow(BLACK_FOREST, ELIMINATION, false);
    }
}
