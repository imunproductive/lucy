//! Shared statescript value, full-frame and chunk writers for build 104319.
//!
//! Client research (1.74; chunk reader 0x7FF7898A6450, frame 0x7FF78991E3C0):
//!
//! - A ch1 chunk is `w_u32 first | w_var span | bit fragmented | w_var size in
//!   bits | payload`; first 0 is a full frame. The client keeps the highest
//!   `last` (first + span) it applied and drops chunks at or below it, so an
//!   unchanged chunk can be sent again safely.
//! - The payload starts `bit lead | bit H`; with H (an owner frame) `bit C |
//!   w_var CmFD`, and in a delta also `w_var LBSS` (the body's length). An
//!   owner frame is skipped unless its CmFD is larger than the last one
//!   (unsigned, so 0 never applies).
//! - A full frame lists the instances (w_u16 index steps, each with a
//!   descriptor (0x7FF78991ADC0): 16 bits graph, bit creator -> entity id,
//!   bit flag, bit parent -> w_var parent instance and w_var parent state
//!   index, bit references), then the entity's variables, then per instance
//!   its variables (a presence bit per m_syncVars entry with an identifier
//!   and instance scope, 0x7FF78991C270), one bit per state, and in an owner
//!   frame its event list; then the frame's event list. The creator is the
//!   instance's `#27`; without the bit it is the frame's own entity
//!   (0x7FF789C1D880). It destroys the network instances it does not list
//!   and first clears the variables the network set. The dispatcher makes
//!   new instances and destroys gone ones first (0x7FF78991EE10); an
//!   instance of the same id, graph, parent and `#27` is kept
//!   (0x7FF78991F200).
//! - Owner-frame states are the graph's m_states minus client-only and
//!   server-only ones (an entry DataTool could not read counts as client-only;
//!   the only one, 0D24 st75, sits under a client-only UI presenter). A
//!   SubScript state that is on carries its child's instance id (w_u16) and a
//!   bit. Instances the server makes never run their own Entry nodes (only
//!   client-only ones run), so the server sends what those would have done.
//! - A delta lists only what changed: instances (w_u16 index steps; a bit
//!   "gone", else a bit "full descriptor follows"), the entity variables, per
//!   listed instance a mask over its m_syncVars entries plus an id list
//!   (0x7FF78991CA20; an entry without an id gives variable id 0), then a mask
//!   over all its m_states (0x7FF78991C620): a LogicalButton / WeaponVolley /
//!   Effect state gets X then StAc and its payload, every other state A and
//!   its payload when A = 1. Then, in an owner frame, the event lists (EIns),
//!   "more" and the frame's events. A delta may name, without a descriptor,
//!   only instances the client has a network record of; otherwise it refuses
//!   the frame (0x7FF78991C510), rolling it back and retrying every tick.
//! - Masks (0x7FF789C630E0): up to 8 entries as plain bits, else one bit per
//!   group of 8 entries and the groups that have a bit set. signed_u16 is a
//!   sign then w_u16 (0x7FF789C63290); a count is `0 / 10 / 110 / 111 + w_u16`
//!   (0x7FF789C63400); a tagged value is read at 0x7FF789C67DE0.
//! - A variable is a tagged value and its bindings (0x7FF78991D4F0): the
//!   states whose output it takes while they are active (Stack entries), each
//!   with its instance, state index (m_statesBitCount bits), output slot and
//!   priority. Short form bit 0: the client takes the instance from the
//!   variable's bag (0x7FF78991D6DB). A variable is read at 0x7FF78991D2C0 and
//!   resolved by (instance id, low 16 bits of the id) at 0x7FF78AAB3150; the
//!   value is stored by 0x7FF78AAB78A0. The client crashes when there is none
//!   (id 0, or an instance it lacks) and it uses it anyway, and on a listed
//!   instance, binding or event list naming an instance it lacks
//!   (0x7FF78991CA20, 0x7FF78991D6E5, 0x7FF78991BA30).
//! - The linked-state field (0x7FF78AA837D0): a bit; with it the linked
//!   instance's id and its state index in that graph's width (m_statesBitCount
//!   in an owner frame, else a remote-sync index). An unknown id crashes the
//!   client (0x7FF78AA8385A).
//! - An owner event list (0x7FF78991BA30) has 2-bit codes, times from 16 *
//!   CmFD on, each the step from the one before. A last tick t means
//!   `lastTick = 16 * (CmFD + 1) - t` on the client (0x7FF78AAA8400). Effect
//!   sends its activation counter & 3 (0x7FF789894540); ClientOnlyPulser is
//!   0x7FF78990D760.
//! - H = 0 frames (0x7FF78991BE30 / 0x7FF78991C620) walk the graph's
//!   m_remoteSyncNodes entries that are states (the client casts each to the
//!   state base; actions fail) and neither client-only nor server-only. In a
//!   full frame each networked state has X: 0 aborts it, 1 then A (StEC: Y,
//!   then StAc) and, when on, its payload with H = 0, full = 1. A delta has a
//!   mask over the whole raw list and per set bit A (StEC: X, then StAc) and
//!   the payload with full = 0. After a full frame the client aborts every
//!   active state of the instance that is neither remote-synced nor
//!   client-only.
//! - The client builds a body's initial graphs itself on its create
//!   (0x7FF789AF9DE0; the networked create, component vt+0x78, gives each the
//!   highest id + 1: 1..N in definition order).

use std::{collections::BTreeMap, error::Error, fmt};

use indexmap::IndexMap;

use crate::graph::{Graph, ModeValue};
use orpheus_core::bits::{BitError, BitReader, BitWriter};

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Integer(i64),
    Float(f64),
    Asset(u64),
    Entity(u32),
    Vector([f64; 3]),
    Handle {
        kind: u8,
        instance: u32,
        state: u16,
        extra: u16,
    },
    String(String),
    Array(Vec<Value>),
    /// A map built by an expression (opcode 19): integer keys. On the wire
    /// the keyed type-13 array, tag 15 subtype 6 with the keyed bit, read at
    /// 104319 RVA D57430 (leafall903/ORPHEUS (MIT), `statescript.py`
    /// `_write_keyed_array`, `docs/COMMUNICATION_WHEEL.txt`).
    Map(Vec<(Value, Value)>),
    /// Authored typed pairs (`STU_7757D403`): resource keys and values in
    /// authored order. On the wire the type-12 pair array, tag 15 subtype
    /// 7, read at 104319 RVA D57899 (upstream `_write_pair_array`).
    Pairs(Vec<(Value, Value)>),
}

impl From<ModeValue> for Value {
    fn from(value: ModeValue) -> Self {
        match value {
            ModeValue::Bool(value) => Self::Bool(value),
            ModeValue::Integer(value) => Self::Integer(value),
            ModeValue::Asset(value) => Self::Asset(value),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WireError {
    Bit(BitError),
    ZeroCommandFrame,
    InstanceId,
    GraphIndex(u32),
    SlotOutOfRange,
    InvalidVariable,
    InvalidBinding,
    InvalidValue,
    ChunkRange,
}

impl fmt::Display for WireError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Bit(error) => error.fmt(f),
            Self::ZeroCommandFrame => f.write_str("an owner frame needs a nonzero command frame"),
            Self::InstanceId => f.write_str("script instance IDs must be distinct and positive"),
            Self::GraphIndex(index) => write!(f, "script graph {index} exceeds the wire index"),
            Self::SlotOutOfRange => f.write_str("script slot is outside the graph layout"),
            Self::InvalidVariable => f.write_str("zero terminates the script variable list"),
            Self::InvalidBinding => {
                f.write_str("script binding is outside its wire representation")
            }
            Self::InvalidValue => f.write_str("script value is outside its wire representation"),
            Self::ChunkRange => f.write_str("script chunk range or length exceeds the wire width"),
        }
    }
}

impl Error for WireError {}

impl From<BitError> for WireError {
    fn from(value: BitError) -> Self {
        Self::Bit(value)
    }
}

#[derive(Clone, Copy)]
pub enum ValueEncoding {
    Explicit,
    Compact,
}

impl ValueEncoding {
    fn compact(self) -> bool {
        matches!(self, Self::Compact)
    }
}

pub fn write_value(
    out: &mut BitWriter,
    value: &Value,
    encoding: ValueEncoding,
) -> Result<(), WireError> {
    let compact = encoding.compact();
    match value {
        Value::Bool(value) => {
            out.bits(0, 4)?;
            out.bit(*value);
        }
        Value::Integer(value) if (i64::from(i32::MIN)..=i64::from(u32::MAX)).contains(value) => {
            let (tag, width) = if compact && (0..16).contains(value) {
                (1, 4)
            } else if compact && i8::try_from(*value).is_ok() {
                (2, 8)
            } else if compact && i16::try_from(*value).is_ok() {
                (3, 16)
            } else {
                (4, 32)
            };
            out.bits(tag, 4)?;
            out.signed(*value, width)?;
        }
        Value::Float(value) if (*value as f32).is_finite() => {
            if compact && value.fract() == 0.0 && (0.0..16.0).contains(value) {
                out.bits(5, 4)?;
                out.bits(*value as u64, 4)?;
            } else if compact && value.fract() == 0.0 && (-128.0..128.0).contains(value) {
                out.bits(6, 4)?;
                out.signed(*value as i64, 8)?;
            } else {
                out.bits(8, 4)?;
                out.f32(*value as f32);
            }
        }
        Value::Asset(guid) => {
            let kind = guid >> 48;
            out.bits(9, 4)?;
            out.bit(true);
            out.bits(kind, 16)?;
            if kind != 0 {
                out.w_var(*guid as u32);
            }
        }
        Value::Entity(entity) => {
            out.bits(13, 4)?;
            let present = !compact || *entity != 0;
            out.bit(present);
            if present {
                out.bits(u64::from(*entity), 32)?;
            }
        }
        Value::Vector(parts)
            if compact && parts.iter().all(|value| (*value as f32).is_finite()) =>
        {
            out.bits(12, 4)?;
            for &part in parts {
                out.bit(part != 0.0);
            }
            for &part in parts {
                if part != 0.0 {
                    out.f32(part as f32);
                }
            }
        }
        Value::Handle {
            kind,
            instance,
            state,
            extra,
        } if compact => {
            out.bits(14, 4)?;
            out.bits(u64::from(kind & 7), 3)?;
            out.w_var(*instance);
            out.w_u16(*state);
            out.w_u16(*extra);
        }
        Value::Null if compact => {
            out.bits(15, 4)?;
            out.bits(3, 3)?;
        }
        Value::String(text) if compact && !text.contains('\0') => {
            out.bits(15, 4)?;
            out.bits(2, 3)?;
            out.raw(text.as_bytes());
            out.bits(0, 8)?;
        }
        Value::Array(items) if compact => {
            let length = u16::try_from(items.len()).map_err(|_| WireError::InvalidValue)?;
            out.bits(15, 4)?;
            out.bits(6, 3)?;
            out.w_u16(length);
            out.bit(false);
            if !items.is_empty() {
                out.bit(false);
                for item in items {
                    write_value(out, item, encoding)?;
                }
            }
        }
        Value::Map(entries) if compact => {
            let length = u16::try_from(entries.len()).map_err(|_| WireError::InvalidValue)?;
            out.bits(15, 4)?;
            out.bits(6, 3)?;
            out.w_u16(length);
            out.bit(true); // keyed
            if !entries.is_empty() {
                out.bit(false); // every value tagged
            }
            for (index, (key, value)) in entries.iter().enumerate() {
                let Value::Integer(key) = key else {
                    return Err(WireError::InvalidValue);
                };
                let key = u16::try_from(*key).map_err(|_| WireError::InvalidValue)?;
                if index != 0 {
                    out.bit(true); // later pairs have a presence bit
                }
                out.bits(u64::from(key), 16)?;
                write_value(out, value, encoding)?;
            }
        }
        Value::Pairs(entries) if compact => {
            let length = u16::try_from(entries.len()).map_err(|_| WireError::InvalidValue)?;
            out.bits(15, 4)?;
            out.bits(7, 3)?;
            out.w_u16(length);
            out.bit(false); // keys are values, not uint16 indexes
            if !entries.is_empty() {
                out.bit(false); // each key and value has its own tag
            }
            for (key, value) in entries {
                write_value(out, key, encoding)?;
                write_value(out, value, encoding)?;
            }
        }
        _ => return Err(WireError::InvalidValue),
    }
    Ok(())
}

pub type Variables = IndexMap<u32, Value>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Binding {
    pub instance: u16,
    pub state: u16,
    pub state_bits: u8,
    pub slot: u8,
    pub priority: i8,
    pub above: bool,
}

pub(crate) type Bindings = BTreeMap<u32, Vec<Binding>>;

pub(crate) fn variable(
    out: &mut BitWriter,
    value: &Value,
    encoding: ValueEncoding,
) -> Result<(), WireError> {
    bound_variable(out, value, encoding, 0, &[])
}

pub(crate) fn variable_list(
    out: &mut BitWriter,
    variables: &Variables,
    encoding: ValueEncoding,
) -> Result<(), WireError> {
    bound_variable_list(out, variables, encoding, 0, &Bindings::new())
}

pub(crate) fn bound_variable(
    out: &mut BitWriter,
    value: &Value,
    encoding: ValueEncoding,
    own: u16,
    bindings: &[Binding],
) -> Result<(), WireError> {
    write_value(out, value, encoding)?;
    match bindings.len() {
        0 => out.bit(false),
        1 => out.bits(0b01, 2)?,
        2 => out.bits(0b011, 3)?,
        count => {
            let count = u16::try_from(count).map_err(|_| WireError::InvalidBinding)?;
            out.bits(0b111, 3)?;
            out.w_u16(count);
        }
    }
    for binding in bindings {
        if binding.instance == 0
            || binding.state_bits > u16::BITS as u8
            || u32::from(binding.state) >= 1 << binding.state_bits
        {
            return Err(WireError::InvalidBinding);
        }
        out.bit(false); // no path
        out.bit(binding.instance != own);
        if binding.instance != own {
            out.bit(false); // positive instance ID
            out.w_u16(binding.instance);
        }
        out.bits(u64::from(binding.state), usize::from(binding.state_bits))?;
        out.bit(binding.slot != 0);
        if binding.slot != 0 {
            out.w_var(u32::from(binding.slot));
        }
        let explicit = binding.priority != 0 || !binding.above;
        out.bit(explicit);
        if explicit {
            out.bit(true); // integer priority
            out.bit(binding.priority < 0);
            out.w_var(u32::from(binding.priority.unsigned_abs()));
            out.bit(binding.above);
        }
        out.bit(false); // no per-frame re-resolution
    }
    Ok(())
}

pub(crate) fn bound_variable_list(
    out: &mut BitWriter,
    variables: &Variables,
    encoding: ValueEncoding,
    own: u16,
    bindings: &Bindings,
) -> Result<(), WireError> {
    out.bit(!variables.is_empty());
    if !variables.is_empty() {
        let mut emit = |id, value: &Value| {
            if id == 0 {
                return Err(WireError::InvalidVariable);
            }
            out.w_var(id);
            let links = bindings.get(&id).map_or(&[][..], Vec::as_slice);
            bound_variable(out, value, encoding, own, links)
        };
        if encoding.compact() {
            let mut ids: Vec<_> = variables.keys().copied().collect();
            ids.sort_unstable();
            for id in ids {
                emit(id, &variables[&id])?;
            }
        } else {
            for (&id, value) in variables {
                emit(id, value)?;
            }
        }
        out.w_var(0);
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Layout {
    pub graph: u16,
    pub owner_slots: usize,
    pub remote_slots: usize,
    pub presence_slots: usize,
}

impl TryFrom<&Graph> for Layout {
    type Error = WireError;

    fn try_from(graph: &Graph) -> Result<Self, Self::Error> {
        Ok(Self {
            graph: u16::try_from(graph.index).map_err(|_| WireError::GraphIndex(graph.index))?,
            owner_slots: graph.owner_positions().len(),
            remote_slots: graph.remote_positions().len(),
            presence_slots: graph.presence_vars().count(),
        })
    }
}

pub struct Instance {
    pub id: u16,
    pub layout: Layout,
    /// The entity it was created with (its `#27`) when that isn't the
    /// entity the frame is for.
    pub creator: Option<u32>,
    /// Parent instance and raw state index.
    pub parent: Option<(u16, u16)>,
    /// The scope state that opened it, for a scope instance.
    pub scope: Option<ScopeDescriptor>,
    pub presence: BTreeMap<usize, Value>,
    pub variables: Variables,
    /// Owner entries contain the payload after the active bit. Remote entries
    /// contain the complete state record, including its presence and active bits.
    pub states: BTreeMap<usize, BitWriter>,
}

impl Instance {
    pub fn new(id: u16, layout: Layout, parent: Option<(u16, u16)>) -> Self {
        Self {
            id,
            layout,
            creator: None,
            parent,
            scope: None,
            presence: BTreeMap::new(),
            variables: Variables::new(),
            states: BTreeMap::new(),
        }
    }
}

/// A scope instance's link to the scope state that opened it on another
/// entity (`STU_550D0C5C`, `vm/scopes.rs`): the scope's id (the state's
/// `m_C41A9BE2.m_5AEA0A51`) and the state's entity, instance and raw index.
/// The client keeps the id in the instance (`+8`, which picks the entries
/// and the scope plug that run in it) and the state as the instance's
/// parent reference (`+808`; `sub_7FF789C1F140`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ScopeDescriptor {
    pub scope: u32,
    pub entity: u32,
    pub instance: u16,
    pub state: u16,
}

/// An instance descriptor, as the client reads it (0x7FF78991ADC0). The
/// entity after the first bit becomes the instance's `#27` when the client
/// creates it (0x7FF78991F3D8 → `sub_7FF789887F20` → `sub_7FF78AAA4B10`);
/// without the bit the client takes the frame's own entity. It also keeps a
/// predicted instance only when that one's `#27` is the same
/// (0x7FF78991F2C7). So `creator` is the entity it was created with when
/// that is another entity (an aura's applier), else none. After the parent
/// comes the references bit: a scope id (w_var, the pending record's `+0xC`,
/// which becomes the instance's `+8` at `0x7FF78991F3E6`), a count (w_u16)
/// and per reference an entity id, then, when that is valid, its instance
/// and raw state (two w_var), copied into the instance's parent references
/// (`+808`, `0x7FF78991F41B`).
pub(crate) fn write_descriptor(
    out: &mut BitWriter,
    graph: u16,
    creator: Option<u32>,
    parent: Option<(u16, u16)>,
    scope: Option<ScopeDescriptor>,
) -> Result<(), WireError> {
    out.bits(u64::from(graph), 16)?;
    out.bit(creator.is_some());
    if creator.is_some() {
        out.entity_id(creator);
    }
    out.bit(false); // flag clear
    out.bit(parent.is_some());
    if let Some((parent, state)) = parent {
        out.w_var(u32::from(parent));
        out.w_var(u32::from(state));
    }
    out.bit(scope.is_some());
    if let Some(scope) = scope {
        out.w_var(scope.scope);
        out.w_u16(1);
        out.entity_id(Some(scope.entity));
        out.w_var(u32::from(scope.instance));
        out.w_var(u32::from(scope.state));
    }
    Ok(())
}

pub fn owner_full(
    command_frame: u32,
    instances: &[Instance],
    variables: &Variables,
) -> Result<BitWriter, WireError> {
    if command_frame == 0 {
        return Err(WireError::ZeroCommandFrame);
    }
    full_frame(
        Some(command_frame),
        instances,
        variables,
        ValueEncoding::Explicit,
    )
}

pub fn remote_full(instances: &[Instance], variables: &Variables) -> Result<BitWriter, WireError> {
    full_frame(None, instances, variables, ValueEncoding::Compact)
}

pub fn variables_frame(variables: &Variables) -> Result<BitWriter, WireError> {
    full_frame(None, &[], variables, ValueEncoding::Explicit)
}

fn full_frame(
    command_frame: Option<u32>,
    instances: &[Instance],
    variables: &Variables,
    encoding: ValueEncoding,
) -> Result<BitWriter, WireError> {
    let owner = command_frame.is_some();
    let mut ordered: Vec<_> = instances.iter().collect();
    ordered.sort_unstable_by_key(|instance| instance.id);
    let mut out = BitWriter::new();
    out.bit(false);
    out.bit(owner);
    if let Some(command_frame) = command_frame {
        out.bit(false);
        out.w_var(command_frame);
    }
    let mut last = 0;
    for instance in &ordered {
        if instance.id <= last {
            return Err(WireError::InstanceId);
        }
        out.w_u16(instance.id - last);
        last = instance.id;
        write_descriptor(
            &mut out,
            instance.layout.graph,
            instance.creator,
            instance.parent,
            instance.scope,
        )?;
    }
    out.w_u16(0);
    variable_list(&mut out, variables, encoding)?;
    for instance in ordered {
        let states = if owner {
            instance.layout.owner_slots
        } else {
            instance.layout.remote_slots
        };
        if instance
            .presence
            .last_key_value()
            .is_some_and(|(&slot, _)| slot >= instance.layout.presence_slots)
            || instance
                .states
                .last_key_value()
                .is_some_and(|(&slot, _)| slot >= states)
        {
            return Err(WireError::SlotOutOfRange);
        }
        for slot in 0..instance.layout.presence_slots {
            let value = instance.presence.get(&slot);
            out.bit(value.is_some());
            if let Some(value) = value {
                variable(&mut out, value, encoding)?;
            }
        }
        variable_list(&mut out, &instance.variables, encoding)?;
        for slot in 0..states {
            match instance.states.get(&slot) {
                Some(payload) => {
                    if owner {
                        out.bit(true);
                    } else if payload.bit_len() == 0 {
                        return Err(WireError::InvalidValue);
                    }
                    out.append(payload);
                }
                None => out.bit(false),
            }
        }
        if owner {
            out.bits(3, 2)?; // end instance events
        }
    }
    out.bits(0, 2)?; // no frame events
    Ok(out)
}

/// The head of a full frame as the client reads it: the owner frame's
/// CmFD (none for an observer's), the instances (id, graph, parent instance
/// and raw state), the entity's variables and their Stack links (instance,
/// raw state: [`bound_variable`]'s form, which names the state in its
/// graph's `m_statesBitCount` bits), and the bit where the instances'
/// sections begin. For tests and logs: a value [`read_value`] can't read,
/// or a link with a path, priority form or re-resolution the writer never
/// sends, is an error.
#[derive(Clone, Debug, PartialEq)]
pub struct FullHead {
    pub command_frame: Option<u32>,
    pub instances: Vec<ListedInstance>,
    pub variables: Variables,
    pub links: BTreeMap<u32, Vec<(u16, u16)>>,
    /// The bit after the variables: the first instance's presence slots.
    pub end: usize,
}

/// An instance a full frame lists: id, graph, creator (none: the frame's
/// entity) and parent (instance, raw state).
pub type ListedInstance = (u16, u16, Option<u32>, Option<(u32, u32)>);

/// `states_bits` gives a graph's `m_statesBitCount`, for the states the
/// entity variables' Stack links name.
pub fn read_full_head(
    bytes: &[u8],
    states_bits: impl Fn(u16) -> Option<u8>,
) -> Result<FullHead, WireError> {
    let mut bits = BitReader::new(bytes);
    if bits.bit()? {
        return Err(WireError::InvalidValue);
    }
    let command_frame = if bits.bit()? {
        bits.bit()?;
        Some(bits.w_var()?)
    } else {
        None
    };
    let mut instances = Vec::new();
    let mut id = 0_u16;
    loop {
        let step = bits.w_u16()?;
        if step == 0 {
            break;
        }
        id = id.checked_add(step).ok_or(WireError::InstanceId)?;
        let graph = bits.bits(16)? as u16;
        let creator = if bits.bit()? { bits.entity_id()? } else { None };
        bits.bit()?;
        let parent = if bits.bit()? {
            Some((bits.w_var()?, bits.w_var()?))
        } else {
            None
        };
        // A scope instance's references (scope id, count, then each
        // entity and, for a valid one, its instance and state).
        if bits.bit()? {
            bits.w_var()?;
            for _ in 0..bits.w_u16()? {
                if bits.entity_id()?.is_some() {
                    bits.w_var()?;
                    bits.w_var()?;
                }
            }
        }
        instances.push((id, graph, creator, parent));
    }
    let mut variables = Variables::new();
    let mut links = BTreeMap::new();
    if bits.bit()? {
        loop {
            let variable = bits.w_var()?;
            if variable == 0 {
                break;
            }
            let value = read_value(&mut bits)?;
            let bound = read_links(&mut bits, &instances, &states_bits)?;
            if !bound.is_empty() {
                links.insert(variable, bound);
            }
            variables.insert(variable, value);
        }
    }
    Ok(FullHead {
        command_frame,
        instances,
        variables,
        links,
        end: bits.position(),
    })
}

/// A tagged value as [`write_value`] writes it, in either encoding. For
/// tests and logs: a map's or pair array's untagged form, which the writer
/// never sends, is an error.
pub fn read_value(bits: &mut BitReader<'_>) -> Result<Value, WireError> {
    let f32_value = |bits: &mut BitReader<'_>| bits.f32().map(f64::from);
    Ok(match bits.bits(4)? {
        0 => Value::Bool(bits.bit()?),
        1 => Value::Integer(bits.signed(4)?),
        2 => Value::Integer(bits.signed(8)?),
        3 => Value::Integer(bits.signed(16)?),
        4 => Value::Integer(bits.signed(32)?),
        5 => Value::Float(bits.bits(4)? as f64),
        6 => Value::Float(bits.signed(8)? as f64),
        8 => Value::Float(f32_value(bits)?),
        9 => {
            bits.bit()?;
            let kind = bits.bits(16)?;
            let index = if kind == 0 {
                0
            } else {
                u64::from(bits.w_var()?)
            };
            Value::Asset(kind << 48 | index)
        }
        12 => {
            let present = [bits.bit()?, bits.bit()?, bits.bit()?];
            let mut parts = [0.0; 3];
            for (part, present) in parts.iter_mut().zip(present) {
                if present {
                    *part = f32_value(bits)?;
                }
            }
            Value::Vector(parts)
        }
        13 => Value::Entity(if bits.bit()? {
            bits.bits(32)? as u32
        } else {
            0
        }),
        14 => Value::Handle {
            kind: bits.bits(3)? as u8,
            instance: bits.w_var()?,
            state: bits.w_u16()?,
            extra: bits.w_u16()?,
        },
        15 => match bits.bits(3)? {
            2 => {
                let mut text = Vec::new();
                loop {
                    match bits.bits(8)? as u8 {
                        0 => break,
                        byte => text.push(byte),
                    }
                }
                Value::String(String::from_utf8(text).map_err(|_| WireError::InvalidValue)?)
            }
            3 => Value::Null,
            subtype @ (6 | 7) => {
                let length = bits.w_u16()?;
                let keyed = bits.bit()?;
                // Every item tagged.
                if length != 0 && bits.bit()? {
                    return Err(WireError::InvalidValue);
                }
                match (subtype, keyed) {
                    (6, false) => Value::Array(
                        (0..length)
                            .map(|_| read_value(bits))
                            .collect::<Result<_, _>>()?,
                    ),
                    (6, true) => Value::Map(
                        (0..length)
                            .map(|index| {
                                // Later pairs have a presence bit.
                                if index != 0 && !bits.bit()? {
                                    return Err(WireError::InvalidValue);
                                }
                                let key = Value::Integer(bits.bits(16)? as i64);
                                Ok((key, read_value(bits)?))
                            })
                            .collect::<Result<_, _>>()?,
                    ),
                    (7, false) => Value::Pairs(
                        (0..length)
                            .map(|_| Ok((read_value(bits)?, read_value(bits)?)))
                            .collect::<Result<_, WireError>>()?,
                    ),
                    _ => return Err(WireError::InvalidValue),
                }
            }
            _ => return Err(WireError::InvalidValue),
        },
        _ => return Err(WireError::InvalidValue),
    })
}

/// An entity variable's Stack links, as [`bound_variable`] writes them with
/// no own instance: (instance, raw state) each.
fn read_links(
    bits: &mut BitReader<'_>,
    instances: &[ListedInstance],
    states_bits: &impl Fn(u16) -> Option<u8>,
) -> Result<Vec<(u16, u16)>, WireError> {
    let count = if !bits.bit()? {
        0
    } else if !bits.bit()? {
        1
    } else if !bits.bit()? {
        2
    } else {
        bits.w_u16()?
    };
    (0..count)
        .map(|_| {
            // No path; another instance than the frame's own (an entity
            // variable has none), by a positive id.
            if bits.bit()? || !bits.bit()? || bits.bit()? {
                return Err(WireError::InvalidBinding);
            }
            let instance = bits.w_u16()?;
            let width = instances
                .iter()
                .find(|&&(id, ..)| id == instance)
                .and_then(|&(_, graph, ..)| states_bits(graph))
                .ok_or(WireError::InvalidBinding)?;
            let state = bits.bits(usize::from(width))? as u16;
            if bits.bit()? {
                bits.w_var()?; // the slot
            }
            if bits.bit()? {
                // An integer priority: sign, magnitude, above-equals.
                if !bits.bit()? {
                    return Err(WireError::InvalidBinding);
                }
                bits.bit()?;
                bits.w_var()?;
                bits.bit()?;
            }
            if bits.bit()? {
                return Err(WireError::InvalidBinding);
            }
            Ok((instance, state))
        })
        .collect()
}

pub fn subscript(child: u16) -> BitWriter {
    let mut out = BitWriter::new();
    out.w_u16(child);
    out.bit(false);
    out
}

pub fn owner_ack(command_frame: u32) -> Result<BitWriter, WireError> {
    if command_frame == 0 {
        return Err(WireError::ZeroCommandFrame);
    }
    let mut out = BitWriter::new();
    out.bits(2, 3)?;
    out.w_var(command_frame);
    out.w_var(7); // body length through the "more" bit
    out.w_u16(0); // no instances
    out.bits(0, 5)?; // variables, instance events, more, frame events
    Ok(out)
}

pub fn chunk(payload: &BitWriter, first: u32, span: u32) -> Result<BitWriter, WireError> {
    first.checked_add(span).ok_or(WireError::ChunkRange)?;
    let width = u32::try_from(payload.bit_len()).map_err(|_| WireError::ChunkRange)?;
    let mut out = BitWriter::new();
    out.w_u32(first);
    out.w_var(span);
    out.bit(false);
    out.w_var(width);
    out.append(payload);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn check(bits: &BitWriter, count: usize, expected: &str) {
        let actual: String = bits
            .as_bytes()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        assert_eq!(bits.bit_len(), count);
        assert_eq!(actual, expected);
    }

    #[test]
    fn tagged_values_match_the_python_writer() {
        let values = [
            Value::Bool(true),
            Value::Integer(-23),
            Value::Integer(3),
            Value::Integer(132),
            Value::Integer(65_540),
            Value::Float(4.0),
            Value::Float(-3.0),
            Value::Float(1.5),
            Value::Asset(0x0280_0000_0000_001c),
            Value::Entity(0),
            Value::Entity(0xa000_0001),
            Value::Array(vec![
                Value::Null,
                Value::String("Rust".into()),
                Value::Bool(false),
            ]),
            Value::Handle {
                kind: 3,
                instance: 20,
                state: 10,
                extra: 4,
            },
            Value::Vector([0.0, -1.0, 2.0]),
        ];
        let mut bits = BitWriter::new();
        for value in &values {
            write_value(&mut bits, value, ValueEncoding::Compact).unwrap();
        }
        check(
            &bits,
            379,
            "50d263861080080002008aac1f0100807f32a0008ed603000040df86df4baa6e8e0e00f85050c4060000fc0500000002",
        );
        // The keyed and pair arrays, then every value read back.
        let arrays = [
            Value::Map(vec![
                (Value::Integer(7), Value::Bool(true)),
                (Value::Integer(300), Value::Array(Vec::new())),
            ]),
            Value::Pairs(vec![(
                Value::Asset(0x0d60_0000_0000_0310),
                Value::Integer(2),
            )]),
        ];
        for value in &arrays {
            write_value(&mut bits, value, ValueEncoding::Compact).unwrap();
        }
        let mut reader = BitReader::new(bits.as_bytes());
        for value in values.iter().chain(&arrays) {
            assert_eq!(read_value(&mut reader).as_ref(), Ok(value));
        }
        assert_eq!(reader.position(), bits.bit_len());
    }

    /// Read in the client's order (0x7FF78991ADC0): graph, the creator bit
    /// and entity id (0x7FF789CC0F70: 2 type bits, then 8, 16 or 32 bits of
    /// index), the flag, the parent, the references: a scope instance's
    /// scope id, one reference and its entity, instance and state.
    #[test]
    fn a_descriptor_carries_a_creator_the_way_the_client_reads_it() {
        let mut out = BitWriter::new();
        write_descriptor(&mut out, 0x0c72, Some(0xa000_0202), Some((1, 43)), None).unwrap();
        let scope = ScopeDescriptor {
            scope: 12,
            entity: 0x8000_010b,
            instance: 1,
            state: 1,
        };
        write_descriptor(&mut out, 0x265f, Some(0x8000_010b), None, Some(scope)).unwrap();
        let mut bits = BitReader::new(out.as_bytes());
        assert_eq!(bits.bits(16), Ok(0x0c72));
        assert_eq!(bits.bit(), Ok(true));
        assert_eq!(bits.bits(2), Ok(1));
        assert_eq!((bits.bit(), bits.bit()), (Ok(true), Ok(false)));
        assert_eq!(bits.bits(16), Ok(0x202));
        assert_eq!(bits.bit(), Ok(false));
        assert_eq!(bits.bit(), Ok(true));
        assert_eq!((bits.w_var(), bits.w_var()), (Ok(1), Ok(43)));
        assert_eq!(bits.bit(), Ok(false));
        assert_eq!(bits.bits(16), Ok(0x265f));
        assert_eq!(bits.bit(), Ok(true));
        assert_eq!(bits.entity_id(), Ok(Some(0x8000_010b)));
        assert_eq!((bits.bit(), bits.bit()), (Ok(false), Ok(false)));
        assert_eq!(bits.bit(), Ok(true));
        assert_eq!((bits.w_var(), bits.w_u16()), (Ok(12), Ok(1)));
        assert_eq!(bits.entity_id(), Ok(Some(0x8000_010b)));
        assert_eq!((bits.w_var(), bits.w_var()), (Ok(1), Ok(1)));
        assert_eq!(bits.position(), out.bit_len());
    }

    #[test]
    fn full_owner_and_ack_match_python_with_an_active_child() {
        let variables = Variables::from([
            (12, Value::Bool(true)),
            (37, Value::Integer(-23)),
            (42, Value::Float(1.5)),
            (77, Value::Asset(0x0280_0000_0000_001c)),
            (105, Value::Entity(0xa000_0101)),
        ]);
        check(
            &variables_frame(&variables).unwrap(),
            213,
            "8018202594feffff8f0a020000ffd09401057090d6030200400100",
        );
        let mut parent = Instance::new(
            1,
            Layout {
                graph: 0x13c1,
                owner_slots: 2,
                remote_slots: 0,
                presence_slots: 2,
            },
            None,
        );
        parent.presence.insert(0, Value::Bool(false));
        parent.variables.insert(3, Value::Integer(8));
        parent.states.insert(0, subscript(2));
        parent.states.insert(1, BitWriter::new());
        let child = Instance::new(
            2,
            Layout {
                graph: 0x288a,
                owner_slots: 1,
                remote_slots: 0,
                presence_slots: 0,
            },
            Some((1, 43)),
        );
        check(
            &owner_full(1001, &[child, parent], &variables).unwrap(),
            378,
            "2a7d40044f805044a1002b400c90124affffff47050100807f68ca80023848eb010100a000043440080000000024ce00",
        );
        let ack = owner_ack(1001).unwrap();
        check(&ack, 40, "2a7dc00100");
        check(&chunk(&ack, 2, 7).unwrap(), 76, "04001c80a2d2071c0000");
    }

    #[test]
    fn rejects_out_of_range_inputs_without_emitting_a_complete_frame() {
        let variables = Variables::from([(0, Value::Bool(true))]);
        assert_eq!(variables_frame(&variables), Err(WireError::InvalidVariable));
        assert_eq!(owner_ack(0), Err(WireError::ZeroCommandFrame));
        assert_eq!(
            chunk(&BitWriter::new(), u32::MAX, 1),
            Err(WireError::ChunkRange)
        );
        let layout = Layout {
            graph: 1,
            owner_slots: 1,
            remote_slots: 1,
            presence_slots: 1,
        };
        let mut instance = Instance::new(1, layout, None);
        instance.presence.insert(1, Value::Bool(false));
        assert_eq!(
            owner_full(1, &[instance], &Variables::new()),
            Err(WireError::SlotOutOfRange)
        );
        let mut bits = BitWriter::new();
        assert_eq!(
            write_value(&mut bits, &Value::Integer(i64::MAX), ValueEncoding::Compact),
            Err(WireError::InvalidValue)
        );
    }
}
