//! Frozen player-body creation records for the 104319 match.
//!
//! Owner and observer layouts follow Match._body_builder and Player.pose
//! in ow174/game/match.py.

use std::f64::consts::PI;

use crate::pools::PoolCatalog;
use lucy_content::heroes::Hero;
use lucy_wire::entity::{
    Component, ComponentValue, Create, CreateFrame, EntityUpdate, FRAME_ZERO_BACK, LeaseBlock,
    Movement, NO_INPUT, Operation, SkinTheme,
};

pub const CROUCHED_MOVEMENT_FLAG: u32 = 0x2;
/// Frames past the packet's frame for the movement state of the owner's
/// create of his own body: his client's command frames run at most 16
/// ahead of a packet's frame (`0x7FF7896DAF7A`..`0x7FF7896DB296`), so the
/// state is ahead of every frame he predicted, and below the frames of the
/// records that follow (upstream `CREATE_LEAD`, leafall903/ORPHEUS (MIT)
/// 36d57fef; checked there with the original client).
pub const OWN_CREATE_LEAD: u32 = 18;

pub struct BodyCreate<'a> {
    pub entity: u32,
    pub hero: &'a Hero,
    /// The body's filter bits (component 26, `scene::body_filter`): its
    /// team's (the shared free-for-all one if applicable) and its kind.
    pub filter: u64,
    pub position: [f64; 3],
    pub yaw: i32,
    pub skin: SkinTheme,
    /// The body's current pools (component 51); None = the hero's own pools.
    pub health: Option<Component>,
    /// The target tags its states hold (component 76,
    /// `scene::target_tags_component`); None: none touched yet. The client
    /// reads a create's components with its update reader once the entity
    /// is built (`0x7FF78969C360` → `0x7FF78969E5D0`).
    pub tags: Option<Component>,
}

pub enum BodyView<'a> {
    Owner {
        pose: Option<Movement>,
        /// Frozen from the current body script after its instance is assigned.
        leases: &'a [LeaseBlock],
    },
    /// Another client's: `pose` is the body's mover pose, `gravity` its
    /// mover's gravity scale (as [`remote_movement`] sends them).
    Observer { pose: Movement, gravity: f64 },
}

/// Initial owner movement uses its pose and crouch flag. The client's mover
/// continues from this state without an input history or animation counters.
pub fn owner_pose(source: Movement) -> Movement {
    Movement {
        position: source.position,
        yaw: source.yaw,
        pitch: source.pitch,
        velocity: source.velocity,
        flags: source.flags & CROUCHED_MOVEMENT_FLAG,
        ..Movement::default()
    }
}

/// Another player's body as other clients' movement records carry it
/// (match.py Player.movement): their animation plays idle, run, crouch or a
/// jump from it. The record's frame is the packet's (the server tick), so the
/// client reads the input fields. A movement key counts as held while the
/// last command with throttles is not older than this tick, so a standing
/// body carries the frame of that command. The gravity scale is the body's
/// mover's (mover.py `GRAVITY_SCALE`, every hero's 1.75).
///
/// The spring steps and speed are the mover's. Python sent neither: steps 0,
/// the bottom of their range, on which the other clients put the body's
/// collision capsule a whole stance and pogo too low, under the floor
/// (`Posture::spring_steps` in `lucy-physics`; measured on remote NPC
/// bodies, leafall903/ORPHEUS (MIT) `docs/MCCREE_ROLL.txt`). Not checked
/// with two clients: that an enemy player now blocks the other's walk, and
/// that the other's own shots now meet his body.
pub fn remote_movement(pose: Movement, tick: u32, gravity: f64) -> Movement {
    // The mover's pose says None when its own frame had throttles.
    let last_input = match pose.input_frame {
        None => pose.frame,
        Some(NO_INPUT) => None,
        Some(frame) => Some(frame),
    };
    let input_frame = if pose.throttles != [0; 2] || last_input.is_some_and(|frame| frame >= tick) {
        None
    } else {
        Some(last_input.unwrap_or(NO_INPUT))
    };
    Movement {
        position: pose.position,
        yaw: pose.yaw,
        pitch: pose.pitch,
        velocity: pose.velocity,
        flags: pose.flags,
        throttles: pose.throttles,
        input_frame,
        input_throttles: pose.input_throttles,
        air_ticks: pose.air_ticks,
        gravity,
        spring: pose.spring,
        spring_speed: pose.spring_speed,
        move_to: pose.move_to,
        turn_rate: pose.turn_rate,
        ..Movement::default()
    }
}

pub fn yaw_rotation(yaw: f64) -> [f32; 4] {
    let half = yaw * PI / 65536.0;
    let (sin, cos) = half.sin_cos();
    [0.0, sin as f32, 0.0, cos as f32]
}

/// Capture the ordered components, skin and pose once so budget probes and
/// reliable retries serialize the same entity lifetime.
pub fn body_create(input: BodyCreate<'_>, view: BodyView<'_>, pools: &PoolCatalog) -> EntityUpdate {
    let health = input
        .health
        .unwrap_or_else(|| pools.body_pools(input.hero.guid).vitals().component());
    let mut components = vec![
        health,
        Component {
            index: 26,
            fields: vec![Some(ComponentValue::Id(input.filter))],
        },
        Component {
            index: 123,
            fields: vec![Some(ComponentValue::Id(input.hero.guid))],
        },
    ];
    let mut definition = Create::new(input.hero.body, Vec::new());
    // The owner's create puts his body's state [`OWN_CREATE_LEAD`] frames
    // past the packet's frame. His client keeps every body's movement
    // states, his own one's too, and takes a later record into them only
    // above the newest kept frame (`0x7FF7896DA7E7`, unsigned); at the
    // create's default 0xFFFFFFFE none would ever get in, and while a
    // `STU_4773DA56` registration is on (Rocket Punch's dash, `0D06` st67)
    // the client draws his first-person view from those kept states: it
    // stayed at the spawn, out of the map (refs report Doomfist.md). At a
    // frame he had already predicted, upstream saw his body start under
    // the floor. Another client's create carries the body's state at frame
    // 0, older than every record after it, or its component 24 sphere
    // history (`0x7FF789B80F70`, appends only on a higher frame) keeps the
    // create's sample and its shots' rewind meets the body at the world
    // origin (HitPredict.md "Day (2026-11-17), part 2"; bots likewise,
    // `scene::bot_create`).
    let movement = match view {
        BodyView::Owner { pose, leases } => {
            if !leases.is_empty() {
                components.push(Component {
                    index: 90,
                    fields: vec![Some(ComponentValue::Leases(leases.to_vec()))],
                });
            }
            definition.movement = pose;
            definition.frame = CreateFrame::PastPacket(OWN_CREATE_LEAD);
            pose
        }
        BodyView::Observer { pose, gravity } => {
            definition.movement = Some(Movement {
                // The create's own frame carries no input.
                input_frame: None,
                throttles: [0; 2],
                input_throttles: [0; 2],
                ..remote_movement(pose, 0, gravity)
            });
            definition.frame = CreateFrame::Back(FRAME_ZERO_BACK);
            None
        }
    };
    components.extend(input.tags);
    definition.components = components;
    definition.position = input.position.map(|position| position as f32);
    definition.rotation = yaw_rotation(f64::from(input.yaw));
    definition.skin = input.skin;
    EntityUpdate {
        entity: input.entity,
        operation: Some(Operation::Create(Box::new(definition))),
        movement,
        ..EntityUpdate::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lucy_content::heroes::SOLDIER;
    use lucy_core::bits::{BitReader, BitWriter};
    use lucy_script::vitals::{Death, Vitals};
    use lucy_wire::entity::{
        CREATE_BASE, WITH_MOVEMENT, WITH_SCALE, WITH_TRANSFORM, create, frame, write_movement,
    };
    use sha2::{Digest, Sha256};

    const BODY: u32 = 0xa000_0101;
    const REINHARDT: u64 = 0x02e0_0000_0000_0007;
    const THEME: u64 = 0x0a50_0000_0000_1234;
    const TEAM_BIT: u64 = 0x80_0000;
    const LEASE_SPAN: u32 = 1 << 22;

    fn leases() -> [LeaseBlock; 2] {
        [4, 5].map(|index| LeaseBlock {
            definition: 0x0400_0000_0000_1180,
            start: index * LEASE_SPAN,
            stop: (index + 1) * LEASE_SPAN,
            first_id: 0x10000,
            per_frame: 1,
        })
    }

    fn definition(update: &EntityUpdate) -> &Create {
        let Some(Operation::Create(definition)) = &update.operation else {
            panic!("expected body create");
        };
        definition
    }

    #[test]
    fn owner_create_captures_pose_golden_skin_and_lease_in_python_order() {
        let hero = Hero {
            guid: SOLDIER,
            name: "Soldier: 76".into(),
            role: "Damage".into(),
            body: 0x0400_0000_0000_03cf,
        };
        let source = Movement {
            position: [54.56640625, 1.125, 42.1474609375],
            yaw: -31010,
            pitch: -1234,
            velocity: [0.125, -0.5, 5.5],
            flags: CROUCHED_MOVEMENT_FLAG | 0x2000,
            throttles: [5, 100],
            input_frame: Some(900),
            input_throttles: [25, 127],
            air_ticks: 7,
            gravity: 1.75,
            frame: Some(899),
            crouch_frame: Some(850),
            spring: [125, 73],
            spring_speed: 2.0,
            fall: 0.25,
            // A create starts the body's counter at 0.
            discontinuity: 3,
            move_to: None,
            turn_rate: None,
            nav_path: None,
        };
        let pose = owner_pose(source);
        assert_eq!(
            pose,
            Movement {
                position: source.position,
                yaw: source.yaw,
                pitch: source.pitch,
                velocity: source.velocity,
                flags: CROUCHED_MOVEMENT_FLAG,
                ..Movement::default()
            }
        );
        let mut lease = leases();
        let update = body_create(
            BodyCreate {
                entity: BODY,
                hero: &hero,
                filter: TEAM_BIT << 1,
                position: [54.56507, 1.0, 42.14788],
                yaw: -31000,
                skin: SkinTheme {
                    guid: THEME,
                    golden: true,
                },
                health: None,
                tags: None,
            },
            BodyView::Owner {
                pose: Some(pose),
                leases: &lease,
            },
            &PoolCatalog::default(),
        );
        assert_eq!(update.entity, BODY);
        assert_eq!(update.movement, Some(pose));
        let definition = definition(&update);
        assert_eq!(definition.flags, WITH_SCALE | WITH_TRANSFORM);
        assert_eq!(definition.frame, CreateFrame::PastPacket(OWN_CREATE_LEAD));
        assert_eq!(definition.movement, update.movement);
        assert_eq!(
            definition
                .components
                .iter()
                .map(|component| component.index)
                .collect::<Vec<_>>(),
            [51, 26, 123, 90]
        );
        assert_eq!(
            definition.components[3].fields[0],
            Some(ComponentValue::Leases(lease.to_vec()))
        );
        // Python's create kept the create's default frame; with it the
        // record is Python's.
        let python = Create {
            frame: CreateFrame::Back(1),
            ..definition.clone()
        };
        let record = create(3, &python, 1200).unwrap();
        assert_eq!(record.bit_len(), 1365);
        assert_eq!(
            format!("{:x}", Sha256::digest(record.as_bytes())),
            "a157b1187c3679a0e869d69ec935efa82d85d66b92ed5f3616fea8c93fe1ee46"
        );
        let python_update = EntityUpdate {
            operation: Some(Operation::Create(Box::new(python.clone()))),
            ..update.clone()
        };
        let packet = frame(1200, 900, 0, &[], &[], &[&python_update]).unwrap();
        assert_eq!(
            format!("{:x}", Sha256::digest(&packet)),
            "44d49b099d2766c02592e1dd41acef3d53b1d41c10bd6c9edb173755d683dbab"
        );
        // Ours puts the state 18 frames past each packet that carries it.
        for tick in [1200, 1300] {
            let at = Create {
                frame: CreateFrame::Back(CREATE_BASE - (tick + OWN_CREATE_LEAD)),
                ..definition.clone()
            };
            assert_eq!(
                create(3, definition, tick).unwrap(),
                create(3, &at, 0).unwrap()
            );
        }
        lease[0].start = 9;
        assert_eq!(create(3, &python, 1200).unwrap(), record);
        assert_ne!(
            definition.components[3].fields[0],
            Some(ComponentValue::Leases(lease.to_vec()))
        );
    }

    #[test]
    fn remote_records_hold_keys_until_the_tick_after_the_last_input() {
        let standing = Movement {
            position: [1.0, 2.0, 3.0],
            yaw: 1200,
            pitch: -40,
            flags: CROUCHED_MOVEMENT_FLAG,
            input_throttles: [0, 127],
            air_ticks: 3,
            gravity: 1.0,
            frame: Some(1000),
            input_frame: Some(990),
            crouch_frame: Some(995),
            spring: [4, 5],
            spring_speed: 0.25,
            ..Movement::default()
        };
        let record = remote_movement(standing, 1003, 1.75);
        // The spring goes as the mover has it: Python's records left it out,
        // and the client then put the body's capsule under the floor
        // (`remote_movement`).
        assert_eq!(
            record,
            Movement {
                position: standing.position,
                yaw: 1200,
                pitch: -40,
                flags: CROUCHED_MOVEMENT_FLAG,
                input_frame: Some(990),
                input_throttles: [0, 127],
                air_ticks: 3,
                gravity: 1.75,
                spring: [4, 5],
                spring_speed: 0.25,
                ..Movement::default()
            }
        );
        // The record's frame is the packet's: an input at or after this tick
        // is still held, and a running body has no input frame.
        assert_eq!(remote_movement(standing, 990, 1.75).input_frame, None);
        let running = Movement {
            throttles: [0, 127],
            input_frame: None,
            ..standing
        };
        assert_eq!(remote_movement(running, 1003, 1.75).input_frame, None);
        let fresh = Movement::default();
        assert_eq!(
            remote_movement(fresh, 1003, 1.75).input_frame,
            Some(NO_INPUT)
        );
    }

    /// Another client's create of a player's body: its damaged pools
    /// frozen, no lease, and its movement state (as its records carry it,
    /// with no input) at frame 0, which the client's histories of the body
    /// take the later records after. Python's create had no state; the rest
    /// of the record is its bytes, the components padded from where they
    /// now start.
    #[test]
    fn observer_create_freezes_damaged_pools_and_puts_the_pose_at_frame_zero() {
        let hero = Hero {
            guid: REINHARDT,
            name: "Reinhardt".into(),
            role: "Tank".into(),
            body: 0x0400_0000_0000_0024,
        };
        let mut live = Vitals::new(300.0, 200.0, 0.0);
        let hit = Death {
            time: 3.0,
            frame: 1200,
            killer: BODY,
        };
        assert_eq!(live.damage(30.0, hit).dealt, 25.0);
        live.clear_changed();
        let pose = Movement {
            position: [4.5, 2.0, -3.25],
            yaw: 16384,
            velocity: [0.0, 0.0, 2.5],
            flags: CROUCHED_MOVEMENT_FLAG,
            throttles: [0, 127],
            input_frame: Some(1190),
            spring: [125, 73],
            spring_speed: 2.0,
            ..Movement::default()
        };
        let update = body_create(
            BodyCreate {
                entity: 0xa000_0201,
                hero: &hero,
                filter: TEAM_BIT,
                position: pose.position,
                yaw: pose.yaw,
                skin: SkinTheme {
                    guid: 0,
                    golden: true,
                },
                health: Some(live.component()),
                tags: None,
            },
            BodyView::Observer {
                pose,
                gravity: 1.75,
            },
            &PoolCatalog::default(),
        );
        assert_eq!(update.movement, None);
        let definition = definition(&update);
        let state = Movement {
            input_frame: None,
            throttles: [0; 2],
            input_throttles: [0; 2],
            ..remote_movement(pose, 0, 1.75)
        };
        assert_eq!(definition.movement, Some(state));
        assert_eq!(
            definition
                .components
                .iter()
                .map(|component| component.index)
                .collect::<Vec<_>>(),
            [51, 26, 123]
        );
        let python = Create {
            movement: None,
            frame: CreateFrame::Back(1),
            ..definition.clone()
        };
        let python = create(5, &python, 0).unwrap();
        assert_eq!(python.bit_len(), 843);
        assert_eq!(
            format!("{:x}", Sha256::digest(python.as_bytes())),
            "79d53aa18f160aa2245a17da9ce7f2ef80ac04dae880f65830f66c9c4d4a1898"
        );
        let record = create(5, definition, 0).unwrap();
        let mut bits = BitReader::new(record.as_bytes());
        let mut plain = BitReader::new(python.as_bytes());
        let same = |bits: &mut BitReader<'_>, other: &mut BitReader<'_>, count: usize| {
            for _ in 0..count {
                assert_eq!(bits.bit().unwrap(), other.bit().unwrap());
            }
        };
        // The definition's index, then its flags with the movement bit.
        same(&mut bits, &mut plain, 1);
        assert_eq!(bits.guid_index().unwrap(), plain.guid_index().unwrap());
        let flags = plain.bits(8).unwrap();
        assert_eq!(bits.bits(8).unwrap(), flags | u64::from(WITH_MOVEMENT));
        // The transform and scale, then the state.
        same(&mut bits, &mut plain, 10 * 32);
        let start = bits.position();
        // Its flags (the crouch bit in the first byte), the grid, then the
        // frame counted back from the create's base in 32 bits: frame 0.
        assert!(bits.bit().unwrap());
        assert_eq!(
            (bits.bit().unwrap(), bits.bits(8).unwrap()),
            (true, u64::from(CROUCHED_MOVEMENT_FLAG))
        );
        for _ in 1..4 {
            assert!(!bits.bit().unwrap());
        }
        assert!(!bits.bit().unwrap());
        assert!(bits.bit().unwrap());
        assert!(!bits.bit().unwrap());
        assert!(!bits.bit().unwrap());
        let frame_back = bits.bits(32).unwrap() as u32;
        assert_eq!(CREATE_BASE.wrapping_sub(frame_back), 0);
        // The whole state as a create writes it, then the components as
        // Python's create has them, padded to bytes from their new start.
        let mut written = BitWriter::new();
        write_movement(
            &mut written,
            &state,
            Some(frame_back),
            Some(CREATE_BASE),
            None,
        )
        .unwrap();
        let mut bits = BitReader::with_range(record.as_bytes(), start, record.bit_len()).unwrap();
        same(
            &mut bits,
            &mut BitReader::new(written.as_bytes()),
            written.bit_len(),
        );
        let at = |origin| lucy_wire::entity::update(origin, &definition.components).unwrap();
        let components = at(5 + bits.position());
        assert_eq!(bits.left(), components.bit_len());
        same(
            &mut bits,
            &mut BitReader::new(components.as_bytes()),
            components.bit_len(),
        );
        // Python's create ends in the same components.
        let python_components = at(5 + plain.position());
        assert_eq!(
            python.bit_len() - plain.position(),
            python_components.bit_len()
        );
        same(
            &mut plain,
            &mut BitReader::new(python_components.as_bytes()),
            python_components.bit_len(),
        );
        assert_eq!(live.heal(25.0), 25.0);
        assert_eq!(create(5, definition, 0).unwrap(), record);
        assert_ne!(live.component(), definition.components[0]);
    }

    #[test]
    #[ignore = "requires the derived 104319 hero and graph catalogs"]
    fn all_heroes_match_python_owner_and_observer_create_bytes() {
        use lucy_content::heroes::HeroCatalog;
        use lucy_script::graph::GraphStore;

        let root = lucy_testkit::runtime_dir();
        let heroes = HeroCatalog::load(root.join("data/game_heroes_174.json")).unwrap();
        let graphs =
            GraphStore::load_runtime(root.join("data"), "game_graphs_174.json.gz").unwrap();
        let pools = PoolCatalog::from_graphs(&graphs).unwrap();
        let leases = leases();
        let mut digest = Sha256::new();
        assert_eq!(heroes.heroes().count(), 32);
        for (index, hero) in heroes.heroes().enumerate() {
            let step = i32::try_from(index).unwrap();
            let pose = Movement {
                position: [
                    f64::from(step) / 8.0 + 1.0,
                    1.0,
                    -4.5 + f64::from(step) / 16.0,
                ],
                yaw: -30000 + step * 1473,
                pitch: 700 - step * 9,
                velocity: [0.25, 0.0, -1.0],
                flags: if index % 3 == 0 {
                    CROUCHED_MOVEMENT_FLAG
                } else {
                    0
                },
                ..Movement::default()
            };
            for own in [false, true] {
                let origin = (index + usize::from(own)) % 8;
                let view = if own {
                    BodyView::Owner {
                        pose: Some(pose),
                        leases: &leases,
                    }
                } else {
                    BodyView::Observer {
                        pose,
                        gravity: 1.75,
                    }
                };
                let body = body_create(
                    BodyCreate {
                        entity: BODY,
                        hero,
                        filter: TEAM_BIT << (index % 2),
                        position: pose.position,
                        yaw: pose.yaw,
                        skin: SkinTheme {
                            guid: THEME,
                            golden: index % 2 == 0,
                        },
                        health: None,
                        tags: None,
                    },
                    view,
                    &pools,
                );
                // Python's observer create had no movement state (ours at
                // frame 0: `observer_create_freezes_damaged_pools_and_puts_the_pose_at_frame_zero`),
                // and its owner create kept the create's default frame
                // (ours: `OWN_CREATE_LEAD` past the packet).
                let ours = definition(&body);
                let python = Create {
                    movement: if own { ours.movement } else { None },
                    frame: CreateFrame::Back(1),
                    ..ours.clone()
                };
                let record = create(origin, &python, 0).unwrap();
                digest.update(hero.guid.to_le_bytes());
                digest.update([u8::from(own)]);
                digest.update(u32::try_from(origin).unwrap().to_le_bytes());
                digest.update(u32::try_from(record.bit_len()).unwrap().to_le_bytes());
                digest.update(record.as_bytes());
            }
        }
        assert_eq!(
            format!("{:x}", digest.finalize()),
            "d307ad8c6f86b8e05c30a717afc32b29e2e15022d410f4e5b4572abd1a8f84ab"
        );
    }
}
