use super::*;

const RANGE: u64 = 0x0800_0000_0000_0688;
const RIALTO: u64 = 0x0800_0000_0000_0871;

/// What the client's own code gives for these points, run on the dumped
/// image (`dumps/ingame_7FF788F10000.exe`) in an emulator over the map's
/// chunk 0x12 document 0 blocks (2026-11-19, DayAiCells): the voxel from
/// `0x7FF78A619180` (the grid's floats as `0x7FF78A614BD0` sets them for
/// {16.0, 22.4}), then `0x7FF789AECFF0` (walk) or `0x7FF78A5FB310` (air) on
/// the cell: its node (fourth argument), the entry it points to (fifth, as
/// an index into the entries) and the brick node (seventh, as an index into
/// the brick lists). The points are random voxels of each graph and of
/// neither, plus the Practice Range's floor at (59.9, 1.0, 42.2) (its walk
/// voxel 3; the air starts a voxel higher) and a point under the ceiling the
/// grappling hook hits at y 11.26 (Grapple.md).
#[allow(clippy::type_complexity)]
const CLIENT: [(u64, Graph, Vec3, [i32; 3], Option<(u64, u32, u32)>); 23] = [
    (
        RANGE,
        Graph::Walk,
        [0.125, 31.656816, -9.125],
        [131072, 8282, 131035],
        Some((0x817FF80000001983, 8, 1)),
    ),
    (
        RANGE,
        Graph::Walk,
        [-2.125, 32.006668, -25.625],
        [131063, 8283, 130969],
        Some((0x817FE7FF000019B7, 73, 13)),
    ),
    (
        RANGE,
        Graph::Walk,
        [88.375, 41.452713, 18.875],
        [131425, 8310, 131147],
        Some((0x81801805000034A3, 180, 38)),
    ),
    (
        RANGE,
        Graph::Walk,
        [36.375, -19.421797, -12.375],
        [131217, 8136, 131022],
        Some((0x7F7FF802000208D3, 54, 42)),
    ),
    (
        RANGE,
        Graph::Walk,
        [84.875, 14.863848, 54.875],
        [131411, 8234, 131291],
        None,
    ),
    (
        RANGE,
        Graph::Walk,
        [59.9, 1.2, 42.2],
        [131311, 8195, 131240],
        Some((0x80802803000002AF, 108, 84)),
    ),
    (
        RANGE,
        Graph::Air,
        [-10.125, 12.764727, 1.625],
        [131031, 8228, 131078],
        Some((0x808007FF00002457, 47, 18)),
    ),
    (
        RANGE,
        Graph::Air,
        [58.625, -3.328535, -59.375],
        [131306, 8182, 130834],
        Some((0x7F7FC8030000352B, 1172, 39)),
    ),
    (
        RANGE,
        Graph::Air,
        [66.375, 23.96004, 19.375],
        [131337, 8260, 131149],
        Some((0x81801804000004CB, 30, 64)),
    ),
    (
        RANGE,
        Graph::Air,
        [30.625, -7.876631, -10.125],
        [131194, 8169, 131031],
        Some((0x7F7FF8010002297B, 1774, 196)),
    ),
    (
        RANGE,
        Graph::Air,
        [109.375, -11.725019, -62.625],
        [131509, 8158, 130821],
        None,
    ),
    (
        RANGE,
        Graph::Air,
        [54.5654, 11.0, 43.6923],
        [131290, 8223, 131246],
        None,
    ),
    (
        RANGE,
        Graph::Air,
        [59.9, 1.2, 42.2],
        [131311, 8195, 131240],
        None,
    ),
    (
        RIALTO,
        Graph::Walk,
        [-14.375, -0.87956053, -34.375],
        [131014, 8189, 130934],
        Some((0x7F7FD7FF00003F47, 47, 3)),
    ),
    (
        RIALTO,
        Graph::Walk,
        [-57.625, 4.0183887, -180.875],
        [130841, 8203, 130348],
        Some((0x807F47FC00000ADB, 18, 4)),
    ),
    (
        RIALTO,
        Graph::Walk,
        [-20.125, -1.2294141, -47.875],
        [130991, 8188, 130880],
        Some((0x7F7FD7FE00003C2F, 139, 8)),
    ),
    (
        RIALTO,
        Graph::Walk,
        [-9.875, 0.17, -52.375],
        [131032, 8192, 130862],
        Some((0x807FC7FF000202DB, 83, 40)),
    ),
    (
        RIALTO,
        Graph::Walk,
        [110.375, -19.77165, 5.375],
        [131513, 8135, 131093],
        None,
    ),
    (
        RIALTO,
        Graph::Air,
        [-58.125, 42.502274, -143.625],
        [130839, 8313, 130497],
        Some((0x817F77FC00003817, 1834, 19)),
    ),
    (
        RIALTO,
        Graph::Air,
        [-46.625, 16.263262, -152.125],
        [130885, 8238, 130463],
        Some((0x807F67FD00002DC7, 1349, 6)),
    ),
    (
        RIALTO,
        Graph::Air,
        [-34.625, 15.563555, -169.625],
        [130933, 8236, 130393],
        Some((0x807F57FD00002DB7, 276, 54)),
    ),
    (
        RIALTO,
        Graph::Air,
        [-69.375, 5.7676563, -138.875],
        [130794, 8208, 130516],
        Some((0x807F77FB0002116B, 962, 83)),
    ),
    (
        RIALTO,
        Graph::Air,
        [-80.125, 13.11458, -92.375],
        [130751, 8229, 130702],
        None,
    ),
];

#[test]
#[ignore = "requires derived 104319 AI cells"]
fn lookups_find_what_the_clients_reader_finds() {
    let runtime = orpheus_testkit::runtime_dir();
    let range = AiCells::load(file(&runtime, RANGE)).unwrap();
    let rialto = AiCells::load(file(&runtime, RIALTO)).unwrap();
    assert_eq!((range.len(), rialto.len()), (208, 282));
    for (map, graph, point, [x, y, z], client) in CLIENT {
        let cells = if map == RANGE { &range } else { &rialto };
        let voxel = Voxel::of(point);
        assert_eq!(voxel, Voxel { x, y, z }, "{point:?}");
        let found = cells
            .find(graph, voxel)
            .map(|hit| (hit.node(voxel), hit.entry, hit.brick));
        assert_eq!(found, client, "{graph:?} {point:?}");
    }
}

#[test]
fn bad_files_are_refused() {
    let mut file = MAGIC.to_vec();
    file.extend_from_slice(&1u32.to_le_bytes());
    file.extend_from_slice(&(HEADER as u32).to_le_bytes());
    let mut block = vec![0u8; HEADER];
    // Section 1 before section 0.
    block[..8].copy_from_slice(&(HEADER as u64 + 8).to_le_bytes());
    file.extend_from_slice(&block);
    file.extend_from_slice(&0u32.to_le_bytes());
    assert!(AiCells::from_bytes(&file).is_err());
    assert!(AiCells::from_bytes(b"ORPHAIC0").is_err());
}

/// The walk-graph neighbours the client's own neighbour walk gives for
/// these nodes (2026-11-19, DayAiCells): `0x7FF78A612A10` then
/// `0x7FF78A612590` until it returns 0, set up as `0x7FF78A6263A0` sets them
/// (all directions), run in the emulator over the dumped image with the
/// grid's cell lookup answered from the map's blocks; each neighbour's unit
/// and place from the node it gives. Six nodes have outgoing links (three
/// lead into other cells, two to entries other than a unit's first), two
/// only implicit ones.
#[allow(clippy::type_complexity)]
const CLIENT_NEIGHBOURS: [(u64, Vec3, &[([i32; 3], u32)]); 8] = [
    (
        RANGE,
        [96.375, 6.11751, 44.375],
        &[
            ([32863, 2052, 32811], 0),
            ([32863, 2052, 32812], 0),
            ([32863, 2052, 32813], 0),
            ([32864, 2052, 32811], 8),
            ([32864, 2052, 32813], 0),
        ],
    ),
    (
        RANGE,
        [8.625, 31.656816, -0.625],
        &[
            ([32775, 2070, 32766], 0),
            ([32775, 2070, 32767], 0),
            ([32775, 2070, 32768], 0),
            ([32776, 2070, 32766], 0),
            ([32776, 2070, 32768], 0),
            ([32777, 2070, 32766], 0),
            ([32777, 2070, 32767], 0),
            ([32777, 2070, 32768], 0),
        ],
    ),
    (
        RANGE,
        [79.625, 38.653885, 39.625],
        &[
            ([32846, 2075, 32806], 8),
            ([32846, 2075, 32807], 8),
            ([32846, 2075, 32808], 0),
            ([32847, 2075, 32806], 8),
            ([32847, 2075, 32808], 0),
            ([32848, 2075, 32807], 0),
            ([32848, 2075, 32808], 0),
        ],
    ),
    (
        RANGE,
        [40.625, 35.15535, 10.375],
        &[
            ([32808, 2073, 32777], 0),
            ([32809, 2073, 32777], 0),
            ([32809, 2073, 32778], 0),
            ([32809, 2073, 32779], 0),
        ],
    ),
    (
        RIALTO,
        [39.375, 6.11751, -48.125],
        &[
            ([32806, 2052, 32718], 0),
            ([32806, 2052, 32719], 0),
            ([32806, 2052, 32720], 0),
            ([32807, 2052, 32718], 0),
            ([32807, 2052, 32720], 0),
            ([32808, 2052, 32718], 0),
            ([32808, 2052, 32719], 0),
            ([32808, 2052, 32720], 0),
        ],
    ),
    (
        RIALTO,
        [-64.875, 4.0183887, -175.625],
        &[
            ([32702, 2050, 32592], 24),
            ([32702, 2050, 32593], 24),
            ([32703, 2050, 32591], 16),
            ([32703, 2050, 32593], 24),
            ([32704, 2050, 32591], 0),
            ([32704, 2050, 32592], 0),
            ([32704, 2050, 32593], 0),
        ],
    ),
    (
        RIALTO,
        [128.375, 26.409014, -4.125],
        &[
            ([32896, 2066, 32762], 0),
            ([32896, 2066, 32764], 0),
            ([32897, 2066, 32762], 0),
            ([32897, 2066, 32763], 0),
            ([32897, 2066, 32764], 0),
        ],
    ),
    (
        RIALTO,
        [-37.625, 5.0679493, -78.625],
        &[
            ([32729, 2051, 32688], 0),
            ([32729, 2051, 32689], 0),
            ([32729, 2051, 32690], 0),
            ([32730, 2051, 32688], 0),
            ([32730, 2051, 32690], 0),
            ([32731, 2051, 32688], 0),
            ([32731, 2051, 32689], 0),
            ([32731, 2051, 32690], 0),
        ],
    ),
];

#[test]
#[ignore = "requires derived 104319 AI cells"]
fn walk_links_lead_where_the_clients_neighbour_walk_does() {
    let runtime = orpheus_testkit::runtime_dir();
    let range = AiCells::load(file(&runtime, RANGE)).unwrap();
    let rialto = AiCells::load(file(&runtime, RIALTO)).unwrap();
    let mut next = Vec::new();
    for (map, point, client) in CLIENT_NEIGHBOURS {
        let cells = if map == RANGE { &range } else { &rialto };
        let node = cells.node(Graph::Walk, Voxel::of(point)).unwrap();
        cells.neighbours(Graph::Walk, node, &mut next);
        let mut found: Vec<([i32; 3], u32)> =
            next.iter().map(|node| (node.unit, node.place)).collect();
        found.sort();
        assert_eq!(found, client, "{point:?}");
    }
}

/// A walk searched a little work at a time is the walk of the whole search:
/// Rialto from the plaza at (−9.9, 0, −52.4) to the far end of the bridge
/// at (66.9, 0.2, −53.4), with a walk test that refuses every line longer
/// than 10 m.
#[test]
#[ignore = "requires derived 104319 AI cells"]
fn a_walk_split_over_ticks_is_the_walk_of_the_whole_search() {
    let cells = AiCells::load(file(&orpheus_testkit::runtime_dir(), RIALTO)).unwrap();
    let (from, to) = ([-9.875, 0.17, -52.375], [66.875, 0.17, -53.375]);
    let short = |a: Vec3, b: Vec3| (a[0] - b[0]).hypot(a[2] - b[2]) <= 10.0;
    let mut whole = WalkSearch::new(from, to);
    let mut work = u32::MAX;
    let walk = whole.advance(&cells, &mut work, short).unwrap().unwrap();
    assert!(walk.len() > 2, "{walk:?}");
    assert!(
        walk.windows(2).all(|pair| short(pair[0], pair[1])),
        "{walk:?}"
    );
    let mut split = WalkSearch::new(from, to);
    let mut calls = 0;
    let parts = loop {
        calls += 1;
        let mut work = 50;
        if let Some(walk) = split.advance(&cells, &mut work, short) {
            break walk.unwrap();
        }
    };
    assert!(calls > 10, "{calls} calls");
    assert_eq!(parts, walk);
}

/// The client's path query (`0x7FF78A626BC0`) run in the emulator after its
/// start (2026-11-19, DayAiCells): the query object as `0x7FF789B51810(q,
/// 2, 3, 2)` and the start function leave it (search 1, the window's middle
/// entry holding the start; the start function itself skipped), mode 0 over
/// the walk graph and mode 1 over the air, the grid's hash map holding the
/// map's cells; each step `{voxel, node, direction}` of its path. Random
/// start voxels and goals in the window.
#[allow(clippy::type_complexity)]
const CLIENT_PATHS: [(u64, Graph, [i32; 3], [i32; 3], Option<&[(u8, u64, u8)]>); 16] = [
    (
        RANGE,
        Graph::Walk,
        [131148, 8142, 131214],
        [131151, 8143, 131215],
        Some(&[
            (10, 0x7F80280100000CCF, 26),
            (31, 0x7F80280100000CCF, 6),
            (47, 0x7F80280100000CCF, 2),
            (63, 0x7F80280100000CCF, 2),
        ]),
    ),
    (
        RANGE,
        Graph::Walk,
        [131576, 8282, 130864],
        [131573, 8282, 130868],
        Some(&[
            (2, 0x817FC80700001B3B, 26),
            (54, 0x817FC80700001B37, 13),
            (42, 0x817FC80700001B37, 13),
            (30, 0x817FC80700001B37, 13),
            (18, 0x817FC80700001B77, 0),
        ]),
    ),
    (
        RANGE,
        Graph::Walk,
        [131058, 8282, 131147],
        [131050, 8282, 131155],
        Some(&[
            (46, 0x818017FF000018B3, 26),
            (18, 0x818017FF000018F3, 13),
            (6, 0x818017FF000018F3, 13),
            (58, 0x818017FF000018EF, 13),
            (46, 0x818017FF000018EF, 13),
            (18, 0x818017FF0000192F, 13),
            (6, 0x818017FF0000192F, 13),
            (58, 0x818017FF0000192B, 13),
            (46, 0x818017FF0000192B, 13),
        ]),
    ),
    (
        RANGE,
        Graph::Walk,
        [131576, 8139, 131031],
        [131583, 8136, 131030],
        Some(&[
            (15, 0x7F7FF8070000097B, 26),
            (26, 0x7F7FF8070000097B, 12),
            (42, 0x7F7FF8070000097B, 2),
            (58, 0x7F7FF8070000097B, 2),
            (9, 0x7F7FF8070000097F, 11),
            (25, 0x7F7FF8070000097F, 2),
            (40, 0x7F7FF8070000097F, 11),
            (56, 0x7F7FF8070000097F, 2),
        ]),
    ),
    (
        RANGE,
        Graph::Air,
        [131185, 8286, 130894],
        [131193, 8291, 130898],
        Some(&[
            (26, 0x817FD80100021CF3, 26),
            (47, 0x817FD80100021CF3, 6),
            (48, 0x817FD80100002133, 6),
            (5, 0x817FD80100002137, 6),
            (26, 0x817FD80100002137, 6),
            (43, 0x817FD80100002137, 7),
            (59, 0x817FD80100002137, 2),
            (11, 0x817FD8010000213B, 2),
            (27, 0x817FD8010000213B, 2),
        ]),
    ),
    (
        RANGE,
        Graph::Air,
        [131275, 8188, 131017],
        [131276, 8189, 131018],
        Some(&[(52, 0x7F7FF80300043C8B, 26), (9, 0x7F7FF80300043C8F, 6)]),
    ),
    (
        RANGE,
        Graph::Air,
        [131322, 8265, 131045],
        [131316, 8268, 131047],
        Some(&[
            (37, 0x817FF80300000A7B, 26),
            (26, 0x817FF80300000A7B, 17),
            (15, 0x817FF80300000A7B, 17),
            (60, 0x817FF80300000E77, 18),
            (44, 0x817FF80300000E77, 14),
            (28, 0x817FF80300000E77, 14),
            (12, 0x817FF80300000E77, 14),
        ]),
    ),
    (
        RANGE,
        Graph::Air,
        [131143, 8301, 130818],
        [131139, 8296, 130817],
        Some(&[
            (57, 0x817FC80100002C07, 26),
            (36, 0x817FC80100002C07, 24),
            (23, 0x817FC80100002807, 23),
            (6, 0x817FC80100002807, 23),
            (53, 0x817FC80100002803, 23),
            (52, 0x817FC80100002803, 21),
        ]),
    ),
    (
        RIALTO,
        Graph::Walk,
        [130838, 8206, 130479],
        [130841, 8206, 130470],
        Some(&[
            (46, 0x807F67FC00000ED7, 26),
            (26, 0x807F67FC00000ED7, 15),
            (6, 0x807F67FC00000ED7, 15),
            (2, 0x807F67FC00000ED7, 16),
            (62, 0x807F67FC00000E93, 15),
            (58, 0x807F67FC00000E93, 16),
            (54, 0x807F67FC00000E93, 16),
            (2, 0x807F67FC00000E97, 3),
            (30, 0x807F67FC00000E57, 3),
            (42, 0x807F67FC00000E57, 3),
            (58, 0x807F67FC00000E57, 2),
            (10, 0x807F67FC00000E5B, 2),
            (26, 0x807F67FC00000E5B, 2),
        ]),
    ),
    (
        RIALTO,
        Graph::Walk,
        [131342, 8190, 130927],
        [131343, 8189, 130934],
        Some(&[
            (46, 0x7F7FD80400003ECF, 26),
            (50, 0x7F7FD80400023F0F, 1),
            (54, 0x7F7FD80400023F0F, 0),
            (58, 0x7F7FD80400023F0F, 0),
            (62, 0x7F7FD80400023F0F, 0),
            (50, 0x7F7FD80400023F4F, 0),
            (54, 0x7F7FD80400023F4F, 0),
            (57, 0x7F7FD80400023F4F, 9),
        ]),
    ),
    (
        RIALTO,
        Graph::Walk,
        [131363, 8192, 130783],
        [131362, 8192, 130785],
        Some(&[
            (60, 0x807FB804000001E3, 26),
            (32, 0x807FB80400000223, 13),
            (36, 0x807FB80400000223, 0),
        ]),
    ),
    (
        RIALTO,
        Graph::Walk,
        [131237, 8192, 130880],
        [131239, 8192, 130873],
        Some(&[
            (16, 0x807FD80200000027, 26),
            (44, 0x807FC802000003E7, 3),
            (56, 0x807FC802000003E7, 3),
            (52, 0x807FC802000003E7, 16),
            (48, 0x807FC802000003E7, 16),
            (60, 0x807FC802000003A7, 16),
            (56, 0x807FC802000003A7, 16),
            (52, 0x807FC802000003A7, 16),
        ]),
    ),
    (
        RIALTO,
        Graph::Air,
        [130815, 8235, 130808],
        [130819, 8237, 130811],
        Some(&[
            (51, 0x807FB7FB00002BBF, 26),
            (4, 0x807FB7FC00002F83, 6),
            (25, 0x807FB7FC00002F83, 6),
            (45, 0x807FB7FC00002F83, 1),
            (61, 0x807FB7FC00002F83, 2),
        ]),
    ),
    (
        RIALTO,
        Graph::Air,
        [130997, 8240, 130813],
        [131003, 8246, 130821],
        Some(&[
            (20, 0x807FB7FE000033F7, 26),
            (41, 0x807FB7FE000033F7, 6),
            (62, 0x807FB7FE000033F7, 6),
            (3, 0x807FC7FE0000303B, 6),
            (20, 0x807FC7FE0000343B, 6),
            (41, 0x807FC7FE0000343B, 6),
            (62, 0x807FC7FE0000343B, 6),
            (50, 0x807FC7FE0000347B, 0),
            (54, 0x807FC7FE0000347B, 0),
        ]),
    ),
    (
        RIALTO,
        Graph::Air,
        [131242, 8302, 130925],
        [131248, 8298, 130934],
        Some(&[
            (38, 0x817FD80200002EEB, 26),
            (57, 0x817FD80200002EEB, 10),
            (12, 0x817FD80200002EEF, 10),
            (19, 0x817FD80200002B2F, 10),
            (38, 0x817FD80200002B2F, 10),
            (58, 0x817FD80200002B2F, 1),
            (14, 0x817FD80200002B33, 1),
            (2, 0x817FD80200002B73, 0),
            (6, 0x817FD80200002B73, 0),
            (10, 0x817FD80200002B73, 0),
        ]),
    ),
    (
        RIALTO,
        Graph::Air,
        [131114, 8311, 130702],
        [131123, 8315, 130709],
        Some(&[
            (43, 0x817FA800000034EB, 26),
            (60, 0x817FA800000038EB, 6),
            (1, 0x817FA8000000392F, 6),
            (22, 0x817FA8000000392F, 6),
            (43, 0x817FA8000000392F, 6),
            (63, 0x817FA8000000392F, 1),
            (3, 0x817FA80000003973, 1),
            (23, 0x817FA80000003973, 1),
            (39, 0x817FA80000003973, 2),
            (55, 0x817FA80000003973, 2),
        ]),
    ),
];

#[test]
#[ignore = "requires derived 104319 AI cells"]
fn the_query_finds_the_clients_paths() {
    let runtime = orpheus_testkit::runtime_dir();
    let range = AiCells::load(file(&runtime, RANGE)).unwrap();
    let rialto = AiCells::load(file(&runtime, RIALTO)).unwrap();
    for (map, graph, [x, y, z], [gx, gy, gz], client) in CLIENT_PATHS {
        let cells = if map == RANGE { &range } else { &rialto };
        let end = |voxel: Voxel| End {
            node: cells.node(graph, voxel).unwrap(),
            sub: voxel.sub(),
        };
        let start = end(Voxel { x, y, z });
        let goal = end(Voxel {
            x: gx,
            y: gy,
            z: gz,
        });
        let steps = client_path(cells, graph, start, goal).map(|steps| {
            steps
                .iter()
                .map(|step| (step.sub, step.node.client_id(), step.direction))
                .collect::<Vec<_>>()
        });
        assert_eq!(steps.as_deref(), client, "{graph:?} {start:?}");
    }
}
