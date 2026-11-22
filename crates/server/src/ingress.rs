//! Incoming message ordering, pong replies, and map entry milestones.

use std::{collections::HashMap, error::Error, fmt, time::Duration};

use lucy_core::bits::BitReader;
use lucy_wire::commands::{InputError, InputRecord, read_input};
use lucy_wire::jam::codec::{Fields, Schemas, Value};
use lucy_wire::messages::{ChannelError, Message, read_channel};

const PONG: u32 = 20306;
const PING_INDEX: u32 = 0x78;
const PING_STAMPS: u32 = 0x80;
/// A client's reliable messages wait for a lost one before them at most
/// this many at a time ([`IngressError::Backlog`]): the client sends the
/// lost one again long before so many others follow it.
const MAX_PARKED: usize = 256;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MessageKind {
    Ping,
    MapLoaded,
    GameMessage,
    /// A communication wheel choice (`lucy_sim::wheel`).
    CommunicationAction,
    /// The client's hero settings (`lucy_content::hero_settings`).
    HeroSettings,
    Other,
}

impl From<u32> for MessageKind {
    fn from(id: u32) -> Self {
        match id {
            21610 => Self::Ping,
            21601 | 21602 => Self::MapLoaded,
            21616 => Self::GameMessage,
            lucy_sim::wheel::COMMUNICATION_ACTION => Self::CommunicationAction,
            lucy_content::hero_settings::HERO_SETTINGS => Self::HeroSettings,
            _ => Self::Other,
        }
    }
}

#[derive(Debug)]
pub enum IngressError {
    Channel(ChannelError),
    Commands(InputError),
    /// More reliable messages wait for a lost one than [`MAX_PARKED`]: the
    /// client is broken, and its player leaves.
    Backlog,
}

impl fmt::Display for IngressError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Channel(error) => write!(f, "bad game channel: {error}"),
            Self::Commands(error) => write!(f, "bad game input: {error}"),
            Self::Backlog => write!(f, "over {MAX_PARKED} game messages wait for a lost one"),
        }
    }
}

impl Error for IngressError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Channel(error) => Some(error),
            Self::Commands(error) => Some(error),
            Self::Backlog => None,
        }
    }
}

#[derive(Debug, Default)]
pub struct GameIngress {
    // Reliable ordinals increase throughout the session, unlike UDP packet numbers.
    expected_reliable: u64,
    parked: HashMap<u32, Message>,
}

impl GameIngress {
    pub fn new() -> Self {
        Self::default()
    }

    /// Decode a complete channel before changing its reliable state. Dispatch
    /// newly available messages before parsing commands, since a game message
    /// can change which player body should receive those commands.
    pub fn receive(
        &mut self,
        schemas: &Schemas,
        payload: &[u8],
        mut dispatch: impl FnMut(Message),
    ) -> Result<InputRecord, IngressError> {
        let mut reader = BitReader::new(payload);
        let channel = read_channel(&mut reader, schemas).map_err(IngressError::Channel)?;
        for reliable in channel.reliable {
            if u64::from(reliable.sequence) < self.expected_reliable {
                continue;
            }
            if self.parked.len() == MAX_PARKED
                && u64::from(reliable.sequence) != self.expected_reliable
                && !self.parked.contains_key(&reliable.sequence)
            {
                return Err(IngressError::Backlog);
            }
            self.parked.insert(reliable.sequence, reliable.message);
            while let Ok(expected) = u32::try_from(self.expected_reliable) {
                let Some(message) = self.parked.remove(&expected) else {
                    break;
                };
                self.expected_reliable += 1;
                dispatch(message);
            }
        }
        for message in channel.unreliable {
            dispatch(message);
        }
        read_input(&mut reader).map_err(IngressError::Commands)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InvalidPingField(pub u32);

impl fmt::Display for InvalidPingField {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid game ping field +0x{:X}", self.0)
    }
}

impl Error for InvalidPingField {}

/// A pong (20306) for a client's ping (21610), waiting for the frame that
/// carries it: the ping's index, its two stamps and the server's first
/// two. The third goes in as the frame leaves ([`Pong::sent`]).
///
/// The stamps (client research, 1.74; refs/reports/Latency.md "Pong"):
/// the client makes 21610 with `[made, 0]` (`0x7FF7896EC520`, every
/// 0.25 s) and its channel writes the second as the datagram leaves (the
/// message writer `0x7FF789B7ADE0` sets 21610's stamp 1, and 20306's stamp
/// 4, to the send time). The same shared code on the server appended the
/// receive time to 21610 (the reader `0x7FF789B7A480`), the server's
/// handler its own time, and the writer the send time: `[c made, c sent,
/// s received, s made, s sent]`. The client's handler (`0x7FF7896E89E0`)
/// appends its receive time (s5) and takes `(s5 − s1) − (s4 − s2)` (the
/// subtraction only when it is the smaller) as PING, the round trip less
/// the server's hold: so "sent" is when the datagram leaves, not when the
/// ping was read. It puts `s3 × 10⁻⁹` into a ring (component 105 +0x36D8)
/// that no code reads, so the made stamp has no effect; it is the
/// handler's time by the symmetry of the client's own stamps. With two
/// server stamps (the Python reference's) the client took its own receive
/// time for s4, so `s4 − s2` mixed its clock with ours (wall time since
/// 1970 then): a PING of 1.8·10⁹ s (Latency.md §6).
///
/// The server's stamps are the service's monotonic clock in nanoseconds;
/// the client only subtracts them from each other.
#[derive(Clone, Debug, PartialEq)]
pub struct Pong {
    index: u64,
    stamps: Vec<Value>,
}

impl Pong {
    /// The pong for `ping`, read at `received` (its datagram's arrival) and
    /// answered at `made`.
    pub fn new(
        ping: &Fields,
        received: Duration,
        made: Duration,
    ) -> Result<Self, InvalidPingField> {
        let index = match ping.get(&PING_INDEX) {
            None => 0,
            Some(Value::Unsigned(index)) if u32::try_from(*index).is_ok() => *index,
            _ => return Err(InvalidPingField(PING_INDEX)),
        };
        let mut stamps = match ping.get(&PING_STAMPS) {
            None => Vec::new(),
            Some(Value::Array(stamps))
                if stamps
                    .iter()
                    .all(|stamp| matches!(stamp, Value::Unsigned(_))) =>
            {
                stamps.clone()
            }
            _ => return Err(InvalidPingField(PING_STAMPS)),
        };
        stamps.extend([
            Value::Unsigned(nanoseconds(received)),
            Value::Unsigned(nanoseconds(made)),
        ]);
        Ok(Self { index, stamps })
    }

    /// The message for the frame that leaves at `sent`.
    pub fn sent(mut self, sent: Duration) -> Message {
        self.stamps.push(Value::Unsigned(nanoseconds(sent)));
        Message {
            id: PONG,
            fields: Fields::from([
                (PING_INDEX, Value::Unsigned(self.index)),
                (PING_STAMPS, Value::Array(self.stamps)),
            ]),
        }
    }
}

fn nanoseconds(time: Duration) -> u64 {
    u64::try_from(time.as_nanos()).unwrap_or(u64::MAX)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MapReadyStep {
    Ownership,
    Game,
    World,
    Controller,
}

impl MapReadyStep {
    const fn delay(self) -> Duration {
        Duration::from_millis(match self {
            Self::Ownership => 300,
            Self::Game => 400,
            Self::World => 600,
            Self::Controller => 700,
        })
    }

    const fn next(self) -> Option<Self> {
        match self {
            Self::Ownership => Some(Self::Game),
            Self::Game => Some(Self::World),
            Self::World => Some(Self::Controller),
            Self::Controller => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct WorldReadiness {
    /// The client acknowledged the entire reliable prefix through ownership.
    pub ownership_delivered: bool,
    pub collision_ready: bool,
    pub bot_scripts_ready: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MapReadiness {
    loaded_at: Option<Duration>,
    next: Option<MapReadyStep>,
}

impl Default for MapReadiness {
    fn default() -> Self {
        Self {
            loaded_at: None,
            next: Some(MapReadyStep::Ownership),
        }
    }
}

impl MapReadiness {
    /// Anchor the entry schedule to the first dispatched map-loaded notice.
    /// Both inputs and tick times use the same monotonic time origin.
    pub fn mark_loaded(&mut self, now: Duration) -> bool {
        if self.loaded_at.is_some() {
            return false;
        }
        self.loaded_at = Some(now);
        true
    }

    /// Take one due action. A tick can take several actions in order, checking
    /// the live world prerequisites again after each action.
    pub fn take_due(&mut self, now: Duration, world: WorldReadiness) -> Option<MapReadyStep> {
        let waited = now.checked_sub(self.loaded_at?)?;
        let step = self.next?;
        if waited < step.delay()
            || (step == MapReadyStep::World
                && !(world.ownership_delivered && world.collision_ready && world.bot_scripts_ready))
        {
            return None;
        }
        self.next = step.next();
        Some(step)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lucy_core::bits::BitWriter;
    use lucy_wire::messages::{encode, write_channel};

    fn schemas() -> Schemas {
        Schemas::from_json(
            r#"{
                "C77F6403": {
                    "20306": [
                        {"type":6,"off":120,"size":4,"count":0,"array":false},
                        {"type":8,"off":128,"size":8,"count":-1,"array":true}
                    ]
                },
                "A7FBBD0C": {
                    "21600":[], "21602":[], "21603":[], "21604":[], "21616":[]
                }
            }"#,
        )
        .unwrap()
    }

    fn channel(schemas: &Schemas, reliable: &[(u32, u32)], unreliable: &[u32]) -> BitWriter {
        let message = |id| encode(schemas, id, &Fields::new()).unwrap();
        let reliable: Vec<_> = reliable
            .iter()
            .map(|&(sequence, id)| (sequence, message(id)))
            .collect();
        let unreliable: Vec<_> = unreliable.iter().copied().map(message).collect();
        let mut writer = BitWriter::new();
        write_channel(&mut writer, &reliable, &unreliable).unwrap();
        writer
    }

    #[test]
    fn fills_the_reliable_gap_before_unreliable_messages() {
        let schemas = schemas();
        let mut ingress = GameIngress::new();
        let mut delivered = Vec::new();
        let mut dispatch = |message: Message| delivered.push(message.id);
        let first = channel(&schemas, &[(1, 21603), (2, 21604)], &[21602]);
        let second = channel(&schemas, &[(1, 21616)], &[]);
        let third = channel(&schemas, &[(0, 21600), (1, 21603)], &[21602]);
        for frame in [first, second, third] {
            assert_eq!(
                ingress
                    .receive(&schemas, frame.as_bytes(), &mut dispatch)
                    .unwrap(),
                InputRecord::default()
            );
        }
        assert_eq!(delivered, [21602, 21600, 21616, 21604, 21602]);
    }

    #[test]
    fn a_truncated_channel_cannot_dispatch_its_valid_prefix() {
        let schemas = schemas();
        let mut ingress = GameIngress::new();
        let frame = channel(&schemas, &[(0, 21600), (1, 21603)], &[]).into_bytes();
        let bad = &frame[..frame.len() - 1];
        assert!(matches!(
            ingress.receive(&schemas, bad, |_| panic!("unexpected message")),
            Err(IngressError::Channel(_))
        ));

        let mut delivered = Vec::new();
        ingress
            .receive(&schemas, &frame, |message| delivered.push(message.id))
            .unwrap();
        assert_eq!(delivered, [21600, 21603]);
    }

    #[test]
    fn a_bad_input_record_does_not_undo_dispatched_messages() {
        let schemas = schemas();
        let mut ingress = GameIngress::new();
        let mut map = MapReadiness::default();
        let now = Duration::from_secs(10);
        let mut frame = channel(&schemas, &[(0, 21602)], &[]);
        frame.bit(true); // Presence bit without its 32-bit length.
        assert!(matches!(
            ingress.receive(&schemas, frame.as_bytes(), |message| {
                assert_eq!(MessageKind::from(message.id), MessageKind::MapLoaded);
                assert!(map.mark_loaded(now));
            }),
            Err(IngressError::Commands(InputError::Truncated))
        ));
        assert_eq!(
            map.take_due(now + Duration::from_millis(300), WorldReadiness::default()),
            Some(MapReadyStep::Ownership)
        );
        assert!(matches!(
            ingress.receive(&schemas, frame.as_bytes(), |_| panic!("duplicate message")),
            Err(IngressError::Commands(_))
        ));
    }

    #[test]
    fn too_many_messages_after_a_lost_one_refuse_the_client() {
        let schemas = schemas();
        let mut ingress = GameIngress::new();
        // Message 0 is lost: 1 to 256 wait for it, a 257th is one too many.
        for sequence in 1..=256 {
            let frame = channel(&schemas, &[(sequence, 21600)], &[]);
            ingress
                .receive(&schemas, frame.as_bytes(), |_| panic!("message 0 is lost"))
                .unwrap();
        }
        let frame = channel(&schemas, &[(257, 21600)], &[]);
        assert!(matches!(
            ingress.receive(&schemas, frame.as_bytes(), |_| panic!("message 0 is lost")),
            Err(IngressError::Backlog)
        ));
        // A message that is already waiting comes again, or the lost one.
        let mut delivered = 0;
        let frame = channel(&schemas, &[(256, 21600), (0, 21602)], &[]);
        ingress
            .receive(&schemas, frame.as_bytes(), |_| delivered += 1)
            .unwrap();
        assert_eq!(delivered, 257);
    }

    #[test]
    fn a_body_change_is_visible_when_the_returned_commands_are_applied() {
        let schemas = schemas();
        let mut frame = channel(&schemas, &[], &[21616]);
        // Input record from tests/test_game_commands.py::record(5000).
        let commands = [
            0x55, 0x01, 0, 0, 0x46, 0, 0, 0x04, 0x10, 0x27, 0, 0, 0x2e, 0, 0, 0x06, 0x7f, 0, 0, 0,
            0x80, 0x11, 0, 0x29, 0x4b, 0,
        ];
        frame.raw_bits(&commands, 203).unwrap();

        let mut body = None;
        let commands = GameIngress::new()
            .receive(&schemas, frame.as_bytes(), |message| {
                assert_eq!(MessageKind::from(message.id), MessageKind::GameMessage);
                body = Some(9);
            })
            .unwrap()
            .commands;
        assert_eq!(body, Some(9));
        assert_eq!(
            commands
                .iter()
                .map(|command| command.frame)
                .collect::<Vec<_>>(),
            [5000, 5001]
        );
    }

    #[test]
    fn reliable_ordinals_do_not_wrap_with_udp_packet_sequences() {
        let schemas = schemas();
        let mut ingress = GameIngress {
            expected_reliable: u64::from(u32::MAX),
            ..GameIngress::default()
        };
        let frame = channel(&schemas, &[(u32::MAX, 21600), (0, 21603)], &[]);
        let mut delivered = Vec::new();
        ingress
            .receive(&schemas, frame.as_bytes(), |message| {
                delivered.push(message.id)
            })
            .unwrap();
        assert_eq!(delivered, [21600]);
        assert!(ingress.parked.is_empty());
    }

    #[test]
    fn builds_the_pong_with_the_three_server_stamps() {
        let ping = Fields::from([
            (0x78, Value::Unsigned(3)),
            (
                0x80,
                Value::Array(vec![Value::Unsigned(111), Value::Unsigned(222)]),
            ),
        ]);
        let nanos = Duration::from_nanos;
        let pending = Pong::new(&ping, nanos(333), nanos(444)).unwrap();
        let reply = pending.sent(nanos(555));
        assert_eq!(reply.id, 20306);
        // The index, then the count and the stamps: the client's made and
        // sent, the server's received, made and sent (`Pong`).
        let bytes = encode(&schemas(), reply.id, &reply.fields).unwrap();
        let mut expected = vec![0, 6, 3, 0, 0, 0, 5, 0, 0, 0];
        for stamp in [111_u64, 222, 333, 444, 555] {
            expected.extend(stamp.to_le_bytes());
        }
        assert_eq!(bytes, expected);
        let defaults = Pong::new(&Fields::new(), nanos(1), nanos(2))
            .unwrap()
            .sent(nanos(3));
        assert_eq!(defaults.fields[&0x78], Value::Unsigned(0));
        assert_eq!(
            defaults.fields[&0x80],
            Value::Array(vec![
                Value::Unsigned(1),
                Value::Unsigned(2),
                Value::Unsigned(3)
            ])
        );
        let malformed = Fields::from([(0x80, Value::Array(vec![Value::Signed(-1)]))]);
        assert_eq!(
            Pong::new(&malformed, nanos(1), nanos(2)),
            Err(InvalidPingField(0x80))
        );
        assert_eq!(MessageKind::from(21610), MessageKind::Ping);
        assert_eq!(MessageKind::from(21601), MessageKind::MapLoaded);
        assert_eq!(MessageKind::from(21603), MessageKind::Other);
        assert_eq!(MessageKind::from(21604), MessageKind::HeroSettings);
        assert_eq!(MessageKind::from(21608), MessageKind::CommunicationAction);
    }

    #[test]
    fn matches_a_python_mixed_frame_with_the_runtime_schema() {
        let schemas = Schemas::game();
        // Python write_channel + tests/test_game_commands.py::record(5000).
        let frame = [
            0x23, 0, 0, 0, 0, 0, 1, 2, 1, 10, 3, 0, 0, 0, 2, 0, 0, 0, 111, 0, 0, 0, 0, 0, 0, 0,
            222, 0, 0, 0, 0, 0, 0, 0, 0x55, 1, 0, 0, 0x46, 0, 0, 4, 0x10, 0x27, 0, 0, 0x2e, 0, 0,
            6, 0x7f, 0, 0, 0, 0x80, 0x11, 0, 0x29, 0x4b, 0,
        ];
        let mut map = MapReadiness::default();
        let mut ping = None;
        let commands = GameIngress::new()
            .receive(&schemas, &frame, |message| {
                match MessageKind::from(message.id) {
                    MessageKind::MapLoaded => assert!(map.mark_loaded(Duration::ZERO)),
                    MessageKind::Ping => {
                        assert_eq!(
                            map.take_due(Duration::from_millis(300), WorldReadiness::default()),
                            Some(MapReadyStep::Ownership)
                        );
                        ping = Some(message.fields);
                    }
                    kind => panic!("unexpected message kind {kind:?}"),
                }
            })
            .unwrap()
            .commands;
        assert_eq!(commands[0].frame, 5000);
        assert_eq!(commands[1].frame, 5001);
        assert_eq!(
            ping,
            Some(Fields::from([
                (0x78, Value::Unsigned(3)),
                (
                    0x80,
                    Value::Array(vec![Value::Unsigned(111), Value::Unsigned(222)])
                ),
            ]))
        );
    }

    #[test]
    fn map_steps_are_relative_to_the_first_notice_and_wait_for_the_world() {
        let mut map = MapReadiness::default();
        let waiting = WorldReadiness::default();
        let ready = WorldReadiness {
            ownership_delivered: true,
            collision_ready: true,
            bot_scripts_ready: true,
        };
        let time = |offset: u64| Duration::from_millis(10_000 + offset);
        assert_eq!(map.take_due(time(1_000), ready), None);
        assert!(map.mark_loaded(time(0)));
        assert!(!map.mark_loaded(time(250)));
        assert_eq!(map.take_due(Duration::ZERO, ready), None);
        assert_eq!(map.take_due(time(299), ready), None);
        assert_eq!(
            map.take_due(time(300), waiting),
            Some(MapReadyStep::Ownership)
        );
        assert_eq!(map.take_due(time(399), waiting), None);
        assert_eq!(map.take_due(time(400), waiting), Some(MapReadyStep::Game));
        assert_eq!(map.take_due(time(599), ready), None);
        for blocker in [
            WorldReadiness {
                ownership_delivered: false,
                ..ready
            },
            WorldReadiness {
                collision_ready: false,
                ..ready
            },
            WorldReadiness {
                bot_scripts_ready: false,
                ..ready
            },
        ] {
            assert_eq!(map.take_due(time(600), blocker), None);
        }
        assert_eq!(map.take_due(time(600), ready), Some(MapReadyStep::World));
        assert_eq!(map.take_due(time(699), ready), None);
        assert_eq!(
            map.take_due(time(700), ready),
            Some(MapReadyStep::Controller)
        );
        assert_eq!(map.take_due(time(701), ready), None);
    }
}
