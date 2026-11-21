//! Data-driven statescript payload layouts and verified runtime projections.
//!
//! Layouts and H0/H1 projections follow the Python reference. The prepared
//! runtime supplies the client table, extracted in IDA from the client code:
//! per state class (STU hash) the class the client builds for it (the
//! registry winner), whether it is an StEC class (X then StAc, else A), its
//! codec name and its payload layout (the property block's fields, then the
//! class reader's). Layout ops are read in order, LSB first: bits n, f32,
//! w_u16, var_b, signed_var, entity_id, value (a tagged value), link; `if c
//! then else` with c one of H, !H, full, H&full, a field read before, !field or
//! valid:field (a valid entity); `repeat count body`. H = an owner frame, full
//! = a full frame (a delta reads with full = 0). A state whose payload the
//! server cannot write is sent only while it is off, and never when it is an
//! StEC class (an StEC state carries its payload when off too). Reader widths
//! are documented in `orpheus_core::bits`.

use std::{
    collections::{BTreeMap, HashMap},
    error::Error,
    fmt, io,
    sync::atomic::{AtomicU64, Ordering},
};

use indexmap::IndexMap;
use serde::Deserialize;

use crate::graph::Node;
use crate::wire::{Value, ValueEncoding, WireError, write_value};
use orpheus_core::bits::{BitError, BitWriter};

pub type StatePayload = BTreeMap<String, Value>;
pub type LayoutFields<'a> = BTreeMap<&'a str, FieldValue<'a>>;

#[derive(Clone, Debug, PartialEq)]
pub enum FieldValue<'a> {
    Scalar(Value),
    Unsigned(u64),
    Rows(Vec<LayoutFields<'a>>),
}

impl From<Value> for FieldValue<'_> {
    fn from(value: Value) -> Self {
        Self::Scalar(value)
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct LayoutMode {
    pub owner: bool,
    pub full: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PayloadMode {
    Remote {
        full: bool,
    },
    Owner {
        full: bool,
        frame_time: i64,
        frame_ms: u32,
    },
}

impl PayloadMode {
    fn layout(self) -> LayoutMode {
        match self {
            Self::Remote { full } => LayoutMode { owner: false, full },
            Self::Owner { full, .. } => LayoutMode { owner: true, full },
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PayloadError {
    Bit(BitError),
    Wire(WireError),
    Unverified(String),
    Unsupported(String),
    Field(String),
    Rows {
        field: String,
        expected: usize,
        actual: usize,
    },
    TimeRange,
}

impl fmt::Display for PayloadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Bit(error) => error.fmt(f),
            Self::Wire(error) => error.fmt(f),
            Self::Unverified(class) => write!(f, "no verified payload layout for {class}"),
            Self::Unsupported(class) => write!(f, "no runtime payload projection for {class}"),
            Self::Field(name) => write!(f, "invalid state payload field {name:?}"),
            Self::Rows {
                field,
                expected,
                actual,
            } => {
                write!(f, "{field:?} needs {expected} records, received {actual}")
            }
            Self::TimeRange => f.write_str("state payload time is outside the supported range"),
        }
    }
}

impl Error for PayloadError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Bit(error) => Some(error),
            Self::Wire(error) => Some(error),
            _ => None,
        }
    }
}

impl From<BitError> for PayloadError {
    fn from(error: BitError) -> Self {
        Self::Bit(error)
    }
}

impl From<WireError> for PayloadError {
    fn from(error: WireError) -> Self {
        Self::Wire(error)
    }
}

#[derive(Debug)]
pub struct CodecTable {
    /// Unique to this table's content (0 for the client's), so a node's
    /// cached entry ([`Self::node_codec`]) is never read with another table.
    id: u64,
    entries: Vec<Codec>,
    index: HashMap<String, usize>,
}

impl CodecTable {
    /// The client's codec table. A debug build refuses a table with a
    /// verified layout whose codec has no projection (`Family`): an active
    /// state of that class could never be sent (a release build leaves it
    /// out of frames and logs it once, `snapshot.rs`).
    pub fn client() -> Self {
        let mut table = Self::from_json(orpheus_fixed::STATESCRIPT_CODECS)
            .expect("the embedded codec table parses");
        // Every client table is the same table.
        table.id = 0;
        if cfg!(debug_assertions) {
            let missing: Vec<_> = table
                .classes()
                .filter(|codec| codec.verified() && !codec.projected())
                .map(|codec| (&codec.hash, &codec.codec))
                .collect();
            assert!(
                missing.is_empty(),
                "client codecs with no payload family: {missing:?}"
            );
        }
        table
    }

    pub fn from_json(text: &str) -> io::Result<Self> {
        let raw: RawTable = serde_json::from_str(text)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        let mut entries = Vec::with_capacity(raw.classes.len());
        let mut index = HashMap::with_capacity(raw.classes.len() * 3);
        for (hash, row) in raw.classes {
            let ordinal = entries.len();
            for alias in [hash.clone(), row.name.clone(), format!("STU_{hash}")] {
                index.insert(alias, ordinal);
            }
            let family = row.codec.as_deref().and_then(Family::parse);
            entries.push(Codec {
                hash,
                name: row.name,
                codec: row.codec,
                layout: row.layout,
                stec: row.stec,
                status: row.status,
                family,
            });
        }
        static NEXT_ID: AtomicU64 = AtomicU64::new(1);
        Ok(Self {
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            entries,
            index,
        })
    }

    pub fn codec(&self, class: &str) -> Option<&Codec> {
        self.entries.get(*self.index.get(class)?)
    }

    /// The codec of `node`'s class, looked up once for each node (with the
    /// first table that asks; another table looks it up each time).
    pub fn node_codec(&self, node: &Node) -> Option<&Codec> {
        let &(table, entry) = node
            .codec_cell()
            .get_or_init(|| (self.id, self.index.get(&node.class).copied()));
        if table == self.id {
            entry.map(|entry| &self.entries[entry])
        } else {
            self.codec(&node.class)
        }
    }

    pub fn classes(&self) -> impl Iterator<Item = &Codec> {
        self.entries.iter()
    }

    /// An unprojected state can only be sent off if its class has no StEC
    /// payload on the off path.
    pub fn sendable(&self, class: &str, active: bool) -> bool {
        sendable(self.codec(class), active)
    }

    /// [`Self::sendable`] for `node`'s class.
    pub fn node_sendable(&self, node: &Node, active: bool) -> bool {
        sendable(self.node_codec(node), active)
    }
}

fn sendable(codec: Option<&Codec>, active: bool) -> bool {
    codec.map_or(!active, |codec| {
        codec.projected() || (!active && !codec.stec())
    })
}

#[derive(Debug)]
pub struct Codec {
    pub hash: String,
    pub name: String,
    pub codec: Option<String>,
    layout: Option<Vec<Op>>,
    stec: bool,
    status: String,
    family: Option<Family>,
}

impl Codec {
    pub fn verified(&self) -> bool {
        self.status == "VERIFIED" && self.layout.is_some()
    }

    pub fn empty(&self) -> bool {
        self.layout.as_ref().is_some_and(Vec::is_empty)
    }

    pub fn stec(&self) -> bool {
        self.stec
    }

    pub fn projected(&self) -> bool {
        self.verified() && self.family.is_some()
    }

    pub fn payload(
        &self,
        payload: &StatePayload,
        mode: PayloadMode,
    ) -> Result<BitWriter, PayloadError> {
        self.verified_layout()?;
        let family = self
            .family
            .ok_or_else(|| PayloadError::Unsupported(self.name.clone()))?;
        self.encode_layout(&project(family, payload, mode)?, mode.layout())
    }

    /// Write fields already expressed in the verified layout's names. Missing
    /// fields use the reference writer's zero, null or absent-link defaults.
    pub fn encode_layout(
        &self,
        fields: &LayoutFields<'_>,
        mode: LayoutMode,
    ) -> Result<BitWriter, PayloadError> {
        self.encode_layout_with_link(fields, mode, |out, _| {
            out.bit(false);
            Ok(())
        })
    }

    /// Linked-state references depend on the frame's instance context.
    pub fn encode_layout_with_link<'a>(
        &self,
        fields: &LayoutFields<'a>,
        mode: LayoutMode,
        mut link: impl FnMut(&mut BitWriter, Option<&FieldValue<'a>>) -> Result<(), PayloadError>,
    ) -> Result<BitWriter, PayloadError> {
        let mut out = BitWriter::new();
        write_ops(&mut out, self.verified_layout()?, fields, mode, &mut link)?;
        Ok(out)
    }

    fn verified_layout(&self) -> Result<&[Op], PayloadError> {
        self.layout
            .as_deref()
            .filter(|_| self.status == "VERIFIED")
            .ok_or_else(|| PayloadError::Unverified(self.name.clone()))
    }
}

#[derive(Deserialize)]
struct RawTable {
    classes: IndexMap<String, RawClass>,
}

#[derive(Deserialize)]
struct RawClass {
    name: String,
    codec: Option<String>,
    layout: Option<Vec<Op>>,
    stec: bool,
    status: String,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
enum Op {
    Bits {
        f: String,
        n: usize,
    },
    F32 {
        f: String,
    },
    #[serde(rename = "w_u16")]
    Wu16 {
        f: String,
    },
    VarB {
        f: String,
    },
    SignedVar {
        f: String,
    },
    EntityId {
        f: String,
    },
    Value {
        f: String,
    },
    Link {
        f: String,
    },
    If {
        c: String,
        #[serde(rename = "then")]
        yes: Vec<Op>,
        #[serde(rename = "else")]
        no: Vec<Op>,
    },
    Repeat {
        f: String,
        count: Count,
        body: Vec<Op>,
    },
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum Count {
    Fixed(usize),
    Field(String),
}

fn invalid_field(name: &str) -> PayloadError {
    PayloadError::Field(name.into())
}

fn value_integer(value: &Value) -> Option<i128> {
    match value {
        Value::Bool(flag) => Some(i128::from(*flag)),
        Value::Integer(number) => Some(i128::from(*number)),
        Value::Entity(entity) => Some(i128::from(*entity)),
        Value::Float(number) if (i128::MIN as f64..-(i128::MIN as f64)).contains(number) => {
            Some(*number as i128)
        }
        _ => None,
    }
}

fn layout_integer(fields: &LayoutFields<'_>, name: &str) -> Result<i128, PayloadError> {
    match fields.get(name) {
        None => Ok(0),
        Some(FieldValue::Unsigned(number)) => Ok(i128::from(*number)),
        Some(FieldValue::Scalar(value)) => value_integer(value).ok_or_else(|| invalid_field(name)),
        Some(FieldValue::Rows(_)) => Err(invalid_field(name)),
    }
}

fn value_float(value: &Value) -> Option<f64> {
    match value {
        Value::Float(value) => Some(*value),
        Value::Integer(value) => Some(*value as f64),
        Value::Bool(value) => Some(f64::from(u8::from(*value))),
        _ => None,
    }
}

fn layout_float(fields: &LayoutFields<'_>, name: &str) -> Result<f32, PayloadError> {
    let value = match fields.get(name) {
        None => 0.0,
        Some(FieldValue::Unsigned(number)) => *number as f64,
        Some(FieldValue::Scalar(value)) => value_float(value).ok_or_else(|| invalid_field(name))?,
        Some(FieldValue::Rows(_)) => return Err(invalid_field(name)),
    };
    let narrow = value as f32;
    if value.is_finite() && !narrow.is_finite() {
        return Err(invalid_field(name));
    }
    Ok(narrow)
}

fn value_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(value) => *value,
        Value::Integer(value) => *value != 0,
        Value::Float(value) => *value != 0.0,
        Value::Entity(value) => *value != 0,
        Value::String(value) => !value.is_empty(),
        Value::Array(value) => !value.is_empty(),
        _ => true,
    }
}

fn layout_truthy(value: Option<&FieldValue<'_>>) -> bool {
    match value {
        None => false,
        Some(FieldValue::Scalar(value)) => value_truthy(value),
        Some(FieldValue::Unsigned(value)) => *value != 0,
        Some(FieldValue::Rows(value)) => !value.is_empty(),
    }
}

fn condition(
    test: &str,
    fields: &LayoutFields<'_>,
    mode: LayoutMode,
) -> Result<bool, PayloadError> {
    Ok(match test {
        "H" => mode.owner,
        "!H" => !mode.owner,
        "full" => mode.full,
        "H&full" => mode.owner && mode.full,
        _ => {
            if let Some(name) = test.strip_prefix("valid:") {
                // No entity (null) is the invalid id, as `entity_id` writes it.
                let entity = match fields.get(name) {
                    Some(FieldValue::Scalar(Value::Null)) => 0,
                    _ => layout_integer(fields, name)?,
                };
                entity != 0 && entity & 0x1fff_ffff != 0x1fff_ffff
            } else if let Some(name) = test.strip_prefix('!') {
                !layout_truthy(fields.get(name))
            } else {
                layout_truthy(fields.get(test))
            }
        }
    })
}

type LinkWriter<'fields, 'call> =
    dyn FnMut(&mut BitWriter, Option<&FieldValue<'fields>>) -> Result<(), PayloadError> + 'call;

fn write_ops<'a>(
    out: &mut BitWriter,
    ops: &[Op],
    fields: &LayoutFields<'a>,
    mode: LayoutMode,
    link: &mut LinkWriter<'a, '_>,
) -> Result<(), PayloadError> {
    for op in ops {
        match op {
            Op::Bits { f, n } => {
                let mask = match *n {
                    0..=63 => (1_u64 << *n) - 1,
                    64 => u64::MAX,
                    _ => return Err(invalid_field(f)),
                };
                out.bits(layout_integer(fields, f)? as u64 & mask, *n)?;
            }
            Op::F32 { f } => out.f32(layout_float(fields, f)?),
            Op::Wu16 { f } => {
                out.w_u16(u16::try_from(layout_integer(fields, f)?).map_err(|_| invalid_field(f))?)
            }
            Op::VarB { f } => {
                out.var_b(u32::try_from(layout_integer(fields, f)?).map_err(|_| invalid_field(f))?)
            }
            Op::SignedVar { f } => {
                let value = layout_integer(fields, f)?;
                let absolute = u32::try_from(value.unsigned_abs()).map_err(|_| invalid_field(f))?;
                out.bit(value < 0);
                out.w_var(absolute);
            }
            Op::EntityId { f } => {
                let entity = match fields.get(f.as_str()) {
                    Some(FieldValue::Scalar(Value::Null)) => 0,
                    _ => u32::try_from(layout_integer(fields, f)?).map_err(|_| invalid_field(f))?,
                };
                out.entity_id((entity != 0).then_some(entity));
            }
            Op::Value { f } => {
                let number;
                let value = match fields.get(f.as_str()) {
                    None => &Value::Null,
                    Some(FieldValue::Scalar(value)) => value,
                    Some(FieldValue::Unsigned(value)) if *value <= u64::from(u32::MAX) => {
                        number = Value::Integer(*value as i64);
                        &number
                    }
                    _ => return Err(invalid_field(f)),
                };
                write_value(out, value, ValueEncoding::Compact)?;
            }
            Op::Link { f } => link(out, fields.get(f.as_str()))?,
            Op::If { c, yes, no } => {
                let branch = if condition(c, fields, mode)? { yes } else { no };
                write_ops(out, branch, fields, mode, link)?;
            }
            Op::Repeat { f, count, body } => {
                let count = match count {
                    Count::Fixed(count) => *count,
                    Count::Field(name) => usize::try_from(layout_integer(fields, name)?)
                        .map_err(|_| invalid_field(name))?,
                };
                let rows = match fields.get(f.as_str()) {
                    None | Some(FieldValue::Scalar(Value::Null)) => &[][..],
                    Some(FieldValue::Rows(rows)) => rows.as_slice(),
                    _ => return Err(invalid_field(f)),
                };
                if rows.len() < count {
                    return Err(PayloadError::Rows {
                        field: f.clone(),
                        expected: count,
                        actual: rows.len(),
                    });
                }
                for row in &rows[..count] {
                    write_ops(out, body, row, mode, link)?;
                }
            }
        }
    }
    Ok(())
}

#[derive(Clone, Copy, Debug)]
enum Family {
    None,
    Switch,
    DelayedSwitch,
    Stack,
    Anim,
    Chase,
    Button,
    Ability,
    Targets,
    Volley,
    Subscript,
    Message,
    Send,
    Link,
    Hits,
    Flags13,
    Frames,
    Counter2,
    Pulser,
    Start,
    Rewind,
    TwoVectors,
    Shockwave,
    ValueSlot,
    BlockU8,
    BlockU16,
    StackCounter,
    Bit,
    VarFlag,
    OptionalVar,
    Placement,
    LinkedTarget,
    Blink,
}

impl Family {
    fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "none" => Self::None,
            "switch" => Self::Switch,
            "two_bits" => Self::DelayedSwitch,
            "stack" => Self::Stack,
            "anim" => Self::Anim,
            "chase" => Self::Chase,
            "button" => Self::Button,
            "ability" => Self::Ability,
            "targets" => Self::Targets,
            "volley" => Self::Volley,
            "subscript" => Self::Subscript,
            "message" => Self::Message,
            "send" => Self::Send,
            "link" => Self::Link,
            "hits" => Self::Hits,
            "flags13" => Self::Flags13,
            "frames" => Self::Frames,
            "counter2" => Self::Counter2,
            "pulser" => Self::Pulser,
            "start" => Self::Start,
            "time16+s64" => Self::Rewind,
            "two_vectors+s64" => Self::TwoVectors,
            "shockwave" => Self::Shockwave,
            "value_slot" => Self::ValueSlot,
            "none+u8" => Self::BlockU8,
            "none+u16" => Self::BlockU16,
            "stack_counter" => Self::StackCounter,
            "bit" => Self::Bit,
            "239B1E64" => Self::VarFlag,
            "63B7ADFD" => Self::OptionalVar,
            "861D9015" => Self::Placement,
            "linked_target" => Self::LinkedTarget,
            "blink" => Self::Blink,
            _ => return None,
        })
    }
}

struct Vm<'a>(&'a StatePayload);

impl Vm<'_> {
    fn flag(&self, name: &str) -> bool {
        self.0.get(name).is_some_and(value_truthy)
    }

    fn optional_int(&self, name: &str) -> Result<Option<i64>, PayloadError> {
        match self.0.get(name) {
            None | Some(Value::Null) => Ok(None),
            Some(value) => value_integer(value)
                .and_then(|number| i64::try_from(number).ok())
                .map(Some)
                .ok_or_else(|| invalid_field(name)),
        }
    }

    fn integer(&self, name: &str, default: i64) -> Result<i64, PayloadError> {
        Ok(self.optional_int(name)?.unwrap_or(default))
    }

    fn float(&self, name: &str, default: f64) -> Result<f64, PayloadError> {
        self.0.get(name).map_or(Ok(default), |value| {
            value_float(value).ok_or_else(|| invalid_field(name))
        })
    }

    /// An unsigned integer up to `max` (0 when absent).
    fn unsigned(&self, name: &str, max: u32) -> Result<u32, PayloadError> {
        u32::try_from(self.integer(name, 0)?)
            .ok()
            .filter(|value| *value <= max)
            .ok_or_else(|| invalid_field(name))
    }

    /// A vector (zero when absent).
    fn vector(&self, name: &str) -> Result<[f32; 3], PayloadError> {
        match self.0.get(name) {
            None | Some(Value::Null) => Ok([0.0; 3]),
            Some(Value::Vector(parts)) => Ok(parts.map(|part| part as f32)),
            Some(_) => Err(invalid_field(name)),
        }
    }

    /// The 32 bits the client keeps for a value: a float's bits, or an
    /// unsigned integer (0 when absent).
    fn raw32(&self, name: &str) -> Result<i64, PayloadError> {
        match self.0.get(name) {
            Some(Value::Float(value)) => Ok(i64::from((*value as f32).to_bits())),
            _ => Ok(i64::from(self.unsigned(name, u32::MAX)?)),
        }
    }

    /// An entity id, 0 for none (null).
    fn entity(&self, name: &str) -> Result<u32, PayloadError> {
        self.unsigned(name, u32::MAX)
    }
}

/// Whether `entity` is an entity the client's `valid` test accepts.
fn valid_entity(entity: u32) -> bool {
    entity != 0 && entity & 0x1fff_ffff != 0x1fff_ffff
}

fn project(
    family: Family,
    payload: &StatePayload,
    mode: PayloadMode,
) -> Result<LayoutFields<'static>, PayloadError> {
    let vm = Vm(payload);
    let owner = mode.layout().owner;
    let mut fields = LayoutFields::new();
    match family {
        Family::None | Family::Send | Family::Link => {}
        Family::Switch => {
            fields.insert("current", Value::Bool(vm.flag("current")).into());
        }
        Family::DelayedSwitch => {
            fields.insert("a", Value::Bool(vm.flag("current")).into());
            fields.insert("b", Value::Bool(vm.flag("undecided")).into());
        }
        Family::Stack if owner => {
            fields.insert("top", Value::Bool(vm.flag("top")).into());
            fields.insert("under", Value::Bool(vm.flag("under")).into());
        }
        Family::Button | Family::Anim | Family::Counter2 => {
            fields.insert("counter", Value::Integer(vm.integer("counter", 0)?).into());
        }
        Family::Volley => project_volley(&mut fields, &vm, mode)?,
        Family::Chase => project_chase(&mut fields, &vm, mode)?,
        Family::Ability => project_ability(&mut fields, &vm, mode)?,
        Family::Targets if owner => project_targets(&mut fields, &vm)?,
        Family::Subscript if owner => {
            fields.insert("child", Value::Integer(vm.integer("child", 0)?).into());
        }
        Family::Message => {
            fields.insert("stacked", Value::Bool(vm.flag("stacked")).into());
        }
        Family::Hits => project_hits(&mut fields, &vm)?,
        Family::Flags13 if owner => {
            let value = vm.integer("value", 0)?;
            fields.insert("low", Value::Integer(value & 63).into());
            fields.insert("more", Value::Bool(value >> 6 != 0).into());
            fields.insert("high", Value::Integer((value >> 6) & 127).into());
        }
        // The reference H0 flags13 projection leaves low/more/high unset.
        Family::Stack | Family::Targets | Family::Subscript | Family::Flags13 => {}
        Family::Frames => {
            fields.insert("target", Value::Integer(vm.integer("target", 0)?).into());
        }
        Family::Pulser => {
            fields.insert("fresh", Value::Bool(vm.flag("fresh")).into());
            fields.insert("count", Value::Integer(vm.integer("count", 0)?).into());
        }
        Family::Start => project_start(&mut fields, &vm, mode)?,
        Family::Rewind => project_rewind(&mut fields, &vm)?,
        Family::TwoVectors => project_two_vectors(&mut fields, &vm)?,
        Family::Shockwave => project_shockwave(&mut fields, &vm)?,
        // ScaleTime (`STU_2F07848F`, StEC; `Write_STU_2F07848F`
        // 0x7FF789B97600): the state's active flag, then the value it keeps
        // for that side (`+0x1C8` while on, `+0x1CC` while off).
        Family::ValueSlot => {
            let active = vm.flag("active");
            let value = vm.raw32(if active { "on" } else { "off" })?;
            fields.insert("which", Value::Bool(active).into());
            fields.insert("value", Value::Integer(value).into());
        }
        // The property block's only sent field (`statescript_codecs.json`
        // `block`); the class reader is the base nullsub. `STU_43DB06B1`
        // and `STU_4C96470A` set their byte to 0 in their begin and to 1
        // from another slot (0x7FF7897F9990, 0x7FF7897F8AC0).
        Family::BlockU8 => {
            let value = vm.unsigned("value", u8::MAX.into())?;
            fields.insert("m_B4802CE3", Value::Integer(value.into()).into());
        }
        Family::BlockU16 => {
            let value = vm.unsigned("value", u16::MAX.into())?;
            fields.insert("m_AE2D8911", Value::Integer(value.into()).into());
        }
        // `Write_STU_491F6FF1` 0x7FF78AAA6E90: the stack's two bits in owner
        // frames, then its activation counter (`+0x44`) as a w_u16.
        Family::StackCounter => {
            if owner {
                fields.insert("top", Value::Bool(vm.flag("top")).into());
                fields.insert("under", Value::Bool(vm.flag("under")).into());
            }
            let counter = vm.integer("counter", 0)? & 0xffff;
            fields.insert("counter", Value::Integer(counter).into());
        }
        // `STU_4BD2E886` (a UX presenter): the reader 0x7FF7899E7050 keeps
        // the bit at `+0x470`; the client has no writer.
        Family::Bit => {
            fields.insert("value", Value::Bool(vm.flag("value")).into());
        }
        // `Write_STU_239B1E64` 0x7FF78AAADF40: a var_b (`+0x1C0`), a bit
        // (`+0x1C4`).
        Family::VarFlag => {
            let value = vm.unsigned("value", u32::MAX)?;
            fields.insert("value", Value::Integer(value.into()).into());
            fields.insert("flag", Value::Bool(vm.flag("flag")).into());
        }
        // `Write_STU_63B7ADFD` 0x7FF789C557F0: a value (`+0x1D8`) sent only
        // when not 0, after a bit (`+0x1DF`).
        Family::OptionalVar => {
            let value = vm.unsigned("value", u32::MAX)?;
            fields.insert("a", Value::Bool(value != 0).into());
            fields.insert("b", Value::Bool(vm.flag("flag")).into());
            fields.insert("value", Value::Integer(value.into()).into());
        }
        Family::Placement => project_placement(&mut fields, &vm)?,
        Family::LinkedTarget if owner && mode.layout().full => {
            project_linked_target(&mut fields, &vm)?;
        }
        // The link alone (an absent link, as for `link`).
        Family::LinkedTarget => {}
        // Tracer's Blink effect (`STU_99B885B8`, `Write_STU_99B885B8`
        // 0x7FF789BA5C20): observers only, the blink's 32-bit id
        // (`+0x1C0`); their reader 0x7FF7897F77D0 keeps the ids it has
        // seen (`+0x1C8`, sorted) and plays each new one.
        Family::Blink => {
            fields.insert("value", Value::Integer(vm.raw32("id")?).into());
        }
    }
    Ok(fields)
}

/// Recall's rewind (`STU_41B3FC2B`, owner and observer frames alike):
/// - the property block's s64 `m_0007BF47`, the host time (ms) the rewind
///   ends: the client's begin (0x7FF7897FD090) sets it to its begin time +
///   `m_FAF3B2D3` seconds × 1000 rounded, and its reader
///   (`Read_STU_41B3FC2B` 0x7FF7897FEEF0) re-arms the state's finish (event
///   2) at it (`0x7FF78AA81470`);
/// - 16 bits, frames: the reader keeps `frames × 16000 / 10^6` seconds
///   (`0x7FF78BC26B30` = 16000, the frame in µs) at `+0x970`, which only
///   the begin reads: the length of history it copies into the recorder it
///   rewinds along (0x7FF789ABF230). The client has no writer (the base
///   nullsub).
fn project_rewind(fields: &mut LayoutFields<'static>, vm: &Vm<'_>) -> Result<(), PayloadError> {
    fields.insert("m_0007BF47", Value::Integer(vm.integer("end", 0)?).into());
    let frames = vm.unsigned("frames", u16::MAX.into())?;
    fields.insert("time", Value::Integer(frames.into()).into());
    Ok(())
}

/// `STU_746F8777` (`Write_STU_746F8777` 0x7FF789B97E10): the property
/// block's s64 `m_0007BF47`, a due time (ms) its reader 0x7FF7897F4520
/// arms event 3 at (at least half its `+0x1F4` seconds away); a bit
/// (`+0x1FB`); a bit the writer always sets; then two vectors, `+0x1D0`
/// and `+0x1C0`, as six f32.
fn project_two_vectors(
    fields: &mut LayoutFields<'static>,
    vm: &Vm<'_>,
) -> Result<(), PayloadError> {
    fields.insert("m_0007BF47", Value::Integer(vm.integer("end", 0)?).into());
    fields.insert("a", Value::Bool(vm.flag("flag")).into());
    fields.insert("b", Value::Bool(true).into());
    let [first, second] = [vm.vector("first")?, vm.vector("second")?];
    let rows = first
        .into_iter()
        .chain(second)
        .map(|part| LayoutFields::from([("x", Value::Float(part.into()).into())]))
        .collect();
    fields.insert("v", FieldValue::Rows(rows));
    Ok(())
}

/// Shockwave (`STU_01D67FA0`, `Write_STU_01D67FA0` 0x7FF789BA5C90): when
/// its list (`m_B3B10C9E`, 8-byte records) has a record, a bit, the count,
/// each record's u16 at +4 (w_u16), its 16 bits at +6 and its 32 bits at
/// +0, then the block's f32 `m_6A79BD98`; an empty list is a 0 bit alone.
/// Payload: `items` as `[a, b, c]` rows (+4, +6, +0), `m_6A79BD98`.
fn project_shockwave(fields: &mut LayoutFields<'static>, vm: &Vm<'_>) -> Result<(), PayloadError> {
    let items = match vm.0.get("items") {
        None => &[][..],
        Some(Value::Array(items)) => items.as_slice(),
        _ => return Err(invalid_field("items")),
    };
    fields.insert("has_list", Value::Bool(!items.is_empty()).into());
    if items.is_empty() {
        return Ok(());
    }
    let mut rows = Vec::with_capacity(items.len());
    for item in items {
        let Value::Array(row) = item else {
            return Err(invalid_field("items"));
        };
        let [a, b, c] = row.as_slice() else {
            return Err(invalid_field("items"));
        };
        rows.push(LayoutFields::from([
            ("a", a.clone().into()),
            ("b", b.clone().into()),
            ("c", c.clone().into()),
        ]));
    }
    let count = u16::try_from(rows.len()).map_err(|_| invalid_field("items"))?;
    fields.insert("n", Value::Integer(count.into()).into());
    fields.insert("items", FieldValue::Rows(rows));
    let f = vm.float("m_6A79BD98", 0.0)?;
    fields.insert("f", Value::Float(f).into());
    Ok(())
}

/// The spray placement (`STU_861D9015`, `Write_STU_861D9015`
/// 0x7FF789C1FAA0): when placed (`+0x1F8`), a direction (`+0x1C0`) as two
/// 16-bit angles ([`direction16`]), a position (`+0x1D0`) as three f32, a
/// second direction (`+0x1E0`), then an optional entity (`+0x1F0`) with an
/// optional var_b (`+0x1F4`, sent when not 0) and a bit (`+0x1F9`).
fn project_placement(fields: &mut LayoutFields<'static>, vm: &Vm<'_>) -> Result<(), PayloadError> {
    let placed = vm.flag("placed");
    fields.insert("a", Value::Bool(placed).into());
    if !placed {
        return Ok(());
    }
    let (v0, v1) = direction16(vm.vector("direction")?);
    let [x, y, z] = vm.vector("position")?;
    let (v5, v6) = direction16(vm.vector("second_direction")?);
    for (name, value) in [
        ("v0", u32::from(v0)),
        ("v1", u32::from(v1)),
        ("v2", x.to_bits()),
        ("v3", y.to_bits()),
        ("v4", z.to_bits()),
        ("v5", u32::from(v5)),
        ("v6", u32::from(v6)),
    ] {
        fields.insert(name, Value::Integer(value.into()).into());
    }
    let entity = vm.entity("entity")?;
    let extra = vm.unsigned("extra", u32::MAX)?;
    fields.insert("has_entity", Value::Bool(valid_entity(entity)).into());
    fields.insert("entity", Value::Integer(entity.into()).into());
    fields.insert("has_extra", Value::Bool(extra != 0).into());
    fields.insert("extra", Value::Integer(extra.into()).into());
    fields.insert("g", Value::Bool(vm.flag("flag")).into());
    Ok(())
}

/// A unit direction as the client's two 16-bit angles (0x7FF789CC1050;
/// read back by 0x7FF789CC1360): the yaw `atan2(x, z)` × 32768/π and the
/// pitch `asin(y)` × 65536/π, each rounded half away from zero, + 0x8000
/// and kept within 0..=0xFFFF.
fn direction16([x, y, z]: [f32; 3]) -> (u16, u16) {
    const STEEP: f32 = 0.999_845_5;
    let quantize = |angle: f32| {
        let rounded = if angle >= 0.0 {
            angle + 0.5
        } else {
            angle - 0.5
        };
        (rounded as i32 + 0x8000).clamp(0, 0xffff) as u16
    };
    let yaw = if x == 0.0 && z == 0.0 {
        0.0
    } else {
        x.atan2(z)
    };
    let pitch = if y > STEEP {
        std::f32::consts::FRAC_PI_2
    } else if y < -STEEP {
        -std::f32::consts::FRAC_PI_2
    } else {
        y.asin()
    };
    (quantize(yaw * 10_430.378), quantize(pitch * 20_860.756))
}

/// What `Write_STU_9935FBC8` (0x7FF789B98260) compares each of its seven
/// floats (`+0x1EC` to `+0x204`) with: a float equal to its default is a 0
/// bit, any other a 1 bit and the f32 (0x7FF789BB61F0).
const LINKED_TARGET_DEFAULTS: [f32; 7] =
    [0.0, 0.0, -174_532.9, -174_532.9, 174_532.9, 174_532.9, 0.0];

/// `STU_9935FBC8` in an owner's full frame, after its link: an entity
/// (`+0x1E8`) only when valid and with bit 31 set, then the seven floats
/// (`values`, missing ones at their defaults).
fn project_linked_target(
    fields: &mut LayoutFields<'static>,
    vm: &Vm<'_>,
) -> Result<(), PayloadError> {
    let entity = vm.entity("entity")?;
    let has_entity = valid_entity(entity) && entity & 0x8000_0000 != 0;
    fields.insert("has_entity", Value::Bool(has_entity).into());
    fields.insert("entity", Value::Integer(entity.into()).into());
    let values = match vm.0.get("values") {
        None => &[][..],
        Some(Value::Array(values)) if values.len() <= LINKED_TARGET_DEFAULTS.len() => {
            values.as_slice()
        }
        _ => return Err(invalid_field("values")),
    };
    let mut rows = Vec::with_capacity(LINKED_TARGET_DEFAULTS.len());
    for (index, default) in LINKED_TARGET_DEFAULTS.into_iter().enumerate() {
        let value = match values.get(index) {
            None => default,
            Some(value) => value_float(value).ok_or_else(|| invalid_field("values"))? as f32,
        };
        rows.push(LayoutFields::from([
            ("set", Value::Bool(value != default).into()),
            ("value", Value::Float(value.into()).into()),
        ]));
    }
    fields.insert("values", FieldValue::Rows(rows));
    Ok(())
}

fn project_volley(
    fields: &mut LayoutFields<'static>,
    vm: &Vm<'_>,
    mode: PayloadMode,
) -> Result<(), PayloadError> {
    if let PayloadMode::Owner { frame_time, .. } = mode {
        let start = vm.integer("start", frame_time)?;
        let elapsed = frame_time
            .checked_sub(start)
            .ok_or(PayloadError::TimeRange)?;
        let offset = vm.integer("offset", 0)?;
        fields.insert("t1", Value::Integer(elapsed).into());
        fields.insert("has_offset", Value::Bool(offset != 0).into());
        if offset != 0 {
            fields.insert("offset", Value::Integer(offset).into());
            fields.insert(
                "subindex",
                Value::Integer(vm.integer("subindex", 0)?).into(),
            );
        }
    }
    let volleys = vm.integer("volleys", 1)? & 0xff;
    fields.insert("has_volleys", Value::Bool(volleys != 1).into());
    fields.insert("volleys", Value::Integer(volleys).into());
    fields.insert("counter", Value::Integer(vm.integer("counter", 0)?).into());
    Ok(())
}

/// Widowmaker's Grappling Hook cast (`STU_4BBC5D09`): in an owner frame,
/// the milliseconds from its begin to the start of the frame's window, as
/// its writer `0x7FF789BA5C40` gives them (the host's frame × frame
/// length − the begin at `+72`); the reader `0x7FF789BA1150` takes the
/// begin back from the frame's. Negative when the cast began inside the
/// frame's window (a timer's end). Observers get nothing.
fn project_start(
    fields: &mut LayoutFields<'static>,
    vm: &Vm<'_>,
    mode: PayloadMode,
) -> Result<(), PayloadError> {
    if let PayloadMode::Owner { frame_time, .. } = mode {
        let start = vm
            .optional_int("start")?
            .ok_or_else(|| invalid_field("start"))?;
        let t = frame_time
            .checked_sub(start)
            .ok_or(PayloadError::TimeRange)?;
        fields.insert("t", Value::Integer(t).into());
    }
    Ok(())
}

fn project_chase(
    fields: &mut LayoutFields<'static>,
    vm: &Vm<'_>,
    mode: PayloadMode,
) -> Result<(), PayloadError> {
    if let Some(Value::Vector([x, y, z])) = vm.0.get("cur") {
        fields.insert("vector", Value::Bool(true).into());
        for (name, part) in [("x", *x), ("y", *y), ("z", *z)] {
            fields.insert(name, Value::Float(part).into());
        }
    } else {
        fields.insert("x", Value::Float(vm.float("cur", 0.0)?).into());
    }
    fields.insert("reached", Value::Bool(vm.flag("reached")).into());
    let remaining = vm.optional_int("remaining")?;
    if let Some(remaining) = remaining {
        fields.insert("has_remaining", Value::Bool(true).into());
        fields.insert("big", Value::Bool(remaining >= 1 << 16).into());
        fields.insert("remaining", Value::Integer(remaining.max(0)).into());
    }
    // The last-step time goes with a timed chase only. A client that reads
    // it (`Read_STU_6AE5D1A1` sets `+624`) takes the chase's value from the
    // block's cur and the time since (`0x7FF78AAAEC80` through output slot
    // 49 `0x7FF78AAA80A0`), which for a chase without a duration steps
    // only by its `m_9B0385B4` speed (unset: none), not by its own steps:
    // Rocket Punch's charge `0D06` st11 (`#9272`) and st36 (`#1590`) stood
    // at the server's first chunk's cur for the whole charge (df8 s1, df10,
    // df12 s3). Without it the reader takes now as the last step and the
    // client goes on stepping from cur.
    if let PayloadMode::Owner {
        frame_time,
        frame_ms,
        ..
    } = mode
        && remaining.is_some()
        && let Some(last) = vm.optional_int("last")?
    {
        let t = elapsed(frame_time, frame_ms, last)?;
        if t >= 0 {
            fields.insert("has_last", Value::Bool(true).into());
            fields.insert("t", Value::Integer(t).into());
        }
    }
    Ok(())
}

fn project_ability(
    fields: &mut LayoutFields<'static>,
    vm: &Vm<'_>,
    mode: PayloadMode,
) -> Result<(), PayloadError> {
    fields.insert("flags", Value::Integer(vm.integer("flags", 0)?).into());
    let cur = vm.float("cur", 0.0)?;
    if cur > 0.0 || (mode.layout().owner && cur.is_nan()) {
        let rate = vm.float("rate", 1.0)?;
        fields.insert("has_cur", Value::Bool(true).into());
        fields.insert("cur", Value::Float(cur).into());
        fields.insert("has_rate", Value::Bool(rate != 1.0).into());
        fields.insert("rate", Value::Float(rate).into());
        if let PayloadMode::Owner {
            frame_time,
            frame_ms,
            ..
        } = mode
        {
            if let Some(last) = vm.optional_int("last")? {
                fields.insert(
                    "t",
                    Value::Integer(elapsed(frame_time, frame_ms, last)?.max(0)).into(),
                );
            }
        }
    }
    Ok(())
}

/// A scope state's targets (`snapshot.rs` builds the rows: target, host
/// entity or null, instance, index, keep), as `Write_STU_07AD90EE`
/// (0x7FF789C56790) writes them; melee's hit list (`STU_77E13026`) has
/// none.
fn project_hits(fields: &mut LayoutFields<'static>, vm: &Vm<'_>) -> Result<(), PayloadError> {
    let hits = match vm.0.get("hits") {
        None => &[][..],
        Some(Value::Array(hits)) => hits.as_slice(),
        _ => return Err(invalid_field("hits")),
    };
    let mut rows = Vec::with_capacity(hits.len());
    for hit in hits {
        let Value::Array(row) = hit else {
            return Err(invalid_field("hits"));
        };
        let [target, host, instance, index, keep] = row.as_slice() else {
            return Err(invalid_field("hits"));
        };
        rows.push(LayoutFields::from([
            ("value", target.clone().into()),
            ("entity", host.clone().into()),
            ("extra", instance.clone().into()),
            ("w", index.clone().into()),
            ("a", keep.clone().into()),
            ("b", Value::Bool(true).into()),
        ]));
    }
    let count = u16::try_from(rows.len()).map_err(|_| invalid_field("hits"))?;
    fields.insert("n", Value::Integer(i64::from(count)).into());
    fields.insert("hits", FieldValue::Rows(rows));
    fields.insert("flag", Value::Bool(vm.flag("flag")).into());
    Ok(())
}

fn project_targets(fields: &mut LayoutFields<'static>, vm: &Vm<'_>) -> Result<(), PayloadError> {
    let targets = match vm.0.get("targets") {
        None => &[][..],
        Some(Value::Array(targets)) => targets.as_slice(),
        _ => return Err(invalid_field("targets")),
    };
    let mut rows = Vec::with_capacity(targets.len());
    for target in targets {
        let Value::Array(pair) = target else {
            return Err(invalid_field("targets"));
        };
        let [entity, age] = pair.as_slice() else {
            return Err(invalid_field("targets"));
        };
        rows.push(LayoutFields::from([
            ("entity", entity.clone().into()),
            ("age", age.clone().into()),
        ]));
    }
    // The client's writer (`0x7FF789BA5F80`) sends the list only when it
    // has a row; an empty list is the bit alone.
    fields.insert("has_list", Value::Bool(!rows.is_empty()).into());
    if !rows.is_empty() {
        let count = u16::try_from(rows.len()).map_err(|_| invalid_field("targets"))?;
        fields.insert("n", Value::Integer(i64::from(count)).into());
        fields.insert("targets", FieldValue::Rows(rows));
    }
    Ok(())
}

fn elapsed(frame_time: i64, frame_ms: u32, last: i64) -> Result<i64, PayloadError> {
    frame_time
        .checked_add(i64::from(frame_ms))
        .and_then(|next| next.checked_sub(last))
        .ok_or(PayloadError::TimeRange)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const FIXTURE: &str = r#"{"classes":{
        "ABCDEF00":{
            "name":"FixtureState","codec":"custom","stec":false,"status":"VERIFIED","layout":[
                {"op":"bits","f":"tag","n":64},
                {"op":"if","c":"H","then":[{"op":"signed_var","f":"dt"}],"else":[{"op":"w_u16","f":"short"}]},
                {"op":"f32","f":"scale"},
                {"op":"if","c":"H&full","then":[{"op":"value","f":"data"}],"else":[]},
                {"op":"if","c":"full","then":[{"op":"link","f":"bind"}],"else":[]},
                {"op":"w_u16","f":"n"},
                {"op":"repeat","count":"n","f":"rows","body":[
                    {"op":"entity_id","f":"entity"},
                    {"op":"if","c":"valid:entity","then":[{"op":"var_b","f":"extra"}],"else":[]},
                    {"op":"bits","f":"flag","n":1}]},
                {"op":"repeat","count":2,"f":"flags","body":[{"op":"bits","f":"set","n":1}]},
                {"op":"if","c":"!a","then":[{"op":"bits","f":"b","n":1}],"else":[]}
            ]
        },
        "ABCDEF01":{"name":"NoLayout","codec":"unknown","stec":true,"status":"UNVERIFIED","layout":null},
        "ABCDEF02":{"name":"Empty","codec":"none","stec":false,"status":"VERIFIED","layout":[]}
    }}"#;

    fn fields() -> LayoutFields<'static> {
        LayoutFields::from([
            ("tag", FieldValue::Unsigned(0xfedc_ba98_7654_3210)),
            ("dt", Value::Integer(-70000).into()),
            ("short", Value::Integer(300).into()),
            ("scale", Value::Float(1.25).into()),
            (
                "data",
                Value::Array(vec![
                    Value::Bool(true),
                    Value::Integer(-23),
                    Value::String("x".into()),
                ])
                .into(),
            ),
            ("bind", Value::Integer(9).into()),
            ("n", Value::Integer(2).into()),
            (
                "rows",
                FieldValue::Rows(vec![
                    LayoutFields::from([
                        ("entity", Value::Entity(0xa000_0101).into()),
                        ("extra", Value::Integer(70000).into()),
                        ("flag", Value::Bool(true).into()),
                    ]),
                    LayoutFields::from([("entity", Value::Entity(0xbfff_ffff).into())]),
                ]),
            ),
            (
                "flags",
                FieldValue::Rows(vec![
                    LayoutFields::from([("set", Value::Bool(true).into())]),
                    LayoutFields::new(),
                ]),
            ),
            ("b", Value::Bool(true).into()),
        ])
    }

    fn check(out: &BitWriter, length: usize, hex: &str) {
        let actual: String = out
            .as_bytes()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        assert_eq!((out.bit_len(), actual.as_str()), (length, hex));
    }

    #[test]
    fn resolves_all_aliases_and_rejects_unverified_or_unprojected_active_states() {
        let table = CodecTable::from_json(FIXTURE).unwrap();
        let codec = table.codec("FixtureState").unwrap();
        assert!(std::ptr::eq(table.codec("ABCDEF00").unwrap(), codec));
        assert!(std::ptr::eq(table.codec("STU_ABCDEF00").unwrap(), codec));
        assert!(codec.verified());
        assert!(!codec.projected());
        assert!(!table.sendable("FixtureState", true));
        assert!(table.sendable("FixtureState", false));
        assert!(!table.sendable("NoLayout", false));
        assert!(table.sendable("missing", false));
        assert!(table.codec("Empty").unwrap().empty());
        assert!(table.sendable("Empty", true));
        let mode = PayloadMode::Remote { full: true };
        assert_eq!(
            codec.payload(&StatePayload::new(), mode),
            Err(PayloadError::Unsupported("FixtureState".into()))
        );
        assert_eq!(
            table
                .codec("NoLayout")
                .unwrap()
                .payload(&StatePayload::new(), mode),
            Err(PayloadError::Unverified("NoLayout".into()))
        );
    }

    #[test]
    fn generic_layout_matches_python_for_modes_links_tagged_values_and_repeats() {
        let table = CodecTable::from_json(FIXTURE).unwrap();
        let codec = table.codec("FixtureState").unwrap();
        let fields = fields();
        let owner = codec
            .encode_layout_with_link(
                &fields,
                LayoutMode {
                    owner: true,
                    full: true,
                },
                |out, link| {
                    assert_eq!(link, Some(&FieldValue::Scalar(Value::Integer(9))));
                    out.bit(true);
                    out.w_u16(9);
                    Ok(())
                },
            )
            .unwrap();
        check(
            &owner,
            291,
            "1032547698badcfe878b0800000000fd791ba0a4bff0004a520101c3450400ecffffff0f05",
        );
        let remote = codec.encode_layout(&fields, LayoutMode::default()).unwrap();
        check(
            &remote,
            214,
            "1032547698badcfeb304000080fe900a08182e220060ffffff7f28",
        );
    }

    /// A row with no entity (null: a scope target with no instance yet,
    /// `project_hits`) writes the invalid id and skips its `valid:entity`
    /// branch, as one with entity 0 does.
    #[test]
    fn a_null_entity_is_the_invalid_id() {
        let table = CodecTable::from_json(FIXTURE).unwrap();
        let codec = table.codec("FixtureState").unwrap();
        let encode = |entity: Value| {
            let mut fields = fields();
            let Some(FieldValue::Rows(rows)) = fields.get_mut("rows") else {
                unreachable!("the fixture has rows");
            };
            rows[1].insert("entity", entity.into());
            let out = codec.encode_layout(&fields, LayoutMode::default()).unwrap();
            (out.bit_len(), out.as_bytes().to_vec())
        };
        assert_eq!(encode(Value::Null), encode(Value::Entity(0)));
    }

    #[test]
    fn malformed_layout_fields_report_sizes_and_types() {
        let table = CodecTable::from_json(FIXTURE).unwrap();
        let codec = table.codec("FixtureState").unwrap();
        let mut fields = fields();
        fields.insert("n", Value::Integer(3).into());
        assert_eq!(
            codec.encode_layout(&fields, LayoutMode::default()),
            Err(PayloadError::Rows {
                field: "rows".into(),
                expected: 3,
                actual: 2
            })
        );
        fields.insert("short", Value::Integer(-1).into());
        assert_eq!(
            codec.encode_layout(&fields, LayoutMode::default()),
            Err(PayloadError::Field("short".into()))
        );
    }

    fn sample_value(value: &serde_json::Value) -> Value {
        use serde_json::Value as Json;
        match value {
            Json::Null => Value::Null,
            Json::Bool(value) => Value::Bool(*value),
            Json::Number(number) => number
                .as_i64()
                .map_or_else(|| Value::Float(number.as_f64().unwrap()), Value::Integer),
            Json::Array(items) => Value::Array(items.iter().map(sample_value).collect()),
            Json::Object(object) => {
                let parts = object["vector"].as_array().unwrap();
                Value::Vector(std::array::from_fn(|axis| parts[axis].as_f64().unwrap()))
            }
            Json::String(string) => Value::String(string.clone()),
        }
    }

    fn samples() -> BTreeMap<String, Vec<StatePayload>> {
        // tests/test_statescript_codecs.py::SAMPLES with explicit vectors and
        // all three delayed switch payloads from the generic layout writer.
        let corpus = json!({
            "none":[{}],
            "switch":[{"current":true},{"current":false}],
            "two_bits":[
                {"current":false,"undecided":false},
                {"current":false,"undecided":true},
                {"current":true,"undecided":false}
            ],
            "stack":[{"top":true,"under":false},{"top":false,"under":true}],
            "button":[{"counter":0},{"counter":13},{"counter":21}],
            "volley":[
                {"start":15840,"counter":5},
                {"start":16003,"offset":-7,"subindex":2,"volleys":3,"counter":70}
            ],
            "anim":[{"counter":6},{"counter":9}],
            "chase":[
                {"cur":1.5,"reached":false,"remaining":null,"last":null},
                {"cur":{"vector":[1.0,-2.0,0.5]},"reached":true,"remaining":70000,"last":16005},
                {"cur":0.25,"remaining":300,"last":15960}
            ],
            "ability":[
                {"flags":5,"cur":0.0},
                {"flags":2,"cur":3.5,"rate":1.0,"last":15960},
                {"flags":7,"cur":1.25,"rate":0.5,"last":16016}
            ],
            "targets":[{},{"targets":[
                [0xa000_0101_u32,0],[0xa000_0102_u32,255],[0xa000_0103_u32,256],
                [0xa000_0104_u32,65535],[0xa000_0105_u32,65536]
            ]}],
            "subscript":[{"child":13},{"child":300}],
            "message":[{"stacked":true},{"stacked":false}],
            "send":[{}],
            "link":[{}],
            "hits":[{"flag":true},{"flag":false}],
            "flags13":[{"value":0},{"value":63},{"value":64},{"value":8191}],
            "frames":[{"target":9199}],
            "counter2":[{"counter":0},{"counter":3},{"counter":6}],
            "pulser":[{"fresh":false,"count":0},{"fresh":true,"count":300}]
        });
        corpus
            .as_object()
            .unwrap()
            .iter()
            .map(|(family, cases)| {
                let cases = cases
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|case| {
                        case.as_object()
                            .unwrap()
                            .iter()
                            .map(|(key, value)| (key.clone(), sample_value(value)))
                            .collect()
                    })
                    .collect();
                (family.clone(), cases)
            })
            .collect()
    }

    #[test]
    fn actual_payloads_match_python_in_owner_and_both_observer_modes() {
        use sha2::{Digest, Sha256};

        let table = CodecTable::client();
        assert_eq!(table.classes().count(), 598);
        assert_eq!(
            table.classes().filter(|codec| codec.verified()).count(),
            595
        );
        assert_eq!(table.classes().filter(|codec| codec.stec()).count(), 4);
        // ScaleTime (StEC) carries its payload while off too: sendable once
        // its family (`value_slot`) exists.
        assert!(table.sendable("STU_2F07848F", false));
        let mut projected: Vec<_> = table.classes().filter(|codec| codec.projected()).collect();
        projected.sort_by(|left, right| left.hash.cmp(&right.hash));
        // Every verified class (`CodecTable::client`).
        assert_eq!(projected.len(), 595);
        let samples = samples();
        assert_eq!(samples.len(), 19);
        // Python's families; the others came later, each with its own test
        // below (`project_start` and the families after it).
        let (projected, later): (Vec<_>, Vec<_>) = projected
            .into_iter()
            .partition(|codec| samples.contains_key(codec.codec.as_deref().unwrap()));
        assert_eq!(
            later
                .iter()
                .map(|codec| codec.hash.as_str())
                .collect::<Vec<_>>(),
            [
                "01D67FA0", "239B1E64", "2F07848F", "41B3FC2B", "43DB06B1", "491F6FF1", "4A67370F",
                "4BBC5D09", "4BD2E886", "4C96470A", "63B7ADFD", "746F8777", "861D9015", "9935FBC8",
                "99B885B8"
            ]
        );
        let modes = [
            PayloadMode::Owner {
                full: false,
                frame_time: 16000,
                frame_ms: 16,
            },
            PayloadMode::Remote { full: true },
            PayloadMode::Remote { full: false },
        ];
        let mut digest = Sha256::new();
        let mut count = 0;
        for codec in projected {
            let cases = &samples[codec.codec.as_deref().unwrap()];
            for (mode, style) in modes.into_iter().enumerate() {
                for (index, payload) in cases.iter().enumerate() {
                    let out = codec.payload(payload, style).unwrap();
                    digest.update(codec.hash.as_bytes());
                    digest.update([u8::try_from(mode).unwrap()]);
                    digest.update(u32::try_from(index).unwrap().to_le_bytes());
                    digest.update(u32::try_from(out.bit_len()).unwrap().to_le_bytes());
                    digest.update(out.as_bytes());
                    count += 1;
                }
            }
        }
        assert_eq!(count, 1833);
        // Python's digest, but for the empty TrackTargets list: Python
        // sent the list bit and a zero count, the client's writer
        // (`0x7FF789BA5F80`) sends the bit alone (0).
        assert_eq!(
            format!("{:x}", digest.finalize()),
            "ae1a884a29f5614e9448c6a06b61c926b78be7010558bdb9d6211b47924ab98b"
        );
    }

    // The families after Python's, each against the client's writer (or,
    // without one, its reader): the expected bits are written in the
    // writer's order.

    const OWNER: PayloadMode = PayloadMode::Owner {
        full: false,
        frame_time: 16000,
        frame_ms: 16,
    };
    const OWNER_FULL: PayloadMode = PayloadMode::Owner {
        full: true,
        frame_time: 16000,
        frame_ms: 16,
    };
    const REMOTE: PayloadMode = PayloadMode::Remote { full: true };

    fn encode(class: &str, payload: serde_json::Value, mode: PayloadMode) -> BitWriter {
        static TABLE: std::sync::LazyLock<CodecTable> =
            std::sync::LazyLock::new(CodecTable::client);
        let payload = payload
            .as_object()
            .unwrap()
            .iter()
            .map(|(key, value)| (key.clone(), sample_value(value)))
            .collect();
        TABLE.codec(class).unwrap().payload(&payload, mode).unwrap()
    }

    fn written(write: impl FnOnce(&mut BitWriter)) -> BitWriter {
        let mut out = BitWriter::new();
        write(&mut out);
        out
    }

    /// Recall's rewind: its end (the block's s64), then the frames of
    /// history in 16 bits, for the owner and observers alike.
    #[test]
    fn recalls_rewind_writes_its_end_then_its_frames_of_history() {
        let payload = json!({"end": 124_442, "frames": 188});
        let expected = written(|out| {
            out.bits(124_442, 64).unwrap();
            out.bits(188, 16).unwrap();
        });
        for mode in [OWNER, REMOTE] {
            assert_eq!(encode("STU_41B3FC2B", payload.clone(), mode), expected);
        }
    }

    #[test]
    fn two_vectors_write_their_due_time_flag_and_six_floats() {
        let payload = json!({
            "end": 20000, "flag": true,
            "first": {"vector": [1.0, 2.0, 3.0]}, "second": {"vector": [-1.0, 0.5, 0.25]}
        });
        let expected = written(|out| {
            out.bits(20000, 64).unwrap();
            out.bit(true);
            out.bit(true); // always set by the writer
            for part in [1.0, 2.0, 3.0, -1.0, 0.5, 0.25] {
                out.f32(part);
            }
        });
        assert_eq!(encode("STU_746F8777", payload, OWNER), expected);
    }

    #[test]
    fn a_shockwave_writes_its_records_and_speed_or_a_zero_bit() {
        let payload = json!({"items": [[300, -2, 0xA000_0101_u32], [5, 7, 9]], "m_6A79BD98": 12.5});
        let expected = written(|out| {
            out.bit(true);
            out.w_u16(2);
            out.w_u16(300);
            out.bits(0xfffe, 16).unwrap();
            out.bits(0xA000_0101, 32).unwrap();
            out.w_u16(5);
            out.bits(7, 16).unwrap();
            out.bits(9, 32).unwrap();
            out.f32(12.5);
        });
        assert_eq!(encode("STU_01D67FA0", payload, REMOTE), expected);
        let empty = encode("STU_01D67FA0", json!({"m_6A79BD98": 12.5}), REMOTE);
        assert_eq!(empty, written(|out| out.bit(false)));
    }

    #[test]
    fn scale_time_writes_the_value_of_its_side() {
        let payload = |active| json!({"active": active, "on": 0.5, "off": 1.0});
        for (active, value) in [(true, 0.5_f32), (false, 1.0)] {
            let expected = written(|out| {
                out.bit(active);
                out.bits(value.to_bits().into(), 32).unwrap();
            });
            assert_eq!(encode("STU_2F07848F", payload(active), REMOTE), expected);
        }
    }

    #[test]
    fn block_fields_write_their_byte_or_short() {
        for class in ["STU_43DB06B1", "STU_4C96470A"] {
            let out = encode(class, json!({"value": 1}), OWNER);
            assert_eq!(out, written(|out| out.bits(1, 8).unwrap()));
        }
        let out = encode("STU_4A67370F", json!({"value": 65535}), REMOTE);
        assert_eq!(out, written(|out| out.bits(0xffff, 16).unwrap()));
    }

    #[test]
    fn a_counted_stack_writes_its_bits_to_the_owner_and_its_counter_to_all() {
        let payload = json!({"top": true, "under": false, "counter": 70000});
        // The activation counter is 16 bits: 70000 is 4464.
        let owner = written(|out| {
            out.bit(true);
            out.bit(false);
            out.w_u16(4464);
        });
        assert_eq!(encode("STU_491F6FF1", payload.clone(), OWNER), owner);
        let observer = written(|out| out.w_u16(4464));
        assert_eq!(encode("STU_491F6FF1", payload, REMOTE), observer);
    }

    #[test]
    fn one_bit_states_write_their_bit() {
        let out = encode("STU_4BD2E886", json!({"value": true}), REMOTE);
        assert_eq!(out, written(|out| out.bit(true)));
    }

    #[test]
    fn var_flag_writes_a_var_b_then_its_flag() {
        let out = encode("STU_239B1E64", json!({"value": 70000, "flag": true}), OWNER);
        let expected = written(|out| {
            out.var_b(70000);
            out.bit(true);
        });
        assert_eq!(out, expected);
    }

    #[test]
    fn an_optional_var_writes_its_value_only_when_not_zero() {
        let out = encode("STU_63B7ADFD", json!({"value": 300, "flag": true}), OWNER);
        let expected = written(|out| {
            out.bit(true);
            out.bit(true);
            out.var_b(300);
        });
        assert_eq!(out, expected);
        let zero = encode("STU_63B7ADFD", json!({"value": 0, "flag": true}), OWNER);
        assert_eq!(zero, written(|out| out.bit(false)));
    }

    /// The client's angles (0x7FF789CC1050): yaw `atan2(x, z)`, pitch
    /// `asin(y)`, each scaled to 16 bits around 0x8000; a pitch past
    /// ±0.9998455 is ±π/2 (clamped to 0xFFFF at the top).
    #[test]
    fn a_placement_writes_its_directions_as_angles() {
        assert_eq!(direction16([0.0, 0.0, 1.0]), (0x8000, 0x8000));
        assert_eq!(direction16([1.0, 0.0, 0.0]), (0xc000, 0x8000));
        assert_eq!(direction16([0.0, 0.0, -1.0]), (0xffff, 0x8000));
        assert_eq!(direction16([0.0, 1.0, 0.0]), (0x8000, 0xffff));
        assert_eq!(direction16([0.0, -1.0, 0.0]), (0x8000, 0));
        let payload = json!({
            "placed": true,
            "direction": {"vector": [0.0, 0.0, 1.0]},
            "position": {"vector": [10.5, -2.0, 3.25]},
            "second_direction": {"vector": [1.0, 0.0, 0.0]},
            "entity": 0xA000_0101_u32, "extra": 70000, "flag": true
        });
        let expected = written(|out| {
            out.bit(true);
            out.bits(0x8000, 16).unwrap();
            out.bits(0x8000, 16).unwrap();
            for part in [10.5_f32, -2.0, 3.25] {
                out.bits(part.to_bits().into(), 32).unwrap();
            }
            out.bits(0xc000, 16).unwrap();
            out.bits(0x8000, 16).unwrap();
            out.bit(true);
            out.entity_id(Some(0xA000_0101));
            out.bit(true);
            out.var_b(70000);
            out.bit(true);
        });
        assert_eq!(encode("STU_861D9015", payload, REMOTE), expected);
        let unplaced = encode("STU_861D9015", json!({"placed": false}), REMOTE);
        assert_eq!(unplaced, written(|out| out.bit(false)));
    }

    /// After its (absent) link, only an owner's full frame carries the
    /// entity (when bit 31 is set) and the seven floats, each a 0 bit when
    /// it equals the writer's default.
    #[test]
    fn a_linked_target_writes_its_entity_and_changed_floats_in_full_owner_frames() {
        let payload =
            |entity: u32| json!({"entity": entity, "values": [0.0, 1.5, -174_532.9, 0.0]});
        let expected = |entity: Option<u32>| {
            written(|out| {
                out.bit(false); // link
                out.bit(entity.is_some());
                if entity.is_some() {
                    out.entity_id(entity);
                }
                out.bit(false);
                out.bit(true);
                out.f32(1.5);
                out.bit(false);
                out.bit(true); // 0 is not the default -174532.9
                out.f32(0.0);
                for _ in 4..7 {
                    out.bit(false);
                }
            })
        };
        assert_eq!(
            encode("STU_9935FBC8", payload(0xA000_0101), OWNER_FULL),
            expected(Some(0xA000_0101))
        );
        assert_eq!(
            encode("STU_9935FBC8", payload(0x2000_0101), OWNER_FULL),
            expected(None)
        );
        for mode in [OWNER, REMOTE] {
            let out = encode("STU_9935FBC8", payload(0xA000_0101), mode);
            assert_eq!(out, written(|out| out.bit(false)));
        }
    }

    #[test]
    fn a_blink_effect_sends_its_id_to_observers_only() {
        let out = encode("STU_99B885B8", json!({"id": 77}), REMOTE);
        assert_eq!(out, written(|out| out.bits(77, 32).unwrap()));
        assert_eq!(
            encode("STU_99B885B8", json!({"id": 77}), OWNER).bit_len(),
            0
        );
    }

    /// A chase's last-step time goes to the owner with a timed chase only:
    /// his client's output of a chase that read one is the block's cur
    /// moved by the chase's speed over the time since (`0x7FF78AAAEC80`),
    /// which froze Rocket Punch's charge `0D06` st11 (`#9272`, no
    /// duration) at the server's first chunk's value for the whole charge.
    #[test]
    fn a_chase_sends_its_last_step_only_with_a_duration() {
        let untimed = json!({"cur": 0.25, "reached": false, "last": 15990});
        let expected = written(|out| {
            out.bit(false); // vector
            out.bit(false); // remaining
            out.bit(false); // reached
            out.f32(0.25);
            out.bit(false); // no last step
        });
        assert_eq!(encode("STU_6AE5D1A1", untimed, OWNER), expected);
        let timed = json!({"cur": 0.25, "reached": false, "remaining": 1400, "last": 15990});
        let expected = written(|out| {
            out.bit(false);
            out.bit(true);
            out.bit(false);
            out.f32(0.25);
            out.bit(false); // 16-bit remaining
            out.bits(1400, 16).unwrap();
            out.bit(true);
            out.var_b(26); // 16000 + 16 - 15990
        });
        assert_eq!(encode("STU_6AE5D1A1", timed, OWNER), expected);
    }
}
