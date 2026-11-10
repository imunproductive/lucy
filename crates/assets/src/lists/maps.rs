//! `list-maps`: every map header (`09F`) with a `STUMapHeader`, by GUID
//! (`ListMaps`, `DataModels/MapHeader.cs`, `GameModes/GameMode.cs`).

use orpheus_stu::{Object, Value};
use serde_json::{Map, Value as Json, json};

use super::{
    Data, array, enum_json, guid, guid_json, guid_label, guids, instance, int, known_name,
    with_type,
};
use crate::guid_name;

const MAP_HEADER: u16 = 0x09F;

/// `Enum_668FA6B6`.
const STATES: &[(i64, &str)] = &[(0, "Enabled"), (1, "Disabled")];
/// `STUMapType`.
const MAP_TYPES: &[(i64, &str)] = &[(0, "OffenseDefenseMap"), (1, "MirroredMap"), (2, "PVE")];

pub(super) fn list(data: &Data) -> Json {
    let mut maps = Map::new();
    for key in data.all(MAP_HEADER) {
        if let Some(map) = MapHeader::read(data, key) {
            maps.insert(guid_name(key), map);
        }
    }
    Json::Object(maps)
}

/// A map header's names, the part `MapHeaderLite` keeps.
struct MapHeader<'a> {
    key: u64,
    header: &'a Object,
    name: Option<String>,
    variant_name: Option<String>,
}

impl MapHeader<'_> {
    /// The header of `key`, `None` without a `STUMapHeader`.
    fn read(data: &Data, key: u64) -> Option<Json> {
        let asset = data.asset(key)?;
        let header = instance(&asset, "STUMapHeader")?;
        Some(MapHeader::new(data, key, header).json(data))
    }

    fn new<'a>(data: &Data, key: u64, header: &'a Object) -> MapHeader<'a> {
        // A menu map's name in DataTool's GUID names replaces its display
        // name; the header shares its index with the map (`m_map`).
        let name = known_name(with_type(guid(header, "m_map"), MAP_HEADER))
            .map(str::to_owned)
            .or_else(|| data.string(guid(header, "m_displayName")));
        MapHeader {
            key,
            header,
            name,
            variant_name: data.string(guid(header, "m_overrideName")),
        }
    }

    fn json(&self, data: &Data) -> Json {
        let header = self.header;
        // Game modes without a `STUGameMode` are left out.
        let modes = guids(header, "m_supportedGamemodes").map(|modes| {
            modes
                .into_iter()
                .filter_map(|mode| {
                    let asset = data.asset(mode)?;
                    let stu = instance(&asset, "STUGameMode")?;
                    Some(json!({"GUID": guid_json(mode), "Name": data.text(stu, "m_displayName")}))
                })
                .collect::<Vec<_>>()
        });
        // Seasonal versions of the map: the celebration and the map header
        // shown during it, which shares the variant map's index.
        let variants = array(header, "m_celebrationOverrides").map(|variants| {
            variants
                .iter()
                .filter_map(|variant| match variant {
                    Value::Object(variant) => Some(variant),
                    _ => None,
                })
                .map(|variant| {
                    let celebration = guid(variant, "m_celebrationType");
                    let key = with_type(guid(variant, "m_map"), MAP_HEADER);
                    json!({
                        "Name": guid_label(celebration),
                        "Virtual01C": guid_json(celebration),
                        "MapInfo": lite(data, key),
                    })
                })
                .collect::<Vec<_>>()
        });
        json!({
            "GUID": guid_json(self.key),
            "Name": self.name,
            "Description": data.text(header, "m_389CB894"),
            "Description2": data.text(header, "m_ACB95597"),
            "Subline": data.text(header, "m_EBCFAD22"),
            "StateA": data.text(header, "m_8EBADA44"),
            "StateB": data.text(header, "m_5AFE2F61"),
            "VariantName": self.variant_name,
            "MapGUID": guid_json(guid(header, "m_map")),
            "GameModes": modes,
            "State": enum_json(STATES, int(header, "m_A125818B")),
            "MapType": enum_json(MAP_TYPES, int(header, "m_mapType")),
            "Thumbnail": guid_json(guid(header, "m_smallMapIcon")),
            "Image": guid_json(guid(header, "m_loadingScreen")),
            "FlagImage": guid_json(guid(header, "m_loadingScreenFlag")),
            "CelebrationVariants": variants,
        })
    }
}

/// `MapHeaderLite`: GUID and names, all null without a `STUMapHeader`.
fn lite(data: &Data, key: u64) -> Json {
    let asset = data.asset(key);
    match asset
        .as_deref()
        .and_then(|asset| instance(asset, "STUMapHeader"))
    {
        Some(header) => {
            let map = MapHeader::new(data, key, header);
            json!({"GUID": guid_json(key), "Name": map.name, "VariantName": map.variant_name})
        }
        None => json!({"GUID": null, "Name": null, "VariantName": null}),
    }
}
