//! State graph metadata for retail 1.74 and its owner/remote wire tables.

use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    fs::File,
    io::{self, BufReader, Read},
    path::Path,
    sync::{
        Arc, OnceLock, RwLock, RwLockReadGuard,
        atomic::{AtomicUsize, Ordering},
    },
};

use flate2::bufread::MultiGzDecoder;
use serde::Deserialize;
use serde_json::{Map, Value, value::RawValue};

use orpheus_core::guid::parse_guid;

use crate::{
    ai_definitions::{AI_DEFINITIONS_FILE, AiDefinitions},
    anim_lengths::{ANIM_LENGTHS_FILE, AnimLengths},
    expr::{ConfigTable, Variable},
    rulesets::{self, Rulesets},
    weapons::{TUNABLES_FILE, Tunables},
};

pub const MAX_GRAPH_BYTES: u64 = 128 * 1024 * 1024;
const GRAPH_TYPE: u64 = 0x0580;
pub const TARGET_TAG_TYPE: u64 = 0x0d60;

#[derive(Debug, Deserialize)]
pub struct Header {
    #[serde(rename = "m_nodesBitCount")]
    pub nodes_bits: u8,
    #[serde(rename = "m_statesBitCount")]
    pub states_bits: u8,
    #[serde(rename = "m_remoteSyncNodesBitCount")]
    pub remote_bits: u8,
    #[serde(rename = "m_syncVarsBitCount")]
    pub sync_vars_bits: u8,
}

#[derive(Debug, Deserialize)]
pub struct Node {
    pub pos: u16,
    pub class: String,
    pub uid: u32,
    pub rid: u16,
    pub remote: i32,
    pub state: Option<u16>,
    #[serde(deserialize_with = "node_flag")]
    pub client_only: bool,
    #[serde(deserialize_with = "node_flag")]
    pub server_only: bool,
    pub fields: Value,
    /// What the VM and the projection read of `fields` on every pass,
    /// worked out once by [`Graph::compile`].
    #[serde(skip)]
    info: NodeInfo,
}

/// A node's flags and ids taken from its configuration once, when its
/// graph loads.
#[derive(Debug, Default)]
struct NodeInfo {
    state: bool,
    action: bool,
    presenter: bool,
    remote_synced: bool,
    message: Option<u64>,
    /// Its class's entry in a codec table, by the table's id
    /// ([`crate::codecs::CodecTable::node_codec`]).
    codec: OnceLock<(u64, Option<usize>)>,
    /// Its configs compiled ([`Node::configs`]).
    configs: OnceLock<ConfigTable>,
}

impl NodeInfo {
    fn of(node: &Node) -> Self {
        Self {
            state: node.fields.get("m_2BBEEAB8").is_some_and(truthy),
            action: node.fields.get("m_ADEB6E05").is_some_and(truthy),
            presenter: node.class == "STUStatescriptStateUXPresenter"
                || node.fields.get("m_45216F79").is_some(),
            remote_synced: false,
            message: node
                .field("m_gameMessage.m_gameMessage")
                .and_then(asset_guid),
            codec: OnceLock::new(),
            configs: OnceLock::new(),
        }
    }
}

impl Node {
    pub fn is_networked(&self) -> bool {
        !self.client_only && !self.server_only
    }

    pub fn is_state(&self) -> bool {
        self.info.state
    }

    pub fn is_action(&self) -> bool {
        self.info.action
    }

    /// A UX presenter state, which an observer's frames leave out.
    pub fn is_presenter(&self) -> bool {
        self.info.presenter
    }

    /// Whether the node is in its graph's remote-synced list
    /// ([`Graph::remote_positions`]).
    pub fn is_remote_synced(&self) -> bool {
        self.info.remote_synced
    }

    /// The game message of its `m_gameMessage` (a
    /// `STUConfigVarGameMessage`), if it names one.
    pub fn message(&self) -> Option<u64> {
        self.info.message
    }

    pub(crate) fn codec_cell(&self) -> &OnceLock<(u64, Option<usize>)> {
        &self.info.codec
    }

    /// Its fields' configs compiled for the evaluator, the first time the
    /// VM evaluates one of them (most nodes of a loaded graph never run).
    /// The fields must not change after that.
    pub(crate) fn configs(&self) -> &ConfigTable {
        self.info
            .configs
            .get_or_init(|| ConfigTable::of(&self.fields))
    }

    pub fn field(&self, path: &str) -> Option<&Value> {
        field_at(&self.fields, path)
    }

    /// The plug's targets in data order, including unresolved targets.
    pub fn links(&self, path: &str) -> impl Iterator<Item = (Option<u16>, &str)> {
        self.field(path)
            .filter(|field| field.get("$").is_some())
            .and_then(|field| field.get("links")?.as_array())
            .into_iter()
            .flatten()
            .filter_map(|link| {
                let pair = link.as_array()?;
                let position = match pair.first()? {
                    Value::Null => None,
                    value => Some(u16::try_from(value.as_u64()?).ok()?),
                };
                Some((position, pair.get(1)?.as_str()?))
            })
    }

    pub fn plugs(&self) -> BTreeMap<String, &Value> {
        let mut found = BTreeMap::new();
        collect_plugs(&self.fields, "", &mut found);
        found
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
pub struct SyncVar(pub Option<u32>, pub u8, pub u8);

impl SyncVar {
    pub fn has_presence_bit(self) -> bool {
        self.0.is_some() && self.1 == 0
    }
}

#[derive(Debug, Default)]
struct WireSlots {
    by_list_index: Vec<Option<usize>>,
    positions: Vec<u16>,
}

impl WireSlots {
    fn build(order: &[Option<u16>], graph: &Graph, require_state: bool) -> Self {
        let mut positions = Vec::new();
        let mut first_slot = vec![None; graph.by_pos.len()];
        let by_list_index = order
            .iter()
            .map(|&position| {
                let node = graph.node(position?)?;
                if !node.is_networked() || (require_state && !node.is_state()) {
                    return None;
                }
                let slot = *first_slot[usize::from(node.pos)].get_or_insert(positions.len());
                positions.push(node.pos);
                Some(slot)
            })
            .collect();
        Self {
            by_list_index,
            positions,
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct Graph {
    pub index: u32,
    pub header: Header,
    pub fields: Value,
    pub items: u16,
    pub containers: BTreeMap<u16, String>,
    pub nodes: Vec<Node>,
    #[serde(rename = "unknown_nodes")]
    pub unknown_rids: Vec<u16>,
    pub states: Vec<Option<u16>>,
    pub entries: Vec<Option<u16>>,
    #[serde(rename = "remote")]
    pub remote_nodes: Vec<Option<u16>>,
    pub sync_vars: Vec<SyncVar>,
    #[serde(skip)]
    by_pos: Vec<Option<usize>>,
    #[serde(skip)]
    by_rid: Vec<Option<usize>>,
    #[serde(skip)]
    owner: WireSlots,
    #[serde(skip)]
    remote: WireSlots,
    #[serde(skip)]
    presence: Vec<usize>,
    #[serde(skip)]
    kill_feed_pulser: Option<u16>,
    #[serde(skip)]
    server_only_variables: BTreeSet<u32>,
    /// The nodes the server runs (not client-only) by the game message
    /// they name, each list in node order.
    #[serde(skip)]
    by_message: BTreeMap<u64, Vec<usize>>,
    /// The receiving states the server runs whose `m_gameMessage` is an
    /// expression, not a message (`0F4A` st53: `07A1` or `07A2` by the
    /// attacking team), in node order.
    #[serde(skip)]
    computed_message: Vec<usize>,
    /// Each weapon volley state's raw index and its ammunition variable
    /// (`m_24E351A4`), in node order.
    #[serde(skip)]
    volley_ammo: Vec<(u16, Variable)>,
}

impl Graph {
    fn compile(&mut self) -> io::Result<()> {
        for node in &mut self.nodes {
            node.info = NodeInfo::of(node);
        }
        self.by_pos = vec![None; usize::from(self.items)];
        if let Some(max) = self.nodes.iter().map(|node| node.rid).max() {
            self.by_rid = vec![None; usize::from(max) + 1];
        }
        for (index, node) in self.nodes.iter().enumerate() {
            let position = self
                .by_pos
                .get_mut(usize::from(node.pos))
                .ok_or_else(|| invalid("graph node outside the item table"))?;
            *position = Some(index);
            self.by_rid[usize::from(node.rid)] = Some(index);
        }
        self.owner = WireSlots::build(&self.states, self, false);
        self.remote = WireSlots::build(&self.remote_nodes, self, true);
        for index in 0..self.remote.positions.len() {
            let node = self.by_pos[usize::from(self.remote.positions[index])]
                .expect("remote slots reference nodes");
            self.nodes[node].info.remote_synced = true;
        }
        for (index, node) in self.nodes.iter().enumerate() {
            if node.client_only {
                continue;
            }
            if let Some(message) = node.message() {
                self.by_message.entry(message).or_default().push(index);
            } else if node.class == "STUStatescriptStateGameMessageEntry"
                && node
                    .field("m_gameMessage")
                    .is_some_and(|value| !value.is_null())
            {
                self.computed_message.push(index);
            }
        }
        self.presence = self
            .sync_vars
            .iter()
            .enumerate()
            .filter_map(|(index, var)| var.has_presence_bit().then_some(index))
            .collect();
        self.kill_feed_pulser = self.find_kill_feed_pulser();
        self.server_only_variables = self
            .fields
            .get("m_9CEC6985")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(asset_guid)
            .map(|guid| u32::from(guid as u16))
            .collect();
        self.volley_ammo = self
            .nodes
            .iter()
            .filter(|node| node.class == "STUStatescriptStateWeaponVolley")
            .filter_map(|node| {
                let raw = node.state?;
                let Some(Ok(Some(variable))) = node.field("m_24E351A4").map(crate::expr::lvalue)
                else {
                    return None;
                };
                Some((raw, variable))
            })
            .collect();
        Ok(())
    }

    /// The variables only the server keeps (`m_9CEC6985`): the frames leave
    /// them out.
    pub fn server_only_variables(&self) -> &BTreeSet<u32> {
        &self.server_only_variables
    }

    /// Each weapon volley state's raw index and the variable its
    /// ammunition goes to (`m_24E351A4`), in node order.
    pub fn volley_ammo(&self) -> &[(u16, Variable)] {
        &self.volley_ammo
    }

    /// The state of the pulser whose client-only plug posts a kill feed line
    /// to the mode UI (`KILL_FEED_PULSERS` in `ow174/game/script/driver.py`,
    /// found here instead of listed): a `ClientOnlyPulser` (`STU_B9898052`)
    /// whose plug `m_B6A27F9A` leads, through any plugs, to a client-only
    /// node that posts `0765.025`. `01CF` st108 and `0C18` st8 in 104319.
    fn find_kill_feed_pulser(&self) -> Option<u16> {
        const PULSER: &str = "STU_B9898052";
        const KILL_FEED: u64 = 0x0240_0000_0000_0765;
        self.nodes
            .iter()
            .filter(|node| node.class == PULSER)
            .find(|pulser| {
                let mut pending: Vec<u16> = pulser
                    .links("m_B6A27F9A")
                    .filter_map(|(position, _)| position)
                    .collect();
                let mut seen = BTreeSet::new();
                while let Some(position) = pending.pop() {
                    let Some(node) = self.node(position).filter(|_| seen.insert(position)) else {
                        continue;
                    };
                    if node.client_only && node.message() == Some(KILL_FEED) {
                        return true;
                    }
                    for plug in node.plugs().into_keys() {
                        pending.extend(node.links(&plug).filter_map(|(position, _)| position));
                    }
                }
                false
            })
            .and_then(|pulser| pulser.state)
    }

    /// Whether a node the server runs (not client-only) sends game message
    /// `message` (a `STUConfigVarGameMessage` in `m_gameMessage`).
    pub fn sends_message(&self, message: u64) -> bool {
        self.by_message.contains_key(&message)
    }

    /// The nodes the server runs (not client-only) that name game message
    /// `message` in their `m_gameMessage`, in node order.
    pub fn message_nodes(&self, message: u64) -> impl Iterator<Item = &Node> {
        self.by_message
            .get(&message)
            .into_iter()
            .flatten()
            .map(|&index| &self.nodes[index])
    }

    /// The receiving states the server runs whose message is an
    /// expression, evaluated as a message comes (`m_gameMessage`), in node
    /// order.
    pub fn computed_message_nodes(&self) -> impl Iterator<Item = &Node> {
        self.computed_message
            .iter()
            .map(|&index| &self.nodes[index])
    }

    pub fn node(&self, pos: u16) -> Option<&Node> {
        self.nodes.get((*self.by_pos.get(usize::from(pos))?)?)
    }

    pub fn by_rid(&self, rid: u16) -> Option<&Node> {
        self.nodes.get((*self.by_rid.get(usize::from(rid))?)?)
    }

    pub fn state(&self, index: usize) -> Option<&Node> {
        self.node((*self.states.get(index)?)?)
    }

    pub fn remote_node(&self, index: usize) -> Option<&Node> {
        self.node((*self.remote_nodes.get(index)?)?)
    }

    pub fn node_ref(&self, value: &Value) -> Option<&Node> {
        self.node(u16::try_from(value.get("node")?.as_u64()?).ok()?)
    }

    pub fn owner_positions(&self) -> &[u16] {
        &self.owner.positions
    }

    pub fn remote_positions(&self) -> &[u16] {
        &self.remote.positions
    }

    pub fn owner_slot(&self, index: usize) -> Option<usize> {
        *self.owner.by_list_index.get(index)?
    }

    pub fn remote_slot(&self, index: usize) -> Option<usize> {
        *self.remote.by_list_index.get(index)?
    }

    pub fn presence_vars(&self) -> impl Iterator<Item = (usize, SyncVar)> + '_ {
        self.presence
            .iter()
            .map(|&index| (index, self.sync_vars[index]))
    }

    pub fn presence_bit(&self, var: u32) -> Option<usize> {
        self.presence_vars()
            .position(|(_, entry)| entry.0 == Some(var))
    }

    /// The state of the graph's kill feed pulser, if it has one.
    pub fn kill_feed_pulser(&self) -> Option<u16> {
        self.kill_feed_pulser
    }

    pub fn graph_refs(&self) -> BTreeSet<u64> {
        let mut found = BTreeSet::new();
        collect_graphs(&self.fields, &mut found);
        for node in &self.nodes {
            collect_graphs(&node.fields, &mut found);
        }
        found
    }
}

#[derive(Debug, Deserialize)]
pub struct GraphBinding {
    pub graph: Option<u32>,
    #[serde(rename = "m_1EB5A024")]
    pub sync: Option<Vec<Value>>,
    #[serde(rename = "m_75408640")]
    pub flags: i64,
}

#[derive(Debug, Deserialize)]
pub struct Body {
    pub hero: Option<String>,
    pub name: String,
    pub graphs: Vec<GraphBinding>,
    pub manager: Option<u32>,
    pub weapons: Vec<Option<u32>>,
    #[serde(default)]
    pub ai_weapon_buttons: Map<String, Value>,
    /// An omitted list means the source definition was unavailable.
    #[serde(default, deserialize_with = "body_target_tags")]
    pub target_tags: Option<BTreeSet<u64>>,
}

/// A single match's own ruleset ([`GraphStore::add_match_ruleset`]): its
/// mode, its values by ruleset key, the values only some of its bodies
/// get, and the mode entity's variables (the mode's with those values
/// over them).
#[derive(Debug)]
struct MatchRuleset {
    mode: u64,
    values: BTreeMap<u32, ModeValue>,
    bodies: Vec<BodyValues>,
    variables: Arc<BTreeMap<u32, ModeValue>>,
}

/// Ruleset values a custom match gives one team's bodies, one hero's, or
/// one hero's of one team (a Workshop script's `heroes` blocks: `Team 1`,
/// `General` > `Doomfist`, `Team 2` > `Doomfist`).
#[derive(Clone, Debug, PartialEq)]
pub struct BodyValues {
    /// The room's team (0 is Team 1); none: every team.
    pub team: Option<u8>,
    /// None: every hero.
    pub hero: Option<u64>,
    pub values: BTreeMap<u32, ModeValue>,
}

impl BodyValues {
    fn covers(&self, hero: u64, team: Option<u8>) -> bool {
        self.hero.is_none_or(|own| own == hero) && self.team.is_none_or(|own| Some(own) == team)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ModeValue {
    Bool(bool),
    Integer(i64),
    Float(f64),
    Asset(u64),
}

/// A game ruleset's (`0C0`) mode-level values as the bundle keeps them:
/// the ruleset's mode and the values it sets.
#[derive(Deserialize)]
struct RulesetVariables {
    mode: Value,
    variables: BTreeMap<u32, Value>,
}

/// A bundle as read: each graph's JSON is kept as text, to be parsed on
/// its own ([`parse_graphs`]).
#[derive(Deserialize)]
struct Bundle<'a> {
    version: u32,
    #[serde(borrow)]
    graphs: Vec<&'a RawValue>,
    bodies: BTreeMap<String, Body>,
    #[serde(default)]
    entities: BTreeMap<String, Body>,
    #[serde(default)]
    mode_variables: BTreeMap<String, BTreeMap<u32, Value>>,
    #[serde(default)]
    ruleset_variables: BTreeMap<String, RulesetVariables>,
    missing: Vec<u32>,
    #[serde(default)]
    auras: BTreeMap<String, Value>,
}

impl<'a> Bundle<'a> {
    fn read(data: &'a [u8]) -> io::Result<Self> {
        let bundle: Self = serde_json::from_slice(data).map_err(invalid)?;
        if bundle.version != 1 {
            return Err(invalid("unsupported state graph bundle version"));
        }
        Ok(bundle)
    }
}

/// The graphs of `raw`, parsed and compiled on the PC's threads: a bundle's
/// JSON is tens of megabytes, seconds on one thread in a debug build.
fn parse_graphs(raw: &[&RawValue]) -> io::Result<Vec<Graph>> {
    let next = AtomicUsize::new(0);
    let work = || -> io::Result<Vec<(usize, Graph)>> {
        let mut parsed = Vec::new();
        loop {
            let index = next.fetch_add(1, Ordering::Relaxed);
            let Some(text) = raw.get(index) else {
                return Ok(parsed);
            };
            let mut graph: Graph = serde_json::from_str(text.get()).map_err(invalid)?;
            graph.compile()?;
            parsed.push((index, graph));
        }
    };
    let threads = std::thread::available_parallelism().map_or(1, usize::from);
    let mut parsed: Vec<(usize, Graph)> = std::thread::scope(|scope| {
        let helpers: Vec<_> = (1..threads.min(raw.len()))
            .map(|_| scope.spawn(work))
            .collect();
        let mut parsed = work()?;
        for helper in helpers {
            parsed.extend(
                helper
                    .join()
                    .unwrap_or_else(|panic| std::panic::resume_unwind(panic))?,
            );
        }
        io::Result::Ok(parsed)
    })?;
    parsed.sort_unstable_by_key(|(index, _)| *index);
    Ok(parsed.into_iter().map(|(_, graph)| graph).collect())
}

/// The files the server loads beside a bundle ([`GraphStore::load_runtime`]).
struct RuntimeFiles {
    rulesets: Arc<Rulesets>,
    tunables: Arc<Tunables>,
    anim_lengths: Arc<AnimLengths>,
    ai_definitions: Arc<AiDefinitions>,
}

impl RuntimeFiles {
    fn load(data: &Path) -> io::Result<Self> {
        Ok(Self {
            rulesets: Arc::new(Rulesets::load(data.join(rulesets::FILE))?),
            tunables: Arc::new(Tunables::load(data.join(TUNABLES_FILE))?),
            anim_lengths: Arc::new(AnimLengths::load(data.join(ANIM_LENGTHS_FILE))?),
            ai_definitions: Arc::new(AiDefinitions::load(data.join(AI_DEFINITIONS_FILE))?),
        })
    }
}

/// A gzipped bundle's JSON.
fn read_gzip(input: impl io::BufRead) -> io::Result<Vec<u8>> {
    let mut data = Vec::new();
    MultiGzDecoder::new(input)
        .take(MAX_GRAPH_BYTES + 1)
        .read_to_end(&mut data)?;
    if data.len() as u64 > MAX_GRAPH_BYTES {
        return Err(invalid("state graph bundle exceeds the size limit"));
    }
    Ok(data)
}

fn read_gzip_file(path: &Path) -> io::Result<Vec<u8>> {
    read_gzip(BufReader::new(File::open(path)?))
}

/// An aura (`053`, `STU_76F2C784`): a status a body carries, its effect
/// authored as a graph that runs on the body (Biotic Grenade's `0267`
/// runs `0C71`, which plays `0C72` on an enemy and `0B99` on an ally).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Aura {
    /// The graph an aura runs on the body it is on: `m_5A4051BC` (41 of the
    /// 65 auras), else `m_14AA219A` (the other 24; Nano Boost's `0269` runs
    /// `0B9F`, with its damage modifiers). Assumption: the two run alike;
    /// no client code reads either (the aura is server-only data).
    pub graph: Option<u32>,
    /// `m_7DAE8F10`: how long the aura's own instance stays on the body,
    /// in seconds; 0 until its graph exits (`0267` 0.1, the Biotic Field's
    /// `025F` 0). Assumption: read from these two uses.
    pub seconds: f64,
}

#[derive(Debug)]
pub struct GraphStore {
    graphs: BTreeMap<u32, Arc<Graph>>,
    bodies: BTreeMap<u64, Body>,
    entities: BTreeMap<u64, Body>,
    mode_variables: BTreeMap<u64, Arc<BTreeMap<u32, ModeValue>>>,
    /// Each game ruleset's mode and the mode entity's variables in its
    /// games: the mode's, with the ruleset's own values over them.
    ruleset_variables: BTreeMap<u64, (u64, Arc<BTreeMap<u32, ModeValue>>)>,
    /// The rulesets of single matches (a custom game's settings), by the
    /// ruleset id the match plays ([`GraphStore::add_match_ruleset`]).
    match_rulesets: RwLock<HashMap<u64, MatchRuleset>>,
    /// The heroes' and modes' ruleset defaults, which the server loads
    /// beside the bundle; none until [`GraphStore::with_rulesets`].
    rulesets: Arc<Rulesets>,
    /// The install's tunable values (`STU_082FA44E`), loaded beside the
    /// bundle; none until [`GraphStore::with_tunables`].
    tunables: Arc<Tunables>,
    /// Each body's animation lengths (`game_anim_lengths_174.json`),
    /// loaded beside the bundle; none until
    /// [`GraphStore::with_anim_lengths`].
    anim_lengths: Arc<AnimLengths>,
    /// The hero bodies' AI definitions and difficulty profiles
    /// (`game_ai_definitions_174.json`), loaded beside the bundle; none
    /// until [`GraphStore::with_ai_definitions`].
    ai_definitions: Arc<AiDefinitions>,
    missing: Vec<u32>,
    auras: BTreeMap<u64, Aura>,
    /// The aura graphs and the graphs they play (`PlayScript`), whose
    /// PlayScript actions the server runs.
    aura_graphs: BTreeSet<u32>,
    /// The Workshop's action graphs: the 211 that hold its server-only
    /// done node `STU_1E93FE1F` (`refs/reports/Workshop.md` §2), whose
    /// PlayScript actions the server runs (Chase Variable `1CCC` plays
    /// `1CCF` on each target player).
    workshop_actions: BTreeSet<u32>,
    /// The Workshop's values (`054/0162`) as the client evaluates them:
    /// `1B4A`'s state `STU_25CBEFB0`, `m_108E51DD`, each value's `01C`
    /// identifier with its config ([`GraphStore::workshop_value`]).
    workshop_values: BTreeMap<u64, Value>,
}

impl GraphStore {
    pub fn load(path: impl AsRef<Path>) -> io::Result<Self> {
        Self::from_json(&read_gzip_file(path.as_ref())?)
    }

    pub fn from_gzip(input: impl io::BufRead) -> io::Result<Self> {
        Self::from_json(&read_gzip(input)?)
    }

    pub fn from_json(data: &[u8]) -> io::Result<Self> {
        let bundle = Bundle::read(data)?;
        let graphs = parse_graphs(&bundle.graphs)?
            .into_iter()
            .map(|graph| (graph.index, Arc::new(graph)))
            .collect();
        Self::assemble(bundle, graphs)
    }

    /// The store of `bundle`, its graphs parsed.
    fn assemble(bundle: Bundle<'_>, graphs: BTreeMap<u32, Arc<Graph>>) -> io::Result<Self> {
        let auras: BTreeMap<u64, Aura> = keyed_by_guid(bundle.auras)?
            .into_iter()
            .map(|(guid, fields)| {
                let graph = ["m_5A4051BC", "m_14AA219A"]
                    .into_iter()
                    .find_map(|name| fields.get(name).and_then(asset_guid))
                    .filter(|&graph| graph >> 48 == GRAPH_TYPE)
                    .and_then(|graph| u32::try_from(graph & 0xFFFF_FFFF_FFFF).ok());
                let seconds = fields
                    .get("m_7DAE8F10")
                    .and_then(Value::as_f64)
                    .unwrap_or(0.0);
                (guid, Aura { graph, seconds })
            })
            .collect();
        let aura_graphs = played_closure(&graphs, auras.values().filter_map(|aura| aura.graph));
        let mode_variables: BTreeMap<u64, Arc<BTreeMap<u32, ModeValue>>> =
            keyed_by_guid(bundle.mode_variables)?
                .into_iter()
                .map(|(guid, values)| Ok((guid, Arc::new(mode_values(values)?))))
                .collect::<io::Result<_>>()?;
        let ruleset_variables = keyed_by_guid(bundle.ruleset_variables)?
            .into_iter()
            .map(|(guid, ruleset)| {
                let mode = asset_guid(&ruleset.mode)
                    .ok_or_else(|| invalid("a ruleset's mode must be a GUID"))?;
                let mut variables = mode_variables
                    .get(&mode)
                    .map(|variables| BTreeMap::clone(variables))
                    .unwrap_or_default();
                variables.extend(mode_values(ruleset.variables)?);
                Ok((guid, (mode, Arc::new(variables))))
            })
            .collect::<io::Result<_>>()?;
        let workshop_values = graphs
            .get(&WORKSHOP_VALUES_GRAPH)
            .map(|graph| workshop_values(graph))
            .unwrap_or_default();
        let workshop_actions = graphs
            .iter()
            .filter(|(_, graph)| graph.nodes.iter().any(|node| node.class == "STU_1E93FE1F"))
            .map(|(&index, _)| index)
            .collect();
        Ok(Self {
            graphs,
            bodies: keyed_by_guid(bundle.bodies)?,
            entities: keyed_by_guid(bundle.entities)?,
            mode_variables,
            ruleset_variables,
            match_rulesets: RwLock::default(),
            rulesets: Arc::default(),
            tunables: Arc::default(),
            anim_lengths: Arc::default(),
            ai_definitions: Arc::default(),
            missing: bundle.missing,
            auras,
            aura_graphs,
            workshop_actions,
            workshop_values,
        })
    }

    /// The aura `guid` (a `053` GUID).
    pub fn aura(&self, guid: u64) -> Option<&Aura> {
        self.auras.get(&guid)
    }

    /// Whether `graph` runs as an aura or is played by one.
    pub fn is_aura_graph(&self, graph: u32) -> bool {
        self.aura_graphs.contains(&graph)
    }

    /// Whether `graph` is one of the Workshop's actions.
    pub fn is_workshop_action(&self, graph: u32) -> bool {
        self.workshop_actions.contains(&graph)
    }

    /// The config the client evaluates for Workshop value `value` (its
    /// `01C` GUID), with the value's parameters in `STU_710FE2E1`,
    /// `STU_F619A2A1`… (`1B4A`).
    pub fn workshop_value(&self, value: u64) -> Option<&Value> {
        self.workshop_values.get(&value)
    }

    /// A bundle of the runtime `data` folder with the folder's ruleset
    /// defaults (`game_rulesets_174.json`), tunable values
    /// (`game_tunables_174.json`), animation lengths
    /// (`game_anim_lengths_174.json`) and AI definitions
    /// (`game_ai_definitions_174.json`), as the server runs it.
    pub fn load_runtime(data: impl AsRef<Path>, bundle: &str) -> io::Result<Self> {
        let data = data.as_ref();
        Ok(Self::load(data.join(bundle))?.with_runtime_files(&RuntimeFiles::load(data)?))
    }

    /// The hero bundle `heroes` and the bot bundle `bots` of the runtime
    /// `data` folder, each as [`GraphStore::load_runtime`] loads it; no bot
    /// store when the folder has no bot bundle. The bot bundle is the hero
    /// bundle's graphs, all but a few the same, with the bots' own: a graph
    /// whose JSON is the same in both is parsed once, and the stores share
    /// it.
    pub fn load_runtime_pair(
        data: impl AsRef<Path>,
        heroes: &str,
        bots: &str,
    ) -> io::Result<(Self, Option<Self>)> {
        let data = data.as_ref();
        let files = RuntimeFiles::load(data)?;
        let hero_json = read_gzip_file(&data.join(heroes))?;
        let bot_json = match read_gzip_file(&data.join(bots)) {
            Ok(json) => Some(json),
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(error) => return Err(error),
        };
        let hero_bundle = Bundle::read(&hero_json)?;
        let bot_bundle = bot_json.as_deref().map(Bundle::read).transpose()?;
        // Every hero graph, then each bot graph whose text no hero graph has.
        let hero_texts: HashMap<&str, usize> = hero_bundle
            .graphs
            .iter()
            .enumerate()
            .map(|(position, &raw)| (raw.get(), position))
            .collect();
        let mut raw = hero_bundle.graphs.clone();
        let bot_sources: Vec<usize> = bot_bundle
            .iter()
            .flat_map(|bundle| bundle.graphs.iter().copied())
            .map(|text| {
                hero_texts.get(text.get()).copied().unwrap_or_else(|| {
                    raw.push(text);
                    raw.len() - 1
                })
            })
            .collect();
        let parsed: Vec<Arc<Graph>> = parse_graphs(&raw)?.into_iter().map(Arc::new).collect();
        let by_index = |positions: &mut dyn Iterator<Item = usize>| -> BTreeMap<u32, Arc<Graph>> {
            positions
                .map(|position| {
                    let graph = &parsed[position];
                    (graph.index, Arc::clone(graph))
                })
                .collect()
        };
        let hero_graphs = by_index(&mut (0..hero_bundle.graphs.len()));
        let heroes = Self::assemble(hero_bundle, hero_graphs)?.with_runtime_files(&files);
        let bots = match bot_bundle {
            Some(bundle) => {
                let graphs = by_index(&mut bot_sources.into_iter());
                Some(Self::assemble(bundle, graphs)?.with_runtime_files(&files))
            }
            None => None,
        };
        Ok((heroes, bots))
    }

    fn with_runtime_files(self, files: &RuntimeFiles) -> Self {
        self.with_rulesets(Arc::clone(&files.rulesets))
            .with_tunables(Arc::clone(&files.tunables))
            .with_anim_lengths(Arc::clone(&files.anim_lengths))
            .with_ai_definitions(Arc::clone(&files.ai_definitions))
    }

    /// The AI definitions an AI-possessed body's graphs read
    /// ([`crate::ai_definitions`]).
    pub fn with_ai_definitions(mut self, ai_definitions: Arc<AiDefinitions>) -> Self {
        self.ai_definitions = ai_definitions;
        self
    }

    pub fn ai_definitions(&self) -> &AiDefinitions {
        &self.ai_definitions
    }

    /// The bodies' animation lengths, which `Anim` states without an
    /// authored length run for.
    pub fn with_anim_lengths(mut self, anim_lengths: Arc<AnimLengths>) -> Self {
        self.anim_lengths = anim_lengths;
        self
    }

    pub fn anim_lengths(&self) -> &AnimLengths {
        &self.anim_lengths
    }

    /// The tunable values the graphs read (`STU_082FA44E`).
    pub fn with_tunables(mut self, tunables: Arc<Tunables>) -> Self {
        self.tunables = tunables;
        self
    }

    pub fn tunables(&self) -> &Tunables {
        &self.tunables
    }

    /// The ruleset defaults the bodies' graphs read
    /// (`GetGameRulesetValue`).
    pub fn with_rulesets(mut self, rulesets: Arc<Rulesets>) -> Self {
        self.rulesets = rulesets;
        self
    }

    pub fn rulesets(&self) -> &Rulesets {
        &self.rulesets
    }

    pub fn graph(&self, index: u32) -> Option<&Graph> {
        self.graphs.get(&index).map(Arc::as_ref)
    }

    pub fn graph_indexes(&self) -> impl Iterator<Item = u32> + '_ {
        self.graphs.keys().copied()
    }

    pub fn bodies(&self) -> &BTreeMap<u64, Body> {
        &self.bodies
    }

    /// The body definition of hero `hero`.
    pub fn hero_body(&self, hero: u64) -> Option<u64> {
        self.bodies.iter().find_map(|(&guid, body)| {
            (body.hero.as_deref().and_then(parse_guid) == Some(hero)).then_some(guid)
        })
    }

    /// The `.003` definitions the hero bodies' weapon scripts build as
    /// cosmetics: the gun models. The client builds a cosmetic only when its
    /// definition is resident, which the login preload list (20505) decides.
    /// Found live by AipNooBest (`docs/notes/references.md`).
    pub fn weapon_cosmetics(&self) -> BTreeSet<u64> {
        self.bodies
            .values()
            .flat_map(|body| self.body_weapon_cosmetics(body))
            .collect()
    }

    pub fn body_weapon_cosmetics<'a>(&'a self, body: &'a Body) -> impl Iterator<Item = u64> + 'a {
        body.weapons
            .iter()
            .flatten()
            .filter_map(|&weapon| self.graph(weapon))
            .flat_map(|graph| {
                graph
                    .states
                    .iter()
                    .flatten()
                    .filter_map(|&pos| graph.node(pos))
            })
            .filter(|node| node.class == "STUStatescriptStateCosmeticEntity")
            .filter_map(|node| node.field("m_entityDef.m_entityDef").and_then(asset_guid))
    }

    pub fn entity_body(&self, guid: u64) -> Option<&Body> {
        self.entities.get(&guid)
    }

    /// Every entity definition the bundle keeps, by GUID.
    pub fn entity_bodies(&self) -> &BTreeMap<u64, Body> {
        &self.entities
    }

    /// The mode entity's variables at the start of a game of `mode` that
    /// `ruleset` sets: the mode's own (`0C5.m_3CE93B76`) with the ruleset's
    /// mode-level values over them (`0C0` `m_gamemode.m_3CE93B76`: the
    /// Practice Range's `0C0:34` turns the hero limit #9548 off). `ruleset`
    /// is a `0C0` GUID, a match's own ruleset
    /// ([`GraphStore::add_match_ruleset`], its values over the mode's), or 0
    /// for none. None: the bundle has none for the mode.
    pub fn mode_variables(&self, mode: u64, ruleset: u64) -> Option<Arc<BTreeMap<u32, ModeValue>>> {
        if let Some(own) = self.match_rulesets().get(&ruleset)
            && own.mode == mode
        {
            return Some(Arc::clone(&own.variables));
        }
        match self.ruleset_variables.get(&ruleset) {
            Some((of, variables)) if *of == mode => Some(Arc::clone(variables)),
            _ => self.mode_variables.get(&mode).cloned(),
        }
    }

    /// `GetGameRulesetValue`'s answer for `key` after the entity variable:
    /// a match ruleset's own value of the key (for `hero` of any team, then
    /// for every body), else the hero's and mode's schema default
    /// ([`Rulesets::default`]).
    pub fn ruleset_value(
        &self,
        key: u32,
        hero: u64,
        mode: u64,
        ruleset: u64,
    ) -> Option<crate::wire::Value> {
        let own = self.match_rulesets().get(&ruleset).and_then(|own| {
            own.bodies
                .iter()
                .rev()
                .filter(|body| body.covers(hero, None))
                .find_map(|body| body.values.get(&key))
                .or_else(|| own.values.get(&key))
                .copied()
        });
        match own {
            Some(value) => Some(value.into()),
            None => self.rulesets.default(key, hero, mode),
        }
    }

    /// A body's entity variables from a match's own ruleset: its values,
    /// then those of the [`BodyValues`] for the body's `hero` and `team`
    /// in order. A graph's `GetGameRulesetValue` reads the owner's entity
    /// variable of the key first, on the server and on each client
    /// (`Eval_STU_3832D36C`); the client has no other way to a custom
    /// match's values (its fallback is the schema defaults,
    /// `sub_7FF789CAC540`), so they go to the bodies its graphs run on: the
    /// enemy health bars (`0D24` st11, #9539), the cooldown scalars the
    /// ability graphs read (#9528, #9536, …). Empty for every other match.
    pub fn body_variables(
        &self,
        ruleset: u64,
        hero: u64,
        team: Option<u8>,
    ) -> crate::wire::Variables {
        let rulesets = self.match_rulesets();
        let Some(own) = rulesets.get(&ruleset) else {
            return crate::wire::Variables::new();
        };
        own.bodies
            .iter()
            .filter(|body| body.covers(hero, team))
            .fold(own.values.clone(), |mut values, body| {
                values.extend(&body.values);
                values
            })
            .into_iter()
            .map(|(key, value)| (key, value.into()))
            .collect()
    }

    /// Give a single match its own ruleset `id` (unused by any `0C0`):
    /// its games of `mode` read `values` before the schema defaults and
    /// over the mode entity's own variables, and its bodies get them with
    /// the `bodies` that cover them ([`GraphStore::body_variables`]).
    /// Empty for every match that is not a custom game;
    /// [`GraphStore::remove_match_ruleset`] when it ends.
    pub fn add_match_ruleset(
        &self,
        id: u64,
        mode: u64,
        values: BTreeMap<u32, ModeValue>,
        bodies: Vec<BodyValues>,
    ) {
        let mut variables = self
            .mode_variables
            .get(&mode)
            .map(|variables| BTreeMap::clone(variables))
            .unwrap_or_default();
        variables.extend(values.iter().map(|(&key, &value)| (key, value)));
        self.match_rulesets.write().expect("match rulesets").insert(
            id,
            MatchRuleset {
                mode,
                values,
                bodies,
                variables: Arc::new(variables),
            },
        );
    }

    pub fn remove_match_ruleset(&self, id: u64) {
        self.match_rulesets
            .write()
            .expect("match rulesets")
            .remove(&id);
    }

    fn match_rulesets(&self) -> RwLockReadGuard<'_, HashMap<u64, MatchRuleset>> {
        self.match_rulesets.read().expect("match rulesets")
    }

    pub fn missing(&self) -> &[u32] {
        &self.missing
    }
}

pub fn asset_guid(value: &Value) -> Option<u64> {
    let text = value.as_str()?;
    let hex = text.strip_prefix("0x")?;
    if hex.len() == 16
        && hex
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'A'..=b'F'))
    {
        parse_guid(text)
    } else {
        None
    }
}

pub fn target_tag_guid(value: &Value) -> Option<u64> {
    asset_guid(value).filter(|guid| (guid >> 48) & 0xfff == TARGET_TAG_TYPE)
}

fn body_target_tags<'de, D: serde::Deserializer<'de>>(
    decoder: D,
) -> Result<Option<BTreeSet<u64>>, D::Error> {
    Option::<Vec<Value>>::deserialize(decoder)?
        .map(|tags| {
            tags.iter()
                .map(|tag| {
                    target_tag_guid(tag).ok_or_else(|| {
                        serde::de::Error::custom("body target tag must be a packed .06C GUID")
                    })
                })
                .collect()
        })
        .transpose()
}

/// `value`'s member `key`, as `value.get(key)` finds it. The graphs'
/// objects are mostly short: one of up to [`SCANNED_MEMBERS`] members is
/// found by comparing the keys in order, which is faster than hashing the
/// key (the objects keep their order, so their maps hash with SipHash).
pub fn member<'a>(value: &'a Value, key: &str) -> Option<&'a Value> {
    match value {
        Value::Object(members) if members.len() <= SCANNED_MEMBERS => members
            .iter()
            .find_map(|(name, member)| (name == key).then_some(member)),
        Value::Object(members) => members.get(key),
        _ => None,
    }
}

const SCANNED_MEMBERS: usize = 12;

pub fn field_at<'a>(fields: &'a Value, path: &str) -> Option<&'a Value> {
    let mut value = fields;
    for part in path.split('.') {
        let (name, mut indexes) = part.split_at(part.find('[').unwrap_or(part.len()));
        if name.is_empty() || !name.chars().all(|ch| ch == '_' || ch.is_alphanumeric()) {
            return None;
        }
        value = member(value, name)?;
        while !indexes.is_empty() {
            let (index, tail) = indexes.strip_prefix('[')?.split_once(']')?;
            if index.is_empty() || !index.bytes().all(|ch| ch.is_ascii_digit()) {
                return None;
            }
            value = value.get(index.parse::<usize>().ok()?)?;
            indexes = tail;
        }
    }
    Some(value)
}

fn collect_plugs<'a>(value: &'a Value, path: &str, out: &mut BTreeMap<String, &'a Value>) {
    match value {
        Value::Object(fields) if fields.contains_key("$") && fields.contains_key("links") => {
            out.insert(path.to_owned(), value);
        }
        Value::Object(fields) => {
            for (key, item) in fields {
                if key != "$" {
                    let name = if path.is_empty() {
                        key.clone()
                    } else {
                        format!("{path}.{key}")
                    };
                    collect_plugs(item, &name, out);
                }
            }
        }
        Value::Array(values) => {
            for (index, item) in values.iter().enumerate() {
                collect_plugs(item, &format!("{path}[{index}]"), out);
            }
        }
        _ => {}
    }
}

fn collect_graphs(value: &Value, out: &mut BTreeSet<u64>) {
    match value {
        Value::Object(fields) => {
            for field in fields.values() {
                collect_graphs(field, out);
            }
        }
        Value::Array(values) => {
            for item in values {
                collect_graphs(item, out);
            }
        }
        _ => {
            if let Some(guid) = asset_guid(value).filter(|guid| guid >> 48 == GRAPH_TYPE) {
                out.insert(guid & 0x0000_ffff_ffff_ffff);
            }
        }
    }
}

/// `roots` and every graph their `PlayScript` actions play, in turn.
fn played_closure(
    graphs: &BTreeMap<u32, Arc<Graph>>,
    roots: impl Iterator<Item = u32>,
) -> BTreeSet<u32> {
    let mut found = BTreeSet::new();
    let mut queue: Vec<u32> = roots.collect();
    while let Some(index) = queue.pop() {
        if !found.insert(index) {
            continue;
        }
        let Some(graph) = graphs.get(&index) else {
            continue;
        };
        queue.extend(
            graph
                .nodes
                .iter()
                .filter(|node| node.class == "STUStatescriptActionPlayScript")
                .filter_map(|node| node.field("m_script.m_graph").and_then(asset_guid))
                .filter(|&played| played >> 48 == GRAPH_TYPE)
                .filter_map(|played| u32::try_from(played & 0xFFFF_FFFF_FFFF).ok()),
        );
    }
    found
}

/// The graph whose state holds the Workshop's value table: `0CBB` (every
/// mode entity's shared subgraph) plays it from its entry.
const WORKSHOP_VALUES_GRAPH: u32 = 0x1b4a;

/// `1B4A`'s `STU_25CBEFB0` map (`m_108E51DD`): each Workshop value's `01C`
/// GUID and the config the client evaluates for it
/// (`sub_7FF789C14870` looks a value up there by its identifier).
fn workshop_values(graph: &Graph) -> BTreeMap<u64, Value> {
    graph
        .nodes
        .iter()
        .filter(|node| node.class == "STU_25CBEFB0")
        .filter_map(|node| node.fields.get("m_108E51DD")?.get("entries")?.as_array())
        .flatten()
        .filter_map(|entry| {
            let [key, config] = entry.as_array()?.as_slice() else {
                return None;
            };
            Some((asset_guid(key)?, config.clone()))
        })
        .collect()
}

fn keyed_by_guid<T>(records: BTreeMap<String, T>) -> io::Result<BTreeMap<u64, T>> {
    records
        .into_iter()
        .map(|(key, value)| {
            let hex = key.strip_prefix("0x").unwrap_or(&key);
            let guid = u64::from_str_radix(hex, 16).map_err(invalid)?;
            Ok((guid, value))
        })
        .collect()
}

fn mode_values(values: BTreeMap<u32, Value>) -> io::Result<BTreeMap<u32, ModeValue>> {
    values
        .into_iter()
        .map(|(var, value)| Ok((var, mode_value(&value)?)))
        .collect()
}

fn mode_value(value: &Value) -> io::Result<ModeValue> {
    if let Some(value) = value.as_bool() {
        return Ok(ModeValue::Bool(value));
    }
    if let Some(value) = value.as_i64() {
        return Ok(ModeValue::Integer(value));
    }
    if let Some(value) = value.as_f64() {
        return Ok(ModeValue::Float(value));
    }
    asset_guid(value)
        .map(ModeValue::Asset)
        .ok_or_else(|| invalid("unsupported mode variable"))
}

fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(value) => *value,
        Value::Number(value) => value.as_f64() != Some(0.0),
        Value::String(value) => !value.is_empty(),
        Value::Array(value) => !value.is_empty(),
        Value::Object(value) => !value.is_empty(),
    }
}

fn node_flag<'de, D: serde::Deserializer<'de>>(decoder: D) -> Result<bool, D::Error> {
    Value::deserialize(decoder).map(|value| truthy(&value))
}

fn invalid(error: impl std::fmt::Display) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn fixture() -> Value {
        let node = |pos, rid, client_only, fields: Value| {
            json!({
                "pos": pos, "uid": 0, "rid": rid, "remote": 0, "state": null,
                "class": "Node", "client_only": client_only, "server_only": 0, "fields": fields
            })
        };
        let body = json!({
            "hero": null, "name": "Fixture", "manager": null, "weapons": [null, 42],
            "graphs": [{"graph": 42, "m_1EB5A024": null, "m_75408640": 0}]
        });
        json!({
            "version": 1,
            "graphs": [{
                "index": 42,
                "header": {"m_nodesBitCount": 4, "m_statesBitCount": 3,
                    "m_remoteSyncNodesBitCount": 3, "m_syncVarsBitCount": 3},
                "fields": {"ref": "0x058000000000001A", "ignored": "0x058000000000001b"},
                "items": 5, "containers": {"3": "Opaque"}, "unknown_nodes": [8],
                "nodes": [
                    node(0, 3, false, json!({
                        "m_2BBEEAB8": true,
                        "entries": [{"trigger": {"$": "Plug", "links": [[2, "input"], [null, "unknown"]]} }],
                        "related": ["0x058000000000002B", "0x040000000000002A"]
                    })),
                    node(1, 5, true, json!({"m_2BBEEAB8": true})),
                    node(2, 6, false, json!({"m_ADEB6E05": true}))
                ],
                "states": [0, 1, null, 0],
                "entries": [0, 3, 2, null],
                "remote": [2, null, 1, 0],
                "sync_vars": [[null, 0, 0], [12, 1, 0], [3, 0, 1], [42, 0, 0]]
            }],
            "bodies": {"0x0400000000000001": body.clone()},
            "entities": {"0x0400000000000002": body},
            "mode_variables": {"0x0230000000000018": {
                "1": false, "2": 3, "3": "0x0D8000000000195F"
            }},
            "missing": [43]
        })
    }

    #[test]
    fn body_target_tags_distinguish_unknown_empty_and_invalid_data() {
        let mut data = fixture();
        let body = "0x0400000000000001";
        let entity = "0x0400000000000002";
        let load = |data: &Value| GraphStore::from_json(&serde_json::to_vec(data).unwrap());

        assert!(
            load(&data).unwrap().bodies()[&0x0400_0000_0000_0001]
                .target_tags
                .is_none()
        );
        data["bodies"][body]["target_tags"] = json!([]);
        data["entities"][entity]["target_tags"] = json!([
            "0x0D60000000000159",
            "0x0D60000000000109",
            "0x0D60000000000159"
        ]);
        let parsed = load(&data).unwrap();
        assert_eq!(
            parsed.bodies()[&0x0400_0000_0000_0001].target_tags,
            Some(BTreeSet::new())
        );
        assert_eq!(
            parsed
                .entity_body(0x0400_0000_0000_0002)
                .unwrap()
                .target_tags,
            Some(BTreeSet::from([
                0x0d60_0000_0000_0109,
                0x0d60_0000_0000_0159
            ]))
        );
        data["bodies"][body]["target_tags"] = json!(["0x0400000000000109"]);
        assert_eq!(load(&data).unwrap_err().kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn builds_wire_slots_and_keeps_data_order_through_holes() {
        let store = GraphStore::from_json(&serde_json::to_vec(&fixture()).unwrap()).unwrap();
        let graph = store.graph(42).unwrap();
        assert_eq!(graph.owner_positions(), [0, 0]);
        assert_eq!(graph.remote_positions(), [0]);
        assert_eq!(graph.owner_slot(0), Some(0));
        assert_eq!(graph.owner_slot(3), Some(0));
        assert_eq!(graph.owner_slot(1), None);
        assert_eq!(graph.remote_slot(0), None);
        assert_eq!(graph.remote_slot(3), Some(0));
        assert_eq!(graph.presence_bit(42), Some(1));
        assert_eq!(graph.presence_bit(12), None);
        assert_eq!(graph.graph_refs(), BTreeSet::from([26, 43]));
        assert_eq!(graph.by_rid(6).unwrap().pos, 2);
        assert_eq!(graph.node_ref(&json!({"node": 2})).unwrap().rid, 6);
        assert!(graph.node(3).is_none());

        let node = graph.node(0).unwrap();
        let plug_path = "entries[0].trigger";
        assert_eq!(node.plugs().keys().next().unwrap(), plug_path);
        assert_eq!(
            node.links(plug_path).collect::<Vec<_>>(),
            [(Some(2), "input"), (None, "unknown")]
        );
        for invalid in [
            "entries[-1].trigger",
            "entries[0]x",
            ".entries[0]",
            "entries[1]",
        ] {
            assert_eq!(node.field(invalid), None);
        }
        assert_eq!(store.missing(), [43]);
        assert_eq!(
            store.bodies()[&0x0400_0000_0000_0001].graphs[0].graph,
            Some(42)
        );
        assert_eq!(
            store.entity_body(0x0400_0000_0000_0002).unwrap().name,
            "Fixture"
        );
        assert_eq!(
            *store.mode_variables(0x0230_0000_0000_0018, 0).unwrap(),
            BTreeMap::from([
                (1, ModeValue::Bool(false)),
                (2, ModeValue::Integer(3)),
                (3, ModeValue::Asset(0x0d80_0000_0000_195f)),
            ])
        );
    }

    /// A game ruleset's mode-level values (`0C0` `m_gamemode.m_3CE93B76`)
    /// go over its mode's variables in its games: the Practice Range's
    /// `0C0:34` sets the hero limit #9548 to `254D`. A game without a
    /// ruleset, or of another mode, has the mode's own. A match's own
    /// ruleset (a custom game) does the same with its values, and answers
    /// `GetGameRulesetValue` with them before the schema defaults, until it
    /// is removed.
    #[test]
    fn a_rulesets_values_go_over_its_modes_variables() {
        const MODE: u64 = 0x0230_0000_0000_0018;
        const RULESET: u64 = 0x0fd0_0000_0000_0034;
        let mut data = fixture();
        data["ruleset_variables"] = json!({"0x0FD0000000000034": {
            "mode": "0x0230000000000018",
            "variables": {"2": 5, "9531": 0.3, "9548": "0x0D8000000000254D"}
        }});
        let store = GraphStore::from_json(&serde_json::to_vec(&data).unwrap()).unwrap();
        let own = BTreeMap::from([
            (1, ModeValue::Bool(false)),
            (2, ModeValue::Integer(3)),
            (3, ModeValue::Asset(0x0d80_0000_0000_195f)),
        ]);
        let mut ruled = own.clone();
        ruled.extend([
            (2, ModeValue::Integer(5)),
            (9531, ModeValue::Float(0.3)),
            (9548, ModeValue::Asset(0x0d80_0000_0000_254d)),
        ]);
        assert_eq!(store.mode_variables(MODE, RULESET).as_deref(), Some(&ruled));
        assert_eq!(store.mode_variables(MODE, 0).as_deref(), Some(&own));
        assert_eq!(store.mode_variables(0x0230_0000_0000_005a, RULESET), None);

        const CUSTOM: u64 = 0x0fd0_ffff_0000_0001;
        const DOOMFIST: u64 = 0x02e0_0000_0000_012f;
        assert_eq!(store.ruleset_value(9531, 0, MODE, CUSTOM), None);
        store.add_match_ruleset(
            CUSTOM,
            MODE,
            BTreeMap::from([(9531, ModeValue::Float(0.0)), (2, ModeValue::Integer(12))]),
            vec![
                BodyValues {
                    team: None,
                    hero: Some(DOOMFIST),
                    values: BTreeMap::from([(9744, ModeValue::Float(0.25))]),
                },
                BodyValues {
                    team: Some(1),
                    hero: None,
                    values: BTreeMap::from([(9531, ModeValue::Float(0.5))]),
                },
            ],
        );
        let mut custom = own.clone();
        custom.extend([(9531, ModeValue::Float(0.0)), (2, ModeValue::Integer(12))]);
        assert_eq!(store.mode_variables(MODE, CUSTOM).as_deref(), Some(&custom));
        assert_eq!(
            store.ruleset_value(9531, 0, MODE, CUSTOM),
            Some(crate::wire::Value::Float(0.0))
        );
        // A hero's own value is his in either team; a team's needs the
        // body's team.
        assert_eq!(
            store.ruleset_value(9744, DOOMFIST, MODE, CUSTOM),
            Some(crate::wire::Value::Float(0.25))
        );
        assert_eq!(store.ruleset_value(9744, 0, MODE, CUSTOM), None);
        use crate::wire::{Value as Wire, Variables};
        assert_eq!(
            store.body_variables(CUSTOM, DOOMFIST, Some(1)),
            Variables::from([
                (2, Wire::Integer(12)),
                (9531, Wire::Float(0.5)),
                (9744, Wire::Float(0.25)),
            ])
        );
        assert_eq!(
            store.body_variables(CUSTOM, 0x02e0_0000_0000_0002, Some(0)),
            Variables::from([(2, Wire::Integer(12)), (9531, Wire::Float(0.0))])
        );
        assert!(store.body_variables(0, DOOMFIST, Some(0)).is_empty());
        // Every other match keeps its values.
        assert_eq!(store.ruleset_value(9531, 0, MODE, 0), None);
        assert_eq!(store.mode_variables(MODE, 0).as_deref(), Some(&own));
        store.remove_match_ruleset(CUSTOM);
        assert_eq!(store.ruleset_value(9531, 0, MODE, CUSTOM), None);
        assert_eq!(store.mode_variables(MODE, CUSTOM).as_deref(), Some(&own));
    }

    #[test]
    fn reads_gzip_and_rejects_truncated_or_inconsistent_data() {
        use flate2::{Compression, write::GzEncoder};
        use std::io::Write;

        let mut encoder = GzEncoder::new(Vec::new(), Compression::fast());
        encoder
            .write_all(&serde_json::to_vec(&fixture()).unwrap())
            .unwrap();
        let bytes = encoder.finish().unwrap();
        assert!(
            GraphStore::from_gzip(bytes.as_slice())
                .unwrap()
                .graph(42)
                .is_some()
        );
        assert!(GraphStore::from_gzip(&bytes[..bytes.len() - 4]).is_err());
        let mut bad = fixture();
        bad["graphs"][0]["nodes"][0]["pos"] = json!(6);
        assert!(GraphStore::from_json(&serde_json::to_vec(&bad).unwrap()).is_err());
        bad["version"] = json!(2);
        assert!(GraphStore::from_json(&serde_json::to_vec(&bad).unwrap()).is_err());
    }

    #[test]
    #[ignore = "requires a derived 104319 state graph bundle"]
    fn whole_retail_wire_index_matches_the_python_reference() {
        use sha2::{Digest, Sha256};
        let root = orpheus_testkit::runtime_dir();
        let store = GraphStore::load(root.join("data/game_graphs_174.json.gz")).unwrap();
        let mut digest = Sha256::new();
        fn sequence(digest: &mut Sha256, items: impl IntoIterator<Item = u64>) {
            let items: Vec<_> = items.into_iter().collect();
            digest.update((items.len() as u64).to_le_bytes());
            for item in items {
                digest.update(item.to_le_bytes());
            }
        }
        // The initial graphs of the entities hero weapon volleys spawn (and
        // what they reference), which the derivation adds and the Python
        // extractor didn't follow (`crates/assets/tests/graphs.rs`
        // `SPAWNED_GRAPHS`): the reference vectors don't cover them.
        const SPAWNED: [u32; 25] = [
            0x00C3, 0x0132, 0x0252, 0x065E, 0x096B, 0x0BA6, 0x0C1A, 0x0C1B, 0x0CCC, 0x0CCE, 0x0D00,
            0x0E84, 0x10FF, 0x1100, 0x1502, 0x150F, 0x151A, 0x157F, 0x1590, 0x1688, 0x1689, 0x18EB,
            0x1C26, 0x1EB9, 0x22B8,
        ];
        // The graphs the reference vectors cover: those the derivation
        // reaches from its own root graphs (`ROOT_GRAPHS` and
        // `MODE_FLOW_GRAPHS` in `crates/assets/src/graphs.rs`) and the hero
        // bodies' graphs, through the graphs each references. The
        // derivation has since added more, which the vectors don't cover;
        // each is left out by its reason, counted, so a wrong walk can't
        // hide a graph:
        // - the spawned graphs above;
        // - the aura graphs (2026-11-13, `aura_graphs`): an aura's
        //   `m_5A4051BC` (`GraphStore::is_aura_graph`), or its
        //   `m_14AA219A` (the 24 auras without the first), and what they
        //   reference;
        // - the map placeables' graphs (2026-11-14, `placed_entities`):
        //   their definitions' graphs and the graphs their instance data
        //   name (in the bundle's root graphs), and what they reference.
        const ROOTS: [u32; 25] = [
            0x13C1, 0x0C90, 0x288A, 0x288B, 0x288D, 0x20E4, 0x13C0, 0x0F4A, 0x0EC2, 0x0C8E, 0x0258,
            0x0D76, 0x0D0B, 0x0D6F, 0x0D38, 0x0F60, 0x0F62, 0x1066, 0x0F73, 0x0F74, 0x1058, 0x0F4C,
            0x0E8E, 0x1225, 0x0D32,
        ];
        let raw: Value = {
            let mut text = Vec::new();
            MultiGzDecoder::new(BufReader::new(
                File::open(root.join("data/game_graphs_174.json.gz")).unwrap(),
            ))
            .read_to_end(&mut text)
            .unwrap();
            serde_json::from_slice(&text).unwrap()
        };
        let body_graphs = |body: &Body| -> Vec<u32> {
            body.graphs
                .iter()
                .filter_map(|binding| binding.graph)
                .chain(body.manager)
                .chain(body.weapons.iter().flatten().copied())
                .collect()
        };
        // The graphs `starts` reach that `reached` doesn't hold yet; they
        // join it.
        let walk = |reached: &mut BTreeSet<u32>, starts: Vec<u32>| -> BTreeSet<u32> {
            let mut queue = starts;
            let mut new = BTreeSet::new();
            while let Some(index) = queue.pop() {
                let Some(graph) = store.graph(index).filter(|_| reached.insert(index)) else {
                    continue;
                };
                new.insert(index);
                queue.extend(
                    graph
                        .graph_refs()
                        .into_iter()
                        .filter_map(|graph| u32::try_from(graph).ok()),
                );
            }
            new
        };
        let aura_graphs = |field: &str| -> Vec<u32> {
            raw["auras"]
                .as_object()
                .into_iter()
                .flat_map(|auras| auras.values())
                .filter_map(|aura| asset_guid(&aura[field]))
                .filter_map(|graph| u32::try_from(graph & 0xFFFF_FFFF_FFFF).ok())
                .collect()
        };
        let mut reached = BTreeSet::new();
        let covered = walk(
            &mut reached,
            ROOTS
                .into_iter()
                .chain(store.bodies().values().flat_map(body_graphs))
                .collect(),
        );
        let spawned = walk(&mut reached, SPAWNED.to_vec());
        let auras = walk(&mut reached, aura_graphs("m_5A4051BC"));
        let second_auras = walk(&mut reached, aura_graphs("m_14AA219A"));
        // The Workshop's condition and action graphs (2026-11-20,
        // `workshop_graphs`), roots of their own: the catalog's.
        let catalog: Value = serde_json::from_slice(
            &std::fs::read(root.join("data/workshop_catalog_174.json")).unwrap(),
        )
        .unwrap();
        let workshop_roots: BTreeSet<u32> = ["events", "conditions", "actions"]
            .into_iter()
            .flat_map(|kind| catalog[kind].as_array().unwrap())
            .filter_map(|entry| asset_guid(&entry["graph"]))
            .filter_map(|graph| u32::try_from(graph & 0xFFFF_FFFF_FFFF).ok())
            .collect();
        let placeables = walk(
            &mut reached,
            raw["roots"]["graphs"]
                .as_array()
                .unwrap()
                .iter()
                .map(|graph| u32::try_from(graph.as_u64().unwrap()).unwrap())
                .filter(|graph| !workshop_roots.contains(graph))
                .chain(store.entities.values().flat_map(body_graphs))
                .collect(),
        );
        let workshop = walk(&mut reached, workshop_roots.into_iter().collect());
        let unreached: Vec<u32> = store
            .graph_indexes()
            .filter(|index| !reached.contains(index))
            .collect();
        eprintln!(
            "left out: spawned {}, aura m_5A4051BC {}, aura m_14AA219A {}, placeables {}, \
             Workshop {}; unreached {unreached:04X?}",
            spawned.len(),
            auras.len(),
            second_auras.len(),
            placeables.len(),
            workshop.len()
        );
        assert!(
            unreached.is_empty(),
            "graphs no walk reaches: {unreached:04X?}"
        );
        assert!(spawned.iter().all(|index| SPAWNED.contains(index)));
        let left_out = (
            spawned.len(),
            auras.len(),
            second_auras.len(),
            placeables.len(),
            workshop.len(),
        );
        let mut hashed = 0;
        for index in store.graph_indexes() {
            if !covered.contains(&index) {
                continue;
            }
            hashed += 1;
            let graph = store.graph(index).unwrap();
            sequence(
                &mut digest,
                [
                    u64::from(index),
                    u64::from(graph.items),
                    u64::from(graph.header.nodes_bits),
                    u64::from(graph.header.states_bits),
                    u64::from(graph.header.remote_bits),
                    u64::from(graph.header.sync_vars_bits),
                    graph.nodes.len() as u64,
                ],
            );
            sequence(
                &mut digest,
                graph.owner_positions().iter().copied().map(u64::from),
            );
            sequence(
                &mut digest,
                graph.remote_positions().iter().copied().map(u64::from),
            );
            sequence(
                &mut digest,
                graph.presence_vars().flat_map(|(index, var)| {
                    [
                        index as u64,
                        u64::from(var.0.unwrap()),
                        u64::from(var.1),
                        u64::from(var.2),
                    ]
                }),
            );
            sequence(&mut digest, graph.graph_refs());
            sequence(
                &mut digest,
                graph.unknown_rids.iter().copied().map(u64::from),
            );
        }
        assert_eq!(store.bodies().len(), 32);
        let digest = format!("{:x}", digest.finalize());
        let expected = match hashed {
            373 => "f54fb85e91d33b669c73239ddc77fb0f8f7fa6e6d0f04b83532bc1f953a4ac7c",
            405 => {
                assert_eq!(store.entities.len(), 8);
                assert_eq!(store.mode_variables.len(), 25);
                "eab09343964a53d44a53baa8d990ff9c2a3eea8922d9eb1927136fb7b661fc94"
            }
            // Rust only (no Python vector): every mode's script, team
            // controller and team body, and the map entity graphs.
            412 => {
                assert_eq!(store.mode_variables.len(), 25);
                // Data version 25: the auras counted include those only the
                // placeables' graphs name, as their walk comes first.
                assert_eq!(left_out, (25, 44, 21, 159, 237));
                "1c4229042f26d5ec0168a49e7f82d55167543ae849baa86f9bfdba7ece355691"
            }
            count => panic!("no reference vector for the {count}-graph bundle (digest {digest})"),
        };
        assert_eq!(digest, expected);
    }
}
