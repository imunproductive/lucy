//! The XP gains and the level curve build 104319 derives
//! (`game_progression_174.json`).

use serde_json::{Value, json};

fn derived() -> Value {
    assert_eq!(
        orpheus_testkit::derived_outputs("game_progression"),
        ["data/game_progression_174.json"]
    );
    orpheus_testkit::runtime_json("data/game_progression_174.json")
}

#[test]
#[ignore = "requires an Overwatch 1.74 install (just test-game <install>)"]
fn xp_gains_and_settings_are_the_installs() {
    let table = derived();
    let gains = table["xp_gains"].as_object().unwrap();
    let amounts: Vec<(&str, f64, i64)> = gains
        .iter()
        .map(|(name, gain)| {
            (
                name.as_str(),
                gain["amount"].as_f64().unwrap(),
                gain["type"].as_i64().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        amounts,
        [
            ("bronze_medal", 25.0, 0),
            ("silver_medal", 50.0, 1),
            ("gold_medal", 75.0, 2),
            ("match_finish", 75.0, 3),
            ("time", 1.75, 5),
            ("win", 250.0, 4),
            ("backfill", 200.0, 7),
            ("consecutive_match", 150.0, 8),
            ("first_win_of_the_day", 1500.0, 9),
        ]
    );
    assert_eq!(gains["time"]["text"], "0x0DE0000000002563");
    assert_eq!(
        table["xp_settings"],
        json!({
            "m_E609C2C4": 1.2, "m_D99C35B7": 1.2, "m_F8D5359A": 1.2, "m_F63C0C7B": 0.25,
            "m_84F13CFF": 0.3, "m_398CB1D9": 0.2, "m_94EA7D40": 20,
        })
    );
}

#[test]
#[ignore = "requires an Overwatch 1.74 install (just test-game <install>)"]
fn the_level_curve_and_rewards_are_the_installs() {
    let table = derived();
    assert_eq!(
        table["level_experience"],
        json!([
            0, 0, 2000, 3500, 5000, 6500, 7500, 8500, 9500, 10500, 11500, 12500, 13500, 14500,
            15000, 15500, 16000, 16500, 17000, 17500, 18000, 18500, 19000, 19500, 20000,
        ])
    );
    assert_eq!(table["level_up_unlocks"], json!(["0x02500000000003E1"]));
    assert_eq!(
        table["ruleset_win_rewards"],
        json!([
            {"wins": 3, "unlock": "0x02500000000003E1"},
            {"wins": 6, "unlock": "0x02500000000003E1"},
            {"wins": 9, "unlock": "0x02500000000003E1"},
        ])
    );
}
