//! The index the queries read: every variable, identifier, graph, asset and
//! class each node holds ("reading a node" and "the index" in
//! `tools/ssq.py`). Python keeps it in a pickle; it builds in about a second,
//! so the port builds it on each run.

use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    fs, io,
    path::Path,
};

use indexmap::IndexMap;

use super::{
    data::{Bundle, GRAPH_TYPE, Graph, Names, guid},
    json::{self, Int, J},
    render::{IDENT_TYPE, constant_type, dynamic, guid_text, label, operands, render, value_type},
};

pub const ROLES: [&str; 6] = ["body", "ability", "weapon", "mode", "controller", "UI"];
const ABILITY: &str = "STUStatescriptStateAbility";
const EXPRESSION_DATA: &str = "STUConfigVarExpressionData";
const REMOTE_SYNC_VAR: &str = "STUStatescriptRemoteSyncVar";
/// Objects that pair a variable identifier with a value: (class, identifier
/// field, value field, what it is).
const ENTRIES: &[(&str, &str, &str, &str)] = &[
    (
        "STUStatescriptSchemaEntry",
        "m_0D09D2D9",
        "m_value",
        "default",
    ),
    ("STU_BDFD54D7", "m_1BF2BEEA", "m_A53100D5", "default"),
    ("STU_6364CD4F", "m_0D09D2D9", "m_value", "set"),
];

fn entry_fields(cls: &str) -> Option<(&'static str, &'static str, &'static str)> {
    ENTRIES
        .iter()
        .find(|(name, ..)| *name == cls)
        .map(|&(_, key, field, what)| (key, field, what))
}

/// What one node (or a graph's own fields) holds.
#[derive(Default)]
pub struct Refs<'a> {
    /// (kind, variable, scope, path)
    pub vars: Vec<(&'static str, Int, Int, String)>,
    /// (graph index, path)
    pub graphs: Vec<(Int, String)>,
    /// (identifier, holder class, field, path)
    pub ids: Vec<(Int, String, String, String)>,
    /// (identifier, what, value config var, path)
    pub entries: Vec<(Int, &'static str, Option<&'a J>, String)>,
    /// (identifier, scope, path)
    pub syncs: Vec<(Int, Option<Int>, String)>,
    /// (GUID, path)
    pub assets: Vec<(Int, String)>,
    /// (class, object)
    pub objects: Vec<(&'a str, &'a J)>,
    /// Logical button values.
    pub buttons: Vec<Option<&'a J>>,
}

/// Walks a node's fields and sorts every variable, identifier, graph and
/// asset it holds.
pub struct Walker<'n> {
    var_fields: &'n HashMap<String, HashMap<String, String>>,
}

impl<'n> Walker<'n> {
    pub fn new(names: &'n Names) -> Self {
        Self {
            var_fields: &names.var_fields,
        }
    }

    pub fn run<'a>(&self, fields: &'a J, cls: &'a str) -> Refs<'a> {
        let mut refs = Refs::default();
        if fields.as_obj().is_some() {
            self.obj(&mut refs, fields, cls, "");
        }
        refs
    }

    fn declared(&self, holder: &str, field: &str) -> Option<&str> {
        self.var_fields.get(holder)?.get(field).map(String::as_str)
    }

    fn obj<'a>(&self, refs: &mut Refs<'a>, value: &'a J, cls: &'a str, path: &str) {
        let obj = value.as_obj().expect("an object");
        refs.objects.push((cls, value));
        // Every STUConfigVar has it, nothing else does (checked with OWLib).
        let config = obj.contains("m_EE729DCB");
        if let Some((key, field, what)) = entry_fields(cls) {
            if let Some(ident) = obj.val(key).and_then(guid) {
                refs.entries
                    .push((ident & 0xFFFF, what, obj.val(field), path.to_owned()));
            }
        } else if cls == REMOTE_SYNC_VAR {
            if let Some(ident) = obj.val("m_0D09D2D9").and_then(guid) {
                let scope = obj.val("m_56341592").and_then(J::as_int);
                refs.syncs.push((ident & 0xFFFF, scope, path.to_owned()));
            }
        } else if cls == "STUConfigVarLogicalButton" {
            refs.buttons.push(obj.val("m_logicalButton"));
        }
        for (key, item) in obj.iter() {
            if key != "$" && key != "links" {
                let path = if path.is_empty() {
                    key.to_owned()
                } else {
                    format!("{path}.{key}")
                };
                self.value(refs, item, cls, config, key, &path);
            }
        }
    }

    fn value<'a>(
        &self,
        refs: &mut Refs<'a>,
        item: &'a J,
        holder: &'a str,
        config: bool,
        field: &str,
        path: &str,
    ) {
        match item {
            J::Obj(obj) => {
                let Some(cls) = item.class() else {
                    return; // {"node": ...} or {"item": ...}
                };
                if cls == "STUConfigVarDynamic" {
                    let kind = self.kind(holder, config, field);
                    self.var(refs, item, kind, path);
                    refs.objects.push((cls, item));
                } else if obj.val("m_expression").and_then(J::as_obj).is_some() {
                    let out_field = !config && self.declared(holder, field) == Some("var");
                    self.expression(refs, item, cls, path, out_field);
                } else {
                    self.obj(refs, item, cls, path);
                }
            }
            J::Arr(items) => {
                for (number, element) in items.iter().enumerate() {
                    self.value(
                        refs,
                        element,
                        holder,
                        config,
                        field,
                        &format!("{path}[{number}]"),
                    );
                }
            }
            J::Str(_) => {
                let Some(guid) = guid(item) else {
                    return;
                };
                if guid >> 48 == GRAPH_TYPE {
                    refs.graphs.push((guid & 0xFFFF_FFFF_FFFF, path.to_owned()));
                } else if guid >> 48 != IDENT_TYPE {
                    refs.assets.push((guid, path.to_owned()));
                } else if holder != REMOTE_SYNC_VAR
                    && !entry_fields(holder).is_some_and(|(key, ..)| key == field)
                {
                    refs.ids.push((
                        guid & 0xFFFF,
                        holder.to_owned(),
                        field.to_owned(),
                        path.to_owned(),
                    ));
                }
            }
            _ => {}
        }
    }

    fn kind(&self, holder: &str, config: bool, field: &str) -> &'static str {
        if config {
            return "read"; // an argument of a config var
        }
        match self.declared(holder, field) {
            Some("var") => "write",
            Some("list") => "list",
            _ => "read",
        }
    }

    fn var(&self, refs: &mut Refs, cfg: &J, kind: &'static str, path: &str) {
        let (scope, var) = dynamic(cfg).expect("a dynamic config var");
        refs.vars.push((kind, var, scope, path.to_owned()));
    }

    /// The variables of an expression by what its bytecode does with them;
    /// in a variable field the first variable pushed is the one written
    /// (`expr.lvalue`).
    fn expression<'a>(
        &self,
        refs: &mut Refs<'a>,
        cfg: &'a J,
        cls: &'a str,
        path: &str,
        out_field: bool,
    ) {
        refs.objects.push((cls, cfg));
        let data = cfg.at("m_expression");
        refs.objects.push((EXPRESSION_DATA, data));
        let code: Vec<Int> = data
            .at("m_opcodes")
            .items()
            .iter()
            .map(|op| op.as_int().unwrap_or(0))
            .collect();
        let (uses, first) = bytecode_uses(&code);
        for (number, dynamic) in data.at("m_dynamicVars").items().iter().enumerate() {
            if dynamic.class() != Some("STUConfigVarDynamic") {
                continue;
            }
            let mut kinds: BTreeSet<&'static str> = uses
                .get(&(number as Int))
                .cloned()
                .unwrap_or_else(|| BTreeSet::from(["unused"]));
            if out_field && Some(number as Int) == first {
                kinds.remove("read");
                kinds.insert("write");
            }
            for kind in kinds {
                self.var(
                    refs,
                    dynamic,
                    kind,
                    &format!("{path}.m_expression.m_dynamicVars[{number}]"),
                );
            }
        }
        if let Some(obj) = data.as_obj() {
            for (key, item) in obj.iter() {
                if key != "$" && key != "m_dynamicVars" {
                    self.value(
                        refs,
                        item,
                        EXPRESSION_DATA,
                        true,
                        key,
                        &format!("{path}.m_expression.{key}"),
                    );
                }
            }
        }
        for (key, item) in cfg.as_obj().expect("an object").iter() {
            if key != "$" && key != "m_expression" {
                self.value(refs, item, cls, true, key, &format!("{path}.{key}"));
            }
        }
    }
}

/// What an expression's bytecode does with each `m_dynamicVars` entry (the
/// ops of `expr.run`), following its jumps (code after an END is only
/// reached by a jump), and the entry its first push reads.
pub fn bytecode_uses(code: &[Int]) -> (HashMap<Int, BTreeSet<&'static str>>, Option<Int>) {
    let mut uses: HashMap<Int, BTreeSet<&'static str>> = HashMap::new();
    let mut first = None;
    let mut seen = HashSet::new();
    let mut todo = vec![0];
    let len = code.len() as Int;
    let at = |pc: Int| code[usize::try_from(pc).expect("pc in the code")];
    while let Some(mut pc) = todo.pop() {
        while 0 <= pc && pc < len && seen.insert(pc) {
            let op = at(pc);
            let arg = if pc + 1 < len { at(pc + 1) } else { 0 };
            match op {
                0 => break,
                1 => {
                    pc += arg;
                    continue;
                }
                // and, or, jump if false, jump if true: both ways
                2..=5 => {
                    todo.push(pc + arg);
                    pc += 2;
                    continue;
                }
                8..=10 => {
                    let number = if op == 10 && pc + 2 < len {
                        at(pc + 2)
                    } else {
                        arg
                    };
                    uses.entry(number).or_default().insert(if op == 10 {
                        "parent"
                    } else {
                        "read"
                    });
                    first.get_or_insert(number);
                }
                7 | 13 => {
                    uses.entry(arg)
                        .or_default()
                        .insert(if op == 7 { "id" } else { "param" });
                }
                _ => {}
            }
            pc += 1 + operands(op);
        }
    }
    (uses, first)
}

/// A graph's numbers and node table (`graph_summary`).
pub struct Summary {
    pub nodes: usize,
    pub states: usize,
    pub entries: usize,
    pub remote: usize,
    pub unknown: usize,
    pub networked: usize,
    pub client_only: usize,
    pub server_only: usize,
    pub owner_states: usize,
    pub remote_states: usize,
    pub sync_vars: usize,
    pub presence: Vec<Int>,
    /// Nodes, states, remote sync nodes and sync vars.
    pub bits: [String; 4],
    pub prediction: String,
    pub classes: IndexMap<String, usize>,
    pub nodes_info: HashMap<Int, NodeInfo>,
    /// (rid, st, logical buttons) of each Ability state.
    pub abilities: Vec<(Int, Option<Int>, Vec<Int>)>,
}

pub struct NodeInfo {
    pub state: Option<Int>,
    pub cls: String,
    pub client_only: bool,
    pub server_only: bool,
}

impl Summary {
    fn of(graph: &Graph) -> Self {
        let header = |key: &str| json::opt_str(graph.header.val(key));
        let mut classes = IndexMap::new();
        for node in &graph.nodes {
            *classes.entry(node.cls.clone()).or_insert(0) += 1;
        }
        Self {
            nodes: graph.nodes.len(),
            states: graph.states.iter().flatten().count(),
            entries: graph.entries.iter().flatten().count(),
            remote: graph.remote_nodes.iter().flatten().count(),
            unknown: graph.unknown,
            networked: graph
                .nodes
                .iter()
                .filter(|node| node.is_networked())
                .count(),
            client_only: graph.nodes.iter().filter(|node| node.client_only).count(),
            server_only: graph.nodes.iter().filter(|node| node.server_only).count(),
            owner_states: graph.owner_states(),
            remote_states: graph.remote_states(),
            sync_vars: graph.sync_vars.len(),
            presence: graph
                .presence_vars()
                .filter_map(|entry| entry.var)
                .collect(),
            bits: [
                header("m_nodesBitCount"),
                header("m_statesBitCount"),
                header("m_remoteSyncNodesBitCount"),
                header("m_syncVarsBitCount"),
            ],
            prediction: header("m_predictionBehavior"),
            classes,
            nodes_info: graph
                .nodes
                .iter()
                .map(|node| {
                    let info = NodeInfo {
                        state: node.state,
                        cls: node.cls.clone(),
                        client_only: node.client_only,
                        server_only: node.server_only,
                    };
                    (node.rid, info)
                })
                .collect(),
            abilities: Vec::new(),
        }
    }
}

/// One use of a variable.
pub struct VarUse {
    pub gi: Int,
    pub rid: Option<Int>,
    pub kind: &'static str,
    pub scope: Int,
    pub path: String,
}

/// A default, set value or override of a variable.
pub struct EntryUse {
    pub what: &'static str,
    pub gi: Int,
    pub rid: Option<Int>,
    pub place: String,
    pub text: String,
    pub kind: Option<String>,
}

/// An `m_syncVars` entry of a graph.
pub struct GraphSync {
    pub gi: Int,
    pub number: usize,
    pub scope: Option<Int>,
    pub flag: String,
    pub bit: Option<usize>,
}

/// A node's remote sync variable.
pub struct NodeSync {
    pub gi: Int,
    pub rid: Option<Int>,
    pub scope: Option<Int>,
    pub path: String,
}

pub struct IdUse {
    pub gi: Int,
    pub rid: Option<Int>,
    pub holder: String,
    pub field: String,
    pub path: String,
}

pub struct Edge {
    pub parent: Int,
    pub rid: Option<Int>,
    pub child: Int,
    pub path: String,
}

/// Per field of a class: objects that have it, objects where it is set,
/// and how often each kind of value is in it.
pub type FieldStats = IndexMap<String, (usize, usize, IndexMap<String, usize>)>;

/// (role, owner, slot) of a place a graph is attached.
pub type Attachment = (String, String, String);

/// A class of OWLib's STU types: hash, parent, (field, type, offset).
pub type OwlibClass = (
    String,
    Option<String>,
    Vec<(String, String, Option<String>)>,
);

#[derive(Default)]
pub struct Index {
    pub graphs: BTreeMap<Int, Summary>,
    pub vars: HashMap<Int, Vec<VarUse>>,
    pub syncs: HashMap<Int, Vec<NodeSync>>,
    pub sync_vars: HashMap<Int, Vec<GraphSync>>,
    pub entries: HashMap<Int, Vec<EntryUse>>,
    pub ids: HashMap<Int, Vec<IdUse>>,
    pub assets: HashMap<Int, Vec<(Int, Option<Int>, String)>>,
    pub class_nodes: HashMap<String, Vec<(Int, Int)>>,
    pub class_values: HashMap<String, Vec<(Int, Option<Int>)>>,
    pub edges: Vec<Edge>,
    pub fields: HashMap<String, FieldStats>,
    pub objects: HashMap<String, usize>,
    /// variable -> (graph, scope, type) of each constant written to it.
    pub constant_writes: HashMap<Int, IndexMap<(Int, Int, &'static str), usize>>,
    pub attach: HashMap<Int, Vec<Attachment>>,
    pub roles: HashMap<Int, Vec<&'static str>>,
    pub owlib: HashMap<String, OwlibClass>,
}

impl Index {
    pub fn build(bundle: &Bundle, names: &Names, owlib: Option<&Path>) -> io::Result<Self> {
        let walker = Walker::new(names);
        let graphs: Vec<(Int, &Graph)> = bundle
            .graphs
            .iter()
            .map(|(&gi, graph)| (gi, graph))
            .collect();
        // Each thread indexes runs of neighbouring graphs, and the runs are
        // merged in order: merging one graph at a time costs as much as
        // indexing it.
        let threads = std::thread::available_parallelism().map_or(1, usize::from);
        let runs: Vec<&[(Int, &Graph)]> = graphs
            .chunks(graphs.len().div_ceil(threads * 4).max(1))
            .collect();
        let mut parts = super::par_map(&runs, |run| {
            let mut part = Index::default();
            for &(gi, graph) in *run {
                part.add_graph(&walker, names, gi, graph);
            }
            part
        })
        .into_iter();
        let mut index = parts.next().unwrap_or_default();
        for part in parts {
            index.merge(part);
        }
        index.attach = index.attachments(bundle, names);
        index.roles = index.roles();
        if let Some(folder) = owlib {
            let wanted: BTreeSet<&str> = index
                .objects
                .keys()
                .chain(index.class_nodes.keys())
                .map(String::as_str)
                .collect();
            index.owlib = owlib_types(folder, &wanted)?;
        }
        Ok(index)
    }

    /// Indexes the next graph.
    fn add_graph(&mut self, walker: &Walker, names: &Names, gi: Int, graph: &Graph) {
        let mut summary = Summary::of(graph);
        self.add_refs(names, gi, None, walker.run(&graph.fields, "graph"));
        for node in &graph.nodes {
            let refs = walker.run(&node.fields, &node.cls);
            if node.cls == ABILITY {
                let buttons: BTreeSet<Int> = refs
                    .buttons
                    .iter()
                    .filter_map(|value| match value {
                        Some(J::Int(value)) => Some(*value),
                        _ => None,
                    })
                    .collect();
                summary
                    .abilities
                    .push((node.rid, node.state, buttons.into_iter().collect()));
            }
            self.add_refs(names, gi, Some(node.rid), refs);
            self.class_nodes
                .entry(node.cls.clone())
                .or_default()
                .push((gi, node.rid));
        }
        for entry in &graph.sync_vars {
            if let Some(var) = entry.var {
                let bit = if entry.has_presence_bit() {
                    graph.presence_bit(var)
                } else {
                    None
                };
                self.sync_vars.entry(var).or_default().push(GraphSync {
                    gi,
                    number: entry.index,
                    scope: entry.scope,
                    flag: json::py_str(&entry.flag),
                    bit,
                });
            }
        }
        self.graphs.insert(gi, summary);
    }

    /// Appends the index of the next graphs: each list in graph order, each
    /// count summed, each map's keys in the order they first came.
    fn merge(&mut self, part: Index) {
        fn append<K: std::hash::Hash + Eq, V>(
            into: &mut HashMap<K, Vec<V>>,
            from: HashMap<K, Vec<V>>,
        ) {
            for (key, values) in from {
                into.entry(key).or_default().extend(values);
            }
        }
        self.graphs.extend(part.graphs);
        append(&mut self.vars, part.vars);
        append(&mut self.syncs, part.syncs);
        append(&mut self.sync_vars, part.sync_vars);
        append(&mut self.entries, part.entries);
        append(&mut self.ids, part.ids);
        append(&mut self.assets, part.assets);
        append(&mut self.class_nodes, part.class_nodes);
        append(&mut self.class_values, part.class_values);
        self.edges.extend(part.edges);
        for (cls, stats) in part.fields {
            let into = self.fields.entry(cls).or_default();
            for (key, (objects, set, labels)) in stats {
                let entry = into.entry(key).or_default();
                entry.0 += objects;
                entry.1 += set;
                for (label, count) in labels {
                    *entry.2.entry(label).or_default() += count;
                }
            }
        }
        for (cls, count) in part.objects {
            *self.objects.entry(cls).or_default() += count;
        }
        for (var, writes) in part.constant_writes {
            let into = self.constant_writes.entry(var).or_default();
            for (key, count) in writes {
                *into.entry(key).or_default() += count;
            }
        }
    }

    fn add_refs(&mut self, names: &Names, gi: Int, rid: Option<Int>, refs: Refs) {
        for (kind, var, scope, path) in refs.vars {
            self.vars.entry(var).or_default().push(VarUse {
                gi,
                rid,
                kind,
                scope,
                path,
            });
        }
        for (child, path) in refs.graphs {
            if child != gi {
                self.edges.push(Edge {
                    parent: gi,
                    rid,
                    child,
                    path,
                });
            }
        }
        for (ident, holder, field, path) in refs.ids {
            self.ids.entry(ident).or_default().push(IdUse {
                gi,
                rid,
                holder,
                field,
                path,
            });
        }
        for (ident, what, cfg, place) in refs.entries {
            let cfg = cfg.unwrap_or(&J::Null);
            self.entries.entry(ident).or_default().push(EntryUse {
                what,
                gi,
                rid,
                place,
                text: render(names, cfg, 0),
                kind: value_type(names, cfg),
            });
        }
        for (ident, scope, path) in refs.syncs {
            self.syncs.entry(ident).or_default().push(NodeSync {
                gi,
                rid,
                scope,
                path,
            });
        }
        for (guid, path) in refs.assets {
            self.assets.entry(guid).or_default().push((gi, rid, path));
        }
        let mut seen = HashSet::new();
        for (number, (cls, value)) in refs.objects.into_iter().enumerate() {
            *self.objects.entry(cls.to_owned()).or_default() += 1;
            // A node's own class is not a value of it.
            if (number > 0 || rid.is_none()) && seen.insert(cls) {
                self.class_values
                    .entry(cls.to_owned())
                    .or_default()
                    .push((gi, rid));
            }
            let Some(obj) = value.as_obj() else {
                continue;
            };
            let stats = self.fields.entry(cls.to_owned()).or_default();
            for (key, item) in obj.iter() {
                if key == "$" {
                    continue;
                }
                let entry = stats.entry(key.to_owned()).or_default();
                entry.0 += 1;
                if !item.is_null() {
                    entry.1 += 1;
                    *entry.2.entry(label(names, item)).or_default() += 1;
                }
            }
            let written = obj.get("m_out_Var").and_then(dynamic);
            let constant = obj
                .val("m_value")
                .and_then(J::class)
                .and_then(constant_type);
            if let (Some((scope, var)), Some(kind)) = (written, constant) {
                *self
                    .constant_writes
                    .entry(var)
                    .or_default()
                    .entry((gi, scope, kind))
                    .or_default() += 1;
            }
        }
    }

    /// graph -> (role, owner, slot): hero bodies (initial graphs, weapon
    /// component), game modes (script, team controllers and body scripts) and
    /// the named UI graphs. Body overrides go to `entries`.
    fn attachments(&mut self, bundle: &Bundle, names: &Names) -> HashMap<Int, Vec<Attachment>> {
        let mut attach: HashMap<Int, Vec<Attachment>> = HashMap::new();
        let mut add = |graph: Int, role: &str, owner: &str, slot: String| {
            attach
                .entry(graph)
                .or_default()
                .push((role.to_owned(), owner.to_owned(), slot));
        };
        for (body, record) in &bundle.bodies {
            let hero = record
                .val("name")
                .filter(|name| name.truthy())
                .map_or_else(|| guid_text(*body), json::py_str);
            for (slot, entry) in record.at("graphs").items().iter().enumerate() {
                let Some(graph) = entry.val("graph").and_then(J::as_int) else {
                    continue;
                };
                add(graph, "body", &hero, format!("init#{}", slot + 1));
                for item in entry.at("m_1EB5A024").items() {
                    let Some(ident) = item.val("m_0D09D2D9").and_then(guid) else {
                        continue;
                    };
                    let cfg = item.at("m_value");
                    self.entries
                        .entry(ident & 0xFFFF)
                        .or_default()
                        .push(EntryUse {
                            what: "override",
                            gi: graph,
                            rid: None,
                            place: format!("{hero} init#{}", slot + 1),
                            text: render(names, cfg, 0),
                            kind: value_type(names, cfg),
                        });
                }
            }
            if let Some(manager) = record.val("manager").and_then(J::as_int) {
                add(manager, "weapon", &hero, "manager".into());
            }
            for (slot, weapon) in record.at("weapons").items().iter().enumerate() {
                if let Some(weapon) = weapon.as_int() {
                    add(weapon, "weapon", &hero, format!("weapon[{slot}]"));
                }
            }
        }
        for (guid, mode) in &names.modes {
            let owner = mode.name.clone().unwrap_or_else(|| guid_text(*guid));
            for (field, script) in mode.scripts() {
                if let Some(script) = script.and_then(|text| hex(text)) {
                    add(script, "mode", &owner, field.into());
                }
            }
            for team in &mode.teams {
                if let Some(controller) = team.controller.as_deref().and_then(hex) {
                    add(
                        controller,
                        "controller",
                        &owner,
                        format!("{} controller", team.team),
                    );
                }
                if let Some(body) = team.body.as_deref().and_then(hex) {
                    add(body, "mode", &owner, format!("{} body script", team.team));
                }
            }
        }
        for (gi, role, name) in &names.graphs {
            add(*gi, role, name, String::new());
        }
        for (gi, summary) in &self.graphs {
            if let Some(entries) = attach.get_mut(gi).filter(|_| !summary.abilities.is_empty()) {
                for (role, ..) in entries.iter_mut() {
                    if role == "body" {
                        *role = "ability".into();
                    }
                }
            }
        }
        attach
    }

    /// Each graph's roles: its own attachments, else the roles of the graphs
    /// that name it.
    fn roles(&self) -> HashMap<Int, Vec<&'static str>> {
        let mut parents: HashMap<Int, BTreeSet<Int>> = HashMap::new();
        for edge in &self.edges {
            parents.entry(edge.child).or_default().insert(edge.parent);
        }
        let mut found: HashMap<Int, BTreeSet<String>> = self
            .attach
            .iter()
            .map(|(gi, entries)| (*gi, entries.iter().map(|(role, ..)| role.clone()).collect()))
            .collect();
        let every: BTreeSet<Int> = self
            .graphs
            .keys()
            .chain(parents.keys())
            .copied()
            .filter(|gi| !self.attach.contains_key(gi))
            .collect();
        let mut changed = true;
        while changed {
            changed = false;
            for gi in &every {
                let new: BTreeSet<String> = parents
                    .get(gi)
                    .into_iter()
                    .flatten()
                    .flat_map(|parent| found.get(parent).cloned().unwrap_or_default())
                    .collect();
                if found.get(gi).cloned().unwrap_or_default() != new {
                    found.insert(*gi, new);
                    changed = true;
                }
            }
        }
        found
            .into_iter()
            .map(|(gi, value)| {
                let roles = ROLES
                    .iter()
                    .copied()
                    .filter(|role| value.contains(*role))
                    .collect();
                (gi, roles)
            })
            .collect()
    }
}

/// `int(text, 16)` of a graph index in the names catalog.
fn hex(text: &str) -> Option<Int> {
    Int::from_str_radix(text.trim(), 16).ok()
}

/// class -> (hash, parent, fields) from OWLib's STU types
/// (`TankLib/STU/Types`) for the classes of the data and their parents.
fn owlib_types(folder: &Path, wanted: &BTreeSet<&str>) -> io::Result<HashMap<String, OwlibClass>> {
    if !folder.is_dir() {
        return Ok(HashMap::new());
    }
    let mut files = Vec::new();
    collect_cs(folder, &mut files)?;
    files.sort();
    let mut every: HashMap<String, OwlibClass> = HashMap::new();
    for path in files {
        let text = fs::read_to_string(&path)?;
        for (name, class) in stu_classes(&text) {
            every.insert(name, class);
        }
    }
    let mut out: HashMap<String, OwlibClass> = HashMap::new();
    for cls in wanted {
        let mut cls = Some(cls.to_string());
        while let Some(name) = cls.take() {
            if out.contains_key(&name) {
                break;
            }
            let Some(class) = every.get(&name) else {
                break;
            };
            cls = class.1.clone();
            out.insert(name, class.clone());
        }
    }
    Ok(out)
}

/// Every `.cs` file under a folder (`Path.rglob("*.cs")`).
fn collect_cs(folder: &Path, out: &mut Vec<std::path::PathBuf>) -> io::Result<()> {
    for entry in fs::read_dir(folder)? {
        let path = entry?.path();
        if path.is_dir() {
            collect_cs(&path, out)?;
        } else if path.extension().is_some_and(|extension| extension == "cs") {
            out.push(path);
        }
    }
    Ok(())
}

/// The classes of one C# file, as Python finds them with
/// `\[STU\((0x[0-9A-F]+)(?:,\s*\d+)?\)\]\s*public class (\w+)(?:\s*:\s*(\w+))?\s*\{(.*?)\n    \}`.
fn stu_classes(text: &str) -> Vec<(String, OwlibClass)> {
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(found) = text[from..].find("[STU(0x") {
        let start = from + found;
        from = start + 1;
        let mut cursor = Cursor::new(text, start + "[STU(0x".len());
        let Some(hash) = cursor.take_while(|ch| ch.is_ascii_digit() || ('A'..='F').contains(&ch))
        else {
            continue;
        };
        let checkpoint = cursor.at;
        if !(cursor.literal(",") && {
            cursor.spaces();
            cursor.take_while(|ch| ch.is_ascii_digit()).is_some()
        }) {
            cursor.at = checkpoint;
        }
        if !cursor.literal(")]") {
            continue;
        }
        cursor.spaces();
        if !cursor.literal("public class ") {
            continue;
        }
        let Some(name) = cursor.take_while(is_word) else {
            continue;
        };
        let checkpoint = cursor.at;
        cursor.spaces();
        let parent = if cursor.literal(":") {
            cursor.spaces();
            cursor.take_while(is_word)
        } else {
            None
        };
        if parent.is_none() {
            cursor.at = checkpoint;
        }
        cursor.spaces();
        if !cursor.literal("{") {
            continue;
        }
        let Some(length) = text[cursor.at..].find("\n    }") else {
            continue;
        };
        let body = &text[cursor.at..cursor.at + length];
        let hash = format!("{hash:0>8}").to_uppercase();
        out.push((
            name.to_owned(),
            (hash, parent.map(str::to_owned), stu_fields(body)),
        ));
    }
    out
}

/// (field, type, offset) as Python finds them with
/// `\[STUField\(0x[0-9A-F]+(?:,\s*(\d+))?[^\]]*\)\][^\n]*\n\s*public ([\w<>\[\]]+) (m_\w+)`.
fn stu_fields(body: &str) -> Vec<(String, String, Option<String>)> {
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(found) = body[from..].find("[STUField(0x") {
        let start = from + found;
        from = start + 1;
        let mut cursor = Cursor::new(body, start + "[STUField(0x".len());
        if cursor
            .take_while(|ch| ch.is_ascii_digit() || ('A'..='F').contains(&ch))
            .is_none()
        {
            continue;
        }
        let checkpoint = cursor.at;
        let offset = if cursor.literal(",") {
            cursor.spaces();
            cursor.take_while(|ch| ch.is_ascii_digit())
        } else {
            None
        };
        if offset.is_none() {
            cursor.at = checkpoint;
        }
        // `[^\]]*\)\]`: up to the first `]`, which must close a `)`.
        let Some(close) = body[cursor.at..].find(']') else {
            continue;
        };
        let close = cursor.at + close;
        if close == cursor.at || !body[..close].ends_with(')') {
            continue;
        }
        cursor.at = close + 1;
        let Some(line_end) = body[cursor.at..].find('\n') else {
            continue;
        };
        cursor.at += line_end + 1;
        cursor.spaces();
        if !cursor.literal("public ") {
            continue;
        }
        let Some(kind) = cursor.take_while(|ch| is_word(ch) || matches!(ch, '<' | '>' | '[' | ']'))
        else {
            continue;
        };
        if !cursor.literal(" m_") {
            continue;
        }
        let Some(rest) = cursor.take_while(is_word) else {
            continue;
        };
        out.push((
            format!("m_{rest}"),
            kind.to_owned(),
            offset.map(str::to_owned),
        ));
    }
    out
}

fn is_word(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_'
}

struct Cursor<'a> {
    text: &'a str,
    at: usize,
}

impl<'a> Cursor<'a> {
    fn new(text: &'a str, at: usize) -> Self {
        Self { text, at }
    }

    fn take_while(&mut self, wanted: impl Fn(char) -> bool) -> Option<&'a str> {
        let rest = &self.text[self.at..];
        let length = rest.find(|ch: char| !wanted(ch)).unwrap_or(rest.len());
        if length == 0 {
            return None;
        }
        self.at += length;
        Some(&rest[..length])
    }

    fn spaces(&mut self) {
        self.take_while(char::is_whitespace);
    }

    fn literal(&mut self, wanted: &str) -> bool {
        if self.text[self.at..].starts_with(wanted) {
            self.at += wanted.len();
            true
        } else {
            false
        }
    }
}
