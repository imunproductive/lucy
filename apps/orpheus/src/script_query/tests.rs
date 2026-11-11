//! `tests/test_ssq.py`: the queries against known facts of the 1.74 graphs
//! (the hero catalog, the graph tables of the frame format, the mover mods
//! and each game mode's scripts), on the data derived from the install.

use std::{
    collections::{BTreeMap, HashMap},
    sync::LazyLock,
};

use super::{
    Query, QueryCommand,
    data::Names,
    index::bytecode_uses,
    json::Int,
    render::{alias, guid_text},
    views::{parse_guid, parse_var},
};

static QUERY: LazyLock<Query> = LazyLock::new(|| {
    Query::load(&orpheus_testkit::runtime_dir(), None).expect("the derived game data loads")
});

fn query() -> &'static Query {
    &QUERY
}

fn run(command: QueryCommand) -> Vec<String> {
    query().run(&command)
}

fn show(graph: &str, state: i64) -> Vec<String> {
    run(QueryCommand::Show {
        graph: graph.into(),
        state_index: Some(state),
        depth: Some(0),
    })
}

fn var(var: &str, graph: Option<&str>) -> Vec<String> {
    run(QueryCommand::Var {
        var: var.into(),
        graph: graph.map(Into::into),
    })
}

fn graphs_of_mode(mode: &str) -> Vec<String> {
    run(QueryCommand::Graphs {
        hero: None,
        mode: Some(mode.into()),
        entity: None,
    })
}

fn find(text: &str) -> Vec<String> {
    run(QueryCommand::Find {
        text: text.split(' ').map(Into::into).collect(),
        limit: 40,
    })
}

fn hero(name: &str) -> Vec<String> {
    run(QueryCommand::Hero { name: name.into() })
}

/// The numbers in a line, in order.
fn numbers(text: &str) -> Vec<i64> {
    text.split(|ch: char| !ch.is_ascii_digit())
        .filter(|part| !part.is_empty())
        .map(|part| part.parse().unwrap())
        .collect()
}

/// The numbers `show GRAPH` prints in its head.
struct Header {
    nodes: i64,
    net_client_server: (i64, i64, i64),
    states: i64,
    entries: i64,
    remote: i64,
    owner_states: i64,
    remote_states: i64,
    presence: i64,
    presence_vars: Vec<i64>,
    bits: (i64, i64, i64),
    classes: BTreeMap<String, i64>,
    lines: Vec<String>,
}

fn header(graph: &str) -> Header {
    let lines = show(graph, 0);
    let counts = numbers(&lines[1]);
    let frames = lines
        .iter()
        .find(|line| line.starts_with("frames: "))
        .unwrap();
    let (frame_counts, presence_vars) = frames.split_once(" with a presence bit: ").unwrap();
    let frame_counts = numbers(frame_counts);
    let bits = numbers(
        lines
            .iter()
            .find(|line| line.starts_with("bits: "))
            .unwrap(),
    );
    let classes = lines
        .iter()
        .find_map(|line| line.strip_prefix("classes: "))
        .unwrap();
    Header {
        nodes: counts[0],
        net_client_server: (counts[1], counts[2], counts[3]),
        states: counts[4],
        entries: counts[5],
        remote: counts[6],
        owner_states: frame_counts[0],
        remote_states: frame_counts[1],
        presence: frame_counts[3],
        presence_vars: presence_vars
            .split(", ")
            .filter_map(|var| var.strip_prefix('#'))
            .map(|var| numbers(var)[0])
            .collect(),
        bits: (bits[0], bits[1], bits[2]),
        classes: class_counts(classes),
        lines,
    }
}

/// `StateStack x10, Switch(case subgraphs) x6` by name.
fn class_counts(text: &str) -> BTreeMap<String, i64> {
    let mut counts = BTreeMap::new();
    for part in text.split(", ") {
        let (name, count) = part.rsplit_once(" x").unwrap();
        *counts.entry(name.to_owned()).or_default() += count.parse::<i64>().unwrap();
    }
    counts
}

/// Soldier: 76's Sprint graph as a catalog row: nodes, states, entries,
/// networked / client-only / server-only, remote sync nodes, bit widths and
/// node classes.
const CATALOG_0255: &str = "| 0255 | 0 | init#5 | initial | root | - | 58 | 34 | 7 | 53/5/0 | 19 | 6/6/5 | LET_GAME_DECIDE | \
StateStack x10, ActionSetVar x8, StateBooleanSwitch x8, Entry x6, StateLogicalButton x5, \
Condition_Bool x4, ActionSendGameMessage x2, MovementMod x2, StateAnim x2, ActionCriteria x1, \
ActionSendClientOnlyGameMessage x1, ClientOnlySharedVars x1, EntryClientOnlySharedVars x1, \
STU_75D1C308 x1, STU_E9694EFD x1, StateAbility x1, StateChaseVar x1, StateEffect(StEC) x1, \
StateUXPresenter x1, StateWait x1 |";

/// One row of the graph tables: m_states, H=1 states, remote, H=0 states,
/// presence bits, and the nodes / states / remote bit widths.
type TableRow = (i64, i64, i64, i64, i64, (i64, i64, i64));

const GRAPH_TABLE: &[(&str, TableRow)] = &[
    ("13C1", (49, 43, 20, 19, 12, (7, 6, 5))),
    ("20E4", (26, 1, 1, 1, 0, (5, 5, 1))),
    ("288A", (12, 6, 1, 1, 0, (4, 4, 1))),
    ("288B", (146, 30, 21, 19, 4, (9, 8, 5))),
    ("0C90", (138, 118, 58, 56, 29, (9, 8, 6))),
    ("13C0", (22, 20, 10, 10, 3, (6, 5, 4))),
    ("0C8E", (67, 48, 31, 29, 16, (8, 7, 5))),
];

/// Soldier: 76's body graphs: presence bits / H=0 states.
const BODY_GRAPHS: &[(&str, (i64, i64))] = &[
    ("0033", (4, 7)),
    ("004B", (4, 16)),
    ("0043", (7, 13)),
    ("0251", (0, 0)),
    ("0255", (9, 18)),
    ("0257", (6, 13)),
    ("0259", (7, 25)),
    ("091B", (0, 0)),
    ("08B6", (2, 5)),
    ("0015", (1, 13)),
    ("0254", (23, 47)),
    ("01CF", (22, 20)),
];

/// Each game mode's controller graph (`STUGameMode m_teams[].m_controllerScript`).
const CONTROLLERS: &[(&str, &str)] = &[
    ("0018", "13C1"),
    ("0010", "0C90"),
    ("0014", "0C90"),
    ("0015", "0C90"),
    ("0016", "0C90"),
    ("0017", "0C90"),
    ("005A", "0C90"),
    ("0067", "0C90"),
    ("0003", "0CA2"),
    ("002A", "0CA2"),
    ("000F", "0D2A"),
    ("001A", "0D2A"),
    ("0009", "0D6F"),
    ("0046", "0D6F"),
    ("0008", "0DA5"),
    ("0025", "0F44"),
    ("001D", "0F62"),
    ("001E", "0F74"),
    ("0059", "0F74"),
    ("0070", "0F74"),
    ("0020", "0FC3"),
    ("0029", "11D0"),
    ("0019", "13A4"),
    ("0043", "194E"),
    ("0007", "2E98"),
];

#[test]
#[ignore = "needs game install"]
fn sprint_graph_matches_its_catalog_row() {
    let row: Vec<&str> = CATALOG_0255
        .trim_matches('|')
        .split('|')
        .map(str::trim)
        .collect();
    let found = header("0255");
    assert_eq!(found.nodes, row[6].parse::<i64>().unwrap());
    assert_eq!(found.states, row[7].parse::<i64>().unwrap());
    assert_eq!(found.entries, row[8].parse::<i64>().unwrap());
    let split =
        |cell: &str| -> Vec<i64> { cell.split('/').map(|part| part.parse().unwrap()).collect() };
    let net = split(row[9]);
    assert_eq!(found.net_client_server, (net[0], net[1], net[2]));
    assert_eq!(found.remote, row[10].parse::<i64>().unwrap());
    let bits = split(row[11]);
    assert_eq!(found.bits, (bits[0], bits[1], bits[2]));
    assert_eq!(found.classes, class_counts(row[13]));
    assert!(
        found
            .lines
            .contains(&"abilities: rid4 st1 35 ABILITY_1".to_owned())
    );
}

#[test]
#[ignore = "needs game install"]
fn frame_counts_of_the_graph_tables() {
    for &(graph, (states, owner, remote, remote_states, presence, bits)) in GRAPH_TABLE {
        let found = header(graph);
        assert_eq!(
            (
                found.states,
                found.owner_states,
                found.remote,
                found.remote_states
            ),
            (states, owner, remote, remote_states),
            "{graph}"
        );
        assert_eq!((found.presence, found.bits), (presence, bits), "{graph}");
    }
    for &(graph, (presence, remote_states)) in BODY_GRAPHS {
        let found = header(graph);
        assert_eq!(
            (found.presence, found.remote_states),
            (presence, remote_states),
            "{graph}"
        );
    }
}

#[test]
#[ignore = "needs game install"]
fn presence_bits_in_order() {
    // 13C1's presence bits in order, 288B's v25377, v13161, v32791, v478.
    assert_eq!(
        header("13C1").presence_vars,
        [
            2114, 8849, 8916, 12511, 8418, 8419, 8738, 13162, 10476, 8432, 10489, 9023
        ]
    );
    assert_eq!(header("288B").presence_vars, [25377, 13161, 32791, 478]);
}

#[test]
#[ignore = "needs game install"]
fn graph_288b_is_the_hero_select_screen() {
    let graphs = run(QueryCommand::Graphs {
        hero: None,
        mode: None,
        entity: None,
    });
    let row = graphs
        .iter()
        .find(|line| line.starts_with("288B "))
        .unwrap();
    let words: Vec<&str> = row.split_whitespace().collect();
    assert_eq!(
        words,
        ["288B", "UI", "269", "146", "hero", "select", "screen"],
        "{row}"
    );
    // 288A slot 0 = st0 SubScript -> 288B
    assert!(
        header("288B")
            .lines
            .contains(&"named by: 288A rid1 st0 SubScript".to_owned())
    );
    assert!(find("hero select").contains(&"  288B  UI  hero select screen".to_owned()));
}

/// Whether a `graphs` row says the graph is started by a SubScript state of
/// `parent`: `from 13C1 rid\d+ st42 SubScript$`.
fn started_by(row: &str, parent: &str, state: u32) -> bool {
    let Some((_, from)) = row.split_once(&format!("from {parent} rid")) else {
        return false;
    };
    let rest = from.trim_start_matches(|ch: char| ch.is_ascii_digit());
    rest.len() < from.len() && rest == format!(" st{state} SubScript")
}

#[test]
#[ignore = "needs game install"]
fn who_starts_the_hero_select_graphs() {
    // 13C1 st42 SubScript -> 20E4, st43 -> 288A; 288A st0 -> 288B, st1 -> 288D, st3 -> 292E.
    let rows: HashMap<String, String> = graphs_of_mode("practice range")[2..]
        .iter()
        .map(|line| (line.chars().take(4).collect(), line.clone()))
        .collect();
    for (graph, parent, state) in [
        ("20E4", "13C1", 42),
        ("288A", "13C1", 43),
        ("288B", "288A", 0),
        ("288D", "288A", 1),
        ("292E", "288A", 3),
    ] {
        assert!(started_by(&rows[graph], parent, state), "{}", rows[graph]);
    }
}

#[test]
#[ignore = "needs game install"]
fn sprint_movement_mods() {
    // Sprint (0255) state 0 "speed add 0.5" (a dynamic config var, 0.5 from
    // the VM), state 20 "crouch intent set 0" (mover mod index 0 = speed, 21
    // = crouch intent). The derived bundle keeps these objects' fields in
    // another order than the prepared pack did, so the fields are checked
    // one by one.
    let state0 = show("0255", 0);
    assert!(state0.contains(&"rid2 st0 MovementMod (remote 0)".to_owned()));
    let mods = |lines: &[String], class: &str| {
        let prefix = format!("    m_BC5E91CF=[{class}{{");
        lines
            .iter()
            .find(|line| line.starts_with(&prefix) && line.ends_with("}]"))
            .cloned()
            .unwrap_or_else(|| panic!("no {class} mod in {lines:?}"))
    };
    let speed = mods(&state0, "STU_4CB0950F");
    assert!(
        speed.contains("m_A9561CA0=0") && speed.contains("m_9B7A63EA=#1002"),
        "{speed}"
    );
    let crouch = mods(&show("0255", 20), "STU_56DEE1BB");
    assert!(
        crouch.contains("m_A9561CA0=21") && crouch.contains("m_9B7A63EA=0"),
        "{crouch}"
    );
    assert!(
        var("1002", Some("0255")).contains(&"  0255 rid3 ActionSetVar  m_out_Var = 0.5".to_owned())
    );
}

#[test]
#[ignore = "needs game install"]
fn movement_mod_count() {
    // The 1.74 graphs contain 171; an extended runtime can add more.
    let cls = "STU_316CFEF2";
    let count = query()
        .bundle
        .graphs
        .values()
        .flat_map(|graph| &graph.nodes)
        .filter(|node| node.cls == cls)
        .count();
    assert!(count >= 171, "{count}");
    let lines = run(QueryCommand::Class {
        name: "MovementMod".into(),
    });
    assert!(
        lines[0]
            .starts_with("STU_316CFEF2  alias MovementMod (from research catalog)  hash 316CFEF2"),
        "{}",
        lines[0]
    );
    let nodes = format!("nodes: {count} in ");
    assert!(
        lines.iter().any(|line| line.starts_with(&nodes)),
        "{lines:?}"
    );
    assert!(find("316CFEF2").contains(&format!("  {cls}  MovementMod  {count} nodes")));
}

#[test]
#[ignore = "needs game install"]
fn aliases_are_unique() {
    let index = &query().index;
    let classes: std::collections::BTreeSet<&String> = index
        .objects
        .keys()
        .chain(index.class_nodes.keys())
        .collect();
    let aliases: std::collections::BTreeSet<&str> = classes
        .iter()
        .map(|cls| alias(&query().names, cls))
        .collect();
    assert_eq!(aliases.len(), classes.len());
}

#[test]
#[ignore = "needs game install"]
fn soldier() {
    // Soldier's roots and his catalog row (57 graphs, 2847 nodes, 1610
    // networked, 140 classes, 0F9C missing).
    let lines = hero("soldier");
    let has = |text: &str| lines.iter().any(|line| line == text);
    assert!(has(
        "initial graphs (instance 1..N): 0033 004B 0043 0251 0255 0257 0259 091B 08B6; manager 0015; weapons 0:- 1:0254 2:-"
    ));
    assert!(has(
        "57 graphs (1 not in the data: 0F9C), 2847 nodes (1610 networked), 140 node classes"
    ));
    for (slot, graph) in [(5, "0255"), (6, "0257"), (7, "0259")] {
        let found = lines.iter().any(|line| {
            line.strip_prefix(graph)
                .and_then(|rest| rest.trim_start().strip_prefix("ability "))
                .is_some_and(|rest| rest.ends_with(&format!(" init#{slot}")))
        });
        assert!(found, "{graph}");
    }
    let rows = run(QueryCommand::Graphs {
        hero: Some("soldier".into()),
        mode: None,
        entity: None,
    });
    assert_eq!(
        rows[0],
        "Soldier: 76: body 03CF.003, 57 graphs (1 not in the data: 0F9C)"
    );
    assert!(
        rows.iter()
            .any(|line| line.starts_with("0F9C ") && line.ends_with("(not in the data)"))
    );
}

#[test]
#[ignore = "needs game install"]
fn soldier_abilities() {
    // The buttons: 0043 st6 QUICK_WEAPON, 0255 st1 ABILITY_1, 0257 st0
    // ABILITY_2, 0259 st0 ABILITY_3, 0254 st31 SECONDARY_FIRE.
    let lines = hero("soldier");
    for text in [
        "0043 rid13 st6 31 QUICK_WEAPON",
        "0255 rid4 st1 35 ABILITY_1",
        "0257 rid2 st0 36 ABILITY_2",
        "0259 rid2 st0 37 ABILITY_3",
        "0254 rid61 st31 29 SECONDARY_FIRE",
    ] {
        assert!(lines.contains(&format!("  ability {text}")), "{text}");
    }
}

#[test]
#[ignore = "needs game install"]
fn var_699_is_written_by_sprint_and_read_by_its_icon() {
    // Sprint 0255: #699 from the Ability's start/stop var lists; st31's icon
    // reads `#699 ? 0 : (#2575 || #2575e)` and `#699 == 1`.
    let lines = var("699", Some("0255"));
    let has = |text: &str| lines.iter().any(|line| line == text);
    assert!(has(
        "  0255 rid4 st1 StateAbility  m_EF3C5C73[0].m_out_Var = 1"
    ));
    assert!(has(
        "  0255 rid4 st1 StateAbility  m_19899EAC[0].m_out_Var = 0"
    ));
    let reads: Vec<&String> = lines
        .iter()
        .filter(|line| line.starts_with("  0255 rid53 st31 StateUXPresenter"))
        .collect();
    assert!(
        reads
            .iter()
            .any(|line| line.ends_with(": (#699 ? 0.0 : (#2575 || #2575e))")),
        "{reads:?}"
    );
    assert!(
        reads.iter().any(|line| line.ends_with(": (#699 == 1.0)")),
        "{reads:?}"
    );
    assert!(has(
        "  0255 m_syncVars[59]: instance, presence bit 7, m_AC9480C7=0"
    ));
    let write = lines
        .iter()
        .position(|line| line == "write (3 in 1 graph):")
        .unwrap();
    assert!(lines[write + 1].starts_with("  0255 rid4 st1 StateAbility"));
}

#[test]
#[ignore = "needs game install"]
fn entity_scoped_bools_of_hero_select() {
    // v31435 and v2438 are entity-scoped bools written with STUConfigVarBool.
    let lines = var("31435", Some("288B"));
    assert!(lines[1].starts_with("scope: entity x12 ("), "{}", lines[1]);
    assert_eq!(
        lines[2],
        "type: bool x5 (from defaults and constant writes)"
    );
    // Every constant write of v2438 in 288B is an entity bool too; its
    // instance uses are where game message 1360.025 lands (rid22) before
    // rid201 copies it into the entity variable.
    let lines = var("2438", Some("288B"));
    let start = lines
        .iter()
        .position(|line| line == "write (10 in 1 graph):")
        .unwrap();
    let end = lines
        .iter()
        .position(|line| line == "read (10 in 1 graph):")
        .unwrap();
    let writes = &lines[start + 1..end];
    let constant = writes
        .iter()
        .filter(|line| line.ends_with("= true [entity]") || line.ends_with("= false [entity]"))
        .count();
    assert_eq!(constant, 8);
    assert!(lines[2].starts_with("type: bool x8 "), "{}", lines[2]);
    assert!(writes.contains(&"  288B rid201 ActionSetVar  m_out_Var = #2438 [entity]".to_owned()));
    assert!(
        writes[0].starts_with("  288B rid22 EntryGameMessage  m_params[0].m_out_Var: "),
        "{}",
        writes[0]
    );
}

#[test]
#[ignore = "needs game install"]
fn names_in_the_data() {
    assert!(
        var("CURRENT_HERO", None)[0].starts_with("#940  identifier 03AC.01C  name CURRENT_HERO")
    );
    assert!(
        find("03CF.003")
            .contains(&"  Soldier: 76  hero 006E.075  body 03CF.003  Damage".to_owned())
    );
    let found = find("0018.0C5");
    assert!(
        found
            .iter()
            .any(|line| line.starts_with("  Practice Range  0018.0C5  script 13C0")),
        "{found:?}"
    );
}

#[test]
fn notation() {
    assert_eq!(guid_text(0x0580000000000255), "0255.01B");
    assert_eq!(guid_text(0x0D800000000002BB), "02BB.01C");
    assert_eq!(guid_text(0x02E000000000006E), "006E.075");
    assert_eq!(guid_text(0x04000000000003CF), "03CF.003");
    assert_eq!(guid_text(0x0230000000000018), "0018.0C5");
    let names = Names::embedded();
    let vars: Vec<Option<Int>> = ["699", "#699", "#7644e", "0x2BB", "02BB.01C"]
        .iter()
        .map(|text| parse_var(&names, text))
        .collect();
    assert_eq!(
        vars,
        [Some(699), Some(699), Some(7644), Some(699), Some(699)]
    );
    assert_eq!(parse_var(&names, "CURRENT_HERO"), Some(940));
    assert_eq!(parse_guid("0186.00D"), Some(0x0300000000000186));
    assert_eq!(parse_guid("0255.01B"), Some(0x0580000000000255));
}

#[test]
fn expressions_are_read_by_their_jumps() {
    // The code after an END is reached by a jump only (0255 st31).
    let (uses, first) = bytecode_uses(&[8, 0, 4, 6, 6, 0, 0, 8, 8, 1, 3, 4, 8, 2, 0]);
    let mut uses: Vec<(Int, Vec<&str>)> = uses
        .into_iter()
        .map(|(number, kinds)| (number, kinds.into_iter().collect()))
        .collect();
    uses.sort();
    assert_eq!(
        uses,
        [(0, vec!["read"]), (1, vec!["read"]), (2, vec!["read"])]
    );
    assert_eq!(first, Some(0));
}

#[test]
#[ignore = "needs game install"]
fn every_expression_variable_is_used() {
    let unused = query()
        .index
        .vars
        .values()
        .flatten()
        .filter(|found| found.kind == "unused")
        .count();
    assert_eq!(unused, 0);
}

#[test]
#[ignore = "needs game install"]
fn each_mode_names_its_controller() {
    for &(mode, controller) in CONTROLLERS {
        let rows = graphs_of_mode(mode);
        let prefix = format!("{controller} ");
        assert!(
            rows[2..]
                .iter()
                .any(|line| line.starts_with(&prefix) && line.contains(" controller")),
            "{mode}: {:?}",
            &rows[..rows.len().min(6)]
        );
    }
    assert!(
        graphs_of_mode("001E")
            .iter()
            .any(|line| line.starts_with("1058 ") && line.contains("FFA body script"))
    );
    assert!(
        graphs_of_mode("Practice Range")
            .iter()
            .any(|line| line.starts_with("0C90 ") && line.contains("TeamRed controller"))
    );
}
