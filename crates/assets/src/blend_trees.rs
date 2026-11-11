//! The clips an entity's animation blend trees play when no animation
//! category is set: where the idle pose of a body comes from.
//!
//! A model component names a blend tree set (`021`, `STUAnimBlendTreeSet`).
//! The set lists animation aliases (`m_253EE7C8`, the `0C8…` keys the
//! statescript graphs play) and, in the same order, one item each
//! (`m_blendTreeItems`, `STUAnimBlendTreeSet_BlendTreeItem`): the blend tree
//! (`020`, `m_C0214513`) that plays the alias and the values the item binds
//! to it (`m_F6E6D4B1`, `STU_3B150012`: aliases in `m_69D5B5ED`, clips
//! (`006`) in `m_5AD927D3`, categories in `m_B4975FA6`, …). A shared tree
//! names a value by a slot (`m_63DEDAC0`, counted from 1 into the item's
//! list of that kind; 0 for the value written in the node itself). The set
//! starts from its root alias (`m_rootAnimAliasGUID`).
//!
//! A tree is a graph of nodes from its `m_rootNode`, joined by links. Its
//! switches on an animation category (`STU_D2CD8F5A`, whose
//! `m_categoryDataFlow` is the category the game sets) hold one link per
//! category (`STU_C9E2FF36`, `m_category`) and one without: the branch
//! taken when no category is set. Nodes play an alias (`m_animAlias`, the
//! item of the set for that alias, whose tree runs in turn) or a clip
//! (`m_animation`). State machine transitions (`m_transitions`) only play
//! between states. Nodes driven by a game value (a data flow, `048…`) or an
//! animation slot (`0B6`) name nothing the data resolves.
//!
//! [`idle_clips`] follows every link from the root but the transitions and
//! the categorised branches, through every alias bound in the set, and
//! returns the clips found. Assumption: a body standing idle has no
//! category set (the data does not say which categories the game sets;
//! the Training Bot's graph `0BA8` pushes category `015:007F` in one state).

use std::collections::{BTreeSet, HashMap, HashSet};

use orpheus_stu::{Asset, Object, Value};

use crate::Install;

/// STUAnimBlendTreeSet: the root alias, the aliases and their items.
const ROOT_ALIAS: u32 = 0x8493_5843;
const ALIASES: u32 = 0x253E_E7C8;
const ITEMS: u32 = 0x6AFC_D1A5;
/// An item: its tree and its bound values (aliases, clips).
const TREE: u32 = 0xC021_4513;
const BINDINGS: u32 = 0xF6E6_D4B1;
const BOUND_ALIASES: u32 = 0x69D5_B5ED;
const BOUND_CLIPS: u32 = 0x5AD9_27D3;
/// STUAnimBlendTree: the root node.
const ROOT_NODE: u32 = 0xD649_7916;
/// Node fields: the parent (back up the graph), the transitions, a link's
/// category, an alias, a clip.
const PARENT: u32 = 0x3868_F518;
const TRANSITIONS: u32 = 0x4E86_E083;
const CATEGORY: u32 = 0x2C54_AEAF;
const ALIAS: u32 = 0xB4FF_0E2F;
const ANIMATION: u32 = 0x3F5B_86A4;
/// A value's slot in the item's bindings, and the value written inline
/// (an alias's `m_76158B9E`, a clip's `m_value`).
const SLOT: u32 = 0x63DE_DAC0;
const INLINE_ALIAS: u32 = 0x7615_8B9E;
const VALUE: u32 = 0x07DD_813E;

fn field(object: &Object, hash: u32) -> Option<&Value> {
    object
        .fields
        .iter()
        .find(|(field, _)| *field == hash)
        .map(|(_, value)| value)
}

fn key(value: Option<&Value>) -> u64 {
    match value {
        Some(Value::Guid(key) | Value::UInt(key)) => *key,
        _ => 0,
    }
}

fn keys(object: Option<&Object>, hash: u32) -> Vec<u64> {
    match object.and_then(|object| field(object, hash)) {
        Some(Value::Array(items)) => items.iter().map(|item| key(Some(item))).collect(),
        _ => Vec::new(),
    }
}

/// A slotted value: the item's bound value in that slot, else the inline
/// one.
fn slotted(value: &Object, bound: &[u64], inline: u32) -> u64 {
    match field(value, SLOT) {
        Some(Value::Int(slot)) if *slot > 0 => usize::try_from(*slot - 1)
            .ok()
            .and_then(|index| bound.get(index))
            .copied()
            .unwrap_or(0),
        _ => key(field(value, inline)),
    }
}

struct Walk<'a> {
    install: &'a Install,
    set: &'a Asset,
    items: HashMap<u64, &'a Object>,
    aliases: HashSet<u64>,
    clips: BTreeSet<u64>,
}

impl Walk<'_> {
    fn alias(&mut self, alias: u64) -> Result<(), String> {
        if alias == 0 || !self.aliases.insert(alias) {
            return Ok(());
        }
        // An alias the set binds no tree to plays nothing here.
        let Some(item) = self.items.get(&alias).copied() else {
            return Ok(());
        };
        let bindings = field(item, BINDINGS).and_then(|value| self.set.resolve(value));
        let aliases = keys(bindings, BOUND_ALIASES);
        let clips = keys(bindings, BOUND_CLIPS);
        let tree = key(field(item, TREE));
        if tree == 0 {
            return Ok(());
        }
        let tree = self.install.stu(tree).map_err(|error| error.to_string())?;
        let mut found = Vec::new();
        let mut seen = HashSet::new();
        let mut stack: Vec<&Value> = tree
            .root()
            .and_then(|root| field(root, ROOT_NODE))
            .into_iter()
            .collect();
        while let Some(value) = stack.pop() {
            let object = match value {
                Value::Instance(index) if seen.insert(*index) => tree.resolve(value),
                Value::Object(object) => Some(object),
                Value::Array(items) => {
                    stack.extend(items);
                    None
                }
                _ => None,
            };
            let Some(object) = object else {
                continue;
            };
            if field(object, CATEGORY).is_some_and(|category| *category != Value::Null) {
                continue;
            }
            if let Some(Value::Object(alias)) = field(object, ALIAS) {
                found.push(slotted(alias, &aliases, INLINE_ALIAS));
            }
            if let Some(Value::Object(clip)) = field(object, ANIMATION) {
                let clip = slotted(clip, &clips, VALUE);
                if clip != 0 {
                    self.clips.insert(clip);
                }
            }
            stack.extend(
                object
                    .fields
                    .iter()
                    .filter(|(hash, _)| *hash != PARENT && *hash != TRANSITIONS)
                    .map(|(_, value)| value),
            );
        }
        found.into_iter().try_for_each(|alias| self.alias(alias))
    }
}

/// The clips the set `set` plays with no animation category set, from its
/// root alias (see the module notes).
pub(crate) fn idle_clips(install: &Install, set: u64) -> Result<BTreeSet<u64>, String> {
    let asset = install.stu(set).map_err(|error| error.to_string())?;
    let Some(root) = asset.root() else {
        return Ok(BTreeSet::new());
    };
    let aliases = keys(Some(root), ALIASES);
    let items = match field(root, ITEMS) {
        Some(Value::Array(items)) => items.iter().map(|item| asset.resolve(item)).collect(),
        _ => Vec::new(),
    };
    let mut walk = Walk {
        install,
        set: &asset,
        items: aliases
            .into_iter()
            .zip(items)
            .filter_map(|(alias, item)| Some((alias, item?)))
            .collect(),
        aliases: HashSet::new(),
        clips: BTreeSet::new(),
    };
    walk.alias(key(field(root, ROOT_ALIAS)))?;
    Ok(walk.clips)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The Training Bot's blend tree set (`5A8.021`) and the clips it
    /// names.
    const SET: u64 = 0x0040_0000_0000_05A8;
    const fn clip(index: u64) -> u64 {
        0x0A00_0000_0000_0000 | index
    }

    /// With no category set the Training Bot's trees reach its standing
    /// pose (`3920`, under alias `805A8009`), the hover layer (`391F`) and
    /// a hit layer (`3A9D`), not the clips behind a category: the static
    /// pose of category `0004` (`3A0A`), the arms-forward branch of
    /// category `007F` (`3A07`, `3A08`, `394F`) or the moving branch of
    /// category `0005` (`3990`, `3952`).
    #[test]
    #[ignore = "requires an Overwatch 1.74 install (just test-game <install>)"]
    fn the_training_bot_idles_in_its_uncategorised_branches() {
        let install = Install::open(&orpheus_testkit::game_dir(), "enUS").unwrap();
        let clips = idle_clips(&install, SET).unwrap();
        for reached in [0x3920, 0x391F, 0x3A9D] {
            assert!(clips.contains(&clip(reached)), "{reached:X} {clips:X?}");
        }
        for behind in [0x3A0A, 0x3A07, 0x3A08, 0x394F, 0x3990, 0x3952] {
            assert!(!clips.contains(&clip(behind)), "{behind:X} {clips:X?}");
        }
    }
}
