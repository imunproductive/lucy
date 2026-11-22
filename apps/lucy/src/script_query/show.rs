//! `graphs` and `show` ("graphs" and "show" in `tools/ssq.py`).

use std::collections::{BTreeSet, HashMap, HashSet};

use super::json::J;
use super::{
    Query,
    data::{Graph, Node, is_plug},
    index::Walker,
    json::{Int, str_repr},
    render::{CUT, alias, button_text, cut, guid_text, mode_name, render, var_name, var_text},
    views::{Links, parse_graph},
};

/// The fields of the statescript base classes (OWLib: `STUStatescriptBase`,
/// `State`, `Entry`, `Condition`, `Action` and its `STU_4C2054BF`), the same
/// in every node class: `class` does not list them.
pub const BASE_FIELDS: &[&str] = &[
    "m_BF5B22B7",
    "m_8BF03679",
    "m_A2287776",
    "m_AED90719",
    "m_2BBEEAB8",
    "m_ADEB6E05",
    "m_stateGroup",
    "m_transitionPlug",
    "m_4F8F2F3F",
    "m_0B1AA8CA",
    "m_beginPlug",
    "m_F198FD3A",
    "m_onBeginPlug",
    "m_onEndPlug",
    "m_subgraphPlug",
    "m_outPlug",
    "m_5AEA0A51",
    "m_inPlug",
    "m_truePlug",
    "m_falsePlug",
];
/// `STUStatescriptBase`'s remote sync variable lists.
const SYNC_LISTS: &[&str] = &["m_BF5B22B7", "m_8BF03679"];
/// Left out of `show`.
const BOOKKEEPING: &[&str] = &["m_2BBEEAB8", "m_ADEB6E05", "m_0B1AA8CA", "m_EE729DCB"];
/// Left out of `show` while they are 0.
const QUIET_ZERO: &[&str] = &["m_A2287776", "m_AED90719", "m_5AEA0A51"];
const DEFAULT_PLUGS: &[&str] = &["m_beginPlug", "m_inPlug"];

pub fn ability_text(query: &Query, rid: Int, st: Option<Int>, buttons: &[Int]) -> String {
    let st = st.map_or_else(|| "None".into(), |st| st.to_string());
    let buttons: Vec<String> = buttons
        .iter()
        .map(|&value| button_text(&query.names, Some(&J::Int(value))))
        .collect();
    let buttons = if buttons.is_empty() {
        "no button".into()
    } else {
        buttons.join(" ")
    };
    format!("rid{rid} st{st} {buttons}")
}

impl Query {
    pub fn graphs<'a>(
        &self,
        hero: Option<&'a str>,
        mode: Option<&'a str>,
        entity: Option<&'a str>,
    ) -> Vec<String> {
        let given = |text: Option<&'a str>| text.filter(|text| !text.is_empty());
        let (hero, mode, entity) = (given(hero), given(mode), given(entity));
        if hero.is_none() && mode.is_none() && entity.is_none() {
            let parents = self.parents_of();
            let missing: Vec<String> = self
                .bundle
                .missing
                .iter()
                .map(|gi| format!("{gi:04X}"))
                .collect();
            let missing = if missing.is_empty() {
                "-".into()
            } else {
                missing.join(" ")
            };
            let mut out = vec![
                format!(
                    "{} graphs in the data; named by them but not in the data: {missing}",
                    self.index.graphs.len()
                ),
                format!(
                    "{:6} {:16} {:>5} {:>6}  WHERE",
                    "GRAPH", "ROLE", "NODES", "STATES"
                ),
            ];
            for (&gi, summary) in &self.index.graphs {
                out.push(format!(
                    "{gi:04X}   {:16} {:5} {:6}  {}",
                    self.roles_text(gi),
                    summary.nodes,
                    summary.states,
                    self.where_text(gi, &parents)
                ));
            }
            return out;
        }
        let mut out = Vec::new();
        for text in [hero, entity].into_iter().flatten() {
            let Some((body, record)) = self.find_body(text) else {
                out.push(format!(
                    "no hero body for {} (the data has the 32 hero bodies)",
                    str_repr(text)
                ));
                continue;
            };
            let (roots, how) = body_roots(record);
            let order = self.closure(&roots);
            let missing: Vec<String> = order
                .iter()
                .filter(|(gi, ..)| !self.index.graphs.contains_key(gi))
                .map(|(gi, ..)| format!("{gi:04X}"))
                .collect();
            let missing_text = if missing.is_empty() {
                String::new()
            } else {
                format!(
                    " ({} not in the data: {})",
                    missing.len(),
                    missing.join(" ")
                )
            };
            out.push(format!(
                "{}: body {}, {} graphs{missing_text}",
                super::json::py_str(record.at("name")),
                guid_text(body),
                order.len()
            ));
            out.extend(self.graph_rows(&order, &how));
            out.push(String::new());
        }
        if let Some(text) = mode {
            let (guid, mode) = match self.find_mode(text) {
                Ok(Some(found)) => found,
                Ok(None) => {
                    let mut guids: Vec<Int> =
                        self.names.modes.iter().map(|(guid, _)| *guid).collect();
                    guids.sort();
                    let known: Vec<String> = guids
                        .iter()
                        .map(|guid| mode_name(&self.names, *guid))
                        .collect();
                    out.push(format!(
                        "no game mode {}; modes: {}",
                        str_repr(text),
                        known.join(", ")
                    ));
                    return out;
                }
                Err(error) => {
                    out.push(error);
                    return out;
                }
            };
            let mut roots = Vec::new();
            let mut how: Vec<(Int, Vec<String>)> = Vec::new();
            let mut add = |graph: Int, slot: String| {
                roots.push(graph);
                match how.iter_mut().find(|(known, _)| *known == graph) {
                    Some((_, slots)) => slots.push(slot),
                    None => how.push((graph, vec![slot])),
                }
            };
            for (field, script) in mode.scripts() {
                if let Some(graph) = script.and_then(|text| Int::from_str_radix(text, 16).ok()) {
                    add(graph, field.into());
                }
            }
            for team in &mode.teams {
                for (graph, slot) in [
                    (&team.controller, "controller"),
                    (&team.body, "body script"),
                ] {
                    if let Some(graph) = graph
                        .as_deref()
                        .and_then(|text| Int::from_str_radix(text, 16).ok())
                    {
                        add(graph, format!("{} {slot}", team.team));
                    }
                }
            }
            let order = self.closure(&roots);
            let missing = order
                .iter()
                .filter(|(gi, ..)| !self.index.graphs.contains_key(gi))
                .count();
            let missing = if missing > 0 {
                format!(", {missing} not in the data")
            } else {
                String::new()
            };
            out.push(format!(
                "{} ({}): {} graphs{missing}",
                mode_name(&self.names, guid),
                guid_text(guid),
                order.len()
            ));
            let how: HashMap<Int, String> = how
                .into_iter()
                .map(|(graph, slots)| (graph, slots.join(", ")))
                .collect();
            out.extend(self.graph_rows(&order, &how));
        }
        out
    }

    pub fn show(
        &self,
        text: &str,
        state: Option<i64>,
        depth: Option<i64>,
        full: bool,
    ) -> Vec<String> {
        let Some(graph) = parse_graph(text).and_then(|gi| self.bundle.graph(gi)) else {
            return vec![format!("graph {} is not in the data", str_repr(text))];
        };
        let mut shower = Shower {
            query: self,
            graph,
            depth,
            full,
            walker: Walker::new(&self.names),
            done: HashSet::new(),
            out: Vec::new(),
        };
        shower.head();
        if let Some(state) = state {
            let Some(node) = graph.state(state) else {
                let gi = graph.index;
                shower.out.push(format!(
                    "{gi:04X} has no state {state} (m_states has {})",
                    graph.states.len()
                ));
                return shower.out;
            };
            shower.out.push(String::new());
            shower.node(node, 0, "");
            return shower.out;
        }
        for node in graph.listed(&graph.entries) {
            shower.out.push(String::new());
            shower.node(node, 0, "entry ");
        }
        let rest: Vec<&Node> = graph
            .nodes
            .iter()
            .filter(|node| !shower.done.contains(&node.rid))
            .collect();
        if !rest.is_empty() {
            shower.out.push(String::new());
            shower
                .out
                .push(format!("not reached from an entry ({}):", rest.len()));
            for node in rest {
                if !shower.done.contains(&node.rid) {
                    shower.node(node, 0, "");
                }
            }
        }
        shower.out
    }
}

/// A hero body's initial graphs, manager and weapon scripts, and how each is
/// attached.
pub fn body_roots(record: &J) -> (Vec<Int>, HashMap<Int, String>) {
    let mut roots = Vec::new();
    let mut how = HashMap::new();
    for (slot, entry) in record.at("graphs").items().iter().enumerate() {
        if let Some(graph) = entry.val("graph").and_then(J::as_int) {
            roots.push(graph);
            how.entry(graph).or_insert(format!("init#{}", slot + 1));
        }
    }
    if let Some(manager) = record.val("manager").and_then(J::as_int) {
        roots.push(manager);
        how.entry(manager).or_insert("manager".into());
    }
    for (slot, weapon) in record.at("weapons").items().iter().enumerate() {
        if let Some(weapon) = weapon.as_int() {
            roots.push(weapon);
            how.entry(weapon).or_insert(format!("weapon[{slot}]"));
        }
    }
    (roots, how)
}

struct Shower<'a> {
    query: &'a Query,
    graph: &'a Graph,
    depth: Option<i64>,
    /// Field values whole instead of cut at `CUT` characters.
    full: bool,
    walker: Walker<'a>,
    done: HashSet<Int>,
    out: Vec<String>,
}

impl Shower<'_> {
    fn label(&self, rid: Int) -> String {
        self.query.node_label(self.graph.index, Some(rid))
    }

    fn head(&mut self) {
        let query = self.query;
        let gi = self.graph.index;
        let summary = &query.index.graphs[&gi];
        let parents: Links = query.parents_of();
        self.out.push(format!(
            "{gi:04X}.01B  {}: {}",
            query.roles_text(gi),
            query.where_text(gi, &parents)
        ));
        let mut line = format!(
            "{} nodes ({} networked, {} client-only, {} server-only), {} states, {} entries, {} remote sync nodes",
            summary.nodes,
            summary.networked,
            summary.client_only,
            summary.server_only,
            summary.states,
            summary.entries,
            summary.remote
        );
        if summary.unknown > 0 {
            line.push_str(&format!(
                ", {} nodes DataTool could not read",
                summary.unknown
            ));
        }
        self.out.push(line);
        let presence: Vec<String> = summary
            .presence
            .iter()
            .map(|var| var_name(&query.names, *var, None))
            .collect();
        let presence_text = if presence.is_empty() {
            "-".into()
        } else {
            presence.join(", ")
        };
        self.out.push(format!(
            "frames: {} owner states, {} remote states; {} sync vars, {} with a presence bit: {presence_text}",
            summary.owner_states,
            summary.remote_states,
            summary.sync_vars,
            presence.len()
        ));
        let [nodes, states, remote, vars] = &summary.bits;
        self.out.push(format!(
            "bits: nodes {nodes}, states {states}, remote {remote}, sync vars {vars}; m_predictionBehavior {}",
            summary.prediction
        ));
        let named_by: BTreeSet<(Int, Int)> = parents
            .get(&gi)
            .into_iter()
            .flatten()
            .map(|(parent, rid, _)| (*parent, rid.unwrap_or(-1)))
            .collect();
        let text: Vec<String> = named_by
            .iter()
            .map(|&(parent, rid)| {
                let rid = (rid >= 0).then_some(rid);
                format!("{parent:04X} {}", query.node_label(parent, rid))
            })
            .collect();
        let text = if text.is_empty() {
            "-".into()
        } else {
            text.join(", ")
        };
        self.out.push(format!("named by: {text}"));
        let children: BTreeSet<Int> = query
            .children_of()
            .get(&gi)
            .into_iter()
            .flatten()
            .map(|(child, ..)| *child)
            .collect();
        let children: Vec<String> = children
            .iter()
            .map(|child| format!("{child:04X}"))
            .collect();
        let children = if children.is_empty() {
            "-".into()
        } else {
            children.join(" ")
        };
        self.out.push(format!("names: {children}"));
        let mut counts: Vec<(&String, &usize)> = summary.classes.iter().collect();
        counts.sort_by(|a, b| {
            b.1.cmp(a.1)
                .then_with(|| alias(&query.names, a.0).cmp(alias(&query.names, b.0)))
        });
        let counts: Vec<String> = counts
            .iter()
            .map(|(cls, count)| format!("{} x{count}", alias(&query.names, cls)))
            .collect();
        self.out.push(format!("classes: {}", counts.join(", ")));
        if !summary.abilities.is_empty() {
            let abilities: Vec<String> = summary
                .abilities
                .iter()
                .map(|(rid, st, buttons)| ability_text(query, *rid, *st, buttons))
                .collect();
            self.out
                .push(format!("abilities: {}", abilities.join("; ")));
        }
    }

    fn node(&mut self, node: &Node, level: i64, how: &str) {
        let names = &self.query.names;
        let pad = "  ".repeat(level as usize);
        let mut text = format!("{pad}{how}{}", self.label(node.rid));
        if node.remote >= 0 {
            text.push_str(&format!(" (remote {})", node.remote));
        }
        if !self.done.insert(node.rid) {
            self.out.push(format!("{text} (above)"));
            return;
        }
        self.out.push(text);
        let refs = self.walker.run(&node.fields, &node.cls);
        let mut fields = Vec::new();
        if let Some(obj) = node.fields.as_obj() {
            for (key, value) in obj.iter() {
                if value.is_null()
                    || is_plug(value)
                    || SYNC_LISTS.contains(&key)
                    || BOOKKEEPING.contains(&key)
                {
                    continue;
                }
                let plugs = value
                    .as_arr()
                    .is_some_and(|items| !items.is_empty() && items.iter().all(is_plug));
                if !plugs && !(QUIET_ZERO.contains(&key) && !value.truthy()) {
                    let value = render(names, value, 0);
                    let value = if self.full { value } else { cut(&value, CUT) };
                    fields.push(format!("{key}={value}"));
                }
            }
        }
        if !fields.is_empty() {
            self.out.push(format!("{pad}    {}", fields.join(" | ")));
        }
        let mut uses: Vec<(&str, Vec<String>)> = Vec::new();
        for (kind, var, scope, _) in &refs.vars {
            let text = var_text(*var, Some(*scope));
            match uses.iter_mut().find(|(known, _)| known == kind) {
                Some((_, found)) => {
                    if !found.contains(&text) {
                        found.push(text);
                    }
                }
                None => uses.push((kind, vec![text])),
            }
        }
        if !uses.is_empty() {
            uses.sort_by(|a, b| a.0.cmp(b.0));
            let parts: Vec<String> = uses
                .iter()
                .map(|(kind, found)| format!("{kind} {}", found.join(" ")))
                .collect();
            self.out
                .push(format!("{pad}    vars: {}", parts.join("; ")));
        }
        let syncs: BTreeSet<String> = refs
            .syncs
            .iter()
            .map(|(ident, scope, _)| var_text(*ident, *scope))
            .collect();
        if !syncs.is_empty() {
            let syncs: Vec<&str> = syncs.iter().map(String::as_str).collect();
            self.out
                .push(format!("{pad}    remote sync vars: {}", syncs.join(" ")));
        }
        for (child, path) in &refs.graphs {
            if *child != self.graph.index {
                let missing = if self.query.index.graphs.contains_key(child) {
                    ""
                } else {
                    " (not in the data)"
                };
                self.out
                    .push(format!("{pad}    graph {child:04X}{missing} at {path}"));
            }
        }
        for (path, plug) in node.plugs() {
            for link in plug.at("links").items() {
                let target = link.items().first().and_then(J::as_int);
                let plug_path = link
                    .items()
                    .get(1)
                    .map(super::json::py_str)
                    .unwrap_or_default();
                let end = target.and_then(|target| self.graph.node(target));
                let into = if DEFAULT_PLUGS.contains(&plug_path.as_str()) {
                    String::new()
                } else {
                    format!("({plug_path}) ")
                };
                match end {
                    None => self.out.push(format!(
                        "{pad}  {path} -> {into}a node DataTool could not read"
                    )),
                    Some(end) if self.depth.is_some_and(|depth| level >= depth) => {
                        let label = self.label(end.rid);
                        self.out.push(format!("{pad}  {path} -> {into}{label} ..."));
                    }
                    Some(end) => self.node(end, level + 1, &format!("{path} -> {into}")),
                }
            }
        }
    }
}
