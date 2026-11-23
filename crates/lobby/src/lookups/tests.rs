use super::*;
use crate::accounts::account_id_for;
use lucy_wire::jam::codec::{Schemas, Value};

fn id(lo: u64) -> Value {
    Struct(Fields::from([(0, Unsigned(lo)), (8, Unsigned(1 << 56))]))
}

#[test]
#[ignore = "requires the game data derived from a 1.74 install (just test-game <install>)"]
fn names_come_from_saved_accounts_or_the_id() {
    let state = tempfile::tempdir().unwrap();
    let accounts =
        Accounts::new(state.path().join("profiles"), state.path().join("t.json")).unwrap();
    accounts.get("Alpha").unwrap();
    let alpha = u64::from(account_id_for("Alpha"));
    let request = Fields::from([(0x78, Array(vec![id(alpha), id(0x1234_5678)]))]);
    let reply = player_names(&request, &accounts).unwrap();
    assert_eq!((reply.0, reply.1), (NAME_REPLY, NAMES_FOUND));
    let names: Vec<_> = match &reply.2[&0x78] {
        Array(names) => names
            .iter()
            .map(|name| match name {
                Struct(fields) => (fields[&0].clone(), fields[&0x18].clone()),
                _ => panic!("a name"),
            })
            .collect(),
        _ => panic!("names"),
    };
    assert_eq!(
        names,
        [
            (id(alpha), Value::String("Alpha".into())),
            (id(0x1234_5678), Value::String("Player22136".into())),
        ]
    );
    Schemas::lobby(&lucy_testkit::runtime_dir())
        .unwrap()
        .encode(reply.0, reply.1, &reply.2)
        .unwrap();
}

#[test]
#[ignore = "requires the game data derived from a 1.74 install (just test-game <install>)"]
fn the_owl_poll_has_no_match_and_the_store_no_products() {
    let schemas = Schemas::lobby(&lucy_testkit::runtime_dir()).unwrap();
    let (crc, id, fields) = no_owl_match();
    assert_eq!(schemas.encode(crc, id, &fields).unwrap(), [1, 0, 0, 0, 0]);
    // The store's currency follows the region (ISO 4217: 840 USD, 978 EUR,
    // 826 GBP, 643 RUB); an unknown region is the US.
    for (region, currency) in [
        ("US", 840),
        ("EU", 978),
        ("GB", 826),
        ("RU", 643),
        ("?", 840),
    ] {
        let messages = store(region);
        let ids: Vec<_> = messages.iter().map(|&(crc, id, _)| (crc, id)).collect();
        assert_eq!(ids, [(STORE, STORE_STATE), (STORE, STORE_PRODUCTS)]);
        let products = &messages[1].2;
        assert_eq!(products[&0x78], Unsigned(currency), "{region}");
        assert_eq!(products[&0x80], Array(Vec::new()));
        assert_eq!(products[&0x9c], Unsigned(0));
        for (crc, id, fields) in &messages {
            schemas.encode(*crc, *id, fields).unwrap();
        }
    }
}
