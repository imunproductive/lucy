//! Views the queries share ("shared views" in `tools/ssq.py`): node labels,
//! where a graph is attached, the graphs a root names, and the parsers of
//! graph, variable, GUID, hero and mode arguments.

use std::collections::{HashMap, HashSet, VecDeque};

use super::{
    Query,
    data::{GRAPH_TYPE, Mode, Names, find_hero},
    index::Attachment,
    json::{Int, J, str_repr},
    render::{IDENT_TYPE, MODE_TYPE, alias, guid_text, type_bits},
};

/// (parent, rid, path) of each graph that names a graph.
pub type Links = HashMap<Int, Vec<(Int, Option<Int>, String)>>;

impl Query {
    pub fn parents_of(&self) -> Links {
        let mut found: Links = HashMap::new();
        for edge in &self.index.edges {
            found
                .entry(edge.child)
                .or_default()
                .push((edge.parent, edge.rid, edge.path.clone()));
        }
        found
    }

    pub fn children_of(&self) -> Links {
        let mut found: Links = HashMap::new();
        for edge in &self.index.edges {
            found
                .entry(edge.parent)
                .or_default()
                .push((edge.child, edge.rid, edge.path.clone()));
        }
        found
    }

    /// `rid12 st7` for a node of a graph.
    pub fn node_index(&self, gi: Int, rid: Int) -> String {
        let state = self.index.graphs[&gi]
            .nodes_info
            .get(&rid)
            .and_then(|info| info.state);
        match state {
            Some(state) => format!("rid{rid} st{state}"),
            None => format!("rid{rid}"),
        }
    }

    /// `rid12 st7 Alias [client-only]` for a node of a graph, `graph field`
    /// for no node.
    pub fn node_label(&self, gi: Int, rid: Option<Int>) -> String {
        let Some(rid) = rid else {
            return "graph field".into();
        };
        let Some(info) = self.index.graphs[&gi].nodes_info.get(&rid) else {
            return format!("rid{rid}");
        };
        let flags: Vec<&str> = [("client", info.client_only), ("server", info.server_only)]
            .into_iter()
            .filter(|(_, on)| *on)
            .map(|(flag, _)| flag)
            .collect();
        let mut text = format!(
            "{} {}",
            self.node_index(gi, rid),
            alias(&self.names, &info.cls)
        );
        if !flags.is_empty() {
            text.push_str(&format!(" [{}-only]", flags.join("+")));
        }
        text
    }

    pub fn roles_text(&self, gi: Int) -> String {
        match self.index.roles.get(&gi) {
            Some(roles) if !roles.is_empty() => roles.join("+"),
            _ => "-".into(),
        }
    }

    pub fn where_text(&self, gi: Int, parents: &Links) -> String {
        if let Some(entries) = self.index.attach.get(&gi) {
            return attach_text(entries);
        }
        let mut found: Vec<(Int, Option<Int>)> = Vec::new();
        for (parent, rid, _) in parents.get(&gi).into_iter().flatten() {
            if !found.contains(&(*parent, *rid)) {
                found.push((*parent, *rid));
            }
        }
        if found.is_empty() {
            return "-".into();
        }
        let text: Vec<String> = found
            .iter()
            .take(3)
            .map(|(parent, rid)| format!("{parent:04X} {}", self.node_label(*parent, *rid)))
            .collect();
        let more = if found.len() > 3 {
            format!(" (+{} more)", found.len() - 3)
        } else {
            String::new()
        };
        format!("from {}{more}", text.join(", "))
    }

    /// (graph, parent, rid) for the roots and every graph they name, breadth
    /// first.
    pub fn closure(&self, roots: &[Int]) -> Vec<(Int, Option<Int>, Option<Int>)> {
        let children = self.children_of();
        let mut found = HashSet::new();
        let mut order = Vec::new();
        let mut queue: VecDeque<(Int, Option<Int>, Option<Int>)> =
            roots.iter().map(|&root| (root, None, None)).collect();
        while let Some((gi, parent, rid)) = queue.pop_front() {
            if !found.insert(gi) {
                continue;
            }
            order.push((gi, parent, rid));
            for (child, child_rid, _) in children.get(&gi).into_iter().flatten() {
                if !found.contains(child) {
                    queue.push_back((*child, Some(gi), *child_rid));
                }
            }
        }
        order
    }

    pub fn graph_rows(
        &self,
        order: &[(Int, Option<Int>, Option<Int>)],
        how: &HashMap<Int, String>,
    ) -> Vec<String> {
        let mut rows = vec![format!(
            "{:6} {:16} {:>5} {:>6}  HOW",
            "GRAPH", "ROLE", "NODES", "STATES"
        )];
        for &(gi, parent, rid) in order {
            let text = match (how.get(&gi).filter(|text| !text.is_empty()), parent) {
                (Some(text), _) => text.clone(),
                (None, Some(parent)) => {
                    format!("from {parent:04X} {}", self.node_label(parent, rid))
                }
                (None, None) => "-".into(),
            };
            let summary = self.index.graphs.get(&gi);
            let size = match summary {
                Some(summary) => format!("{:5} {:6}", summary.nodes, summary.states),
                None => format!("{:>5} {:>6}", "-", "-"),
            };
            let missing = if summary.is_some() {
                ""
            } else {
                " (not in the data)"
            };
            rows.push(format!(
                "{gi:04X}   {:16} {size}  {text}{missing}",
                self.roles_text(gi)
            ));
        }
        rows
    }

    pub fn parse_var(&self, text: &str) -> Option<Int> {
        parse_var(&self.names, text)
    }

    /// A hero body by hero name, hero GUID or body GUID / index.
    pub fn find_body(&self, text: &str) -> Option<(Int, &J)> {
        let bodies = &self.bundle.bodies;
        if let Some(hero) = find_hero(&self.heroes, text) {
            if let Some((body, record)) = bodies.iter().find(|(body, _)| *body == hero.body) {
                return Some((*body, record));
            }
        }
        let number = parse_hex(text)?;
        bodies
            .iter()
            .find(|(body, record)| {
                let hero = record
                    .val("hero")
                    .filter(|hero| hero.truthy())
                    .and_then(J::as_str)
                    .and_then(parse_hex);
                number == *body
                    || Some(number) == hero
                    || (number >> 48 == 0 && number == body & 0xFFFF_FFFF_FFFF)
            })
            .map(|(body, record)| (*body, record))
    }

    /// A game mode by name (whole or the start) or GUID / index; an error
    /// when a name fits several.
    pub fn find_mode(&self, text: &str) -> Result<Option<(Int, &Mode)>, String> {
        let modes = &self.names.modes;
        let wanted = text.trim().to_lowercase();
        let name = |mode: &Mode| mode.name.as_deref().unwrap_or("").to_lowercase();
        let mut found: Vec<&(Int, Mode)> = modes
            .iter()
            .filter(|(_, mode)| name(mode) == wanted)
            .collect();
        if found.is_empty() {
            found = modes
                .iter()
                .filter(|(_, mode)| name(mode).starts_with(&wanted))
                .collect();
        }
        if found.len() > 1 {
            let guids: Vec<String> = found.iter().map(|(guid, _)| guid_text(*guid)).collect();
            return Err(format!(
                "{} is several modes: {}",
                str_repr(text),
                guids.join(", ")
            ));
        }
        if let Some((guid, mode)) = found.first() {
            return Ok(Some((*guid, mode)));
        }
        let Some(number) = parse_hex(text) else {
            return Ok(None);
        };
        let guid = if number >> 48 != 0 {
            number
        } else {
            (MODE_TYPE << 48) | number
        };
        Ok(self.names.mode(guid).map(|mode| (guid, mode)))
    }
}

/// Where a graph is attached, short: slots shared by many heroes or modes
/// are counted.
pub fn attach_text(entries: &[Attachment]) -> String {
    let mut by_slot: Vec<((&str, &str), Vec<&str>)> = Vec::new();
    for (role, owner, slot) in entries {
        let key = (role.as_str(), slot.as_str());
        match by_slot.iter_mut().find(|(known, _)| *known == key) {
            Some((_, owners)) => owners.push(owner),
            None => by_slot.push((key, vec![owner])),
        }
    }
    let parts: Vec<String> = by_slot
        .into_iter()
        .map(|((role, slot), owners)| {
            let mut unique: Vec<&str> = Vec::new();
            for owner in owners {
                if !unique.contains(&owner) {
                    unique.push(owner);
                }
            }
            if slot.is_empty() {
                unique.join(", ")
            } else if unique.len() <= 2 {
                format!("{} {slot}", unique.join(", "))
            } else {
                let kind = if matches!(role, "body" | "ability" | "weapon") {
                    "heroes"
                } else {
                    "modes"
                };
                format!("{slot} of {} {kind}", unique.len())
            }
        })
        .collect();
    parts.join("; ")
}

/// A variable: `699`, `#699`, `#7644e`, `v699`, `0x2BB`, `02BB.01C`,
/// `0x0D800000000002BB` or a known name.
pub fn parse_var(names: &Names, text: &str) -> Option<Int> {
    let raw = text.trim();
    if let Some(number) = var_number(raw) {
        return Some(number);
    }
    if raw.to_lowercase().starts_with("0x") || raw.contains('.') {
        if let Some(number) = parse_hex(raw) {
            return Some(if number >> 48 == IDENT_TYPE {
                number & 0xFFFF
            } else {
                number
            });
        }
    }
    let wanted = raw.to_lowercase();
    names
        .vars
        .iter()
        .find(|(_, entry)| entry.name.to_lowercase() == wanted)
        .map(|(var, _)| *var)
}

/// `699`, `#699`, `v699`, `#7644e`: `[#v]?(\d+)e?` in any case.
fn var_number(raw: &str) -> Option<Int> {
    let body = raw.strip_prefix(['#', 'v', 'V']).unwrap_or(raw);
    let body = body.strip_suffix(['e', 'E']).unwrap_or(body);
    if body.is_empty() || !body.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    body.parse().ok()
}

/// Whether an argument names entity scope only (`[#v]?\d+e`).
pub fn entity_only(text: &str) -> bool {
    let raw = text.trim();
    raw.ends_with(['e', 'E']) && var_number(raw).is_some()
}

/// `0255`, `0x255`, `0x0580000000000255` or `0255.01B` as a number.
pub fn parse_hex(text: &str) -> Option<Int> {
    let raw = text.trim();
    let hex = |text: &str| -> Option<Int> {
        let digits = match text.split_once('.') {
            Some((digits, kind))
                if kind.len() == 3 && kind.bytes().all(|byte| byte.is_ascii_hexdigit()) =>
            {
                digits
            }
            Some(_) => return None,
            None => text,
        };
        if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return None;
        }
        Int::from_str_radix(digits, 16).ok()
    };
    raw.strip_prefix("0x").and_then(hex).or_else(|| hex(raw))
}

/// A full GUID from `0x0300000000000186` or `0186.00D`; None for a bare
/// index.
pub fn parse_guid(text: &str) -> Option<Int> {
    let raw = text.trim();
    let body = raw.strip_prefix("0x").unwrap_or(raw);
    if let Some((index, kind)) = body.split_once('.') {
        let hex =
            |text: &str| !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_hexdigit());
        if hex(index) && kind.len() == 3 && hex(kind) {
            let index = Int::from_str_radix(index, 16).ok()?;
            let kind = Int::from_str_radix(kind, 16).ok()?;
            return Some((type_bits(kind) << 48) | index);
        }
    }
    parse_hex(text).filter(|number| number >> 48 != 0)
}

pub fn parse_graph(text: &str) -> Option<Int> {
    let number = parse_hex(text)?;
    Some(if number >> 48 == GRAPH_TYPE {
        number & 0xFFFF_FFFF_FFFF
    } else {
        number
    })
}
