//! `data/game_anim_lengths_174.json`: how long the animation an alias plays
//! runs on each hero and bot body, read from the compiled game data of the
//! body's blend tree set. A statescript `Anim` state without an authored
//! length runs for this length (`crates/script/src/anim_lengths.rs`).
//!
//! Ported from leafall903/ORPHEUS (MIT), commit `c239a84`,
//! `tools/extract_emote_durations.py`; upstream's research notes are
//! `docs/ANIMATION_ALIAS_STATE.txt` and the code comments of that tool.
//!
//! **The body's set.** A body (`003`) names its blend tree set (`021`) in
//! its model component (`m_animBlendTreeSet`). A body needs exactly one set
//! (as upstream's greeting table); a body with none or several is left out.
//!
//! **The alias's item.** The set lists the aliases (`m_253EE7C8`, `0C8`
//! keys) and, in the same order, one item each (`m_blendTreeItems`); a set
//! whose two lists differ in length is left out, as is an alias outside
//! type `0C8` or with a zero index. The length comes from the item's
//! compiled game data (`m_gameData`, `STU_7D00A73D`), not from its blend
//! tree (`m_C0214513`, what [`crate::blend_trees`] walks).
//!
//! **The walk.** The game data lists nodes (`m_nodes`). From node 0,
//! pass-through nodes (`STU_30C2E3AE`) lead on to the node their
//! `m_38CF6CD3` names (a loop or an index out of range ends the walk with
//! nothing). Then, at most 48 deep:
//!
//! - a leaf (`STU_3C8FB392`) names the record (`m_9D16E038`, an index into
//!   the game data's `m_animDatas`, `STUAnimGameData_AnimationData`) whose
//!   `m_D8514AD9` is the length in seconds (finite and above zero);
//! - embedded data (`STU_331B7B06` with `m_53A1B672` 0) walks the game data
//!   it holds (`m_EB45D767`) from its node 0 in the same way;
//! - a blend (`STU_4178508A` with `m_5E009A60` 1) of leaves (`m_540E2FC8`,
//!   with strictly rising thresholds `m_0EC67F50`, one each) has a length
//!   only when every leaf gives the same one;
//! - a category selector (`STU_623B2B19` with `m_E6058AAD` 1, no
//!   `m_4D532CFC` or `m_53D9C839`) has one key (`m_9F7CA0BB`) per branch
//!   (`m_540E2FC8`), the last key 0 (the default branch). A key's low 31
//!   bits are a category index (`015`); with the top bit clear the branch
//!   is also taken for a category whose parents (the category's `015`
//!   asset, `m_0E5A577A`) hold that index. The first matching branch
//!   wins.
//!
//! Every other node gives nothing.
//!
//! **The table.** An alias without selectors has one length for every
//! category (`any`). With selectors, its length is listed for no category
//! (`0x0000000000000000`) and for each category: with only top-bit keys
//! ("strict"), for each key's category, and also for `any` when every one
//! resolves; otherwise for every category of the install, by its parents.
//!
//! **The scalar.** The leaf also names a sample row (`m_76E8D211`, used
//! when `m_3A9A1E85` is set) of the set's model data (`m_D1467FCA`,
//! `STU_B38E3226`, rows `m_A867458D`), whose track `m_3D2FFAAA` has times
//! (`m_4DC48800`) and values (`m_1EB5B02B`). The scalar is 0 without a row
//! or with an empty track, the value of a one-key track, and `null` (no
//! answer, which also hides `any`) for a longer track; the same walk picks
//! it. An `Anim` state writes it to its `m_EBEFB4A7` output.
//!
//! **The data.** The 38 hero and bot bodies name 33 sets. The common
//! graph `02C5` (node 51) plays alias `0C8:043C` without a length: 21 of
//! the sets list it, mostly as one leaf (Reaper's `0007.021`: 4.77 s), but
//! Soldier: 76's `011C.021` doesn't, so on him that state still never
//! ends (the server can't tell what the client does with an alias its
//! set lacks). Every scalar these sets give is 0.
//!
//! The file: `{"bodies": {body: set}, "sets": {set: {alias: {"lengths":
//! {category: seconds}, "scalars": {category: scalar | null}}}}}`, sorted,
//! numbers as `f32`, empty maps left out, written without indentation
//! (selectors that inherit list every category of the install: about
//! 21,000 lengths over 33 sets).

use std::{
    collections::{BTreeMap, BTreeSet, HashSet},
    path::Path,
};

use orpheus_stu::{Asset, Object, Value};
use serde_json::{Map, json};

use crate::{
    Install, Output, Result, asset_error,
    blend_trees::{ALIASES, ITEMS, field, keys},
    guid_hex,
    heroes::{Assets, bodies, movers::float, parse_guid},
};

const ENTITY: u16 = 0x003;
const CATEGORY: u16 = 0x015;
/// A category's GUID: its type bits and index.
pub(crate) const CATEGORY_TYPE: u64 = 0x0280_0000_0000_0000;
/// The bits of a GUID DataTool names as its index.
const INDEX: u64 = 0xFFFF_FFFF_FFFF;
/// The key the file writes `any` for (upstream's `EMOTE_ANY_CATEGORY`).
const ANY: u64 = u64::MAX;
/// How deep the walk goes (upstream: the client's `F4CBC0` limits nested
/// alias references).
const MAX_DEPTH: usize = 48;

/// STUModelComponent.
const MODEL_COMPONENT: u32 = 0x1918_35F9;
/// STUAnimCategory and its parents.
const CATEGORY_CLASS: u32 = 0xCB5A_89D2;
const PARENTS: u32 = 0x0E5A_577A;
/// An item's game data; the set's model data.
const GAME_DATA_FIELD: u32 = 0x9AD6_CC25;
const SET_MODEL: u32 = 0xD146_7FCA;
/// The game data (`STU_7D00A73D`): its nodes and records.
const GAME_DATA: u32 = 0x7D00_A73D;
const NODES: u32 = 0x9B21_11C5;
const RECORDS: u32 = 0x083D_C038;
/// `STUAnimGameData_AnimationData` and its length.
const RECORD: u32 = 0x65DD_9C84;
const SECONDS: u32 = 0xD851_4AD9;
/// Pass-through and its next node.
const PASS: u32 = 0x30C2_E3AE;
const NEXT: u32 = 0x38CF_6CD3;
/// Embedded data, its flag and its game data.
const EMBEDDED: u32 = 0x331B_7B06;
const EMBEDDED_FLAG: u32 = 0x53A1_B672;
const EMBEDDED_DATA: u32 = 0xEB45_D767;
/// A blend: its flag and thresholds; its children (`m_540E2FC8`, shared
/// with the selector).
const BLEND: u32 = 0x4178_508A;
const BLEND_FLAG: u32 = 0x5E00_9A60;
const THRESHOLDS: u32 = 0x0EC6_7F50;
const CHILDREN: u32 = 0x540E_2FC8;
/// A category selector: its flag, keys and the two fields that make it
/// one this walk doesn't read.
const SELECTOR: u32 = 0x623B_2B19;
const SELECTOR_FLAG: u32 = 0xE605_8AAD;
const SELECTOR_KEYS: u32 = 0x9F7C_A0BB;
const SELECTOR_OTHER: [u32; 2] = [0x4D53_2CFC, 0x53D9_C839];
/// A leaf: its record, its sample row and the row's flag.
const LEAF: u32 = 0x3C8F_B392;
const LEAF_RECORD: u32 = 0x9D16_E038;
const LEAF_ROW: u32 = 0x76E8_D211;
const LEAF_ROW_SET: u32 = 0x3A9A_1E85;
/// The model data (`STU_B38E3226`), its rows (`STU_CE2BEF36`), a row's
/// track (`STU_0AB74EE9`) and the track's times and values.
const MODEL_DATA: u32 = 0xB38E_3226;
const ROWS: u32 = 0xA867_458D;
const ROW: u32 = 0xCE2B_EF36;
const TRACK_FIELD: u32 = 0x3D2F_FAAA;
const TRACK: u32 = 0x0AB7_4EE9;
const TIMES: u32 = 0x4DC4_8800;
const VALUES: u32 = 0x1EB5_B02B;

fn integer(object: &Object, hash: u32) -> i64 {
    match field(object, hash) {
        Some(Value::Int(value)) => *value,
        Some(Value::UInt(value)) => i64::try_from(*value).unwrap_or(i64::MAX),
        _ => 0,
    }
}

fn items(object: &Object, hash: u32) -> &[Value] {
    match field(object, hash) {
        Some(Value::Array(items)) => items,
        _ => &[],
    }
}

fn numbers(object: &Object, hash: u32) -> Option<Vec<f64>> {
    items(object, hash)
        .iter()
        .map(|item| match item {
            Value::Float(value) => Some(*value),
            _ => None,
        })
        .collect()
}

fn indexes(object: &Object, hash: u32) -> Vec<i64> {
    items(object, hash)
        .iter()
        .map(|item| match item {
            Value::Int(value) => *value,
            Value::UInt(value) => i64::try_from(*value).unwrap_or(i64::MAX),
            _ => -1,
        })
        .collect()
}

/// A walk's result: a record, or a selector's branches by key (a branch
/// may give nothing).
#[derive(Clone, Debug, PartialEq)]
enum Tree<R> {
    Record(R),
    Select(Vec<(u64, Option<Tree<R>>)>),
}

/// What a leaf gives: the length or the scalar track.
trait Reading {
    type Record: Clone + PartialEq;

    fn read(&self, data: &Object, leaf: &Object) -> Option<Self::Record>;
}

/// The leaf's record's length in seconds (upstream `primary_record`; the
/// client's `F4CCA0` emits the index, `BB8970` reads the length).
struct Lengths<'a> {
    set: &'a Asset,
}

impl Reading for Lengths<'_> {
    type Record = f32;

    fn read(&self, data: &Object, leaf: &Object) -> Option<f32> {
        let index = usize::try_from(integer(leaf, LEAF_RECORD)).ok()?;
        let record = self.set.resolve(items(data, RECORDS).get(index)?)?;
        if record.class != RECORD {
            return None;
        }
        let seconds = match field(record, SECONDS) {
            Some(Value::Float(seconds)) => *seconds as f32,
            _ => return None,
        };
        (seconds.is_finite() && seconds > 0.0).then_some(seconds)
    }
}

/// A sample row's track (upstream `scalar_track`; with a sample group the
/// client's `F517F0` writes the row index plus one).
#[derive(Clone, Debug, PartialEq)]
struct Track {
    times: Vec<f64>,
    values: Vec<f64>,
}

impl Track {
    const EMPTY: Self = Self {
        times: Vec::new(),
        values: Vec::new(),
    };

    /// The scalar an `Anim` state outputs: 0 for an empty track, the
    /// value of a one-key track, none for a longer one (upstream
    /// `wheel_animation_scalars`).
    fn scalar(&self) -> Option<f64> {
        match self.values[..] {
            [] => Some(0.0),
            [value] => Some(value),
            _ => None,
        }
    }
}

struct Scalars<'a> {
    set: &'a Asset,
}

impl Reading for Scalars<'_> {
    type Record = Track;

    fn read(&self, _data: &Object, leaf: &Object) -> Option<Track> {
        let index = integer(leaf, LEAF_ROW);
        if index < 0 || integer(leaf, LEAF_ROW_SET) == 0 {
            return Some(Track::EMPTY);
        }
        let model = self
            .set
            .root()
            .and_then(|root| field(root, SET_MODEL))
            .and_then(|model| self.set.resolve(model))
            .filter(|model| model.class == MODEL_DATA)?;
        let row = self
            .set
            .resolve(items(model, ROWS).get(usize::try_from(index).ok()?)?)
            .filter(|row| row.class == ROW)?;
        let Some(track) = field(row, TRACK_FIELD).and_then(|track| self.set.resolve(track)) else {
            return Some(Track::EMPTY);
        };
        if track.class != TRACK {
            return None;
        }
        let (times, values) = (numbers(track, TIMES)?, numbers(track, VALUES)?);
        if values.is_empty() {
            return Some(Track::EMPTY);
        }
        if times.len() != values.len()
            || !times.iter().chain(&values).all(|value| value.is_finite())
            || times.windows(2).any(|pair| pair[0] > pair[1])
        {
            return None;
        }
        Some(Track { times, values })
    }
}

/// The node the walk starts from: node 0, through pass-through nodes
/// (upstream `first_animation_node`: the client's `F4EA30` child before
/// its IK output).
fn first_node<'a>(set: &'a Asset, data: Option<&'a Object>) -> Option<&'a Object> {
    let nodes = items(data.filter(|data| data.class == GAME_DATA)?, NODES);
    let mut index = 0;
    let mut visited = HashSet::new();
    while !nodes.is_empty() && visited.insert(index) {
        let node = set.resolve(&nodes[index])?;
        if node.class != PASS {
            return Some(node);
        }
        index = usize::try_from(integer(node, NEXT)).unwrap_or(0);
        if !(0 < index && index < nodes.len()) {
            break;
        }
    }
    None
}

/// The game data's nodes at `indexes`, all after node 0 and in range.
fn children<'a>(set: &'a Asset, data: &'a Object, indexes: &[i64]) -> Option<Vec<&'a Object>> {
    let nodes = items(data, NODES);
    indexes
        .iter()
        .map(|&index| {
            let index = usize::try_from(index).ok().filter(|&index| index > 0)?;
            set.resolve(nodes.get(index)?)
        })
        .collect()
}

struct Walk<'a, R> {
    set: &'a Asset,
    reading: &'a R,
    /// Every selector key met (but the default's).
    keys: BTreeSet<u64>,
}

impl<R: Reading> Walk<'_, R> {
    fn leaf(&self, data: &Object, node: &Object) -> Option<R::Record> {
        (node.class == LEAF)
            .then(|| self.reading.read(data, node))
            .flatten()
    }

    /// Upstream `primary_tree`.
    fn tree(
        &mut self,
        data: Option<&Object>,
        node: Option<&Object>,
        depth: usize,
    ) -> Option<Tree<R::Record>> {
        let (data, node) = (data?, node?);
        if depth >= MAX_DEPTH {
            return None;
        }
        match node.class {
            LEAF => self.leaf(data, node).map(Tree::Record),
            EMBEDDED => {
                if integer(node, EMBEDDED_FLAG) != 0 {
                    return None;
                }
                let nested = field(node, EMBEDDED_DATA).and_then(|data| self.set.resolve(data));
                self.tree(nested, first_node(self.set, nested), depth + 1)
            }
            // The client's `F4D310` picks adjacent branches: the length is
            // known only when every branch agrees.
            BLEND if integer(node, BLEND_FLAG) == 1 => {
                let indexes = indexes(node, CHILDREN);
                let stops = numbers(node, THRESHOLDS)?;
                if indexes.is_empty()
                    || indexes.len() != stops.len()
                    || !stops.iter().all(|stop| stop.is_finite())
                    || stops.windows(2).any(|pair| pair[0] >= pair[1])
                {
                    return None;
                }
                let mut records = children(self.set, data, &indexes)?
                    .into_iter()
                    .map(|child| self.leaf(data, child));
                let first = records.next()?;
                records
                    .all(|record| record == first)
                    .then_some(first)
                    .flatten()
                    .map(Tree::Record)
            }
            SELECTOR if integer(node, SELECTOR_FLAG) == 1 => {
                let keys: Vec<u64> = indexes(node, SELECTOR_KEYS)
                    .into_iter()
                    .map(|key| u64::try_from(key).unwrap_or(u64::MAX))
                    .collect();
                let indexes = indexes(node, CHILDREN);
                let (&last, listed) = keys.split_last()?;
                if keys.len() != indexes.len()
                    || SELECTOR_OTHER
                        .iter()
                        .any(|&hash| !items(node, hash).is_empty())
                    || last != 0
                    || listed
                        .iter()
                        .any(|&key| !(0 < key && key <= 0xFFFF_FFFF && key & 0x7FFF_FFFF != 0))
                {
                    return None;
                }
                let children = children(self.set, data, &indexes)?;
                self.keys.extend(listed);
                let branches = keys
                    .iter()
                    .zip(children)
                    .map(|(&key, child)| (key, self.tree(Some(data), Some(child), depth + 1)))
                    .collect();
                Some(Tree::Select(branches))
            }
            _ => None,
        }
    }
}

/// The branch a selector takes for `category`, whose parents are
/// `parents` (upstream `select_primary`).
fn select<R: Clone>(mut tree: &Tree<R>, category: u64, parents: &BTreeSet<u64>) -> Option<R> {
    loop {
        match tree {
            Tree::Record(record) => return Some(record.clone()),
            Tree::Select(branches) => {
                let (default, listed) = branches.split_last()?;
                let choice = listed
                    .iter()
                    .find(|(key, _)| {
                        let index = key & 0x7FFF_FFFF;
                        category == CATEGORY_TYPE | index
                            || (key & 0x8000_0000 == 0 && parents.contains(&index))
                    })
                    .unwrap_or(default);
                tree = choice.1.as_ref()?;
            }
        }
    }
}

/// An item's records by category ([`ANY`] for every category); upstream
/// `animation_records`.
fn records<R: Reading>(
    set: &Asset,
    item: &Object,
    categories: &BTreeMap<u64, BTreeSet<u64>>,
    reading: &R,
) -> BTreeMap<u64, R::Record> {
    let data = field(item, GAME_DATA_FIELD).and_then(|data| set.resolve(data));
    let mut walk = Walk {
        set,
        reading,
        keys: BTreeSet::new(),
    };
    let Some(tree) = walk.tree(data, first_node(set, data), 0) else {
        return BTreeMap::new();
    };
    if let Tree::Record(record) = tree {
        return BTreeMap::from([(ANY, record)]);
    }
    // Strict selectors give their default for every category they don't
    // list; the others need each category's parents.
    let none = BTreeSet::new();
    let strict = walk.keys.iter().all(|key| key & 0x8000_0000 != 0);
    let mut candidates = BTreeMap::from([(0, &none)]);
    if strict {
        candidates.extend(
            walk.keys
                .iter()
                .map(|key| (CATEGORY_TYPE | (key & 0x7FFF_FFFF), &none)),
        );
    } else {
        candidates.extend(
            categories
                .iter()
                .map(|(&category, parents)| (category, parents)),
        );
    }
    let mut result: BTreeMap<u64, R::Record> = candidates
        .iter()
        .filter_map(|(&category, parents)| Some((category, select(&tree, category, parents)?)))
        .collect();
    if strict && result.len() == candidates.len() {
        result.insert(ANY, result[&0].clone());
    }
    result
}

fn category_key(category: u64) -> String {
    if category == ANY {
        "any".into()
    } else {
        guid_hex(category)
    }
}

/// The aliases of the set: each one's lengths and scalars (see the module
/// notes).
fn set_table(
    set: &Asset,
    categories: &BTreeMap<u64, BTreeSet<u64>>,
) -> Map<String, serde_json::Value> {
    let mut table = Map::new();
    let Some(root) = set.root() else {
        return table;
    };
    let aliases = keys(Some(root), ALIASES);
    let items = items(root, ITEMS);
    if aliases.len() != items.len() {
        return table;
    }
    for (alias, item) in aliases.into_iter().zip(items) {
        let Some(item) = set.resolve(item) else {
            continue;
        };
        if alias >> 48 != 0x0C80 || alias & 0xFFFF_FFFF == 0 {
            continue;
        }
        let lengths: Map<_, _> = records(set, item, categories, &Lengths { set })
            .into_iter()
            .map(|(category, seconds)| (category_key(category), float(f64::from(seconds))))
            .collect();
        let scalars: Map<_, _> = records(set, item, categories, &Scalars { set })
            .into_iter()
            .map(|(category, track)| {
                let scalar = track.scalar().map_or(serde_json::Value::Null, float);
                (category_key(category), scalar)
            })
            .collect();
        let mut entry = Map::new();
        if !lengths.is_empty() {
            entry.insert("lengths".into(), lengths.into());
        }
        if !scalars.is_empty() {
            entry.insert("scalars".into(), scalars.into());
        }
        if !entry.is_empty() {
            table.insert(guid_hex(alias), entry.into());
        }
    }
    table
}

/// Every animation category of the install and the parent indexes it
/// lists (upstream `load_category_parents`).
fn categories(install: &Install) -> Result<BTreeMap<u64, BTreeSet<u64>>> {
    let guids = install.guids(CATEGORY);
    let mut parents = BTreeMap::new();
    for (guid, asset) in install.stu_all(&guids) {
        let asset = asset?;
        let root = asset
            .root()
            .filter(|root| root.class == CATEGORY_CLASS)
            .ok_or_else(|| asset_error(format!("{}: not an animation category", guid_hex(guid))))?;
        let listed = indexes(root, PARENTS)
            .into_iter()
            .filter_map(|index| u64::try_from(index).ok())
            .collect();
        parents.insert(CATEGORY_TYPE | (guid & INDEX), listed);
    }
    if parents.is_empty() {
        return Err(asset_error("the install has no animation categories"));
    }
    Ok(parents)
}

/// The body's one blend tree set, if it names exactly one.
fn body_set(body: &Asset) -> Option<u64> {
    let sets: BTreeSet<u64> = body
        .instances
        .iter()
        .flatten()
        .filter(|object| object.class == MODEL_COMPONENT)
        .filter_map(|component| match component.get("m_animBlendTreeSet") {
            Some(Value::Guid(set)) if *set != 0 => Some(*set),
            _ => None,
        })
        .collect();
    let mut sets = sets.into_iter();
    match (sets.next(), sets.next()) {
        (Some(set), None) => Some(set),
        _ => None,
    }
}

pub(crate) fn derive(install: &Install, out: &Path) -> Result<Vec<Output>> {
    let assets = Assets::of(install, ENTITY);
    let mut bodies_table = BTreeMap::new();
    for record in bodies(out)? {
        let key = record
            .get("body")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| asset_error(format!("a hero without a body: {record}")))?;
        let Some(asset) = assets.read(install, parse_guid(key)?)? else {
            continue;
        };
        if let Some(set) = body_set(&asset) {
            bodies_table.insert(key.to_owned(), set);
        }
    }
    let categories = categories(install)?;
    let mut sets = BTreeMap::new();
    for &set in bodies_table.values() {
        if sets.contains_key(&set) {
            continue;
        }
        let asset = install.stu(set)?;
        sets.insert(set, set_table(&asset, &categories));
    }
    let bodies_json: Map<_, _> = bodies_table
        .into_iter()
        .map(|(body, set)| (body, json!(guid_hex(set))))
        .collect();
    let sets_json: Map<_, _> = sets
        .into_iter()
        .map(|(set, table)| (guid_hex(set), table.into()))
        .collect();
    let file = json!({"bodies": bodies_json, "sets": sets_json});
    Ok(vec![Output {
        path: "data/game_anim_lengths_174.json".into(),
        bytes: serde_json::to_vec(&file).expect("JSON values serialize"),
    }])
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALIAS: u64 = 0x0C80_0000_0000_043C;

    fn object(class: u32, fields: Vec<(u32, Value)>) -> Option<Object> {
        Some(Object { class, fields })
    }

    fn uints(values: &[u64]) -> Value {
        Value::Array(values.iter().map(|&value| Value::UInt(value)).collect())
    }

    fn record(seconds: f64) -> Value {
        Value::Object(Object {
            class: RECORD,
            fields: vec![(SECONDS, Value::Float(seconds))],
        })
    }

    fn leaf(record: i64) -> Option<Object> {
        object(LEAF, vec![(LEAF_RECORD, Value::Int(record))])
    }

    /// A set binding [`ALIAS`] to game data (instance 1) with these nodes
    /// (instances 2…) and records.
    fn set(nodes: Vec<Option<Object>>, records: Vec<Value>) -> Asset {
        let count = nodes.len();
        let mut instances = vec![
            object(
                0xFC47_A2ED,
                vec![
                    (ALIASES, uints(&[ALIAS])),
                    (
                        ITEMS,
                        Value::Array(vec![Value::Object(Object {
                            class: 0xDDD1_8945,
                            fields: vec![(GAME_DATA_FIELD, Value::Instance(1))],
                        })]),
                    ),
                ],
            ),
            object(
                GAME_DATA,
                vec![
                    (
                        NODES,
                        Value::Array((2..2 + count).map(Value::Instance).collect()),
                    ),
                    (RECORDS, Value::Array(records)),
                ],
            ),
        ];
        instances.extend(nodes);
        Asset { instances }
    }

    fn table(asset: &Asset, categories: &BTreeMap<u64, BTreeSet<u64>>) -> serde_json::Value {
        set_table(asset, categories)
            .remove(&guid_hex(ALIAS))
            .unwrap_or_default()
    }

    fn lengths(asset: &Asset) -> serde_json::Value {
        table(asset, &BTreeMap::new())["lengths"].clone()
    }

    fn selector(keys: &[u64], children: &[u64]) -> Option<Object> {
        object(
            SELECTOR,
            vec![
                (SELECTOR_FLAG, Value::UInt(1)),
                (SELECTOR_KEYS, uints(keys)),
                (CHILDREN, uints(children)),
            ],
        )
    }

    /// Pass-through nodes lead to the leaf; its record's length holds for
    /// every category.
    #[test]
    fn a_leaf_past_pass_through_nodes_holds_for_every_category() {
        let pass = |next| object(PASS, vec![(NEXT, Value::UInt(next))]);
        let asset = set(
            vec![pass(2), leaf(1), pass(1)],
            vec![record(9.0), record(2.5)],
        );
        assert_eq!(lengths(&asset), json!({"any": 2.5}));
        // A loop of pass-through nodes gives nothing.
        let asset = set(vec![pass(1), pass(0)], vec![record(2.5)]);
        assert_eq!(table(&asset, &BTreeMap::new()), serde_json::Value::Null);
    }

    /// Embedded data runs its own game data, whose records it reads.
    #[test]
    fn embedded_data_reads_its_own_records() {
        let mut asset = set(
            vec![object(
                EMBEDDED,
                vec![
                    (EMBEDDED_FLAG, Value::UInt(0)),
                    (EMBEDDED_DATA, Value::Instance(3)),
                ],
            )],
            vec![record(9.0)],
        );
        asset.instances.push(object(
            GAME_DATA,
            vec![
                (NODES, Value::Array(vec![Value::Instance(4)])),
                (RECORDS, Value::Array(vec![record(1.5)])),
            ],
        ));
        asset.instances.push(leaf(0));
        assert_eq!(lengths(&asset), json!({"any": 1.5}));
    }

    /// Upstream: a blend's length counts only when every branch agrees.
    #[test]
    fn a_blend_has_a_length_only_when_its_branches_agree() {
        let blend = |thresholds: &[f64]| {
            object(
                BLEND,
                vec![
                    (BLEND_FLAG, Value::UInt(1)),
                    (CHILDREN, uints(&[1, 2])),
                    (
                        THRESHOLDS,
                        Value::Array(thresholds.iter().map(|&t| Value::Float(t)).collect()),
                    ),
                ],
            )
        };
        let agree = set(
            vec![blend(&[0.0, 1.0]), leaf(0), leaf(1)],
            vec![record(2.0), record(2.0)],
        );
        assert_eq!(lengths(&agree), json!({"any": 2.0}));
        let differ = set(
            vec![blend(&[0.0, 1.0]), leaf(0), leaf(1)],
            vec![record(2.0), record(3.0)],
        );
        // The leaves' scalars (no sample row: 0) still agree.
        assert_eq!(
            table(&differ, &BTreeMap::new()),
            json!({"scalars": {"any": 0.0}})
        );
        let unordered = set(
            vec![blend(&[1.0, 1.0]), leaf(0), leaf(1)],
            vec![record(2.0), record(2.0)],
        );
        assert_eq!(lengths(&unordered), serde_json::Value::Null);
    }

    /// A strict selector (top-bit keys) lists its categories, no category
    /// (its default) and, as all resolve, every category.
    #[test]
    fn a_strict_selector_lists_its_categories_and_every_category() {
        let asset = set(
            vec![selector(&[0x8000_0005, 0], &[1, 2]), leaf(0), leaf(1)],
            vec![record(1.0), record(2.0)],
        );
        assert_eq!(
            lengths(&asset),
            json!({"0x0000000000000000": 2.0, "0x0280000000000005": 1.0, "any": 2.0})
        );
    }

    /// Without the top bit a key also matches the categories whose parents
    /// list it; each category of the install is listed, `any` is not.
    #[test]
    fn an_inheriting_selector_lists_every_category_by_its_parents() {
        let asset = set(
            vec![selector(&[7, 0], &[1, 2]), leaf(0), leaf(1)],
            vec![record(1.0), record(2.0)],
        );
        let categories = BTreeMap::from([
            (CATEGORY_TYPE | 3, BTreeSet::new()),
            (CATEGORY_TYPE | 7, BTreeSet::new()),
            (CATEGORY_TYPE | 9, BTreeSet::from([7])),
        ]);
        assert_eq!(
            table(&asset, &categories)["lengths"],
            json!({
                "0x0000000000000000": 2.0,
                "0x0280000000000003": 2.0,
                "0x0280000000000007": 1.0,
                "0x0280000000000009": 1.0,
            })
        );
    }

    /// The leaf's sample row: 0 without one, the value of a one-key track,
    /// `null` for a longer one.
    #[test]
    fn the_scalar_is_a_constant_track() {
        let row = |values: &[f64]| {
            let floats =
                |values: &[f64]| Value::Array(values.iter().map(|&v| Value::Float(v)).collect());
            let times: Vec<f64> = (0..values.len()).map(|t| t as f64).collect();
            Value::Object(Object {
                class: ROW,
                fields: vec![(
                    TRACK_FIELD,
                    Value::Object(Object {
                        class: TRACK,
                        fields: vec![(TIMES, floats(&times)), (VALUES, floats(values))],
                    }),
                )],
            })
        };
        let sampled = |row: i64| {
            object(
                LEAF,
                vec![
                    (LEAF_RECORD, Value::Int(0)),
                    (LEAF_ROW, Value::Int(row)),
                    (LEAF_ROW_SET, Value::UInt(1)),
                ],
            )
        };
        let mut asset = set(
            vec![
                selector(&[0x8000_0001, 0x8000_0002, 0], &[1, 2, 3]),
                sampled(0),
                sampled(1),
                leaf(0),
            ],
            vec![record(1.0)],
        );
        let model = asset.instances.len();
        asset.instances.push(object(
            MODEL_DATA,
            vec![(ROWS, Value::Array(vec![row(&[0.5]), row(&[0.5, 1.0])]))],
        ));
        asset.instances[0]
            .as_mut()
            .unwrap()
            .fields
            .push((SET_MODEL, Value::Instance(model)));
        assert_eq!(
            table(&asset, &BTreeMap::new())["scalars"],
            json!({
                "0x0000000000000000": 0.0,
                "0x0280000000000001": 0.5,
                "0x0280000000000002": null,
                "any": 0.0,
            })
        );
    }
}
