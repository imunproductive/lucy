//! `list-heroes`: every hero asset (`075`) with a `STUHero`, by GUID
//! (`ListHeroes`, `DataModels/Hero/Hero.cs`, `Hero/Loadout.cs`).

use serde_json::{Map, Value as Json, json};

use super::{
    Data, array, enum_json, float_json, guid, guid_json, guids, instance, int, known_name,
};
use crate::guid_name;

const HERO: u16 = 0x075;

/// `Enum_0C014B4A`.
const GENDERS: &[(i64, &str)] = &[(1, "Generic"), (2, "Male"), (3, "Female")];
/// `STUHeroSize`.
const SIZES: &[(i64, &str)] = &[(0, "Normal"), (1, "Small"), (2, "Large")];
/// `LoadoutCategory`.
const LOADOUT_CATEGORIES: &[(i64, &str)] = &[
    (0, "Weapon"),
    (1, "Ability"),
    (2, "PassiveAbility"),
    (3, "UltimateAbility"),
];

pub(super) fn list(data: &Data) -> Json {
    let mut heroes = Map::new();
    for key in data.all(HERO) {
        let Some(asset) = data.asset(key) else {
            continue;
        };
        let Some(hero) = instance(&asset, "STUHero") else {
            continue;
        };
        heroes.insert(guid_name(key), self::hero(data, key, hero));
    }
    Json::Object(heroes)
}

fn hero(data: &Data, key: u64, hero: &orpheus_stu::Object) -> Json {
    let color = match hero.get("m_heroColor") {
        Some(orpheus_stu::Value::Floats(rgba)) if rgba.len() == 4 => {
            [rgba[0], rgba[1], rgba[2], rgba[3]]
        }
        _ => [0.0; 4],
    };
    // Loadouts whose asset has no `STULoadout` are left out.
    let loadouts = guids(hero, "m_heroLoadout").map(|loadouts| {
        loadouts
            .into_iter()
            .filter_map(|loadout| {
                let asset = data.asset(loadout)?;
                let loadout = instance(&asset, "STULoadout")?;
                Some(json!({
                    "Name": data.text(loadout, "m_name"),
                    "Description": data.text(loadout, "m_description"),
                    "Category": enum_json(LOADOUT_CATEGORIES, int(loadout, "m_category")),
                    "MovieGUID": guid_json(guid(loadout, "m_infoMovie")),
                    "TextureGUID": guid_json(guid(loadout, "m_texture")),
                }))
            })
            .collect::<Vec<_>>()
    });
    // Hero gallery portraits, small hero select icons and the like.
    let images = array(hero, "m_8203BFE1").map(|images| {
        images
            .iter()
            .filter_map(|image| match image {
                orpheus_stu::Value::Object(image) => Some(json!({
                    "Id": guid_json(guid(image, "m_id")),
                    "TextureGUID": guid_json(guid(image, "m_texture")),
                })),
                _ => None,
            })
            .collect::<Vec<_>>()
    });
    json!({
        "GUID": guid_json(key),
        // Blizzard ends some names with a space.
        "Name": data.string(guid(hero, "m_0EDCE350")).map(|name| name.trim_end_matches(' ').to_owned()),
        "Description": data.description(guid(hero, "m_3446F580")),
        "Class": known_name(guid(hero, "m_category")),
        "Gender": enum_json(GENDERS, int(hero, "m_gender")),
        "Size": enum_json(SIZES, int(hero, "m_heroSize")),
        "Color": hex(color),
        "sRGBColor": hex(non_linear(color)),
        "GalleryColor": {
            "R": float_json(color[0]),
            "G": float_json(color[1]),
            "B": float_json(color[2]),
            "A": float_json(color[3]),
        },
        "IsHero": int(hero, "m_62746D34") > 0,
        "SupportsAi": int(hero, "m_906C3711") > 0,
        "Loadouts": loadouts,
        "Images": images,
    })
}

/// TankLib's `teColorRGBA.ToHex`: `#RRGGBB A`, each channel times 255
/// rounded half to even, then the alpha as .NET prints a float.
fn hex([r, g, b, a]: [f32; 4]) -> String {
    let byte = |channel: f32| f64::from(channel * 255.0).round_ties_even() as i64 as u8;
    format!("#{:02X}{:02X}{:02X} {a}", byte(r), byte(g), byte(b))
}

/// TankLib's `teColorRGBA.ToNonLinear`: each color channel to the power
/// 1/2.2 (computed in single precision), alpha unchanged.
fn non_linear([r, g, b, a]: [f32; 4]) -> [f32; 4] {
    let exponent = f64::from(1.0f32 / 2.2f32);
    let channel = |value: f32| f64::from(value).powf(exponent) as f32;
    [channel(r), channel(g), channel(b), a]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colors_print_as_tanklib_does() {
        let reaper = [0.111_932_434, 0.0, 0.010_329_823, 1.0];
        assert_eq!(hex(reaper), "#1D0003 1");
        assert_eq!(hex(non_linear(reaper)), "#5E0020 1");
        assert_eq!(hex([0.0; 4]), "#000000 0");
    }
}
