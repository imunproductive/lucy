//! A player's communication wheel choice, from the client to his body.
//!
//! The client sends a wheel choice as ClientOutInstance 21608 (A7FBBD0C,
//! built by event 2603A40 through 611C10): `+0x78` the action (`0A9`),
//! `+0x80` a bool, `+0x84` an entity (a selected marker, zero by default).
//! The server queues game message `02A3` on the sender's current body with
//! the action in `08FC`, the bool in `03E1` and, for ACKNOWLEDGE only, a
//! nonzero marker in `0644` (leafall903/ORPHEUS (MIT),
//! `Match.communication_action` in `match.py`; `docs/COMMUNICATION_WHEEL.txt`).
//! Other actions ignore the entity.
//!
//! The body's own graphs do the rest: `05AD` and `0686` run the callouts and
//! the voice lines, and every client gets them in the body's frames (the
//! callout actions in the frames' remote messages, the voice line pulse as a
//! state). Sprays are refused here: their placement state (`STU_861D9015`,
//! an eye ray against collision) is not run by the server.

use lucy_script::{
    wheel,
    wire::{Value, Variables},
};
use lucy_wire::{jam::codec::Value as Field, messages::Message};

/// ClientOutInstance 21608.
pub const COMMUNICATION_ACTION: u32 = 21608;

/// The game message and parameters a 21608 gives the sender's body; none for
/// a message that is no wheel choice the server runs.
pub fn selection(message: &Message) -> Option<(u64, Variables)> {
    if message.id != COMMUNICATION_ACTION {
        return None;
    }
    let fields = &message.fields;
    let Field::Unsigned(action) = *fields.get(&0x78)? else {
        return None;
    };
    if !wheel::is_action(action) || wheel::is_spray(action) {
        return None;
    }
    let flag = match fields.get(&0x80) {
        Some(Field::Bool(flag)) => *flag,
        Some(Field::Unsigned(flag)) => *flag != 0,
        _ => false,
    };
    let mut parameters = Variables::new();
    parameters.insert(wheel::ACTION_PARAMETER, Value::Asset(action));
    parameters.insert(wheel::FLAG_PARAMETER, Value::Bool(flag));
    if wheel::action_index(action) == Some(wheel::ACKNOWLEDGE) {
        let marker = match fields.get(&0x84) {
            Some(Field::Struct(entity)) => match entity.get(&0) {
                Some(Field::Unsigned(entity)) => u32::try_from(*entity).unwrap_or(0),
                _ => 0,
            },
            _ => 0,
        };
        if marker != 0 {
            parameters.insert(wheel::MARKER_PARAMETER, Value::Entity(marker));
        }
    }
    Some((wheel::SELECTION_MESSAGE, parameters))
}

#[cfg(test)]
mod tests {
    use super::*;
    use lucy_wire::jam::codec::Fields;

    fn choice(action: u64, flag: bool, marker: u64) -> Message {
        let mut entity = Fields::new();
        entity.insert(0, Field::Unsigned(marker));
        let mut fields = Fields::new();
        fields.insert(0x78, Field::Unsigned(action));
        fields.insert(0x80, Field::Bool(flag));
        fields.insert(0x84, Field::Struct(entity));
        Message {
            id: COMMUNICATION_ACTION,
            fields,
        }
    }

    #[test]
    fn a_wheel_choice_becomes_the_bodys_selection_message() {
        let (message, parameters) = selection(&choice(0x0a90_0000_0000_016c, true, 0)).unwrap();
        assert_eq!(message, 0x0240_0000_0000_02a3);
        assert_eq!(
            parameters.into_iter().collect::<Vec<_>>(),
            [
                (0x08fc, Value::Asset(0x0a90_0000_0000_016c)),
                (0x03e1, Value::Bool(true)),
            ]
        );
        // Only ACKNOWLEDGE reads the marker, and only a nonzero one.
        let (_, parameters) =
            selection(&choice(0x0a90_0000_0000_0170, false, 0xa000_0105)).unwrap();
        assert_eq!(parameters[&0x0644], Value::Entity(0xa000_0105));
        let (_, parameters) =
            selection(&choice(0x0a90_0000_0000_016b, false, 0xa000_0105)).unwrap();
        assert!(!parameters.contains_key(&0x0644));
        let (_, parameters) = selection(&choice(0x0a90_0000_0000_0170, false, 0)).unwrap();
        assert!(!parameters.contains_key(&0x0644));
    }

    #[test]
    fn other_assets_and_sprays_are_refused() {
        assert!(selection(&choice(0x0a90_0000_0000_01f5, false, 0)).is_none());
        assert!(selection(&choice(0x0240_0000_0000_016c, false, 0)).is_none());
        assert!(selection(&choice(0x0a90_0000_0000_0173, false, 0)).is_none());
    }
}
