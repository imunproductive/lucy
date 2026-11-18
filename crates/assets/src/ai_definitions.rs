//! `data/game_ai_definitions_174.json`: the AI definitions of the hero
//! bodies, what drives a hero when the game plays it (Practice vs AI and
//! custom games' AI). The Python reference server had no AI and read none of
//! this; the research is in `D:/Work/Coding/refs/reports/AiDefinitions.md`.
//!
//! - `heroes`: each hero of `game_heroes_174.json`, by hero GUID: its name,
//!   body, whether the AI may play it (`STUHero.m_906C3711`, DataTool's
//!   `SupportsAi`) and the AI definition its body names
//!   (`STUEntityDefinition.m_04A8896C`, an `003`).
//! - `definitions`: each AI definition those bodies name, by GUID: the
//!   graphs of its own statescript component (`m_B634821A`, with their
//!   overrides), the seven per-mode graph lists of its
//!   `STUAIBehaviorComponent` (`STUGameModeSpecificBehaviorScripts`, by
//!   their hashed field names: which mode takes which list is not known),
//!   `m_72D00D60`, its difficulty profiles (`m_difficultyProfiles`: a
//!   difficulty identifier `01C` and its `0AE` profile) and its AI weapon
//!   controls (`STU_993B1B10.m_CD486A71`, `STUAIWeaponFireControlInfo`), and
//!   whether it declares component 92 (`STU_76AF64F0`, `component_92`),
//!   which the graphs' `STU_D3EB8D87` tests on a body's possessor.
//! - `profiles`: each `0AE` difficulty profile they name (`STU_806CF87E`),
//!   with its parent (`m_469E51E3`) and its values resolved through its
//!   parents: identifier `01C` → float (`m_D680006F`, `STU_FF175FB5`
//!   `m_identifier` → `m_9B7A63EA`). A profile lists every value of its
//!   parent; `m_06F26E44` set marks one inherited unchanged (true of all
//!   3,108 such entries in the install), so the values are taken as listed.
//!   Hero graphs read them through `STU_4049CF77`.
//!
//! An override or schema value is a number (`STUConfigVarInt`,
//! `STUConfigVarFloat`) or a bool (`STUConfigVarBool`); another config
//! class is an error.

use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    path::Path,
};

use orpheus_stu::{Asset, Object, Value};
use serde_json::{Map, Value as Json, json};

use crate::{
    Install, Output, Result, asset_error,
    game_queues::{INDEX, Node, key, typed},
    heroes::parse_guid,
};

pub(crate) const AI_DEFINITIONS: &str = "data/game_ai_definitions_174.json";

const HERO: u16 = 0x075;
const ENTITY: u16 = 0x003;
const PROFILE: u16 = 0x0AE;

/// The seven per-mode lists of `STUAIBehaviorComponent`, in its field order.
const MODE_LISTS: [&str; 7] = [
    "m_9325CAD8",
    "m_4A10B1EF",
    "m_853DD825",
    "m_B6DCC63B",
    "m_3B8A79C3",
    "m_7D218322",
    "m_099695F2",
];

const SOURCE: &str = "Build 1.74.0.0.104319: each hero (075 m_906C3711 = supports AI) and the AI \
definition its body names (003 m_04A8896C); per definition its STUStatescriptComponent graphs, the \
STUAIBehaviorComponent per-mode graph lists and difficulty profiles, its AI weapon controls; each \
0AE difficulty profile (STU_806CF87E) named, values resolved through m_469E51E3 parents.";

pub(crate) fn derive(install: &Install, out: &Path) -> Result<Vec<Output>> {
    let path = out.join("data/game_heroes_174.json");
    let heroes: Json = std::fs::read(&path)
        .map_err(|error| asset_error(format!("{}: {error}", path.display())))
        .and_then(|bytes| {
            serde_json::from_slice(&bytes)
                .map_err(|error| asset_error(format!("{}: {error}", path.display())))
        })?;
    let heroes = heroes
        .as_object()
        .ok_or_else(|| asset_error("game_heroes_174.json: expected a hero table"))?;

    let stored = Stored::new(install);
    let mut hero_table = Map::new();
    let mut definitions = BTreeSet::new();
    for (hero_key, record) in heroes {
        let hero = parse_guid(hero_key)?;
        let body = record
            .get("body")
            .and_then(Json::as_str)
            .map(parse_guid)
            .transpose()?
            .ok_or_else(|| asset_error(format!("game_heroes_174.json: {hero_key} has no body")))?;
        let hero_asset = stored.read(HERO, hero)?;
        let supports_ai = Node::root(&hero_asset)?.int("m_906C3711") > 0;
        let body_asset = stored.read(ENTITY, body)?;
        let definition = Node::root(&body_asset)?.guid("m_04A8896C");
        hero_table.insert(
            key(hero),
            json!({
                "name": record.get("name").cloned().unwrap_or(Json::Null),
                "body": key(body),
                "supports_ai": supports_ai,
                "ai_definition": (definition != 0).then(|| key(definition)),
            }),
        );
        if definition != 0 {
            definitions.insert(definition & INDEX);
        }
    }

    let profiles = load_profiles(install)?;
    let mut named = BTreeSet::new();
    let mut definition_table = Map::new();
    for &index in &definitions {
        let asset = stored.read(ENTITY, index)?;
        let record = definition(&asset, &mut named)
            .map_err(|error| asset_error(format!("{index:012X}.003: {error}")))?;
        definition_table.insert(key(typed(index, ENTITY)), record);
    }
    let mut profile_table = Map::new();
    let mut pending: Vec<u64> = named.into_iter().collect();
    while let Some(index) = pending.pop() {
        if profile_table.contains_key(&key(typed(index, PROFILE))) {
            continue;
        }
        let profile = profiles
            .get(&index)
            .ok_or_else(|| asset_error(format!("no difficulty profile {index:012X}.0AE")))?;
        if let Some(parent) = profile.parent {
            pending.push(parent);
        }
        let values: Map<String, Json> = resolved(&profiles, index)?
            .into_iter()
            .map(|(identifier, value)| (key(identifier), float(value)))
            .collect();
        profile_table.insert(
            key(typed(index, PROFILE)),
            json!({
                "parent": profile.parent.map(|parent| key(typed(parent, PROFILE))),
                "values": values,
            }),
        );
    }
    let profile_table: Map<String, Json> = {
        let mut sorted: Vec<(String, Json)> = profile_table.into_iter().collect();
        sorted.sort_by(|a, b| a.0.cmp(&b.0));
        sorted.into_iter().collect()
    };

    let table = json!({
        "source": SOURCE,
        "heroes": hero_table,
        "definitions": definition_table,
        "profiles": profile_table,
    });
    Ok(vec![Output::json(AI_DEFINITIONS, &table)])
}

/// The install's GUIDs of the types read here, by index: a stored GUID may
/// carry bits besides its type and index.
struct Stored<'a> {
    install: &'a Install,
    guids: HashMap<(u16, u64), u64>,
}

impl<'a> Stored<'a> {
    fn new(install: &'a Install) -> Self {
        let guids = [HERO, ENTITY]
            .into_iter()
            .flat_map(|kind| {
                install
                    .guids(kind)
                    .into_iter()
                    .map(move |guid| ((kind, guid & INDEX), guid))
            })
            .collect();
        Self { install, guids }
    }

    fn read(&self, kind: u16, guid: u64) -> Result<Asset> {
        let index = guid & INDEX;
        let guid = self
            .guids
            .get(&(kind, index))
            .ok_or_else(|| asset_error(format!("no asset {index:012X}.{kind:03X}")))?;
        self.install.stu(*guid)
    }
}

/// One AI definition; the profiles it names are added to `named`.
fn definition(asset: &Asset, named: &mut BTreeSet<u64>) -> Result<Json> {
    let mut record = Map::new();
    let graphs = match component(asset, "STUStatescriptComponent")? {
        Some(component) => component
            .nodes("m_B634821A")
            .map(graph_entry)
            .collect::<Result<Vec<_>>>()?,
        None => Vec::new(),
    };
    record.insert("graphs".into(), graphs.into());
    let behavior = component(asset, "STUAIBehaviorComponent")?
        .ok_or_else(|| asset_error("an AI definition without an STUAIBehaviorComponent"))?;
    record.insert(
        "m_72D00D60".into(),
        match behavior.value("m_72D00D60") {
            Some(&Value::Float(value)) => float(value),
            _ => return Err(asset_error("STUAIBehaviorComponent without m_72D00D60")),
        },
    );
    let mut modes = Map::new();
    for name in MODE_LISTS {
        let Some(list) = behavior.node(name) else {
            continue;
        };
        let graphs = list
            .nodes("m_96053EC6")
            .map(graph_entry)
            .collect::<Result<Vec<_>>>()?;
        let schema = match list.node("m_200EF939") {
            Some(schema) => entries(schema.nodes("m_entries"))?,
            None => Map::new(),
        };
        modes.insert(name.into(), json!({"graphs": graphs, "schema": schema}));
    }
    record.insert("modes".into(), modes.into());
    let mut difficulties = Map::new();
    for profile in behavior.nodes("m_difficultyProfiles") {
        let difficulty = profile.guid("m_13E9E420");
        let asset = profile.guid("m_AFEEA472");
        if difficulty == 0 || asset == 0 {
            return Err(asset_error(
                "an STUAIDifficultyProfile without its identifier or 0AE",
            ));
        }
        named.insert(asset & INDEX);
        difficulties.insert(key(difficulty), key(typed(asset & INDEX, PROFILE)).into());
    }
    record.insert("difficulty_profiles".into(), difficulties.into());
    let weapons = match component(asset, "STU_993B1B10")? {
        Some(control) => control
            .nodes("m_CD486A71")
            .map(|row| {
                json!({
                    "slot": row.int("m_A60150E2"),
                    "button": row.int("m_4F5E7D4C"),
                    "m_ED6D601D": row.int("m_ED6D601D"),
                    "identifier": nonzero(row.guid("m_F2810377")),
                    "hardpoint": nonzero(row.guid("m_C3657821")),
                })
            })
            .collect(),
        None => Vec::new(),
    };
    record.insert("weapons".into(), weapons.into());
    record.insert(
        "component_92".into(),
        component(asset, "STU_76AF64F0")?.is_some().into(),
    );
    Ok(record.into())
}

fn nonzero(guid: u64) -> Json {
    if guid == 0 {
        Json::Null
    } else {
        key(guid).into()
    }
}

/// An `STUStatescriptGraphWithOverrides`: `{"graph", "overrides"}`.
fn graph_entry(entry: Node<'_>) -> Result<Json> {
    let graph = entry.guid("m_graph");
    if graph == 0 {
        return Err(asset_error("a graph entry without its graph"));
    }
    Ok(json!({
        "graph": key(graph),
        "overrides": entries(entry.nodes("m_1EB5A024"))?,
    }))
}

/// `STUStatescriptSchemaEntry`s: identifier → value.
fn entries<'a>(list: impl Iterator<Item = Node<'a>>) -> Result<Map<String, Json>> {
    let mut values = Map::new();
    for entry in list {
        let identifier = entry.guid("m_0D09D2D9");
        let value = entry
            .node("m_value")
            .ok_or_else(|| asset_error("a schema entry without a value"))?;
        let value = match (value.class_name(), value.value("m_value")) {
            (Some("STUConfigVarInt"), Some(&Value::Int(number))) => json!(number),
            (Some("STUConfigVarInt"), Some(&Value::UInt(number))) => json!(number),
            (Some("STUConfigVarFloat"), Some(&Value::Float(number))) => float(number),
            (Some("STUConfigVarBool"), Some(&Value::Int(number))) => json!(number != 0),
            (Some("STUConfigVarBool"), Some(&Value::UInt(number))) => json!(number != 0),
            (class, _) => {
                return Err(asset_error(format!(
                    "a schema entry with an unread value class {class:?}"
                )));
            }
        };
        values.insert(key(identifier), value);
    }
    Ok(values)
}

/// The one component of class `class` of an entity definition.
fn component<'a>(asset: &'a Asset, class: &str) -> Result<Option<Node<'a>>> {
    let root = asset
        .root()
        .filter(|root| root.class_name() == Some("STUEntityDefinition"))
        .ok_or_else(|| asset_error("expected a STUEntityDefinition root"))?;
    let entries = match root.get("m_componentMap") {
        None | Some(Value::Null) => return Ok(None),
        Some(Value::Map(entries)) => entries,
        Some(_) => return Err(asset_error("invalid entity component map")),
    };
    let mut found = entries
        .iter()
        .filter_map(|(_, index)| asset.instances.get((*index)?)?.as_ref())
        .filter(|object: &&Object| object.class_name() == Some(class));
    let first = found.next();
    if found.next().is_some() {
        return Err(asset_error(format!("duplicate {class} components")));
    }
    Ok(first.map(|object| Node::of(asset, object)))
}

struct Profile {
    parent: Option<u64>,
    /// Identifier → value, as listed.
    values: BTreeMap<u64, f64>,
}

/// Every difficulty profile (`0AE`, `STU_806CF87E`) in the install, by index.
fn load_profiles(install: &Install) -> Result<BTreeMap<u64, Profile>> {
    let mut profiles = BTreeMap::new();
    for (index, asset) in crate::game_queues::load(install, PROFILE)? {
        let root = Node::root(&asset)?;
        if root.class_name() != Some("STU_806CF87E") {
            return Err(asset_error(format!(
                "{index:012X}.0AE: expected an STU_806CF87E"
            )));
        }
        let parent = root.guid("m_469E51E3");
        let mut values = BTreeMap::new();
        if let Some(Value::Map(entries)) = root.value("m_D680006F") {
            for &(_, instance) in entries {
                let Some(object) = instance.and_then(|at| asset.instances.get(at)?.as_ref()) else {
                    continue;
                };
                let entry = Node::of(&asset, object);
                let identifier = entry.guid("m_identifier");
                let Some(&Value::Float(value)) = entry.value("m_9B7A63EA") else {
                    return Err(asset_error(format!(
                        "{index:012X}.0AE: a value without its float"
                    )));
                };
                values.insert(identifier, value);
            }
        }
        profiles.insert(
            index,
            Profile {
                parent: (parent != 0).then_some(parent & INDEX),
                values,
            },
        );
    }
    Ok(profiles)
}

/// A profile's values over its parents'.
fn resolved(profiles: &BTreeMap<u64, Profile>, index: u64) -> Result<BTreeMap<u64, f64>> {
    let mut chain = Vec::new();
    let mut at = Some(index);
    while let Some(index) = at {
        if chain.contains(&index) || chain.len() > 16 {
            return Err(asset_error(format!("{index:012X}.0AE: a parent loop")));
        }
        chain.push(index);
        let profile = profiles
            .get(&index)
            .ok_or_else(|| asset_error(format!("no difficulty profile {index:012X}.0AE")))?;
        at = profile.parent;
    }
    let mut values = BTreeMap::new();
    for index in chain.into_iter().rev() {
        values.extend(&profiles[&index].values);
    }
    Ok(values)
}

/// An `f32` as DataTool prints it, read back as a double (as `tunables`).
fn float(value: f64) -> Json {
    let shortest: f64 = (value as f32).to_string().parse().expect("a float");
    json!(shortest)
}
