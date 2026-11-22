use serde_json::{Value, json};

use super::*;

/// A volume of the given box with the listed cells walkable.
fn volume(
    origin: Cell,
    size: [i32; 3],
    cells: &[Cell],
    floor: Vec3,
    neighbours: &[usize],
) -> Value {
    let count = (size[0] * size[1] * size[2]) as usize;
    let mut walkable = vec![0u32; count.div_ceil(32)];
    for cell in cells {
        let [x, y, z] = [0, 1, 2].map(|axis| cell[axis] - origin[axis]);
        let bit = (z + size[2] * (x + size[0] * y)) as usize;
        walkable[bit / 32] |= 1 << (bit % 32);
    }
    json!({
        "floor": floor,
        "origin": origin,
        "size": size,
        "neighbours": neighbours,
        "walkable": walkable,
    })
}

/// The cells of a 4 × 4 tile on layer 0, but the `blocked` ones.
fn tile(x0: i32, z0: i32, blocked: &[[i32; 2]]) -> Vec<Cell> {
    (x0..x0 + 4)
        .flat_map(|x| (z0..z0 + 4).map(move |z| [x, 0, z]))
        .filter(|cell| !blocked.contains(&[cell[0], cell[2]]))
        .collect()
}

const FLOOR: f32 = 0.35;

/// Four 4 × 4 tiles around (4, 4) and a ramp far away:
///
/// ```text
/// z 8 +---+---+
///     | 3 | 2 |   2 has a 2 × 2 hole at x 5..7, z 5..7
///   4 +---+---+
///     | 0 | 1 |   links 0-1, 1-2, 2-3; 0 and 3 touch but aren't linked
///   0 +---+---+
///     0   4   8 x
/// ```
///
/// Volume 4, unlinked, is a ramp at x 20..24, z 0: layer 0 for x 20 and 21,
/// layer 1 for x 22 and 23.
fn plaza() -> Navigation {
    let hole = [[5, 5], [5, 6], [6, 5], [6, 6]];
    let file = json!({"volumes": [
        volume([0, 0, 0], [4, 1, 4], &tile(0, 0, &[]), [1.5, FLOOR, 1.5], &[1]),
        volume([4, 0, 0], [4, 1, 4], &tile(4, 0, &[]), [5.5, FLOOR, 1.5], &[0, 2]),
        volume([4, 0, 4], [4, 1, 4], &tile(4, 4, &hole), [7.5, FLOOR, 7.5], &[1, 3]),
        volume([0, 0, 4], [4, 1, 4], &tile(0, 4, &[]), [1.5, FLOOR, 5.5], &[2]),
        volume(
            [20, 0, 0],
            [4, 2, 1],
            &[[20, 0, 0], [21, 0, 0], [22, 1, 0], [23, 1, 0]],
            [20.5, FLOOR, 0.5],
            &[],
        ),
    ]});
    Navigation::from_json(&serde_json::to_vec(&file).unwrap()).unwrap()
}

fn close(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-3
}

#[test]
fn layers_round_floors_to_the_quarter_layer_grid() {
    // Floor points of Temple of Anubis volumes 0, 53 and 113: on the 0.35 m
    // grid, with float error above or below.
    assert_eq!(layer_of(1.753_267_5), 1);
    assert_eq!(layer_of(9.799_898), 7);
    assert_eq!(layer_of(11.199_312), 8);
    assert_eq!(layer_of(0.0), 0);
    assert_eq!(layer_of(-0.5), -1);
    assert_eq!(cell_of([-32.625, 1.753_267_5, 64.375]), [-33, 1, 64]);
}

#[test]
fn a_point_stands_in_its_cell_or_one_layer_above() {
    let nav = plaza();
    let spot = Spot {
        volume: 0,
        cell: [1, 0, 1],
    };
    assert_eq!(nav.locate([1.2, FLOOR, 1.7]), Some(spot));
    // A body about 0.8 m above the floor, and one in the next layer up.
    assert_eq!(nav.locate([1.2, FLOOR + 0.8, 1.7]), Some(spot));
    assert_eq!(nav.locate([1.2, 2.0, 1.7]), Some(spot));
    assert_eq!(nav.locate([1.2, 3.5, 1.7]), None);
    assert_eq!(nav.volume_at([6.5, FLOOR, 2.5]), Some(1));
    assert_eq!(nav.volume_at([5.5, FLOOR, 5.5]), None, "the hole");
    assert_eq!(nav.volume_at([22.5, 1.75, 0.5]), Some(4));
}

/// Floors are rounded up to the 0.35 m grid: feet on a floor at 1.2 stand
/// under the 1.4 (the bottom of layer 1) the volume gives, in layer 0.
#[test]
fn feet_under_a_rounded_up_floor_stand_in_its_cell() {
    let file =
        json!({"volumes": [volume([0, 1, 0], [1, 1, 1], &[[0, 1, 0]], [0.5, 1.4, 0.5], &[])]});
    let nav = Navigation::from_json(&serde_json::to_vec(&file).unwrap()).unwrap();
    let spot = Spot {
        volume: 0,
        cell: [0, 1, 0],
    };
    assert_eq!(nav.locate([0.5, 1.2, 0.5]), Some(spot));
    assert_eq!(nav.locate([0.5, 0.9, 0.5]), None);
}

#[test]
fn points_off_the_walkable_cells_snap_to_the_nearest() {
    let nav = plaza();
    assert_eq!(
        nav.nearest([5.7, FLOOR, 5.6], SNAP),
        Some(Spot {
            volume: 2,
            cell: [5, 0, 4],
        })
    );
    assert_eq!(nav.nearest([5.7, FLOOR, 5.6], 0.5), None);
}

#[test]
fn routes_follow_links_not_touching_cells() {
    let nav = plaza();
    let route = nav.route(0, 3).unwrap();
    assert_eq!(route.volumes, [0, 1, 2, 3]);
    // Floor to border centre to floor: the borders' centres are (4, 2),
    // (6, 4) and (4, 6).
    let expected = 3.0 * 6.5f32.sqrt() + 2.5f32.sqrt() + 2.0 * 14.5f32.sqrt();
    assert!(close(route.length, expected), "{}", route.length);
    assert_eq!(
        nav.route(2, 2),
        Some(Route {
            volumes: vec![2],
            length: 0.0,
        })
    );
    assert_eq!(nav.route(0, 4), None);
    assert_eq!(nav.component(3), 0);
    assert_eq!(nav.component(4), 4);
}

/// Every point of a walk, sampled every 2 cm, is over a walkable cell
/// within a layer of its height (waypoint heights are floor estimates; a
/// line between them can run a little under a floor of the next layer), and
/// consecutive samples are in the same or linked volumes.
fn assert_walkable(nav: &Navigation, walk: &Walk) {
    let mut last: Option<usize> = None;
    for pair in walk.waypoints.windows(2) {
        let steps = (distance(pair[0], pair[1]) / 0.02).ceil().max(1.0) as usize;
        for step in 0..=steps {
            let t = step as f32 / steps as f32;
            let point = [0, 1, 2].map(|axis| pair[0][axis] + (pair[1][axis] - pair[0][axis]) * t);
            let [x, layer, z] = cell_of(point);
            let under: Vec<usize> = (0..nav.volumes().len())
                .filter(|&volume| {
                    (layer - 1..=layer + 1)
                        .any(|layer| nav.volumes()[volume].walkable([x, layer, z]))
                })
                .collect();
            let volume = match last {
                None => under.first().copied(),
                Some(last) => under
                    .iter()
                    .copied()
                    .find(|&volume| volume == last)
                    .or_else(|| {
                        under
                            .iter()
                            .copied()
                            .find(|volume| nav.volumes()[last].neighbours.contains(volume))
                    }),
            };
            let Some(volume) = volume else {
                panic!(
                    "{point:?} (after volume {last:?}, over {under:?}) of {:?}",
                    walk.waypoints
                );
            };
            last = Some(volume);
        }
    }
}

#[test]
fn paths_go_around_unlinked_borders_and_holes() {
    let nav = plaza();
    let walk = nav.path([1.5, FLOOR, 1.5], [1.5, FLOOR, 6.5]).unwrap();
    assert_eq!(walk.route.volumes, [0, 1, 2, 3]);
    assert_eq!(walk.waypoints.first(), Some(&[1.5, FLOOR, 1.5]));
    assert_eq!(walk.waypoints.last(), Some(&[1.5, FLOOR, 6.5]));
    assert!(walk.waypoints.len() > 2, "{:?}", walk.waypoints);
    assert!(walk.length() > 5.0);
    assert_walkable(&nav, &walk);

    // Across the hole's tile: around the hole.
    let walk = nav.path([4.5, FLOOR, 4.5], [7.5, FLOOR, 7.5]).unwrap();
    assert_eq!(walk.route.volumes, [2]);
    assert_walkable(&nav, &walk);

    // In the open: a straight line.
    let walk = nav.path([0.5, FLOOR, 0.5], [7.5, FLOOR, 3.5]).unwrap();
    assert_eq!(walk.waypoints, [[0.5, FLOOR, 0.5], [7.5, FLOOR, 3.5]]);
}

#[test]
fn paths_climb_layers_and_stop_at_unlinked_volumes() {
    let nav = plaza();
    let walk = nav.path([20.5, FLOOR, 0.5], [23.5, 1.75, 0.5]).unwrap();
    assert_eq!(
        walk.waypoints,
        [[20.5, FLOOR, 0.5], [23.5, FLOOR + LAYER, 0.5]]
    );
    assert_eq!(nav.path([1.5, FLOOR, 1.5], [20.5, FLOOR, 0.5]), None);
    // A goal off the map's cells snaps to its nearest cell's centre.
    let walk = nav.path([0.5, FLOOR, 0.5], [5.6, FLOOR, 5.7]).unwrap();
    assert_eq!(walk.waypoints.last(), Some(&[4.5, FLOOR, 5.5]));
    assert_eq!(nav.path([0.5, FLOOR, 0.5], [50.0, FLOOR, 50.0]), None);
}

/// An L of corridors three cells wide (x 0..3 up z 0..10, then z 7..10 out
/// to x 10): the walk round the inner corner (3, 7) keeps a cell from it,
/// along the middle of each corridor; without the edge cost it would turn
/// at the corner's cell (2.5, 6.5), its capsule on the wall.
#[test]
fn walks_turn_a_cell_away_from_walls() {
    let cells: Vec<Cell> = (0..10)
        .flat_map(|x| (0..10).map(move |z| [x, 0, z]))
        .filter(|&[x, _, z]| x < 3 || z >= 7)
        .collect();
    let file = json!({"volumes": [volume([0, 0, 0], [10, 1, 10], &cells, [1.5, FLOOR, 0.5], &[])]});
    let nav = Navigation::from_json(&serde_json::to_vec(&file).unwrap()).unwrap();
    let walk = nav.path([1.5, FLOOR, 0.5], [9.5, FLOOR, 8.5]).unwrap();
    assert_walkable(&nav, &walk);
    let turns = &walk.waypoints[1..walk.waypoints.len() - 1];
    assert!(!turns.is_empty());
    for turn in turns {
        let gap = (turn[0] - 3.0).hypot(turn[2] - 7.0);
        assert!(gap >= 1.0, "{turn:?} in {:?}", walk.waypoints);
    }
}

/// The caller's test decides the lines and the steps: a line it refuses
/// keeps the cells' turns, a step between neighbouring cells it refuses (a
/// railing the cells don't show) is walked round, a step to another layer
/// it refuses is no way.
#[test]
fn paths_keep_only_lines_and_layer_steps_the_caller_walks() {
    let nav = plaza();
    let short = |a: Vec3, b: Vec3| distance(a, b) <= 3.0;
    let walk = nav
        .path_with([0.5, FLOOR, 0.5], [7.5, FLOOR, 3.5], short)
        .unwrap();
    assert!(walk.waypoints.len() > 2, "{:?}", walk.waypoints);
    for pair in walk.waypoints.windows(2) {
        assert!(distance(pair[0], pair[1]) <= 3.0, "{:?}", walk.waypoints);
    }
    assert_walkable(&nav, &walk);
    // A railing along x = 2 from z = 0 to 2 in tile 0.
    let railing = |a: Vec3, b: Vec3| {
        let crosses = (a[0] - 2.0) * (b[0] - 2.0) < 0.0;
        let z = a[2] + (b[2] - a[2]) * (2.0 - a[0]) / (b[0] - a[0]);
        !(crosses && z < 2.0)
    };
    let walk = nav
        .path_with([0.5, FLOOR, 0.5], [3.5, FLOOR, 0.5], railing)
        .unwrap();
    for pair in walk.waypoints.windows(2) {
        assert!(railing(pair[0], pair[1]), "{:?}", walk.waypoints);
    }
    assert_walkable(&nav, &walk);
    let flat = |a: Vec3, b: Vec3| a[1] == b[1];
    assert_eq!(
        nav.path_with([20.5, FLOOR, 0.5], [23.5, 1.75, 0.5], flat),
        None
    );
}

/// A search run a little work at a time (a server's ticks) finds the walk
/// the whole search finds, asking the caller the same tests in the same
/// order; it does nothing without work, and with the work of one walk test
/// asks at most one at a time. The railing (as in
/// `paths_keep_only_lines_and_layer_steps_the_caller_walks`) refuses some.
#[test]
fn a_search_split_over_ticks_finds_the_walk_of_the_whole_search() {
    let nav = plaza();
    let railing = |a: Vec3, b: Vec3| {
        let crosses = (a[0] - 2.0) * (b[0] - 2.0) < 0.0;
        let z = a[2] + (b[2] - a[2]) * (2.0 - a[0]) / (b[0] - a[0]);
        !(crosses && z < 2.0)
    };
    let (from, to) = ([0.5, FLOOR, 0.5], [7.5, FLOOR, 7.5]);
    let mut whole_asked = Vec::new();
    let whole = nav.path_with(from, to, |a, b| {
        whole_asked.push((a, b));
        railing(a, b)
    });
    assert!(whole.is_some());
    assert!(whole_asked.iter().any(|&(a, b)| !railing(a, b)));

    let mut search = PathSearch::new(from, to);
    let mut none = 0;
    assert_eq!(search.advance(&nav, &mut none, |_, _| unreachable!()), None);
    let mut asked = Vec::new();
    let mut ticks = 0;
    let split = loop {
        ticks += 1;
        let before = asked.len();
        let mut work = CHECK_WORK;
        let done = search.advance(&nav, &mut work, |a, b| {
            asked.push((a, b));
            railing(a, b)
        });
        assert!(asked.len() - before <= 1, "tick {ticks}");
        if let Some(walk) = done {
            break walk;
        }
    };
    assert_eq!(split, whole);
    assert_eq!(asked, whole_asked);
    assert!(ticks > whole_asked.len(), "{ticks} ticks");
}

#[test]
fn free_cells_spread_over_interior_cells() {
    let nav = plaza();
    // Volume 1's interior cells are its middle four.
    let cells = nav.free_cells([5.9, FLOOR, 1.9], 3.0, 3);
    assert_eq!(cells.len(), 3);
    assert_eq!(cells[0], [5.5, FLOOR, 1.5], "the nearest first");
    assert_eq!(cells[1], [6.5, FLOOR, 2.5], "then the farthest");
    assert!([[5.5, FLOOR, 2.5], [6.5, FLOOR, 1.5]].contains(&cells[2]));
    assert_eq!(nav.free_cells([5.9, FLOOR, 1.9], 3.0, 10).len(), 4);
    // The ramp's cells have no interior ones; any walkable cell does.
    assert_eq!(nav.free_cells([20.5, FLOOR, 0.5], 1.5, 4).len(), 2);
    assert!(nav.free_cells([50.0, FLOOR, 50.0], 3.0, 3).is_empty());
}

#[test]
fn bad_files_are_refused() {
    let file = |volume: Value| serde_json::to_vec(&json!({"volumes": [volume]})).unwrap();
    let short = json!({"floor": [0.0, 0.0, 0.0], "origin": [0, 0, 0], "size": [8, 1, 8],
        "neighbours": [], "walkable": [0]});
    assert!(Navigation::from_json(&file(short)).is_err());
    let self_linked = volume([0, 0, 0], [1, 1, 1], &[[0, 0, 0]], [0.5, 0.0, 0.5], &[0]);
    assert!(Navigation::from_json(&file(self_linked)).is_err());
}

const ANUBIS: u64 = 0x0800_0000_0000_005B;
const PRACTICE_RANGE: u64 = 0x0800_0000_0000_0688;

fn derived(map: u64) -> Navigation {
    Navigation::load(file(&lucy_testkit::runtime_dir(), map)).unwrap()
}

/// The counts in `docs/notes/ow1-preservation.md` ("Navigation data").
#[test]
#[ignore = "requires derived 104319 navigation"]
fn anubis_and_practice_range_graphs_match_the_notes() {
    let anubis = derived(ANUBIS);
    assert_eq!(anubis.volumes().len(), 117);
    let edges: usize = anubis
        .volumes()
        .iter()
        .enumerate()
        .map(|(index, volume)| {
            volume
                .neighbours
                .iter()
                .filter(|&&other| other > index)
                .count()
        })
        .sum();
    assert_eq!(edges, 113);
    let mut sizes = HashMap::<usize, usize>::new();
    for index in 0..anubis.volumes().len() {
        *sizes.entry(anubis.component(index)).or_default() += 1;
    }
    assert_eq!(sizes.values().max(), Some(&85));
    for nav in [&anubis, &derived(PRACTICE_RANGE)] {
        for (index, volume) in nav.volumes().iter().enumerate() {
            assert!(volume.walkable(cell_of(volume.floor)), "volume {index}");
            // Within 1.5 cm of the 0.35 m grid (Practice Range volume 65:
            // 43.385838 for 124 × 0.35).
            let y = volume.floor[1];
            let off = y - (y / FLOOR_STEP).round() * FLOOR_STEP;
            assert!(off.abs() < 0.015, "volume {index}: {y}");
        }
    }
    assert_eq!(derived(PRACTICE_RANGE).volumes().len(), 67);
}

/// From the attacker spawn (the team with Point A's attack tag) to the
/// points; the notes give about 125 m to A and 206 m to B.
#[test]
#[ignore = "requires derived 104319 navigation and maps"]
fn anubis_attackers_walk_to_both_points() {
    let runtime = lucy_testkit::runtime_dir();
    let maps: Value =
        serde_json::from_slice(&std::fs::read(runtime.join("data/game_maps_174.json")).unwrap())
            .unwrap();
    let anubis = &maps["0x080000000000005B"];
    let position =
        |value: &Value| -> Vec3 { serde_json::from_value(value["position"].clone()).unwrap() };
    let objectives = anubis["objectives"].as_array().unwrap();
    let attack = objectives[0]["attack_tag"].as_str().unwrap();
    let spawn = anubis["spawns"]
        .as_array()
        .unwrap()
        .iter()
        .find(|spawn| {
            spawn["tags"]
                .as_array()
                .unwrap()
                .iter()
                .any(|tag| tag == attack)
        })
        .map(position)
        .unwrap();
    let nav = derived(ANUBIS);
    for (objective, about) in [(&objectives[0], 125.0), (&objectives[1], 206.0)] {
        let walk = nav.path(spawn, position(objective)).unwrap();
        let length = walk.route.length;
        assert!(
            (length - about).abs() < about * 0.05,
            "route {length} m, about {about}"
        );
        // The waypoints cut corners the route's floor points don't.
        assert!(walk.length() < length + 10.0, "walk {} m", walk.length());
        assert!(walk.length() > distance(spawn, position(objective)));
        assert_walkable(&nav, &walk);
    }
    let around_a = nav.free_cells(position(&objectives[0]), 6.0, 6);
    assert_eq!(around_a.len(), 6);
    for cell in &around_a {
        assert!(distance(*cell, position(&objectives[0])) <= 6.0);
        assert_eq!(
            nav.component(nav.volume_at(*cell).unwrap()),
            nav.component(nav.volume_at(spawn).unwrap())
        );
    }
}
