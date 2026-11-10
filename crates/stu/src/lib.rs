//! Reads build 104319's structured data assets (STUv2), the format of graphs,
//! entity definitions, game modes and most other game data.
//!
//! An asset is a 36-byte header of three tables (instances, inline array
//! types, field bags) and the sizes and offsets of two buffers: `data`, where
//! each instance is a field bag index and then its fields, and `dyn`, which
//! holds arrays, strings and hash maps. A field bag lists field hashes and their
//! sizes, so a field the type table doesn't know is skipped by size. What a
//! field holds comes from the type table (`types`), generated from OWLib.
//!
//! Ported from OWLib's TankLib (MIT) at revision `7afe5f52`; see `CREDITS.md`.

#![forbid(unsafe_code)]

mod read;
pub mod types;

use std::fmt;

pub use read::read;
pub use types::{Kind, Types};

/// A decoded asset: its instances by index, `None` where the class is unknown.
#[derive(Debug)]
pub struct Asset {
    pub instances: Vec<Option<Object>>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Object {
    /// The class hash.
    pub class: u32,
    /// Field hash and value, in the order the asset lists them.
    pub fields: Vec<(u32, Value)>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Null,
    Int(i64),
    UInt(u64),
    Float(f64),
    String(String),
    /// An asset reference; 0 is none.
    Guid(u64),
    Floats(Vec<f32>),
    Bytes(Vec<u8>),
    /// An index into [`Asset::instances`].
    Instance(usize),
    Object(Object),
    Array(Vec<Value>),
    /// A hash map: each key and the instance it names, if any.
    Map(Vec<(u64, Option<usize>)>),
}

#[derive(Debug)]
pub struct Error(String);

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

impl Object {
    /// A field by name, through the type table.
    pub fn get(&self, name: &str) -> Option<&Value> {
        let hash = Types::get().field_named(self.class, name)?.hash;
        self.fields
            .iter()
            .find(|(field, _)| *field == hash)
            .map(|(_, value)| value)
    }

    pub fn class_name(&self) -> Option<&'static str> {
        Types::get().class(self.class).map(|class| class.name)
    }
}

impl Asset {
    /// The first instance, which DataTool treats as the asset's root.
    pub fn root(&self) -> Option<&Object> {
        self.instances.first()?.as_ref()
    }

    /// The instance a value refers to.
    pub fn resolve<'a>(&'a self, value: &'a Value) -> Option<&'a Object> {
        match value {
            Value::Instance(index) => self.instances.get(*index)?.as_ref(),
            Value::Object(object) => Some(object),
            _ => None,
        }
    }
}
