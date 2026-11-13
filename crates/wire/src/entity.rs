//! Server world frames for retail build 104319.
//!
//! The wire layout and component fields come from the Python reference; frame
//! selection follows its ordered send budget.
//!
//! Client research (1.74). A kind-1 datagram's world frame, as the client
//! reads it (0x7FF7896DE110): var_a packet frame; ch0 the message channel;
//! ch3 `var_a ack, 4 bits flags, 1 bit "ping follows"` (the client drops its
//! commands below ack - 1); ch4 1 bit (1 = keep, 0 resets the entity
//! channel); ch5 1 bit (the end of an empty list); then 7 bits of entity
//! count, and per entity its id and one bit per channel in order (0 ends the
//! entity, 1 runs that channel's reader, which has presence bits of its own).
//!
//! The entity channel (ch4, reader 0x7FF78969BBC0) takes a 2-bit op: 1
//! create, 2 update, 3 destroy; a create or update carries 14 bits of length
//! and that many bits. A create (0x7FF78969C360, body 0x7FF78969DAB0): bit
//! from map: 1 -> 32 bits placeable index, 1 bit transform override (0), then
//! the update body (a server-owned placeable's entity id must be `0x80000000 |
//! n`); else the 003 definition's GUID index, 8 bits flags: 0x01 the skin
//! (with 0x40, 32 bits of a skin theme 0A6, bit 31 asking for a remap, and 1
//! bit for the theme's golden weapon; else a skin 076 GUID index); 0x08 3 f32
//! position and a quaternion (x y z w); 0x08 or 0x04 3 f32 scale; 0x02 a
//! movement-state block; then the update body. Flag 0x10 is the player's own
//! entity (command components added when 20302 named it), 0x20 adds component
//! 74; the transform floats are thrown away on that path.
//!
//! The update body (0x7FF78969E5D0 / 0x7FF78969B870): 8 bits count, then per
//! component its index, a pad to the next payload byte, a u8 mask length and
//! the mask (one bit per replicated field), then the set fields' values.
//! Scalars go as whole bytes; a u64 marked as an id is a byte mask and its
//! non-zero bytes (a 16-byte id as low, high, unpadded: 0x7FF78A76B500); a
//! teString is a u32 byte count and the bytes (0x7FF78AAF0300).
//!
//! Component notes: MPlayerComponentData (field records at 0x7FF78BFEAE50,
//! names from the PS4 data): m_lobbyPlayerID, m_battleTag, m_playerLevel, s32
//! audio locale, group token, party state, platform, m_rankedLevel,
//! m_heroicRank, m_rankedLevelTier (0-6), m_moderator. MECPossessorData:
//! current, view target, primary possessable. MECGameModeParticipant:
//! portrait frame, s16, slot, lfg role, u8, role queue role, can select a
//! hero, u8, u8. The health component (type 0x7FF78BFE8720): 16 health, 16
//! armour and 16 shield pools, then u32, u32, entity, u8, u8. A health pool
//! (type 0x7FF78BFE8660, 20 bytes): u32 id (0 = none), f32 max, f32 current,
//! u32 type (0 health, 1 armour, 2 shields; array t holds type t,
//! 0x7FF7894D0530), then four bytes. The HUD bar (0x7FF7894CCEE0) draws a pool
//! as its type only when byte 17 is 1 and byte 18 is 0, else as over-health in
//! another colour; 1 is the client's default for byte 17 (0x7FF78A1FFBD0).
//!
//! The movement-state block (0x7FF789B6F680) is a delta against a mostly-zero
//! reference state: command frame, flags, aim pitch, throttles and the last
//! command that had any, air ticks, gravity scale, last stand-up frame, Euler
//! angles, position, velocity, spring offsets and speed, gravity's part of the
//! velocity. After one bit (0 = against that reference) the same block is a
//! ch3 movement record; the client only interpolates between records and
//! keeps a state only when its command frame is newer than the entity's
//! newest (gate 0x7FF7896DA7D3). Another client animates a body from it
//! (0x7FF789B1EB10). Fields +16 to +40 (0x7FF789B6F80E to 0x7FF789B71E50) have
//! reference values +16 = 0, +20 = +40 = 0xFFFFFFFF, +24 = 0.0, +28 = 1.0
//! (move-speed scale), +32 to +35 = 0, +36 = 0xFFFFFFFF. +36 is the frame of
//! the last command with throttles: a 0 bit makes it the record's frame (+8);
//! the client reads +34/+35 only when +36 is not +8 (0x7FF789B71DF6), else it
//! copies +32/+33, so the record's own frame must go as the 0 bit without
//! +34/+35. An s8 throttle is 1 and its byte, or 0 for the reference's 0
//! (0x7FF789B71C50). Spring offsets +52/+56 are the reference's steps plus a
//! signed delta (0x7FF789B770B0); +60 spring speed; +0x360 parent entity,
//! +0x364 its scene-node matrix; +56, +384 and +832 are three optional raw
//! floats (0x7FF789B71CE0). A record's frame counts back from its base
//! (0x7FF789B76D00: the packet frame on ch3); a create's movement state counts
//! back from a fixed base (0x7FF78969DD3C).

use std::{borrow::Borrow, error::Error, fmt};

use crate::link::{HEADER_SIZE, MAX_DATAGRAM};
use crate::messages::{ChannelError, MAX_PER_KIND, write_channel};
use orpheus_core::bits::{BitError, BitWriter};

pub const MAX_ENTITY_RECORDS: usize = 127;
pub const MAX_PAYLOAD: usize = MAX_DATAGRAM - HEADER_SIZE;
pub const GAME_MODE_ENTITY: u32 = 0xd000_0010;

pub const WITH_SKIN: u8 = 0x01;
pub const WITH_MOVEMENT: u8 = 0x02;
pub const WITH_SCALE: u8 = 0x04;
pub const WITH_TRANSFORM: u8 = 0x08;
pub const PLAYER_PATH: u8 = 0x10;
pub const WITH_PARTICIPANT: u8 = 0x20;
pub const SKIN_THEME: u8 = 0x40;

pub const POOLS: usize = 16;
pub const HEALTH_POOL: usize = 0;
pub const ARMOUR_POOL: usize = 1;
pub const SHIELDS_POOL: usize = 2;

pub const NO_INPUT: u32 = u32::MAX;
pub const CREATE_BASE: u32 = u32::MAX;
pub const ANGLE_UNITS: f64 = 65536.0 / std::f64::consts::TAU;
pub const POSITION_UNITS: f64 = 1024.0;
pub const MOVEMENT_AXIS_BITS: usize = 24;
pub const MAX_MOVEMENT_AXIS: f64 = ((1 << (MOVEMENT_AXIS_BITS - 1)) - 1) as f64 / POSITION_UNITS;
pub const GMP_STATS_FIELDS: usize = 45;

#[derive(Debug)]
pub enum WorldError {
    Bits(BitError),
    Channel(ChannelError),
    UnknownComponent(u8),
    FieldCount { component: u8, count: usize },
    FieldType { component: u8, field: usize },
    LengthOverflow,
    InvalidQuantizedValue(f64),
    InputFrameNeedsRecordFrame,
    FutureMovement { frame: u32, tick: u32 },
    TooManyEntities,
    EntityRecordTooLong,
    ReliableMessageDoesNotFit,
    QueuedDataDoesNotFit,
}

impl fmt::Display for WorldError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Bits(error) => fmt::Display::fmt(error, f),
            Self::Channel(error) => fmt::Display::fmt(error, f),
            Self::UnknownComponent(index) => write!(f, "unknown world component {index}"),
            Self::FieldCount { component, count } => {
                write!(f, "component {component} does not have {count} fields")
            }
            Self::FieldType { component, field } => {
                write!(f, "component {component} field {field} has the wrong type")
            }
            Self::LengthOverflow => f.write_str("component value length does not fit in uint32"),
            Self::InvalidQuantizedValue(value) => {
                write!(f, "invalid movement value {value} on the 1/1024 grid")
            }
            Self::InputFrameNeedsRecordFrame => {
                f.write_str("a state with an input frame needs the record's frame")
            }
            Self::FutureMovement { frame, tick } => {
                write!(
                    f,
                    "a state of frame {frame} cannot go in packet frame {tick}"
                )
            }
            Self::TooManyEntities => write!(f, "at most {MAX_ENTITY_RECORDS} entities per frame"),
            Self::EntityRecordTooLong => f.write_str("an entity record is at most 16383 bits"),
            Self::ReliableMessageDoesNotFit => {
                f.write_str("oldest reliable message exceeds datagram budget")
            }
            Self::QueuedDataDoesNotFit => {
                f.write_str("oldest queued data exceeds datagram budget; fragmentation required")
            }
        }
    }
}

impl Error for WorldError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Bits(error) => Some(error),
            Self::Channel(error) => Some(error),
            _ => None,
        }
    }
}

impl From<BitError> for WorldError {
    fn from(value: BitError) -> Self {
        Self::Bits(value)
    }
}

impl From<ChannelError> for WorldError {
    fn from(value: ChannelError) -> Self {
        Self::Channel(value)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LeaseBlock {
    pub definition: u64,
    pub start: u32,
    pub stop: u32,
    pub first_id: u32,
    pub per_frame: u8,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ComponentValue {
    Id(u64),
    DbId(u64, u64),
    Entity(u32),
    Text(String),
    Blob20([u8; 20]),
    Leases(Vec<LeaseBlock>),
    U32Array(Vec<u32>),
    U8(u8),
    I8(i8),
    I16(i16),
    U16(u16),
    U32(u32),
    I32(i32),
    U64(u64),
    F32(f32),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Component {
    pub index: u8,
    /// Ordered field values. Absent and omitted trailing fields clear their mask bits.
    pub fields: Vec<Option<ComponentValue>>,
}

#[derive(Clone, Copy)]
enum Field {
    Unsupported,
    Id,
    DbId,
    Entity,
    Text,
    Blob20,
    Leases,
    U32Array,
    U8,
    I8,
    I16,
    U16,
    U32,
    I32,
    U64,
    F32,
}

fn layout(index: u8) -> Result<&'static [Field], WorldError> {
    use Field::*;
    const HEALTH: [Field; 53] = {
        let mut fields = [Blob20; 53];
        fields[48] = U32;
        fields[49] = U32;
        fields[50] = Entity;
        fields[51] = U8;
        fields[52] = U8;
        fields
    };
    const STATS: [Field; GMP_STATS_FIELDS] = {
        let mut fields = [U8; GMP_STATS_FIELDS];
        let mut index = 0;
        while index < 35 {
            fields[index] = match index {
                // These nested fields have no writer in the reference.
                0..=4 => Unsupported,
                5..=18 => U64,
                _ => F32,
            };
            index += 1;
        }
        fields[35] = U16;
        fields
    };
    Ok(match index {
        26 | 123 => &[Id],
        29 => &[DbId, Text, U32, I32, I32, I32, I32, I16, I16, I8, U8],
        44 => &[Entity, Entity, Entity],
        51 => &HEALTH,
        64 => &STATS,
        74 => &[Id, I16, I8, U8, U8, I8, U8, U8, U8],
        75 => &[Id; 6],
        90 => &[Leases],
        114 => &[
            U32Array, Id, Id, U64, U64, U64, U64, I32, F32, F32, U32, U8, U8, U8, U8, U8, U8, U8,
            U8, U8,
        ],
        _ => return Err(WorldError::UnknownComponent(index)),
    })
}

fn pad(out: &mut BitWriter, origin: usize) -> Result<(), BitError> {
    out.bits(0, origin.wrapping_add(out.bit_len()).wrapping_neg() & 7)
}

fn byte_mask(out: &mut BitWriter, value: u64) {
    let mut bytes = [0; 9];
    let mut length = 1;
    for (index, byte) in value.to_le_bytes().into_iter().enumerate() {
        if byte != 0 {
            bytes[0] |= 1 << index;
            bytes[length] = byte;
            length += 1;
        }
    }
    out.raw(&bytes[..length]);
}

fn write_count(out: &mut BitWriter, count: usize) -> Result<(), WorldError> {
    let count = u32::try_from(count).map_err(|_| WorldError::LengthOverflow)?;
    out.raw(&count.to_le_bytes());
    Ok(())
}

fn write_field(
    out: &mut BitWriter,
    origin: usize,
    field: Field,
    value: &ComponentValue,
) -> Result<bool, WorldError> {
    match (field, value) {
        (Field::Id, ComponentValue::Id(number)) => {
            pad(out, origin)?;
            byte_mask(out, *number);
        }
        (Field::DbId, ComponentValue::DbId(low, high)) => {
            byte_mask(out, *low);
            byte_mask(out, *high);
        }
        (Field::Text, ComponentValue::Text(text)) => {
            pad(out, origin)?;
            write_count(out, text.len())?;
            out.raw(text.as_bytes());
        }
        (Field::Blob20, ComponentValue::Blob20(data)) => out.raw(data),
        (Field::Leases, ComponentValue::Leases(blocks)) => {
            pad(out, origin)?;
            write_count(out, blocks.len())?;
            for block in blocks {
                out.raw(&block.definition.to_le_bytes());
                out.raw(&block.start.to_le_bytes());
                out.raw(&block.stop.to_le_bytes());
                out.raw(&block.first_id.to_le_bytes());
                out.raw(&[block.per_frame]);
            }
        }
        (Field::U32Array, ComponentValue::U32Array(numbers)) => {
            pad(out, origin)?;
            write_count(out, numbers.len())?;
            for number in numbers {
                out.raw(&number.to_le_bytes());
            }
        }
        (Field::Entity, ComponentValue::Entity(number))
        | (Field::U32, ComponentValue::U32(number)) => out.raw(&number.to_le_bytes()),
        (Field::U8, ComponentValue::U8(number)) => out.raw(&[*number]),
        (Field::I8, ComponentValue::I8(number)) => out.raw(&number.to_le_bytes()),
        (Field::I16, ComponentValue::I16(number)) => out.raw(&number.to_le_bytes()),
        (Field::U16, ComponentValue::U16(number)) => out.raw(&number.to_le_bytes()),
        (Field::I32, ComponentValue::I32(number)) => out.raw(&number.to_le_bytes()),
        (Field::U64, ComponentValue::U64(number)) => out.raw(&number.to_le_bytes()),
        (Field::F32, ComponentValue::F32(number)) => out.f32(*number),
        _ => return Ok(false),
    }
    Ok(true)
}

fn write_components(
    out: &mut BitWriter,
    origin: usize,
    components: &[Component],
) -> Result<(), WorldError> {
    out.bits(components.len() as u64, 8)?;
    for component in components {
        let fields = layout(component.index)?;
        if component.fields.len() > fields.len() {
            return Err(WorldError::FieldCount {
                component: component.index,
                count: component.fields.len(),
            });
        }
        let mut mask = vec![0; fields.len().div_ceil(8)];
        for (index, value) in component.fields.iter().enumerate() {
            if value.is_some() {
                mask[index / 8] |= 1 << (index % 8);
            }
        }
        out.bits(u64::from(component.index), 8)?;
        pad(out, origin)?;
        out.bits(mask.len() as u64, 8)?;
        out.raw(&mask);
        for (index, (&field, value)) in fields.iter().zip(&component.fields).enumerate() {
            let Some(value) = value else {
                continue;
            };
            if !write_field(out, origin, field, value)? {
                return Err(WorldError::FieldType {
                    component: component.index,
                    field: index,
                });
            }
        }
    }
    Ok(())
}

/// A single client health pool. Each of the three kinds has 16 slots.
pub fn pool(id: u32, kind: u32, most: f32, now: f32) -> [u8; 20] {
    let mut data = [0; 20];
    data[..4].copy_from_slice(&id.to_le_bytes());
    data[4..8].copy_from_slice(&most.to_le_bytes());
    data[8..12].copy_from_slice(&now.to_le_bytes());
    data[12..16].copy_from_slice(&kind.to_le_bytes());
    data[16..].copy_from_slice(&[0, 1, 0, 1]);
    data
}

/// Encode supplied pool fields at `kind * POOLS + slot`, omitting all others.
/// A zero ID clears a slot. ORPHEUS writes an all-zero blob for a clear;
/// the client's ancillary bytes for a native clear have not been verified.
pub fn pool_component(slots: impl IntoIterator<Item = (usize, [u8; 20])>) -> Component {
    let mut fields = vec![None; 3 * POOLS];
    for (field, bytes) in slots {
        fields[field] = Some(ComponentValue::Blob20(bytes));
    }
    Component { index: 51, fields }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Movement {
    pub position: [f64; 3],
    pub yaw: i32,
    pub pitch: i32,
    pub velocity: [f64; 3],
    pub flags: u32,
    /// Right, then forward.
    pub throttles: [i8; 2],
    /// Last command with throttles. None means this record's frame, NO_INPUT means never.
    pub input_frame: Option<u32>,
    pub input_throttles: [i8; 2],
    pub air_ticks: u32,
    pub gravity: f64,
    pub frame: Option<u32>,
    pub crouch_frame: Option<u32>,
    pub spring: [i32; 2],
    pub spring_speed: f64,
    pub fall: f32,
    /// The body's discontinuity counter (+12; ow1-preservation's movement
    /// "field 3", a teleport generation). The owner's client compares it
    /// with its own body's: equal, a different state is a correction it
    /// replays its later commands on; different, it takes the state as it
    /// is, without replay (`orpheus_physics::mover::correction`). A create
    /// starts the body at 0.
    pub discontinuity: u32,
}

fn quantize(value: f64) -> Result<i64, WorldError> {
    let steps = (value * POSITION_UNITS).round_ties_even();
    if !(i64::MIN as f64..-(i64::MIN as f64)).contains(&steps) {
        return Err(WorldError::InvalidQuantizedValue(value));
    }
    Ok(steps as i64)
}

fn selector(out: &mut BitWriter, value: i64, index: usize, width: usize) -> Result<(), BitError> {
    out.bit(true);
    out.bits(0, index)?;
    out.bit(true);
    out.signed(value, width)
}

fn delta(out: &mut BitWriter, value: i64, widths: &[usize]) -> Result<(), BitError> {
    out.bit(value != 0);
    if value == 0 {
        return Ok(());
    }
    for &width in widths {
        let half = 1_i64 << (width - 1);
        let fits = (-half..half).contains(&value);
        out.bit(fits);
        if fits {
            return out.signed(value, width);
        }
    }
    out.signed(value, 32)
}

fn throttle(out: &mut BitWriter, value: i8) -> Result<(), BitError> {
    out.bit(value != 0);
    if value != 0 {
        out.signed(i64::from(value), 8)?;
    }
    Ok(())
}

fn input_fields(
    out: &mut BitWriter,
    state: &Movement,
    input_frame: Option<u32>,
    gravity: i64,
) -> Result<(), BitError> {
    delta(out, i64::from(state.air_ticks), &[4, 10])?;
    out.bit(false); // No timed movement event.
    delta(out, gravity, &[8, 10, 24])?;
    out.bit(false); // Move-speed scale remains 1.

    // Equal-to-record frames omit the historical throttles. The client copies
    // the current throttles in that case, and would consume the extras as angles.
    out.bit(input_frame.is_some());
    if let Some(frame) = input_frame {
        out.bit(frame != NO_INPUT);
        if frame != NO_INPUT {
            out.bit(false);
            out.bits(u64::from(frame), 32)?;
        }
    }
    out.bit(state.crouch_frame.is_some());
    if let Some(frame) = state.crouch_frame {
        out.bit(false);
        out.bits(u64::from(frame), 32)?;
    }
    for value in state.throttles {
        throttle(out, value)?;
    }
    if input_frame.is_some() {
        for value in state.input_throttles {
            throttle(out, value)?;
        }
    }
    Ok(())
}

/// Encode the movement state against the client's fixed reference state.
pub fn write_movement(
    out: &mut BitWriter,
    state: &Movement,
    frame_back: Option<u32>,
    base: Option<u32>,
    parent: Option<u32>,
) -> Result<(), WorldError> {
    let frame_back = frame_back.unwrap_or(0);
    let frame = base.map(|base| base.wrapping_sub(frame_back));
    if state.input_frame.is_some() && frame.is_none() {
        return Err(WorldError::InputFrameNeedsRecordFrame);
    }
    let input_frame = state.input_frame.filter(|&value| Some(value) != frame);
    let gravity = quantize(state.gravity)?;
    let speed = quantize(state.spring_speed)?;
    let [x, y, z] = state.position.map(quantize);
    let position = [x?, y?, z?];
    let [x, y, z] = state.velocity.map(quantize);
    let velocity = [x?, y?, z?];

    out.bit(state.flags != 0);
    if state.flags != 0 {
        for byte in state.flags.to_le_bytes() {
            out.bit(byte != 0);
            if byte != 0 {
                out.bits(u64::from(byte), 8)?;
            }
        }
    }
    out.bit(false); // 1/1024 grid.
    out.bit(frame_back != 0);
    if frame_back != 0 {
        let width = if frame_back < 16 {
            out.bit(true);
            4
        } else {
            out.bit(false);
            out.bit(frame_back < 1024);
            if frame_back < 1024 { 10 } else { 32 }
        };
        out.bits(u64::from(frame_back), width)?;
    }
    // +12: present, then one width bit (0: none, raw 32 bits follow), as
    // ow1-preservation decoded it in the 104319 client (field 3: "changed
    // bit, raw bit 0, u32"; 0xc61b60). Its short width is not known.
    out.bit(state.discontinuity != 0);
    if state.discontinuity != 0 {
        out.bit(false);
        out.bits(u64::from(state.discontinuity), 32)?;
    }
    let pitch = state.pitch as u16;
    if pitch != 0 {
        selector(out, i64::from(pitch), 2, 16)?;
    } else {
        out.bit(false);
    }
    input_fields(out, state, input_frame, gravity)?;
    let yaw = state.yaw as u16;
    if yaw != 0 {
        selector(out, i64::from(yaw), 2, 16)?;
    } else {
        out.bit(false);
    }
    out.bits(0, 2)?; // No Euler pitch or roll.
    for axis in position {
        selector(out, axis, 2, MOVEMENT_AXIS_BITS)?;
    }
    let has_velocity = velocity != [0; 3];
    out.bit(has_velocity);
    if has_velocity {
        for axis in velocity {
            if axis != 0 {
                selector(out, axis, 2, MOVEMENT_AXIS_BITS)?;
            } else {
                out.bit(false);
            }
        }
    }
    out.bits(0, 9)?;
    out.bit(parent.is_some());
    if let Some(parent) = parent {
        out.bits(u64::from(parent), 32)?;
        out.bits(0, 32)?; // Scene-node matrix index.
    }
    delta(out, i64::from(state.spring[0]), &[8, 12, 17])?;
    delta(out, speed, &[8, 10, 24])?;
    delta(out, i64::from(state.spring[1]), &[8, 12, 17])?;
    out.bit(false); // +384
    out.bit(state.fall != 0.0);
    if state.fall != 0.0 {
        out.bit(false);
        out.bit(true);
        out.f32(state.fall);
        out.bit(false);
    }
    Ok(())
}

/// A channel-3 movement record, including its own 12-bit length.
pub fn movement_record(state: &Movement, tick: Option<u32>) -> Result<BitWriter, WorldError> {
    let frame_back = match (state.frame, tick) {
        (Some(frame), Some(tick)) => Some(
            tick.checked_sub(frame)
                .ok_or(WorldError::FutureMovement { frame, tick })?,
        ),
        _ => None,
    };
    let mut body = BitWriter::new();
    body.bit(false);
    write_movement(&mut body, state, frame_back, tick, None)?;
    let mut out = BitWriter::new();
    out.bits((12 + body.bit_len()) as u64, 12)?;
    out.append(&body);
    Ok(out)
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct SkinTheme {
    pub guid: u64,
    pub golden: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Create {
    pub definition: u64,
    pub components: Vec<Component>,
    pub flags: u8,
    pub position: [f32; 3],
    pub rotation: [f32; 4],
    pub movement: Option<Movement>,
    pub frame_back: u32,
    pub skin: SkinTheme,
    pub movement_parent: Option<u32>,
}

impl Create {
    pub fn new(definition: u64, components: Vec<Component>) -> Self {
        Self {
            definition,
            components,
            flags: WITH_TRANSFORM | WITH_SCALE,
            position: [0.0; 3],
            rotation: [0.0, 0.0, 0.0, 1.0],
            movement: None,
            frame_back: 1,
            skin: SkinTheme::default(),
            movement_parent: None,
        }
    }
}

/// Create a definition-based entity. The origin is its bit offset in the payload.
pub fn create(origin: usize, entity: &Create) -> Result<BitWriter, WorldError> {
    let mut flags = entity.flags;
    if entity.movement.is_some() {
        flags |= WITH_MOVEMENT;
    }
    if entity.skin.guid != 0 {
        flags |= WITH_SKIN | SKIN_THEME;
    }
    let mut out = BitWriter::new();
    out.bit(false);
    out.guid_index(entity.definition);
    out.bits(u64::from(flags), 8)?;
    if entity.skin.guid != 0 {
        out.bits(entity.skin.guid & 0x7fff_ffff, 32)?;
        out.bit(entity.skin.golden);
    }
    if flags & WITH_TRANSFORM != 0 {
        for value in entity.position.into_iter().chain(entity.rotation) {
            out.f32(value);
        }
    }
    if flags & (WITH_TRANSFORM | WITH_SCALE) != 0 {
        for _ in 0..3 {
            out.f32(1.0);
        }
    }
    if let Some(movement) = &entity.movement {
        write_movement(
            &mut out,
            movement,
            Some(entity.frame_back),
            Some(CREATE_BASE),
            entity.movement_parent,
        )?;
    }
    write_components(&mut out, origin, &entity.components)?;
    Ok(out)
}

pub fn create_placeable(origin: usize, index: u32) -> Result<BitWriter, WorldError> {
    let mut out = BitWriter::new();
    out.bit(true);
    out.bits(u64::from(index), 32)?;
    out.bit(false); // Keep the map's transform.
    write_components(&mut out, origin, &[])?;
    Ok(out)
}

pub fn update(origin: usize, components: &[Component]) -> Result<BitWriter, WorldError> {
    let mut out = BitWriter::new();
    write_components(&mut out, origin, components)?;
    Ok(out)
}

#[derive(Clone, Debug, PartialEq)]
pub enum Operation {
    Create(Box<Create>),
    Placeable(u32),
    Update(Vec<Component>),
    Destroy,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct EntityUpdate {
    pub entity: u32,
    pub operation: Option<Operation>,
    pub movement: Option<Movement>,
    pub chunk: Option<BitWriter>,
    pub ch2: Option<BitWriter>,
}

fn write_entity(out: &mut BitWriter, item: &EntityUpdate, tick: u32) -> Result<(), WorldError> {
    let after_ch1 = item.operation.is_some() || item.movement.is_some() || item.ch2.is_some();
    out.entity_id(Some(item.entity));
    out.bit(true); // Channel 0 reads nothing after its run bit.
    out.bit(after_ch1 || item.chunk.is_some());
    if !after_ch1 && item.chunk.is_none() {
        return Ok(());
    }
    out.bit(item.chunk.is_some());
    if let Some(chunk) = &item.chunk {
        out.bit(false); // Skipped by the client.
        out.append(chunk);
    }
    out.bit(after_ch1);
    if !after_ch1 {
        return Ok(());
    }
    if let Some(ch2) = &item.ch2 {
        out.append(ch2);
    } else {
        out.bits(0, 2)?; // No event or state on channel 2.
    }
    out.bit(true);
    out.bit(item.movement.is_some());
    if let Some(state) = &item.movement {
        out.append(&movement_record(state, Some(tick))?);
    }
    out.bit(item.operation.is_some());
    let Some(operation) = &item.operation else {
        return Ok(());
    };
    let opcode = match operation {
        Operation::Create(_) | Operation::Placeable(_) => 1,
        Operation::Update(_) => 2,
        Operation::Destroy => 3,
    };
    out.bits(opcode, 2)?;
    let origin = out.bit_len() + 14;
    let record = match operation {
        Operation::Create(entity) => Some(create(origin, entity)?),
        Operation::Placeable(index) => Some(create_placeable(origin, *index)?),
        Operation::Update(components) => Some(update(origin, components)?),
        Operation::Destroy => None,
    };
    if let Some(record) = record {
        if record.bit_len() >= 1 << 14 {
            return Err(WorldError::EntityRecordTooLong);
        }
        out.bits(record.bit_len() as u64, 14)?;
        out.append(&record);
    }
    out.bit(false); // No channel 5 payload.
    Ok(())
}

/// Encode one kind-1 payload. Entity and message ordering belong to the caller.
pub fn frame(
    tick: u32,
    ack: u32,
    reliable: &[(u32, Vec<u8>)],
    unreliable: &[Vec<u8>],
    entities: &[&EntityUpdate],
) -> Result<Vec<u8>, WorldError> {
    if entities.len() > MAX_ENTITY_RECORDS {
        return Err(WorldError::TooManyEntities);
    }
    let mut out = BitWriter::new();
    out.var_a(tick);
    write_channel(&mut out, reliable, unreliable)?;
    out.var_a(ack);
    out.bits(0, 4)?;
    out.bit(false); // No ping time.
    out.bit(true); // Keep channel 4.
    out.bit(false); // Empty channel 5.
    out.bits(entities.len() as u64, 7)?;
    for entity in entities {
        write_entity(&mut out, entity, tick)?;
    }
    Ok(out.into_bytes())
}

pub struct FrameCandidates<'a, Queued = EntityUpdate> {
    pub tick: u32,
    pub reliable: &'a [(u32, Vec<u8>)],
    pub unreliable: &'a [Vec<u8>],
    pub queued: &'a [Queued],
    pub movements: &'a [EntityUpdate],
    /// The final movement, if present, corrects the owner.
    pub owner_correction: bool,
    pub reliable_only: bool,
}

#[derive(Debug)]
pub struct SelectedFrame<'a> {
    pub payload: Vec<u8>,
    pub reliable_count: usize,
    pub unreliable_count: usize,
    pub entities: Vec<&'a EntityUpdate>,
}

/// Fit candidates in channel priority order. This only selects a payload.
/// Queue removal, resend bookkeeping and correction commits belong to the
/// session and must happen after its datagram is sent successfully.
pub fn select_frame<'a, Queued: Borrow<EntityUpdate>>(
    candidates: FrameCandidates<'a, Queued>,
    ack_for_records: impl Fn(&[&EntityUpdate]) -> u32,
) -> Result<SelectedFrame<'a>, WorldError> {
    let FrameCandidates {
        tick,
        reliable,
        unreliable,
        queued,
        movements,
        owner_correction,
        reliable_only,
    } = candidates;
    let mut reliable = &reliable[..reliable.len().min(MAX_PER_KIND)];
    let mut unreliable = if reliable_only {
        &[]
    } else {
        &unreliable[..unreliable.len().min(MAX_PER_KIND)]
    };
    let had_queued_data = !reliable.is_empty() || !unreliable.is_empty() || !queued.is_empty();
    let mut entities: Vec<_> = if reliable_only {
        Vec::new()
    } else {
        queued
            .iter()
            .map(Borrow::borrow)
            .chain(movements)
            .take(MAX_ENTITY_RECORDS)
            .collect()
    };
    let encode =
        |reliable: &[(u32, Vec<u8>)], unreliable: &[Vec<u8>], records: &[&EntityUpdate]| {
            frame(
                tick,
                ack_for_records(records),
                reliable,
                unreliable,
                records,
            )
        };
    let mut payload = encode(reliable, unreliable, &entities)?;
    if payload.len() > MAX_PAYLOAD {
        if !unreliable.is_empty() || !entities.is_empty() {
            payload = encode(reliable, &[], &[])?;
        }
        while payload.len() > MAX_PAYLOAD {
            if reliable.len() <= 1 {
                return Err(WorldError::ReliableMessageDoesNotFit);
            }
            reliable = &reliable[..reliable.len() / 2];
            payload = encode(reliable, &[], &[])?;
        }
        while !unreliable.is_empty() {
            let attempt = encode(reliable, unreliable, &[])?;
            if attempt.len() <= MAX_PAYLOAD {
                payload = attempt;
                break;
            }
            unreliable = &unreliable[..unreliable.len() / 2];
        }
        while !entities.is_empty() {
            let attempt = encode(reliable, unreliable, &entities)?;
            if attempt.len() <= MAX_PAYLOAD {
                payload = attempt;
                break;
            }
            entities.truncate(entities.len() / 2);
        }
    }
    let own = if owner_correction && !reliable_only {
        movements.last()
    } else {
        None
    };
    if let Some(own) = own {
        let prefix = queued.len();
        if prefix <= entities.len() && !entities.iter().any(|record| std::ptr::eq(*record, own)) {
            let mut keep = entities.len().min(MAX_ENTITY_RECORDS - 1);
            while keep >= prefix {
                let mut records = entities[..keep].to_vec();
                records.push(own);
                let Ok(attempt) = encode(reliable, unreliable, &records) else {
                    break; // Preserve the already validated packet.
                };
                if attempt.len() <= MAX_PAYLOAD {
                    entities = records;
                    payload = attempt;
                    break;
                }
                if keep == prefix {
                    break;
                }
                keep = prefix + (keep - prefix) / 2;
            }
        }
    }
    if !reliable_only
        && had_queued_data
        && reliable.is_empty()
        && unreliable.is_empty()
        && entities.is_empty()
    {
        return Err(WorldError::QueuedDataDoesNotFit);
    }
    Ok(SelectedFrame {
        payload,
        reliable_count: reliable.len(),
        unreliable_count: unreliable.len(),
        entities,
    })
}

#[cfg(test)]
mod tests;
