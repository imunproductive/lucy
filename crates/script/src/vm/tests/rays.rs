//! Ray casts (`vm/rays.rs`): the action `STU_08D6BA6E`, RayCast and
//! TrackRay against a map wall and the listed bodies.

use std::sync::Mutex;

use super::*;

const PROFILE: u64 = 0x0690_0000_0000_0065;
/// A profile whose rays don't test the bodies' hit shapes (`00A1`, which
/// stores no `m_0220680A`): they pass every body.
const PASSING: u64 = 0x0690_0000_0000_00A1;
const TRACK_RAY: &str = "STUStatescriptStateTrackRay";

/// A wall facing −x at `wall` (none: no wall), and the bodies.
#[derive(Debug)]
struct Range {
    bodies: Vec<TargetCandidate>,
    wall: Option<f64>,
    profiles: Mutex<Vec<Option<u64>>>,
}

impl Range {
    fn new(wall: Option<f64>) -> Self {
        Self {
            bodies: vec![
                body(SEARCHER, 0, [0.0; 3]),
                body(ALLY, 0, [2.0, 0.0, 0.0]),
                body(ENEMY, 1, [5.0, 0.0, 0.0]),
            ],
            wall,
            profiles: Mutex::new(Vec::new()),
        }
    }
}

impl World for Range {
    fn target_candidates(&self, _site: Site) -> Option<&[TargetCandidate]> {
        Some(&self.bodies)
    }

    fn has_target_tag(&self, _site: Site, entity: u32, tag: u64) -> Option<bool> {
        let body = self.bodies.iter().find(|body| body.entity == entity)?;
        Some(body.tags.as_ref()?.contains(&tag))
    }

    fn map_rays(&self) -> Option<&dyn MapRays> {
        Some(self)
    }

    /// The searcher's eye, 1.5 m over its feet, looking along +x.
    fn eye(&self, _site: Site, _entity: u32) -> Option<([f64; 3], [i32; 2])> {
        Some(([0.0, 1.5, 0.0], [16384, 0]))
    }
}

impl MapRays for Range {
    fn cast(
        &self,
        origin: [f64; 3],
        direction: [f64; 3],
        reach: f64,
        profile: Option<u64>,
    ) -> Option<Option<MapHit>> {
        self.profiles.lock().unwrap().push(profile);
        let hit = self.wall.and_then(|wall| {
            let distance = (wall - origin[0]) / direction[0];
            (direction[0] > 0.0 && distance <= reach).then(|| MapHit {
                distance,
                point: std::array::from_fn(|axis| origin[axis] + direction[axis] * distance),
                normal: [-1.0, 0.0, 0.0],
            })
        });
        Some(hit)
    }

    fn meets_bodies(&self, profile: Option<u64>) -> bool {
        profile != Some(PASSING)
    }
}

fn vec3(x: f64, y: f64, z: f64) -> Json {
    json!({"$":"STUConfigVarVec3","m_value":{"X":x,"Y":y,"Z":z}})
}

/// Enemies only (`m_89831602` 1: no friendly body), and the ray profile.
fn enemies() -> Json {
    enemies_with(PROFILE)
}

fn enemies_with(profile: u64) -> Json {
    json!({"$":"STUConfigVarFilter","m_84065FD1":1,"m_1E14CCA8":0,"m_4EA686DB":16,
        "m_27CF71D5":0,"m_66747E7D":0,"m_89831602":1,"m_condition":null,
        "m_C1903771":{"$":"STU_20DB8AB5","m_C1903771":format!("0x{profile:016X}"),
            "m_resourceKey":null}})
}

/// No body passes masks of zero: the map only.
fn map_only() -> Json {
    json!({"$":"STUConfigVarFilter","m_84065FD1":0,"m_1E14CCA8":0,"m_4EA686DB":0,
        "m_27CF71D5":0,"m_66747E7D":0,"m_89831602":0})
}

/// The action's and RayCast's fields: from a metre over the origin along
/// +x for 10 m; hit to #10, entity #11, point #12, normal #13.
fn cast_fields(filter: Json) -> Json {
    json!({
        "m_startPosWS":vec3(0.0, 1.0, 0.0),
        "m_dirWS":vec3(2.0, 0.0, 0.0),
        "m_castLength":{"$":"STUConfigVarFloat","m_value":10.0},
        "m_castFilter":filter,
        "m_out_HitSomething":var(10, false),
        "m_out_HitEntity":var(11, false),
        "m_out_HitPointWS":var(12, false),
        "m_out_HitNormal":var(13, false),
        "m_type":0,
    })
}

fn one_node(class: &str, state: bool, fields: Json) -> GraphStore {
    let plug_name = if state { "m_beginPlug" } else { "m_inPlug" };
    let mut fields = fields;
    if !state {
        fields["m_outPlug"] = plug(&[], &[]);
    }
    store(vec![graph(
        42,
        vec![
            entry(&[(1, plug_name)]),
            node(1, class, state.then_some(0), fields),
        ],
        if state { json!([1]) } else { json!([]) },
        json!([0]),
    )])
}

fn start(graphs: GraphStore, world: &mut Range) -> Component {
    start_bound(graphs, world, &[])
}

/// [`start`] with instance variables bound at the root.
fn start_bound(graphs: GraphStore, world: &mut Range, bound: &[(u32, Value)]) -> Component {
    let mut root = Root::new(1, 42);
    root.variables.extend(bound.iter().cloned());
    Component::start(
        Arc::new(graphs),
        SEARCHER,
        Variables::new(),
        vec![root],
        0,
        world,
        MAX_INIT_WORK,
    )
    .unwrap()
}

/// A TrackRay from a metre over the origin along +x for 6 m, its hit to
/// #20, entity to #21, hit entities to #22, point to #24; `extra` fields
/// on top.
fn track_fields(filter: Json, extra: Json) -> Json {
    let mut fields = json!({
        "m_3BC31C57":vec3(0.0, 1.0, 0.0),
        "m_F5732403":{"$":"STU_9E9028E6","m_direction":vec3(1.0, 0.0, 0.0),
            "m_6A79BD98":{"$":"STUConfigVarFloat","m_value":6.0}},
        "m_filter":filter,
        "m_target":{"$":"STU_404DE6D3",
            "m_7BA44A69":var(20, false), "m_692DFF72":var(21, false),
            "m_750377AA":var(22, false), "m_F2F8606D":var(24, false)},
        "m_1DA04609":1, "m_E4A295CE":1, "m_1C2A9945":1,
    });
    for (key, value) in extra.as_object().unwrap() {
        fields[key] = value.clone();
    }
    fields
}

/// Every kind of body (`m_84065FD1` `0x3FFFFFFF`), as the hero rays below
/// use it.
fn every_body() -> Json {
    json!({"$":"STUConfigVarFilter","m_84065FD1":1073741823,"m_1E14CCA8":0,"m_4EA686DB":0,
        "m_27CF71D5":0,"m_66747E7D":0,"m_89831602":0})
}

fn assumed(component: &Component, class: &str) -> bool {
    component
        .snapshot()
        .unwrap()
        .assumptions
        .iter()
        .any(|item| item.class == class)
}

fn outputs(component: &Component, ids: &[u32]) -> Vec<Value> {
    let variables = &component.snapshot().unwrap().instances[&1].variables;
    ids.iter()
        .map(|id| variables.get(id).cloned().unwrap_or(Value::Null))
        .collect()
}

fn close(value: &Value, expected: [f64; 3]) -> bool {
    matches!(value, Value::Vector(vector)
        if (0..3).all(|axis| (vector[axis] - expected[axis]).abs() < 1e-9))
}

/// The action meets the enemy's capsule before the wall, passing the
/// caster and the ally its filter leaves out; with masks of zero, or a ray
/// profile that meets no body, it meets the wall, which has no entity. The
/// filter's ray profile reaches the map.
#[test]
fn the_cast_action_meets_the_nearest_body_its_filter_passes_or_the_map() {
    let mut world = Range::new(Some(8.0));
    let component = start(
        one_node("STU_08D6BA6E", false, cast_fields(enemies())),
        &mut world,
    );
    let values = outputs(&component, &[10, 11, 12, 13]);
    assert_eq!(values[..2], [Value::Bool(true), Value::Entity(ENEMY)]);
    assert!(close(&values[2], [4.6, 1.0, 0.0]), "{values:?}");
    assert!(close(&values[3], [-1.0, 0.0, 0.0]), "{values:?}");
    assert_eq!(*world.profiles.lock().unwrap(), [Some(PROFILE)]);

    let mut world = Range::new(Some(8.0));
    let component = start(
        one_node("STU_08D6BA6E", false, cast_fields(map_only())),
        &mut world,
    );
    let values = outputs(&component, &[10, 11, 12, 13]);
    assert_eq!(values[..2], [Value::Bool(true), Value::Entity(0)]);
    assert!(close(&values[2], [8.0, 1.0, 0.0]), "{values:?}");
    assert_eq!(*world.profiles.lock().unwrap(), [None]);

    // A ray of a profile that doesn't test the bodies' hit shapes passes
    // the enemy its filter passes.
    let mut world = Range::new(Some(8.0));
    let component = start(
        one_node("STU_08D6BA6E", false, cast_fields(enemies_with(PASSING))),
        &mut world,
    );
    let values = outputs(&component, &[10, 11, 12, 13]);
    assert_eq!(values[..2], [Value::Bool(true), Value::Entity(0)]);
    assert!(close(&values[2], [8.0, 1.0, 0.0]), "{values:?}");

    // A miss writes the hit and the entity, not the point and normal.
    let mut world = Range::new(None);
    let component = start(
        one_node("STU_08D6BA6E", false, cast_fields(map_only())),
        &mut world,
    );
    assert_eq!(
        outputs(&component, &[10, 11, 12, 13]),
        [
            Value::Bool(false),
            Value::Entity(0),
            Value::Null,
            Value::Null
        ]
    );
}

/// RayCast casts at its begin and again each tick: the enemy walking off
/// leaves the wall; the wall gone, it misses and keeps the last point.
#[test]
fn a_ray_cast_state_follows_what_its_ray_meets() {
    let mut world = Range::new(Some(8.0));
    let mut component = start(
        one_node("STUStatescriptStateRayCast", true, cast_fields(enemies())),
        &mut world,
    );
    assert_eq!(
        outputs(&component, &[10, 11]),
        [Value::Bool(true), Value::Entity(ENEMY)]
    );
    world.bodies[2].feet = [5.0, 0.0, 9.0];
    component.advance_frame(1, &mut world).unwrap();
    let values = outputs(&component, &[10, 11, 12]);
    assert_eq!(values[..2], [Value::Bool(true), Value::Entity(0)]);
    assert!(close(&values[2], [8.0, 1.0, 0.0]), "{values:?}");
    world.wall = None;
    component.advance_frame(2, &mut world).unwrap();
    let values = outputs(&component, &[10, 11, 12]);
    assert_eq!(values[..2], [Value::Bool(false), Value::Entity(0)]);
    assert!(close(&values[2], [8.0, 1.0, 0.0]), "{values:?}");
}

/// TrackRay from a world point to its end: a hit fills its target's
/// fields; a miss's point is the end and its normal points back.
#[test]
fn a_track_ray_writes_its_targets_fields() {
    let target = json!({"$":"STU_404DE6D3",
        "m_7BA44A69":var(20, false), "m_692DFF72":var(21, false),
        "m_750377AA":var(22, false), "m_5ED803CF":var(23, false),
        "m_F2F8606D":var(24, false), "m_A7C2505B":var(25, false)});
    let fields = |filter: Json| {
        json!({
            "m_3BC31C57":vec3(0.0, 1.0, 0.0),
            "m_F5732403":{"$":"STU_9E9028E6","m_direction":vec3(1.0, 0.0, 0.0),
                "m_6A79BD98":{"$":"STUConfigVarFloat","m_value":6.0}},
            "m_filter":filter,
            "m_target":target.clone(),
            "m_1DA04609":1, "m_E4A295CE":1, "m_1C2A9945":1,
        })
    };
    let mut world = Range::new(None);
    let component = start(
        one_node("STUStatescriptStateTrackRay", true, fields(enemies())),
        &mut world,
    );
    let values = outputs(&component, &[20, 21, 22, 23, 24, 25]);
    assert_eq!(
        values[..3],
        [
            Value::Bool(true),
            Value::Entity(ENEMY),
            Value::Array(vec![Value::Entity(ENEMY)])
        ]
    );
    assert!(close(&values[3], [4.6, 1.0, 0.0]), "{values:?}");
    assert!(close(&values[4], [4.6, 1.0, 0.0]), "{values:?}");
    assert!(close(&values[5], [-1.0, 0.0, 0.0]), "{values:?}");

    let mut world = Range::new(None);
    let component = start(
        one_node("STUStatescriptStateTrackRay", true, fields(map_only())),
        &mut world,
    );
    let values = outputs(&component, &[20, 21, 22, 23, 24, 25]);
    assert_eq!(
        values[..4],
        [
            Value::Bool(false),
            Value::Entity(0),
            Value::Array(Vec::new()),
            Value::Null
        ]
    );
    assert!(close(&values[4], [6.0, 1.0, 0.0]), "{values:?}");
    assert!(close(&values[5], [-1.0, 0.0, 0.0]), "{values:?}");
}

/// Type 1 is not modelled: the state casts nothing and says so; a
/// world without a map does the same.
#[test]
fn a_ray_the_server_cant_cast_writes_nothing() {
    let mut fields = cast_fields(enemies());
    fields["m_type"] = json!(1);
    let mut world = Range::new(Some(8.0));
    let component = start(
        one_node("STUStatescriptStateRayCast", true, fields),
        &mut world,
    );
    let snapshot = component.snapshot().unwrap();
    assert!(!snapshot.instances[&1].variables.contains_key(&10));
    assert!(
        snapshot
            .assumptions
            .iter()
            .any(|item| item.class == "STUStatescriptStateRayCast m_type"
                && item.answer == "casts nothing")
    );

    let snapshot = initialize(
        &one_node("STU_08D6BA6E", false, cast_fields(map_only())),
        42,
        Variables::new(),
        vec![Root::new(1, 42)],
        &mut Bodies(Vec::new()),
        MAX_INIT_WORK,
    )
    .unwrap();
    assert!(!snapshot.variables.contains_key(&10));
    assert!(
        snapshot
            .assumptions
            .iter()
            .any(|item| item.class == "STU_08D6BA6E map")
    );
}

/// A cast's filter tests its tag lists and condition on each body the ray
/// meets, nearest first, as a target search does: Brigitte's bash ray
/// (`0EE3` st81) needs tag `0385.06C` (`m_B486603E`), so it passes the
/// untagged ally for the tagged enemy; Roadhog's hook rays (`01C9` st21,
/// `param(8) != #40`) pass the body they hooked (#40). A tag the world
/// can't tell casts nothing and names the list.
#[test]
fn a_rays_filter_tests_its_tag_lists_and_condition_on_each_body() {
    const BASH_TAG: u64 = 0x0d60_0000_0000_0385;
    let tagged = || {
        let mut world = Range::new(None);
        world.bodies[1].tags = Some(Arc::new(Default::default()));
        world.bodies[2].tags = Some(Arc::new([BASH_TAG].into_iter().collect()));
        world
    };
    let mut bash = every_body();
    bash["m_B486603E"] = json!({"$":"STU_357DABBB",
        "m_7D7C86A1":format!("0x{BASH_TAG:016X}"),"m_resourceKey":null});
    let graph = || one_node(TRACK_RAY, true, track_fields(bash.clone(), json!({})));
    let component = start(graph(), &mut tagged());
    assert_eq!(
        outputs(&component, &[20, 21]),
        [Value::Bool(true), Value::Entity(ENEMY)]
    );
    let component = start(graph(), &mut Range::new(None));
    assert_eq!(outputs(&component, &[20]), [Value::Null]);
    assert!(assumed(
        &component,
        "STUStatescriptStateTrackRay filter.m_B486603E"
    ));

    let mut hook = every_body();
    hook["m_condition"] = json!({"$":"STU_919FD47C","m_expression":{
        "m_opcodes":[13,0,8,1,38,0], "m_dynamicVars":[var(8, false), var(40, false)]}});
    let component = start_bound(
        one_node(TRACK_RAY, true, track_fields(hook, json!({}))),
        &mut Range::new(None),
        &[(40, Value::Entity(ALLY))],
    );
    assert_eq!(
        outputs(&component, &[20, 21]),
        [Value::Bool(true), Value::Entity(ENEMY)]
    );
}

/// What a TrackRay leaves out: the entities of `m_7AB75223` (Moira's
/// grasp `0F76` st31: `#375[0]`, the body its search found) beside
/// `m_ignoreEntities`. The caster is left out only when listed: an action
/// cast at it from behind meets it unless `m_ignoreEntities` names the
/// owner (as `0C43` st2 and `1A2A` st14 do).
#[test]
fn a_ray_leaves_out_its_listed_entities_and_only_them() {
    let second = json!({"$":"STU_919FD47C","m_expression":{
        "m_opcodes":[8,0,6,0,17,0], "m_dynamicVars":[var(375, false)],
        "m_D99EF254":[0.0]}});
    let component = start_bound(
        one_node(
            TRACK_RAY,
            true,
            track_fields(every_body(), json!({"m_7AB75223":second})),
        ),
        &mut Range::new(None),
        &[(
            375,
            Value::Array(vec![Value::Entity(ALLY), Value::Entity(ENEMY)]),
        )],
    );
    assert_eq!(
        outputs(&component, &[20, 21]),
        [Value::Bool(true), Value::Entity(ENEMY)]
    );

    let mut fields = cast_fields(every_body());
    fields["m_startPosWS"] = vec3(-3.0, 1.0, 0.0);
    let component = start(
        one_node("STU_08D6BA6E", false, fields.clone()),
        &mut Range::new(None),
    );
    assert_eq!(
        outputs(&component, &[10, 11]),
        [Value::Bool(true), Value::Entity(SEARCHER)]
    );
    fields["m_ignoreEntities"] = json!([{"$":"STU_B5A0CAF0"}]);
    let component = start(
        one_node("STU_08D6BA6E", false, fields),
        &mut Range::new(None),
    );
    assert_eq!(
        outputs(&component, &[10, 11]),
        [Value::Bool(true), Value::Entity(ALLY)]
    );
}

/// A TrackRay's start in its entity's transform turns with the entity's
/// yaw, while its `STU_9E9028E6` direction is the world's: the Lucioball
/// ball's ground ray (`0C43` st26: from the owner, `(0, -1, 0)` for
/// 1.1 m) ends 1.1 m under the owner's feet whichever way it faces, and
/// Roadhog's hook start (`01C9` st21: `(0, 1, 0)` in the owner's
/// transform) is a metre over them. A start ahead of a body facing +x
/// lies along +x, and the ray from there leaves its start entity out.
#[test]
fn a_track_ray_starts_in_its_entitys_transform_and_casts_in_the_worlds() {
    let owner = json!({"$":"STU_B5A0CAF0"});
    let lucioball = json!({
        "m_56B8547E":owner,
        "m_3BC31C57":null,
        "m_F5732403":{"$":"STU_9E9028E6","m_direction":vec3(0.0, -1.0, 0.0),
            "m_6A79BD98":{"$":"STUConfigVarFloat","m_value":1.1}},
    });
    let mut world = Range::new(None);
    world.bodies[0].yaw = std::f64::consts::FRAC_PI_2;
    let component = start(
        one_node(TRACK_RAY, true, track_fields(map_only(), lucioball)),
        &mut world,
    );
    let values = outputs(&component, &[20, 24]);
    assert_eq!(values[0], Value::Bool(false));
    // The length is an f32 config (1.1 is 1.10000002...).
    assert!(
        matches!(&values[1], Value::Vector([x, y, z])
            if x.abs() < 1e-9 && (y + 1.1).abs() < 1e-6 && z.abs() < 1e-9),
        "{values:?}"
    );

    // Ahead of a body facing +x: along +x, to the enemy past the ally.
    let ahead = json!({"m_56B8547E":owner, "m_3BC31C57":vec3(0.0, 1.0, 3.0)});
    let component = start(
        one_node(TRACK_RAY, true, track_fields(every_body(), ahead)),
        &mut world,
    );
    assert_eq!(
        outputs(&component, &[20, 21]),
        [Value::Bool(true), Value::Entity(ENEMY)]
    );

    // Roadhog's start: a metre over the feet; back along -x it meets
    // nothing, its end 6 m away.
    let mut hook = json!({"m_56B8547E":owner, "m_3BC31C57":vec3(0.0, 1.0, 0.0)});
    hook["m_F5732403"] = json!({"$":"STU_9E9028E6","m_direction":vec3(-1.0, 0.0, 0.0),
        "m_6A79BD98":{"$":"STUConfigVarFloat","m_value":6.0}});
    let component = start(
        one_node(TRACK_RAY, true, track_fields(every_body(), hook)),
        &mut Range::new(None),
    );
    let values = outputs(&component, &[20, 24]);
    assert_eq!(values[0], Value::Bool(false));
    assert!(close(&values[1], [-6.0, 1.0, 0.0]), "{values:?}");
}

/// Zarya's beam (`0241` st43: `m_3B0AD973` true, radius 0.15, every body
/// but the dead) casts: its hit fills `m_750377AA`, and the host's hit
/// report it asks for is recorded as not made.
#[test]
fn a_track_ray_with_the_hit_report_casts_and_records_the_report() {
    let mut beam = every_body();
    beam["m_4EA686DB"] = json!(20);
    let extra = json!({
        "m_57DAE1B5":{"$":"STU_919FD47C","m_expression":{
            "m_opcodes":[6,0,0], "m_D99EF254":[0.15]}},
        "m_3B0AD973":{"$":"STUConfigVarBool","m_value":1},
    });
    let component = start(
        one_node(TRACK_RAY, true, track_fields(beam, extra)),
        &mut Range::new(None),
    );
    assert_eq!(
        outputs(&component, &[20, 21, 22]),
        [
            Value::Bool(true),
            Value::Entity(ALLY),
            Value::Array(vec![Value::Entity(ALLY)])
        ]
    );
    assert!(assumed(
        &component,
        "STUStatescriptStateTrackRay m_3B0AD973"
    ));
}

/// The capsule test: entering the side, a cap, and starting inside.
#[test]
fn a_ray_enters_a_capsule_at_its_surface() {
    let capsule = body(ENEMY, 1, [0.0; 3]);
    let (distance, normal) = capsule
        .ray_hit([-5.0, 1.0, 0.0], [1.0, 0.0, 0.0], 10.0, 0.0)
        .unwrap();
    assert!((distance - 4.6).abs() < 1e-9 && (normal[0] + 1.0).abs() < 1e-9);
    let (distance, normal) = capsule
        .ray_hit([0.0, 5.0, 0.0], [0.0, -1.0, 0.0], 10.0, 0.1)
        .unwrap();
    assert!((distance - 3.1).abs() < 1e-9, "{distance}");
    assert!((normal[1] - 1.0).abs() < 1e-9);
    assert_eq!(
        capsule.ray_hit([0.0, 1.0, 0.0], [1.0, 0.0, 0.0], 10.0, 0.0),
        None
    );
    assert_eq!(
        capsule.ray_hit([-5.0, 1.0, 0.0], [1.0, 0.0, 0.0], 4.0, 0.0),
        None
    );
}

/// The Grappling Hook's cast (`STU_4BBC5D09`) binds its start (#431, slot
/// 0) and hardpoint's point (#432, slot 1) to itself while it flies, as the
/// client's begin does; the owner frames carry the bindings, so the
/// client's own end can write the hit through them. A hit releases both
/// with the hit point; a miss leaves the start when the state ends.
#[test]
fn the_grappling_hooks_cast_binds_its_start_until_its_end() {
    let float = |value: f64| json!({"$":"STUConfigVarFloat","m_value":value});
    let fields = json!({
        "m_5ED803CF":var(431, false), "m_F2F8606D":var(432, false),
        "m_64801077":var(433, false), "m_D7A2A03B":var(434, false),
        "m_9E1A7C58":var(435, false), "m_1FFD7AC0":var(38, false),
        "m_86CF0002":var(8695, false), "m_EA134F86":var(1219, false),
        "m_58B4C4D7":var(1220, false), "m_9169D109":var(1221, false),
        "m_66B658A5":map_only(), "m_speed":float(40.0), "m_2AFC083F":float(20.0),
    });
    let bound = |component: &Component| {
        let snapshot = component.snapshot().unwrap();
        [431, 432].map(|id| snapshot.bindings.get(&(1, id)).cloned())
    };
    let binding = |slot| {
        Some(vec![StackBinding {
            instance: 1,
            raw_state: 0,
            priority: LINK_PRIORITY,
            slot,
        }])
    };
    let eye = [0.0, 1.5, 0.0];

    // A wall 4 m ahead: the hook, 40 m/s, updated at each window's last
    // millisecond, reaches it in frame 6 (111 ms: 4.44 m).
    let mut world = Range::new(Some(4.0));
    let mut component = start(one_node("STU_4BBC5D09", true, fields.clone()), &mut world);
    assert_eq!(bound(&component), [binding(0), binding(1)]);
    let values = outputs(&component, &[431, 432]);
    assert!(values.iter().all(|value| close(value, eye)), "{values:?}");
    for frame in 1..6 {
        component.advance_frame(frame, &mut world).unwrap();
        assert_eq!(bound(&component), [binding(0), binding(1)], "frame {frame}");
    }
    component.advance_frame(6, &mut world).unwrap();
    assert_eq!(bound(&component), [None, None]);
    let values = outputs(&component, &[431, 432, 435]);
    assert!(close(&values[0], [4.0, 1.5, 0.0]), "{values:?}");
    assert!(close(&values[1], [4.0, 1.5, 0.0]), "{values:?}");
    assert_eq!(values[2], Value::Bool(true));

    // No wall: the hook misses at its 20 m in frame 31 (511 ms: 20.44 m),
    // and the state's end leaves the start.
    let mut world = Range::new(None);
    let mut component = start(one_node("STU_4BBC5D09", true, fields), &mut world);
    for frame in 1..31 {
        component.advance_frame(frame, &mut world).unwrap();
    }
    assert_eq!(bound(&component), [binding(0), binding(1)]);
    component.advance_frame(31, &mut world).unwrap();
    assert_eq!(bound(&component), [None, None]);
    let values = outputs(&component, &[431, 432, 435]);
    assert!(
        values[..2].iter().all(|value| close(value, eye)),
        "{values:?}"
    );
    assert_eq!(values[2], Value::Bool(false));
}
