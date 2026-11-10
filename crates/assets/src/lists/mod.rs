//! DataTool's list outputs: `extracted_heroes.json` (`list-heroes`),
//! `extracted_maps.json` (`list-maps`), `extracted_items.json`
//! (`list-unlocks`) and `extracted_general_unlocks.json`
//! (`list-general-unlocks`), each as `--json` writes it.
//!
//! Ported from OWLib's DataTool (MIT) at revision `7afe5f52`: the list tools
//! in `DataTool/ToolLogic/List` and the data models in `DataTool/DataModels`
//! they serialize. Only the members these files contain are ported. The JSON
//! keeps DataTool's member order; a GUID is written `%012X.%03X`, or `null`
//! when zero; an enum is written by its OWLib name.

mod heroes;
mod maps;
mod unlocks;

use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    path::Path,
    rc::Rc,
};

use orpheus_stu::{Asset, Object, Types, Value};
use serde_json::Value as Json;

use crate::{Install, Output, Result, guid_name, guid_type};

pub(crate) fn derive(install: &Install, _out: &Path) -> Result<Vec<Output>> {
    let data = Data::new(install);
    Ok(vec![
        Output::json("data/extracted_heroes.json", &heroes::list(&data)),
        Output::json("data/extracted_maps.json", &maps::list(&data)),
        Output::json("data/extracted_items.json", &unlocks::heroes(&data)),
        Output::json(
            "data/extracted_general_unlocks.json",
            &unlocks::general(&data),
        ),
    ])
}

/// The install's structured data and strings, each read once. The first
/// asset asked for of a type loads every asset of that type, as one at a time
/// would decode a bundle once per asset.
struct Data<'a> {
    install: &'a Install,
    assets: RefCell<HashMap<u64, Option<Rc<Asset>>>>,
    loaded: RefCell<HashSet<u16>>,
    strings: RefCell<HashMap<u64, Option<String>>>,
}

impl<'a> Data<'a> {
    fn new(install: &'a Install) -> Self {
        Self {
            install,
            assets: RefCell::default(),
            loaded: RefCell::default(),
            strings: RefCell::default(),
        }
    }

    /// Every GUID of one asset type with their assets loaded, in DataTool's
    /// order: by name (index, then type).
    fn all(&self, kind: u16) -> Vec<u64> {
        let mut guids = self.install.guids(kind);
        if self.loaded.borrow_mut().insert(kind) {
            let mut assets = self.assets.borrow_mut();
            for (guid, asset) in self.install.stu_all(&guids) {
                assets.insert(guid, asset.ok().map(Rc::new));
            }
        }
        guids.sort_by_key(|guid| (guid & 0xFFFF_FFFF_FFFF, *guid));
        guids
    }

    /// An asset, or `None` when the GUID is zero or the install can't give
    /// it, as DataTool's `OpenSTUSafe` treats both.
    fn asset(&self, guid: u64) -> Option<Rc<Asset>> {
        if guid == 0 {
            return None;
        }
        if !self.loaded.borrow().contains(&guid_type(guid)) {
            self.all(guid_type(guid));
        }
        self.assets.borrow().get(&guid).cloned().flatten()
    }

    /// A display string, `None` when the GUID is zero or missing
    /// (DataTool's `GetString`).
    fn string(&self, guid: u64) -> Option<String> {
        if let Some(text) = self.strings.borrow().get(&guid) {
            return text.clone();
        }
        let text = self.install.string(guid);
        self.strings.borrow_mut().insert(guid, text.clone());
        text
    }

    /// A string GUID field's text, as JSON.
    fn text(&self, object: &Object, field: &str) -> Json {
        self.string(guid(object, field)).into()
    }

    /// The text of a `STU_96ABC153` description (DataTool's
    /// `GetDescriptionString`).
    fn description(&self, guid: u64) -> Option<String> {
        let asset = self.asset(guid)?;
        let description = instance(&asset, "STU_96ABC153")?;
        self.string(self::guid(description, "m_94672A2A"))
    }
}

/// The first instance of a class or one derived from it (TankLib's
/// `teStructuredData.GetInstance<T>`).
fn instance<'a>(asset: &'a Asset, class: &str) -> Option<&'a Object> {
    let types = Types::get();
    let class = types.class_named(class)?.hash;
    asset
        .instances
        .iter()
        .flatten()
        .find(|object| types.is_a(object.class, class))
}

fn guid(object: &Object, field: &str) -> u64 {
    match object.get(field) {
        Some(Value::Guid(guid)) => *guid,
        _ => 0,
    }
}

fn int(object: &Object, field: &str) -> i64 {
    match object.get(field) {
        Some(Value::Int(value)) => *value,
        Some(Value::UInt(value)) => *value as i64,
        _ => 0,
    }
}

/// An array field's elements; `None` when the field is null.
fn array<'a>(object: &'a Object, field: &str) -> Option<&'a [Value]> {
    match object.get(field) {
        Some(Value::Array(items)) => Some(items),
        _ => None,
    }
}

/// An array field of GUIDs; `None` when the field is null.
fn guids(object: &Object, field: &str) -> Option<Vec<u64>> {
    array(object, field).map(|items| {
        items
            .iter()
            .map(|item| match item {
                Value::Guid(guid) => *guid,
                _ => 0,
            })
            .collect()
    })
}

/// An inline instance field.
fn inline<'a>(object: &'a Object, field: &str) -> Option<&'a Object> {
    match object.get(field) {
        Some(Value::Object(inner)) => Some(inner),
        _ => None,
    }
}

/// A GUID as DataTool's JSON writes it: `null` when zero.
fn guid_json(guid: u64) -> Json {
    if guid == 0 {
        Json::Null
    } else {
        guid_name(guid).into()
    }
}

/// A 32-bit float as DataTool's JSON writes it: its shortest decimal form.
fn float_json(value: f32) -> Json {
    let shortest: f64 = value.to_string().parse().expect("a float's own text");
    serde_json::Number::from_f64(shortest).map_or(Json::Null, Json::Number)
}

/// An enum by its OWLib member name, or its number when OWLib names none.
fn enum_json(names: &[(i64, &str)], value: i64) -> Json {
    names
        .iter()
        .find(|(number, _)| *number == value)
        .map_or_else(|| value.to_string(), |(_, name)| (*name).to_owned())
        .into()
}

/// The GUID with another asset type (TankLib's `teResourceGUID.SetType`).
fn with_type(guid: u64, kind: u16) -> u64 {
    let raw = ((kind - 1) << 4).reverse_bits();
    guid & !(0xFFF << 48) | u64::from(raw) << 48
}

/// A GUID's name in DataTool's `Static/GUIDNames.csv`, if it has one
/// (`GetNullableGUIDName`).
fn known_name(guid: u64) -> Option<&'static str> {
    let index = guid & 0xFFFF_FFFF_FFFF;
    let kind = guid_type(guid);
    GUID_NAMES
        .iter()
        .find(|(known_index, known_kind, _)| *known_index == index && *known_kind == kind)
        .map(|(_, _, name)| *name)
}

/// A GUID's name in `GUIDNames.csv`, else the GUID itself (`GetGUIDName`).
fn guid_label(guid: u64) -> String {
    known_name(guid).map_or_else(|| guid_name(guid), str::to_owned)
}

/// The rows of DataTool's `Static/GUIDNames.csv` (rev `7afe5f52`) for the two
/// asset types these lists name: identifiers (`01C`: hero classes, unlock
/// categories, celebrations) and map headers (`09F`: menu maps). Every row is
/// canonical, so the names carry no index suffix.
const GUID_NAMES: &[(u64, u16, &str)] = &[
    (0x850, 0x01C, "Tank"),
    (0x851, 0x01C, "Support"),
    (0x84E, 0x01C, "Damage"),
    (0x410B, 0x01C, "Base"),
    (0x3F9D, 0x01C, "SummerGames"),
    (0x3F9B, 0x01C, "Halloween"),
    (0x3F9E, 0x01C, "Winter"),
    (0x3FA1, 0x01C, "Lunar"),
    (0x3FA4, 0x01C, "Archives"),
    (0x3FA5, 0x01C, "Anniversary"),
    (0x3FA6, 0x01C, "OWL"),
    (0x41E9, 0x01C, "Legacy"),
    (0x3F9C, 0x01C, "Special"),
    (0x3FAA, 0x01C, "Achievement"),
    (0x471D, 0x01C, "Limited"),
    (0x4335, 0x01C, "ExtraSpecial"),
    (0x4058, 0x01C, "LegacyOWL"),
    (0x3FA7, 0x01C, "BCRF"),
    (0x7A9A, 0x01C, "Pachimari"),
    (0x7A82, 0x01C, "Blizzard"),
    (0x40A1, 0x01C, "Competitive"),
    (0x7ACD, 0x01C, "ESports"),
    (0x2168, 0x01C, "Christmas"),
    (0x2498, 0x01C, "LunarNewYear"),
    (0x237C, 0x01C, "LunarNewYear"),
    (0x2497, 0x01C, "NewYearResolution"),
    (0x24A6, 0x01C, "ValentinesDay"),
    (0x24A7, 0x01C, "ValentinesDay"),
    (0x2658, 0x01C, "StPatricksDay"),
    (0x2659, 0x01C, "Easter"),
    (0x212E, 0x01C, "Halloween"),
    (0x664, 0x09F, "Menu/Hanamura"),
    (0x675, 0x09F, "Menu/Gibraltar"),
    (0x676, 0x09F, "Menu/Kings Row"),
    (0x677, 0x09F, "Menu/Volskaya"),
    (0x678, 0x09F, "Menu/Hollywood"),
    (0x6E0, 0x09F, "Menu/Eichenwalde (Halloween)"),
    (0x700, 0x09F, "Menu/Kings Row (Winter) (2019)"),
    (0x710, 0x09F, "Menu/Busan (Lunar)"),
    (0x715, 0x09F, "Menu/Lijiang (Lunar)"),
    (0x764, 0x09F, "Menu/Kings Row (Archives)"),
    (0x794, 0x09F, "Menu/Horizon Lunar Colony"),
    (0x801, 0x09F, "Menu/Junkertown"),
    (0x817, 0x09F, "Menu/World Cup"),
    (0x85F, 0x09F, "Menu/Black Forest (Winter) (Day)"),
    (0x862, 0x09F, "Menu/OWL"),
    (0x864, 0x09F, "Menu/OWL"),
    (0x8B4, 0x09F, "Menu/Rialto (Archives)"),
    (0x8BE, 0x09F, "Menu/Rialto"),
    (0xA13, 0x09F, "Menu/OWL"),
    (0xA5A, 0x09F, "Menu/Chateau (Halloween)"),
    (0xA9E, 0x09F, "Menu/Blizzard World (Winter)"),
    (0xAF0, 0x09F, "Menu/Paris"),
    (0xB2D, 0x09F, "Menu/OWL All Stars (2020)"),
    (0xB55, 0x09F, "Menu/Havana"),
    (0xB56, 0x09F, "Menu/Havana (Archives)"),
    (0xBCE, 0x09F, "Menu/Summer Games (2020)"),
    (0xC04, 0x09F, "Menu/Eichenwalde (Alt)"),
    (0xC46, 0x09F, "Menu/Temple of Anubis (Alt)"),
    (0xC61, 0x09F, "Menu/Black Forest (Winter) (Night)"),
    (0xC83, 0x09F, "Menu/OWL"),
    (0xC8F, 0x09F, "Menu/Anniversary"),
    (0xCAC, 0x09F, "Menu/Route 66 (Alt)"),
    (0xCCF, 0x09F, "Menu/Anniversary"),
    (0xCE0, 0x09F, "Menu/Anniversary"),
    (0xCE6, 0x09F, "Menu/Paris (Alt)"),
    (0xCFE, 0x09F, "Menu/Sydney (Lucioball)"),
    (0xCFF, 0x09F, "Menu/Busan (Lucioball)"),
    (0xD0A, 0x09F, "Menu/OWL"),
    (0xD2C, 0x09F, "Menu/Tracer Comic Challenge"),
    (0xD2D, 0x09F, "Menu/OWL Grand Finals (2020)"),
    (0xD67, 0x09F, "Menu/Symmetra Challenge"),
    (0xD77, 0x09F, "Menu/Kings Row (Winter) (2020)"),
    (0xD7A, 0x09F, "Menu/Antarctica (Winter)"),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_type_round_trips_through_the_type_bits() {
        let map = 0x0800_0000_0000_005B;
        let header = with_type(map, 0x09F);
        assert_eq!(guid_type(header), 0x09F);
        assert_eq!(header & 0xFFFF_FFFF_FFFF, 0x5B);
        assert_eq!(guid_type(with_type(header, 0x002)), 0x002);
    }

    #[test]
    fn floats_keep_their_shortest_form() {
        assert_eq!(float_json(0.111_932_434).to_string(), "0.111932434");
        assert_eq!(float_json(1.0), serde_json::json!(1.0));
    }
}
