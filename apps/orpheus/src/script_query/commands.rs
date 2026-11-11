//! `var`, `class`, `find` and `hero` (their sections in `tools/ssq.py`).

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use indexmap::IndexMap;

use super::{
    Query,
    data::{ENTITY, field_at},
    json::{self, Int, J, str_repr},
    render::{
        IDENT_TYPE, alias, class_hash, cut, guid_text, mode_name, plural, render, scope_text,
        var_name, var_text,
    },
    show::{BASE_FIELDS, ability_text, body_roots},
    views::{attach_text, entity_only, parse_graph, parse_guid, parse_hex},
};

/// `Counter.most_common()`: by count, ties in first-counted order.
fn most_common<K: Clone>(counter: &IndexMap<K, usize>) -> Vec<(K, usize)> {
    let mut items: Vec<(K, usize)> = counter
        .iter()
        .map(|(key, count)| (key.clone(), *count))
        .collect();
    items.sort_by_key(|item| std::cmp::Reverse(item.1));
    items
}

impl Query {
    /// One use of a variable: the node, the field path, and the value
    /// written (an `m_out_Var`'s `m_value`) or the expression or object it
    /// sits in.
    fn use_line(&self, gi: Int, rid: Option<Int>, path: &str) -> String {
        let node = rid.and_then(|rid| self.bundle.graph(gi)?.by_rid(rid));
        let mut shown = String::new();
        if let Some(node) = node {
            let written = path
                .strip_suffix("m_out_Var")
                .and_then(|head| field_at(&node.fields, &format!("{head}m_value")));
            let names = &self.names;
            if let Some(written) = written {
                shown = format!(" = {}", cut(&render(names, written, 0), 100));
            } else if let Some((head, _)) = path.split_once(".m_expression.") {
                let holder = field_at(&node.fields, head).unwrap_or(&J::Null);
                shown = format!(": {}", cut(&render(names, holder, 0), 100));
            } else if let Some((head, _)) = path.rsplit_once('.') {
                if let Some(holder) = field_at(&node.fields, head) {
                    shown = format!(": {}", cut(&render(names, holder, 0), 100));
                }
            }
        }
        format!("  {gi:04X} {}  {path}{shown}", self.node_label(gi, rid))
    }

    pub fn var(&self, text: &str, graph: Option<&str>) -> Vec<String> {
        let Some(var) = self.parse_var(text) else {
            return vec![format!(
                "{} is not a variable id or a known variable name",
                str_repr(text)
            )];
        };
        let only = graph
            .filter(|graph| !graph.is_empty())
            .and_then(parse_graph);
        let entity_only = entity_only(text);
        let keep = |gi: Int, scope: Option<Int>| {
            only.is_none_or(|only| gi == only)
                && (!entity_only || scope.is_none() || scope == Some(ENTITY))
        };
        let uses: Vec<_> = self
            .index
            .vars
            .get(&var)
            .into_iter()
            .flatten()
            .filter(|found| keep(found.gi, Some(found.scope)))
            .collect();
        let entry = self.names.var(var);
        let mut head = format!(
            "{}  identifier {}",
            var_text(var, None),
            guid_text((IDENT_TYPE << 48) | var)
        );
        match entry {
            Some(entry) => head.push_str(&format!("  name {} ({})", entry.name, entry.from)),
            None => head.push_str("  no name known"),
        }
        if let Some(only) = only {
            head.push_str(&format!("  (only {only:04X})"));
        }
        if entity_only {
            head.push_str("  (entity scope only)");
        }
        let mut out = vec![head];
        if !uses.is_empty() {
            let mut scopes: BTreeMap<Int, usize> = BTreeMap::new();
            for found in &uses {
                *scopes.entry(found.scope).or_default() += 1;
            }
            let counts: Vec<String> = scopes
                .iter()
                .map(|(scope, count)| format!("{} x{count}", scope_text(Some(*scope))))
                .collect();
            out.push(format!(
                "scope: {} (an instance variable is one per instance: the same id in two graphs is two variables)",
                counts.join(", ")
            ));
        }
        let defaults: Vec<_> = self
            .index
            .entries
            .get(&var)
            .into_iter()
            .flatten()
            .filter(|item| keep(item.gi, None))
            .collect();
        let mut types: IndexMap<String, usize> = IndexMap::new();
        for item in &defaults {
            if let Some(kind) = item.kind.as_ref().filter(|kind| !kind.is_empty()) {
                *types.entry(kind.clone()).or_default() += 1;
            }
        }
        for (&(gi, scope, kind), count) in
            self.index.constant_writes.get(&var).into_iter().flatten()
        {
            if keep(gi, Some(scope)) {
                *types.entry(kind.to_owned()).or_default() += count;
            }
        }
        let found_types: Vec<String> = most_common(&types)
            .iter()
            .map(|(kind, count)| format!("{kind} x{count}"))
            .collect();
        let found_types = if found_types.is_empty() {
            "unknown".into()
        } else {
            found_types.join(", ")
        };
        out.push(format!(
            "type: {found_types} (from defaults and constant writes)"
        ));
        out.push(if defaults.is_empty() {
            "defaults: none".into()
        } else {
            format!("defaults and set values ({}):", defaults.len())
        });
        for item in &defaults {
            let place = if item.what == "override" {
                item.place.clone()
            } else {
                format!("{} {}", self.node_label(item.gi, item.rid), item.place)
            };
            let kind = item
                .kind
                .as_ref()
                .filter(|kind| !kind.is_empty())
                .map_or_else(String::new, |kind| format!(" ({kind})"));
            out.push(format!(
                "  {:8} {:04X} {place}: {}{kind}",
                item.what, item.gi, item.text
            ));
        }
        for kind in ["write", "read", "parent", "id", "param", "list", "unused"] {
            let mut found: Vec<_> = uses.iter().filter(|found| found.kind == kind).collect();
            if found.is_empty() {
                continue;
            }
            found.sort_by(|a, b| {
                (a.gi, a.rid.unwrap_or(-1), &a.path).cmp(&(b.gi, b.rid.unwrap_or(-1), &b.path))
            });
            let graphs: HashSet<Int> = found.iter().map(|found| found.gi).collect();
            out.push(format!(
                "{kind} ({} in {}):",
                found.len(),
                plural(graphs.len(), "graph")
            ));
            for found in found {
                let entity = if found.scope == ENTITY {
                    " [entity]"
                } else {
                    ""
                };
                out.push(format!(
                    "{}{entity}",
                    self.use_line(found.gi, found.rid, &found.path)
                ));
            }
        }
        let sync: Vec<_> = self
            .index
            .sync_vars
            .get(&var)
            .into_iter()
            .flatten()
            .filter(|item| keep(item.gi, item.scope))
            .collect();
        out.push(if sync.is_empty() {
            "graph sync vars: none".into()
        } else {
            format!("graph sync vars (m_syncVars, {}):", sync.len())
        });
        for item in &sync {
            let place = match item.bit {
                Some(bit) => format!("presence bit {bit}"),
                None => "no presence bit".into(),
            };
            out.push(format!(
                "  {:04X} m_syncVars[{}]: {}, {place}, m_AC9480C7={}",
                item.gi,
                item.number,
                scope_text(item.scope),
                item.flag
            ));
        }
        let remote: Vec<_> = self
            .index
            .syncs
            .get(&var)
            .into_iter()
            .flatten()
            .filter(|item| keep(item.gi, item.scope))
            .collect();
        out.push(if remote.is_empty() {
            "node remote sync vars: none".into()
        } else {
            format!("node remote sync vars ({}):", remote.len())
        });
        for item in &remote {
            out.push(format!(
                "  {:04X} {}  {} ({})",
                item.gi,
                self.node_label(item.gi, item.rid),
                item.path,
                scope_text(item.scope)
            ));
        }
        let ids: Vec<_> = self
            .index
            .ids
            .get(&var)
            .into_iter()
            .flatten()
            .filter(|item| keep(item.gi, None))
            .collect();
        if !ids.is_empty() {
            let mut kinds: IndexMap<String, usize> = IndexMap::new();
            for item in &ids {
                let name = format!("{}.{}", alias(&self.names, &item.holder), item.field);
                *kinds.entry(name).or_default() += 1;
            }
            let kinds: Vec<String> = most_common(&kinds)
                .iter()
                .map(|(name, count)| format!("{name} x{count}"))
                .collect();
            out.push(format!(
                "the identifier as a value ({}): {}",
                ids.len(),
                kinds.join(", ")
            ));
            for item in &ids {
                out.push(format!(
                    "  {:04X} {}  {}",
                    item.gi,
                    self.node_label(item.gi, item.rid),
                    item.path
                ));
            }
        }
        if uses.is_empty()
            && defaults.is_empty()
            && sync.is_empty()
            && remote.is_empty()
            && ids.is_empty()
        {
            out.push("not used in the data".into());
        }
        out
    }

    /// Every class of the data, sorted.
    fn known_classes(&self) -> Vec<&str> {
        let known: BTreeSet<&str> = self
            .index
            .objects
            .keys()
            .chain(self.index.class_nodes.keys())
            .map(String::as_str)
            .collect();
        known.into_iter().collect()
    }

    /// A class's hash: the names catalog's, else OWLib's.
    fn hash_of<'a>(&'a self, cls: &'a str) -> Option<&'a str> {
        class_hash(&self.names, cls)
            .or_else(|| self.index.owlib.get(cls).map(|owlib| owlib.0.as_str()))
            .filter(|hash| !hash.is_empty())
    }

    fn find_class(&self, text: &str) -> Option<&str> {
        let known = self.known_classes();
        let raw = text.trim();
        if let Some(cls) = known.iter().find(|cls| **cls == raw) {
            return Some(cls);
        }
        let wanted = raw.to_lowercase();
        for cls in &known {
            let client = self
                .names
                .class(cls)
                .and_then(|info| info.client.as_deref())
                .unwrap_or("");
            if wanted == alias(&self.names, cls).to_lowercase() || wanted == client.to_lowercase() {
                return Some(cls);
            }
        }
        let upper = raw.to_uppercase();
        let hex = upper.strip_prefix("STU_").unwrap_or(&upper);
        let hex = hex.strip_prefix("0X").unwrap_or(hex);
        if (1..=8).contains(&hex.len())
            && hex
                .bytes()
                .all(|byte| matches!(byte, b'0'..=b'9' | b'A'..=b'F'))
        {
            let hex = format!("{hex:0>8}");
            return known
                .into_iter()
                .find(|cls| self.hash_of(cls) == Some(hex.as_str()));
        }
        None
    }

    pub fn class(&self, text: &str) -> Vec<String> {
        let Some(cls) = self.find_class(text) else {
            return vec![format!(
                "no class {} in the data (try: orpheus script query find {text})",
                str_repr(text)
            )];
        };
        let names = &self.names;
        let info = names.class(cls);
        let owlib = self.index.owlib.get(cls);
        let hashed = self.hash_of(cls);
        let mut head = format!("{cls}  alias {}", alias(names, cls));
        if let Some(from) = info.and_then(|info| info.from.as_deref()) {
            head.push_str(&format!(" (from {from})"));
        }
        if let Some(hash) = hashed {
            head.push_str(&format!("  hash {hash}"));
        }
        if let Some(client) = info.and_then(|info| info.client.as_deref()) {
            head.push_str(&format!("  client name {client}"));
        }
        let mut out = vec![head];
        if let Some(owlib) = owlib {
            let mut chain = Vec::new();
            let mut parent = owlib.1.as_deref();
            while let Some(name) = parent {
                chain.push(name);
                parent = self
                    .index
                    .owlib
                    .get(name)
                    .and_then(|owlib| owlib.1.as_deref());
            }
            let chain = if chain.is_empty() {
                "-".into()
            } else {
                chain.join(" > ")
            };
            out.push(format!("parents (OWLib): {chain}"));
        }
        let graphs = &self.index.graphs;
        let node_uses = self
            .index
            .class_nodes
            .get(cls)
            .map_or(&[][..], Vec::as_slice);
        if let Some(&(gi, rid)) = node_uses.first() {
            let is_state = graphs[&gi].nodes_info[&rid].state.is_some();
            out.push(format!("VM: {}", vm_text(cls, is_state)));
            let mut by_graph: Vec<(Int, Vec<Int>)> = Vec::new();
            for &(gi, rid) in node_uses {
                match by_graph.iter_mut().find(|(known, _)| *known == gi) {
                    Some((_, rids)) => rids.push(rid),
                    None => by_graph.push((gi, vec![rid])),
                }
            }
            let networked = node_uses
                .iter()
                .filter(|(gi, rid)| {
                    let info = &graphs[gi].nodes_info[rid];
                    !info.client_only && !info.server_only
                })
                .count();
            out.push(format!(
                "nodes: {} in {} ({networked} networked)",
                node_uses.len(),
                plural(by_graph.len(), "graph")
            ));
            for (gi, rids) in &by_graph {
                let rids: Vec<String> = rids.iter().map(|rid| self.node_index(*gi, *rid)).collect();
                out.push(format!(
                    "  {gi:04X} ({}): {}",
                    self.roles_text(*gi),
                    rids.join(", ")
                ));
            }
        }
        let value_uses = self
            .index
            .class_values
            .get(cls)
            .map_or(&[][..], Vec::as_slice);
        if !value_uses.is_empty() {
            let mut by_graph: Vec<(Int, Vec<Option<Int>>)> = Vec::new();
            for &(gi, rid) in value_uses {
                match by_graph.iter_mut().find(|(known, _)| *known == gi) {
                    Some((_, rids)) => rids.push(rid),
                    None => by_graph.push((gi, vec![rid])),
                }
            }
            let fields = value_uses.iter().filter(|(_, rid)| rid.is_none()).count();
            let mut places = Vec::new();
            if value_uses.len() > fields {
                places.push(plural(value_uses.len() - fields, "node"));
            }
            if fields > 0 {
                places.push(plural(fields, "graph field"));
            }
            out.push(format!(
                "as a value: in {} of {}",
                places.join(" and "),
                plural(by_graph.len(), "graph")
            ));
            for (gi, rids) in &by_graph {
                let labels: Vec<String> =
                    rids.iter().map(|rid| self.node_label(*gi, *rid)).collect();
                out.push(format!("  {gi:04X}: {}", labels.join(", ")));
            }
        }
        // field -> (type, offset), in declaration order; a repeated field
        // keeps its first place and its last declaration.
        let mut declared: IndexMap<&str, (&str, Option<&str>)> = IndexMap::new();
        for (field, kind, offset) in owlib.map_or(&[][..], |owlib| owlib.2.as_slice()) {
            declared.insert(field, (kind, offset.as_deref()));
        }
        let var_fields = names.var_fields.get(cls);
        let base: &[&str] = if node_uses.is_empty() {
            &[]
        } else {
            BASE_FIELDS
        };
        let mut note = String::new();
        if owlib.is_some() {
            note.push_str("; declared type and offset from OWLib");
        }
        if !node_uses.is_empty() {
            note.push_str("; the statescript base classes' fields are left out");
        }
        out.push(format!(
            "fields ({} objects in the data{note}):",
            self.index.objects.get(cls).copied().unwrap_or(0)
        ));
        let mut shown: HashSet<&str> = base.iter().copied().collect();
        shown.extend(["m_EE729DCB", "links"]);
        let place_of =
            |kind: &str, offset: Option<&str>| format!("+{} {kind}", offset.unwrap_or("None"));
        for (field, (count, filled, kinds)) in self.index.fields.get(cls).into_iter().flatten() {
            if !shown.insert(field) {
                continue;
            }
            let place = match declared.get(field.as_str()) {
                Some((kind, offset)) if !kind.is_empty() => place_of(kind, *offset),
                _ => String::new(),
            };
            let values: Vec<String> = most_common(kinds)
                .iter()
                .take(4)
                .map(|(name, number)| format!("{name} x{number}"))
                .collect();
            let values = if values.is_empty() {
                String::new()
            } else {
                format!(": {}", values.join(", "))
            };
            let extra = match var_fields
                .and_then(|fields| fields.get(field))
                .map(String::as_str)
            {
                Some("var") => "  (a variable: written)",
                Some("list") => "  (a variable list)",
                _ => "",
            };
            out.push(format!(
                "  {field:24} {place:28} set in {filled}/{count}{values}{extra}"
            ));
        }
        for (field, (kind, offset)) in &declared {
            if !shown.contains(field) {
                out.push(format!(
                    "  {field:24} {:28} not in the data",
                    place_of(kind, *offset)
                ));
            }
        }
        match codec_entry(names, cls) {
            Some(entry) => {
                out.push("codec (crates/fixed/data/statescript_codecs.json):".into());
                match entry.as_obj() {
                    Some(obj) => {
                        for (key, value) in obj.iter() {
                            out.push(format!("  {key}: {}", codec_value(value)));
                        }
                    }
                    None => out.push(format!("  entry: {}", codec_value(&entry))),
                }
            }
            None => out.push("codec: no entry in crates/fixed/data/statescript_codecs.json".into()),
        }
        out
    }

    pub fn find(&self, words: &[String], limit: i64) -> Vec<String> {
        let text = words.join(" ");
        let text = text.trim();
        let wanted = text.to_lowercase();
        if wanted.is_empty() {
            return vec!["nothing to find".into()];
        }
        let names = &self.names;
        let number = parse_hex(text);
        let guid = parse_guid(text);
        let mut hits: HashMap<&str, Vec<String>> = HashMap::new();
        let mut hit = |kind: &'static str, line: String| hits.entry(kind).or_default().push(line);
        for cls in self.known_classes() {
            let client = names
                .class(cls)
                .and_then(|info| info.client.as_deref())
                .unwrap_or("");
            let words = [
                cls,
                alias(names, cls),
                client,
                self.hash_of(cls).unwrap_or(""),
            ];
            if words
                .iter()
                .any(|word| !word.is_empty() && word.to_lowercase().contains(&wanted))
            {
                let count = self.index.class_nodes.get(cls).map_or(0, Vec::len);
                let what = if count > 0 {
                    plural(count, "node")
                } else {
                    plural(self.index.objects.get(cls).copied().unwrap_or(0), "object")
                };
                hit("class", format!("{cls}  {}  {what}", alias(names, cls)));
            }
        }
        let fields: BTreeMap<&String, _> = self.index.fields.iter().collect();
        for (cls, stats) in fields {
            for (field, (count, filled, _)) in stats {
                if field.to_lowercase().contains(&wanted) {
                    hit(
                        "field",
                        format!(
                            "{field} in {} ({cls}): set in {filled}/{count}",
                            alias(names, cls)
                        ),
                    );
                }
            }
        }
        let graphs: BTreeSet<Int> = self
            .index
            .graphs
            .keys()
            .chain(self.bundle.missing.iter())
            .copied()
            .collect();
        for gi in graphs {
            let place = self
                .index
                .attach
                .get(&gi)
                .map(|entries| attach_text(entries))
                .unwrap_or_default();
            if (number.is_some() && parse_graph(text) == Some(gi))
                || (!place.is_empty() && place.to_lowercase().contains(&wanted))
            {
                let missing = if self.index.graphs.contains_key(&gi) {
                    ""
                } else {
                    " (not in the data)"
                };
                let place = if place.is_empty() { "-".into() } else { place };
                hit(
                    "graph",
                    format!("{gi:04X}  {}  {place}{missing}", self.roles_text(gi)),
                );
            }
        }
        for hero in &self.heroes {
            if hero.name.to_lowercase().contains(&wanted)
                || guid == Some(hero.guid)
                || guid == Some(hero.body)
            {
                hit(
                    "hero",
                    format!(
                        "{}  hero {}  body {}  {}",
                        hero.name,
                        guid_text(hero.guid),
                        guid_text(hero.body),
                        hero.role
                    ),
                );
            }
        }
        let mut modes: Vec<_> = names.modes.iter().collect();
        modes.sort_by_key(|(guid, _)| *guid);
        for (mode_guid, mode) in modes {
            let name = mode.name.as_deref().unwrap_or("");
            if name.to_lowercase().contains(&wanted) || guid == Some(*mode_guid) {
                let teams: Vec<String> = mode
                    .teams
                    .iter()
                    .map(|team| {
                        format!(
                            "{} {}/{}",
                            team.team,
                            team.controller.as_deref().unwrap_or("-"),
                            team.body.as_deref().unwrap_or("-")
                        )
                    })
                    .collect();
                hit(
                    "mode",
                    format!(
                        "{}  {}  script {}  controller/body script: {}",
                        mode_name(names, *mode_guid),
                        guid_text(*mode_guid),
                        mode.script.as_deref().unwrap_or("-"),
                        teams.join(" ")
                    ),
                );
            }
        }
        for (map_guid, entry) in &self.maps {
            let name = entry.val("name").filter(|name| name.truthy());
            let name_text = name.map(json::py_str).unwrap_or_default();
            if name_text.to_lowercase().contains(&wanted) || guid == Some(*map_guid) {
                let modes: Vec<String> = entry
                    .at("modes")
                    .items()
                    .iter()
                    .filter_map(J::as_str)
                    .filter_map(|mode| Int::from_str_radix(mode.trim_start_matches("0x"), 16).ok())
                    .map(|mode| mode_name(names, mode))
                    .collect();
                let modes = if modes.is_empty() {
                    "-".into()
                } else {
                    modes.join(", ")
                };
                hit(
                    "map",
                    format!(
                        "{}  {}  modes: {modes}",
                        json::opt_str(entry.val("name")),
                        guid_text(*map_guid)
                    ),
                );
            }
        }
        let mut vars: Vec<_> = names.vars.iter().collect();
        vars.sort_by_key(|(var, _)| *var);
        for (var, entry) in vars {
            if entry.name.to_lowercase().contains(&wanted) {
                hit(
                    "var",
                    format!("{}  ({})", var_name(names, *var, None), entry.from),
                );
            }
        }
        if let Some(var) = self.parse_var(text) {
            if let Some(uses) = self
                .index
                .vars
                .get(&var)
                .filter(|_| names.var(var).is_none())
            {
                let graphs: HashSet<Int> = uses.iter().map(|found| found.gi).collect();
                hit(
                    "var",
                    format!(
                        "{}  {} uses in {}",
                        var_name(names, var, None),
                        uses.len(),
                        plural(graphs.len(), "graph")
                    ),
                );
            }
        }
        for (value, name) in &names.buttons {
            if name.to_lowercase().contains(&wanted) {
                hit("button", format!("{value} {name}"));
            }
        }
        if let Some((guid, found)) =
            guid.and_then(|guid| Some((guid, self.index.assets.get(&guid)?)))
        {
            if !found.is_empty() {
                let places: Vec<String> = found
                    .iter()
                    .take(10)
                    .map(|(gi, rid, path)| {
                        format!("{gi:04X} {} {path}", self.node_label(*gi, *rid))
                    })
                    .collect();
                let more = if found.len() > 10 { " ..." } else { "" };
                hit(
                    "asset",
                    format!(
                        "{} in {}: {}{more}",
                        guid_text(guid),
                        plural(found.len(), "place"),
                        places.join(", ")
                    ),
                );
            }
        }
        let mut out = Vec::new();
        for kind in [
            "class", "field", "graph", "hero", "mode", "map", "var", "button", "asset",
        ] {
            let Some(lines) = hits.get(kind) else {
                continue;
            };
            out.push(format!("{kind} ({}):", lines.len()));
            let len = lines.len() as i64;
            let shown = if limit < 0 {
                (len + limit).max(0)
            } else {
                limit.min(len)
            };
            out.extend(
                lines[..shown as usize]
                    .iter()
                    .map(|line| format!("  {line}")),
            );
            if len > limit {
                out.push(format!("  ... {} more (--limit)", len - limit));
            }
        }
        if out.is_empty() {
            out.push(format!("nothing matches {}", str_repr(text)));
        }
        out
    }

    pub fn hero(&self, text: &str) -> Vec<String> {
        let Some((body, record)) = self.find_body(text) else {
            return vec![format!("no hero {}", str_repr(text))];
        };
        let names = &self.names;
        let hero_guid = record
            .val("hero")
            .filter(|hero| hero.truthy())
            .and_then(J::as_str)
            .and_then(parse_hex);
        let hero = hero_guid.and_then(|guid| self.heroes.iter().find(|hero| hero.guid == guid));
        let (roots, how) = body_roots(record);
        let order = self.closure(&roots);
        let present: Vec<Int> = order
            .iter()
            .map(|(gi, ..)| *gi)
            .filter(|gi| self.index.graphs.contains_key(gi))
            .collect();
        let missing: Vec<String> = order
            .iter()
            .map(|(gi, ..)| *gi)
            .filter(|gi| !self.index.graphs.contains_key(gi))
            .map(|gi| format!("{gi:04X}"))
            .collect();
        let mut classes: IndexMap<&str, usize> = IndexMap::new();
        for gi in &present {
            for (cls, count) in &self.index.graphs[gi].classes {
                *classes.entry(cls).or_default() += count;
            }
        }
        let mut out = vec![format!(
            "{}  hero {}  {}  body {}",
            json::py_str(record.at("name")),
            hero.map_or_else(|| "-".into(), |hero| guid_text(hero.guid)),
            hero.map_or("", |hero| hero.role.as_str()),
            guid_text(body)
        )];
        let initial: Vec<String> = record
            .at("graphs")
            .items()
            .iter()
            .map(|entry| {
                entry
                    .val("graph")
                    .and_then(J::as_int)
                    .map_or_else(|| "-".into(), |graph| format!("{graph:04X}"))
            })
            .collect();
        let weapons: Vec<String> = record
            .at("weapons")
            .items()
            .iter()
            .enumerate()
            .map(|(slot, weapon)| match weapon.as_int() {
                Some(weapon) => format!("{slot}:{weapon:04X}"),
                None => format!("{slot}:-"),
            })
            .collect();
        let weapons = if weapons.is_empty() {
            "-".into()
        } else {
            weapons.join(" ")
        };
        let manager = record
            .val("manager")
            .and_then(J::as_int)
            .map_or_else(|| "-".into(), |manager| format!("{manager:04X}"));
        out.push(format!(
            "initial graphs (instance 1..N): {}; manager {manager}; weapons {weapons}",
            initial.join(" ")
        ));
        let node_count: usize = present.iter().map(|gi| self.index.graphs[gi].nodes).sum();
        let networked: usize = present
            .iter()
            .map(|gi| self.index.graphs[gi].networked)
            .sum();
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
            "{} graphs{missing_text}, {node_count} nodes ({networked} networked), {} node classes",
            order.len(),
            classes.len()
        ));
        for (slot, entry) in record.at("graphs").items().iter().enumerate() {
            for item in entry.at("m_1EB5A024").items() {
                let Some(ident) = item.val("m_0D09D2D9").and_then(super::data::guid) else {
                    continue;
                };
                let value = render(names, item.at("m_value"), 0);
                let graph = entry.val("graph").and_then(J::as_int).unwrap_or(0);
                out.push(format!(
                    "  init#{} {graph:04X} override {} = {value}",
                    slot + 1,
                    var_name(names, ident & 0xFFFF, None)
                ));
            }
        }
        out.push(String::new());
        out.extend(self.graph_rows(&order, &how));
        for gi in &present {
            for (rid, st, buttons) in &self.index.graphs[gi].abilities {
                out.push(format!(
                    "  ability {gi:04X} {}",
                    ability_text(self, *rid, *st, buttons)
                ));
            }
        }
        let mut counts: Vec<(&&str, &usize)> = classes.iter().collect();
        counts.sort_by(|a, b| {
            b.1.cmp(a.1)
                .then_with(|| alias(names, a.0).cmp(alias(names, b.0)))
        });
        let counts: Vec<String> = counts
            .iter()
            .map(|(cls, count)| format!("{} x{count}", alias(names, cls)))
            .collect();
        out.push(String::new());
        out.push(format!("classes: {}", counts.join(", ")));
        out.push(String::new());
        let graph_set: HashSet<Int> = present.iter().copied().collect();
        let mut shared: BTreeMap<Int, BTreeMap<&str, BTreeSet<Int>>> = BTreeMap::new();
        for (var, uses) in &self.index.vars {
            for found in uses {
                if graph_set.contains(&found.gi) && found.scope == ENTITY {
                    shared
                        .entry(*var)
                        .or_default()
                        .entry(found.kind)
                        .or_default()
                        .insert(found.gi);
                }
            }
        }
        let mut rows = Vec::new();
        for (var, kinds) in &shared {
            let every: BTreeSet<Int> = kinds.values().flatten().copied().collect();
            if every.len() < 2 {
                continue;
            }
            let parts: Vec<String> = kinds
                .iter()
                .map(|(kind, found)| {
                    let graphs: Vec<String> = found.iter().map(|gi| format!("{gi:04X}")).collect();
                    format!("{kind} {}", graphs.join(" "))
                })
                .collect();
            rows.push(format!(
                "  {}: {}",
                var_name(names, *var, Some(ENTITY)),
                parts.join("; ")
            ));
        }
        out.push(format!(
            "entity vars used by 2 or more of these graphs ({}):",
            rows.len()
        ));
        out.extend(rows);
        out
    }
}

/// A codec entry's value: text as it is, anything else as JSON.
fn codec_value(value: &J) -> String {
    match value {
        J::Str(text) => text.clone(),
        other => json::dumps(other),
    }
}

/// The class's entry in `crates/fixed/data/statescript_codecs.json`
/// (`classes` by hash, else by class name).
fn codec_entry(names: &super::data::Names, cls: &str) -> Option<J> {
    let data = json::parse(orpheus_fixed::STATESCRIPT_CODECS.as_bytes()).ok()?;
    let table = data.at("classes").as_obj()?;
    let hashed = class_hash(names, cls);
    let keys = [
        hashed.map(str::to_owned),
        Some(cls.to_owned()),
        Some(format!("STU_{}", hashed.unwrap_or("None"))),
    ];
    for key in keys.into_iter().flatten() {
        if let Some(entry) = table.get(&key) {
            return Some(entry.clone());
        }
    }
    table
        .iter()
        .map(|(_, item)| item)
        .find(|item| item.as_obj().is_some() && item.at("name").as_str() == Some(cls))
        .cloned()
}

/// How the server's VM runs a node class: the state kinds and node actions
/// of `orpheus_script::vm` (`run_node` and `begin` in `vm/nodes.rs`). Python
/// printed its registries instead (`runtime.STATE_CLASSES` and
/// `runtime.HANDLERS`); keep this table in step with the VM.
fn vm_text(cls: &str, is_state: bool) -> String {
    let state = |kind: &str| format!("state kind {kind} (orpheus_script::vm)");
    let action = |what: &str| format!("action: {what} (orpheus_script::vm)");
    match cls {
        "STUStatescriptEntry" | "STU_571CC72E" | "STUStatescriptEntryGameMessage" => {
            "entry: follows m_outPlug (orpheus_script::vm)".into()
        }
        "STUStatescriptActionSetVar" => action("stores its variables"),
        "STUStatescriptActionSendGameMessage" => action("sends its game message"),
        "STU_22926252" => action("exits the instance when m_67B81D25 and m_EB35D2F1 are unset"),
        "STU_12F5D52D" => action("stores each m_143D4C5B entry"),
        "STU_3387AB5D" => action("condition on m_condition"),
        "STU_821116A6" => action("condition: m_814F8EC3 contains m_value"),
        "STU_7DEB522D" => action("condition: the logical button is held"),
        "STU_4E2BCAAB" => action("condition, assumed false"),
        "STU_BC5E4622" => action("CycleIndex"),
        "STU_187D5125" => action("stops the instance's abilities"),
        "STU_08D6BA6E" => action("raycast, assumed to hit nothing"),
        "STUStatescriptActionSwitch" => action("switch on m_262AE982"),
        "STUStatescriptActionEffect"
        | "STUStatescriptActionCriteria"
        | "STU_CEBDAF17"
        | "STU_100EF8C2"
        | "STUStatescriptActionSendClientGlobalGameMessage"
        | "STUStatescriptActionPlayScript"
        | "STU_7106C356"
        | "STU_C43F9944" => action("no-op on the server"),
        "STU_1C2CC6D5" => action("ClearArray: writes a fresh empty array to m_26E3B366"),
        "STU_4C3509FB" | "STU_CBE8BDAA" => action("inserts m_value into the m_26E3B366 array"),
        "STU_E9694EFD" | "STU_6E5C5F66" => action("assumed no-op"),
        "STUStatescriptState"
        | "STU_9D7BF987"
        | "STU_B4CD94F4"
        | "STU_90C3BCEA"
        | "STUStatescriptStateDataFlowMapping"
        | "STU_E13B30A8"
        | "STUStatescriptStateUXPresenter"
        | "STUStatescriptStateAnimatedCamera"
        | "STU_7B9E8909"
        | "STU_40E05F6B" => state("Plain"),
        "STU_D9843704" => state("Plain, in its zero-valued networked form"),
        "STU_BD02E168" => state("Plain, as a client subgraph controller"),
        "STU_CD46AF93" => state("Plain, when networked"),
        "STU_F5A99B2A" => state("Plain, with networked tips"),
        "STU_066B4605" => state("Plain without an AI control target"),
        "STUStatescriptStateHealthPool" => state("HealthPool, in its base form"),
        "STU_E56BA8D8" => state("TargetTag, when networked and not remote"),
        "STUStatescriptStateBooleanSwitch" => state("BooleanSwitch"),
        "STU_B591EBFE" => state("DelayedSwitch"),
        "STU_DEBD057B" => state("Switch"),
        "STU_01178155" => state("ActiveQuery"),
        "STUStatescriptStateGamePadVibration" => state("GamePadVibration"),
        "STU_37D754C8" | "STUStatescriptStateSubScript" => state("SubScript"),
        "STUStatescriptStateGameMessageEntry" => state("MessageIdle"),
        "STUStatescriptStateWait" => state("Wait"),
        "STU_E3CEF6B9" => state("ServerFrames"),
        "STU_9C9A422A" => state("Defer"),
        "STU_B9898052" => state("Pulser"),
        "STU_E4A30BCF" => state("DisableButton"),
        "STU_D7F9E1C3" => state("AiFire"),
        "STUStatescriptStateMoveTo" => state("MoveTo"),
        "STU_D74D4F47" => state("WaitFrames"),
        "STU_91EF24A9" => state("AiCandidate"),
        "STUStatescriptStateLogicalButton" => state("LogicalButton"),
        "STUStatescriptStateStack" | "STU_A97E6999" => state("Stack"),
        "STUStatescriptStateAbility" => state("Ability"),
        "STU_316CFEF2" => state("MovementMod"),
        "STU_96F1DBCF" => state("MovementSample, as the owner's movement sampler"),
        "STUStatescriptStateAnim" => state("Anim"),
        "STU_691BFA55" | "STU_7CA2FBAF" => state("Effect"),
        "STU_38EE1100" => state("SendMessage"),
        "STUStatescriptStateCosmeticEntity" => state("CosmeticEntity"),
        "STUStatescriptStateChaseVar" => state("ChaseVar"),
        "STU_2F4E2E3F" => state("Watch"),
        "STUStatescriptStateTrackTargets" => state("TrackTargets, finding nothing"),
        "STU_23A00A53"
        | "STUStatescriptStateModifyHealth"
        | "STU_0A6BE10E"
        | "STUStatescriptStatePhysicsExplosion"
        | "STU_B5C87F3D"
        | "STU_54FEB21C" => state("Plain, assumed to have no effect"),
        "STU_77E13026" => state("HitList, recording no hits"),
        "STUStatescriptStateWeaponVolley" => state("WeaponVolley"),
        _ if is_state => "a plain State (no model of its own)".into(),
        _ => "no model: it only follows m_outPlug".into(),
    }
}
