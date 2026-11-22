//! `game_graphs_174.json.gz` and `game_graphs_bots_174.json.gz`: the
//! statescript graphs the server runs, after `tools/extract_game_graphs.py`
//! and the target tag step of `lucy runtime prepare --target-tags-xml`.
//!
//! The hero bundle keeps every graph reachable from the hero bodies (their
//! statescript component's initial graphs and their weapon scripts) and from
//! the controller, hero select, HUD and mode graphs, following every graph
//! reference (a GUID of type `01B` anywhere in a kept graph) and the initial
//! graphs of every entity a kept weapon volley spawns (its projectile and
//! what the projectile leaves where it lands). It also holds
//! each mode's initial variables, the mode-level values of the game
//! rulesets that set any, and each body's authored target tags. The
//! bot bundle adds the Practice Range bot bodies with their target tags and
//! AI definitions (with the AI weapon buttons) and the graphs placed by the
//! maps' AI spawners; its hero bodies have no target tags. It keeps the
//! volley-spawned graphs too, and so the auras they apply: a bot runs the
//! auras put on it from this bundle (the Biotic Orbs' `02D5` → `1101`,
//! applied by the orb's `1100`).
//!
//! Per graph it keeps what a runtime needs and drops the editor layout
//! (positions, comments, display names, colours). The file format is
//! described in `ow174/game/script/graph.py`, which reads it.
//!
//! The Python tool read DataTool's XML (DragonML). This module walks the
//! decoded asset the way DataTool writes it, so the values come out as the
//! XML gave them:
//! - An object's fields are its class's fields, the class's own first, then
//!   each ancestor's, in declaration order; a field the asset doesn't store
//!   holds its default. Numbers, strings and nulls are XML attributes; asset
//!   references, objects, arrays, vectors and maps are child elements.
//! - Enums are numbers (the tool mapped DataTool's member names back to
//!   their values). A `float` is the shortest decimal that reads back as the
//!   same `f32`, as .NET prints it.
//! - An asset reference keeps only its type and index (`%012X.%03X`); the
//!   all-ones type is DataTool's invalid GUID, `0xFFFFFFFFFFFFFFFF`.
//! - An object is written once, at its first place in document order (fields
//!   in order, arrays and maps in order); later uses are references to it.

use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque},
    fs,
    io::Write,
    path::Path,
};

use flate2::{Compression, write::GzEncoder};
use lucy_stu::{Asset, Kind, Object, Types, Value, types::Field};
use serde_json::{Map, Value as Json, json};

use crate::{Install, Output, Result, asset_error, guid_name, guid_type};

const GRAPH: u16 = 0x01B;
const ENTITY: u16 = 0x003;
const MODE: u16 = 0x0C5;
const RULESET: u16 = 0x0C0;
const IDENTIFIER: u16 = 0x01C;
const TARGET_TAG: u16 = 0x06C;
/// Auras (`STU_76F2C784`): the graph an aura runs on the body it is on.
const AURA: u16 = 0x053;
const INDEX: u64 = 0xFFFF_FFFF_FFFF;

/// The graphs every bundle starts from besides the bodies'.
const ROOT_GRAPHS: [u64; 11] = [
    0x13C1, // Practice Range controller
    0x0C90, // PvP controller
    0x288A, // hero select host
    0x288B, // hero select screen
    0x288D, // hero select team list
    0x20E4, // weapon and ability HUD
    0x13C0, // Practice Range mode
    0x0F4A, // PvP mode
    0x0EC2, // PvP mode
    0x0C8E, // PvP mode body
    0x0258, // Biotic Field (the initial graph of 03DC.003, which 0257's canister leaves where it lands)
];

/// The match flow's graphs, also roots: the other modes' scripts, team
/// controllers and team bodies (`0C5` `m_teams`, and the script of the
/// mode entity `16B2`; `statescript_names.json` "modes"), and the map
/// entity graphs of the objectives and doors (their
/// `STUStatescriptComponentInstanceData` in the map chunks).
const MODE_FLOW_GRAPHS: [u64; 14] = [
    0x0D76, // Skirmish mode
    0x0D0B, // Elimination mode
    0x0D6F, // Elimination controller
    0x0D38, // Elimination body
    0x0F60, // Team Deathmatch mode
    0x0F62, // Team Deathmatch controller
    0x1066, // Team Deathmatch body
    0x0F73, // Deathmatch and Bounty Hunter mode
    0x0F74, // Deathmatch controller
    0x1058, // Deathmatch body
    0x0F4C, // map objective
    0x0E8E, // map capture point
    0x1225, // map payload
    0x0D32, // map spawn door
];

/// Every root graph of a bundle besides the bodies' and the AI spawners'.
fn bundle_roots() -> impl Iterator<Item = u64> {
    ROOT_GRAPHS.into_iter().chain(MODE_FLOW_GRAPHS)
}

/// `STUGraphItem`'s editor fields.
const LAYOUT: [&str; 4] = ["m_pos", "m_displayName", "m_comment", "m_F008EA57"];
const NODE_KEYS: [(&str, &str); 6] = [
    ("m_uniqueID", "uid"),
    ("m_871A8203", "rid"),
    ("m_602E1F8F", "remote"),
    ("m_26B6454C", "state"),
    ("m_clientOnly", "client_only"),
    ("m_serverOnly", "server_only"),
];
const LISTS: [(&str, &str); 3] = [
    ("m_states", "states"),
    ("m_entries", "entries"),
    ("m_remoteSyncNodes", "remote"),
];

pub(crate) fn derive(install: &Install, out: &Path) -> Result<Vec<Output>> {
    Ok(vec![
        bundle(install, out, false)?.into_output("data/game_graphs_174.json.gz")?,
        bundle(install, out, true)?.into_output("data/game_graphs_bots_174.json.gz")?,
    ])
}

struct Bundle(Json);

impl Bundle {
    /// Compact JSON, gzipped at level 9 with no timestamp, as the tool wrote it.
    fn into_output(self, path: &str) -> Result<Output> {
        let text = serde_json::to_vec(&self.0).expect("JSON values serialize");
        let mut encoder = GzEncoder::new(Vec::new(), Compression::new(9));
        encoder.write_all(&text)?;
        Ok(Output {
            path: path.into(),
            bytes: encoder.finish()?,
        })
    }
}

/// The hero bundle (`bots` false) or the bot bundle.
fn bundle(install: &Install, out: &Path, bots: bool) -> Result<Bundle> {
    let assets = Assets::new(install);
    let variables = mode_variables(install)?;
    let ruleset_variables = ruleset_variables(install)?;
    let (ai_roots, bot_entities) = if bots {
        let spawners = out.join("data/ai_spawners_174.json");
        (
            ai_graph_roots(&spawners)?,
            bot_entities(&assets, &spawners)?,
        )
    } else {
        (BTreeSet::new(), BTreeSet::new())
    };
    let heroes = read_json(&out.join("data/game_heroes_174.json"))?;
    let heroes = heroes
        .as_object()
        .ok_or_else(|| asset_error("game_heroes_174.json: expected a hero table"))?;

    let mut queue = VecDeque::new();
    let mut bodies = Map::new();
    for (hero, entry) in heroes {
        let body = entry
            .get("body")
            .and_then(Json::as_str)
            .and_then(|text| u64::from_str_radix(text.trim_start_matches("0x"), 16).ok())
            .ok_or_else(|| asset_error(format!("game_heroes_174.json: {hero} has no body")))?;
        let name = entry.get("name").cloned().unwrap_or(Json::Null);
        let asset = assets.entity(body & INDEX)?;
        let (mut record, used) = body_record(&asset, json!(hero), name)?;
        if !bots {
            let tags = target_tags(&asset)?;
            record.insert("target_tags".into(), tags.into());
        }
        bodies.insert(format!("0x{body:016X}"), Json::Object(record));
        queue.extend(used);
    }
    let mut entities = Map::new();
    if bots {
        for index in bot_entities {
            let asset = assets.entity(index)?;
            let name = format!("entity {index:04X}");
            let (mut record, used) = body_record(&asset, Json::Null, name.into())?;
            let buttons = ai_weapon_buttons(&assets, &asset)?;
            if !buttons.is_empty() {
                record.insert("ai_weapon_buttons".into(), Json::Object(buttons));
            }
            // The bots' bodies as the players' target filters test them
            // (a dead bot's `05AC` hint tests its killer's tags).
            record.insert("target_tags".into(), target_tags(&asset)?.into());
            entities.insert(
                format!("0x{:016X}", 0x0400_0000_0000_0000 | index),
                Json::Object(record),
            );
            queue.extend(used);
        }
    }
    // The server-owned map placeables: their definitions' initial graphs and
    // the graphs their instance data name (`game_map_areas_174.json`).
    let (definitions, placed_roots) = placed_entities(&out.join(crate::map_areas::PATH))?;
    for index in definitions {
        let key = format!("0x{:016X}", 0x0400_0000_0000_0000 | index);
        if entities.contains_key(&key) || !assets.entities.contains_key(&index) {
            continue;
        }
        let asset = assets.entity(index)?;
        let name = format!("entity {index:04X}");
        let (record, used) = body_record(&asset, Json::Null, name.into())?;
        if used.is_empty() {
            continue;
        }
        entities.insert(key, Json::Object(record));
        queue.extend(used);
    }
    let workshop = workshop_graphs(install)?;
    queue.extend(bundle_roots());
    queue.extend(ai_roots.iter().copied());
    queue.extend(placed_roots.iter().copied());
    queue.extend(workshop.iter().copied());

    let mut graphs = BTreeMap::new();
    let mut missing = BTreeSet::new();
    let mut spawned = BTreeSet::new();
    let mut auras = Map::new();
    while let Some(index) = queue.pop_front() {
        if graphs.contains_key(&index) || missing.contains(&index) {
            continue;
        }
        // A graph the install can't provide (`0F9C` is in a bundle whose key
        // the install lacks) is listed as missing, as the dump lacked it.
        let Some(asset) = assets.graph(index) else {
            missing.insert(index);
            continue;
        };
        let asset = asset?;
        let graph = GraphView::new(&asset);
        let record = graph.record(index)?;
        queue.extend(graph.graph_refs());
        queue.extend(aura_graphs(&assets, &record, &mut auras)?);
        queue.extend(spawned_graphs(&assets, &record, &mut spawned)?);
        graphs.insert(index, record);
    }

    let mut root_graphs: BTreeSet<u64> = bundle_roots().collect();
    root_graphs.extend(ai_roots);
    root_graphs.extend(placed_roots);
    root_graphs.extend(workshop);
    let mut body_keys: Vec<&String> = bodies.keys().collect();
    body_keys.sort();
    let mut data = Map::new();
    data.insert("version".into(), 1.into());
    data.insert(
        "roots".into(),
        json!({"bodies": body_keys, "graphs": root_graphs}),
    );
    data.insert("bodies".into(), Json::Object(bodies.clone()));
    data.insert("missing".into(), json!(missing));
    data.insert("auras".into(), Json::Object(auras));
    data.insert("graphs".into(), Json::Array(graphs.into_values().collect()));
    if !entities.is_empty() {
        data.insert("entities".into(), Json::Object(entities));
    }
    data.insert("mode_variables".into(), Json::Object(variables));
    data.insert("ruleset_variables".into(), Json::Object(ruleset_variables));
    Ok(Bundle(Json::Object(data)))
}

/// The definitions (`003` indexes) of every map's server placeables, and
/// the graphs their statescript instance data name (overrides and extra
/// graphs).
fn placed_entities(path: &Path) -> Result<(BTreeSet<u64>, BTreeSet<u64>)> {
    let table = read_json(path)?;
    let mut definitions = BTreeSet::new();
    let mut graphs = BTreeSet::new();
    let placeables = table["maps"]
        .as_object()
        .into_iter()
        .flat_map(|maps| maps.values())
        .flat_map(|map| map["server_placeables"].as_array().into_iter().flatten());
    for placeable in placeables {
        if let Some(definition) = placeable["definition"]
            .as_str()
            .and_then(|text| u64::from_str_radix(text.trim_start_matches("0x"), 16).ok())
        {
            definitions.insert(definition & INDEX);
        }
        for list in ["overrides", "extra"] {
            for binding in placeable["script"][list].as_array().into_iter().flatten() {
                graphs.extend(binding["graph"].as_u64());
            }
        }
    }
    Ok((definitions, graphs))
}

/// `054/0154`, the Workshop events and the condition (`STU_8C73C07E`).
const WORKSHOP_EVENTS: u64 = 0x154;
/// `054/0155`, the Workshop actions (`STU_B85A66BB`).
const WORKSHOP_ACTIONS: u64 = 0x155;
const GENERIC_SETTINGS: u16 = 0x054;

/// The Workshop's graphs: the condition's (`0154.m_16EC4AA9[].m_graph`,
/// the comparison `14A4`) and each action's (`0155.m_35CA5DCD[].m_graph`,
/// 211 of 222 actions; the control-flow actions have none). The server
/// starts one per action a rule runs, with the parameters in `#18739`
/// (`D:/Work/Coding/refs/reports/Workshop.md` §2); the helper graphs they
/// play are followed as references.
fn workshop_graphs(install: &Install) -> Result<BTreeSet<u64>> {
    use crate::game_queues::{Node, typed};
    let mut graphs = BTreeSet::new();
    for (index, list) in [
        (WORKSHOP_EVENTS, "m_16EC4AA9"),
        (WORKSHOP_ACTIONS, "m_35CA5DCD"),
    ] {
        let asset = install.stu(typed(index, GENERIC_SETTINGS))?;
        let root = Node::root(&asset)?;
        let entries: Vec<Node> = match root.node(list) {
            Some(single) if root.list(list).is_empty() => vec![single],
            _ => root.nodes(list).collect(),
        };
        if entries.is_empty() {
            return Err(asset_error(format!("054/{index:04X} has no {list}")));
        }
        for entry in entries {
            let graph = entry.guid("m_graph");
            if graph != 0 && guid_type(graph) == GRAPH {
                graphs.insert(graph & INDEX);
            }
        }
    }
    Ok(graphs)
}

fn read_json(path: &Path) -> Result<Json> {
    let bytes =
        fs::read(path).map_err(|error| asset_error(format!("{}: {error}", path.display())))?;
    serde_json::from_slice(&bytes)
        .map_err(|error| asset_error(format!("{}: {error}", path.display())))
}

/// The install's assets of the types the bundles read, by index.
struct Assets<'a> {
    install: &'a Install,
    graphs: HashMap<u64, u64>,
    entities: HashMap<u64, u64>,
    auras: HashMap<u64, u64>,
}

impl<'a> Assets<'a> {
    fn new(install: &'a Install) -> Self {
        let by_index = |kind| {
            install
                .guids(kind)
                .into_iter()
                .map(|guid| (guid & INDEX, guid))
                .collect()
        };
        Self {
            install,
            graphs: by_index(GRAPH),
            entities: by_index(ENTITY),
            auras: by_index(AURA),
        }
    }

    /// An aura, or `None` when the install has no such aura.
    fn aura(&self, index: u64) -> Option<Result<Asset>> {
        let guid = *self.auras.get(&index)?;
        Some(self.install.stu(guid))
    }

    /// A graph, or `None` when the install has or can read no such graph.
    fn graph(&self, index: u64) -> Option<Result<Asset>> {
        let guid = *self.graphs.get(&index)?;
        let bytes = self.install.raw(guid).ok()?;
        Some(
            lucy_stu::read(&bytes)
                .map_err(|error| asset_error(format!("{}: {error}", guid_name(guid)))),
        )
    }

    fn entity(&self, index: u64) -> Result<Asset> {
        let guid = self
            .entities
            .get(&index)
            .ok_or_else(|| asset_error(format!("no entity definition {index:012X}.003")))?;
        self.install.stu(*guid)
    }
}

/// An asset reference as the bundle writes it: type and index only.
fn guid_text(guid: u64) -> String {
    if guid_type(guid) == 0x1000 {
        return format!("0x{:016X}", u64::MAX);
    }
    format!("0x{:016X}", guid & 0x0FFF_FFFF_FFFF_FFFF)
}

/// The fields of an object in DataTool's order, each with its value.
fn fields(object: &Object) -> impl Iterator<Item = (&'static Field, &Value)> {
    Types::get().fields(object.class).map(move |field| {
        let value = object
            .fields
            .iter()
            .find(|(hash, _)| *hash == field.hash)
            .map_or(&field.default, |(_, value)| value);
        (field, value)
    })
}

fn field<'a>(object: &'a Object, name: &str) -> Option<&'a Value> {
    object.get(name)
}

fn class_name(object: &Object) -> &'static str {
    object.class_name().unwrap_or("STUInstance")
}

/// The object's address: the identity DataTool's references follow.
fn address(object: &Object) -> usize {
    std::ptr::from_ref(object) as usize
}

/// A plug (an `STUGraphPlug`): its class has both `m_links` and `m_parentNode`.
fn is_plug(object: &Object) -> bool {
    let types = Types::get();
    types.field_named(object.class, "m_links").is_some()
        && types.field_named(object.class, "m_parentNode").is_some()
}

/// A value DataTool writes as an attribute: a number, a string or null.
fn is_attribute(asset: &Asset, value: &Value) -> bool {
    match value {
        Value::Null | Value::Int(_) | Value::UInt(_) | Value::Float(_) | Value::String(_) => true,
        Value::Instance(_) => asset.resolve(value).is_none(),
        _ => false,
    }
}

/// An attribute's value as the tool parsed DataTool's text.
fn scalar(field: &Field, value: &Value) -> Result<Json> {
    Ok(match value {
        Value::Int(number) => (*number).into(),
        Value::UInt(number) => (*number).into(),
        Value::Float(number) => float(field.ty.kind == Kind::F32, *number)?,
        Value::String(text) => scalar_text(text),
        _ => Json::Null,
    })
}

/// A float as .NET prints it (the shortest round trip of its own width),
/// read back as the tool did.
fn float(single: bool, number: f64) -> Result<Json> {
    if number.is_nan() {
        return Ok("NaN".into());
    }
    if !number.is_finite() {
        return Err(asset_error("nonfinite numeric value"));
    }
    let number = if single {
        format!("{}", number as f32)
            .parse()
            .expect("a float reads back")
    } else {
        number
    };
    Ok(serde_json::Number::from_f64(number).map_or(Json::Null, Json::Number))
}

/// The tool's reading of attribute text: `{null}`, an integer, a float (a
/// dot or comma decimal separator), or the text itself.
fn scalar_text(text: &str) -> Json {
    if text == "{null}" {
        return Json::Null;
    }
    let unsigned = text.strip_prefix(['+', '-']).unwrap_or(text);
    if !unsigned.is_empty() && unsigned.bytes().all(|byte| byte.is_ascii_digit()) {
        if let Ok(number) = text.parse::<i64>() {
            return number.into();
        }
        if let Ok(number) = text.trim_start_matches('+').parse::<u64>() {
            return number.into();
        }
    }
    if is_float_text(unsigned) {
        if let Ok(number) = text.replace(',', ".").parse::<f64>() {
            if let Some(number) = serde_json::Number::from_f64(number) {
                return Json::Number(number);
            }
        }
    }
    Json::String(text.to_owned())
}

/// `(\d+([.,]\d*)?|[.,]\d+)([Ee][+-]?\d+)?`
fn is_float_text(text: &str) -> bool {
    let (mantissa, exponent) = match text.find(['e', 'E']) {
        Some(at) => (&text[..at], Some(&text[at + 1..])),
        None => (text, None),
    };
    let digits = |part: &str| part.bytes().all(|byte| byte.is_ascii_digit());
    let mantissa_ok = match mantissa.find(['.', ',']) {
        Some(at) => {
            let (whole, fraction) = (&mantissa[..at], &mantissa[at + 1..]);
            digits(whole)
                && digits(fraction)
                && !(whole.is_empty() && fraction.is_empty())
                && (!whole.is_empty() || !fraction.is_empty())
        }
        None => !mantissa.is_empty() && digits(mantissa),
    };
    let exponent_ok = exponent.is_none_or(|part| {
        let part = part.strip_prefix(['+', '-']).unwrap_or(part);
        !part.is_empty() && digits(part)
    });
    mantissa_ok && exponent_ok
}

/// A vector, colour or quaternion: an object of its components.
fn vector(field: &Field, values: &[f32]) -> Result<Json> {
    let names: &[&str] = match field.type_name {
        "teVec2" => &["X", "Y"],
        "teVec3" => &["X", "Y", "Z"],
        "teVec3A" | "teVec4" | "teQuat" => &["X", "Y", "Z", "W"],
        "teColorRGB" => &["R", "G", "B"],
        "teColorRGBA" => &["R", "G", "B", "A"],
        other => return Err(asset_error(format!("unsupported vector type {other}"))),
    };
    let mut out = Map::new();
    out.insert("$".into(), field.type_name.into());
    for (name, value) in names.iter().zip(values) {
        out.insert((*name).into(), float(true, f64::from(*value))?);
    }
    Ok(Json::Object(out))
}

/// What one asset's DataTool XML holds, in document order: each object at
/// its first place (with the field name it was written under; `None` for an
/// array element or the root, `dragon:value` for a map value) and every
/// asset reference.
struct Document<'a> {
    objects: Vec<(&'a Object, Option<&'static str>)>,
    guids: Vec<u64>,
}

impl<'a> Document<'a> {
    fn new(asset: &'a Asset) -> Self {
        let mut document = Self {
            objects: Vec::new(),
            guids: Vec::new(),
        };
        let mut seen = HashSet::new();
        if let Some(root) = asset.root() {
            document.object(asset, root, None, &mut seen);
        }
        document
    }

    fn object(
        &mut self,
        asset: &'a Asset,
        object: &'a Object,
        name: Option<&'static str>,
        seen: &mut HashSet<usize>,
    ) {
        if !seen.insert(address(object)) {
            return;
        }
        self.objects.push((object, name));
        for (field, value) in fields(object) {
            self.value(asset, value, Some(field.name), seen);
        }
    }

    fn value(
        &mut self,
        asset: &'a Asset,
        value: &'a Value,
        name: Option<&'static str>,
        seen: &mut HashSet<usize>,
    ) {
        match value {
            Value::Guid(guid) => self.guids.push(*guid),
            Value::Instance(_) | Value::Object(_) => {
                if let Some(object) = asset.resolve(value) {
                    self.object(asset, object, name, seen);
                }
            }
            Value::Array(items) => {
                for item in items {
                    self.value(asset, item, None, seen);
                }
            }
            Value::Map(entries) => {
                for (_, index) in entries {
                    if let Some(object) =
                        index.and_then(|index| asset.instances.get(index)?.as_ref())
                    {
                        self.object(asset, object, Some("dragon:value"), seen);
                    }
                }
            }
            _ => {}
        }
    }
}

/// An asset in the tool's generic form; with a graph's nodes and items, a
/// reference to one is `{"node": position}` or `{"item": position}` and a
/// plug lists its links.
struct View<'a> {
    asset: &'a Asset,
    nodes: HashMap<usize, usize>,
    items: HashMap<usize, usize>,
    /// The plugs each node holds: plug address -> (node position, field path).
    owners: HashMap<usize, Vec<(usize, String)>>,
    /// The field name each object was first written under.
    names: HashMap<usize, Option<&'static str>>,
    links: bool,
}

impl<'a> View<'a> {
    fn plain(asset: &'a Asset) -> Self {
        Self {
            asset,
            nodes: HashMap::new(),
            items: HashMap::new(),
            owners: HashMap::new(),
            names: HashMap::new(),
            links: false,
        }
    }

    fn resolve(&self, value: &'a Value) -> Option<&'a Object> {
        self.asset.resolve(value)
    }

    /// The object a field holds, or `None`.
    fn object(&self, object: &'a Object, name: &str) -> Option<&'a Object> {
        self.resolve(field(object, name)?)
    }

    /// Every field but `skip`, in the generic form.
    fn fields(&self, object: &'a Object, skip: &[&str]) -> Result<Map<String, Json>> {
        self.fields_where(object, |field| !skip.contains(&field.name))
    }

    fn fields_where(
        &self,
        object: &'a Object,
        keep: impl Fn(&Field) -> bool,
    ) -> Result<Map<String, Json>> {
        let mut out = Map::new();
        let mut encoding = Vec::new();
        for (field, value) in fields(object) {
            if keep(field) {
                out.insert(field.name.into(), self.value(field, value, &mut encoding)?);
            }
        }
        Ok(out)
    }

    /// A field or array element: null, a number, a string, a list, an asset
    /// GUID, `{"node": position}`, `{"item": position}`, or an object
    /// `{"$": class, ...}`.
    fn value(&self, field: &Field, value: &'a Value, encoding: &mut Vec<usize>) -> Result<Json> {
        Ok(match value {
            Value::Null | Value::Int(_) | Value::UInt(_) | Value::Float(_) | Value::String(_) => {
                scalar(field, value)?
            }
            Value::Guid(guid) => guid_text(*guid).into(),
            Value::Array(items) => Json::Array(
                items
                    .iter()
                    .map(|item| self.value(field, item, encoding))
                    .collect::<Result<_>>()?,
            ),
            Value::Floats(values) => vector(field, values)?,
            // A hash map (`STU_25CBEFB0`'s `m_108E51DD` in `1B4A`: the
            // Workshop's values by their `01C` identifier): each key with
            // its object.
            Value::Map(entries) => json!({
                "$": "map",
                "entries": entries
                    .iter()
                    .map(|&(key, index)| {
                        let object = match index.and_then(|index| self.asset.instances.get(index)?.as_ref()) {
                            Some(object) => self.object_value(object, encoding)?,
                            None => Json::Null,
                        };
                        Ok(json!([guid_text(key), object]))
                    })
                    .collect::<Result<Vec<_>>>()?,
            }),
            // A map placeable's UUID (`STUConfigVarEntityReference`, in the
            // map entity graphs): its bytes in hex.
            Value::Bytes(bytes) if field.type_name == "teUUID" => bytes
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
                .into(),
            Value::Bytes(_) => {
                return Err(asset_error(format!(
                    "unsupported {} value in {}",
                    field.type_name, field.name
                )));
            }
            Value::Instance(_) | Value::Object(_) => match self.resolve(value) {
                None => Json::Null,
                Some(object) => self.object_value(object, encoding)?,
            },
        })
    }

    fn object_value(&self, object: &'a Object, encoding: &mut Vec<usize>) -> Result<Json> {
        let key = address(object);
        if let Some(position) = self.nodes.get(&key) {
            return Ok(json!({ "node": position }));
        }
        if let Some(position) = self.items.get(&key) {
            return Ok(json!({ "item": position }));
        }
        if is_plug(object) {
            return self.plug(object, encoding);
        }
        if encoding.contains(&key) {
            return Err(asset_error(format!(
                "reference cycle through {}",
                class_name(object)
            )));
        }
        encoding.push(key);
        let mut out = Map::new();
        out.insert("$".into(), class_name(object).into());
        for (field, value) in fields(object) {
            out.insert(field.name.into(), self.value(field, value, encoding)?);
        }
        encoding.pop();
        Ok(Json::Object(out))
    }

    /// A plug: its fields and `links`, the `[node position, input plug path]`
    /// of each link in order (empty outside a graph).
    fn plug(&self, object: &'a Object, encoding: &mut Vec<usize>) -> Result<Json> {
        let mut out = Map::new();
        out.insert("$".into(), class_name(object).into());
        for (field, value) in fields(object) {
            if field.name != "m_links" && field.name != "m_parentNode" {
                out.insert(field.name.into(), self.value(field, value, encoding)?);
            }
        }
        let mut links = Vec::new();
        if self.links {
            if let Some(Value::Array(items)) = field(object, "m_links") {
                for link in items {
                    links.push(self.target(self.resolve(link)));
                }
            }
        }
        out.insert("links".into(), Json::Array(links));
        Ok(Json::Object(out))
    }

    /// `[node position, the input plug's field path in that node]` of a
    /// link; `[null, name]` when the plug belongs to no node of `m_nodes`,
    /// where `name` is the field the plug was first written under.
    ///
    /// The input plug's `m_parentNode` is the node; without one, the node
    /// whose field holds the plug. The XML often writes an input plug inside
    /// the link that first uses it, so ownership comes from those fields and
    /// not from where the plug sits. Graphs `288B`, `05AD` and `0D24` have
    /// input plugs of no known node; their targets are not guessed.
    fn target(&self, link: Option<&'a Object>) -> Json {
        let Some(plug) = link.and_then(|link| self.object(link, "m_inputPlug")) else {
            return json!([null, null]);
        };
        let key = address(plug);
        let name = self.names.get(&key).copied().flatten();
        let owners = self.owners.get(&key).map_or(&[][..], Vec::as_slice);
        let parent = self.object(plug, "m_parentNode");
        if let Some(position) = parent.and_then(|parent| self.nodes.get(&address(parent))) {
            return match owners.iter().find(|(owner, _)| owner == position) {
                Some((_, path)) => json!([position, path]),
                None => json!([position, name]),
            };
        }
        if parent.is_none() {
            if let Some((position, path)) = owners.first() {
                return json!([position, path]);
            }
        }
        json!([null, name])
    }

    /// `(field path, plug)` for every plug a node or one of its sub-objects
    /// holds.
    fn plugs_in(&self, object: &'a Object, prefix: &str, out: &mut Vec<(String, &'a Object)>) {
        for (field, value) in fields(object) {
            if !is_attribute(self.asset, value) {
                self.plugs_below(value, &format!("{prefix}{}", field.name), out);
            }
        }
    }

    fn plugs_below(&self, value: &'a Value, path: &str, out: &mut Vec<(String, &'a Object)>) {
        match value {
            Value::Array(items) => {
                for (number, item) in items.iter().enumerate() {
                    self.plugs_below(item, &format!("{path}[{number}]"), out);
                }
            }
            Value::Instance(_) | Value::Object(_) => {
                let Some(object) = self.resolve(value) else {
                    return;
                };
                let key = address(object);
                if self.nodes.contains_key(&key) || self.items.contains_key(&key) {
                    return;
                }
                if is_plug(object) {
                    out.push((path.to_owned(), object));
                } else {
                    self.plugs_in(object, &format!("{path}."), out);
                }
            }
            _ => {}
        }
    }
}

/// A `01B` graph: its nodes, items and plugs.
///
/// Nodes live in `m_nodes` (the index is the node's `m_871A8203`) and in
/// `m_graph.m_items` with the editor's other items (subgraph boxes). A node
/// is identified by its position in `m_items`.
struct GraphView<'a> {
    view: View<'a>,
    root: &'a Object,
    item_list: Vec<Option<&'a Object>>,
    node_list: Vec<Option<&'a Object>>,
    guids: Vec<u64>,
}

impl<'a> GraphView<'a> {
    fn new(asset: &'a Asset) -> Self {
        static EMPTY: Object = Object {
            class: 0,
            fields: Vec::new(),
            stored: 0,
        };
        let mut view = View::plain(asset);
        view.links = true;
        let root = asset.root().unwrap_or(&EMPTY);
        let list = |value: Option<&'a Value>| -> Vec<Option<&'a Object>> {
            match value {
                Some(Value::Array(items)) => items.iter().map(|item| asset.resolve(item)).collect(),
                _ => Vec::new(),
            }
        };
        let item_list = list(
            view.object(root, "m_graph")
                .and_then(|graph| field(graph, "m_items")),
        );
        let node_list = list(field(root, "m_nodes"));
        let known: HashSet<usize> = node_list
            .iter()
            .flatten()
            .map(|node| address(node))
            .collect();
        for (position, item) in item_list.iter().enumerate() {
            let Some(item) = item else { continue };
            let key = address(item);
            if known.contains(&key) {
                view.nodes.insert(key, position);
            } else {
                view.items.insert(key, position);
            }
        }
        for node in node_list.iter().flatten() {
            if let Some(&position) = view.nodes.get(&address(node)) {
                let mut plugs = Vec::new();
                view.plugs_in(node, "", &mut plugs);
                for (path, plug) in plugs {
                    view.owners
                        .entry(address(plug))
                        .or_default()
                        .push((position, path));
                }
            }
        }
        let document = Document::new(asset);
        view.names = document
            .objects
            .iter()
            .map(|(object, name)| (address(object), *name))
            .collect();
        Self {
            view,
            root,
            item_list,
            node_list,
            guids: document.guids,
        }
    }

    fn record(&self, index: u64) -> Result<Json> {
        let view = &self.view;
        let root = self.root;
        let mut header = Map::new();
        for (field, value) in fields(root) {
            if is_attribute(view.asset, value) {
                let value = scalar(field, value)?;
                if !value.is_null() {
                    header.insert(field.name.into(), value);
                }
            }
        }
        let skip = [
            "m_graph",
            "m_nodes",
            "m_syncVars",
            "m_states",
            "m_entries",
            "m_remoteSyncNodes",
        ];
        let root_fields = view.fields_where(root, |field| !skip.contains(&field.name))?;
        let root_fields: Map<String, Json> = root_fields
            .into_iter()
            .filter(|(name, _)| {
                let value = field(root, name).expect("a field of the root");
                !is_attribute(view.asset, value)
            })
            .collect();
        let mut containers = Map::new();
        let mut items: Vec<(&usize, &usize)> = view
            .items
            .iter()
            .filter(|(key, _)| {
                self.item_list
                    .iter()
                    .flatten()
                    .any(|item| address(item) == **key)
            })
            .collect();
        items.sort_by_key(|(_, position)| **position);
        for (_, position) in items {
            let item = self.item_list[*position].expect("an item");
            containers.insert(position.to_string(), class_name(item).into());
        }
        let mut nodes: Vec<(&'a Object, usize)> = self
            .node_list
            .iter()
            .flatten()
            .filter_map(|node| Some((*node, *view.nodes.get(&address(node))?)))
            .collect();
        nodes.sort_by_key(|(_, position)| *position);
        let nodes = nodes
            .into_iter()
            .map(|(node, position)| self.node(node, position))
            .collect::<Result<Vec<_>>>()?;
        let unknown: Vec<usize> = self
            .node_list
            .iter()
            .enumerate()
            .filter(|(_, node)| node.is_none())
            .map(|(rid, _)| rid)
            .collect();

        let mut record = Map::new();
        record.insert("index".into(), index.into());
        record.insert("header".into(), Json::Object(header));
        record.insert("fields".into(), Json::Object(root_fields));
        record.insert("items".into(), self.item_list.len().into());
        record.insert("containers".into(), Json::Object(containers));
        record.insert("nodes".into(), Json::Array(nodes));
        record.insert("unknown_nodes".into(), json!(unknown));
        for (name, short) in LISTS {
            record.insert(short.into(), self.positions(name));
        }
        record.insert("sync_vars".into(), self.sync_vars()?);
        Ok(Json::Object(record))
    }

    fn node(&self, node: &'a Object, position: usize) -> Result<Json> {
        let mut record = Map::new();
        record.insert("pos".into(), position.into());
        record.insert("class".into(), class_name(node).into());
        for (name, short) in NODE_KEYS {
            let value = match Types::get().field_named(node.class, name) {
                Some(field_type) => match field(node, name) {
                    Some(value) if is_attribute(self.view.asset, value) => {
                        scalar(field_type, value)?
                    }
                    _ => Json::Null,
                },
                None => Json::Null,
            };
            record.insert(short.into(), value);
        }
        let skip: Vec<&str> = LAYOUT
            .iter()
            .copied()
            .chain(NODE_KEYS.iter().map(|(name, _)| *name))
            .collect();
        record.insert(
            "fields".into(),
            Json::Object(self.view.fields(node, &skip)?),
        );
        Ok(Json::Object(record))
    }

    fn positions(&self, name: &str) -> Json {
        let Some(Value::Array(items)) = field(self.root, name) else {
            return json!([]);
        };
        Json::Array(
            items
                .iter()
                .map(|item| {
                    self.view
                        .resolve(item)
                        .and_then(|object| self.view.nodes.get(&address(object)))
                        .map_or(Json::Null, |position| (*position).into())
                })
                .collect(),
        )
    }

    fn sync_vars(&self) -> Result<Json> {
        let Some(Value::Array(entries)) = field(self.root, "m_syncVars") else {
            return Ok(json!([]));
        };
        let mut out = Vec::new();
        for entry in entries {
            let entry = self
                .view
                .resolve(entry)
                .ok_or_else(|| asset_error("a null sync variable"))?;
            let var = match field(entry, "m_0D09D2D9") {
                Some(Value::Guid(guid)) => json!(guid & INDEX),
                _ => Json::Null,
            };
            let fields = self.view.fields(entry, &["m_0D09D2D9"])?;
            out.push(json!([var, fields["m_56341592"], fields["m_AC9480C7"]]));
        }
        Ok(Json::Array(out))
    }

    /// The graphs this graph references: every `01B` GUID in its document.
    fn graph_refs(&self) -> BTreeSet<u64> {
        self.guids
            .iter()
            .filter(|guid| **guid != 0 && guid_type(**guid) == GRAPH)
            .map(|guid| guid & INDEX)
            .collect()
    }
}

/// The graph index a field names, or null.
fn graph_ref(object: &Object, name: &str) -> Json {
    match field(object, name) {
        Some(Value::Guid(guid)) => json!(guid & INDEX),
        _ => Json::Null,
    }
}

/// A body definition (`003`): its statescript component's initial graphs in
/// order, with their schema overrides, and its weapon component's scripts;
/// and the graphs it uses.
fn body_record(asset: &Asset, hero: Json, name: Json) -> Result<(Map<String, Json>, Vec<u64>)> {
    let view = View::plain(asset);
    let mut graphs = Vec::new();
    let mut weapons = Vec::new();
    let mut manager = Json::Null;
    for (object, written_as) in Document::new(asset).objects {
        match class_name(object) {
            "STUStatescriptComponent" => {
                let Some(Value::Array(entries)) = field(object, "m_B634821A") else {
                    continue;
                };
                for entry in entries {
                    let Some(entry) = view.resolve(entry) else {
                        continue;
                    };
                    let mut record = Map::new();
                    record.insert("graph".into(), graph_ref(entry, "m_graph"));
                    record.extend(view.fields(entry, &["m_graph"])?);
                    graphs.push(Json::Object(record));
                }
            }
            "STUWeaponComponent" if written_as == Some("dragon:value") => {
                manager = graph_ref(object, "m_managerScript");
                if let Some(Value::Array(list)) = field(object, "m_weapons") {
                    for weapon in list {
                        if let Some(weapon) = view.resolve(weapon) {
                            weapons.push(graph_ref(weapon, "m_script"));
                        }
                    }
                }
            }
            _ => {}
        }
    }
    let mut used: BTreeSet<u64> = graphs
        .iter()
        .filter_map(|entry| entry["graph"].as_u64())
        .collect();
    used.extend(manager.as_u64());
    used.extend(weapons.iter().filter_map(Json::as_u64));
    let mut record = Map::new();
    record.insert("hero".into(), hero);
    record.insert("name".into(), name);
    record.insert("graphs".into(), Json::Array(graphs));
    record.insert("manager".into(), manager);
    record.insert("weapons".into(), Json::Array(weapons));
    Ok((record, used.into_iter().collect()))
}

/// The initial graphs (`STUStatescriptComponent.m_B634821A[].m_graph`) of
/// the entities a hero weapon volley spawns and the bundle hasn't followed
/// yet: its projectile (`m_projectileEntity`) and what the projectile leaves
/// where it lands (`m_49CBA3BC`), each an `STU_8556841E` definition.
/// `0045.003`, Tracer's Pulse Bomb, starts `0C1B`; `03DC.003`, the Biotic
/// Field, starts `0258`. A definition the install lacks adds nothing.
fn spawned_graphs(assets: &Assets, record: &Json, seen: &mut BTreeSet<u64>) -> Result<Vec<u64>> {
    let mut entities = BTreeSet::new();
    for node in record["nodes"].as_array().into_iter().flatten() {
        if node["class"] != "STUStatescriptStateWeaponVolley" {
            continue;
        }
        for name in ["m_projectileEntity", "m_49CBA3BC"] {
            entity_definitions(&node["fields"][name], &mut entities);
        }
    }
    let mut out = Vec::new();
    for entity in entities {
        if !seen.insert(entity) || !assets.entities.contains_key(&entity) {
            continue;
        }
        let asset = assets.entity(entity)?;
        let Some(component) = component(&asset, "STUStatescriptComponent")? else {
            continue;
        };
        let Some(Value::Array(entries)) = field(component, "m_B634821A") else {
            continue;
        };
        for entry in entries.iter().filter_map(|entry| asset.resolve(entry)) {
            if let Some(Value::Guid(graph)) = field(entry, "m_graph") {
                if *graph != 0 && guid_type(*graph) == GRAPH {
                    out.push(graph & INDEX);
                }
            }
        }
    }
    Ok(out)
}

/// The auras a graph applies (each `STU_0E766B33` in its nodes: an
/// ApplyAura state's `m_A76A8CF6`, a health change's `m_277A4ECA`, as the
/// Biotic Grenade's explosion in `0B97` names `0267`), each recorded once
/// in `auras` with its fields in the generic form, by GUID; returns the
/// graphs the new ones run (`m_5A4051BC`, and `m_14AA219A`, which the
/// auras without the first name). An aura the install lacks adds nothing.
fn aura_graphs(assets: &Assets, record: &Json, auras: &mut Map<String, Json>) -> Result<Vec<u64>> {
    let mut named = BTreeSet::new();
    aura_refs(&record["nodes"], &mut named);
    let mut out = Vec::new();
    for guid in named {
        let key = format!("0x{guid:016X}");
        if auras.contains_key(&key) {
            continue;
        }
        let Some(asset) = assets.aura(guid & INDEX) else {
            continue;
        };
        let asset = asset?;
        let root = asset
            .root()
            .ok_or_else(|| asset_error(format!("aura {key} has no root")))?;
        let mut fields = View::plain(&asset).fields(root, &[])?;
        fields.insert("$".into(), class_name(root).into());
        for name in ["m_5A4051BC", "m_14AA219A"] {
            if let Some(Value::Guid(graph)) = field(root, name) {
                if *graph != 0 && guid_type(*graph) == GRAPH {
                    out.push(graph & INDEX);
                }
            }
        }
        auras.insert(key, Json::Object(fields));
    }
    Ok(out)
}

/// Every aura GUID an `STU_0E766B33` in a bundle field names.
fn aura_refs(config: &Json, found: &mut BTreeSet<u64>) {
    match config {
        Json::Object(fields) => {
            if fields.get("$").and_then(Json::as_str) == Some("STU_0E766B33") {
                let guid = fields
                    .get("m_A76A8CF6")
                    .and_then(Json::as_str)
                    .and_then(|text| u64::from_str_radix(text.trim_start_matches("0x"), 16).ok());
                if let Some(guid) = guid.filter(|&guid| guid_type(guid) == AURA) {
                    found.insert(guid);
                }
            }
            for value in fields.values() {
                aura_refs(value, found);
            }
        }
        Json::Array(values) => {
            for value in values {
                aura_refs(value, found);
            }
        }
        _ => {}
    }
}

/// Every entity (`003`) index an `STU_8556841E` in a bundle field names, on
/// both sides of a condition.
fn entity_definitions(config: &Json, found: &mut BTreeSet<u64>) {
    match config {
        Json::Object(fields) => {
            if fields.get("$").and_then(Json::as_str) == Some("STU_8556841E") {
                let guid = fields
                    .get("m_entityDef")
                    .and_then(Json::as_str)
                    .and_then(|text| u64::from_str_radix(text.trim_start_matches("0x"), 16).ok());
                if let Some(guid) = guid.filter(|&guid| guid_type(guid) == ENTITY) {
                    found.insert(guid & INDEX);
                }
            }
            for value in fields.values() {
                entity_definitions(value, found);
            }
        }
        Json::Array(values) => {
            for value in values {
                entity_definitions(value, found);
            }
        }
        _ => {}
    }
}

/// The component of `class` in an entity definition's component map: none,
/// or the one entry of that class.
fn component<'a>(asset: &'a Asset, class: &str) -> Result<Option<&'a Object>> {
    let root = asset
        .root()
        .filter(|root| class_name(root) == "STUEntityDefinition")
        .ok_or_else(|| asset_error("expected a STUEntityDefinition root"))?;
    let entries = match field(root, "m_componentMap") {
        None | Some(Value::Null) => return Ok(None),
        Some(Value::Map(entries)) => entries,
        Some(_) => return Err(asset_error("invalid entity component map")),
    };
    let mut found = entries
        .iter()
        .filter_map(|(_, index)| asset.instances.get((*index)?)?.as_ref())
        .filter(|object| class_name(object) == class);
    let first = found.next();
    if found.next().is_some() {
        return Err(asset_error(format!("duplicate {class} components")));
    }
    Ok(first)
}

/// A body's authored target tags (`STUTargetTagComponent.m_targetTags`),
/// sorted and without duplicates or nulls, as `lucy runtime prepare
/// --target-tags-xml` added them to the hero bundle.
fn target_tags(asset: &Asset) -> Result<Vec<Json>> {
    let mut tags = BTreeSet::new();
    let Some(component) = component(asset, "STUTargetTagComponent")? else {
        return Ok(Vec::new());
    };
    let items = match field(component, "m_targetTags") {
        None | Some(Value::Null) => return Ok(Vec::new()),
        Some(Value::Array(items)) => items,
        Some(_) => return Err(asset_error("invalid target tag array")),
    };
    for item in items {
        let Value::Guid(guid) = item else {
            return Err(asset_error("expected a target tag resource reference"));
        };
        if *guid == 0 {
            continue;
        }
        if guid_type(*guid) != TARGET_TAG {
            return Err(asset_error(format!(
                "invalid .06C target tag GUID {}",
                guid_name(*guid)
            )));
        }
        tags.insert(guid & 0x0FFF_FFFF_FFFF_FFFF);
    }
    Ok(tags.into_iter().map(|tag| guid_text(tag).into()).collect())
}

/// The logical button for each slot in the body's associated AI definition
/// (`m_8B9A461F`, a `003`): its `STU_993B1B10` component's
/// `STUAIWeaponFireControlInfo` rows.
fn ai_weapon_buttons(assets: &Assets, asset: &Asset) -> Result<Map<String, Json>> {
    let root = asset
        .root()
        .ok_or_else(|| asset_error("an entity definition without a root"))?;
    let Some(guid) = ai_definition(root)? else {
        return Ok(Map::new());
    };
    let ai = assets.entity(guid & INDEX)?;
    let mut buttons = Map::new();
    let Some(component) = component(&ai, "STU_993B1B10")? else {
        return Ok(buttons);
    };
    let rows = match field(component, "m_CD486A71") {
        None | Some(Value::Null) => return Ok(buttons),
        Some(Value::Array(rows)) => rows,
        Some(_) => return Err(asset_error("invalid AI weapon control m_CD486A71 array")),
    };
    for row in rows {
        let row = ai
            .resolve(row)
            .filter(|row| class_name(row) == "STUAIWeaponFireControlInfo")
            .ok_or_else(|| asset_error("invalid AI weapon control row"))?;
        let slot = match field(row, "m_A60150E2") {
            Some(Value::UInt(slot)) => *slot,
            Some(Value::Int(slot)) if *slot >= 0 => *slot as u64,
            _ => return Err(asset_error("invalid AI weapon slot m_A60150E2")),
        };
        let button = match field(row, "m_4F5E7D4C") {
            Some(Value::UInt(button)) => *button,
            Some(Value::Int(button)) if *button >= 0 => *button as u64,
            _ => {
                return Err(asset_error(format!(
                    "invalid AI weapon logical button for slot {slot}"
                )));
            }
        };
        if buttons.insert(slot.to_string(), button.into()).is_some() {
            return Err(asset_error(format!("duplicate AI weapon slot {slot}")));
        }
    }
    Ok(buttons)
}

/// Each game mode's (`0C5`) initial variables: its first table
/// (`m_3CE93B76`, [`var_value_pairs`]), keeping explicit empty modes.
fn mode_variables(install: &Install) -> Result<Map<String, Json>> {
    let mut modes = Map::new();
    for guid in install.guids(MODE) {
        let asset = install.stu(guid)?;
        let context = |reason: &str| asset_error(format!("{}: {reason}", guid_name(guid)));
        let root = asset
            .root()
            .filter(|root| class_name(root) == "STUGameMode")
            .ok_or_else(|| context("expected a STUGameMode root"))?;
        let variables = var_value_pairs(&asset, root, &context)?;
        modes.insert(guid_text(guid), Json::Object(variables));
    }
    if modes.is_empty() {
        return Err(asset_error("the install has no game modes"));
    }
    Ok(modes)
}

/// Each game ruleset's (`0C0`, `STUGameRuleset`) mode-level values: its
/// game mode (`m_gamemode`, a `STUGameRulesetGameMode`) names the mode
/// and holds the values its own `m_3CE93B76` table sets over the mode's
/// ([`var_value_pairs`]). Only the rulesets that set any: the Practice
/// Range's `0C0:34` sets `0x254C` (9548, the hero limit) to `254D`, where
/// the mode's schema `0008` has `254E`. The team-level (`m_CF58324E`) and
/// per-hero (`m_bodyVars`) values are not read.
fn ruleset_variables(install: &Install) -> Result<Map<String, Json>> {
    let mut rulesets = Map::new();
    for guid in install.guids(RULESET) {
        let asset = install.stu(guid)?;
        let context = |reason: &str| asset_error(format!("{}: {reason}", guid_name(guid)));
        let game = asset
            .root()
            .filter(|root| class_name(root) == "STUGameRuleset")
            .and_then(|root| asset.resolve(field(root, "m_gamemode")?))
            .filter(|game| class_name(game) == "STUGameRulesetGameMode")
            .ok_or_else(|| context("expected a STUGameRuleset with its game mode"))?;
        let mode = match field(game, "m_gamemode") {
            Some(Value::Guid(mode)) if guid_type(*mode) == MODE => *mode,
            _ => return Err(context("m_gamemode must reference a .0C5 game mode")),
        };
        let variables = var_value_pairs(&asset, game, &context)?;
        if !variables.is_empty() {
            rulesets.insert(
                guid_text(guid),
                json!({"mode": guid_text(mode), "variables": variables}),
            );
        }
    }
    Ok(rulesets)
}

/// `object`'s table `m_3CE93B76` of identifier (`01C`, at most `FFFF`) and
/// value pairs (`STUGameModeVarValuePair`), as entity variables. A value
/// is an identifier (`STU_22BF1D21`), a boolean (`STU_43CA1D94`), an
/// integer (`STU_3ED2A372`) or a float (`STU_73715F2E`).
fn var_value_pairs(
    asset: &Asset,
    object: &Object,
    context: &dyn Fn(&str) -> crate::Error,
) -> Result<Map<String, Json>> {
    let pairs = match field(object, "m_3CE93B76") {
        None | Some(Value::Null) => &[][..],
        Some(Value::Array(pairs)) => pairs.as_slice(),
        Some(_) => return Err(context("invalid m_3CE93B76 array")),
    };
    let mut variables = Map::new();
    for pair in pairs {
        let pair = asset
            .resolve(pair)
            .filter(|pair| class_name(pair) == "STUGameModeVarValuePair")
            .ok_or_else(|| context("invalid m_3CE93B76 pair"))?;
        let key = identifier(field(pair, "m_3E783677"))
            .ok_or_else(|| context("m_3E783677 must reference a .01C identifier"))?
            & INDEX;
        if key == 0 || key > 0xFFFF {
            return Err(context(&format!(
                "mode variable identifier {key:X} is outside the entity variable range"
            )));
        }
        let value = asset
            .resolve(field(pair, "m_value").unwrap_or(&Value::Null))
            .ok_or_else(|| context("unsupported mode variable value type missing"))?;
        let number = || match field(value, "m_value") {
            Some(Value::Int(number)) => Some(*number),
            Some(Value::UInt(number)) => i64::try_from(*number).ok(),
            _ => None,
        };
        let value = match class_name(value) {
            "STU_22BF1D21" => guid_text(
                identifier(field(value, "m_value"))
                    .ok_or_else(|| context("m_value must reference a .01C identifier"))?,
            )
            .into(),
            "STU_43CA1D94" => match number() {
                Some(0) => false.into(),
                Some(1) => true.into(),
                _ => return Err(context("invalid STU_43CA1D94 m_value")),
            },
            "STU_3ED2A372" => match number() {
                Some(number) if i32::try_from(number).is_ok() => number.into(),
                _ => return Err(context("invalid STU_3ED2A372 m_value")),
            },
            "STU_73715F2E" => match field(value, "m_value") {
                Some(Value::Float(number)) if number.is_finite() => {
                    let single = Types::get()
                        .field_named(value.class, "m_value")
                        .is_some_and(|field| field.ty.kind == Kind::F32);
                    float(single, *number)?
                }
                _ => return Err(context("invalid STU_73715F2E m_value")),
            },
            other => {
                return Err(context(&format!(
                    "unsupported mode variable value type {other}"
                )));
            }
        };
        if variables.insert(key.to_string(), value).is_some() {
            return Err(context(&format!(
                "duplicate mode variable identifier {key:X}"
            )));
        }
    }
    Ok(variables)
}

/// A `01C` identifier reference.
fn identifier(value: Option<&Value>) -> Option<u64> {
    match value {
        Some(Value::Guid(guid)) if guid_type(*guid) == IDENTIFIER => Some(*guid),
        _ => None,
    }
}

/// The AI definition (`003`) an entity definition's `m_8B9A461F` names.
fn ai_definition(root: &Object) -> Result<Option<u64>> {
    match field(root, "m_8B9A461F") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Guid(guid)) if guid_type(*guid) == ENTITY && guid & INDEX != 0 => {
            Ok(Some(*guid))
        }
        Some(_) => Err(asset_error(
            "m_8B9A461F must reference a .003 entity definition",
        )),
    }
}

/// The bot entity definitions, by index: the bodies of the heroes the maps'
/// AI spawners place (`ai_spawners_174.json` `heroes`, the six Practice
/// Range bodies `1403`, `143E`, `146E`, `14A9`, `14D2`, `14ED` in 1.74) and
/// the AI definitions their `m_8B9A461F` names (`1404`, `14AF`).
fn bot_entities(assets: &Assets, path: &Path) -> Result<BTreeSet<u64>> {
    let table = read_json(path)?;
    let invalid = |reason: String| asset_error(format!("{}: {reason}", path.display()));
    let heroes = table
        .get("heroes")
        .and_then(Json::as_object)
        .ok_or_else(|| invalid("expected a hero table".into()))?;
    let mut entities = BTreeSet::new();
    for (hero, record) in heroes {
        let body = record
            .get("body")
            .and_then(Json::as_str)
            .and_then(|text| text.strip_prefix("0x"))
            .and_then(|digits| u64::from_str_radix(digits, 16).ok())
            .filter(|&body| guid_type(body) == ENTITY && body & INDEX != 0)
            .ok_or_else(|| invalid(format!("hero {hero} has no .003 body")))?;
        if !entities.insert(body & INDEX) {
            continue;
        }
        let asset = assets.entity(body & INDEX)?;
        let root = asset
            .root()
            .ok_or_else(|| asset_error("an entity definition without a root"))?;
        if let Some(ai) = ai_definition(root)? {
            entities.insert(ai & INDEX);
        }
    }
    if entities.is_empty() {
        return Err(invalid("the AI spawners place no hero".into()));
    }
    Ok(entities)
}

/// The `01B` graphs the maps' AI spawners place (`ai_spawners_174.json`),
/// including mode-gated placements.
fn ai_graph_roots(path: &Path) -> Result<BTreeSet<u64>> {
    let table = read_json(path)?;
    let invalid = |reason: String| asset_error(format!("{}: {reason}", path.display()));
    let maps = table
        .get("maps")
        .and_then(Json::as_object)
        .ok_or_else(|| invalid("expected a map table".into()))?;
    let mut roots = BTreeSet::new();
    for (map, record) in maps {
        let spawners = record
            .get("spawners")
            .and_then(Json::as_array)
            .ok_or_else(|| invalid(format!("map {map} must contain a spawner list")))?;
        for (number, spawner) in spawners.iter().enumerate() {
            let context = format!("map {map} spawner {number}");
            let references = spawner
                .get("graphs")
                .and_then(Json::as_array)
                .ok_or_else(|| invalid(format!("{context} must contain a graph list")))?;
            for reference in references {
                let guid = reference
                    .as_str()
                    .and_then(|text| text.strip_prefix("0x"))
                    .filter(|digits| digits.len() == 16)
                    .and_then(|digits| u64::from_str_radix(digits, 16).ok())
                    .ok_or_else(|| invalid(format!("{context} has an invalid graph GUID")))?;
                let graph = guid & INDEX;
                if guid >> 60 != 0 || guid_type(guid) != GRAPH || graph == 0 || graph > 0xFFFF {
                    return Err(invalid(format!(
                        "{context} must reference a .01B graph with a nonzero 16-bit index"
                    )));
                }
                roots.insert(graph);
            }
        }
    }
    Ok(roots)
}
