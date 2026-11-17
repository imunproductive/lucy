//! The scene tag query state and the spawn picker (`vm/entities.rs`), in
//! the shape of the mode's respawn picker `0C8F`.

use super::*;

const SPAWN_TAG: u64 = 0x0d60_0000_0000_0123;
const OTHER_TAG: u64 = 0x0d60_0000_0000_0456;
const SPAWNS: [u32; 3] = [0x8000_0010, 0x8000_0011, 0x8000_0012];
const PICK: u64 = 0x0240_0000_0000_0685;

/// Three spawn points carry the spawn tag; the second also another one.
struct SpawnPoints;

impl World for SpawnPoints {
    fn entities_with_tag(&self, _site: Site, tag: u64) -> Option<Vec<u32>> {
        Some(match tag {
            SPAWN_TAG => SPAWNS.to_vec(),
            OTHER_TAG => vec![SPAWNS[1]],
            _ => Vec::new(),
        })
    }

    fn has_target_tag(&self, site: Site, entity: u32, tag: u64) -> Option<bool> {
        Some(self.entities_with_tag(site, tag)?.contains(&entity))
    }
}

fn tag(tag: u64) -> Json {
    json!({"$":"STU_357DABBB","m_7D7C86A1":format!("0x{tag:016X}"),"m_resourceKey":null})
}

/// Entry: st0 lists the entities with the spawn tag but not `excluded`
/// into #10, st1 picks among #10 into #11 (3 s memory) on `0685`, and
/// counts its picks in #12.
fn picker(excluded: Option<u64>) -> GraphStore {
    store(vec![graph(
        42,
        vec![
            node(
                0,
                "STUStatescriptEntry",
                None,
                json!({"m_outPlug":plug(&[(1, "m_beginPlug"), (2, "m_beginPlug")], &[])}),
            ),
            node(
                1,
                "STU_160F2578",
                Some(0),
                json!({
                    "m_4AEEE6A1":var(10, false),
                    "m_9C7BD8B8":tag(SPAWN_TAG),
                    "m_8B2A4DF3":excluded.map(tag),
                }),
            ),
            node(
                2,
                "STU_5E805B5A",
                Some(1),
                json!({
                    "m_814F8EC3":var(10, false),
                    "m_C51AF683":var(11, false),
                    "m_82317B5F":{"$":"STUConfigVarInt","m_value":3},
                    "m_1405CDCB":plug(&[(4, "m_inPlug")], &[]),
                }),
            ),
            node(
                3,
                "STUStatescriptEntryGameMessage",
                None,
                json!({
                    "m_gameMessage":{"m_gameMessage":format!("0x{PICK:016X}")},
                    "m_outPlug":plug(&[(2, "m_F4DD996D")], &[]),
                }),
            ),
            node(
                4,
                "STUStatescriptActionSetVar",
                None,
                json!({"m_out_Var":var(12, false), "m_value":true}),
            ),
        ],
        json!([1, 2]),
        json!([0, 3]),
    )])
}

fn start(graphs: GraphStore) -> Component {
    Component::start(
        Arc::new(graphs),
        42,
        Variables::new(),
        vec![Root::new(1, 42)],
        0,
        &mut SpawnPoints,
        MAX_INIT_WORK,
    )
    .unwrap()
}

fn variable(component: &Component, id: u32) -> Value {
    component.snapshot().unwrap().instances[&1]
        .variables
        .get(&id)
        .cloned()
        .unwrap_or(Value::Null)
}

fn pick(component: &mut Component) -> Value {
    component.queue_message(1, PICK, Variables::new()).unwrap();
    component.run_now(&mut SpawnPoints).unwrap();
    variable(component, 11)
}

#[test]
fn the_spawn_picker_takes_a_tagged_entity_none_of_its_recent_picks_took() {
    let mut component = start(picker(None));
    assert!(component.snapshot().unwrap().diagnostics.is_empty());
    let spawns: Vec<Value> = SPAWNS.iter().map(|&entity| Value::Entity(entity)).collect();
    assert_eq!(variable(&component, 10), Value::Array(spawns.clone()));
    // Within the 3 s memory each of the three points is picked once.
    let mut picked: Vec<Value> = (0..3).map(|_| pick(&mut component)).collect();
    picked.sort_by_key(|value| match value {
        Value::Entity(entity) => *entity,
        _ => 0,
    });
    assert_eq!(picked, spawns);
    // All remembered: the oldest pick is forgotten first, and the pick
    // still takes a spawn point; every pick follows the output.
    assert!(spawns.contains(&pick(&mut component)));
    assert_eq!(variable(&component, 12), Value::Bool(true));
}

#[test]
fn the_tag_query_state_leaves_out_the_excluded_tag_and_nothing_to_pick_gives_null() {
    let component = start(picker(Some(OTHER_TAG)));
    assert_eq!(
        variable(&component, 10),
        Value::Array(vec![Value::Entity(SPAWNS[0]), Value::Entity(SPAWNS[2])])
    );
    // A pick from an empty list writes null and still follows the output.
    let mut empty = start(picker(Some(SPAWN_TAG)));
    assert_eq!(variable(&empty, 10), Value::Array(Vec::new()));
    assert_eq!(pick(&mut empty), Value::Null);
    assert_eq!(variable(&empty, 12), Value::Bool(true));
}

/// Entry: st0 `STU_37072672` picks `count` entries of the entity list #10
/// into #11 from the running index 4, keeping the last picks apart.
fn shuffled(count: i64) -> GraphStore {
    store(vec![graph(
        42,
        vec![
            node(
                0,
                "STUStatescriptEntry",
                None,
                json!({"m_outPlug":plug(&[(1, "m_beginPlug")], &[])}),
            ),
            node(
                1,
                "STU_37072672",
                Some(0),
                json!({
                    "m_814F8EC3":var(10, true),
                    "m_index":{"$":"STUConfigVarInt","m_value":4},
                    "m_3DC6FAAA":var(11, true),
                    "m_E6DD2A93":{"$":"STUConfigVarInt","m_value":count},
                    "m_A07336C2":1,
                }),
            ),
        ],
        json!([1]),
        json!([0]),
    )])
}

fn picks(count: i64, list: Value) -> Vec<Value> {
    let component = Component::start(
        Arc::new(shuffled(count)),
        42,
        Variables::from([(10, list)]),
        vec![Root::new(1, 42)],
        0,
        &mut SpawnPoints,
        MAX_INIT_WORK,
    )
    .unwrap();
    assert!(component.snapshot().unwrap().diagnostics.is_empty());
    match component.snapshot().unwrap().variables.get(&11) {
        Some(Value::Array(items)) => items.clone(),
        other => panic!("the picks are an array, not {other:?}"),
    }
}

#[test]
fn the_shuffled_picker_takes_each_entry_once_a_round_and_keeps_the_last_apart() {
    let list: Vec<Value> = (0..5).map(Value::Integer).collect();
    // Three of five: three different entries of the list.
    let mut three = picks(3, Value::Array(list.clone()));
    three.sort_by_key(|value| expr::to_int(value, 0));
    three.dedup();
    assert_eq!(three.len(), 3);
    assert!(three.iter().all(|value| list.contains(value)));
    // Seven of five: a whole round, then a reshuffle that keeps the
    // round's last two (the last third) out of the next first pick.
    let seven = picks(7, Value::Array(list.clone()));
    let mut round = seven[..5].to_vec();
    round.sort_by_key(|value| expr::to_int(value, 0));
    assert_eq!(round, list);
    assert!(!seven[3..5].contains(&seven[5]), "{seven:?}");
    // None wanted, or nothing to pick: an empty array.
    assert_eq!(picks(0, Value::Array(list)), Vec::new());
    assert_eq!(picks(2, Value::Array(Vec::new())), Vec::new());
}
