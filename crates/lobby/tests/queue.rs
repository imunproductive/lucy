//! The queue messages as the client decodes them; ports the message test of
//! `tests/test_queue.py` (the handler tests are in the server's matchmaker).

use orpheus_lobby::queue::{ENTER_QUEUE, MATCHMAKE, QUEUE, queue_joined};
use orpheus_wire::jam::codec::{
    Fields, Schemas,
    Value::{self, Array, Signed, Struct, Unsigned},
};

/// A 44100 as the client sends it for the card (`tests/test_queue.py`
/// `request`; competitive role queue is card 1B3, its open queue 1B4).
fn request(card: u64) -> Fields {
    let mode = Fields::from([
        (0x0, Unsigned(0x0630_0000_0000_0000 | card)),
        (0x8, Unsigned(0)),
        (0x10, Unsigned(0)),
        (0x18, Unsigned(0)),
    ]);
    let key = Fields::from([
        (0x0, Struct(mode)),
        (0x20, Unsigned(0)),
        (0x24, Unsigned(15_959_616)),
        (0x28, Unsigned(16_025_156)),
    ]);
    Fields::from([(0x78, Struct(key))])
}

/// A value with its integers as one kind: Python has no signedness, and the
/// schemas decode some fields as signed.
fn plain(value: &Value) -> Value {
    match value {
        &Signed(number) if number >= 0 => Unsigned(number as u64),
        Struct(fields) => Struct(
            fields
                .iter()
                .map(|(&offset, value)| (offset, plain(value)))
                .collect(),
        ),
        Array(values) => Array(values.iter().map(plain).collect()),
        other => other.clone(),
    }
}

fn round_trip(schemas: &Schemas, group: u32, id: u32, fields: &Fields) -> Fields {
    let body = schemas.encode(group, id, fields).unwrap();
    schemas.decode(group, id, &body, true).unwrap()
}

/// `tests/test_queue.py::QueueTests::test_the_entry_carries_the_request_key`
#[test]
fn the_entry_carries_the_request_key() {
    let schemas = Schemas::lobby();
    let open_queue = request(0x1b4);
    let decoded = round_trip(&schemas, MATCHMAKE, ENTER_QUEUE, &open_queue);
    let Struct(key) = &decoded[&0x78] else {
        panic!("44100 without a key: {decoded:?}")
    };
    let (group, id, joined) = queue_joined(key);
    assert_eq!((group, id), (QUEUE, 44201));
    let decoded = round_trip(&schemas, group, id, &joined);
    let Struct(entry) = &decoded[&0x78] else {
        panic!("44201 without an entry: {decoded:?}")
    };
    assert_eq!(plain(&entry[&0x0]), plain(&open_queue[&0x78]));
}
