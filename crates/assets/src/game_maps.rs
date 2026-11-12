//! `data/game_maps_174.json`: per map, the placeables the server must create
//! and the spawn points (`tools/extract_game_maps.py`).
//!
//! Inputs: the map headers (`09F`), the game modes (`0C5`) and rulesets
//! (`0C0`, for mode names), the entity definitions (`003`), every chunk of each map (`0BC`) with the entity
//! placeables' instance data, and the English map names of
//! `data/extracted_maps.json` (read from the build folder).
//!
//! Rules, from the client (image base 0x7FF788F10000):
//! - Map placeables get one flat index over the placeable types whose flag is
//!   set in the type table 0x7FF78BD36CE0 (builder 0x7FF78AA52900): light,
//!   area, entity, ... in that order. So the first entity has index
//!   count(light) + count(area).
//! - While the map loads, every entity placeable whose definition (003) has
//!   component bit 58 (STU 0x737C5C41, spawn point) or bit 94 (STU
//!   0xCA1FB0B8) goes into the pending list (functor 0x7FF78969B9A0), with
//!   the placeable's first identifier when it is not zero. The loading screen
//!   ends when the list is empty. The server empties it with a ch4 "from map"
//!   create of each index (entity id 0x80000000 | index).
//! - Each tick the client drops entries whose identifier is not in the game
//!   mode's m_A43573F4 (0x7FF78969EFD0). Entries without an identifier always
//!   stay.
//! - Spawn points are picked by target tag. The mode graphs give team 0 the
//!   tags 0x310 / 0x30D / 0x3CE and team 1 the tags 0x341 / 0x3CC / 0x3CD
//!   (control stages 0 / 1 / 2). Assault, escort and hybrid take the pair from
//!   the current objective: its var 10549 (0x2935) is the defenders' room and
//!   var 10550 (0x2936) the attackers' room. Team 1 attacks first; sides swap
//!   between rounds.
//!
//! The Python extractor read the entities' instance data from a DataTool XML
//! dump; here it is read from the ENTITY chunk's STUv1 blobs, with the values
//! the dump showed (see [`config_value`]).

use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    fs,
    path::Path,
};

use orpheus_stu::Value as Stu;
use serde_json::{Map, Value, json};

use crate::{
    Install, Output, Result, asset_error, guid_hex, guid_name, guid_type,
    map_chunk::{
        AREA, ENTITY_PLACEABLE, Entity, LIGHT, MAP_CHUNK, StuV1, chunk_kind, entities, guid_of,
        name_index, placeables, round, short,
    },
};

pub(crate) const PATH: &str = "data/game_maps_174.json";
/// The English map names, another derivation's output.
const NAMES: &str = "data/extracted_maps.json";

const MAP_HEADER: u16 = 0x09F;
const GAME_MODE: u16 = 0x0C5;
const MAP: u16 = 0x002;
const IDENTIFIER: u16 = 0x01C;
const TARGET_TAG: u16 = 0x06C;

/// Component bit 58.
const SPAWN: u32 = 0x737C_5C41;
/// Component bit 94.
const REQUIRED: u32 = 0xCA1F_B0B8;
/// Team of each spawn room tag.
const TEAM_TAGS: [(u64, i64); 6] = [
    (0x310, 0),
    (0x30D, 0),
    (0x3CE, 0),
    (0x341, 1),
    (0x3CC, 1),
    (0x3CD, 1),
];
/// Control stage of each spawn room tag.
const CONTROL_STAGES: [(u64, i64); 6] = [
    (0x310, 0),
    (0x341, 0),
    (0x30D, 1),
    (0x3CC, 1),
    (0x3CE, 2),
    (0x3CD, 2),
];
/// Stage of each control point tag.
const CONTROL_POINT_TAGS: [(u64, i64); 3] = [(0x41B, 0), (0x41C, 1), (0x41D, 2)];
const FFA_TAG: u64 = 0x413;
/// The control game mode: its maps' spawns get a stage.
const CONTROL_MODE: u64 = 0x17;
/// The objective graph that holds the spawn tags of assault/escort/hybrid.
const OBJECTIVE_GRAPH: u64 = 0xF4C;
/// The capture point graph (its var 0x1DDB is the point name).
const POINT_GRAPH: u64 = 0xE8E;
/// The payload's own graph.
const PAYLOAD_GRAPH: u64 = 0x1225;
/// The maps the Python extractor named itself (`NAMES_SET`), by map index;
/// all but `6C9` lack an English name in the pack's `extracted_maps.json`.
/// Their names are read from the install instead, see [`map_name`]. Other
/// maps without an English name keep the last part of their path, as in the
/// pack.
const NAMED_FROM_HEADER: [u64; 6] = [0x661, 0x751, 0x6FE, 0x79F, 0xAFE, 0x6C9];
/// The end of an Archives mission map's path (`Maps\Venice\Venice - Rialto -
/// Archives`): such a map is named with its mission's mode, see
/// [`map_name`].
const ARCHIVES: &str = " - Archives";
/// The game rulesets (`0C0`): they name a mode its `0C5` leaves unnamed.
const RULESET: u16 = 0x0C0;

// STUv1 instance data classes and their field offsets (TankLib's generated
// types).
/// STUTargetTagInstanceData: m_targetTags +8.
pub(crate) const TARGET_TAG_DATA: u32 = 0x6410_0592;
/// `STUTargetTagInstanceData.m_2746D7E4` (a teUUID): the entity's area, an
/// area placeable (`crate::map_chunk`).
pub(crate) const TARGET_AREA: usize = 24;
const SPAWN_TEAMS_DATA: u32 = 0x7D17_5935; // STU_7D175935: TeamIndex[] m_3A6C757E +8
const STATESCRIPT_DATA: u32 = 0x7552_6BC2; // STUStatescriptComponentInstanceData
/// Its two inline arrays of STUStatescriptGraphWithOverrides (40 bytes: the
/// graph at +0, the embedded STUStatescriptSchemaEntry array at +16).
const GRAPH_ARRAYS: [usize; 2] = [8, 24];
const GRAPH_SIZE: usize = 40;
const SCHEMA_ENTRY: u32 = 0x5866_C45E; // the identifier at +8, the STUConfigVar at +24

/// A map header (`09F`) as the extractor reads it.
struct Header {
    index: u64,
    state: &'static str,
    map: Option<u64>,
    /// The supported modes' indexes.
    modes: Vec<u64>,
    path: String,
    /// `m_displayName`, in English.
    display: Option<String>,
}

/// An entity placeable with its instance data.
pub(crate) struct Placed<'a> {
    pub entity: Entity<'a>,
    pub instances: Vec<(StuV1<'a>, usize)>,
}

/// The first instance of a class among the entity's.
pub(crate) fn instance<'a>(placed: &Placed<'a>, class: u32) -> Option<(StuV1<'a>, usize)> {
    placed
        .instances
        .iter()
        .find(|(stu, root)| stu.class(*root) == Some(class))
        .copied()
}

pub(crate) fn derive(install: &Install, out: &Path) -> Result<Vec<Output>> {
    let names: Map<String, Value> = serde_json::from_slice(&fs::read(out.join(NAMES))?)
        .map_err(|error| asset_error(format!("{NAMES}: {error}")))?;
    let headers = headers(install)?;
    let mode_ids = modes(install);
    let mode_names = mode_names(install);
    let chunks = chunk_index(install);
    let mut components = Components::default();
    let mut maps = Map::new();
    for header in &headers {
        let Some(map) = header.map.filter(|_| !header.modes.is_empty()) else {
            continue;
        };
        let mut entry = Map::new();
        entry.insert(
            "name".into(),
            json!(map_name(header, map, &names, &mode_names)?),
        );
        entry.insert("state".into(), json!(header.state));
        let data = map_data(install, header, map, &chunks, &mode_ids, &mut components)?;
        entry.extend(data);
        maps.insert(guid_hex(guid_of(map, MAP)), Value::Object(entry));
    }
    Ok(vec![Output::json(PATH, &Value::Object(maps))])
}

/// The map headers the install can read (the DataTool dump had only those),
/// sorted by index.
fn headers(install: &Install) -> Result<Vec<Header>> {
    let guids = install.guids(MAP_HEADER);
    let mut out = Vec::with_capacity(guids.len());
    for (guid, asset) in install.stu_all(&guids) {
        let Ok(asset) = asset else { continue };
        let root = asset
            .root()
            .ok_or_else(|| asset_error(format!("{}: no map header", guid_name(guid))))?;
        let state = match root.get("m_A125818B") {
            Some(Stu::Int(1) | Stu::UInt(1)) => "Disabled",
            _ => "Enabled",
        };
        let map = match root.get("m_map") {
            Some(Stu::Guid(map)) if *map != 0 => Some(name_index(*map)),
            _ => None,
        };
        let modes = match root.get("m_supportedGamemodes") {
            Some(Stu::Array(refs)) => refs
                .iter()
                .filter_map(|value| match value {
                    Stu::Guid(mode) if guid_type(*mode) == GAME_MODE => Some(name_index(*mode)),
                    _ => None,
                })
                .collect(),
            _ => Vec::new(),
        };
        let path = match root.get("m_mapName") {
            Some(Stu::String(path)) => path.clone(),
            _ => String::new(),
        };
        let display = match root.get("m_displayName") {
            Some(Stu::Guid(name)) => install.string(*name).filter(|name| !name.is_empty()),
            _ => None,
        };
        out.push(Header {
            index: name_index(guid),
            state,
            map,
            modes,
            path,
            display,
        });
    }
    out.sort_by_key(|header| header.index);
    Ok(out)
}

/// Game mode index -> its m_A43573F4 identifiers.
fn modes(install: &Install) -> HashMap<u64, BTreeSet<u64>> {
    mode_identifiers(install)
        .into_iter()
        .map(|(mode, identifiers)| (name_index(mode), identifiers.into_iter().collect()))
        .collect()
}

/// Every game mode the install can read (`0C5`, sorted by GUID) with its
/// m_A43573F4 identifiers in asset order: a placeable with one of them
/// exists only in the modes that list it.
pub(crate) fn mode_identifiers(install: &Install) -> Vec<(u64, Vec<u64>)> {
    let guids = install.guids(GAME_MODE);
    let mut out = Vec::with_capacity(guids.len());
    for (guid, asset) in install.stu_all(&guids) {
        let Ok(asset) = asset else { continue };
        let identifiers = match asset.root().and_then(|root| root.get("m_A43573F4")) {
            Some(Stu::Array(refs)) => refs
                .iter()
                .filter_map(|value| match value {
                    Stu::Guid(id) if guid_type(*id) == IDENTIFIER => {
                        Some(guid_of(name_index(*id), IDENTIFIER))
                    }
                    _ => None,
                })
                .collect(),
            _ => Vec::new(),
        };
        out.push((guid, identifiers));
    }
    out
}

/// Entity definition (by its lower 32 bits) -> the component STU hashes of
/// its own m_componentMap; empty when the install can't decode it.
#[derive(Default)]
struct Components(HashMap<u64, BTreeSet<u32>>);

impl Components {
    fn load(&mut self, install: &Install, definitions: impl Iterator<Item = u64>) {
        let missing: BTreeSet<u64> = definitions
            .filter(|guid| !self.0.contains_key(&(guid & 0xFFFF_FFFF)))
            .collect();
        let missing: Vec<u64> = missing.into_iter().collect();
        for (guid, asset) in install.stu_all(&missing) {
            let found = match asset
                .ok()
                .as_ref()
                .and_then(|asset| asset.root()?.get("m_componentMap").cloned())
            {
                Some(Stu::Map(entries)) => entries.iter().map(|(key, _)| *key as u32).collect(),
                _ => BTreeSet::new(),
            };
            self.0.insert(guid & 0xFFFF_FFFF, found);
        }
    }

    fn of(&self, definition: u64) -> Option<&BTreeSet<u32>> {
        self.0.get(&(definition & 0xFFFF_FFFF))
    }
}

/// Map index (lower 32 bits) -> the GUIDs of its chunks.
pub(crate) fn chunk_index(install: &Install) -> HashMap<u64, Vec<u64>> {
    let mut chunks: HashMap<u64, Vec<u64>> = HashMap::new();
    for guid in install.guids(MAP_CHUNK) {
        chunks.entry(guid & 0xFFFF_FFFF).or_default().push(guid);
    }
    chunks
}

/// The chunks of a map the install can read, as DataTool extracted them,
/// sorted by kind.
pub(crate) fn map_chunks(
    install: &Install,
    chunks: &HashMap<u64, Vec<u64>>,
    map: u64,
) -> Vec<(u64, Vec<u8>)> {
    let mut guids = chunks
        .get(&(map & 0xFFFF_FFFF))
        .cloned()
        .unwrap_or_default();
    guids.sort_by_key(|guid| chunk_kind(*guid));
    guids
        .into_iter()
        .filter_map(|guid| Some((guid, install.raw(guid).ok()?)))
        .collect()
}

/// (first entity index, entity placeables) of a map: every chunk's
/// placeables, the entities sorted by chunk kind and place in the chunk.
/// An entity's flat placeable index is the first index plus its number.
pub(crate) fn read_map<'a>(chunks: &'a [(u64, Vec<u8>)]) -> Result<(usize, Vec<Placed<'a>>)> {
    let mut lights_and_areas = 0;
    let mut out = Vec::new();
    for (guid, data) in chunks {
        let placed = placeables(data);
        lights_and_areas += placed
            .iter()
            .filter(|placeable| matches!(placeable.kind, LIGHT | AREA))
            .count();
        if placed
            .iter()
            .any(|placeable| placeable.kind == ENTITY_PLACEABLE)
        {
            for entity in entities(data)? {
                let instances = entity
                    .instances()
                    .filter_map(|stu| Some((stu, stu.root()?)))
                    .collect();
                out.push((chunk_kind(*guid), Placed { entity, instances }));
            }
        }
    }
    // Chunks come sorted by GUID and each one's entities in chunk order.
    out.sort_by_key(|(kind, _)| *kind);
    Ok((
        lights_and_areas,
        out.into_iter().map(|(_, placed)| placed).collect(),
    ))
}

fn map_data(
    install: &Install,
    header: &Header,
    map: u64,
    chunks: &HashMap<u64, Vec<u64>>,
    mode_ids: &HashMap<u64, BTreeSet<u64>>,
    components: &mut Components,
) -> Result<Map<String, Value>> {
    let chunk_data = map_chunks(install, chunks, map);
    let (base, entities) = read_map(&chunk_data)?;
    components.load(
        install,
        entities.iter().map(|placed| placed.entity.definition),
    );

    let mut placeables = Vec::new();
    let mut identifiers = Map::new();
    let mut identifier_of: BTreeMap<usize, u64> = BTreeMap::new();
    let mut mirrored = Vec::new();
    let mut spawns: Vec<Map<String, Value>> = Vec::new();
    let mut objectives: Vec<Map<String, Value>> = Vec::new();
    let mut defense_tags: BTreeMap<u64, Vec<i64>> = BTreeMap::new();
    let mut attack_tags: BTreeMap<u64, Vec<i64>> = BTreeMap::new();
    let empty = BTreeSet::new();
    for (number, placed) in entities.iter().enumerate() {
        let entity = &placed.entity;
        let flat = base + number;
        let found = components.of(entity.definition).unwrap_or(&empty);
        let graphs = graph_vars(&placed.instances);
        let position: Vec<f64> = entity.translation.iter().map(|v| short(*v)).collect();
        let rotation: Vec<f64> = entity.rotation.iter().map(|v| short(*v)).collect();
        if let Some(values) = graphs.get(&OBJECTIVE_GRAPH) {
            let order: i64 = match values.text(0x2934)? {
                Some(text) if !text.is_empty() => text
                    .parse()
                    .map_err(|_| asset_error(format!("objective order {text:?}")))?,
                _ => 0,
            };
            let tag = |key| -> Result<Option<u64>> {
                Ok(values
                    .text(key)?
                    .filter(|text| !text.is_empty())
                    .map(|text| ref_index(&text))
                    .transpose()?
                    .filter(|index| *index != 0))
            };
            let defense = tag(0x2935)?;
            let attack = tag(0x2936)?;
            let name = match graphs.get(&POINT_GRAPH) {
                Some(point) => point.text(0x1DDB)?,
                None => None,
            };
            let checkpoint = values.text(0x2931)?.is_some_and(|text| !text.is_empty());
            let tag_guid = |tag: Option<u64>| tag.map(|tag| guid_hex(guid_of(tag, TARGET_TAG)));
            let mut objective = Map::new();
            objective.insert("objective".into(), json!(order));
            objective.insert("name".into(), json!(name));
            objective.insert(
                "kind".into(),
                json!(if checkpoint { "checkpoint" } else { "point" }),
            );
            objective.insert("placeable".into(), json!(flat));
            objective.insert("position".into(), json!(position));
            objective.insert("defense_tag".into(), json!(tag_guid(defense)));
            objective.insert("attack_tag".into(), json!(tag_guid(attack)));
            objectives.push(objective);
            if let Some(defense) = defense {
                defense_tags.entry(defense).or_default().push(order);
            }
            if let Some(attack) = attack {
                attack_tags.entry(attack).or_default().push(order);
            }
        } else if let Some(point) = graphs.get(&POINT_GRAPH) {
            let stage = target_tags(&placed.instances)
                .into_iter()
                .find_map(|tag| lookup(&CONTROL_POINT_TAGS, tag));
            if let Some(stage) = stage {
                let name = match point.text(0x6D9)?.filter(|text| !text.is_empty()) {
                    Some(name) => Some(name),
                    None => point.text(0x1DDB)?,
                };
                let mut objective = Map::new();
                objective.insert("stage".into(), json!(stage));
                objective.insert("name".into(), json!(name));
                objective.insert("kind".into(), json!("control point"));
                objective.insert("placeable".into(), json!(flat));
                objective.insert("position".into(), json!(position));
                objectives.push(objective);
            }
        } else if graphs.contains_key(&PAYLOAD_GRAPH) {
            let mut objective = Map::new();
            objective.insert("kind".into(), json!("payload"));
            objective.insert("placeable".into(), json!(flat));
            objective.insert("position".into(), json!(position));
            objective.insert("rotation".into(), json!(rotation));
            objectives.push(objective);
        }
        if !found.contains(&SPAWN) && !found.contains(&REQUIRED) {
            if entity.client_id == 0 {
                mirrored.push(flat);
            }
            continue;
        }
        placeables.push(flat);
        if entity.identifier != 0 {
            identifiers.insert(flat.to_string(), json!(guid_hex(entity.identifier)));
            identifier_of.insert(flat, entity.identifier);
        }
        if !found.contains(&SPAWN) {
            continue;
        }
        let tags = target_tags(&placed.instances);
        let team = tags.iter().find_map(|tag| lookup(&TEAM_TAGS, *tag));
        let [x, y, z, w] = entity.rotation.map(f64::from);
        let yaw = round(
            (2.0 * (w * y + x * z))
                .atan2(1.0 - 2.0 * (x * x + y * y))
                .to_degrees(),
            3,
        );
        let mut spawn = Map::new();
        spawn.insert("team".into(), json!(team));
        spawn.insert("position".into(), json!(position));
        spawn.insert("yaw".into(), json!(yaw));
        spawn.insert("placeable".into(), json!(flat));
        spawn.insert("entity_index".into(), json!(number));
        spawn.insert("rotation".into(), json!(rotation));
        let tag_guids: Vec<String> = tags
            .iter()
            .map(|tag| guid_hex(guid_of(*tag, TARGET_TAG)))
            .collect();
        spawn.insert("tags".into(), json!(tag_guids));
        spawn.insert("teams".into(), json!(spawn_teams(&placed.instances)));
        if entity.identifier != 0 {
            spawn.insert("identifier".into(), json!(guid_hex(entity.identifier)));
        }
        if tags.contains(&FFA_TAG) {
            spawn.insert("ffa".into(), json!(true));
        }
        spawns.push(spawn);
    }
    // How the first team mode of the map uses each room.
    let control = header.modes.contains(&CONTROL_MODE);
    for spawn in &mut spawns {
        let tags: Vec<u64> = spawn["tags"]
            .as_array()
            .expect("tags are a list")
            .iter()
            .map(|tag| {
                u64::from_str_radix(&tag.as_str().expect("a GUID")[2..], 16).expect("hex")
                    & 0xFFFF_FFFF
            })
            .collect();
        if control {
            let stages: BTreeSet<i64> = tags
                .iter()
                .filter_map(|tag| lookup(&CONTROL_STAGES, *tag))
                .collect();
            if let Some(stage) = stages.first() {
                spawn.insert("stage".into(), json!(stage));
            }
        } else {
            let mut uses: Vec<(i64, &str)> = Vec::new();
            for tag in &tags {
                uses.extend(
                    defense_tags
                        .get(tag)
                        .into_iter()
                        .flatten()
                        .map(|k| (*k, "defense")),
                );
                uses.extend(
                    attack_tags
                        .get(tag)
                        .into_iter()
                        .flatten()
                        .map(|k| (*k, "attack")),
                );
            }
            if !uses.is_empty() {
                uses.sort();
                spawn.insert("objectives".into(), json!(uses));
            }
        }
    }
    let key = |objective: &Map<String, Value>| {
        let number = |field: &str| objective.get(field).and_then(Value::as_i64).unwrap_or(0);
        (
            objective["kind"] == "payload",
            number("stage"),
            number("objective"),
        )
    };
    objectives.sort_by_key(key);
    let mut by_mode = Map::new();
    for mode in &header.modes {
        // A header may name a mode that has no 0C5 asset in this build.
        let Some(allowed) = mode_ids.get(mode) else {
            continue;
        };
        let kept: Vec<usize> = placeables
            .iter()
            .copied()
            .filter(|placeable| {
                identifier_of
                    .get(placeable)
                    .is_none_or(|id| allowed.contains(id))
            })
            .collect();
        by_mode.insert(guid_hex(guid_of(*mode, GAME_MODE)), json!(kept));
    }
    let modes: Vec<String> = header
        .modes
        .iter()
        .map(|mode| guid_hex(guid_of(*mode, GAME_MODE)))
        .collect();
    let mut out = Map::new();
    out.insert("modes".into(), json!(modes));
    out.insert("placeables".into(), json!(placeables));
    out.insert("identifiers".into(), Value::Object(identifiers));
    out.insert("placeables_by_mode".into(), Value::Object(by_mode));
    out.insert("first_entity".into(), json!(base));
    out.insert("spawns".into(), json!(spawns));
    out.insert("objectives".into(), json!(objectives));
    out.insert("mirrored".into(), json!(mirrored));
    Ok(out)
}

fn lookup(table: &[(u64, i64)], tag: u64) -> Option<i64> {
    table
        .iter()
        .find(|(key, _)| *key == tag)
        .map(|(_, value)| *value)
}

/// A DataTool GUID text's index: `"000000002962.01C"` -> 0x2962.
fn ref_index(text: &str) -> Result<u64> {
    let digits = text.split('.').next().unwrap_or_default();
    u64::from_str_radix(digits, 16).map_err(|_| asset_error(format!("not a GUID: {text:?}")))
}

/// The map's name: the English name of its header, else the last part of
/// its path, with the path's variant in parentheses when the name lacks it.
///
/// The maps of [`NAMED_FROM_HEADER`] take their header's own name
/// (`m_displayName`) instead; an Archives mission's map (its path ends in
/// [`ARCHIVES`]) adds its first mode's name in parentheses: "Rialto
/// (Retribution)", "Havana (Storm Rising)", "King's Row (Uprising)".
fn map_name(
    header: &Header,
    map: u64,
    names: &Map<String, Value>,
    mode_names: &HashMap<u64, String>,
) -> Result<String> {
    let unnamed = |what: &str| {
        asset_error(format!(
            "{:012X}.09F: map {map:04X} has no {what}",
            header.index
        ))
    };
    if NAMED_FROM_HEADER.contains(&map) {
        let name = header.display.as_deref().ok_or_else(|| unnamed("name"))?;
        if !header.path.ends_with(ARCHIVES) {
            return Ok(name.to_owned());
        }
        let mode = header
            .modes
            .first()
            .and_then(|mode| mode_names.get(mode))
            .ok_or_else(|| unnamed("game mode name"))?;
        return Ok(format!("{name} ({mode})"));
    }
    let english = names
        .get(&format!("{:012X}.09F", header.index))
        .and_then(|entry| entry.get("Name"))
        .and_then(Value::as_str)
        .filter(|name| !name.is_empty());
    let name = english.map_or_else(
        || {
            header
                .path
                .rsplit('\\')
                .next()
                .unwrap_or_default()
                .to_owned()
        },
        str::to_owned,
    );
    Ok(match variant(&header.path) {
        Some(variant) if !name.contains(variant) => format!("{name} ({variant})"),
        _ => name,
    })
}

/// Game mode index -> its English name: the mode's `m_displayName`, else
/// the description of the first ruleset (`0C0`, by GUID) that plays it (the
/// Uprising modes `0F` and `1A` have no name of their own; their rulesets
/// say "Uprising").
fn mode_names(install: &Install) -> HashMap<u64, String> {
    let text = |value: Option<&Stu>| match value {
        Some(Stu::Guid(guid)) => install.string(*guid).filter(|text| !text.is_empty()),
        _ => None,
    };
    let mut names = HashMap::new();
    let mut rulesets = install.guids(RULESET);
    rulesets.sort_unstable();
    for (_, asset) in install.stu_all(&rulesets) {
        let Ok(asset) = asset else { continue };
        let Some(game) = asset
            .root()
            .and_then(|root| root.get("m_gamemode"))
            .and_then(|game| asset.resolve(game))
        else {
            continue;
        };
        let (Some(Stu::Guid(mode)), Some(name)) =
            (game.get("m_gamemode"), text(game.get("m_description")))
        else {
            continue;
        };
        names.entry(name_index(*mode)).or_insert(name);
    }
    for (guid, asset) in install.stu_all(&install.guids(GAME_MODE)) {
        let Ok(asset) = asset else { continue };
        if let Some(name) = text(asset.root().and_then(|root| root.get("m_displayName"))) {
            names.insert(name_index(guid), name);
        }
    }
    names
}

/// The first `(...)` of a path whose text starts with a word character
/// (Python's `\((\w[^)]*)\)`).
fn variant(path: &str) -> Option<&str> {
    let mut rest = path;
    while let Some(open) = rest.find('(') {
        let after = &rest[open + 1..];
        if let Some(close) = after.find(')') {
            let inside = &after[..close];
            if inside
                .chars()
                .next()
                .is_some_and(|c| c.is_alphanumeric() || c == '_')
            {
                return Some(inside);
            }
        }
        rest = after;
    }
    None
}

/// The target tags (`06C` indexes) of the entity's STUTargetTagInstanceData.
pub(crate) fn target_tags(instances: &[(StuV1<'_>, usize)]) -> Vec<u64> {
    let Some((stu, root)) = instances
        .iter()
        .find(|(stu, root)| stu.class(*root) == Some(TARGET_TAG_DATA))
    else {
        return Vec::new();
    };
    let Some((count, first)) = stu.array(root + 8) else {
        return Vec::new();
    };
    (0..count)
        .filter_map(|k| stu.asset_ref(first + 16 * k))
        .filter(|guid| *guid != 0 && guid_type(*guid) == TARGET_TAG)
        .map(name_index)
        .collect()
}

/// The TeamIndex names of the entity's STU_7D175935.
fn spawn_teams(instances: &[(StuV1<'_>, usize)]) -> Vec<String> {
    let Some((stu, root)) = instances
        .iter()
        .find(|(stu, root)| stu.class(*root) == Some(SPAWN_TEAMS_DATA))
    else {
        return Vec::new();
    };
    let Some((count, first)) = stu.array(root + 8) else {
        return Vec::new();
    };
    (0..count)
        .filter_map(|k| stu.i32(first + 4 * k))
        .map(team_index)
        .collect()
}

/// A TeamIndex value's name, as .NET's `Enum.ToString` gives it.
fn team_index(value: i32) -> String {
    match value {
        0 => "TeamBlue".into(),
        1 => "TeamRed".into(),
        2 => "x64EE800E".into(),
        3 => "xB50082CF".into(),
        4 => "FFA".into(),
        5 => "x3A5ECACD".into(),
        -1 => "xBC219E96".into(),
        other => other.to_string(),
    }
}

/// One graph's variable overrides: identifier index -> the STUConfigVar.
pub(crate) struct GraphVars<'a> {
    stu: StuV1<'a>,
    values: BTreeMap<u64, Option<usize>>,
}

impl GraphVars<'_> {
    /// A variable's value as the DataTool dump showed it, `None` when the
    /// graph doesn't override it.
    fn text(&self, key: u64) -> Result<Option<String>> {
        match self.values.get(&key) {
            Some(Some(value)) => config_value(&self.stu, *value),
            _ => Ok(None),
        }
    }

    /// A number override (an int, float or bool config var), `None` when
    /// the graph doesn't override the variable; an error for a value of
    /// another class.
    pub fn number(&self, key: u64) -> Result<Option<f32>> {
        let Some(Some(value)) = self.values.get(&key) else {
            return Ok(None);
        };
        let class = self.stu.class(*value).unwrap_or(0);
        let number = match class {
            INT_VAR => self.stu.i32(value + 16).map(|value| value as f32),
            FLOAT_VAR => self.stu.f32(value + 16),
            BOOL_VAR => self.stu.u8(value + 16).map(f32::from),
            other => {
                return Err(asset_error(format!(
                    "variable {key:X}: config var class {other:08X} is not a number"
                )));
            }
        };
        number
            .map(Some)
            .ok_or_else(|| asset_error(format!("variable {key:X} runs past its blob")))
    }

    /// A team override (`STU_BD6CC023`, its TeamIndex `m_6CE7A900` at +16:
    /// 0 blue, 1 red, -1 none), `None` when the graph doesn't override the
    /// variable; an error for a value of another class.
    pub fn team(&self, key: u64) -> Result<Option<i32>> {
        let Some(Some(value)) = self.values.get(&key) else {
            return Ok(None);
        };
        match self.stu.class(*value).unwrap_or(0) {
            TEAM_VAR => self
                .stu
                .i32(value + 16)
                .map(Some)
                .ok_or_else(|| asset_error(format!("variable {key:X} runs past its blob"))),
            other => Err(asset_error(format!(
                "variable {key:X}: config var class {other:08X} is not a team"
            ))),
        }
    }
}

/// The config var classes of numbers (`STUConfigVarInt`, `STUConfigVarFloat`,
/// `STUConfigVarBool`), each with its value at +16.
const INT_VAR: u32 = 0x12E4_34A1;
const FLOAT_VAR: u32 = 0x7B45_B325;
const BOOL_VAR: u32 = 0xB567_AADA;
/// The config var class of a team (`STU_BD6CC023`).
const TEAM_VAR: u32 = 0xBD6C_C023;

/// Graph index -> its variables, of the statescript instance data; a later
/// graph of the same index replaces an earlier one, a later entry of the same
/// variable an earlier one.
pub(crate) fn graph_vars<'a>(instances: &[(StuV1<'a>, usize)]) -> HashMap<u64, GraphVars<'a>> {
    let mut out = HashMap::new();
    for (stu, root) in instances {
        if stu.class(*root) != Some(STATESCRIPT_DATA) {
            continue;
        }
        for field in GRAPH_ARRAYS {
            let Some((count, first)) = stu.array(root + field) else {
                continue;
            };
            for n in 0..count {
                let graph = first + GRAPH_SIZE * n;
                let Some(guid) = stu.asset_ref(graph).filter(|guid| *guid != 0) else {
                    continue;
                };
                let mut values = BTreeMap::new();
                for entry in stu.embedded_array(graph + 16) {
                    if stu.class(entry) != Some(SCHEMA_ENTRY) {
                        continue;
                    }
                    let Some(key) = stu.asset_ref(entry + 8) else {
                        continue;
                    };
                    values.insert(name_index(key), stu.embedded(entry + 24));
                }
                out.insert(name_index(guid), GraphVars { stu: *stu, values });
            }
        }
    }
    out
}

/// A config var's value as the extractor took it from the DataTool dump: its
/// own `m_value` attribute (ints, floats, bools), else the first non-zero
/// asset reference in it (as `%012X.%03X`), else its first string. Only the
/// classes that the variables the extractor reads hold are known.
fn config_value(stu: &StuV1<'_>, value: usize) -> Result<Option<String>> {
    let class = stu.class(value).unwrap_or(0);
    let reference = |at: usize| {
        let guid = stu.asset_ref(at)?;
        (name_index(guid) != 0).then(|| guid_name(guid))
    };
    Ok(match class {
        // STUConfigVarInt: i32 m_value at +16.
        0x12E4_34A1 => stu.i32(value + 16).map(|value| value.to_string()),
        // STUConfigVarResource subclasses: m_resourceKey at +16, then their
        // own reference at +32 (STU_357DABBB target tag,
        // STUConfigVarUXDisplayText, STUConfigVarIdentifier).
        0x357D_ABBB | 0x1E2A_F12C | 0xFF80_C481 => {
            reference(value + 16).or_else(|| reference(value + 32))
        }
        // STU_92CA44CC (STUConfigVarStringBase): teString m_value at +16.
        0x92CA_44CC => stu.string(value + 16),
        other => {
            return Err(asset_error(format!(
                "config var class {other:08X} of a map objective is not known"
            )));
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_variant_starts_with_a_word_character() {
        assert_eq!(variant(r"Maps\Kings Row (Winter)"), Some("Winter"));
        assert_eq!(variant(r"Maps\X ( Y) (Z)"), Some("Z"));
        assert_eq!(variant(r"Maps\X"), None);
    }

    #[test]
    fn rounding_follows_python() {
        assert_eq!(round(-170.343_412_3, 3), -170.343);
        assert_eq!(short(0.999_998_33_f32), 0.999_998_33);
        // The float Rust writes as "-1.2621775e-29": correctly rounded to
        // eight digits it doesn't read back, so Python takes nine.
        assert_eq!(short(-1.262_177_5e-29_f32), -1.262_177_45e-29);
    }
}
