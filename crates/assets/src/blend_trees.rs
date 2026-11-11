//! The clips an entity's animation blend trees play: when no animation
//! category is set (where the idle pose of a body comes from) and for a
//! set of categories (where a bot's animated branches come from).
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
//!
//! [`TreeSet::branch`] walks one alias for a set of categories and keeps
//! the clips in the order met, with the rules its notes give: the
//! categorised branches set, layers without a game-driven weight, a state
//! machine's first state, a sequence's order.

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
/// A category switch's category (`m_categoryDataFlow`), a node's children,
/// a layer's weight (`m_weightDriver`), the item's bound categories.
const CATEGORY_FLOW: u32 = 0xD32F_A452;
const CHILDREN: u32 = 0x134E_E5BB;
const WEIGHT_DRIVER: u32 = 0x0F88_291F;
const BOUND_CATEGORIES: u32 = 0xB497_5FA6;
/// Node classes: a layer (`STUAnimNode_LayerChild`), a state machine and a
/// sequence.
const LAYER: u32 = 0x4027_4C18;
const STATE_MACHINE: u32 = 0xBB7A_7240;
const SEQUENCE: u32 = 0xF738_4072;

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

/// A clip a branch plays, in the order the walk meets it; `once` when a
/// sequence plays it before its last child (see [`TreeSet::branch`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Played {
    pub clip: u64,
    pub once: bool,
}

/// A blend tree set read once: its aliases, the item of each and every
/// item's tree.
pub(crate) struct TreeSet {
    set: Asset,
    root: u64,
    aliases: Vec<u64>,
    items: HashMap<u64, Value>,
    trees: HashMap<u64, Asset>,
}

/// The values an item binds to its tree's slots.
struct Bound {
    aliases: Vec<u64>,
    clips: Vec<u64>,
    categories: Vec<u64>,
}

impl TreeSet {
    pub(crate) fn read(install: &Install, set: u64) -> Result<Self, String> {
        let asset = install.stu(set).map_err(|error| error.to_string())?;
        let root = asset.root().ok_or("an empty blend tree set")?;
        let aliases = keys(Some(root), ALIASES);
        let items: HashMap<u64, Value> = match field(root, ITEMS) {
            Some(Value::Array(items)) => {
                aliases.iter().copied().zip(items.iter().cloned()).collect()
            }
            _ => HashMap::new(),
        };
        let mut trees = HashMap::new();
        for item in items.values().filter_map(|item| asset.resolve(item)) {
            let tree = key(field(item, TREE));
            if tree != 0 && !trees.contains_key(&tree) {
                trees.insert(tree, install.stu(tree).map_err(|error| error.to_string())?);
            }
        }
        Ok(Self {
            root: key(field(root, ROOT_ALIAS)),
            set: asset,
            aliases,
            items,
            trees,
        })
    }

    /// The set's root alias (`m_rootAnimAliasGUID`).
    pub(crate) fn root(&self) -> u64 {
        self.root
    }

    /// The aliases the set binds, in its order.
    pub(crate) fn aliases(&self) -> &[u64] {
        &self.aliases
    }

    /// The clips alias `alias` plays while the game sets `categories`, in
    /// the order the walk meets them (duplicates kept). From the alias's
    /// tree root it follows every link but the parent and the transitions,
    /// as [`idle_clips`] does, and through every alias it meets, with
    /// these rules (each an assumption; the data names the nodes, not how
    /// the client runs them):
    ///
    /// - a category switch takes its branches whose category (bound by the
    ///   item's slot, `m_B4975FA6`) is set, else its uncategorised one; a
    ///   branch whose slot binds no category is never taken;
    /// - a layer weighted by a game value (`STUAnimNode_LayerChild` with an
    ///   `m_weightDriver`) adds nothing: the server computes no data flows
    ///   (on the Training Bot these are the hit layer `3A9D`, the moving
    ///   bob `3951` and the arms-forward layer `39FE`);
    /// - a state machine stays in its first state (the trees' idle and
    ///   blink machines start there);
    /// - a sequence plays its children in order, all but the last once.
    pub(crate) fn branch(&self, alias: u64, categories: &[u64]) -> Vec<Played> {
        let mut walk = Branch {
            set: self,
            categories,
            played: Vec::new(),
            path: Vec::new(),
        };
        walk.alias(alias, false);
        walk.played
    }
}

struct Branch<'a> {
    set: &'a TreeSet,
    categories: &'a [u64],
    played: Vec<Played>,
    /// The aliases being walked, so a tree naming itself ends.
    path: Vec<u64>,
}

impl Branch<'_> {
    fn alias(&mut self, alias: u64, once: bool) {
        if alias == 0 || self.path.contains(&alias) {
            return;
        }
        let set = &self.set.set;
        let Some(item) = self
            .set
            .items
            .get(&alias)
            .and_then(|item| set.resolve(item))
        else {
            return;
        };
        let bindings = field(item, BINDINGS).and_then(|value| set.resolve(value));
        let bound = Bound {
            aliases: keys(bindings, BOUND_ALIASES),
            clips: keys(bindings, BOUND_CLIPS),
            categories: keys(bindings, BOUND_CATEGORIES),
        };
        let Some(tree) = self.set.trees.get(&key(field(item, TREE))) else {
            return;
        };
        let Some(root) = tree.root().and_then(|root| field(root, ROOT_NODE)) else {
            return;
        };
        self.path.push(alias);
        self.value(tree, root, &bound, once, &mut HashSet::new());
        self.path.pop();
    }

    fn value(
        &mut self,
        tree: &Asset,
        value: &Value,
        bound: &Bound,
        once: bool,
        seen: &mut HashSet<usize>,
    ) {
        match value {
            Value::Instance(index) if seen.insert(*index) => {
                if let Some(object) = tree.resolve(value) {
                    self.node(tree, object, bound, once, seen);
                }
            }
            Value::Object(object) => self.node(tree, object, bound, once, seen),
            Value::Array(items) => {
                for item in items {
                    self.value(tree, item, bound, once, seen);
                }
            }
            _ => {}
        }
    }

    fn node(
        &mut self,
        tree: &Asset,
        object: &Object,
        bound: &Bound,
        once: bool,
        seen: &mut HashSet<usize>,
    ) {
        if object.class == LAYER && field(object, WEIGHT_DRIVER).is_some_and(|d| *d != Value::Null)
        {
            return;
        }
        if let Some(Value::Object(alias)) = field(object, ALIAS) {
            self.alias(slotted(alias, &bound.aliases, INLINE_ALIAS), once);
        }
        if let Some(Value::Object(clip)) = field(object, ANIMATION) {
            let clip = slotted(clip, &bound.clips, VALUE);
            if clip != 0 {
                self.played.push(Played { clip, once });
            }
        }
        let children = match field(object, CHILDREN) {
            Some(Value::Array(children)) => children.as_slice(),
            _ => &[],
        };
        if field(object, CATEGORY_FLOW).is_some() {
            let category = |child: &Value| match tree
                .resolve(child)
                .and_then(|c| field(c, CATEGORY))
            {
                Some(Value::Object(category)) => Some(slotted(category, &bound.categories, VALUE)),
                Some(Value::Guid(category)) => Some(*category),
                _ => None,
            };
            let set: Vec<&Value> = children
                .iter()
                .filter(|child| {
                    category(child).is_some_and(|c| c != 0 && self.categories.contains(&c))
                })
                .collect();
            let taken = if set.is_empty() {
                children
                    .iter()
                    .filter(|child| category(child).is_none())
                    .collect()
            } else {
                set
            };
            for child in taken {
                self.value(tree, child, bound, once, seen);
            }
        } else if object.class == STATE_MACHINE {
            if let Some(first) = children.first() {
                self.value(tree, first, bound, once, seen);
            }
        } else {
            let sequence = object.class == SEQUENCE;
            for (index, child) in children.iter().enumerate() {
                let before_last = sequence && index + 1 < children.len();
                self.value(tree, child, bound, once || before_last, seen);
            }
        }
        for (hash, value) in &object.fields {
            if !matches!(*hash, PARENT | TRANSITIONS | CHILDREN | ALIAS | ANIMATION) {
                self.value(tree, value, bound, once, seen);
            }
        }
    }
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
