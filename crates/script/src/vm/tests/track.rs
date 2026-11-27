//! The payload's mover `STU_49FE0019` and `STU_4A4BEDAA` on a track
//! (`vm/track.rs`).

use super::*;
use crate::track::{Track, TrackPoint};

const ENTITY: u32 = 0xa000_0101;

/// A world whose entity moves on one track: where it was put, in order.
struct TrackWorld {
    track: Arc<Track>,
    placed: Vec<f64>,
}

impl World for TrackWorld {
    fn track(&self, _site: Site, entity: u32) -> Option<Arc<Track>> {
        (entity == ENTITY).then(|| Arc::clone(&self.track))
    }

    fn place_on_track(&mut self, _site: Site, entity: u32, distance: f64) -> bool {
        self.placed.push(distance);
        entity == ENTITY
    }
}

/// Straight along x: waypoints at 0, 4 (a barrier) and 10 m (objective 0's
/// checkpoint).
fn world() -> TrackWorld {
    let point = |entity: u32, at: f64, next: Option<f64>, barrier, objective| TrackPoint {
        entity,
        position: [at, 0.0, 0.0],
        curve: next.map(|next| [[0.0; 3], [0.0; 3], [next - at, 0.0, 0.0]]),
        length: next.map(|next| next - at),
        barrier,
        objective,
    };
    let track = Track::new(&[
        point(0x8000_0010, 0.0, Some(4.0), false, None),
        point(0x8000_0011, 4.0, Some(10.0), true, None),
        point(0x8000_0012, 10.0, None, true, Some(0)),
    ])
    .unwrap();
    TrackWorld {
        track: Arc::new(track),
        placed: Vec::new(),
    }
}

/// A mover at `speed` m/s from #479e: its waypoint plug copies #687 into
/// #900e, its finished plug sets #902e.
fn mover(speed: f64) -> Store {
    Arc::new(store(vec![graph(
        42,
        vec![
            entry(&[(1, "m_beginPlug")]),
            node(
                1,
                "STU_49FE0019",
                Some(0),
                json!({
                    "m_speed":{"$":"STUConfigVarFloat","m_value":speed},
                    "m_971B2969":var(479, true),
                    "m_A3D27CCA":var(499, true),
                    "m_F620FC2A":var(1622, true),
                    "m_330800D9":var(687, false),
                    "m_0ECFADFE":var(7582, true),
                    "m_625C2727":plug(&[(2, "m_inPlug")], &[]),
                    "m_onFinishedPlug":plug(&[(3, "m_inPlug")], &[]),
                }),
            ),
            node(
                2,
                "STUStatescriptActionSetVar",
                None,
                json!({"m_out_Var":var(900, true), "m_value":var(687, false)}),
            ),
            set_entity(3, 902, json!(true)),
        ],
        json!([1]),
        json!([0]),
    )]))
}

fn variable(component: &Component, id: u32) -> Value {
    component
        .snapshot()
        .unwrap()
        .variables
        .get(&id)
        .cloned()
        .unwrap_or(Value::Null)
}

/// Forward at 2 m/s from the start: it tells each waypoint it passes (the
/// one it starts on too), its distance and progress follow, the HUD's
/// markers are the checkpoints' shares of the track, and at the end it
/// finishes.
#[test]
fn the_mover_pushes_the_payload_along_its_track_to_the_end() {
    let mut world = world();
    let mut component = start(mover(2.0), 100, &mut world);
    let snapshot = component.snapshot().unwrap();
    assert!(
        snapshot.initialization_complete() && snapshot.diagnostics.is_empty(),
        "{:?}",
        snapshot.diagnostics
    );
    assert_eq!(
        variable(&component, 7582),
        Value::Array(vec![Value::Float(1.0)])
    );
    assert_eq!(variable(&component, 479), Value::Float(0.0));
    component.advance_frame(101, &mut world).unwrap();
    // The begin ran at frame 100's first millisecond, the tick at frame
    // 101's last: 2 m/s over that time, its share of the 10 m track.
    let moved = 2.0 * ((2 * FRAME_MS - 1) as f64 / 1000.0);
    assert_eq!(variable(&component, 479), Value::Float(moved));
    assert_eq!(variable(&component, 499), Value::Float(moved / 10.0));
    assert_eq!(variable(&component, 1622), Value::Bool(true));
    assert_eq!(variable(&component, 900), Value::Entity(0x8000_0010));
    let mut frame = 101;
    while variable(&component, 900) == Value::Entity(0x8000_0010) {
        frame += 1;
        component.advance_frame(frame, &mut world).unwrap();
    }
    // The barrier at 4 m, passed once the payload reached it.
    assert_eq!(variable(&component, 900), Value::Entity(0x8000_0011));
    assert!((expr::to_float(&variable(&component, 479)) - 4.0).abs() < moved);
    while variable(&component, 902) == Value::Null {
        frame += 1;
        component.advance_frame(frame, &mut world).unwrap();
        assert!(frame < 100 + 10 * 1000 / FRAME_MS as u32, "no end");
    }
    assert_eq!(variable(&component, 900), Value::Entity(0x8000_0012));
    assert_eq!(variable(&component, 479), Value::Float(10.0));
    assert_eq!(variable(&component, 499), Value::Float(1.0));
    assert_eq!(world.placed.last(), Some(&10.0));
    assert!(!component.snapshot().unwrap().instances[&1].states[&0].active);
}

/// Backward it rolls to the last barrier at or behind it and stops there,
/// not moving any more (#1622 false); it tells no waypoint.
#[test]
fn a_rollback_stops_at_the_last_barrier() {
    let mut world = world();
    let variables = Variables::from([(479, Value::Float(7.0))]);
    let mut component = start_with(mover(-1.0), variables, 100, &mut world);
    for frame in 101..=100 + 5 * 1000 / FRAME_MS as u32 {
        component.advance_frame(frame, &mut world).unwrap();
    }
    assert_eq!(variable(&component, 479), Value::Float(4.0));
    assert_eq!(variable(&component, 1622), Value::Bool(false));
    assert_eq!(variable(&component, 900), Value::Null);
    assert!(world.placed.iter().all(|&at| (4.0..=7.0).contains(&at)));
}
