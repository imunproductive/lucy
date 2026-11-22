//! `data/game_queues_174.json`: what each queue card (`0C7`) needs to build a
//! match. Port of `tools/extract_game_queues.py`.
//!
//! It reads the cards (`0C7`), their rulesets (`0C0`), game modes (`0C5`), map
//! catalogs (`039`), map headers (`09F`), card titles (`0D9`), the Arcade menu
//! (`0EE` nodes and their `0CC` tags) and the display strings (`07C`). For each
//! card it writes the name, the rulesets (game mode, players per team,
//! free-for-all), the role slots, the maps and the largest party. The client
//! ships only 4 of the 43 catalogs the cards name. For the other cards the
//! maps come from the retail capture (message 36300 of
//! [`lucy_fixed::RETAIL_TEMPLATES`] lists each competitive card with its
//! maps), or else from the map headers that support the card's modes,
//! narrowed to the maps its Arcade tags name. `maps_source` says which.

use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet, HashMap},
    path::Path,
};

use lucy_stu::{Asset, Object, Types, Value};
use serde_json::{Map, Value as Json, json};

use crate::{Install, Output, Result, asset_error};

/// The index part of a GUID: what DataTool writes before the dot.
pub(crate) const INDEX: u64 = 0xFFFF_FFFF_FFFF;

/// Cards the client picks in code, with no name in the data (the client's
/// functions at `0x7FF78934DC70` and `0x7FF78934AA51`).
const CODE_NAMES: &[(u64, &str)] = &[
    (0x6D, "Quick Play (no role queue)"),
    (0xED, "Quick Play"),
    (0x70, "Play vs AI (AI level 1)"),
    (0x71, "Play vs AI (AI level 2)"),
    (0x72, "Play vs AI (AI level 3)"),
];

/// `Enum_1964FED7::PVE`, a game mode's type.
const MODE_PVE: i64 = 3;
/// `STUMapType::PVE`, a map header's type.
const MAP_PVE: i64 = 2;

/// The message of the retail capture that lists the competitive cards.
const RANKED_CARDS: u64 = 36300;

pub(crate) fn derive(install: &Install, _out: &Path) -> Result<Vec<Output>> {
    let queues = Queues::new(install)?;
    let mut cards = Map::new();
    let mut skipped = Map::new();
    for (&index, asset) in &queues.cards {
        let card = Node::root(asset)?;
        let name = key(typed(index, 0x0C7));
        if card.list("m_rulesets").is_empty() {
            let title = queues
                .card_name(index, card)
                .unwrap_or_else(|| "None".into());
            skipped.insert(
                name,
                json!(format!("{title}: no rulesets, so no game to start")),
            );
            continue;
        }
        cards.insert(name, queues.card(index, card)?);
    }
    // The event version of each map: the celebration (the event kind in
    // catalog/events.py) and its map.
    let mut variants = Map::new();
    for asset in queues.headers.values() {
        let header = Node::root(asset)?;
        if scene(header).to_lowercase().contains("lobby") {
            continue;
        }
        for event in header.nodes("m_celebrationOverrides") {
            let found = variants
                .entry(key(header.guid("m_map")))
                .or_insert_with(|| json!([]));
            if let Json::Array(found) = found {
                found.push(json!({
                    "celebration": key(event.guid("m_celebrationType")),
                    "map": key(event.guid("m_map")),
                }));
            }
        }
    }
    let table = json!({ "cards": cards, "skipped": skipped, "variants": variants });
    Ok(vec![Output::json("data/game_queues_174.json", &table)])
}

/// An instance of a decoded asset, read as DataTool shows it: a field the
/// asset doesn't store has its declared default.
#[derive(Clone, Copy)]
pub(crate) struct Node<'a> {
    asset: &'a Asset,
    object: &'a Object,
}

impl<'a> Node<'a> {
    pub(crate) fn root(asset: &'a Asset) -> Result<Self> {
        let object = asset
            .root()
            .ok_or_else(|| asset_error("an asset without a root instance"))?;
        Ok(Self { asset, object })
    }

    /// An instance of `asset`.
    pub(crate) fn of(asset: &'a Asset, object: &'a Object) -> Self {
        Self { asset, object }
    }

    /// Whether the class has the field at all.
    fn has(self, name: &str) -> bool {
        Types::get().field_named(self.object.class, name).is_some()
    }

    pub(crate) fn value(self, name: &str) -> Option<&'a Value> {
        self.object.get(name).or_else(|| {
            Types::get()
                .field_named(self.object.class, name)
                .map(|field| &field.default)
        })
    }

    /// A reference, 0 when null.
    pub(crate) fn guid(self, name: &str) -> u64 {
        match self.value(name) {
            Some(Value::Guid(guid)) => *guid,
            _ => 0,
        }
    }

    /// An integer or enum.
    pub(crate) fn int(self, name: &str) -> i64 {
        match self.value(name) {
            Some(Value::Int(value)) => *value,
            Some(Value::UInt(value)) => *value as i64,
            _ => 0,
        }
    }

    pub(crate) fn list(self, name: &str) -> &'a [Value] {
        match self.value(name) {
            Some(Value::Array(values)) => values,
            _ => &[],
        }
    }

    /// The references of an array.
    pub(crate) fn guids(self, name: &str) -> impl Iterator<Item = u64> + 'a {
        self.list(name).iter().filter_map(|value| match value {
            Value::Guid(guid) => Some(*guid),
            _ => None,
        })
    }

    /// An embedded or inline instance.
    pub(crate) fn node(self, name: &str) -> Option<Self> {
        let object = self.asset.resolve(self.value(name)?)?;
        Some(Self {
            asset: self.asset,
            object,
        })
    }

    /// The instances of an array.
    pub(crate) fn nodes(self, name: &str) -> impl Iterator<Item = Self> + 'a {
        let asset = self.asset;
        self.list(name).iter().filter_map(move |value| {
            Some(Self {
                asset,
                object: asset.resolve(value)?,
            })
        })
    }

    /// The instance's class, by the type table.
    pub(crate) fn class_name(self) -> Option<&'static str> {
        self.object.class_name()
    }
}

/// A GUID as the 64-bit key the game sends, as `0x` and sixteen hex digits:
/// its index and type, as DataTool names it.
pub(crate) fn key(guid: u64) -> String {
    format!("0x{:016X}", typed(guid & INDEX, lucy_casc::guid_type(guid)))
}

/// The GUID of an index as an asset type: the type minus one, bits
/// reversed, at bits 48..59.
pub(crate) fn typed(index: u64, kind: u16) -> u64 {
    u64::from(((kind - 1) << 4).reverse_bits()) << 48 | index & INDEX
}

/// Every asset of one type the install stores, decoded, by index. Like
/// DataTool, it leaves out assets the install lists but doesn't store (some
/// `09F` headers' content keys are missing from the encoding table).
pub(crate) fn load(install: &Install, kind: u16) -> Result<BTreeMap<u64, Asset>> {
    let stored: Vec<u64> = install
        .guids(kind)
        .into_iter()
        .filter(|&guid| install.game().is_stored(guid))
        .collect();
    install
        .stu_all(&stored)
        .into_iter()
        .map(|(guid, asset)| Ok((guid & INDEX, asset?)))
        .collect()
}

/// A team as the pack names it (`TeamIndex`; names other than blue, red and
/// free-for-all are OWLib's).
fn team_name(team: i64) -> String {
    match team {
        0 => "blue",
        1 => "red",
        4 => "ffa",
        2 => "x64EE800E",
        3 => "xB50082CF",
        5 => "x3A5ECACD",
        -1 => "xBC219E96",
        _ => return team.to_string(),
    }
    .into()
}

/// A map header's scene path, like `Maps\China\China - Garden - CTF`.
fn scene(header: Node<'_>) -> &str {
    match header.value("m_mapName") {
        Some(Value::String(path)) => path,
        _ => "",
    }
}

/// The folder of a map's scene, like `China`.
fn folder(header: Node<'_>) -> &str {
    let parts: Vec<&str> = split_backslashes(scene(header));
    if parts.len() > 2 { parts[1] } else { "" }
}

/// Split at each run of backslashes.
fn split_backslashes(text: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut start = 0;
    let mut chars = text.char_indices().peekable();
    while let Some((at, c)) = chars.next() {
        if c == '\\' {
            parts.push(&text[start..at]);
            while chars.next_if(|&(_, c)| c == '\\').is_some() {}
            start = chars.peek().map_or(text.len(), |&(at, _)| at);
        }
    }
    parts.push(&text[start..]);
    parts
}

/// Python's `str.title()`: each letter after a letter lower-case, others
/// upper-case.
fn title_case(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut after_letter = false;
    for c in text.chars() {
        if after_letter {
            out.extend(c.to_lowercase());
        } else {
            out.extend(c.to_uppercase());
        }
        after_letter = c.is_lowercase() || c.is_uppercase();
    }
    out
}

/// Remove markup tags and the white space before each.
fn strip_tags(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(c) = rest.chars().next() {
        let trimmed = rest.trim_start();
        if let Some(tag) = trimmed.strip_prefix('<')
            && let Some(end) = tag.find('>')
        {
            rest = &tag[end + 1..];
            continue;
        }
        out.push(c);
        rest = &rest[c.len_utf8()..];
    }
    out
}

/// The map names of an Arcade tag like `MAPS: Hanamura, King's Row and
/// Hollywood`, lower-case; `None` for other tags and `MAP: Random`.
fn tag_map_names(tag: &str) -> Option<Vec<String>> {
    if !(tag.starts_with("MAP: ") || tag.starts_with("MAPS: ")) || tag == "MAP: Random" {
        return None;
    }
    let (_, list) = tag.split_once(": ")?;
    Some(
        list.split(',')
            .enumerate()
            .map(|(at, name)| {
                let mut name = name;
                if at > 0 {
                    name = name.trim_start();
                    if let Some(rest) = name.strip_prefix("and")
                        && rest.starts_with(char::is_whitespace)
                    {
                        name = rest;
                    }
                }
                name.trim().to_lowercase()
            })
            .collect(),
    )
}

struct Queues<'a> {
    install: &'a Install,
    strings: RefCell<HashMap<u64, Option<String>>>,
    cards: BTreeMap<u64, Asset>,
    rulesets: BTreeMap<u64, Asset>,
    modes: BTreeMap<u64, Asset>,
    catalogs: BTreeMap<u64, Asset>,
    headers: BTreeMap<u64, Asset>,
    titles: BTreeMap<u64, Asset>,
    /// Each card's name in the Arcade menu, by card index.
    menu_names: HashMap<u64, String>,
    /// Each card's tags in the Arcade menu, by card index.
    menu_tags: HashMap<u64, Vec<String>>,
    /// The maps of each catalog in the retail capture, by catalog GUID.
    captured: HashMap<u64, Vec<u64>>,
    /// The headers each event version of a map replaces, by header index.
    variant_of: HashMap<u64, BTreeSet<u64>>,
    /// The map names each catalog's cards show in the Arcade ("MAPS:
    /// Hanamura, King's Row"), lower-case, by catalog GUID.
    tag_maps: HashMap<u64, BTreeSet<String>>,
}

impl<'a> Queues<'a> {
    fn new(install: &'a Install) -> Result<Self> {
        let mut queues = Self {
            install,
            strings: RefCell::default(),
            cards: load(install, 0x0C7)?,
            rulesets: load(install, 0x0C0)?,
            modes: load(install, 0x0C5)?,
            catalogs: load(install, 0x039)?,
            headers: load(install, 0x09F)?,
            titles: load(install, 0x0D9)?,
            menu_names: HashMap::new(),
            menu_tags: HashMap::new(),
            captured: HashMap::new(),
            variant_of: HashMap::new(),
            tag_maps: HashMap::new(),
        };
        (queues.menu_names, queues.menu_tags) =
            queues.menu(&load(install, 0x0EE)?, &load(install, 0x0CC)?)?;
        queues.captured = queues.captured()?;
        queues.variant_of = queues.variants()?;
        for (index, asset) in &queues.cards {
            let catalog = Node::root(asset)?.guid("m_catalog");
            for tag in queues.menu_tags.get(index).into_iter().flatten() {
                if let Some(names) = tag_map_names(tag) {
                    queues.tag_maps.entry(catalog).or_default().extend(names);
                }
            }
        }
        Ok(queues)
    }

    /// A display string; `None` for a null reference or a missing string.
    fn text(&self, guid: u64) -> Option<String> {
        if guid == 0 {
            return None;
        }
        self.strings
            .borrow_mut()
            .entry(guid)
            .or_insert_with(|| self.install.string(guid))
            .clone()
    }

    /// Each card's name and tags in the Arcade menu. A node names a card
    /// (`m_5DC61E59`) and lists tags (`0CC`, `m_5797DE13`); a group node
    /// (`m_children`) passes its tags to children that have none, and its
    /// name to them too ("Junkenstein's Revenge: Normal").
    #[allow(clippy::type_complexity)]
    fn menu(
        &self,
        nodes: &BTreeMap<u64, Asset>,
        tags: &BTreeMap<u64, Asset>,
    ) -> Result<(HashMap<u64, String>, HashMap<u64, Vec<String>>)> {
        let mut parent = HashMap::new();
        for (&index, asset) in nodes {
            for child in Node::root(asset)?.guids("m_children") {
                parent.insert(child & INDEX, index);
            }
        }
        let own_tags = |index: u64| -> Result<Vec<String>> {
            let mut found = Vec::new();
            for tag in Node::root(&nodes[&index])?.guids("m_5797DE13") {
                if let Some(tag) = tags.get(&(tag & INDEX)) {
                    found.extend(self.text(Node::root(tag)?.guid("m_name")));
                }
            }
            Ok(found.into_iter().filter(|tag| !tag.is_empty()).collect())
        };
        let mut names = HashMap::new();
        let mut card_tags = HashMap::new();
        for (&index, asset) in nodes {
            let node = Node::root(asset)?;
            let card = node.guid("m_5DC61E59") & INDEX;
            if card == 0 || names.contains_key(&card) {
                continue;
            }
            let mut title = self.text(node.guid("m_name")).unwrap_or_default();
            let mut found = own_tags(index)?;
            let mut up = parent.get(&index).copied();
            if found.is_empty()
                && let Some(up) = up
            {
                let group = self
                    .text(Node::root(&nodes[&up])?.guid("m_name"))
                    .unwrap_or_else(|| "None".into());
                title = format!("{group}: {}", title_case(&title));
            }
            while found.is_empty()
                && let Some(at) = up
            {
                found = own_tags(at)?;
                up = parent.get(&at).copied();
            }
            names.insert(card, title);
            card_tags.insert(card, found);
        }
        Ok((names, card_tags))
    }

    /// The maps of each catalog the retail capture shows: message 36300
    /// lists the competitive cards (`+0x98`), each with its card (`+0x18`)
    /// and maps (`+0xD8`).
    fn captured(&self) -> Result<HashMap<u64, Vec<u64>>> {
        let templates: Json = serde_json::from_str(lucy_fixed::RETAIL_TEMPLATES)
            .map_err(|error| asset_error(format!("retail templates: {error}")))?;
        let ranked = templates["server"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|message| message[1].as_u64() == Some(RANKED_CARDS))
            .ok_or_else(|| asset_error("the retail templates lack message 36300"))?;
        let mut maps = HashMap::new();
        for entry in ranked[2]["+0x98"].as_array().into_iter().flatten() {
            let Some(card) = entry["+0x18"]
                .as_u64()
                .and_then(|card| self.cards.get(&(card & INDEX)))
            else {
                continue;
            };
            let catalog = Node::root(card)?.guid("m_catalog");
            if catalog != 0 {
                let found = entry["+0xD8"].as_array().into_iter().flatten();
                maps.insert(catalog, found.filter_map(Json::as_u64).collect());
            }
        }
        Ok(maps)
    }

    /// Event versions of maps (Winter, Halloween, Lunar New Year) and the
    /// headers they replace.
    fn variants(&self) -> Result<HashMap<u64, BTreeSet<u64>>> {
        let mut variants: HashMap<u64, BTreeSet<u64>> = HashMap::new();
        for (&index, asset) in &self.headers {
            let header = Node::root(asset)?;
            let base = header.guid("m_baseMap");
            if base != 0 {
                variants.entry(index).or_default().insert(base & INDEX);
            }
            for event in header.nodes("m_celebrationOverrides") {
                let map = event.guid("m_map") & INDEX;
                variants.entry(map).or_default().insert(index);
            }
        }
        Ok(variants)
    }

    fn header(&self, index: u64) -> Result<Node<'_>> {
        let asset = self
            .headers
            .get(&(index & INDEX))
            .ok_or_else(|| asset_error(format!("no map header {:012X}.09F", index & INDEX)))?;
        Node::root(asset)
    }

    fn supports(header: Node, mode: u64) -> bool {
        header
            .guids("m_supportedGamemodes")
            .any(|found| found == mode)
    }

    /// Whether a header is the event version of a map that supports the mode
    /// itself.
    fn replaces(&self, index: u64, mode: u64) -> Result<bool> {
        for base in self.variant_of.get(&index).into_iter().flatten() {
            if let Some(asset) = self.headers.get(base)
                && Self::supports(Node::root(asset)?, mode)
            {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn card_name(&self, index: u64, card: Node) -> Option<String> {
        let display = card.guid("m_A848F2C7");
        let title = self
            .titles
            .get(&(display & INDEX))
            .filter(|_| display != 0 && lucy_casc::guid_type(display) == 0x0D9)
            .and_then(|asset| Node::root(asset).ok())
            .and_then(|title| self.text(title.guid("m_name")))
            .filter(|title| !title.is_empty());
        let ranked = card
            .node("m_2226CBD8")
            .and_then(|ranked| self.text(ranked.guid("m_1E4E5957")))
            .filter(|ranked| !ranked.is_empty());
        let mut found = CODE_NAMES
            .iter()
            .find(|(code, _)| *code == index)
            .map(|(_, name)| (*name).to_owned());
        // A season or a year: the title names it.
        if found.is_none() && title.is_some() && card.int("m_DF9B21EF") != 0 {
            found.clone_from(&title);
        }
        let found = found
            .or_else(|| {
                self.menu_names
                    .get(&index)
                    .filter(|name| !name.is_empty())
                    .cloned()
            })
            .or(title)
            .or_else(|| {
                ranked.map(|ranked| {
                    if ranked.starts_with("Competitive") {
                        ranked
                    } else {
                        format!("Competitive {ranked}")
                    }
                })
            })
            .or_else(|| match card.guids("m_rulesets").collect::<Vec<_>>()[..] {
                [ruleset] => {
                    let ruleset = Node::root(self.rulesets.get(&(ruleset & INDEX))?).ok()?;
                    self.text(ruleset.node("m_gamemode")?.guid("m_description"))
                        .filter(|name| !name.is_empty())
                }
                _ => None,
            });
        match found {
            Some(found) => Some(strip_tags(&found)),
            None => (display != 0).then(|| key(display)),
        }
    }

    fn ruleset_node(&self, ruleset: u64) -> Result<Node<'_>> {
        let asset = self
            .rulesets
            .get(&(ruleset & INDEX))
            .ok_or_else(|| asset_error(format!("no ruleset {}", key(ruleset))))?;
        Node::root(asset)?
            .node("m_gamemode")
            .ok_or_else(|| asset_error(format!("ruleset {} has no game mode", key(ruleset))))
    }

    /// A ruleset's game mode and players per team. The mode gives each team a
    /// default size (`m_170AA4B8`); the ruleset overrides it (`m_341EF5FA`),
    /// for one team or all, 0 keeping it.
    fn ruleset(&self, ruleset: u64) -> Result<Json> {
        let game = self.ruleset_node(ruleset)?;
        let mode_guid = game.guid("m_gamemode");
        let mode = Node::root(
            self.modes
                .get(&(mode_guid & INDEX))
                .ok_or_else(|| asset_error(format!("no game mode {}", key(mode_guid))))?,
        )?;
        let mut sizes: Vec<(String, i64)> = Vec::new();
        let set = |sizes: &mut Vec<(String, i64)>, side: String, size: i64| match sizes
            .iter_mut()
            .find(|(found, _)| *found == side)
        {
            Some((_, found)) => *found = size,
            None => sizes.push((side, size)),
        };
        for team in mode.nodes("m_teams") {
            set(
                &mut sizes,
                team_name(team.int("m_team")),
                team.int("m_170AA4B8"),
            );
        }
        // Overrides for all teams first, then for one team.
        let mut entries: Vec<(Option<Node>, i64)> = game
            .nodes("m_teams")
            .map(|entry| (entry.node("m_team"), entry.int("m_341EF5FA")))
            .collect();
        entries.sort_by_key(|(team, _)| team.is_some_and(|team| team.has("m_team")));
        for (team, size) in entries {
            let sides = match team.filter(|team| team.has("m_team")) {
                Some(team) => vec![team_name(team.int("m_team"))],
                None => sizes.iter().map(|(side, _)| side.clone()).collect(),
            };
            for side in sides {
                if size != 0 {
                    set(&mut sizes, side, size);
                }
            }
        }
        let free_for_all = sizes
            .iter()
            .find(|(side, _)| side == "ffa")
            .map(|(_, n)| *n);
        let playing: Map<String, Json> = sizes
            .iter()
            .filter(|(side, size)| *size != 0 && side != "ffa")
            .map(|(side, size)| (side.clone(), json!(size)))
            .collect();
        let players =
            free_for_all.unwrap_or_else(|| playing.values().filter_map(Json::as_i64).sum());
        Ok(json!({
            "ruleset": key(ruleset),
            "name": self.text(game.guid("m_description")),
            "mode": key(mode_guid),
            "mode_name": self.text(mode.guid("m_displayName")),
            "free_for_all": free_for_all.is_some(),
            "teams": playing.len(),
            "team_sizes": playing,
            "players": players,
        }))
    }

    fn maps(&self, card: Node, modes: &[u64]) -> Result<(&'static str, Vec<Json>)> {
        let catalog = card.guid("m_catalog");
        let (source, found) = if let Some(asset) = self.catalogs.get(&(catalog & INDEX))
            && catalog != 0
        {
            let found = Node::root(asset)?
                .list("m_mapGUIDs")
                .iter()
                .filter_map(|value| match value {
                    Value::UInt(guid) => Some(*guid),
                    Value::Int(guid) => Some(*guid as u64),
                    Value::Guid(guid) => Some(*guid),
                    _ => None,
                })
                .collect();
            ("catalog", found)
        } else if let Some(found) = self.captured.get(&catalog) {
            ("retail capture", found.clone())
        } else {
            self.derived(catalog, modes)?
        };
        let mut maps = Vec::new();
        for map in found {
            let header = self.header(map)?;
            let supported: Vec<Json> = modes
                .iter()
                .filter(|&&mode| Self::supports(header, mode))
                .map(|&mode| json!(key(mode)))
                .collect();
            maps.push(json!({
                "map": format!("0x{map:016X}"),
                "name": self.text(header.guid("m_displayName")),
                "modes": supported,
            }));
        }
        Ok((source, maps))
    }

    fn display(&self, header: Node) -> String {
        self.text(header.guid("m_displayName"))
            .unwrap_or_default()
            .to_lowercase()
    }

    /// Maps whose header supports one of the modes, without menu scenes
    /// (Lobby), Workshop maps and event versions of a map that supports the
    /// mode itself. A PvE mode takes PvE maps if it has any. When the
    /// catalog's cards name maps in their Arcade tags, only those are kept;
    /// a name that is no header's ("London", "LiJiang Tower") keeps the maps
    /// in that map's folder.
    fn derived(&self, catalog: u64, modes: &[u64]) -> Result<(&'static str, Vec<u64>)> {
        let mut found: Vec<u64> = Vec::new();
        for &mode in modes {
            let mut headers = Vec::new();
            for (&index, asset) in &self.headers {
                let header = Node::root(asset)?;
                if Self::supports(header, mode)
                    && !scene(header).to_lowercase().contains("lobby")
                    && folder(header) != "Workshop"
                    && !self.replaces(index, mode)?
                {
                    headers.push((index, header));
                }
            }
            let mode_asset = self
                .modes
                .get(&(mode & INDEX))
                .ok_or_else(|| asset_error(format!("no game mode {}", key(mode))))?;
            if Node::root(mode_asset)?.int("m_gameModeType") == MODE_PVE
                && headers.iter().any(|(_, h)| h.int("m_mapType") == MAP_PVE)
            {
                headers.retain(|(_, h)| h.int("m_mapType") == MAP_PVE);
            }
            for (index, _) in headers {
                if !found.contains(&index) {
                    found.push(index);
                }
            }
        }
        let map_keys = |indices: &[u64]| -> Vec<u64> {
            let mut keys: Vec<u64> = indices.iter().map(|&index| typed(index, 0x002)).collect();
            keys.sort_unstable();
            keys
        };
        let Some(names) = self
            .tag_maps
            .get(&catalog)
            .filter(|names| !names.is_empty())
        else {
            return Ok(("game modes", map_keys(&found)));
        };
        let mut display = HashMap::new();
        for &index in &found {
            display.insert(index, self.display(self.header(index)?));
        }
        let mut kept: Vec<u64> = found
            .iter()
            .copied()
            .filter(|index| names.contains(&display[index]))
            .collect();
        let shown: BTreeSet<&String> = display.values().collect();
        for missing in names.iter().filter(|name| !shown.contains(name)) {
            let mut folders = BTreeSet::new();
            for asset in self.headers.values() {
                let header = Node::root(asset)?;
                if self.display(header) == *missing || folder(header).to_lowercase() == *missing {
                    folders.insert(folder(header).to_owned());
                }
            }
            for &index in &found {
                if folders.contains(folder(self.header(index)?)) && !kept.contains(&index) {
                    kept.push(index);
                }
            }
        }
        let kept = if kept.is_empty() { &found } else { &kept };
        Ok(("arcade tags", map_keys(kept)))
    }

    fn card(&self, index: u64, card: Node) -> Result<Json> {
        let ruleset_guids: Vec<u64> = card.guids("m_rulesets").collect();
        let rulesets = ruleset_guids
            .iter()
            .map(|&ruleset| self.ruleset(ruleset))
            .collect::<Result<Vec<_>>>()?;
        let mut modes = Vec::new();
        for &ruleset in &ruleset_guids {
            let mode = self.ruleset_node(ruleset)?.guid("m_gamemode");
            if !modes.contains(&mode) {
                modes.push(mode);
            }
        }
        let mut ai_teams = Map::new();
        for rule in card.nodes("m_7FB46D96") {
            let level = rule.int("m_97CFD9DA");
            let team = rule
                .node("m_team")
                .filter(|team| team.has("m_team"))
                .map_or_else(|| "all".to_owned(), |team| team_name(team.int("m_team")));
            if level != 0 {
                ai_teams.insert(team, json!(level));
            }
        }
        // No card field limits a party. It has to fit in one team, in every
        // ruleset; in free-for-all there is no team to hold it.
        let max_party = rulesets
            .iter()
            .map(|ruleset| {
                if ruleset["free_for_all"] == json!(true) {
                    return 1;
                }
                ruleset["team_sizes"]
                    .as_object()
                    .into_iter()
                    .flatten()
                    .filter(|(side, _)| !ai_teams.contains_key(*side))
                    .filter_map(|(_, size)| size.as_i64())
                    .max()
                    .unwrap_or(0)
            })
            .min()
            .unwrap_or(0);
        let (source, maps) = self.maps(card, &modes)?;
        let optional_key = |guid: u64| {
            if guid == 0 {
                Json::Null
            } else {
                json!(key(guid))
            }
        };
        let roles: Map<String, Json> = card
            .nodes("m_66C923AF")
            .map(|slot| (key(slot.guid("m_1F753B44")), json!(slot.int("m_AC4BF158"))))
            .collect();
        Ok(json!({
            "name": self.card_name(index, card),
            "competitive": card.int("m_157AF68C") == 1,
            "groups_only": card.int("m_E566638A") == 1,
            "parent": optional_key(card.guid("m_2593569C")),
            "queue_with": optional_key(card.guid("m_679737B3")),
            "rulesets": rulesets,
            "ai_teams": ai_teams,
            "roles": roles,
            "max_party": max_party,
            "catalog": key(card.guid("m_catalog")),
            "maps_source": source,
            "maps": maps,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_rules_match_python() {
        assert_eq!(
            split_backslashes(r"Maps\\China\\China - Garden"),
            ["Maps", "China", "China - Garden"]
        );
        assert_eq!(split_backslashes(r"\a\\"), ["", "a", ""]);
        assert_eq!(title_case("normal"), "Normal");
        assert_eq!(title_case("hard mode's"), "Hard Mode'S");
        assert_eq!(strip_tags("Name <fg=1>x</fg>"), "Namex");
        assert_eq!(
            tag_map_names("MAPS: Hanamura, King's Row and Hollywood, and Dorado"),
            Some(vec![
                "hanamura".into(),
                "king's row and hollywood".into(),
                "dorado".into()
            ])
        );
        assert_eq!(tag_map_names("MAP: Random"), None);
        assert_eq!(key(0x0630_0000_0000_0001), "0x0630000000000001");
        assert_eq!(typed(0xD4, 0x002), 0x0800_0000_0000_00D4);
    }
}
