use super::*;

const CONTROLLER: u32 = 0xa000_0100;
const BODY: u32 = 0xa000_0101;

fn payload() -> BitWriter {
    let mut out = BitWriter::new();
    out.bits(0x15, 5).unwrap();
    out
}

fn check_chunk(chunk: &ScriptChunk, last: u32, resend: bool, bits: usize, hex: &str) {
    assert_eq!((chunk.last, chunk.resend), (last, resend));
    let chunk = chunk.update.chunk.as_ref().unwrap();
    let encoded: String = chunk
        .as_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    assert_eq!((chunk.bit_len(), encoded.as_str()), (bits, hex));
}

#[test]
fn controller_data_precedes_ack_only_chunks_and_ranges_match_python() {
    let mut stream = Stream::new(CONTROLLER);
    let at = Duration::from_millis;
    assert!(stream.owner_ack(at(0), Some(100)).unwrap().is_none());
    let first = stream
        .full_owner::<StreamError>(100, |cmfd| {
            assert_eq!(cmfd, 100);
            Ok(payload())
        })
        .unwrap();
    check_chunk(&first, 1, true, 41, "000004505001");
    assert_eq!(first.update.entity, CONTROLLER);
    let second = stream
        .full_owner::<StreamError>(100, |cmfd| {
            assert_eq!(cmfd, 101);
            Ok(payload())
        })
        .unwrap();
    check_chunk(&second, 2, true, 41, "000008505001");
    assert!(stream.arrived(first.last));
    assert!(stream.data_in_flight());
    assert!(stream.owner_ack(at(0), Some(104)).unwrap().is_none());
    assert!(stream.arrived(second.last));
    assert!(!stream.data_in_flight());
    assert!(stream.owner_ack(at(99), Some(105)).unwrap().is_none());
    let ack = stream.owner_ack(at(100), Some(105)).unwrap().unwrap();
    check_chunk(&ack, 3, false, 67, "060000f021690e0000");
    assert!(stream.owner_ack(at(199), Some(107)).unwrap().is_none());
    let cumulative = stream.owner_ack(at(200), Some(107)).unwrap().unwrap();
    check_chunk(&cumulative, 4, false, 67, "060004f0216b0e0000");
    assert!(stream.arrived(cumulative.last));
    assert!(!stream.arrived(ack.last));
    assert!(!stream.arrived(9));
    assert_eq!(stream.delivered(), 4);
    let later = stream.owner_ack(at(300), Some(108)).unwrap().unwrap();
    check_chunk(&later, 5, false, 67, "0a0000f0216c0e0000");
}

#[test]
fn writers_and_lease_boundaries_leave_the_existing_stream_usable() {
    let mut stream = Stream::new(CONTROLLER);
    assert_eq!(
        stream.full_owner(42, |_| Err(StreamError::Wire(WireError::InvalidValue))),
        Err(StreamError::Wire(WireError::InvalidValue))
    );
    assert_eq!(stream.command_frame(), 0);
    let full = stream
        .full_owner::<StreamError>(MAX_LEASE_FRAME, |_| Ok(payload()))
        .unwrap();
    assert_eq!(full.last, 1);
    assert!(stream.arrived(full.last));
    assert_eq!(
        stream.owner_ack(Duration::ZERO, Some(MAX_LEASE_FRAME + 1)),
        Err(StreamError::CommandLimit(MAX_LEASE_FRAME + 1))
    );
    assert_eq!(
        stream.owner_ack(Duration::ZERO, Some(1)),
        Err(StreamError::CommandWrap {
            previous: MAX_LEASE_FRAME,
            incoming: 1
        })
    );
    assert_eq!(
        stream.full_owner::<StreamError>(MAX_LEASE_FRAME, |_| unreachable!()),
        Err(StreamError::CommandLimit(MAX_LEASE_FRAME + 1))
    );
    assert_eq!(stream.command_frame(), MAX_LEASE_FRAME);
    assert_eq!(stream.delivered(), 1);
    assert_eq!(stream.full(&payload()).unwrap().last, 2);
    stream.last = u32::MAX;
    assert_eq!(stream.full(&payload()), Err(StreamError::ChunkExhausted));

    let data = payload();
    let mut body = BodyStream::new(BODY);
    body.create_arrived();
    let full = body.full(&data, 7).unwrap();
    body.arrived(full.last);
    let initial = body
        .delta(
            &data,
            8,
            BTreeSet::new(),
            DeltaKind::InitialOwner(MAX_LEASE_FRAME),
        )
        .unwrap();
    body.sent(initial.last);
    body.arrived(initial.last);
    assert_eq!(
        body.owner_kind(1, false, false),
        Err(StreamError::CommandWrap {
            previous: MAX_LEASE_FRAME,
            incoming: 1,
        })
    );
    assert_eq!(
        body.delta(
            &data,
            9,
            BTreeSet::new(),
            DeltaKind::Correction(MAX_LEASE_FRAME + 1),
        ),
        Err(StreamError::CommandLimit(MAX_LEASE_FRAME + 1))
    );
    assert_eq!(body.command_frame(), MAX_LEASE_FRAME);
    assert_eq!(body.base(), Some((initial.last, &8)));
}

#[test]
fn body_full_waits_for_create_and_the_first_owner_delta_retries_unchanged() {
    let mut stream = BodyStream::new(BODY);
    let data = payload();
    let full = stream.full(&data, 100).unwrap();
    check_chunk(&full, 1, true, 41, "000004505001");
    assert!(!stream.ready_to_send(full.last));
    assert!(!stream.arrived(full.last).advanced);
    assert_eq!(stream.owner_kind(101, false, false).unwrap(), None);
    assert_eq!(
        stream.delta(&data, 101, BTreeSet::new(), DeltaKind::InitialOwner(101)),
        Err(StreamError::BaselinePending)
    );
    stream.create_arrived();
    assert!(stream.ready_to_send(full.last));
    assert!(stream.sent(full.last).is_none());
    stream.full_sent(99);
    assert!(stream.lost(full.last).retry);
    assert!(stream.sent(full.last).is_none());
    stream.full_sent(100);
    assert!(stream.arrived(full.last).advanced);
    assert_eq!(stream.base(), Some((1, &100)));
    // The client took 100, the frame of the datagram that brought the full
    // chunk, as the body's command frame: an owner frame at 100 would be
    // stale there.
    assert_eq!(stream.owner_kind(100, false, false).unwrap(), None);
    let change = ChangeKey::State(11, 10);
    let initial_kind = stream.owner_kind(101, false, false).unwrap().unwrap();
    assert_eq!(initial_kind, DeltaKind::InitialOwner(101));
    let original = stream
        .delta(&data, 101, BTreeSet::from([change]), initial_kind)
        .unwrap();
    check_chunk(&original, 2, false, 41, "040000505001");
    assert_eq!(stream.owner_kind(101, false, true).unwrap(), None);
    assert_eq!(
        stream.owner_kind(101, true, true).unwrap(),
        Some(initial_kind)
    );
    let replacement = stream
        .delta(&data, 111, BTreeSet::from([change]), initial_kind)
        .unwrap();
    assert!(stream.discard_unsent(original.last));
    let sent = stream.sent(replacement.last).unwrap();
    assert_eq!((sent.command_frame, *sent.snapshot), (101, 111));
    assert!(sent.changes.contains(&change));
    assert!(!stream.discard_unsent(replacement.last));
    assert!(stream.lost(replacement.last).retry);
    assert_eq!(stream.latest_snapshot(), Some(&111));
    assert!(stream.sent(replacement.last).is_none());
    assert_eq!(stream.owner_kind(117, true, true).unwrap(), None);
    let delivered = stream.arrived(replacement.last);
    assert_eq!(
        delivered,
        BodyDelivery {
            retry: false,
            advanced: true,
            owner_report: Some(true)
        }
    );
    assert_eq!(stream.base(), Some((replacement.last, &111)));
    assert_eq!(stream.owner_kind(116, false, false).unwrap(), None);
    assert_eq!(
        stream.owner_kind(117, false, false).unwrap(),
        Some(DeltaKind::Correction(117))
    );
}

#[test]
fn older_positive_ack_keeps_newer_lost_changes_and_exact_cumulative_ranges() {
    let data = payload();
    let mut stream = BodyStream::new(BODY);
    stream.create_arrived();
    let full = stream.full(&data, 100).unwrap();
    stream.arrived(full.last);
    let initial = stream
        .delta(&data, 101, BTreeSet::new(), DeltaKind::InitialOwner(101))
        .unwrap();
    stream.sent(initial.last);
    stream.arrived(initial.last);

    let old_key = ChangeKey::Variable(11, 53);
    let shared = ChangeKey::State(11, 10);
    let newer_key = ChangeKey::State(11, 16);
    let volley = ChangeKey::VolleyIntroduction(11, 16);
    let old = stream
        .delta(
            &data,
            117,
            BTreeSet::from([old_key, shared]),
            DeltaKind::Correction(117),
        )
        .unwrap();
    check_chunk(&old, 3, false, 41, "060000505001");
    let received = stream
        .delta(
            &data,
            118,
            BTreeSet::from([old_key, shared]),
            DeltaKind::Correction(118),
        )
        .unwrap();
    check_chunk(&received, 4, false, 41, "060004505001");
    let newer = stream
        .delta(
            &data,
            119,
            BTreeSet::from([shared, newer_key, volley]),
            DeltaKind::Correction(119),
        )
        .unwrap();
    for update in [&old, &received, &newer] {
        assert!(stream.sent(update.last).is_some());
    }
    assert_eq!(stream.lost(old.last).owner_report, Some(false));
    assert_eq!(stream.lost(newer.last).owner_report, Some(false));
    assert_eq!(stream.arrived(received.last).owner_report, Some(true));
    assert!(!stream.arrived(old.last).advanced);
    assert_eq!(stream.base(), Some((received.last, &118)));
    assert_eq!(stream.latest_snapshot(), Some(&119));
    assert_eq!(stream.pending().count(), 0);
    assert_eq!(stream.touched(), BTreeSet::from([shared, newer_key]));

    let recovered = stream
        .delta(&data, 120, stream.touched(), DeltaKind::Correction(120))
        .unwrap();
    stream.sent(recovered.last);
    assert!(stream.arrived(recovered.last).advanced);
    assert!(stream.touched().is_empty());
    assert_eq!(stream.latest_snapshot(), None);
}

#[test]
fn a_superseded_owner_report_still_resolves_its_own_transport_outcome() {
    let data = payload();
    let mut stream = BodyStream::new(BODY);
    stream.create_arrived();
    let full = stream.full(&data, 100).unwrap();
    stream.arrived(full.last);
    let initial = stream
        .delta(&data, 101, BTreeSet::new(), DeltaKind::InitialOwner(101))
        .unwrap();
    stream.sent(initial.last);
    stream.arrived(initial.last);
    let early = stream
        .delta(&data, 117, BTreeSet::new(), DeltaKind::Correction(117))
        .unwrap();
    let late = stream
        .delta(&data, 118, BTreeSet::new(), DeltaKind::Correction(118))
        .unwrap();
    stream.sent(early.last);
    stream.sent(late.last);
    assert!(stream.arrived(late.last).advanced);
    assert_eq!(stream.pending().count(), 0);
    let old = stream.lost(early.last);
    assert_eq!(
        old,
        BodyDelivery {
            retry: false,
            advanced: false,
            owner_report: Some(false)
        }
    );
    assert_eq!(stream.lost(early.last), BodyDelivery::default());
    assert_eq!(stream.base(), Some((late.last, &118)));
}

#[test]
fn observer_loss_keeps_the_pending_projection_until_superseded() {
    let data = payload();
    let mut stream = BodyStream::new(BODY);
    stream.create_arrived();
    let full = stream.full(&data, 100).unwrap();
    stream.arrived(full.last);
    let change = ChangeKey::Instance(11);
    let observer = stream
        .delta(
            &data,
            101,
            BTreeSet::from([change]),
            DeltaKind::Observer { resend: false },
        )
        .unwrap();
    assert!(stream.sent(observer.last).is_none());
    assert_eq!(stream.lost(observer.last), BodyDelivery::default());
    assert_eq!(stream.latest_snapshot(), Some(&101));
    assert_eq!(stream.touched(), BTreeSet::from([change]));
    let durable = stream
        .delta(
            &data,
            102,
            BTreeSet::from([change]),
            DeltaKind::Observer { resend: true },
        )
        .unwrap();
    assert!(stream.lost(durable.last).retry);
    stream.retire();
    assert!(!stream.retry(durable.last));
    assert!(!stream.ready_to_send(durable.last));
    assert!(!stream.arrived(durable.last).advanced);
    assert_eq!(stream.full(&data, 103), Err(StreamError::BodyRetired));
}
