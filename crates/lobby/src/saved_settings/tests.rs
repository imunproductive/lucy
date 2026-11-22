use serde_json::json;

use super::*;
use crate::{player_messages, templates::restore_value};
use lucy_wire::jam::codec::{Fields, Schemas};

fn entry(value: i64, key: u64) -> Value {
    Value::Struct(Fields::from([
        (0, Value::Signed(value)),
        (8, Value::Unsigned(key)),
    ]))
}

#[test]
fn a_saved_block_comes_back_in_20802() {
    let schemas = Schemas::lobby();
    let mut profile = Profile::default();
    let social = Value::Struct(Fields::from([
        (0, Value::Bool(false)),
        (5, Value::Unsigned(2)),
    ]));
    assert_eq!(block_field(22202), Some(SOCIAL_SETTINGS));
    assert_eq!(block_field(22204), None);
    save_block(&mut profile, SOCIAL_SETTINGS, Some(&social));
    // The shape Python's profile stores.
    assert_eq!(
        profile.settings["+0x10D"],
        json!({"+0x0": false, "+0x5": 2})
    );
    let settings = player_messages::settings(&schemas, &profile).unwrap();
    assert_eq!(settings[&0x10d], social);
    save_block(&mut profile, "+0x78", None);
    assert_eq!(profile.settings["+0x78"], Json::Null);
    player_messages::settings(&schemas, &profile).unwrap();
}

#[test]
fn stored_values_restore_unchanged() {
    let value = Value::Struct(Fields::from([
        (0, Value::Blob(vec![0xde, 0xad])),
        (4, Value::Float32(0.5)),
        (
            8,
            Value::Array(vec![Value::Signed(-3), Value::Unsigned(u64::MAX)]),
        ),
        (0x10, Value::String("text".into())),
        (0x2a, Value::Bool(true)),
    ]));
    let restored = restore_value(stored(&value)).unwrap();
    let Value::Struct(fields) = restored else {
        panic!("a struct")
    };
    assert_eq!(fields[&0], Value::Blob(vec![0xde, 0xad]));
    assert_eq!(fields[&4], Value::Float64(0.5));
    assert_eq!(
        fields[&8],
        Value::Array(vec![Value::Signed(-3), Value::Unsigned(u64::MAX)])
    );
    assert_eq!(fields[&0x10], Value::String("text".into()));
    assert_eq!(fields[&0x2a], Value::Bool(true));
}

#[test]
fn changed_values_replace_their_setting_id_in_place() {
    let mut profile = Profile::default();
    profile.settings.insert(
        "+0x130".into(),
        json!([{"+0x0": 1, "+0x8": 10}, {"+0x0": 2, "+0x8": 20}, {"+0x0": 3, "+0x8": 10}]),
    );
    save_values(&mut profile, &[entry(5, 20), entry(7, 30)]);
    assert_eq!(
        profile.settings["+0x130"],
        json!([{"+0x0": 3, "+0x8": 10}, {"+0x0": 5, "+0x8": 20}, {"+0x0": 7, "+0x8": 30}])
    );
    // -1 is kept in the wire's u64 form, which 20802 can encode.
    set_saved_value(&mut profile, 10, -1);
    assert_eq!(
        profile.settings["+0x130"],
        json!([{"+0x0": 5, "+0x8": 20}, {"+0x0": 7, "+0x8": 30}, {"+0x0": u64::MAX, "+0x8": 10}])
    );
    player_messages::settings(&Schemas::lobby(), &profile).unwrap();
}

#[test]
fn a_key_saved_signed_or_as_text_matches_the_one_the_client_sends() {
    let mut profile = Profile::default();
    profile.settings.insert(
        "+0x130".into(),
        json!([
            {"+0x0": -1, "+0x8": -2},
            {"+0x0": "0x10", "+0x8": "20"},
            {"+0x0": 1, "+0x8": 0x1_0000_0000u64},
            {"+0x8": 30},
        ]),
    );
    normalize_values(&mut profile);
    assert_eq!(
        profile.settings["+0x130"],
        json!([{"+0x0": u64::MAX, "+0x8": 0xffff_fffeu32}, {"+0x0": 16, "+0x8": 20}])
    );
    save_values(
        &mut profile,
        &[Value::Struct(Fields::from([
            (0, Value::Unsigned(4)),
            (8, Value::Unsigned(0xffff_fffe)),
        ]))],
    );
    assert_eq!(
        profile.settings["+0x130"],
        json!([{"+0x0": 4, "+0x8": 0xffff_fffeu32}, {"+0x0": 16, "+0x8": 20}])
    );
}

#[test]
fn ux_states_and_season_intros_save_only_changes() {
    let mut profile = Profile::default();
    assert!(save_ux_state(
        &mut profile,
        &Value::Unsigned(7),
        &Value::Signed(-4)
    ));
    assert!(!save_ux_state(
        &mut profile,
        &Value::Unsigned(7),
        &Value::Signed(-4)
    ));
    assert!(save_ux_state(
        &mut profile,
        &Value::Unsigned(7),
        &Value::Signed(1)
    ));
    assert_eq!(profile.ux_states["7"], json!(1));
    assert_eq!(
        player_messages::ux_states(&profile).unwrap()[&0x78],
        Value::Array(vec![
            Value::Struct(Fields::from([
                (0, Value::Unsigned(2)),
                (4, Value::Unsigned(0))
            ])),
            Value::Struct(Fields::from([
                (0, Value::Unsigned(0)),
                (4, Value::Unsigned(4))
            ])),
            Value::Struct(Fields::from([
                (0, Value::Unsigned(1)),
                (4, Value::Unsigned(7))
            ])),
        ])
    );
    let card = 0x0630_0000_0000_01b3;
    assert!(season_intro_seen(&mut profile, card));
    assert!(!season_intro_seen(&mut profile, card));
    assert_eq!(profile.seasons_seen, [card as i64]);
}
