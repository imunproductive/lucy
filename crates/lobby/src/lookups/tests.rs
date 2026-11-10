use super::*;
use crate::accounts::account_id_for;
use orpheus_wire::jam::codec::{Schemas, Value};

fn id(lo: u64) -> Value {
    Struct(Fields::from([(0, Unsigned(lo)), (8, Unsigned(1 << 56))]))
}

#[test]
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
    Schemas::lobby().encode(reply.0, reply.1, &reply.2).unwrap();
}

#[test]
fn the_owl_poll_and_the_store_page_replay_retail() {
    let schemas = Schemas::lobby();
    let (crc, id, fields) = no_owl_match();
    assert_eq!(schemas.encode(crc, id, &fields).unwrap(), [1, 0, 0, 0, 0]);
    let templates = RetailTemplates::captured();
    let recorded = templates.all(STORE, None);
    assert!(!recorded.is_empty());
    let us = store(&templates, "US");
    let ru = store(&templates, "RU");
    assert_eq!(us.len(), recorded.len());
    assert_ne!(us, ru, "prices follow the region");
    for (crc, id, fields) in us {
        schemas.encode(crc, id, &fields).unwrap();
    }
}
