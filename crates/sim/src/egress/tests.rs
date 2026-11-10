
use super::*;
use orpheus_core::bits::BitReader;
use orpheus_physics::mover::correction::CorrectionHistory;
use orpheus_wire::entity::{MAX_ENTITY_RECORDS, MAX_PAYLOAD, Movement, Operation};
use orpheus_wire::jam::codec::Schemas;
use orpheus_wire::messages::read_channel;

const MESSAGE: [u8; 2] = [1, 2];

fn frame_records<'a>(
    queued: &'a [EntityUpdate],
    movements: &'a [EntityUpdate],
) -> FrameRecords<'a> {
    FrameRecords {
        tick: 5000,
        queued,
        movements,
        owner_correction: false,
        reliable_only: false,
    }
}

fn destroy(id: u32) -> EntityUpdate {
    EntityUpdate {
        entity: 0x8000_0000 | id,
        operation: Some(Operation::Destroy),
        ..EntityUpdate::default()
    }
}

#[test]
fn transport_failure_retains_message_and_record_candidates() {
    let mut egress = GameEgress::default();
    let reliable = MESSAGE.to_vec();
    let original = reliable.as_ptr();
    egress.queue_reliable(reliable).unwrap();
    egress.queue_unreliable(MESSAGE.to_vec());
    let queued = [destroy(1)];
    let mut sequence = 0;
    let mut attempted = None;
    let failed = egress.send_frame(
        frame_records(&queued, &[]),
        |_| 77,
        |_| panic!("a failed send must not associate records"),
        |payload| {
            attempted = Some(payload);
            sequence += 1;
            Err(std::io::ErrorKind::WouldBlock)
        },
    );
    assert!(matches!(
        failed,
        Err(SendError::Transport(std::io::ErrorKind::WouldBlock))
    ));
    assert!(egress.in_flight.is_empty());
    assert_eq!(egress.reliable[0].1.as_ptr(), original);
    assert_eq!(egress.unreliable.len(), 1);

    let sent = egress
        .send_frame(
            frame_records(&queued, &[]),
            |_| 77,
            |source| source,
            |payload| {
                assert_eq!(Some(payload), attempted);
                Ok::<_, std::io::ErrorKind>(sequence)
            },
        )
        .unwrap();
    assert_eq!(sent.sequence, 1);
    assert_eq!(sent.records, [RecordSource::Queued(0)]);
    assert!(egress.reliable.is_empty());
    assert!(egress.unreliable.is_empty());
    assert_eq!(egress.in_flight[0].reliable[0].1.as_ptr(), original);
}

#[test]
fn loss_requeues_reliable_bytes_once_and_ownership_waits_for_the_prefix() {
    #[derive(Debug, Eq, PartialEq)]
    struct Token(u32);

    let mut egress = GameEgress::default();
    let first = MESSAGE.to_vec();
    let original = first.as_ptr();
    assert_eq!(egress.queue_reliable(first).unwrap(), 0);
    egress.queue_unreliable(MESSAGE.to_vec());
    egress
        .send_frame(
            frame_records(&[destroy(1)], &[]),
            |_| 0,
            |_| Token(1),
            |_| Ok::<_, ()>(10),
        )
        .unwrap();
    let ownership = egress.queue_reliable(MESSAGE.to_vec()).unwrap();
    assert_eq!(ownership, 1);
    egress
        .send_frame(
            frame_records(&[destroy(2)], &[]),
            |_| 0,
            |_| Token(2),
            |_| Ok::<_, ()>(11),
        )
        .unwrap();
    assert_eq!(egress.queue_reliable(MESSAGE.to_vec()).unwrap(), 2);

    let report = egress.acknowledge(12, 1);
    assert_eq!(report.requeued_reliable, 1);
    assert_eq!(
        report.packets,
        [
            PacketDelivery {
                sequence: 10,
                arrived: false,
                records: vec![Token(1)]
            },
            PacketDelivery {
                sequence: 11,
                arrived: true,
                records: vec![Token(2)]
            },
        ]
    );
    assert_eq!(egress.oldest_pending_reliable(), 0);
    assert!(!egress.delivered_through(ownership));
    assert_eq!(egress.reliable[0].1.as_ptr(), original);
    assert!(egress.unreliable.is_empty());
    assert!(egress.acknowledge(12, 1).packets.is_empty());
    assert_eq!(egress.reliable.len(), 2);

    let schemas = Schemas::from_json(r#"{"A7FBBD0C":{"21602":[]}}"#).unwrap();
    egress
        .send_frame(
            frame_records(&[], &[]),
            |_| 0,
            |_| unreachable!(),
            |payload| {
                let mut reader = BitReader::new(&payload);
                assert_eq!(reader.var_a().unwrap(), 5000);
                let channel = read_channel(&mut reader, &schemas).unwrap();
                assert!(channel.unreliable.is_empty());
                assert_eq!(
                    channel
                        .reliable
                        .iter()
                        .map(|message| message.sequence)
                        .collect::<Vec<_>>(),
                    [0, 2]
                );
                Ok::<_, ()>(12)
            },
        )
        .unwrap();
    assert!(!egress.delivered_through(ownership));
    assert!(egress.acknowledge(12, 1).packets.is_empty());
    assert_eq!(egress.acknowledge(13, 1).packets[0].sequence, 12);
    assert!(egress.delivered_through(ownership));
    assert_eq!(egress.oldest_pending_reliable(), 3);
}

#[test]
fn reports_acknowledgements_in_send_order_across_packet_rollover() {
    let mut egress = GameEgress::default();
    for sequence in [u32::MAX - 1, u32::MAX, 0] {
        egress
            .send_frame(
                frame_records(&[destroy(1)], &[]),
                |_| 0,
                |_| sequence,
                |_| Ok::<_, ()>(sequence),
            )
            .unwrap();
    }
    assert!(egress.acknowledge(u32::MAX - 1, 1).packets.is_empty());
    let report = egress.acknowledge(1, 0b101);
    assert_eq!(report.requeued_reliable, 0);
    assert_eq!(
        report.packets,
        [
            PacketDelivery {
                sequence: u32::MAX - 1,
                arrived: true,
                records: vec![u32::MAX - 1]
            },
            PacketDelivery {
                sequence: u32::MAX,
                arrived: false,
                records: vec![u32::MAX]
            },
            PacketDelivery {
                sequence: 0,
                arrived: true,
                records: vec![0]
            },
        ]
    );
    assert!(egress.acknowledge(1, 0b101).packets.is_empty());
}

#[test]
fn an_unsendable_oldest_reliable_never_reaches_transport() {
    let mut egress = GameEgress::<()>::default();
    egress.queue_reliable(vec![0; MAX_PAYLOAD]).unwrap();
    egress.queue_unreliable(MESSAGE.to_vec());
    let result = egress.send_frame(
        frame_records(&[destroy(1)], &[]),
        |_| 0,
        |_| panic!("invalid frame cannot associate records"),
        |_| -> Result<u32, ()> { panic!("invalid frame cannot reach transport") },
    );
    assert!(matches!(
        result,
        Err(SendError::Select(WorldError::ReliableMessageDoesNotFit))
    ));
    assert_eq!(egress.reliable.len(), 1);
    assert_eq!(egress.unreliable.len(), 1);
    assert!(egress.in_flight.is_empty());
}

#[test]
fn reliable_only_fits_the_message_prefix_and_leaves_other_candidates() {
    let mut egress = GameEgress::<RecordSource>::default();
    for _ in 0..2 {
        egress.queue_reliable(vec![0; MAX_PAYLOAD * 3 / 4]).unwrap();
    }
    egress.queue_unreliable(MESSAGE.to_vec());
    let queued = [destroy(1)];
    let records = FrameRecords {
        reliable_only: true,
        ..frame_records(&queued, &[])
    };
    let send = |sequence| {
        move |payload: Vec<u8>| {
            assert!(payload.len() <= MAX_PAYLOAD);
            Ok::<_, ()>(sequence)
        }
    };
    let sent = egress
        .send_frame(records, |_| 0, |_| unreachable!(), send(1))
        .unwrap();
    assert!(sent.records.is_empty());
    assert_eq!(egress.reliable.len(), 1);
    assert_eq!(egress.unreliable.len(), 1);
    assert_eq!(egress.in_flight[0].reliable[0].0, 0);
    let sent = egress
        .send_frame(frame_records(&queued, &[]), |_| 0, |source| source, send(2))
        .unwrap();
    assert_eq!(sent.records, [RecordSource::Queued(0)]);
    assert_eq!(egress.in_flight[1].reliable[0].0, 1);
    assert!(egress.unreliable.is_empty());
}

#[test]
fn records_actual_correction_selection_but_only_tracks_acknowledgeable_records() {
    let mut egress = GameEgress::default();
    let mut queued: Vec<_> = (1..MAX_ENTITY_RECORDS as u32)
        .map(|id| EntityUpdate {
            entity: 0x8000_0000 | id,
            ..EntityUpdate::default()
        })
        .collect();
    queued[0] = destroy(1);
    let movements = [
        EntityUpdate {
            entity: 0x8000_0080,
            movement: Some(Movement::default()),
            ..EntityUpdate::default()
        },
        EntityUpdate {
            entity: 0xa000_0100,
            movement: Some(Movement::default()),
            ..EntityUpdate::default()
        },
    ];
    let sent = egress
        .send_frame(
            FrameRecords {
                owner_correction: true,
                ..frame_records(&queued, &movements)
            },
            |records| {
                u32::from(
                    records
                        .last()
                        .is_some_and(|record| record.entity == movements[1].entity),
                )
            },
            |source| source,
            |payload| {
                let mut reader = BitReader::new(&payload);
                assert_eq!(reader.var_a().unwrap(), 5000);
                assert!(!reader.bit().unwrap());
                assert_eq!(reader.var_a().unwrap(), 1);
                Ok::<_, ()>(7)
            },
        )
        .unwrap();
    assert_eq!(sent.records.len(), MAX_ENTITY_RECORDS);
    assert_eq!(
        sent.records[..queued.len()],
        (0..queued.len())
            .map(RecordSource::Queued)
            .collect::<Vec<_>>()
    );
    assert_eq!(sent.records[queued.len()], RecordSource::Movement(1));
    let report = egress.acknowledge(8, 1);
    assert_eq!(report.packets[0].records, [RecordSource::Queued(0)]);
}

#[test]
fn reliable_ordinals_cannot_wrap() {
    let mut egress = GameEgress::<()> {
        next_reliable: u64::from(u32::MAX),
        ..GameEgress::default()
    };
    assert_eq!(egress.queue_reliable(MESSAGE.to_vec()), Ok(u32::MAX));
    assert_eq!(
        egress.queue_reliable(MESSAGE.to_vec()),
        Err(OrdinalExhausted)
    );
    assert_eq!(egress.reliable.len(), 1);
}

fn correction_pose(frame: u32) -> Movement {
    Movement {
        position: [f64::from(frame) / 1024.0, 1.0, -2.0],
        frame: Some(frame),
        ..Movement::default()
    }
}

#[test]
fn selected_send_advances_ack_only_after_budget_and_transport_succeed() {
    let mut history = CorrectionHistory::new(0xa000_0101, 1);
    history.record(correction_pose(100));
    assert!(history.sent(&history.candidate(100, true).unwrap()));
    for frame in 101..114 {
        history.record(correction_pose(frame));
    }
    let own = history.candidate(107, true).unwrap();
    let movement = [own.update()];
    let queued: Vec<_> = (0..MAX_ENTITY_RECORDS)
        .map(|index| EntityUpdate {
            entity: 0x8000_0001 + index as u32,
            operation: Some(Operation::Destroy),
            ..EntityUpdate::default()
        })
        .collect();
    fn candidates<'a>(
        queued: &'a [EntityUpdate],
        movements: &'a [EntityUpdate],
    ) -> FrameRecords<'a> {
        FrameRecords {
            tick: 107,
            queued,
            movements,
            owner_correction: true,
            reliable_only: false,
        }
    }
    let ack = |records: &[&EntityUpdate]| {
        let selected = records
            .iter()
            .any(|record| std::ptr::eq(*record, &movement[0]));
        history.command_ack(107, Some(113), selected.then_some(&own))
    };
    let decode_ack = |bytes: Vec<u8>| {
        let mut reader = BitReader::new(&bytes);
        assert_eq!(reader.var_a().unwrap(), 107);
        assert!(!reader.bit().unwrap()); // empty JAM channel
        reader.var_a().unwrap()
    };
    let mut egress = GameEgress::default();
    let crowded = egress
        .send_frame(
            candidates(&queued, &movement),
            ack,
            |_| (),
            |bytes| {
                assert_eq!(decode_ack(bytes), 102);
                Ok::<_, ()>(0)
            },
        )
        .unwrap();
    assert_eq!(crowded.records.len(), MAX_ENTITY_RECORDS);
    assert!(!crowded.records.contains(&RecordSource::Movement(0)));
    assert_eq!(history.corrected(), Some(100));

    let failed = egress.send_frame(
        candidates(&[], &movement),
        ack,
        |_| (),
        |bytes| {
            assert_eq!(decode_ack(bytes), 109);
            Err(())
        },
    );
    assert!(matches!(failed, Err(SendError::Transport(()))));
    assert_eq!(history.corrected(), Some(100));
    let sent = egress
        .send_frame(
            candidates(&[], &movement),
            ack,
            |_| (),
            |bytes| {
                assert_eq!(decode_ack(bytes), 109);
                Ok::<_, ()>(2)
            },
        )
        .unwrap();
    assert_eq!(sent.records, [RecordSource::Movement(0)]);
    assert!(history.sent(&own));
    assert_eq!(history.corrected(), Some(107));
    assert_eq!(history.command_ack(108, Some(113), None), 109);
}
