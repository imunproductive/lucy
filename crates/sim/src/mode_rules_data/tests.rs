use super::*;
use serde_json::json;

fn dynamic(id: u32) -> Json {
    json!({"$": "STUConfigVarDynamic", "m_identifier": id, "m_60DB8F99": 0, "m_EE729DCB": 0})
}

fn int(value: i64) -> Json {
    json!({"$": "STUConfigVarInt", "m_value": value, "m_EE729DCB": 0})
}

fn expression(opcodes: &[u8], dynamics: &[u32], floats: &[f64], configs: Json) -> Json {
    json!({
        "$": "STU_919FD47C",
        "m_expression": {
            "$": "STUConfigVarExpressionData",
            "m_fragments": null,
            "m_opcodes": opcodes,
            "m_dynamicVars": dynamics.iter().map(|&id| dynamic(id)).collect::<Vec<_>>(),
            "m_D99EF254": floats,
        },
        "m_configVars": configs,
        "m_EE729DCB": 0,
    })
}

fn set_var(writes: u32, value: Json) -> Json {
    json!({"m_out_Var": dynamic(writes), "m_value": value})
}

/// A fixture node: its uid, class and fields.
type FixtureNode = (u32, String, Json);
/// A fixture graph: its index and nodes.
type FixtureGraph = (u32, Vec<FixtureNode>);

fn node(uid: u32, class: &str, fields: Json) -> FixtureNode {
    (uid, class.into(), fields)
}

fn graph(index: u32, nodes: Vec<FixtureNode>) -> Json {
    let nodes: Vec<Json> = nodes
        .into_iter()
        .enumerate()
        .map(|(pos, (uid, class, fields))| {
            json!({"pos": pos, "class": class, "uid": uid, "rid": pos, "remote": -1,
                "state": null, "client_only": false, "server_only": false, "fields": fields})
        })
        .collect();
    json!({
        "index": index, "items": nodes.len(), "states": [], "entries": [], "remote": [],
        "sync_vars": [], "containers": {}, "unknown_nodes": [], "fields": {},
        "header": {"m_nodesBitCount": 4, "m_statesBitCount": 4,
            "m_remoteSyncNodesBitCount": 4, "m_syncVarsBitCount": 4},
        "nodes": nodes,
    })
}

/// The install's nodes (their opcodes and inputs), with other numbers:
/// hero select 40/35 s, setup 50 s, banks 150/70 s, Control lock 20 s,
/// 100 / (1 × 60) % a second, hold at 98%, rollback 0.5 m/s.
fn nodes() -> Vec<FixtureGraph> {
    let bank = |constant: f64, index: f64| {
        expression(
            &[8, 0, 6, 0, 17, 6, 1, 34, 0],
            &[BANKS],
            &[index, constant],
            Json::Null,
        )
    };
    vec![
        (
            SIDED_GRAPH,
            vec![
                node(895, SET_VAR, set_var(CLOCK, int(50))),
                node(
                    896,
                    SET_VAR,
                    set_var(
                        CLOCK,
                        expression(
                            &[8, 0, 6, 0, 33, 5, 6, 8, 1, 4, 6, 6, 1, 0, 4, 6, 2, 0],
                            &[ROUNDS, ROUND_FLAG],
                            &[1.0, 40.0, 35.0],
                            Json::Null,
                        ),
                    ),
                ),
                node(
                    1297,
                    SET_VAR,
                    set_var(
                        BANK_CUT,
                        expression(
                            &[8, 0, 6, 0, 17, 6, 1, 24, 8, 0, 6, 2, 17, 6, 1, 24, 63, 0],
                            &[BANKS],
                            &[0.0, 150.0, 1.0],
                            Json::Null,
                        ),
                    ),
                ),
                node(
                    1298,
                    BRANCH,
                    json!({"m_condition": expression(
                        &[8, 0, 6, 0, 17, 6, 1, 36, 2, 10, 8, 0, 6, 2, 17, 6, 1, 36, 0],
                        &[BANKS],
                        &[0.0, 150.0, 1.0],
                        Json::Null,
                    )}),
                ),
                node(
                    673,
                    SET_VAR,
                    set_var(
                        BANK_RAISE,
                        expression(
                            &[
                                8, 0, 6, 0, 17, 4, 12, 6, 1, 8, 1, 6, 0, 17, 24, 1, 4, 6, 0, 8, 0,
                                6, 2, 17, 4, 12, 6, 1, 8, 1, 6, 2, 17, 24, 1, 4, 6, 0, 64, 0,
                            ],
                            &[RAISED, BANKS],
                            &[0.0, 70.0, 1.0],
                            Json::Null,
                        ),
                    ),
                ),
                node(
                    698,
                    SET_VAR,
                    set_var(
                        UNDER_FLOOR,
                        json!({"$": "STU_06610EBC", "m_configVars": [bank(70.0, 0.0), bank(70.0, 1.0)]}),
                    ),
                ),
                node(525, SET_VAR, set_var(SPEED_FACTOR, int(1))),
            ],
        ),
        (
            CONTROL_GRAPH,
            vec![
                node(310, SET_VAR, set_var(CLOCK, int(20))),
                node(830, SET_VAR, set_var(CLOCK, int(20))),
                node(831, SET_VAR, set_var(CLOCK, int(20))),
                node(525, SET_VAR, set_var(SPEED_FACTOR, int(1))),
                node(
                    511,
                    STACK,
                    set_var(
                        SCORE_RATE,
                        expression(
                            &[6, 0, 6, 1, 6, 2, 25, 26, 14, 0, 25, 8, 0, 25, 0],
                            &[SPEED_FACTOR],
                            &[100.0, 1.0, 60.0],
                            json!([{"$": RULESET_VALUE, "m_value": dynamic(SCORE_SPEED), "m_EE729DCB": 0}]),
                        ),
                    ),
                ),
                node(
                    879,
                    STACK,
                    set_var(
                        HOLD,
                        expression(
                            &[8, 0, 6, 0, 37, 4, 6, 6, 1, 0, 4, 6, 2, 0],
                            &[PHASE],
                            &[3.0, 98.0, 100.0],
                            Json::Null,
                        ),
                    ),
                ),
            ],
        ),
        (
            PAYLOAD_GRAPH,
            vec![
                node(
                    44,
                    PATH_MOVE,
                    json!({"m_speed": expression(&[6, 0, 8, 0, 25, 0], &[ROLLBACK_FACTOR], &[-0.5], Json::Null)}),
                ),
                node(
                    376,
                    STACK,
                    set_var(
                        ROLLBACK_FACTOR,
                        expression(
                            &[14, 0, 14, 1, 25, 0],
                            &[],
                            &[],
                            json!([
                                {"$": RULESET_VALUE, "m_value": expression(
                                    &[14, 0, 7, 0, 15, 0], &[PAYLOAD_SPEED], &[],
                                    json!([{"$": "STU_A3DEF368", "m_EE729DCB": 0}]))},
                                {"$": "STU_91EFD5B1", "m_value": dynamic(SPEED_FACTOR), "m_default": int(1)},
                            ]),
                        ),
                    ),
                ),
            ],
        ),
    ]
}

fn store(nodes: Vec<FixtureGraph>) -> GraphStore {
    let graphs: Vec<Json> = nodes
        .into_iter()
        .map(|(index, nodes)| graph(index, nodes))
        .collect();
    let bundle = json!({"version": 1, "missing": [], "graphs": graphs, "bodies": {}});
    GraphStore::from_json(&serde_json::to_vec(&bundle).unwrap()).unwrap()
}

const OBJECTIVES: &[u8] = br#"{"capture_area": {"segments": [0.25, 0.5]}}"#;

fn rulesets() -> Rulesets {
    Rulesets::from_json(
        &serde_json::to_vec(&json!({
            "schemas": {
                "0x0A3000000000000C": {"8853": 3, "9519": 2.0},
                "0x0A3000000000000D": {"8853": 4, "8990": 15, "10585": 100, "10586": 80},
                "0x0A3000000000002A": {"8853": 25, "11299": 5},
                "0x0A30000000000028": {"8853": 35, "11298": 15},
                "0x0A3000000000000B": {"9520": 2.0},
            },
            "heroes": {},
            "modes": {
                "0x0230000000000017": ["0x0A3000000000000C"],
                "0x0230000000000009": ["0x0A3000000000000D"],
                "0x023000000000001E": ["0x0A3000000000002A"],
                "0x023000000000001D": ["0x0A30000000000028"],
                "0x0230000000000015": ["0x0A3000000000000B"],
            },
        }))
        .unwrap(),
    )
    .unwrap()
}

/// Every value comes from its node, the capture marks from the derived
/// file, and each mode's settings from its ruleset defaults.
#[test]
fn the_rules_are_read_from_the_graphs_the_objectives_and_the_rulesets() {
    let data = ModeRulesData::read(&store(nodes()), OBJECTIVES).unwrap();
    let rulesets = rulesets();
    let assault = data.rules(&rulesets, ASSAULT).unwrap();
    let rate = f64::from((100.0_f64 / 60.0) as f32);
    assert_eq!(
        assault,
        ModeRules {
            assemble: 40.0,
            assemble_later: 35.0,
            setup: 50.0,
            bank_cap: 150.0,
            bank_floor: 70.0,
            control_unlock: 20.0,
            control: ControlScoring {
                percent_per_second: 0.0,
                hold_at: 98.0,
            },
            capture_marks: [0.25, 0.5],
            rollback_speed: 0.0,
            score_to_win: 0,
            round_time: 0.0,
            hero_select: 0.0,
            tiebreak: 0.0,
        }
    );
    let control = data.rules(&rulesets, CONTROL).unwrap();
    assert_eq!(control.score_to_win, 3);
    assert_eq!(control.control.percent_per_second, rate * 2.0);
    let elimination = data.rules(&rulesets, ELIMINATION).unwrap();
    assert_eq!(
        (
            elimination.score_to_win,
            elimination.round_time,
            elimination.hero_select,
            elimination.tiebreak
        ),
        (4, 100.0, 15.0, 80.0)
    );
    let deathmatch = data.rules(&rulesets, DEATHMATCH).unwrap();
    assert_eq!(
        (deathmatch.score_to_win, deathmatch.round_time),
        (25, 300.0)
    );
    let team = data.rules(&rulesets, TEAM_DEATHMATCH).unwrap();
    assert_eq!((team.score_to_win, team.round_time), (35, 900.0));
    let escort = data.rules(&rulesets, ESCORT).unwrap();
    assert_eq!(escort.rollback_speed, 0.5 * 2.0);
}

/// A mode whose schemas lack a setting its flow plays by doesn't get rules.
#[test]
fn a_mode_without_its_settings_has_no_rules() {
    let data = ModeRulesData::read(&store(nodes()), OBJECTIVES).unwrap();
    let rulesets = rulesets();
    for mode in [HYBRID, BOUNTY_HUNTER] {
        let error = data.rules(&rulesets, mode).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData, "{error}");
    }
    assert!(data.rules(&rulesets, SKIRMISH).is_ok());
    assert!(data.rules(&rulesets, 0x0230_0000_0000_0018).is_ok());
}

/// A node that is gone, changed or reads something unknown fails the
/// load, as do capture marks that aren't two rising ones.
#[test]
fn changed_data_fails_the_load() {
    let changed = |graph: u32, uid: u32, change: &dyn Fn(&mut String, &mut Json)| {
        let mut nodes = nodes();
        let (_, list) = nodes.iter_mut().find(|(index, _)| *index == graph).unwrap();
        let (_, class, fields) = list.iter_mut().find(|(id, _, _)| *id == uid).unwrap();
        change(class, fields);
        ModeRulesData::read(&store(nodes), OBJECTIVES)
            .unwrap_err()
            .to_string()
    };
    let error = changed(SIDED_GRAPH, 895, &|class, _| *class = STACK.into());
    assert!(error.contains("uid 895 is a"), "{error}");
    let error = changed(SIDED_GRAPH, 896, &|_, fields| {
        fields["m_out_Var"]["m_identifier"] = json!(59);
    });
    assert!(error.contains("uid 896 writes"), "{error}");
    let error = changed(SIDED_GRAPH, 1297, &|_, fields| {
        fields["m_value"]["m_expression"]["m_dynamicVars"][0]["m_identifier"] = json!(999);
    });
    assert!(error.contains("reads variable 999"), "{error}");
    let error = changed(SIDED_GRAPH, 1298, &|_, fields| {
        fields["m_condition"]["m_expression"]["m_D99EF254"][1] = json!(100.0);
    });
    assert!(error.contains("uid 1298"), "{error}");
    let error = changed(CONTROL_GRAPH, 831, &|_, fields| {
        fields["m_value"]["m_value"] = json!(25);
    });
    assert!(error.contains("uid 831 locks the point for 25"), "{error}");
    let error = changed(PAYLOAD_GRAPH, 376, &|_, fields| {
        fields["m_value"]["m_configVars"][0]["m_value"]["m_expression"]["m_dynamicVars"][0]["m_identifier"] =
            json!(9521);
    });
    assert!(error.contains("doesn't read ruleset entry 9520"), "{error}");
    let mut nodes = nodes();
    nodes.retain(|(index, _)| *index != PAYLOAD_GRAPH);
    let error = ModeRulesData::read(&store(nodes), OBJECTIVES).unwrap_err();
    assert!(error.to_string().contains("1225 is missing"), "{error}");

    let graphs = store(self::nodes());
    for marks in [
        br#"{"capture_area": {"segments": [0.25, 0.5, 0.75]}}"#.as_slice(),
        br#"{"capture_area": {"segments": [0.5, 0.25]}}"#,
        br#"{"capture_area": {}}"#,
    ] {
        assert!(ModeRulesData::read(&graphs, marks).is_err());
    }
}

/// The install's rules for every flow mode are the ones the tests without
/// the data play by (`fixture`): hero select 30 s then 25 s, setup 45 s,
/// banks cut to 2:00 or raised to 1:00, the Control point locked 30 s and
/// scoring 1% every 1.2 s up to a 99% hold, the capture marks 1/3 and 2/3,
/// a 0.3 m/s rollback, and the ruleset defaults (Control first to 2,
/// Elimination first to 3 with 135 s rounds, a 105 s tiebreaker and 20 s
/// of hero select, Deathmatch 20 and Team Deathmatch 30 kills in 10 min).
#[test]
#[ignore = "requires the 1.74 game data (ORPHEUS_GAME_DIR)"]
fn the_install_gives_the_fixture_rules() {
    let data = orpheus_testkit::runtime_dir().join("data");
    let graphs = GraphStore::load_runtime(&data, "game_graphs_174.json.gz").unwrap();
    let rules = ModeRulesData::load(&graphs, &data).unwrap();
    for mode in FLOW_MODES {
        assert_eq!(
            rules.rules(graphs.rulesets(), mode).unwrap(),
            fixture(mode),
            "mode {mode:016X}"
        );
    }
}
