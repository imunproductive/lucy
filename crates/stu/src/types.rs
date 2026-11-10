//! The STU type table: every class OWLib knows for build 104319, its parent and
//! its own fields, from `data/types.txt` (see `src/bin/gen-types.rs`).

use std::{collections::HashMap, sync::OnceLock};

use crate::Value;

const TABLE: &str = include_str!("../data/types.txt");

/// What one value of a field is on disk.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Kind {
    /// An integer of this many bytes; enums are stored as their underlying type.
    Int {
        bytes: u8,
        signed: bool,
    },
    F32,
    F64,
    String,
    /// An asset reference (`teStructuredDataAssetRef`), stored obfuscated.
    Guid,
    /// A fixed-size value read as raw bytes (UUIDs, dates, database IDs).
    Bytes(u8),
    /// A fixed-size run of `f32`s (vectors, colors, quaternions, matrices).
    Floats(u8),
    /// An index into the asset's instances.
    Embed,
    /// An instance stored in place, of this class (by hash).
    Inline(u32),
    /// A hash map from a 64-bit key to instance indices.
    Map,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FieldType {
    pub kind: Kind,
    pub array: bool,
}

#[derive(Debug)]
pub struct Field {
    pub hash: u32,
    pub name: &'static str,
    pub ty: FieldType,
    /// What the field holds when an asset doesn't store it: the declared
    /// initializer, else zero for numbers and vectors, else null.
    pub default: Value,
}

#[derive(Debug)]
pub struct Class {
    pub hash: u32,
    pub name: &'static str,
    parent: Option<u32>,
    fields: Vec<Field>,
}

pub struct Types {
    classes: HashMap<u32, Class>,
    by_name: HashMap<&'static str, u32>,
}

impl Types {
    pub fn get() -> &'static Types {
        static TYPES: OnceLock<Types> = OnceLock::new();
        TYPES.get_or_init(|| Types::parse(TABLE))
    }

    pub fn class(&self, hash: u32) -> Option<&Class> {
        self.classes.get(&hash)
    }

    pub fn class_named(&self, name: &str) -> Option<&Class> {
        self.class(*self.by_name.get(name)?)
    }

    /// A field of the class or one of its ancestors.
    pub fn field(&self, class: u32, hash: u32) -> Option<&Field> {
        let mut current = self.classes.get(&class);
        while let Some(class) = current {
            if let Some(field) = class.fields.iter().find(|field| field.hash == hash) {
                return Some(field);
            }
            current = class.parent.and_then(|parent| self.classes.get(&parent));
        }
        None
    }

    /// A field of the class or one of its ancestors, by name.
    pub fn field_named(&self, class: u32, name: &str) -> Option<&Field> {
        let mut current = self.classes.get(&class);
        while let Some(class) = current {
            if let Some(field) = class.fields.iter().find(|field| field.name == name) {
                return Some(field);
            }
            current = class.parent.and_then(|parent| self.classes.get(&parent));
        }
        None
    }

    /// Every field of the class and its ancestors, the class's own first.
    pub fn fields(&self, class: u32) -> impl Iterator<Item = &Field> {
        let mut current = self.classes.get(&class);
        std::iter::from_fn(move || {
            let class = current?;
            current = class.parent.and_then(|parent| self.classes.get(&parent));
            Some(class.fields.iter())
        })
        .flatten()
    }

    /// Whether `class` is `ancestor` or derives from it.
    pub fn is_a(&self, class: u32, ancestor: u32) -> bool {
        let mut current = Some(class);
        while let Some(hash) = current {
            if hash == ancestor {
                return true;
            }
            current = self.classes.get(&hash).and_then(|class| class.parent);
        }
        false
    }

    fn parse(text: &'static str) -> Types {
        let mut enums = HashMap::new();
        let mut raw = Vec::new();
        for line in text.lines().filter(|line| !line.starts_with('#')) {
            let words: Vec<&'static str> = line.splitn(6, ' ').collect();
            match words.as_slice() {
                ["enum", name, underlying] => {
                    enums.insert(*name, *underlying);
                }
                ["class", hash, name, parent] => {
                    raw.push((hex(hash), *name, *parent, Vec::new()));
                }
                ["field", hash, name, kind, reader] => {
                    let class = raw.last_mut().expect("a field follows its class");
                    class.3.push((hex(hash), *name, *kind, *reader, None));
                }
                ["field", hash, name, kind, reader, default] => {
                    let class = raw.last_mut().expect("a field follows its class");
                    class
                        .3
                        .push((hex(hash), *name, *kind, *reader, Some(*default)));
                }
                _ => panic!("malformed type table line {line:?}"),
            }
        }
        let by_name: HashMap<&'static str, u32> = raw
            .iter()
            .map(|(hash, name, _, _)| (*name, *hash))
            .collect();
        let classes = raw
            .into_iter()
            .map(|(hash, name, parent, fields)| {
                let fields = fields
                    .into_iter()
                    .map(|(hash, name, kind, reader, default)| {
                        let ty = field_type(kind, reader, &enums, &by_name);
                        Field {
                            hash,
                            name,
                            default: default_value(ty, default),
                            ty,
                        }
                    })
                    .collect();
                let class = Class {
                    hash,
                    name,
                    parent: by_name.get(parent).copied(),
                    fields,
                };
                (hash, class)
            })
            .collect();
        Types { classes, by_name }
    }
}

impl Class {
    pub fn own_fields(&self) -> &[Field] {
        &self.fields
    }
}

/// A field's value when absent, as C# initializes it.
fn default_value(ty: FieldType, declared: Option<&str>) -> Value {
    if ty.array {
        return Value::Null;
    }
    let number = |text: &str| -> f64 {
        text.parse()
            .unwrap_or_else(|_| panic!("default {text:?} for {ty:?}"))
    };
    let float = |text: &str| f64::from(number(text) as f32);
    match (ty.kind, declared) {
        (Kind::Int { signed: true, .. }, declared) => {
            Value::Int(declared.map_or(0, |text| number(text) as i64))
        }
        (Kind::Int { signed: false, .. }, declared) => Value::UInt(
            declared.map_or(0, |text| text.parse().unwrap_or(number(text) as i64 as u64)),
        ),
        (Kind::F32, declared) => Value::Float(declared.map_or(0.0, float)),
        (Kind::F64, declared) => Value::Float(declared.map_or(0.0, number)),
        (Kind::Guid, Some(text)) => Value::Guid(text.parse().expect("a GUID default")),
        (Kind::String, Some(text)) => Value::String(
            text.strip_prefix("s:")
                .expect("a string default")
                .to_owned(),
        ),
        (Kind::Floats(count), declared) => Value::Floats(match declared {
            Some(text) => text
                .strip_prefix("v:")
                .expect("a vector default")
                .split(',')
                .map(|part| number(part) as f32)
                .collect(),
            None => vec![0.0; usize::from(count)],
        }),
        (Kind::Bytes(count), _) => Value::Bytes(vec![0; usize::from(count)]),
        _ => Value::Null,
    }
}

fn hex(text: &str) -> u32 {
    u32::from_str_radix(text, 16).expect("hex hash in the type table")
}

/// Resolve OWLib's C# field type.
fn field_type(
    kind: &str,
    reader: &str,
    enums: &HashMap<&str, &str>,
    classes: &HashMap<&str, u32>,
) -> FieldType {
    let (kind, array) = match kind.strip_suffix("[]") {
        Some(element) => (element, true),
        None => (kind, false),
    };
    let int = |bytes, signed| Kind::Int { bytes, signed };
    let primitive = |name: &str| match name {
        "byte" => Some(int(1, false)),
        "sbyte" => Some(int(1, true)),
        "ushort" => Some(int(2, false)),
        "short" => Some(int(2, true)),
        "uint" => Some(int(4, false)),
        "int" => Some(int(4, true)),
        "ulong" => Some(int(8, false)),
        "long" => Some(int(8, true)),
        "float" => Some(Kind::F32),
        "double" => Some(Kind::F64),
        _ => None,
    };
    let kind = if let Some(kind) = primitive(kind) {
        kind
    } else if kind.starts_with("teStructuredDataAssetRef<") {
        Kind::Guid
    } else if kind.starts_with("teStructuredDataHashMap<") {
        Kind::Map
    } else if let Some(underlying) = enums.get(kind) {
        primitive(underlying).unwrap_or_else(|| panic!("enum {kind} of type {underlying}"))
    } else {
        match kind {
            "teString" => Kind::String,
            "teVec2" => Kind::Floats(2),
            "teVec3" | "teColorRGB" => Kind::Floats(3),
            "teVec3A" | "teVec4" | "teQuat" | "teColorRGBA" => Kind::Floats(4),
            "teMtx43A" => Kind::Floats(16),
            "teUUID" | "DBID" => Kind::Bytes(16),
            "teStructuredDataDateAndTime" => Kind::Bytes(8),
            "teEntityID" => int(4, false),
            // An embedded instance is an index, whatever its declared class
            // (a few are declared as the base `STUInstance`).
            _ if reader == "embed" => Kind::Embed,
            class => Kind::Inline(
                *classes
                    .get(class)
                    .unwrap_or_else(|| panic!("unknown STU field type {class}")),
            ),
        }
    };
    FieldType { kind, array }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_table_resolves_every_field_and_walks_parents() {
        let types = Types::get();
        assert_eq!(types.classes.len(), 5255);
        let fields: usize = types.classes.values().map(|class| class.fields.len()).sum();
        assert_eq!(fields, 14194);
        let set_var = types.class_named("STUStatescriptActionSetVar").unwrap();
        assert_eq!(set_var.hash, 0x75001A77);
        let value = types.field_named(set_var.hash, "m_value").unwrap();
        assert_eq!(value.hash, 0x07DD813E);
        assert_eq!(
            value.ty,
            FieldType {
                kind: Kind::Embed,
                array: false
            }
        );
        // An inherited field: every node has its unique ID.
        let unique = types.field_named(set_var.hash, "m_uniqueID").unwrap();
        assert!(matches!(unique.ty.kind, Kind::Int { .. }));
        let action = types.class_named("STUStatescriptAction").unwrap().hash;
        assert!(types.is_a(set_var.hash, action));
    }
}
