use super::*;
use crate::test_catalog::*;

fn owned(profile: &Profile) -> Vec<u64> {
    let mut guids: Vec<_> = profile.unlocked_guids().into_iter().collect();
    guids.sort_unstable();
    guids
}

#[test]
fn products_cost_credits_league_tokens_or_competitive_points() {
    let collection = collection();
    let shop = Shop::new(&collection);
    let currency = |guid| {
        shop.product(guid)
            .map(|product| (product.price, product.currency))
    };
    assert_eq!(currency(EPIC), Some((250, Currency::Credits)));
    assert_eq!(currency(HOME), Some((100, Currency::LeagueTokens)));
    assert_eq!(currency(GOLDEN), Some((3000, Currency::CompPoints)));
    assert_eq!(currency(SPRAY), Some((25, Currency::Credits)));
    // No price: never sold.
    assert_eq!(currency(ICON), None);
    assert_eq!(currency(REWARD), None);
}

#[test]
fn a_purchase_charges_its_currency_and_brings_the_team_partner() {
    let collection = collection();
    let shop = Shop::new(&collection);
    let mut profile = Profile::default();
    let receipt = shop.purchase(&mut profile, HOME, &collection).unwrap();
    assert_eq!(
        receipt,
        Receipt {
            guid: HOME,
            price: 100,
            currency: Currency::LeagueTokens,
            also: vec![AWAY],
        }
    );
    assert_eq!(profile.league_tokens, 1000 - 100);
    assert_eq!(profile.credits, 50_000);
    assert_eq!(owned(&profile), [HOME, AWAY]);
    assert_eq!(
        profile.unlocked_items,
        [
            Json::String(format!("0x{HOME:016X}")),
            Json::String(format!("0x{AWAY:016X}"))
        ]
    );

    // The partner is owned now, so it cannot be bought, nor come again.
    assert_eq!(
        shop.purchase(&mut profile, AWAY, &collection)
            .unwrap_err()
            .code,
        "already_owned"
    );
}

#[test]
fn refusals_leave_the_profile_alone() {
    let collection = collection();
    let shop = Shop::new(&collection);
    let mut profile = Profile {
        comp_points: 2999,
        ..Profile::default()
    };
    let before = profile.clone();
    for (guid, code, status) in [
        (0, "invalid_input", 400),
        (ICON, "not_purchasable", 400),
        (GOLDEN, "insufficient_balance", 409),
    ] {
        let error = shop.purchase(&mut profile, guid, &collection).unwrap_err();
        assert_eq!((error.code, error.status), (code, status));
    }
    profile.unlock_all = true;
    assert_eq!(
        shop.purchase(&mut profile, EPIC, &collection).unwrap_err(),
        ShopError {
            code: "already_owned",
            message: "You already have this item.",
            status: 409
        }
    );
    profile.unlock_all = false;
    assert_eq!(profile, before);
}

#[test]
fn missing_team_partners_are_added_once() {
    let collection = collection();
    let mut profile = Profile {
        unlocked_items: vec![Json::String(format!("0x{AWAY:016X}"))],
        ..Profile::default()
    };
    assert_eq!(Shop::add_missing_pairs(&mut profile, &collection), [HOME]);
    assert_eq!(owned(&profile), [HOME, AWAY]);
    assert!(Shop::add_missing_pairs(&mut profile, &collection).is_empty());
    let mut everything = Profile {
        unlock_all: true,
        ..Profile::default()
    };
    assert!(Shop::add_missing_pairs(&mut everything, &collection).is_empty());
}
