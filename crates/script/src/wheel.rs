//! The communication wheel's fixed data: its actions, their categories and
//! the slot tables, and what a body has equipped. The callout textures come
//! from the install (`054/01A5`, [`GraphStore::callout_texture`](crate::graph::GraphStore::callout_texture)).
//!
//! Ported from leafall903/ORPHEUS (MIT), commit `c239a84` and later
//! (`src/ow174/game/script/driver.py`, `docs/COMMUNICATION_WHEEL.txt`):
//!
//! - The client sends a wheel choice as ClientOutInstance 21608 (`+0x78`
//!   the action `0A9` GUID, `+0x80` a bool, `+0x84` a marker entity). The
//!   server delivers it to the sender's body as game message `02A3` with
//!   parameters `08FC` (the action) and `03E1` (the bool); `0644` carries a
//!   nonzero marker for action `0170` (ACKNOWLEDGE).
//! - Body graph `004B` starts `05AD` (the main wheel) and `02CF` (sprays).
//!   `05AD` plays `0686` (callouts and voice lines) and `069A` (emotes,
//!   which plays `1040`) on the body itself.
//! - The `STU_4460419A` category test uses the 104319 `.096` action records;
//!   the slot queries use the client's four-entry tables (44A7D0, 44A950,
//!   44A980), -1 for any other action.

/// The type of a wheel action GUID (`.096`).
pub const ACTION_TYPE: u64 = 0x0a90;
/// The game message a wheel choice reaches the body with.
pub const SELECTION_MESSAGE: u64 = 0x0240_0000_0000_02a3;
/// Its parameters: the action, the 21608 bool and the marker entity.
pub const ACTION_PARAMETER: u32 = 0x08fc;
pub const FLAG_PARAMETER: u32 = 0x03e1;
pub const MARKER_PARAMETER: u32 = 0x0644;
/// ACKNOWLEDGE: the only action that reads the marker.
pub const ACKNOWLEDGE: u32 = 0x170;

/// The category enumeration type of `STU_4460419A`.
const CATEGORY_TYPE: u64 = 0x0c88_0000_0000_00cd;
const EMOTE_CATEGORY: u64 = 0x0d80_0000_0000_741e;
const VOICE_CATEGORY: u64 = 0x0d80_0000_0000_741f;
const SPRAY_CATEGORY: u64 = 0x0d80_0000_0000_7420;
const CALLOUT_CATEGORY: u64 = 0x0d80_0000_0000_7428;

/// 44A7D0, 44A980 and 44A950: the zero-based slot of an action.
pub const EMOTE_ACTIONS: [u32; 4] = [0x172, 0x1a1, 0x1a0, 0x19f];
pub const VOICE_ACTIONS: [u32; 4] = [0x16f, 0x19d, 0x19c, 0x19b];
pub const SPRAY_ACTIONS: [u32; 4] = [0x173, 0x1a5, 0x1a4, 0x1a3];

/// The wheel's own self-targeted PlayScript actions: (graph, node) and the
/// graph each plays (upstream `WHEEL_PLAY_GRAPHS`).
pub const PLAY_SCRIPTS: [((u32, u16), u32); 4] = [
    ((0x05ad, 109), 0x069a),
    ((0x05ad, 117), 0x0686),
    ((0x05ad, 131), 0x0686),
    ((0x069a, 9), 0x1040),
];

/// The graphs whose networked actions go to the clients as frame actions:
/// the callouts (`STU_7106C356`) and the effects with a remote index
/// (upstream `replicate_wheel_event`).
pub const ACTION_GRAPHS: [u32; 3] = [0x02cf, 0x05ad, 0x0686];

/// The categories of an action (the `.096` records); none for an action
/// outside the wheel.
fn categories(action: u32) -> impl Iterator<Item = u64> {
    [
        (EMOTE_CATEGORY, matches!(action, 0x172 | 0x19f..=0x1a2)),
        (VOICE_CATEGORY, matches!(action, 0x16f | 0x19b..=0x19e)),
        (SPRAY_CATEGORY, matches!(action, 0x173 | 0x1a3..=0x1a6)),
        (
            CALLOUT_CATEGORY,
            matches!(action, 0x16b..=0x172 | 0x1e5..=0x1f4 | 0x1f7 | 0x1f9),
        ),
    ]
    .into_iter()
    .filter_map(|(category, member)| member.then_some(category))
}

/// The low index of a wheel action GUID.
pub fn action_index(guid: u64) -> Option<u32> {
    (guid >> 48 == ACTION_TYPE && guid & 0xffff_0000_0000 == 0).then_some(guid as u32)
}

/// Whether the client may send this action: it belongs to a wheel
/// category.
pub fn is_action(guid: u64) -> bool {
    action_index(guid).is_some_and(|action| categories(action).next().is_some())
}

/// Sprays need the placement state (`STU_861D9015`, a 4.5 m eye ray), which
/// the server doesn't run yet.
pub fn is_spray(guid: u64) -> bool {
    action_index(guid).is_some_and(|action| categories(action).any(|c| c == SPRAY_CATEGORY))
}

/// `STU_4460419A`: does the action belong to the category (type
/// `0C88…00CD`)? Any other enumeration type answers false.
pub fn has_category(action: u64, category: u64, category_type: u64) -> bool {
    category_type == CATEGORY_TYPE
        && action_index(action).is_some_and(|action| categories(action).any(|c| c == category))
}

/// The slot of an action in a four-entry table; -1 when absent.
pub fn slot(action: u64, table: &[u32; 4]) -> i64 {
    action_index(action)
        .and_then(|action| table.iter().position(|&entry| entry == action))
        .map_or(-1, |slot| slot as i64)
}

/// The graph a wheel PlayScript node plays, if it is one.
pub fn played_graph(graph: u32, node: u16) -> Option<u32> {
    PLAY_SCRIPTS
        .iter()
        .find(|(site, _)| *site == (graph, node))
        .map(|(_, played)| *played)
}

/// The equipped wheel cosmetics of a body, four slots each, zero for an
/// empty slot (the profile's loadout for the hero, upstream
/// `Collection.wheel_cosmetics`).
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Cosmetics {
    /// `.014` animation aliases of the equipped emotes (`STU_250DEABD`).
    pub emote_aliases: [u64; 4],
    /// `.021` blend tree sets of the equipped emotes (`STU_DF7C7C63`).
    pub emote_blend_trees: [u64; 4],
    /// The equipped spray unlocks themselves (`STU_17B517B9`).
    pub spray_unlocks: [u64; 4],
    /// `.00D` effects of the equipped voice lines (`STU_2977339D`).
    pub voice_effects: [u64; 4],
}

/// What a slot query asks for.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Cosmetic {
    EmoteAlias,
    EmoteBlendTree,
    Spray,
    VoiceEffect,
}

impl Cosmetics {
    /// The equipped asset in a slot; zero outside the four slots.
    pub fn equipped(&self, kind: Cosmetic, slot: i64) -> u64 {
        let slots = match kind {
            Cosmetic::EmoteAlias => &self.emote_aliases,
            Cosmetic::EmoteBlendTree => &self.emote_blend_trees,
            Cosmetic::Spray => &self.spray_unlocks,
            Cosmetic::VoiceEffect => &self.voice_effects,
        };
        usize::try_from(slot)
            .ok()
            .and_then(|slot| slots.get(slot))
            .copied()
            .unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const fn action(index: u64) -> u64 {
        ACTION_TYPE << 48 | index
    }

    #[test]
    fn categories_follow_the_action_records() {
        // 172 is both an emote and a main-wheel action.
        assert!(has_category(action(0x172), EMOTE_CATEGORY, CATEGORY_TYPE));
        assert!(has_category(action(0x172), CALLOUT_CATEGORY, CATEGORY_TYPE));
        assert!(has_category(action(0x16c), CALLOUT_CATEGORY, CATEGORY_TYPE));
        assert!(!has_category(action(0x16c), VOICE_CATEGORY, CATEGORY_TYPE));
        assert!(has_category(action(0x19e), VOICE_CATEGORY, CATEGORY_TYPE));
        assert!(!has_category(action(0x16c), CALLOUT_CATEGORY, 0));
        assert!(!is_action(action(0x1f5)));
        assert!(!is_action(0x0240_0000_0000_016c));
        assert!(is_spray(action(0x1a4)));
    }

    #[test]
    fn slots_are_zero_based_and_menu_labels_have_none() {
        assert_eq!(slot(action(0x1a1), &EMOTE_ACTIONS), 1);
        assert_eq!(slot(action(0x16f), &VOICE_ACTIONS), 0);
        assert_eq!(slot(action(0x19e), &VOICE_ACTIONS), -1);
        assert_eq!(slot(action(0x1a3), &SPRAY_ACTIONS), 3);
        let cosmetics = Cosmetics {
            spray_unlocks: [1, 2, 3, 4],
            ..Cosmetics::default()
        };
        assert_eq!(cosmetics.equipped(Cosmetic::Spray, 2), 3);
        assert_eq!(cosmetics.equipped(Cosmetic::Spray, -1), 0);
    }
}
