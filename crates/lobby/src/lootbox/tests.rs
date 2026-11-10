use std::collections::VecDeque;

use serde_json::json;

use super::*;
use crate::{
    accounts::LootBox,
    test_catalog::{
        COMMON, EPIC, HALLOWEEN, HERO, ICON, LEGENDARY, RARE, SPRAY, collection, economy,
    },
};
use orpheus_wire::jam::codec::Schemas;

/// Rolls given in advance: `unit` for Python's `random.random()`, `picks`
/// for `random.choice` and the shuffle's `randbelow`.
struct Script {
    units: VecDeque<f64>,
    picks: VecDeque<usize>,
}

impl Script {
    fn new(units: &[f64], picks: &[usize]) -> Self {
        Self {
            units: units.iter().copied().collect(),
            picks: picks.iter().copied().collect(),
        }
    }

    fn done(&self) -> bool {
        self.units.is_empty() && self.picks.is_empty()
    }
}

impl Dice for Script {
    fn unit(&mut self) -> io::Result<f64> {
        Ok(self.units.pop_front().expect("a scripted roll"))
    }

    fn below(&mut self, len: usize) -> io::Result<usize> {
        let pick = self.picks.pop_front().expect("a scripted pick");
        assert!(pick < len, "pick {pick} of {len}");
        Ok(pick)
    }
}

fn profile_with(boxes: &[(u64, i32)]) -> Profile {
    Profile {
        credits: 1000,
        next_box_id: 100,
        loot_boxes: boxes
            .iter()
            .map(|&(id, box_type)| LootBox {
                id,
                box_type,
                name: crate::accounts::box_name(box_type),
            })
            .collect(),
        ..Profile::default()
    }
}

fn unlocked(profile: &Profile) -> Vec<u64> {
    let mut guids: Vec<_> = profile.unlocked_guids().into_iter().collect();
    guids.sort_unstable();
    guids
}

#[test]
fn a_standard_box_rolls_rarities_credits_and_duplicates() {
    let collection = collection();
    let loot = LootBoxes::new(&collection, economy());
    let mut profile = profile_with(&[(7, 0), (8, 0), (9, 0), (10, 0), (11, 0)]);
    profile.unlocked_items.push(json!("0x0250000000000103"));
    let mut dice = Script::new(
        // guaranteed rare, then legendary, epic, common; slot credits:
        // rare yes, legendary no, epic no
        &[0.5, 0.0, 0.03, 0.9, 0.01, 0.9, 0.9],
        // shuffle keeps the order; legendary, epic, then the third common
        &[3, 2, 1, 0, 0, 2],
    );
    let opening = loot
        .open(7, &mut profile, &collection, true, &mut dice)
        .unwrap();
    assert!(dice.done());
    assert_eq!(opening.box_name, "Standard");
    let summary: Vec<_> = opening
        .drops
        .iter()
        .map(|drop| {
            (
                drop.unlock,
                drop.rarity.as_str(),
                drop.credits,
                drop.duplicate,
                drop.new,
            )
        })
        .collect();
    assert_eq!(
        summary,
        [
            (0x0250_0000_0000_088b, "Rare", 0, false, false),
            (LEGENDARY, "Legendary", 0, false, true),
            (EPIC, "Epic", 50, true, false),
            (SPRAY, "Common", 0, false, true),
        ]
    );
    assert_eq!(opening.drops[0].amount, Some(50));
    assert_eq!(opening.drops[1].hero, HERO);
    assert_eq!(opening.drops[3].hero, 0);
    assert_eq!(profile.credits, 1000 + 50 + 50);
    assert_eq!(unlocked(&profile), [EPIC, LEGENDARY, SPRAY]);
    assert_eq!(profile.stats["boxes_opened"], 1);
    assert_eq!(profile.stats["legendaries_dropped"], 1);
    assert_eq!(profile.stats["credits_earned_from_duplicates"], 50);
    // Four boxes left is below the low stock: the starter set comes back.
    assert_eq!(profile.loot_boxes.len(), 4 + 140);
    assert_eq!(profile.loot_boxes[4].id, 100);
}

#[test]
fn an_event_box_draws_its_first_slot_from_the_event_and_never_credits_there() {
    let collection = collection();
    let loot = LootBoxes::new(&collection, economy());
    let mut profile = profile_with(&[(3, 2)]);
    let mut dice = Script::new(
        // guaranteed legendary, three commons; slots 1-3 from the event or not
        &[0.0, 0.9, 0.9, 0.9, 0.4, 0.6, 0.4],
        // shuffle moves the legendary last; then each slot's pick
        &[0, 2, 1, 0, 0, 0, 0],
    );
    let opening = loot
        .open(3, &mut profile, &collection, false, &mut dice)
        .unwrap();
    assert!(dice.done());
    assert_eq!(opening.box_name, "Halloween Terror");
    let summary: Vec<_> = opening
        .drops
        .iter()
        .map(|drop| (drop.unlock, drop.highlight, drop.order))
        .collect();
    assert_eq!(
        summary,
        [
            (COMMON, 2, 0),
            (ICON, 2, 1),
            (SPRAY, 0, 2),
            (HALLOWEEN, 2, 3)
        ]
    );
    assert!(profile.loot_boxes.is_empty(), "no refill");
}

#[test]
fn a_legendary_box_keeps_its_promised_legendary_and_may_add_coins() {
    let collection = collection();
    let loot = LootBoxes::new(&collection, economy());
    let mut profile = profile_with(&[(1, 7)]);
    let mut dice = Script::new(
        // three ordinary legendaries; the promised slot rolls no credits,
        // the next gives the coins, the rest roll none
        &[0.0, 0.0, 0.0, 0.1, 0.9, 0.9],
        &[0, 0, 0, 0, 0, 0],
    );
    let opening = loot
        .open(1, &mut profile, &collection, false, &mut dice)
        .unwrap();
    assert!(dice.done());
    let amounts: Vec<_> = opening.drops.iter().map(|drop| drop.amount).collect();
    assert_eq!(amounts, [None, Some(500), None, None]);
    // Every legendary is owned after the first: the others are duplicates.
    assert_eq!(opening.drops[0].unlock, LEGENDARY);
    assert!(opening.drops[2].duplicate && opening.drops[3].duplicate);
    assert_eq!(profile.credits, 1000 + 500 + 2 * 200);
}

#[test]
fn a_hero_box_holds_only_its_heros_items_and_an_unknown_box_is_standard() {
    let collection = collection();
    let loot = LootBoxes::new(&collection, economy());
    let mut profile = profile_with(&[(5, 10)]);
    let mut dice = Script::new(&[0.5, 0.9, 0.9, 0.9, 0.9], &[3, 2, 1, 0, 0, 0, 0]);
    let opening = loot
        .open(5, &mut profile, &collection, false, &mut dice)
        .unwrap();
    assert!(dice.done());
    let items: Vec<_> = opening.drops.iter().map(|drop| drop.unlock).collect();
    // Rare first, then commons: the hero's one common skin, then duplicates.
    assert_eq!(items, [RARE, COMMON, COMMON, COMMON]);

    let mut dice = Script::new(&[0.5, 0.9, 0.9, 0.9, 0.9], &[3, 2, 1, 0, 0, 0, 0]);
    let opening = loot
        .open(99, &mut profile, &collection, false, &mut dice)
        .unwrap();
    assert_eq!(opening.box_name, "Standard");
    assert_eq!(opening.drops[1].unlock, ICON);
}

#[test]
fn opening_every_box_reports_the_new_items_without_a_refill() {
    let collection = collection();
    let loot = LootBoxes::new(&collection, economy());
    let mut profile = profile_with(&[(1, 0), (2, 0)]);
    let (opened, granted) = loot
        .open_all(&mut profile, &collection, &mut SystemDice)
        .unwrap();
    assert_eq!(opened, 2);
    assert!(profile.loot_boxes.is_empty());
    assert_eq!(granted.len(), profile.unlocked_guids().len());
    assert_eq!(profile.stats["boxes_opened"], 2);
}

#[test]
fn the_box_messages_match_python() {
    let schemas = Schemas::lobby();
    let drops = [
        Drop {
            hero: HERO,
            unlock: LEGENDARY,
            credits: 0,
            order: 1,
            highlight: 2,
            duplicate: false,
            new: true,
            rarity: "Legendary".into(),
            amount: None,
        },
        Drop {
            hero: 0,
            unlock: 0x0250_0000_0000_088b,
            credits: 0,
            order: 0,
            highlight: 0,
            duplicate: false,
            new: false,
            rarity: "Rare".into(),
            amount: Some(50),
        },
        Drop {
            hero: HERO,
            unlock: EPIC,
            credits: 50,
            order: 2,
            highlight: 0,
            duplicate: true,
            new: false,
            rarity: "Epic".into(),
            amount: None,
        },
    ];
    let hex = |(crc, id, fields): LobbyMessage| {
        schemas
            .encode(crc, id, &fields)
            .unwrap()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    };
    // ow174.services.lootbox.box_result_message((7, 0), drops), encoded.
    assert_eq!(
        hex(opened_message([7, 0], &drops)),
        "03000000400000000000e00204010000000050020000000001000000020000000200000000000000\
         008b0800000000500200000000000000000000000000400000000000e00203010000000050023200\
         00000200000000000000010700000000000000000000000000000000000000"
    );
    assert_eq!(hex(credits_message(49875)), "d3c20000");
    let request = Fields::from([(
        0x78,
        Struct(Fields::from([(0, Array(vec![Unsigned(7), Unsigned(0)]))])),
    )]);
    assert_eq!(box_id(&request), Some(7));
    assert_eq!(box_id(&Fields::new()), None);
}

/// The credits the install gives are the ones `ow174/services/lootbox.py`
/// holds: `DUPLICATE_CREDITS` and `CREDIT_DROPS`.
#[test]
#[ignore = "requires the game data derived from a 1.74 install (just test-game <install>)"]
fn the_installs_credits_are_pythons() {
    let economy = Economy::load(&orpheus_testkit::runtime_dir().join("data")).unwrap();
    let duplicates =
        ["Common", "Rare", "Epic", "Legendary"].map(|rarity| economy.duplicate_credits(rarity));
    assert_eq!(duplicates, [5, 15, 50, 200]);
    assert_eq!(economy.credit_drop("Common"), None);
    let drops = ["Rare", "Epic", "Legendary"].map(|rarity| {
        let drop = economy.credit_drop(rarity).unwrap();
        (drop.unlock, drop.amount)
    });
    assert_eq!(
        drops,
        [
            (0x0250_0000_0000_088b, 50),
            (0x0250_0000_0000_088c, 150),
            (0x0250_0000_0000_088d, 500)
        ]
    );
}
