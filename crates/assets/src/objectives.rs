//! `data/game_objectives_174.json`: what the objectives' native components
//! and map placeables hold that the server reads.
//!
//! - The capture area's progress marks: entity `0x37.003` is the only entity
//!   definition with an `STUCaptureAreaComponent`, and its `m_segments` list
//!   the marks (each an `STUCaptureAreaSegment`'s float `m_4A9567C6`, 1/3 and
//!   2/3 in 1.74) that a capture never falls below.
//! - Per map (keyed like `game_maps_174.json`, its objectives by the same flat
//!   placeable index), from the map's chunks (`crate::map_chunk`):
//!   - `points`: every capture point (an entity running graph `0E8E`): its
//!     base capture and decay seconds (`0E8E` v12395 / v12396, the placeable's
//!     override or the graph's default) and its capture volume. The volume is
//!     the area placeable (chunk kind `0x0A`, placeable type 8) whose UUID the
//!     entity's `STUTargetTagInstanceData.m_2746D7E4` (+24, a teUUID) holds: a
//!     body of `{u32 ×3, u32 box count (low 16 bits), u32, u32 flags, u32
//!     offsets [6..13] from the body start}`, the boxes from offset [6] to [7],
//!     each 40 bytes: rotation `{x, y, z, w}`, world centre, half extents.
//!     That this area is the capture volume is a strong guess (refs report
//!     `CaptureAreas.md` §3: no reader seen).
//!   - `objectives`: every objective (graph `0F4C`): its order (v10548) and
//!     the time it adds when it becomes the current one (`0F4A` uids
//!     646-648): v10552 with "Competitive Rules" in rounds 1-2, v10553 in
//!     later rounds, v10803 without them (`Payloads.md` §1).
//!   - `payloads`: every payload (graph `1225`): its speed factor (v7779),
//!     its waits after it becomes active (v2102, v2118), whether it starts
//!     with a run of its own (v1362), and its track. The track is the linked
//!     list of `STUWaypointInstanceData` (0xB1B9C667) entities from the
//!     payload's `STUParametricMovementComponentInstanceData.m_destination`
//!     (0xE42D9464, +8): per waypoint the cubic `a t³ + b t² + c t + d` to the
//!     next one (+16, +32, +48, +64; d is the waypoint's position, +80), the
//!     next waypoint's UUID (+128), the stored segment length (+160) and the
//!     barrier flag (+164). A waypoint that runs graph `121E` stops the
//!     payload for its v955 seconds, and with v3572 set sends it on alone to
//!     the next such waypoint (`Payloads.md` §4).
//!
//! Field offsets follow OWLib's `STUWaypointInstanceData` and
//! `STUParametricMovementComponentInstanceData` (MIT); the rest is from the
//! refs reports. Data that doesn't fit (a missing area or waypoint, a curve
//! that doesn't end at the next waypoint) fails the derivation.

use std::{
    collections::{BTreeSet, HashMap},
    fs,
    path::Path,
};

use orpheus_stu::{Asset, Value};
use serde_json::{Map, Value as Json, json};

use crate::{
    Install, Output, Result,
    ai_spawners::component,
    asset_error,
    game_maps::{
        self, GraphVars, TARGET_AREA, TARGET_TAG_DATA, chunk_index, graph_vars, instance,
        map_chunks, read_map,
    },
    guid_hex,
    map_chunk::{area_boxes, name_index, short},
};

pub(crate) const OBJECTIVES: &str = "data/game_objectives_174.json";

/// The capture area entity definition (`0x37.003`).
const CAPTURE_AREA: u64 = 0x0400_0000_0000_0037;

/// The asset type of statescript graphs.
const GRAPH: u16 = 0x01B;
/// The objective graph (order and times), the capture point graph, the
/// payload graph and the payload event waypoint graph.
const OBJECTIVE_GRAPH: u64 = 0x0F4C;
const POINT_GRAPH: u64 = 0x0E8E;
const PAYLOAD_GRAPH: u64 = 0x1225;
const WAYPOINT_GRAPH: u64 = 0x121E;

/// 0F4C: the order, the competitive times (rounds 1-2, later rounds) and
/// the time without Competitive rules.
const ORDER: u64 = 10548;
const COMPETITIVE_TIME: u64 = 10552;
const COMPETITIVE_EXTRA_TIME: u64 = 10553;
const QUICK_PLAY_TIME: u64 = 10803;
/// 0E8E: the base capture and decay seconds.
const CAPTURE_TIME: u64 = 12395;
const DECAY_TIME: u64 = 12396;
/// 1225: the speed factor, the two waits after it becomes active, and the
/// run of its own at activation.
const SPEED_FACTOR: u64 = 7779;
const UNLOCK_WAIT: u64 = 2102;
const UNLOCK_ANIMATION: u64 = 2118;
const AUTO_START: u64 = 1362;
/// 121E: the pause and the run of its own to the next event waypoint.
const PAUSE: u64 = 955;
const AUTO_NEXT: u64 = 3572;

/// STUv1 instance data classes.
const WAYPOINT_DATA: u32 = 0xB1B9_C667;
const MOVEMENT_DATA: u32 = 0xE42D_9464;
/// `STUParametricMovementComponentInstanceData.m_destination`.
const DESTINATION: usize = 8;

/// How far a waypoint's curve may end from the next waypoint, in metres.
const CURVE_TOLERANCE: f32 = 0.01;

const SOURCE: &str = "Build 1.74.0.0.104319: entity 0x37.003's STUCaptureAreaComponent.m_segments, each \
STUCaptureAreaSegment's m_4A9567C6 (the capture progress marks). maps: per map (game_maps_174.json keys and \
placeable indexes) its capture points (graph 0E8E: capture = v12395, decay = v12396 seconds; boxes = the chunk \
0x0A area placeable named by STUTargetTagInstanceData.m_2746D7E4, centre, half extents, rotation x y z w), its \
objectives (graph 0F4C: objective = v10548, competitive = v10552, competitive_extra = v10553 (rounds 3+), \
quick_play = v10803), its payloads (graph 1225: speed = v7779, unlock = [v2102, v2118], auto_start = v1362) with \
their tracks (STUWaypointInstanceData from STUParametricMovementComponentInstanceData.m_destination: curve = \
cubic coefficients a, b, c of the segment to the next waypoint, d = position; length = the stored segment length; \
event = graph 121E: pause = v955, auto_next = v3572). Values a placeable doesn't override are the graph's \
defaults.";

pub(crate) fn derive(install: &Install, out: &Path) -> Result<Vec<Output>> {
    let asset = install.stu(CAPTURE_AREA)?;
    let segments = capture_segments(&asset)
        .map_err(|reason| asset_error(format!("{}: {reason}", guid_hex(CAPTURE_AREA))))?;
    let defaults = Defaults::read(install)?;
    let maps_path = out.join(game_maps::PATH);
    let maps: Map<String, Json> = serde_json::from_slice(&fs::read(&maps_path)?)
        .map_err(|error| asset_error(format!("{}: {error}", game_maps::PATH)))?;
    let chunks = chunk_index(install);
    let mut derived = Map::new();
    for (key, entry) in &maps {
        let map = u64::from_str_radix(key.trim_start_matches("0x"), 16)
            .map_err(|_| asset_error(format!("{}: map key {key}", game_maps::PATH)))?;
        let data = map_chunks(install, &chunks, name_index(map));
        // A map whose objective data doesn't fit keeps the reason instead:
        // the server refuses to run a match flow on it.
        let objectives = map_objectives(&data, &defaults)
            .and_then(|objectives| check_placeables(entry, &objectives).map(|()| objectives))
            .unwrap_or_else(|error| Map::from_iter([("error".into(), json!(error.to_string()))]));
        if !objectives.is_empty() {
            derived.insert(key.clone(), Json::Object(objectives));
        }
    }
    Ok(vec![Output::json(
        OBJECTIVES,
        &json!({
            "source": SOURCE,
            "capture_area": {"entity": guid_hex(CAPTURE_AREA), "segments": segments},
            "maps": derived,
        }),
    )])
}

/// The marks of the entity's capture area component, in order.
fn capture_segments(asset: &Asset) -> Result<Vec<Json>, String> {
    let component =
        component(asset, "STUCaptureAreaComponent").ok_or("no STUCaptureAreaComponent")?;
    let Some(Value::Array(segments)) = component.get("m_segments") else {
        return Err("the capture area has no m_segments".into());
    };
    if segments.is_empty() {
        return Err("the capture area has no segments".into());
    }
    segments
        .iter()
        .map(|segment| {
            let segment = asset
                .resolve(segment)
                .filter(|segment| segment.class_name() == Some("STUCaptureAreaSegment"))
                .ok_or("a segment is not an STUCaptureAreaSegment")?;
            match segment.get("m_4A9567C6") {
                Some(&Value::Float(mark)) => Ok(float(mark)),
                _ => Err("a segment has no float m_4A9567C6".into()),
            }
        })
        .collect()
}

/// An `f32` as DataTool prints it, read back as a double (as `game_modes`).
fn float(value: f64) -> Json {
    let shortest: f64 = (value as f32).to_string().parse().expect("a float");
    json!(shortest)
}

/// The public schema defaults of the graphs whose variables this reads.
struct Defaults(HashMap<u64, HashMap<u64, f32>>);

impl Defaults {
    fn read(install: &Install) -> Result<Self> {
        let wanted = [OBJECTIVE_GRAPH, POINT_GRAPH, PAYLOAD_GRAPH, WAYPOINT_GRAPH];
        let mut out = HashMap::new();
        for guid in install.guids(GRAPH) {
            let index = name_index(guid);
            if !wanted.contains(&index) {
                continue;
            }
            let asset = install.stu(guid)?;
            let defaults = schema_defaults(&asset)
                .map_err(|reason| asset_error(format!("graph {index:04X}: {reason}")))?;
            out.insert(index, defaults);
        }
        if let Some(missing) = wanted.iter().find(|graph| !out.contains_key(graph)) {
            return Err(asset_error(format!(
                "the install has no graph {missing:04X}"
            )));
        }
        Ok(Self(out))
    }

    /// A graph variable's value on a placeable: its override, else the
    /// graph's default.
    fn number(&self, graph: u64, vars: &GraphVars<'_>, key: u64) -> Result<f32> {
        if let Some(value) = vars.number(key)? {
            return Ok(value);
        }
        self.0[&graph].get(&key).copied().ok_or_else(|| {
            asset_error(format!(
                "graph {graph:04X} has no number variable {key} in its public schema"
            ))
        })
    }
}

/// The number defaults (int, float, bool) of a graph's public schema, by
/// identifier index.
fn schema_defaults(asset: &Asset) -> Result<HashMap<u64, f32>, String> {
    let root = asset.root().ok_or("no root")?;
    let schema = root
        .get("m_publicSchema")
        .and_then(|schema| asset.resolve(schema))
        .ok_or("no public schema")?;
    let Some(Value::Array(entries)) = schema.get("m_entries") else {
        return Err("a public schema without entries".into());
    };
    let mut out = HashMap::new();
    for entry in entries {
        let entry = asset.resolve(entry).ok_or("an unresolved schema entry")?;
        let Some(&Value::Guid(identifier)) = entry.get("m_0D09D2D9") else {
            continue;
        };
        let Some(config) = entry.get("m_value").and_then(|value| asset.resolve(value)) else {
            continue;
        };
        let number = match config.get("m_value") {
            Some(&Value::Float(value)) => value as f32,
            Some(&Value::Int(value)) => value as f32,
            Some(&Value::UInt(value)) => value as f32,
            _ => continue,
        };
        out.insert(name_index(identifier), number);
    }
    Ok(out)
}

/// One waypoint of a track.
struct Waypoint {
    placeable: usize,
    position: [f32; 3],
    /// a, b, c, d of the cubic to the next waypoint.
    curve: [[f32; 3]; 4],
    next: [u8; 16],
    length: f32,
    barrier: bool,
    event: Option<(f32, bool)>,
}

/// The points, objectives and payloads of a map.
fn map_objectives(chunks: &[(u64, Vec<u8>)], defaults: &Defaults) -> Result<Map<String, Json>> {
    let (base, entities) = read_map(chunks)?;
    let areas = area_boxes(chunks);
    let mut points = Vec::new();
    let mut objectives = Vec::new();
    let mut payloads = Vec::new();
    let mut waypoints: HashMap<[u8; 16], Waypoint> = HashMap::new();
    let mut destinations = Vec::new();
    for (number, placed) in entities.iter().enumerate() {
        let flat = base + number;
        let graphs = graph_vars(&placed.instances);
        let position: Vec<f64> = placed.entity.translation.map(short).to_vec();
        if let Some(vars) = graphs.get(&OBJECTIVE_GRAPH) {
            let number = |key| defaults.number(OBJECTIVE_GRAPH, vars, key);
            objectives.push(json!({
                "placeable": flat,
                "objective": number(ORDER)? as i64,
                "competitive": short(number(COMPETITIVE_TIME)?),
                "competitive_extra": short(number(COMPETITIVE_EXTRA_TIME)?),
                "quick_play": short(number(QUICK_PLAY_TIME)?),
            }));
        }
        if let Some(vars) = graphs.get(&POINT_GRAPH) {
            let area = instance(placed, TARGET_TAG_DATA)
                .and_then(|(stu, root)| stu.uuid(root + TARGET_AREA))
                .filter(|uuid| *uuid != [0; 16]);
            let Some(area) = area else {
                return Err(asset_error(format!("capture point {flat} names no area")));
            };
            let boxes = areas.get(&area).ok_or_else(|| {
                asset_error(format!("capture point {flat}: its area is not a box area"))
            })?;
            let number = |key| defaults.number(POINT_GRAPH, vars, key);
            points.push(json!({
                "placeable": flat,
                "position": position,
                "capture": short(number(CAPTURE_TIME)?),
                "decay": short(number(DECAY_TIME)?),
                "boxes": boxes,
            }));
        }
        if let Some(vars) = graphs.get(&PAYLOAD_GRAPH) {
            let destination = instance(placed, MOVEMENT_DATA)
                .and_then(|(stu, root)| stu.uuid(root + DESTINATION))
                .filter(|uuid| *uuid != [0; 16])
                .ok_or_else(|| asset_error(format!("payload {flat} has no first waypoint")))?;
            let number = |key| defaults.number(PAYLOAD_GRAPH, vars, key);
            let mut payload = Map::new();
            payload.insert("placeable".into(), json!(flat));
            payload.insert("speed".into(), json!(short(number(SPEED_FACTOR)?)));
            payload.insert(
                "unlock".into(),
                json!([
                    short(number(UNLOCK_WAIT)?),
                    short(number(UNLOCK_ANIMATION)?)
                ]),
            );
            payload.insert("auto_start".into(), json!(number(AUTO_START)? != 0.0));
            destinations.push(destination);
            payloads.push(payload);
        }
        if let Some((stu, root)) = instance(placed, WAYPOINT_DATA) {
            let vector = |at: usize| -> Result<[f32; 3]> {
                let mut out = [0.0; 3];
                for (k, value) in out.iter_mut().enumerate() {
                    *value = stu.f32(root + at + 4 * k).ok_or_else(|| {
                        asset_error(format!("waypoint {flat} runs past its blob"))
                    })?;
                }
                Ok(out)
            };
            let event = match graphs.get(&WAYPOINT_GRAPH) {
                Some(vars) => {
                    let number = |key| defaults.number(WAYPOINT_GRAPH, vars, key);
                    Some((number(PAUSE)?, number(AUTO_NEXT)? != 0.0))
                }
                None => None,
            };
            let waypoint = Waypoint {
                placeable: flat,
                position: vector(80)?,
                curve: [vector(16)?, vector(32)?, vector(48)?, vector(64)?],
                next: stu.uuid(root + 128).unwrap_or_default(),
                length: stu.f32(root + 160).unwrap_or_default(),
                barrier: stu.u8(root + 164).unwrap_or_default() != 0,
                event,
            };
            waypoints.insert(placed.entity.uuid(), waypoint);
        }
    }
    for (payload, destination) in payloads.iter_mut().zip(destinations) {
        let track = track(&waypoints, destination)
            .map_err(|reason| asset_error(format!("payload {}: {reason}", payload["placeable"])))?;
        payload.insert("track".into(), Json::Array(track));
    }
    let mut out = Map::new();
    if !points.is_empty() {
        out.insert("points".into(), Json::Array(points));
    }
    if !objectives.is_empty() {
        out.insert("objectives".into(), Json::Array(objectives));
    }
    if !payloads.is_empty() {
        out.insert(
            "payloads".into(),
            Json::Array(payloads.into_iter().map(Json::Object).collect()),
        );
    }
    Ok(out)
}

fn distance(a: [f32; 3], b: [f32; 3]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

/// The track from its first waypoint along the next links: each waypoint
/// with the curve to the next one (none on the last), checked to end there.
fn track(waypoints: &HashMap<[u8; 16], Waypoint>, first: [u8; 16]) -> Result<Vec<Json>, String> {
    let mut out = Vec::new();
    let mut seen = BTreeSet::new();
    let mut at = first;
    loop {
        let waypoint = waypoints
            .get(&at)
            .ok_or_else(|| format!("waypoint {} is not on the map", hex(&at)))?;
        if !seen.insert(at) {
            return Err(format!(
                "the track loops at waypoint {}",
                waypoint.placeable
            ));
        }
        let mut entry = Map::new();
        entry.insert("placeable".into(), json!(waypoint.placeable));
        entry.insert("position".into(), json!(waypoint.position.map(short)));
        let next = (waypoint.next != [0; 16])
            .then(|| waypoints.get(&waypoint.next))
            .flatten();
        if waypoint.next != [0; 16] && next.is_none() {
            return Err(format!(
                "waypoint {} links to {}, which is not on the map",
                waypoint.placeable,
                hex(&waypoint.next)
            ));
        }
        if let Some(next) = next {
            let [a, b, c, d] = waypoint.curve;
            let end: [f32; 3] = std::array::from_fn(|k| a[k] + b[k] + c[k] + d[k]);
            if distance(d, waypoint.position) > CURVE_TOLERANCE
                || distance(end, next.position) > CURVE_TOLERANCE
            {
                return Err(format!(
                    "waypoint {}'s curve runs from {d:?} to {end:?}, not from {:?} to the next \
                     waypoint {:?}",
                    waypoint.placeable, waypoint.position, next.position
                ));
            }
            if waypoint.length.is_nan() || waypoint.length <= 0.0 {
                return Err(format!(
                    "waypoint {} has a segment length of {}",
                    waypoint.placeable, waypoint.length
                ));
            }
            entry.insert(
                "curve".into(),
                json!([a, b, c].map(|vector| vector.map(short))),
            );
            entry.insert("length".into(), json!(short(waypoint.length)));
        }
        entry.insert("barrier".into(), json!(waypoint.barrier));
        if let Some((pause, auto_next)) = waypoint.event {
            entry.insert(
                "event".into(),
                json!({"pause": short(pause), "auto_next": auto_next}),
            );
        }
        out.push(Json::Object(entry));
        if next.is_none() {
            return Ok(out);
        }
        at = waypoint.next;
    }
}

fn hex(uuid: &[u8; 16]) -> String {
    uuid.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Every objective `game_maps` lists for the map has its data here, under
/// the same placeable index.
fn check_placeables(entry: &Json, derived: &Map<String, Json>) -> Result<()> {
    let listed = |list: &str| -> Vec<u64> {
        derived
            .get(list)
            .and_then(Json::as_array)
            .into_iter()
            .flatten()
            .filter_map(|item| item["placeable"].as_u64())
            .collect()
    };
    let (points, objectives, payloads) =
        (listed("points"), listed("objectives"), listed("payloads"));
    for objective in entry["objectives"].as_array().into_iter().flatten() {
        let placeable = objective["placeable"].as_u64().unwrap_or(u64::MAX);
        let found = match objective["kind"].as_str() {
            Some("point") => points.contains(&placeable) && objectives.contains(&placeable),
            Some("checkpoint") => objectives.contains(&placeable),
            Some("control point") => points.contains(&placeable),
            Some("payload") => payloads.contains(&placeable),
            _ => true,
        };
        if !found {
            return Err(asset_error(format!(
                "objective placeable {placeable} ({}) has no derived data",
                objective["kind"]
            )));
        }
    }
    Ok(())
}
