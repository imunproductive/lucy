use super::*;
use lucy_core::bits::BitReader;

fn hex(encoded: &str) -> Vec<u8> {
    encoded
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}

fn entity(entity: u32, operation: Operation) -> EntityUpdate {
    EntityUpdate {
        entity,
        operation: Some(operation),
        ..EntityUpdate::default()
    }
}

fn component<const N: usize>(index: u8, fields: [Option<ComponentValue>; N]) -> Component {
    Component {
        index,
        fields: fields.into(),
    }
}

fn moving() -> Movement {
    Movement {
        position: [1.5, -2.25, 3.0],
        yaw: 33067,
        pitch: -32769,
        velocity: [0.5, 2.0, -1.25],
        flags: 0x8010_0405,
        throttles: [-128, 73],
        input_frame: Some(4997),
        input_throttles: [5, 6],
        air_ticks: 9,
        gravity: 1.75,
        frame: Some(4997),
        crouch_frame: Some(4996),
        spring: [-130, -3000],
        spring_speed: 1.625,
        fall: -3.5,
        discontinuity: 0,
        move_to: None,
        turn_rate: None,
        nav_path: None,
        parent: None,
    }
}

#[test]
fn placeable_create_and_pose_match_python() {
    // The fixtures in tests/test_game_world.py, emitted by Python world.py.
    let item = entity(0x8000_0088, Operation::Placeable(0x88, Vec::new()));
    assert_eq!(
        frame(1234, 1234, 0, &[], &[], &[&item]).unwrap(),
        hex("a509509a005000884b5301220200000000")
    );
    let pose = movement_record(
        &Movement {
            position: [54.56507, 1.0, 42.14788],
            yaw: -31000,
            ..Movement::default()
        },
        None,
    )
    .unwrap();
    assert_eq!(pose.bit_len(), 148);
    assert_eq!(
        pose.as_bytes(),
        hex("94000024ba2139a40d9000040079890a000000")
    );
}

#[test]
fn movement_timing_sentinels_and_signed_fields_match_python() {
    let record = movement_record(&moving(), Some(5000)).unwrap();
    assert_eq!(record.bit_len(), 476);
    assert_eq!(
        record.as_bytes(),
        hex(concat!(
            "dce1820421027ae4ff5f139000070044380100107052ae04920006000970ff9f",
            "000c00134000200110001260ff1f80fa7d021a0024127d0500000306",
        ))
    );
    let never_moved = Movement {
        position: [1.0, 2.0, 3.0],
        input_frame: Some(NO_INPUT),
        input_throttles: [0, 127],
        ..Movement::default()
    };
    let old = Movement {
        frame: Some(3976),
        gravity: 0.5 / POSITION_UNITS, // Ties round to zero, as Python round() does.
        ..never_moved
    };
    let record = movement_record(&old, Some(5000)).unwrap();
    assert_eq!(record.bit_len(), 174);
    assert_eq!(
        record.as_bytes(),
        hex("ae800010000000c13f24000140022000240003000000")
    );

    let mut record = BitWriter::new();
    write_movement(&mut record, &never_moved, Some(0), Some(CREATE_BASE), None).unwrap();
    assert_eq!(record.bit_len(), 116);
    assert_eq!(record.as_bytes(), hex("00000940009000080009c000000000"));
    assert!(matches!(
        movement_record(&never_moved, None),
        Err(WorldError::InputFrameNeedsRecordFrame)
    ));
    assert!(matches!(
        movement_record(&old, Some(3975)),
        Err(WorldError::FutureMovement {
            frame: 3976,
            tick: 3975
        })
    ));
}

/// The discontinuity counter (+12) follows the frame: a 0 bit when it is 0,
/// else 1, 0 (no short width) and its 32 bits (ow1-preservation's field 3).
#[test]
fn discontinuity_counter_follows_the_frame_as_raw_32_bits() {
    let still = Movement {
        position: [1.0, 2.0, 3.0],
        ..Movement::default()
    };
    let turned = Movement {
        discontinuity: 7,
        ..still
    };
    let plain = movement_record(&still, None).unwrap();
    let record = movement_record(&turned, None).unwrap();
    assert_eq!(record.bit_len(), plain.bit_len() + 33);
    let mut bits = BitReader::new(record.as_bytes());
    assert_eq!(bits.bits(12).unwrap() as usize, record.bit_len());
    // The reference bit, no flags, the 1/1024 grid, the base frame.
    assert_eq!(bits.bits(4).unwrap(), 0);
    assert!(bits.bit().unwrap());
    assert!(!bits.bit().unwrap());
    assert_eq!(bits.bits(32).unwrap(), 7);
    // The rest is the record without the counter.
    let mut rest = BitReader::new(plain.as_bytes());
    rest.advance(17).unwrap();
    for _ in 17..plain.bit_len() {
        assert_eq!(bits.bit().unwrap(), rest.bit().unwrap());
    }
}

/// A running `MoveTo` move's record goes as the first of the nine flag
/// blocks after the velocity (0x7FF789B6C720): present, no reference, the
/// phase in 8 bits, the checkpoint's axes as raw floats (an axis of 0 left
/// out), `still` in 16 bits (0 left out).
#[test]
fn a_move_to_record_is_the_first_flag_block_after_the_velocity() {
    let still = Movement {
        position: [1.0, 2.0, 3.0],
        ..Movement::default()
    };
    let pulled = Movement {
        flags: MOVE_TO,
        move_to: Some(MoveToRecord {
            checkpoint: [54.5654296875, 0.0, -42.25],
            still: 3,
            phase: 1,
        }),
        ..still
    };
    let plain = movement_record(
        &Movement {
            flags: MOVE_TO,
            ..still
        },
        None,
    )
    .unwrap();
    let record = movement_record(&pulled, None).unwrap();
    // Phase 1 + 8, checkpoint 1 + 33 + 1 + 33, still 1 + 16, and the
    // block's absolute bit.
    assert_eq!(record.bit_len(), plain.bit_len() + 1 + 9 + 68 + 17);
    let mut bits = BitReader::new(record.as_bytes());
    let mut rest = BitReader::new(plain.as_bytes());
    bits.advance(12).unwrap();
    rest.advance(12).unwrap();
    // Everything up to the block's presence bit is the same.
    let at = (12..plain.bit_len())
        .find(|_| bits.bit().unwrap() != rest.bit().unwrap())
        .unwrap();
    assert!(bits.bit().unwrap(), "no reference");
    assert!(bits.bit().unwrap());
    assert_eq!(bits.bits(8).unwrap(), 1);
    assert!(bits.bit().unwrap());
    assert!(bits.bit().unwrap());
    assert_eq!(bits.f32().unwrap(), 54.565_43);
    assert!(!bits.bit().unwrap());
    assert!(bits.bit().unwrap());
    assert_eq!(bits.f32().unwrap(), -42.25);
    assert!(bits.bit().unwrap());
    assert_eq!(bits.bits(16).unwrap(), 3);
    // Then the other eight blocks and the rest, as without the record.
    for _ in at + 1..plain.bit_len() {
        assert_eq!(bits.bit().unwrap(), rest.bit().unwrap());
    }
}

/// A running path walk's record goes as the sixth flag block (flag 0x20,
/// 0x7FF789B6C800): present, no reference, the state in 8 bits, the seconds,
/// the seconds walked and the cap in 1/256 steps (8, 10 or 24 bits behind
/// width bits), `+12`, `+0` and `+32` absent, the count as an s8, each point
/// as optional raw floats.
#[test]
fn a_path_walk_record_is_the_sixth_flag_block() {
    let still = Movement {
        position: [1.0, 2.0, 3.0],
        flags: NAV_PATH,
        ..Movement::default()
    };
    let mut walk = NavPathRecord {
        state: 1,
        seconds: 0.2,
        elapsed: 0.016,
        cap: 12.5,
        count: 2,
        ..NavPathRecord::default()
    };
    walk.points[0] = [54.5, 6.75, 29.0];
    walk.points[1] = [54.5, 0.0, 28.25];
    let walking = Movement {
        nav_path: Some(walk),
        ..still
    };
    let plain = movement_record(&still, None).unwrap();
    let record = movement_record(&walking, None).unwrap();
    let mut bits = BitReader::new(record.as_bytes());
    let mut rest = BitReader::new(plain.as_bytes());
    bits.advance(12).unwrap();
    rest.advance(12).unwrap();
    // Everything up to the block's presence bit is the same.
    let at = (12..plain.bit_len())
        .find(|_| bits.bit().unwrap() != rest.bit().unwrap())
        .unwrap();
    assert!(bits.bit().unwrap(), "no reference");
    assert!(bits.bit().unwrap());
    assert_eq!(bits.bits(8).unwrap(), 1);
    // 0.2 s is 51 steps, 0.016 s 4, the cap 3200 (past 10 bits).
    for steps in [51, 4] {
        assert!(bits.bit().unwrap() && bits.bit().unwrap());
        assert_eq!(bits.bits(8).unwrap(), steps);
    }
    assert!(bits.bit().unwrap() && !bits.bit().unwrap() && !bits.bit().unwrap());
    assert!(bits.bit().unwrap());
    assert_eq!(bits.bits(24).unwrap(), 3200);
    assert!(!bits.bit().unwrap() && !bits.bit().unwrap() && !bits.bit().unwrap());
    assert!(bits.bit().unwrap() && bits.bit().unwrap());
    assert_eq!(bits.bits(8).unwrap(), 2);
    assert!(bits.bit().unwrap());
    for axis in [54.5, 6.75, 29.0] {
        assert!(bits.bit().unwrap());
        assert_eq!(bits.f32().unwrap(), axis);
    }
    assert!(bits.bit().unwrap());
    assert!(bits.bit().unwrap());
    assert_eq!(bits.f32().unwrap(), 54.5);
    assert!(!bits.bit().unwrap());
    assert!(bits.bit().unwrap());
    assert_eq!(bits.f32().unwrap(), 28.25);
    // Then the last three blocks and the rest, as without the record.
    for _ in at + 1..plain.bit_len() {
        assert_eq!(bits.bit().unwrap(), rest.bit().unwrap());
    }
}

/// A running turn's rate goes as the fifth flag block (flag 0x40,
/// 0x7FF789B6CBF0): present, no reference, then the float as present and
/// raw (0x7FF789B71860).
#[test]
fn a_turn_rate_is_the_fifth_flag_block() {
    let still = Movement {
        position: [1.0, 2.0, 3.0],
        flags: TURNING,
        ..Movement::default()
    };
    let turning = Movement {
        turn_rate: Some(-0.067_020_65),
        ..still
    };
    let plain = movement_record(&still, None).unwrap();
    let record = movement_record(&turning, None).unwrap();
    assert_eq!(record.bit_len(), plain.bit_len() + 3 + 32);
    let mut bits = BitReader::new(record.as_bytes());
    let mut rest = BitReader::new(plain.as_bytes());
    bits.advance(12).unwrap();
    rest.advance(12).unwrap();
    // Everything up to the block's presence bit is the same.
    let at = (12..plain.bit_len())
        .find(|_| bits.bit().unwrap() != rest.bit().unwrap())
        .unwrap();
    assert!(bits.bit().unwrap(), "no reference");
    assert!(bits.bit().unwrap() && bits.bit().unwrap(), "a raw float");
    assert_eq!(bits.f32().unwrap(), -0.067_020_65);
    for _ in at + 1..plain.bit_len() {
        assert_eq!(bits.bit().unwrap(), rest.bit().unwrap());
    }
}

#[test]
fn sparse_pool_update_distinguishes_depletion_removal_and_omission() {
    let component = pool_component([
        (SHIELDS_POOL * POOLS + 15, [0; 20]),
        (
            ARMOUR_POOL * POOLS + 1,
            pool(2, ARMOUR_POOL as u32, 50.0, 0.0),
        ),
    ]);
    let record = update(0, &[component]).unwrap();
    // The frozen world.update emits fields 17 and 47 in mask order.
    assert_eq!(record.bit_len(), 400);
    assert_eq!(
        record.as_bytes(),
        hex(concat!(
            "01330700000200008000",
            "0200000000004842000000000100000000010001",
            "0000000000000000000000000000000000000000",
        ))
    );
}

#[test]
fn mixed_frame_matches_python_at_every_component_and_channel_boundary() {
    use ComponentValue::*;

    // The pools of a body with 200 health (125 left), 25 armour and 50 shields.
    let mut pools = pool_component([
        (
            HEALTH_POOL * POOLS,
            pool(3, HEALTH_POOL as u32, 200.0, 125.0),
        ),
        (ARMOUR_POOL * POOLS, pool(2, ARMOUR_POOL as u32, 25.0, 25.0)),
        (
            SHIELDS_POOL * POOLS,
            pool(1, SHIELDS_POOL as u32, 50.0, 50.0),
        ),
    ]);
    pools.fields.extend([
        Some(U32(0xabc)),
        Some(U32(0xdef)),
        Some(Entity(0x8000_0100)),
        Some(U8(1)),
        Some(U8(2)),
    ]);
    let components = vec![
        component(26, [Some(Id(0x80_0000))]),
        component(
            29,
            [
                Some(DbId(0x100, 0x1_2000_0000)),
                Some(Text("Ä#1".into())),
                Some(U32(7)),
                Some(I32(-1)),
                Some(I32(2)),
                Some(I32(-3)),
                Some(I32(4)),
                Some(I16(-1500)),
                Some(I16(21)),
                Some(I8(-2)),
                Some(U8(3)),
            ],
        ),
        component(
            44,
            [Some(Entity(0xa000_0102)), Some(Entity(0x8000_0077)), None],
        ),
        pools,
        component(
            74,
            [
                Some(Id(0x4003_0000_0000_0000)),
                Some(I16(-102)),
                Some(I8(-3)),
                Some(U8(1)),
                None,
                Some(I8(-7)),
                Some(U8(2)),
                Some(U8(3)),
                Some(U8(4)),
            ],
        ),
        component(
            75,
            [
                Some(Id(1)),
                None,
                Some(Id(3)),
                None,
                Some(Id(0)),
                Some(Id(0xdead_beef_0000_0001)),
            ],
        ),
        component(
            114,
            [
                Some(U32Array(vec![1, 2, u32::MAX])),
                Some(Id(0x0240_0000_0000_0001)),
                None,
                Some(U64(0x0011_2233_4455_6677)),
                Some(U64(0)),
                Some(U64(1)),
                Some(U64(2)),
                Some(I32(-77)),
                Some(F32(1.25)),
                Some(F32(-2.5)),
                Some(U32(0xffff)),
                Some(U8(1)),
            ],
        ),
        component(123, [Some(Id(0x02e0_0000_0000_006e))]),
        component(
            90,
            [Some(Leases(vec![LeaseBlock {
                definition: 0x0400_0000_0000_01f1,
                start: 4,
                stop: 7,
                first_id: 0xa000_0001,
                per_frame: 2,
            }]))],
        ),
    ];
    let mut body = Create::new(0x0400_0000_0000_012f, components);
    body.position = [1.5, -2.25, 3.0];
    body.rotation = [0.0, 0.5, 0.0, 1.0];
    body.movement = Some(moving());
    body.skin = SkinTheme {
        guid: 0x0a50_0000_0000_467c,
        golden: true,
    };
    body.movement_parent = Some(0xa000_00e0);

    let mut chunk = BitWriter::new();
    chunk.bits(0b1011, 4).unwrap();
    let mut ch2 = BitWriter::new();
    ch2.bits(0b101, 3).unwrap();
    let create = EntityUpdate {
        chunk: Some(chunk.clone()),
        ch2: Some(ch2),
        ..entity(0xa000_0100, Operation::Create(Box::new(body)))
    };
    let update = entity(
        0x8000_0077,
        Operation::Update(vec![component(44, [None, Some(Entity(0xa000_0001))])]),
    );
    let destroy = entity(0xa000_0110, Operation::Destroy);
    let chunk_only = EntityUpdate {
        entity: 0xa000_0111,
        chunk: Some(chunk),
        ..EntityUpdate::default()
    };
    let empty = EntityUpdate {
        entity: 0xa000_0112,
        ..EntityUpdate::default()
    };
    let payload = frame(
        0x1234_5678,
        0x0004_0001,
        0,
        &[(7, vec![0, 2, 0xab])],
        &[vec![2, 1, 0xcd]],
        &[&create, &update, &destroy, &chunk_only, &empty],
    )
    .unwrap();
    // Python world.frame on the same synthetic components, inputs and raw channel fragments.
    let expected = hex(concat!(
        "e059d1488c00070000000002ab0201cd060010800a051070bbad90f1259ef88c0000020000ff000040000300000101000000000000fc",
        "00000000000000fe000000fe000000fe000000fe5c9020444087fcff6b0212e000c00a270000229c000008386941835c0924010c0012",
        "e0fe3f0118002680004002200024c0fe3f80e00000a000000000f5fb0434004824fa0a0000062c4103010104801d02ff070201182001",
        "04000000c384233107000000ffffffff02000000fdffffff0400000024fa1500fe032c0103020100a07700008033070100010001001f",
        "03000000000048430000fa420000000000010001020000000000c8410000c84101000000000100010100000000004842000048420200",
        "000000010001bc0a0000ef0d00000001008001024a02ef01c003409afffd01f90203044b01350101010300f101efbeadde7203fb0f00",
        "030000000100000002000000ffffffffc10140027766554433221100000000000000000001000000000000000200000000000000b3ff",
        "ffff0000a03f000020c0ffff0000017b0101c16ee0025a010101000000f1010000000000040400000007000000010000a00270b7d421",
        "2080050102010000a00a2260e92a22e056490404",
    ));
    assert_eq!(payload, expected);
}

#[test]
fn budget_reuses_trimmed_space_and_only_displaces_remote_movement() {
    let reliable = [(1, vec![0; 720]), (2, vec![0; 700])];
    let unreliable = [vec![0; 300], vec![0; 200]];
    let queued = [entity(0x8000_0077, Operation::Placeable(0x77, Vec::new()))];
    let mut chunk = BitWriter::new();
    chunk.raw(&[0; 180]);
    let movements = [
        EntityUpdate {
            entity: 0x8000_0080,
            chunk: Some(chunk.clone()),
            ..EntityUpdate::default()
        },
        EntityUpdate {
            entity: 0x8000_0081,
            chunk: Some(chunk),
            ..EntityUpdate::default()
        },
        EntityUpdate {
            entity: 0xa000_0100,
            movement: Some(moving()),
            ..EntityUpdate::default()
        },
    ];
    let ack = |records: &[&EntityUpdate]| {
        u32::from(records.iter().any(|record| record.entity == 0xa000_0100))
    };
    let result = select_frame(
        FrameCandidates {
            tick: 5000,
            clock: 0,
            reliable: &reliable,
            unreliable: &unreliable,
            queued: &queued,
            movements: &movements,
            owner_correction: true,
            reliable_only: false,
        },
        ack,
    )
    .unwrap();
    assert_eq!((result.reliable_count, result.unreliable_count), (1, 1));
    assert_eq!(result.entities, [&queued[0], &movements[2]]);
    assert_eq!(
        result.payload,
        frame(
            5000,
            1,
            0,
            &reliable[..1],
            &unreliable[..1],
            &result.entities
        )
        .unwrap()
    );
    assert!(result.payload.len() <= MAX_PAYLOAD);
}

/// At the record cap the owner's correction takes the last slot: the queued
/// records keep their order and the 127th waits a packet. (The policy
/// changed on purpose, DoomfistMoves12: the correction used to go only once
/// every queued record fit, and a Workshop burst after a checkpoint's
/// teleport held the teleport back for ~0.5 s.)
#[test]
fn record_cap_keeps_fifo_and_reserves_the_owner_correction() {
    let queued: Vec<_> = (1..=127)
        .map(|id| EntityUpdate {
            entity: 0x8000_0000 | id,
            ..EntityUpdate::default()
        })
        .collect();
    let movements = [
        EntityUpdate {
            entity: 0x8000_0080,
            ..EntityUpdate::default()
        },
        EntityUpdate {
            entity: 0xa000_0100,
            movement: Some(Movement::default()),
            ..EntityUpdate::default()
        },
    ];
    for count in [126, 127] {
        let result = select_frame(
            FrameCandidates {
                tick: 5000,
                clock: 0,
                reliable: &[],
                unreliable: &[],
                queued: &queued[..count],
                movements: &movements,
                owner_correction: true,
                reliable_only: false,
            },
            |records| {
                u32::from(
                    records
                        .last()
                        .is_some_and(|record| record.entity == 0xa000_0100),
                )
            },
        )
        .unwrap();
        assert_eq!(result.entities.len(), MAX_ENTITY_RECORDS);
        assert!(
            result.entities[..126]
                .iter()
                .zip(&queued)
                .all(|(&selected, original)| std::ptr::eq(selected, original))
        );
        assert_eq!(result.entities[126].entity, 0xa000_0100);
        let mut reader = BitReader::new(&result.payload);
        assert_eq!(reader.var_a().unwrap(), 5000);
        assert!(!reader.bit().unwrap()); // Empty message channel.
        assert_eq!(reader.var_a().unwrap(), 1);
    }
}

/// A burst of queued records larger than many packets drains in bounded
/// ticks while the owner's correction goes in every packet: each packet
/// carries the correction and the longest prefix of the rest that fits.
#[test]
fn a_queued_burst_drains_while_the_owner_correction_goes_every_packet() {
    let mut chunk = BitWriter::new();
    chunk.raw(&[0x5a; 300]);
    let burst: Vec<_> = (1..=40)
        .map(|id| EntityUpdate {
            entity: 0x8000_0000 | id,
            chunk: Some(chunk.clone()),
            ..EntityUpdate::default()
        })
        .collect();
    let movements = [EntityUpdate {
        entity: 0xa000_0100,
        movement: Some(moving()),
        ..EntityUpdate::default()
    }];
    let mut left = &burst[..];
    let mut ticks = 0;
    while !left.is_empty() {
        let result = select_frame(
            FrameCandidates {
                tick: 5000 + ticks,
                clock: 0,
                reliable: &[],
                unreliable: &[],
                queued: left,
                movements: &movements,
                owner_correction: true,
                reliable_only: false,
            },
            |_| 0,
        )
        .unwrap();
        let (last, records) = result.entities.split_last().unwrap();
        assert!(std::ptr::eq(*last, &movements[0]), "tick {ticks}");
        assert!(!records.is_empty(), "tick {ticks}: no queued record went");
        assert!(
            records
                .iter()
                .zip(left)
                .all(|(&selected, original)| std::ptr::eq(selected, original))
        );
        assert!(result.payload.len() <= MAX_PAYLOAD);
        left = &left[records.len()..];
        ticks += 1;
    }
    // 40 records of ~300 bytes: at most four beside the correction in a
    // packet, so ten packets or more, and never more than one a record.
    assert!((10..=40).contains(&ticks), "{ticks}");
}

/// A queued record that fits only alone still goes: the correction then
/// waits, so a burst can't starve behind it.
#[test]
fn a_record_that_fits_only_alone_goes_before_the_owner_correction() {
    let record = |len: usize| {
        let mut chunk = BitWriter::new();
        chunk.raw(&vec![0x5a; len]);
        EntityUpdate {
            entity: 0x8000_0001,
            chunk: Some(chunk),
            ..EntityUpdate::default()
        }
    };
    // The largest record that fits a datagram alone.
    let alone = (0..MAX_PAYLOAD)
        .rev()
        .find(|&len| {
            frame(5000, 0, 0, &[], &[], &[&record(len)])
                .is_ok_and(|bytes| bytes.len() <= MAX_PAYLOAD)
        })
        .unwrap();
    let queued = [record(alone)];
    let movements = [EntityUpdate {
        entity: 0xa000_0100,
        movement: Some(moving()),
        ..EntityUpdate::default()
    }];
    let result = select_frame(
        FrameCandidates {
            tick: 5000,
            clock: 0,
            reliable: &[],
            unreliable: &[],
            queued: &queued,
            movements: &movements,
            owner_correction: true,
            reliable_only: false,
        },
        |_| 0,
    )
    .unwrap();
    assert_eq!(result.entities, [&queued[0]]);
}

#[test]
fn rejects_over_budget_oldest_records_and_malformed_components() {
    let reliable = [(1, vec![0; MAX_PAYLOAD])];
    let oversized = [entity(
        0x8000_0077,
        Operation::Update(vec![component(
            29,
            [None, Some(ComponentValue::Text("x".repeat(MAX_PAYLOAD)))],
        )]),
    )];
    let candidates = FrameCandidates {
        tick: 5000,
        clock: 0,
        reliable: &reliable,
        unreliable: &[],
        queued: &oversized,
        movements: &[],
        owner_correction: false,
        reliable_only: false,
    };
    assert!(matches!(
        select_frame(candidates, |_| 0),
        Err(WorldError::ReliableMessageDoesNotFit)
    ));
    let candidates = FrameCandidates {
        tick: 5000,
        clock: 0,
        reliable: &[],
        unreliable: &[],
        queued: &oversized,
        movements: &[],
        owner_correction: false,
        reliable_only: false,
    };
    assert!(matches!(
        select_frame(candidates, |_| 0),
        Err(WorldError::QueuedDataDoesNotFit)
    ));
    assert!(matches!(
        update(3, &[component(26, [Some(ComponentValue::U32(0))])]),
        Err(WorldError::FieldType {
            component: 26,
            field: 0
        })
    ));
    let oversized = entity(
        0x8000_0077,
        Operation::Update(vec![component(
            29,
            [None, Some(ComponentValue::Text("x".repeat(2048)))],
        )]),
    );
    assert!(matches!(
        frame(0, 0, 0, &[], &[], &[&oversized]),
        Err(WorldError::EntityRecordTooLong)
    ));
}
