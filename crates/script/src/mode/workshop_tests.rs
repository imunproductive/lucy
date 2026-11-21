//! Workshop action graphs (`054/0155` `m_graph`) run on the mode entity
//! with their parameters in `#18739`, as the rule engine starts them
//! (`refs/reports/Workshop.md` §2).

use super::*;
use crate::{
    codecs::CodecTable,
    snapshot::{SnapshotEncoder, View},
    vm::RuleTest,
};

const ASSAULT: u64 = 0x0230_0000_0000_0014;
const PLAYER: u32 = 0x100;
const OTHER_PLAYER: u32 = 0x101;

const SET_GLOBAL_VARIABLE: u32 = 0x1367;
const SET_PLAYER_VARIABLE: u32 = 0x136B;
const WAIT: u32 = 0x1370;
const SKIP_IF: u32 = 0x1DA5;
const ABORT_IF: u32 = 0x1DA1;
const ABORT_IF_CONDITION_IS_TRUE: u32 = 0x1DA2;
const LOOP: u32 = 0x1377;
const SET_GLOBAL_VARIABLE_AT_INDEX: u32 = 0x1DDD;
const SMALL_MESSAGE: u32 = 0x1D7B;
const CREATE_HUD_TEXT: u32 = 0x1D8A;
const DESTROY_HUD_TEXT: u32 = 0x1D8B;
const TELEPORT: u32 = 0x1D45;
const CREATE_ICON: u32 = 0x1AB1;
const DESTROY_ICON: u32 = 0x1C17;
const CREATE_EFFECT: u32 = 0x1CE8;

/// The Workshop value Number (`01C:4876`).
const NUMBER: u64 = 0x0D80_0000_0000_4876;

/// Create HUD Text's scope (`1D8A` st36's plug, `m_5AEA0A51` 31): it
/// targets the mode entity itself.
const HUD_TEXT_SCOPE: u32 = 31;
/// Create HUD Text's scope copies its Visible To into `#29763`
/// (`STU_12F5D52D` rid21).
const VISIBLE_TO: u32 = 29763;
/// The last HUD text id Create HUD Text gave, a mode entity variable
/// (`1D8A` rid40/rid39); Destroy HUD Text takes it as its Text ID.
const LAST_TEXT_ID: u32 = 20765;
/// Small Message's targets, `#20176`, which its client-run
/// `STU_100EF8C2` filters on.
const MESSAGE_TARGETS: u32 = 20176;

fn host() -> ModeScript {
    let data = orpheus_testkit::runtime_dir().join("data");
    let graphs = Arc::new(GraphStore::load_runtime(data, "game_graphs_174.json.gz").unwrap());
    let mut world = ModeWorld::new(graphs, ASSAULT, Unknowns::Refuse);
    world.players = vec![
        ModePlayer {
            controller: PLAYER,
            body: Some(0x200),
            hero: None,
            team: 0,
        },
        ModePlayer {
            controller: OTHER_PLAYER,
            body: Some(0x201),
            hero: None,
            team: 0,
        },
    ];
    ModeScript::start_host(world, 1).unwrap()
}

fn run(script: &mut ModeScript, graph: u32, parameters: Vec<Value>) -> WorkshopAction {
    script
        .start_workshop_action(graph, parameters)
        .unwrap()
        .unwrap_or_else(|| panic!("graph {graph:04X} is not in the bundle"))
}

fn finished(script: &ModeScript, action: WorkshopAction) -> ActionOutcome {
    match script.action_status(action) {
        ActionStatus::Finished(outcome) => outcome,
        status => panic!("{action:?}: {status:?}, gaps {:?}", script.gaps()),
    }
}

/// A constant Workshop parameter as the engine gives it: the value inside
/// Number (`4876`), whose `1B4A` config is its parameter 0.
fn constant(value: Value) -> Value {
    crate::expr::workshop_tree(NUMBER, vec![value])
}

fn assert_complete(script: &ModeScript) {
    assert!(script.complete(), "gaps: {:?}", script.gaps());
}

/// Set Global Variable `1367` writes the mode entity's Workshop variable
/// (opcode 72: `#18739[0]` → `FF7F + index`) through `STU_0BDBC7B2`, At
/// Index `1DDD` reads it (opcode 72 as a value) and sets one item, Set
/// Player Variable `136B` loops over the players: all are done at once,
/// and their instances retire.
#[test]
#[ignore = "requires the derived 104319 graph bundle"]
fn variable_actions_are_done_at_once() {
    let mut script = host();
    let set = run(
        &mut script,
        SET_GLOBAL_VARIABLE,
        vec![Value::Integer(3), constant(Value::Float(5.0))],
    );
    assert_eq!(finished(&script, set), ActionOutcome::Done);
    assert_eq!(script.workshop_global(3), Value::Float(5.0));
    let at_index = run(
        &mut script,
        SET_GLOBAL_VARIABLE_AT_INDEX,
        vec![
            Value::Integer(4),
            constant(Value::Integer(2)),
            constant(Value::Float(9.0)),
        ],
    );
    assert_eq!(finished(&script, at_index), ActionOutcome::Done);
    assert_eq!(
        script.workshop_global(4),
        Value::Array(vec![Value::Null, Value::Null, Value::Float(9.0)])
    );
    // Set Player Variable: `STU_D137ECA0` loops over the players, each
    // write goes to the server.
    let players = Value::Array(vec![Value::Entity(PLAYER), Value::Entity(OTHER_PLAYER)]);
    let player = run(
        &mut script,
        SET_PLAYER_VARIABLE,
        vec![
            constant(players),
            Value::Integer(2),
            constant(Value::Float(1.0)),
        ],
    );
    assert_eq!(finished(&script, player), ActionOutcome::Done);
    let id = crate::expr::workshop_variable(2);
    assert_eq!(
        script.take_variable_writes(),
        vec![
            (PLAYER, id, Value::Float(1.0)),
            (OTHER_PLAYER, id, Value::Float(1.0))
        ]
    );
    assert_complete(&script);
    for action in [set, at_index, player] {
        assert!(
            !script
                .snapshot()
                .unwrap()
                .instances
                .contains_key(&action.instance)
        );
    }
}

/// Wait `1370` is done when its timer runs out (0.5 s: the frame that
/// starts after 500 ms), with no gap: its `STU_E3CEF6B9`
/// (WaitForServerFrames) waits one server frame, the server's 16 ms
/// tick (`vm/timers.rs`).
#[test]
#[ignore = "requires the derived 104319 graph bundle"]
fn a_wait_is_done_when_its_time_is_up() {
    let mut script = host();
    let wait = run(
        &mut script,
        WAIT,
        vec![constant(Value::Float(0.5)), Value::Integer(0)],
    );
    assert_eq!(script.action_status(wait), ActionStatus::Running);
    let mut frame = 1;
    while script.action_status(wait) == ActionStatus::Running {
        frame += 1;
        assert!(frame < 100, "the wait never ends");
        script.advance(frame).unwrap();
    }
    assert_eq!(finished(&script, wait), ActionOutcome::Done);
    let elapsed = crate::timeline::frame_start(frame) - wait.start_ms;
    // The timer runs out within the frame `elapsed` after the start began.
    let frame = crate::timeline::FRAME_MS;
    assert!((500 - frame + 1..=500).contains(&elapsed), "{elapsed} ms");
    assert_eq!(script.gaps(), BTreeMap::new());
}

/// The flow actions report their result: Skip If with a true condition
/// skips its count, Abort If stops the rule, its "If Condition Is True"
/// form leaves the test to the rule's conditions, Loop starts again.
#[test]
#[ignore = "requires the derived 104319 graph bundle"]
fn flow_actions_report_their_result() {
    let mut script = host();
    let skip = run(
        &mut script,
        SKIP_IF,
        vec![constant(Value::Bool(true)), constant(Value::Integer(3))],
    );
    assert_eq!(finished(&script, skip), ActionOutcome::Skip(3));
    let no_skip = run(
        &mut script,
        SKIP_IF,
        vec![constant(Value::Bool(false)), constant(Value::Integer(3))],
    );
    assert_eq!(finished(&script, no_skip), ActionOutcome::Done);
    let abort = run(&mut script, ABORT_IF, vec![constant(Value::Bool(true))]);
    assert_eq!(
        finished(&script, abort),
        ActionOutcome::Abort(RuleTest::Always)
    );
    let no_abort = run(&mut script, ABORT_IF, vec![constant(Value::Bool(false))]);
    assert_eq!(finished(&script, no_abort), ActionOutcome::Done);
    let by_conditions = run(&mut script, ABORT_IF_CONDITION_IS_TRUE, Vec::new());
    assert_eq!(
        finished(&script, by_conditions),
        ActionOutcome::Abort(RuleTest::IfConditionsTrue)
    );
    let again = run(&mut script, LOOP, Vec::new());
    assert_eq!(
        finished(&script, again),
        ActionOutcome::Loop(RuleTest::Always)
    );
    assert_complete(&script);
}

/// Create HUD Text `1D8A` is done at once and keeps its text up: its scope
/// state opens scope 31 on the mode entity itself, whose instance holds
/// the Visible To player for the client's presenters, in the frames every
/// client gets. Destroy HUD Text `1D8B` sends `0E35` with the text's id
/// (the last created id, `#20765`), which ends it.
#[test]
#[ignore = "requires the derived 104319 graph bundle"]
fn hud_text_stays_up_until_destroyed() {
    let mut script = host();
    let text = |value: &str| constant(Value::String(value.into()));
    let identifier = |id: u64| Value::Asset(0x0D80_0000_0000_0000 | id);
    let create = run(
        &mut script,
        CREATE_HUD_TEXT,
        vec![
            constant(Value::Array(vec![Value::Entity(PLAYER)])),
            text("Level 1"),
            text(""),
            text("Checkpoint 3"),
            // The enums' catalog defaults: Location, then Sort Order and
            // the colours, Reevaluation, Non-Team Spectators.
            identifier(0x51F3),
            constant(Value::Float(0.0)),
            constant(Value::Null),
            constant(Value::Null),
            constant(Value::Null),
            identifier(0x51AA),
            identifier(0x5C26),
        ],
    );
    assert_eq!(finished(&script, create), ActionOutcome::Done);
    assert_complete(&script);
    let snapshot = script.snapshot().unwrap();
    let (&scope, _) = snapshot
        .instances
        .iter()
        .find(|(_, instance)| {
            instance.graph == CREATE_HUD_TEXT
                && instance.scope.as_ref().is_some_and(|scope| {
                    scope.id == HUD_TEXT_SCOPE && scope.source.instance == create.instance
                })
        })
        .expect("the text's scope instance on the mode entity");
    let graphs = Arc::clone(&script.world.graphs);
    let codecs = CodecTable::client();
    let encoder = SnapshotEncoder::new(&graphs, &codecs);
    let projected = encoder.project(snapshot, View::Observer).unwrap();
    assert!(encoder.full(&projected).is_ok());
    assert!(projected.has_instance(create.instance));
    assert!(projected.has_instance(scope));
    assert_eq!(
        projected.instance_variable(scope, VISIBLE_TO),
        Some(&constant(Value::Array(vec![Value::Entity(PLAYER)])))
    );
    // Destroy HUD Text sends `0E35` with the id to the mode entity, which
    // takes it at once (a message to its own entity).
    let id = script.snapshot().unwrap().variables[&LAST_TEXT_ID].clone();
    let destroy = run(&mut script, DESTROY_HUD_TEXT, vec![constant(id)]);
    assert_eq!(finished(&script, destroy), ActionOutcome::Done);
    script.advance(2).unwrap();
    let instances = &script.snapshot().unwrap().instances;
    assert!(!instances.contains_key(&scope));
    assert!(
        !instances.contains_key(&create.instance),
        "{:?}",
        instances[&create.instance]
    );
    assert_complete(&script);
}

/// Small Message `1D7B` is done at once; its `STU_100EF8C2` (the client
/// sends `0F70` to its mode entity when the local player is in `#20176`)
/// goes in the mode entity's frames as a networked action of the
/// instance, which stays while the frames carry it.
#[test]
#[ignore = "requires the derived 104319 graph bundle"]
fn small_message_reaches_its_target_in_the_frames() {
    let mut script = host();
    let message = run(
        &mut script,
        SMALL_MESSAGE,
        vec![
            constant(Value::Array(vec![Value::Entity(PLAYER)])),
            constant(Value::String("Checkpoint!".into())),
        ],
    );
    assert_eq!(finished(&script, message), ActionOutcome::Done);
    assert_complete(&script);
    let snapshot = script.snapshot().unwrap();
    assert!(snapshot.frame_actions.iter().any(|action| {
        action.instance == message.instance && action.instance_start_ms == message.start_ms
    }));
    let graphs = Arc::clone(&script.world.graphs);
    let codecs = CodecTable::client();
    let encoder = SnapshotEncoder::new(&graphs, &codecs);
    let projected = encoder.project(snapshot, View::Observer).unwrap();
    assert!(encoder.full(&projected).is_ok());
    assert_eq!(
        projected.instance_variable(message.instance, MESSAGE_TARGETS),
        Some(&Value::Array(vec![Value::Entity(PLAYER)]))
    );
    // The frames carry it for `ACTION_LIFETIME_MS`, then it retires.
    let frames = crate::snapshot::ACTION_LIFETIME_MS / crate::timeline::FRAME_MS + 2;
    for frame in 2..2 + frames as u32 {
        script.advance(frame).unwrap();
    }
    let instances = &script.snapshot().unwrap().instances;
    assert!(
        !instances.contains_key(&message.instance),
        "{:?}",
        instances[&message.instance]
    );
}

/// Create Icon `1AB1` with Reevaluation "Visible To" (`5069`): its scope
/// 47 on the mode entity copies Visible To into `#20176` (rid13, a plain
/// copy), which StateStack st2 evaluates on each client. C32S2's
/// level-select icons are made at the match start, visible to the players
/// with `CPData[13]`, then none; when the rules evaluate the player in, the
/// copy follows and the stack the frames carry takes it in the next frame
/// (in the game the icons then show from the hub, ParkourMatch9).
#[test]
#[ignore = "requires the derived 104319 graph bundle"]
fn create_icon_follows_its_reevaluated_visible_to() {
    const ICON_SCOPE: u32 = 47;
    /// The scope instance's copy of Visible To.
    const ICON_VISIBLE_TO: u32 = 20176;
    let mut script = host();
    let identifier = |id: u64| Value::Asset(0x0D80_0000_0000_0000 | id);
    let create = run(
        &mut script,
        CREATE_ICON,
        vec![
            constant(Value::Array(Vec::new())),
            constant(Value::Vector([18.363, 5.867, 9.094])),
            // Checkmark; "Visible To".
            identifier(0x551C),
            identifier(0x5069),
            constant(Value::Null),
            constant(Value::Bool(false)),
        ],
    );
    assert_eq!(finished(&script, create), ActionOutcome::Done);
    let scope = |script: &ModeScript| {
        let snapshot = script.snapshot().unwrap();
        let (&scope, instance) = snapshot
            .instances
            .iter()
            .find(|(_, instance)| {
                instance.scope.as_ref().is_some_and(|scope| {
                    scope.id == ICON_SCOPE && scope.source.instance == create.instance
                })
            })
            .expect("the icon's scope instance on the mode entity");
        (scope, instance.variables.get(&ICON_VISIBLE_TO).cloned())
    };
    let (_, copied) = scope(&script);
    assert_eq!(copied, Some(constant(Value::Array(Vec::new()))));
    script
        .set_action_parameters(
            create,
            &[(0, constant(Value::Array(vec![Value::Entity(PLAYER)])))],
        )
        .unwrap();
    let (id, copied) = scope(&script);
    assert_eq!(
        copied,
        Some(constant(Value::Array(vec![Value::Entity(PLAYER)])))
    );
    // The stack st2 takes the new copy in the next frame, and the frames
    // carry its value.
    script.advance(2).unwrap();
    let graphs = Arc::clone(&script.world.graphs);
    let codecs = CodecTable::client();
    let encoder = SnapshotEncoder::new(&graphs, &codecs);
    let projected = encoder
        .project(script.snapshot().unwrap(), View::Observer)
        .unwrap();
    assert_eq!(
        projected.instance_variable(id, ICON_VISIBLE_TO),
        Some(&Value::Array(vec![Value::Entity(PLAYER)]))
    );
    assert_complete(&script);
}

/// Create Effect `1CE8` with Reevaluation "Visible To" (`5069`): its scope
/// 35 opens on the `304A` it made, not on the mode entity, and copies
/// Visible To into `#20176` there (rid14, a plain copy), which StateStack
/// st0 evaluates on each client; the parameters the Reevaluation leaves
/// out the graph freezes to their values (rid23 → rid27: the radius,
/// `#18739[4]`, copied to `#2101`, which only StateStack st4 would
/// evaluate, and st4 runs only for a reevaluated radius). C32S2's level
/// rings are made at the match start, visible to the players with
/// `CPData[13]`: when the rules evaluate a player in, the copy on the
/// effect follows, the frozen radius stays a number, and the effect's
/// frames carry the stack's value. Before, only the mode entity's own
/// instances took the new parameters, and the reevaluation sent every
/// parameter again as a value tree, so the client's ring had a tree for
/// a radius and drew nothing.
#[test]
#[ignore = "requires the derived 104319 graph bundle"]
fn create_effect_follows_its_reevaluated_visible_to() {
    const EFFECT: u64 = 0x0400_0000_0000_304A;
    const EFFECT_SCOPE: u32 = 35;
    const EFFECT_VISIBLE_TO: u32 = 20176;
    const EFFECT_RADIUS: u32 = 2101;
    let mut script = host();
    let identifier = |id: u64| Value::Asset(0x0D80_0000_0000_0000 | id);
    let create = run(
        &mut script,
        CREATE_EFFECT,
        vec![
            constant(Value::Array(Vec::new())),
            // Ring, white, level 1's first checkpoint, "Visible To".
            identifier(0x5346),
            constant(identifier(0x50AD)),
            constant(Value::Vector([18.363, 4.117, 9.094])),
            constant(Value::Float(1.872)),
            identifier(0x5069),
        ],
    );
    assert_eq!(finished(&script, create), ActionOutcome::Done);
    let [made] = script.created() else {
        panic!("{:?}", script.created());
    };
    assert_eq!(made.definition, EFFECT);
    let ring = made.entity;
    let scope = |script: &ModeScript| {
        let snapshot = script.created_script(ring).unwrap().snapshot().unwrap();
        let (&scope, instance) = snapshot
            .instances
            .iter()
            .find(|(_, instance)| {
                instance.scope.as_ref().is_some_and(|scope| {
                    scope.id == EFFECT_SCOPE && scope.source.instance == create.instance
                })
            })
            .expect("the effect's scope instance on the ring");
        let variable = |id: u32| instance.variables.get(&id).cloned();
        (scope, variable(EFFECT_VISIBLE_TO), variable(EFFECT_RADIUS))
    };
    let (_, copied, radius) = scope(&script);
    assert_eq!(copied, Some(constant(Value::Array(Vec::new()))));
    assert_eq!(radius, Some(Value::Float(1.872)));
    script
        .set_action_parameters(
            create,
            &[(0, constant(Value::Array(vec![Value::Entity(PLAYER)])))],
        )
        .unwrap();
    let (id, copied, radius) = scope(&script);
    assert_eq!(
        copied,
        Some(constant(Value::Array(vec![Value::Entity(PLAYER)])))
    );
    assert_eq!(radius, Some(Value::Float(1.872)));
    script.advance(2).unwrap();
    let graphs = Arc::clone(&script.world.graphs);
    let codecs = CodecTable::client();
    let encoder = SnapshotEncoder::new(&graphs, &codecs);
    let snapshot = script.created_script(ring).unwrap().snapshot().unwrap();
    let projected = encoder.project(snapshot, View::Observer).unwrap();
    assert_eq!(
        projected.instance_variable(id, EFFECT_VISIBLE_TO),
        Some(&Value::Array(vec![Value::Entity(PLAYER)]))
    );
    assert_eq!(
        projected.instance_variable(id, EFFECT_RADIUS),
        Some(&Value::Float(1.872))
    );
    assert_complete(&script);
}

/// Destroy Icon `1C17` sends `0DFA` with its entity to the mode entity,
/// where each Create Icon `1AB1`'s scope 47 takes it when it names its own
/// icon (rid4, `0773` == `#38`): it drops the icon from the created list
/// `#20554e` (rid2) and removes the icon (rid1 `STU_523EE38D` on `#38`),
/// so the clients lose it; the other icon stays. The entity is the one
/// Last Created Entity gives, `#20555e` (`1B4A`'s `4C31`).
#[test]
#[ignore = "requires the derived 104319 graph bundle"]
fn destroy_icon_removes_the_icon_it_names() {
    /// Last Created Entity and the effects and icons up.
    const LAST_CREATED: u32 = 20555;
    const CREATED_ENTITIES: u32 = 20554;
    let mut script = host();
    let identifier = |id: u64| Value::Asset(0x0D80_0000_0000_0000 | id);
    let mut icons = Vec::new();
    for position in [[18.363, 5.867, 9.094], [10.045, 5.843, 9.007]] {
        let create = run(
            &mut script,
            CREATE_ICON,
            vec![
                constant(Value::Array(vec![Value::Entity(PLAYER)])),
                constant(Value::Vector(position)),
                // Checkmark; "None".
                identifier(0x551C),
                identifier(0x506A),
                constant(Value::Null),
                constant(Value::Bool(false)),
            ],
        );
        assert_eq!(finished(&script, create), ActionOutcome::Done);
        let Value::Entity(icon) = script.entity_value(LAST_CREATED) else {
            panic!("{:?}", script.entity_value(LAST_CREATED));
        };
        icons.push((icon, create.instance));
    }
    let [(first, first_action), (second, second_action)] = icons[..] else {
        unreachable!()
    };
    assert_eq!(
        script.entity_value(CREATED_ENTITIES),
        Value::Array(vec![Value::Entity(first), Value::Entity(second)])
    );
    let destroy = run(
        &mut script,
        DESTROY_ICON,
        vec![constant(Value::Entity(first))],
    );
    assert_eq!(finished(&script, destroy), ActionOutcome::Done);
    script.advance(2).unwrap();
    let made: Vec<u32> = script.created().iter().map(|made| made.entity).collect();
    assert_eq!(made, [second]);
    assert_eq!(
        script.entity_value(CREATED_ENTITIES),
        Value::Array(vec![Value::Entity(second)])
    );
    let scopes: Vec<u16> = script
        .snapshot()
        .unwrap()
        .instances
        .values()
        .filter_map(|instance| instance.scope.as_ref())
        .map(|scope| scope.source.instance)
        .collect();
    assert!(!scopes.contains(&first_action), "{scopes:?}");
    assert!(scopes.contains(&second_action), "{scopes:?}");
    assert_complete(&script);
}

/// Teleport `1D45` opens its scope on each target player for the match
/// to run there, and is done at once.
#[test]
#[ignore = "requires the derived 104319 graph bundle"]
fn teleport_opens_its_scope_on_the_target() {
    let mut script = host();
    let teleport = run(
        &mut script,
        TELEPORT,
        vec![
            constant(Value::Array(vec![Value::Entity(PLAYER)])),
            constant(Value::Vector([10.0, 2.0, -5.0])),
        ],
    );
    assert_eq!(finished(&script, teleport), ActionOutcome::Done);
    let changes = script.take_scope_changes();
    // Opened, then closed at once as the instance exits (`STU_22926252`):
    // the state keeps it (`m_4B7DF406`), so it runs on at the target.
    assert!(
        matches!(
            changes.as_slice(),
            [
                ScopeChange::Open {
                    target: PLAYER,
                    graph: TELEPORT,
                    keep: true,
                    ..
                },
                ScopeChange::Close { target: PLAYER, .. }
            ]
        ),
        "{changes:?}"
    );
    assert_complete(&script);
}

/// Set Match Time `1ABC` sends `08D8` to the mode entity with its seconds
/// as `017E`: the mode entity's own entries take it, and the match flow,
/// which stands in for `0F4A`'s, gets it from `take_own_messages`.
#[test]
#[ignore = "requires the derived 104319 graph bundle"]
fn set_match_time_reaches_the_flow_as_an_own_message() {
    let mut script = host();
    run(&mut script, 0x1ABC, vec![constant(Value::Float(1.0))]);
    let own = script.take_own_messages();
    assert!(
        own.iter()
            .any(|message| message.message == 0x0240_0000_0000_08d8
                && message
                    .parameters
                    .get(&0x017e)
                    .is_some_and(|seconds| match seconds {
                        Value::Float(seconds) => *seconds == 1.0,
                        Value::Integer(seconds) => *seconds == 1,
                        _ => false,
                    })),
        "{own:?}"
    );
    assert!(script.take_own_messages().is_empty());
}
