//! Damage and ballistics read from authored weapon volley nodes in build 104319.

use std::{collections::HashMap, fs, io, path::Path};

use serde::Deserialize;
use serde_json::Value as Json;

use crate::{
    expr::{EvalError, to_float},
    graph::{Node, asset_guid},
    wire::Value,
};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HitDefinition {
    pub amount: f64,
    pub loadout: Option<u64>,
    pub critical: f64,
    pub falloff: Option<DamageFalloff>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DamageFalloff {
    pub start: f64,
    pub end: f64,
    pub floor: f64,
}

impl HitDefinition {
    pub fn from_authored(
        config: &Json,
        mut evaluate: impl FnMut(&Json) -> Result<Value, EvalError>,
    ) -> Result<Option<Self>, EvalError> {
        if !config.is_object() {
            return Ok(None);
        }
        let mut field = |config: &Json, key: &str| {
            config
                .get(key)
                .filter(|value| !value.is_null())
                .map(&mut evaluate)
                .transpose()
        };
        let number = |value: Option<Value>| match value {
            Some(Value::Null) | None => None,
            Some(value) => Some(to_float(&value)).filter(|value| value.is_finite()),
        };
        let modify = &config["m_modifyHealth"];
        let amount = number(field(modify, "m_amount")?).unwrap_or(0.0);
        let loadout = match field(modify, "m_loadout")? {
            Some(Value::Asset(guid)) => Some(guid),
            _ => None,
        };
        let falloff = match (
            number(field(config, "m_EAD7F104")?),
            number(field(config, "m_157E5BC5")?),
            number(field(config, "m_72A81154")?),
        ) {
            (Some(start), Some(end), Some(floor)) if start < end => {
                Some(DamageFalloff { start, end, floor })
            }
            _ => None,
        };
        Ok(Some(Self {
            amount,
            loadout,
            critical: number(field(config, "m_A872C70E")?).unwrap_or(1.0),
            falloff,
        }))
    }

    pub fn damage_at(self, distance: f64, head: bool) -> f64 {
        let scale = self.falloff.map_or(1.0, |falloff| {
            if distance <= falloff.start {
                1.0
            } else if distance >= falloff.end {
                falloff.floor
            } else {
                1.0 + (falloff.floor - 1.0) * (distance - falloff.start)
                    / (falloff.end - falloff.start)
            }
        });
        self.amount * scale * if head { self.critical } else { 1.0 }
    }
}

const DEFAULT_LIFETIME: f64 = 5.0;
const RING_EPSILON: f64 = 1e-9;
/// The projectile motion class of an arc: `m_599AB8D7` is its downward
/// acceleration in m/s² (9.8 for Hanzo's arrows, Junkrat's grenades and
/// Torbjörn's rivets; Tracer's Pulse Bomb sets its variable `1CDD` to 30).
const GRAVITY_MOTION: &str = "STU_E1A5B579";
/// Seconds a projectile that sticks waits before it explodes. No asset
/// names it (assumption): the volleys that stick (Tracer's Pulse Bomb,
/// graph `0034` state 0, and Echo's Sticky Bombs, `1C36` state 0) give no
/// delay, and neither does the Pulse Bomb's entity (`0045.003`: a visual,
/// model, target tag and statescript component). One second is the public
/// rule for the Pulse Bomb and what ow1-preservation's server used (run 392,
/// `HANDOFF.md:1770`). The explosion's `m_2FE97560` is 1.0 on the Pulse
/// Bomb too, but also on explosives that go off where they hit (Soldier:
/// 76's Helix Rockets, `0254`; Pharah's rocket has 1.875), so it isn't read
/// as the fuse.
pub const STICK_FUSE: f64 = 1.0;

#[derive(Clone, Debug, PartialEq)]
pub struct ProjectileBallistics {
    pub speed: f64,
    pub lifetime: f64,
    pub definition: Option<u64>,
    pub direct: Option<HitDefinition>,
    pub blast: Option<Blast>,
    /// The arc's downward acceleration in m/s²: the sum of the volley's
    /// gravity motions (`m_projectileMotions`); 0 flies straight.
    pub gravity: f64,
    /// A projectile that sticks to the first body or surface it hits and
    /// explodes this many seconds later; None ends where it hits.
    pub fuse: Option<f64>,
    /// The volley's motion classes the flight doesn't model, which it flies
    /// as if they weren't there (Pharah's `STU_7A5DFFF8`).
    pub unmodelled: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Blast {
    pub hit: HitDefinition,
    pub rings: Vec<(f64, f64)>,
}

impl ProjectileBallistics {
    pub fn from_authored(
        node: &Node,
        mut evaluate: impl FnMut(&Json) -> Result<Value, EvalError>,
    ) -> Result<Option<Self>, EvalError> {
        let speed = authored_number(node.field("m_projectileSpeed"), 0.0, &mut evaluate)?;
        if !speed.is_finite() || speed <= 0.0 {
            return Ok(None);
        }
        let mut lifetime = authored_number(
            node.field("m_projectileLifetime"),
            DEFAULT_LIFETIME,
            &mut evaluate,
        )?;
        if lifetime == 0.0 {
            lifetime = DEFAULT_LIFETIME;
        }
        let reach = authored_number(node.field("m_FEC435C6"), 0.0, &mut evaluate)?;
        if reach != 0.0 {
            lifetime = lifetime.min(reach / speed);
        }
        if !lifetime.is_finite() || lifetime <= 0.0 {
            return Ok(None);
        }
        let definition = match node.field("m_projectileEntity.m_entityDef") {
            Some(config) => match evaluate(config)? {
                Value::Asset(definition) => Some(definition),
                _ => None,
            },
            None => None,
        };
        let direct = HitDefinition::from_authored(
            node.field("m_6396149F").unwrap_or(&Json::Null),
            &mut evaluate,
        )?;
        let config = node.field("m_D2D11CE9").unwrap_or(&Json::Null);
        let blast = if let Some(hit) = HitDefinition::from_authored(config, &mut evaluate)? {
            let mut rings = Vec::new();
            if let Some(rows) = config.get("m_0D30E40C").and_then(Json::as_array) {
                for row in rows.iter().filter(|row| row.is_object()) {
                    let radius = authored_number(row.get("m_radius"), 0.0, &mut evaluate)?;
                    let scalar = authored_number(row.get("m_B399CACB"), 0.0, &mut evaluate)?;
                    if radius.is_finite() && scalar.is_finite() {
                        rings.push((radius, scalar));
                    }
                }
            }
            rings.sort_by(|a, b| a.0.total_cmp(&b.0).then_with(|| a.1.total_cmp(&b.1)));
            Some(Blast { hit, rings })
        } else {
            None
        };
        // Beyond Python, which flew every projectile straight and ended it
        // where it hit (ow174/game/combat.py `_fly`): the volley's arc and
        // whether it sticks.
        let mut gravity = 0.0;
        let mut unmodelled = Vec::new();
        if let Some(motions) = node.field("m_projectileMotions").and_then(Json::as_array) {
            for motion in motions.iter().filter(|motion| motion.is_object()) {
                match motion.get("$").and_then(Json::as_str) {
                    Some(GRAVITY_MOTION) => {
                        gravity += authored_number(motion.get("m_599AB8D7"), 0.0, &mut evaluate)?;
                    }
                    class => unmodelled.push(class.unwrap_or("?").to_owned()),
                }
            }
        }
        if !gravity.is_finite() {
            return Ok(None);
        }
        // The collision config's `m_47999E82` is set on the volleys that
        // stick to what they hit and on no other (assumption: its meaning
        // is read from those two, the Pulse Bomb and Echo's Sticky Bombs).
        let sticks =
            authored_number(node.field("m_55E8761C.m_47999E82"), 0.0, &mut evaluate)? != 0.0;
        Ok(Some(Self {
            speed,
            lifetime,
            definition,
            direct,
            blast,
            gravity,
            fuse: sticks.then_some(STICK_FUSE),
            unmodelled,
        }))
    }

    /// The seconds the projectile lives at most: its flight, then its fuse.
    pub fn bound(&self) -> f64 {
        self.lifetime + self.fuse.unwrap_or(0.0)
    }

    /// Whether it does damage where it hits or explodes (combat.py `_shoot`:
    /// `damaging`).
    pub fn damaging(&self) -> bool {
        self.direct.is_some_and(|hit| hit.amount > 0.0)
            || self
                .blast
                .as_ref()
                .is_some_and(|blast| blast.hit.amount > 0.0)
    }
}

/// The derived tunables file, in the runtime `data` folder: every tunable
/// value (type `130`, `STU_A0E5F3ED`) in the install, its `m_3E5360A1` by
/// GUID (the `orpheus-assets` derivation `game_tunables`).
pub const TUNABLES_FILE: &str = "game_tunables_174.json";

/// The tunable values the graphs read through `STU_082FA44E`, by tunable
/// GUID. One read decides play: Tracer's Pulse Bomb damage, graph `0034`
/// node 51, sets variable `01DC` to 450 in the modes whose mode variable
/// `3460` is set, else to tunable `0130:015D`, 350.
#[derive(Debug, Default)]
pub struct Tunables {
    values: HashMap<u64, f64>,
}

#[derive(Deserialize)]
struct RawTunables {
    tunables: HashMap<String, f64>,
}

impl Tunables {
    pub fn load(path: impl AsRef<Path>) -> io::Result<Self> {
        let path = path.as_ref();
        let data = fs::read(path).map_err(|error| {
            io::Error::new(error.kind(), format!("{}: {error}", path.display()))
        })?;
        Self::from_json(&data)
    }

    pub fn from_json(data: &[u8]) -> io::Result<Self> {
        let invalid = |error: String| io::Error::new(io::ErrorKind::InvalidData, error);
        let raw: RawTunables =
            serde_json::from_slice(data).map_err(|error| invalid(error.to_string()))?;
        let values = raw
            .tunables
            .into_iter()
            .map(|(guid, value)| {
                asset_guid(&Json::String(guid.clone()))
                    .map(|guid| (guid, value))
                    .ok_or_else(|| invalid(format!("invalid tunable GUID {guid:?}")))
            })
            .collect::<io::Result<_>>()?;
        Ok(Self { values })
    }

    pub fn len(&self) -> usize {
        self.values.len()
    }

    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    /// `STU_082FA44E` (a float config): the authored value of its tunable
    /// (`m_DBF61E9A`); none for a tunable the install lacks. Its entity
    /// operand (`m_entity`) could scale the value by something of that
    /// entity's; no asset says how, so it is left out (assumption).
    pub fn value(&self, config: &Json) -> Option<f64> {
        let guid = config
            .get("m_DBF61E9A")?
            .get("m_DBF61E9A")
            .and_then(asset_guid)?;
        self.values.get(&guid).copied()
    }
}

impl Blast {
    pub fn multiplier(&self, distance: f64) -> f64 {
        let Some(&(mut previous, mut scalar)) = self.rings.first() else {
            return 0.0;
        };
        if distance <= previous {
            return scalar;
        }
        for &(radius, next) in &self.rings[1..] {
            if distance <= radius {
                let share = (distance - previous) / (radius - previous).max(RING_EPSILON);
                return scalar + (next - scalar) * share;
            }
            previous = radius;
            scalar = next;
        }
        0.0
    }
}

pub fn authored_number(
    config: Option<&Json>,
    default: f64,
    evaluate: &mut impl FnMut(&Json) -> Result<Value, EvalError>,
) -> Result<f64, EvalError> {
    let Some(config) = config.filter(|value| !value.is_null()) else {
        return Ok(default);
    };
    let value = evaluate(config)?;
    Ok(if value == Value::Null {
        default
    } else {
        to_float(&value)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::expr::{EmptyContext, evaluate};
    use serde_json::json;

    #[test]
    fn evaluates_authored_hit_config_with_the_shared_expression_engine() {
        let int = |value| json!({"$": "STUConfigVarInt", "m_value": value});
        let config = json!({
            "m_modifyHealth": {
                "m_amount": int(20),
                "m_loadout": {"$": "STUConfigVarLoadout", "m_loadout": "0x0B9000000000008D"}
            },
            "m_EAD7F104": int(30),
            "m_157E5BC5": int(50),
            "m_72A81154": {"$": "STUConfigVarFloat", "m_value": 0.3},
            "m_A872C70E": int(2)
        });
        let hit = HitDefinition::from_authored(&config, |value| evaluate(value, &mut EmptyContext))
            .unwrap()
            .unwrap();
        assert_eq!(hit.amount, 20.0);
        assert_eq!(hit.loadout, Some(0x0b90_0000_0000_008d));
        assert_eq!(hit.critical, 2.0);
        assert_eq!(hit.damage_at(10.0, false), 20.0);
        assert!((hit.damage_at(40.0, false) - 13.0).abs() < 1e-6);
        assert!((hit.damage_at(60.0, true) - 12.0).abs() < 1e-6);
        let defaults = HitDefinition::from_authored(&json!({}), |_| panic!("no fields"))
            .unwrap()
            .unwrap();
        assert_eq!(
            (
                defaults.amount,
                defaults.loadout,
                defaults.critical,
                defaults.falloff
            ),
            (0.0, None, 1.0, None)
        );
    }

    /// Graph `0034` node 51's `STU_082FA44E` reads its tunable's value from
    /// the derived table; a tunable the table lacks reads nothing.
    #[test]
    fn a_tunable_config_reads_its_value_from_the_table() {
        let tunables = Tunables::from_json(
            br#"{"source": "", "tunables": {"0x0F4800000000015D": 350.0, "0x0F48000000000001": 0.25}}"#,
        )
        .unwrap();
        let config = |guid: &str| {
            json!({"$": "STU_082FA44E",
                "m_DBF61E9A": {"$": "STU_D9BDAA46", "m_DBF61E9A": guid, "m_resourceKey": null},
                "m_entity": {"$": "STU_B5A0CAF0"}})
        };
        assert_eq!(tunables.value(&config("0x0F4800000000015D")), Some(350.0));
        assert_eq!(tunables.value(&config("0x0F48000000000001")), Some(0.25));
        assert_eq!(tunables.value(&config("0x0F4800000000015E")), None);
        assert_eq!(tunables.value(&json!({"$": "STU_082FA44E"})), None);
        assert!(Tunables::from_json(br#"{"tunables": {"nope": 1.0}}"#).is_err());
    }

    /// The install's tunable `0130:015D` (Tracer's Pulse Bomb damage) is 350.
    #[test]
    #[ignore = "requires the 1.74 game data"]
    fn the_install_s_pulse_bomb_tunable_is_350() {
        let data = orpheus_testkit::runtime_dir().join("data");
        let tunables = Tunables::load(data.join(TUNABLES_FILE)).unwrap();
        let config = json!({"$": "STU_082FA44E",
            "m_DBF61E9A": {"$": "STU_D9BDAA46", "m_DBF61E9A": "0x0F4800000000015D"}});
        assert_eq!(tunables.value(&config), Some(350.0));
    }
}
