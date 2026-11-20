//! What the queries read: the graph bundle, the hero and map tables of the
//! derived game data, and the names catalog (`ow174/game/script/graph.py`,
//! `ow174/game/content.py` and `names` in `tools/ssq.py`).

use std::{
    collections::{BTreeMap, HashMap},
    fs,
    io::{self, Read},
    path::Path,
};

use flate2::read::MultiGzDecoder;
use serde::Deserialize;
use serde_json::value::RawValue;

use super::json::{self, Int, J, Obj, invalid};

/// Variable scopes: sync var `m_56341592`, `STUConfigVarDynamic.m_60DB8F99`.
pub const INSTANCE: Int = 0;
pub const ENTITY: Int = 1;
/// A `01B` GUID's type bits (bits 48 to 63).
pub const GRAPH_TYPE: Int = 0x0580;
const GRAPHS_FILE: &str = "data/game_graphs_174.json.gz";
const HEROES_FILE: &str = "data/game_heroes_174.json";
const MAPS_FILE: &str = "data/game_maps_174.json";

/// An asset GUID of the data (`0x` and 16 upper-case hex digits) as a number
/// (`graphs.guid`).
pub fn guid(value: &J) -> Option<Int> {
    let hex = value.as_str()?.strip_prefix("0x")?;
    (hex.len() == 16
        && hex
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'A'..=b'F')))
    .then(|| Int::from_str_radix(hex, 16).expect("hex digits"))
}

/// A plug: an object with a class and links (`graphs.is_plug`).
pub fn is_plug(value: &J) -> bool {
    value
        .as_obj()
        .is_some_and(|obj| obj.contains("$") && obj.contains("links"))
}

/// The value at a field path such as `m_transitionPlug[0]` or
/// `m_45216F79.m_D44EB181` (`graphs.field_at`).
pub fn field_at<'a>(fields: &'a J, path: &str) -> Option<&'a J> {
    let mut value = fields;
    for part in path.split('.') {
        let (name, mut indexes) = part.split_at(part.find('[').unwrap_or(part.len()));
        if name.is_empty() || !name.chars().all(|ch| ch == '_' || ch.is_alphanumeric()) {
            return None;
        }
        value = value.as_obj()?.val(name)?;
        while !indexes.is_empty() {
            let (index, tail) = indexes.strip_prefix('[')?.split_once(']')?;
            if index.is_empty() || !index.bytes().all(|ch| ch.is_ascii_digit()) {
                return None;
            }
            value = value.as_arr()?.get(index.parse::<usize>().ok()?)?;
            indexes = tail;
        }
    }
    (!value.is_null()).then_some(value)
}

/// One node of a graph (`graphs.Node`).
pub struct Node {
    pub pos: Int,
    pub cls: String,
    pub rid: Int,
    pub remote: Int,
    pub state: Option<Int>,
    pub client_only: bool,
    pub server_only: bool,
    pub fields: J,
}

impl Node {
    fn read(mut record: J) -> io::Result<Self> {
        let int = |key: &str| {
            record
                .at(key)
                .as_int()
                .ok_or_else(|| invalid(format!("graph node without {key}")))
        };
        Ok(Self {
            pos: int("pos")?,
            cls: record.at("class").as_str().unwrap_or_default().to_owned(),
            rid: int("rid")?,
            remote: int("remote")?,
            state: record.at("state").as_int(),
            client_only: record.at("client_only").truthy(),
            server_only: record.at("server_only").truthy(),
            fields: take(&mut record, "fields"),
        })
    }

    pub fn is_state(&self) -> bool {
        self.fields.at("m_2BBEEAB8").truthy()
    }

    pub fn is_networked(&self) -> bool {
        !self.client_only && !self.server_only
    }

    /// Every plug of the node by field path, in field order.
    pub fn plugs(&self) -> Vec<(String, &J)> {
        let mut out = Vec::new();
        collect_plugs(&self.fields, "", &mut out);
        out
    }
}

fn collect_plugs<'a>(value: &'a J, path: &str, out: &mut Vec<(String, &'a J)>) {
    if is_plug(value) {
        match out.iter_mut().find(|(name, _)| name == path) {
            Some(entry) => entry.1 = value,
            None => out.push((path.to_owned(), value)),
        }
    } else if let Some(obj) = value.as_obj() {
        for (key, item) in obj.iter() {
            if key != "$" {
                let name = if path.is_empty() {
                    key.to_owned()
                } else {
                    format!("{path}.{key}")
                };
                collect_plugs(item, &name, out);
            }
        }
    } else if let Some(items) = value.as_arr() {
        for (number, item) in items.iter().enumerate() {
            collect_plugs(item, &format!("{path}[{number}]"), out);
        }
    }
}

/// One `m_syncVars` entry: its position, variable (None for padding), scope
/// and `m_AC9480C7`.
pub struct SyncVar {
    pub index: usize,
    pub var: Option<Int>,
    pub scope: Option<Int>,
    pub flag: J,
}

impl SyncVar {
    pub fn has_presence_bit(&self) -> bool {
        self.var.is_some() && self.scope == Some(INSTANCE)
    }
}

/// One `01B` graph (`graphs.Graph`). The node lists hold indexes into
/// `nodes`.
pub struct Graph {
    pub index: Int,
    pub header: J,
    pub fields: J,
    pub nodes: Vec<Node>,
    pub unknown: usize,
    pub states: Vec<Option<usize>>,
    pub entries: Vec<Option<usize>>,
    pub remote_nodes: Vec<Option<usize>>,
    pub sync_vars: Vec<SyncVar>,
    by_pos: HashMap<Int, usize>,
    by_rid: HashMap<Int, usize>,
}

impl Graph {
    fn read(mut record: J) -> io::Result<Self> {
        let index = record
            .at("index")
            .as_int()
            .ok_or_else(|| invalid("graph without an index"))?;
        let nodes = match take(&mut record, "nodes") {
            J::Arr(records) => records
                .into_iter()
                .map(Node::read)
                .collect::<io::Result<Vec<_>>>()?,
            _ => Vec::new(),
        };
        let by_pos: HashMap<Int, usize> = nodes
            .iter()
            .enumerate()
            .map(|(number, node)| (node.pos, number))
            .collect();
        let by_rid = nodes
            .iter()
            .enumerate()
            .map(|(number, node)| (node.rid, number))
            .collect();
        let list = |key: &str| -> Vec<Option<usize>> {
            record
                .at(key)
                .items()
                .iter()
                .map(|position| by_pos.get(&position.as_int()?).copied())
                .collect()
        };
        let states = list("states");
        let entries = list("entries");
        let remote_nodes = list("remote");
        let sync_vars = record
            .at("sync_vars")
            .items()
            .iter()
            .enumerate()
            .map(|(index, entry)| {
                let item = |number: usize| entry.items().get(number);
                SyncVar {
                    index,
                    var: item(0).and_then(J::as_int),
                    scope: item(1).and_then(J::as_int),
                    flag: item(2).cloned().unwrap_or(J::Null),
                }
            })
            .collect();
        let unknown = record.at("unknown_nodes").items().len();
        Ok(Self {
            index,
            header: take(&mut record, "header"),
            fields: take(&mut record, "fields"),
            nodes,
            unknown,
            states,
            entries,
            remote_nodes,
            sync_vars,
            by_pos,
            by_rid,
        })
    }

    /// The node at a position in `m_items` (what links name).
    pub fn node(&self, position: Int) -> Option<&Node> {
        self.by_pos
            .get(&position)
            .map(|&number| &self.nodes[number])
    }

    /// The node with an `m_nodes` index.
    pub fn by_rid(&self, rid: Int) -> Option<&Node> {
        self.by_rid.get(&rid).map(|&number| &self.nodes[number])
    }

    /// The node of an `m_states` index.
    pub fn state(&self, index: i64) -> Option<&Node> {
        let number = usize::try_from(index).ok()?;
        self.states
            .get(number)
            .copied()
            .flatten()
            .map(|node| &self.nodes[node])
    }

    pub fn listed<'a>(&'a self, list: &'a [Option<usize>]) -> impl Iterator<Item = &'a Node> {
        list.iter().flatten().map(|&number| &self.nodes[number])
    }

    /// The states of an owner (H=1) frame: `m_states` minus client-only and
    /// server-only.
    pub fn owner_states(&self) -> usize {
        self.listed(&self.states)
            .filter(|node| node.is_networked())
            .count()
    }

    /// The states of an H=0 frame: `m_remoteSyncNodes` entries that are
    /// states and neither client-only nor server-only.
    pub fn remote_states(&self) -> usize {
        self.listed(&self.remote_nodes)
            .filter(|node| node.is_state() && node.is_networked())
            .count()
    }

    /// The sync vars with a presence bit in a full frame, in order.
    pub fn presence_vars(&self) -> impl Iterator<Item = &SyncVar> {
        self.sync_vars
            .iter()
            .filter(|entry| entry.has_presence_bit())
    }

    pub fn presence_bit(&self, var: Int) -> Option<usize> {
        self.presence_vars()
            .position(|entry| entry.var == Some(var))
    }
}

/// Moves a value out of an object, leaving null behind.
fn take(record: &mut J, key: &str) -> J {
    match record {
        J::Obj(obj) => obj.take(key),
        _ => J::Null,
    }
}

/// The graph bundle (`ow174/game/script/graph.py`).
pub struct Bundle {
    pub graphs: BTreeMap<Int, Graph>,
    /// Hero body (`003` GUID) -> its record, in file order.
    pub bodies: Vec<(Int, J)>,
    pub missing: Vec<Int>,
}

/// The bundle's top level, each graph's JSON left as text to parse on a
/// thread of its own.
#[derive(Deserialize)]
struct RawBundle<'a> {
    #[serde(borrow)]
    graphs: Vec<&'a RawValue>,
    #[serde(borrow, default)]
    bodies: Option<&'a RawValue>,
    #[serde(borrow, default)]
    missing: Option<&'a RawValue>,
}

impl Bundle {
    pub fn load(data: &Path) -> io::Result<Self> {
        let path = data.join(GRAPHS_FILE);
        let mut bytes = Vec::new();
        MultiGzDecoder::new(io::BufReader::new(fs::File::open(&path).map_err(
            |error| io::Error::new(error.kind(), format!("{}: {error}", path.display())),
        )?))
        .read_to_end(&mut bytes)?;
        let raw: RawBundle = serde_json::from_slice(&bytes).map_err(invalid)?;
        let graphs = super::par_map(&raw.graphs, |text| {
            Graph::read(json::parse(text.get().as_bytes())?)
        })
        .into_iter()
        .map(|graph| graph.map(|graph| (graph.index, graph)))
        .collect::<io::Result<_>>()?;
        let parse = |text: Option<&RawValue>| {
            text.map_or(Ok(J::Null), |text| json::parse(text.get().as_bytes()))
        };
        let bodies = match parse(raw.bodies)? {
            J::Obj(obj) => obj
                .into_entries()
                .map(|(key, record)| Ok((hex_key(&key)?, record)))
                .collect::<io::Result<_>>()?,
            _ => Vec::new(),
        };
        let missing = parse(raw.missing)?
            .items()
            .iter()
            .filter_map(J::as_int)
            .collect();
        Ok(Self {
            graphs,
            bodies,
            missing,
        })
    }

    pub fn graph(&self, index: Int) -> Option<&Graph> {
        self.graphs.get(&index)
    }
}

/// `int(key, 16)` of a `0x...` key.
pub fn hex_key(key: &str) -> io::Result<Int> {
    let hex = key
        .strip_prefix("0x")
        .or_else(|| key.strip_prefix("0X"))
        .unwrap_or(key);
    Int::from_str_radix(hex, 16).map_err(|error| invalid(format!("{key}: {error}")))
}

/// A hero (`content.Hero`).
pub struct Hero {
    pub guid: Int,
    pub name: String,
    pub role: String,
    pub body: Int,
}

/// The heroes of `game_heroes_174.json`, in file order.
pub fn heroes(data: &Path) -> io::Result<Vec<Hero>> {
    let table = read_json(&data.join(HEROES_FILE))?;
    let Some(table) = table.as_obj() else {
        return Err(invalid("the hero table is not an object"));
    };
    table
        .iter()
        .map(|(key, entry)| {
            Ok(Hero {
                guid: hex_key(key)?,
                name: entry.at("name").as_str().unwrap_or_default().to_owned(),
                role: entry.at("class").as_str().unwrap_or_default().to_owned(),
                body: hex_key(entry.at("body").as_str().unwrap_or_default())?,
            })
        })
        .collect()
}

/// The maps of `game_maps_174.json` by GUID; none when the file is missing.
pub fn maps(data: &Path) -> io::Result<BTreeMap<Int, J>> {
    let path = data.join(MAPS_FILE);
    if !path.is_file() {
        return Ok(BTreeMap::new());
    }
    match read_json(&path)? {
        J::Obj(obj) => obj
            .into_entries()
            .map(|(key, entry)| Ok((hex_key(&key)?, entry)))
            .collect(),
        _ => Err(invalid("the map table is not an object")),
    }
}

fn read_json(path: &Path) -> io::Result<J> {
    let bytes = fs::read(path)
        .map_err(|error| io::Error::new(error.kind(), format!("{}: {error}", path.display())))?;
    json::parse(&bytes)
}

/// A hero by name (`soldier`, `Torbjörn`, `dva`) or GUID
/// (`content.find_hero`).
pub fn find_hero<'a>(heroes: &'a [Hero], text: &str) -> Option<&'a Hero> {
    let wanted = plain(text);
    if wanted.is_empty() {
        return None;
    }
    let mut by_name: Vec<(String, &Hero)> = Vec::new();
    for hero in heroes {
        let name = plain(&hero.name);
        match by_name.iter_mut().find(|(known, _)| *known == name) {
            Some(entry) => entry.1 = hero,
            None => by_name.push((name, hero)),
        }
    }
    if let Some((_, hero)) = by_name.iter().find(|(name, _)| *name == wanted) {
        return Some(hero);
    }
    let starts: Vec<_> = by_name
        .iter()
        .filter(|(name, _)| name.starts_with(&wanted))
        .collect();
    if let [(_, hero)] = starts.as_slice() {
        return Some(hero);
    }
    let number = python_hex(text)?;
    heroes.iter().find(|hero| hero.guid == number)
}

fn plain(text: &str) -> String {
    text.to_lowercase()
        .replace('ö', "o")
        .replace('ú', "u")
        .chars()
        .filter(|ch| ch.is_ascii_digit() || ch.is_ascii_lowercase())
        .collect()
}

/// Python's `int(text, 16)`: an optional sign and `0x`, digits with single
/// underscores between them, surrounding white space.
fn python_hex(text: &str) -> Option<Int> {
    let text = text.trim();
    let (negative, text) = match text.as_bytes().first() {
        Some(b'-') => (true, &text[1..]),
        Some(b'+') => (false, &text[1..]),
        _ => (false, text),
    };
    let text = text
        .strip_prefix("0x")
        .or_else(|| text.strip_prefix("0X"))
        .map(|rest| rest.strip_prefix('_').unwrap_or(rest))
        .unwrap_or(text);
    if text.is_empty() || text.starts_with('_') || text.ends_with('_') || text.contains("__") {
        return None;
    }
    let digits: String = text.chars().filter(|ch| *ch != '_').collect();
    let value = Int::from_str_radix(&digits, 16).ok()?;
    Some(if negative { -value } else { value })
}

/// A class's names in the catalog.
#[derive(Default)]
pub struct ClassName {
    pub hash: Option<String>,
    pub alias: Option<String>,
    pub client: Option<String>,
    pub from: Option<String>,
}

pub struct Team {
    pub team: String,
    pub controller: Option<String>,
    pub body: Option<String>,
}

/// A game mode's scripts (the 1.74 `0C5` asset).
pub struct Mode {
    pub name: Option<String>,
    pub script: Option<String>,
    pub second_script: Option<String>,
    pub teams: Vec<Team>,
}

impl Mode {
    /// `mode.get("script")` and `mode.get("m_F88BA3B9")`.
    pub fn scripts(&self) -> [(&'static str, Option<&String>); 2] {
        [
            ("script", self.script.as_ref()),
            ("m_F88BA3B9", self.second_script.as_ref()),
        ]
    }
}

pub struct VarName {
    pub name: String,
    pub from: String,
}

/// `crates/fixed/data/statescript_names.json`: class aliases, the STU fields
/// that hold a variable, logical buttons, the hero select and HUD graphs,
/// each game mode's scripts and the variable names the server uses.
pub struct Names {
    pub classes: HashMap<String, ClassName>,
    pub var_fields: HashMap<String, HashMap<String, String>>,
    pub buttons: BTreeMap<Int, String>,
    pub graphs: Vec<(Int, String, String)>,
    pub modes: Vec<(Int, Mode)>,
    /// In file order (a name lookup takes the first match).
    pub vars: Vec<(Int, VarName)>,
    var_index: HashMap<Int, usize>,
}

impl Names {
    pub fn embedded() -> Self {
        Self::parse(orpheus_fixed::STATESCRIPT_NAMES.as_bytes()).expect("the names catalog parses")
    }

    pub fn parse(bytes: &[u8]) -> io::Result<Self> {
        let data = json::parse(bytes)?;
        let text = |value: &J| {
            value
                .as_str()
                .filter(|text| !text.is_empty())
                .map(str::to_owned)
        };
        let section = |key: &str| data.at(key).as_obj().map(Obj::iter).into_iter().flatten();
        let classes = section("classes")
            .map(|(name, entry)| {
                let info = ClassName {
                    hash: text(entry.at("hash")),
                    alias: text(entry.at("alias")),
                    client: text(entry.at("client")),
                    from: text(entry.at("from")),
                };
                (name.to_owned(), info)
            })
            .collect();
        let var_fields = section("var_fields")
            .map(|(name, fields)| {
                let fields = fields
                    .as_obj()
                    .map(Obj::iter)
                    .into_iter()
                    .flatten()
                    .map(|(field, kind)| (field.to_owned(), json::py_str(kind)))
                    .collect();
                (name.to_owned(), fields)
            })
            .collect();
        let buttons = section("buttons")
            .map(|(key, name)| Ok((decimal(key)?, json::py_str(name))))
            .collect::<io::Result<_>>()?;
        let graphs = section("graphs")
            .map(|(key, entry)| {
                Ok((
                    hex_key(key)?,
                    json::py_str(entry.at("role")),
                    json::py_str(entry.at("name")),
                ))
            })
            .collect::<io::Result<_>>()?;
        let modes = section("modes")
            .map(|(key, mode)| {
                let teams = mode
                    .at("teams")
                    .items()
                    .iter()
                    .map(|team| Team {
                        team: json::py_str(team.at("team")),
                        controller: text(team.at("controller")),
                        body: text(team.at("body")),
                    })
                    .collect();
                let mode = Mode {
                    name: text(mode.at("name")),
                    script: text(mode.at("script")),
                    second_script: text(mode.at("m_F88BA3B9")),
                    teams,
                };
                Ok((hex_key(key)?, mode))
            })
            .collect::<io::Result<_>>()?;
        let vars: Vec<(Int, VarName)> = section("vars")
            .map(|(key, entry)| {
                let name = VarName {
                    name: json::py_str(entry.at("name")),
                    from: json::py_str(entry.at("from")),
                };
                Ok((decimal(key)?, name))
            })
            .collect::<io::Result<_>>()?;
        let mut var_index = HashMap::new();
        for (number, (var, _)) in vars.iter().enumerate() {
            var_index.insert(*var, number);
        }
        Ok(Self {
            classes,
            var_fields,
            buttons,
            graphs,
            modes,
            vars,
            var_index,
        })
    }

    pub fn var(&self, var: Int) -> Option<&VarName> {
        self.var_index.get(&var).map(|&number| &self.vars[number].1)
    }

    pub fn class(&self, cls: &str) -> Option<&ClassName> {
        self.classes.get(cls)
    }

    pub fn mode(&self, guid: Int) -> Option<&Mode> {
        self.modes
            .iter()
            .find(|(key, _)| *key == guid)
            .map(|(_, mode)| mode)
    }
}

fn decimal(key: &str) -> io::Result<Int> {
    key.trim()
        .parse()
        .map_err(|error| invalid(format!("{key}: {error}")))
}
