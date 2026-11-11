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
    /// The share of the explosion's damage its shooter takes where it
    /// reaches him: `m_8A3E3883` (none: 0). Read as the self-damage scale
    /// (assumption): 0.5 on Soldier: 76's Helix Rockets (`0254`), Pharah's
    /// rockets (`0023`), Zarya's (`0241`), Echo's (`1C36`) and Junkrat's
    /// `022A`, 0.25 on Bastion (`024F`) and Sigma (`187F`), and 0 on the
    /// Junkrat volleys that, by the public rule, don't hurt him (`022B`
    /// node 29, `096A`).
    pub self_damage: f64,
    /// The push of the explosion's ModifyHealth, if it has one.
    pub knockback: Option<Knockback>,
}

/// The push an explosion's ModifyHealth gives a body (`m_modifyHealth`):
/// `m_E33EFEE1` metres per second away from it, `m_10399AE7` upward, as
/// leafall903/ORPHEUS (MIT) `heavy_assault.py` `_push` reads the pair, each
/// scaled by the ModifyHealth's knockback rings (`m_knockbackRings`,
/// `STUStatescriptModifyHealthKnockbackRing`) at the body's distance.
#[derive(Clone, Debug, PartialEq)]
pub struct Knockback {
    pub horizontal: f64,
    pub upward: f64,
    /// Each ring's radius and its horizontal scale, `m_579D7CFF`
    /// (assumption: the ring's three scalars are unnamed; `m_9102DA3F` is
    /// absent from Pharah's and Sigma's rings, which have the other two, so
    /// it isn't read), by radius.
    pub horizontal_rings: Vec<(f64, f64)>,
    /// Each ring's radius and its upward scale, `m_10399AE7` (assumption:
    /// the ring field shares its name with the upward speed), by radius.
    pub upward_rings: Vec<(f64, f64)>,
}

impl Knockback {
    /// The horizontal and upward speeds at `distance` from the explosion,
    /// each between its rings as an explosion's damage is (assumption: no
    /// asset says how they blend); none past the last ring.
    pub fn at(&self, distance: f64) -> Option<(f64, f64)> {
        let horizontal = self.horizontal * ring_scale(&self.horizontal_rings, distance);
        let upward = self.upward * ring_scale(&self.upward_rings, distance);
        (horizontal > 0.0 || upward > 0.0).then_some((horizontal, upward))
    }

    /// The push of an explosion's ModifyHealth (`modify`). None without
    /// rings, or when either speed isn't a finite number of at least 0 or
    /// both are 0 (`_push` pushes nothing then; Sigma's `187F` pull,
    /// −0.75, is left out with it). A ring's absent scale is 1 (assumption).
    fn from_authored(
        modify: &Json,
        evaluate: &mut impl FnMut(&Json) -> Result<Value, EvalError>,
    ) -> Result<Option<Self>, EvalError> {
        let horizontal = authored_number(modify.get("m_E33EFEE1"), 0.0, evaluate)?;
        let upward = authored_number(modify.get("m_10399AE7"), 0.0, evaluate)?;
        if ![horizontal, upward]
            .iter()
            .all(|speed| speed.is_finite() && *speed >= 0.0)
            || (horizontal == 0.0 && upward == 0.0)
        {
            return Ok(None);
        }
        let rows = modify.get("m_knockbackRings");
        let horizontal_rings = authored_rings(rows, "m_579D7CFF", 1.0, evaluate)?;
        let upward_rings = authored_rings(rows, "m_10399AE7", 1.0, evaluate)?;
        Ok((!horizontal_rings.is_empty()).then_some(Self {
            horizontal,
            upward,
            horizontal_rings,
            upward_rings,
        }))
    }
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
            let rings = authored_rings(config.get("m_0D30E40C"), "m_B399CACB", 0.0, &mut evaluate)?;
            let self_damage = authored_number(config.get("m_8A3E3883"), 0.0, &mut evaluate)?;
            Some(Blast {
                hit,
                rings,
                self_damage: if self_damage.is_finite() {
                    self_damage.max(0.0)
                } else {
                    0.0
                },
                knockback: Knockback::from_authored(&config["m_modifyHealth"], &mut evaluate)?,
            })
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
        ring_scale(&self.rings, distance)
    }
}

/// A ring's scale at `distance`: the first ring's within its radius, then
/// straight between each ring and the next, none past the last.
fn ring_scale(rings: &[(f64, f64)], distance: f64) -> f64 {
    let Some(&(mut previous, mut scalar)) = rings.first() else {
        return 0.0;
    };
    if distance <= previous {
        return scalar;
    }
    for &(radius, next) in &rings[1..] {
        if distance <= radius {
            let share = (distance - previous) / (radius - previous).max(RING_EPSILON);
            return scalar + (next - scalar) * share;
        }
        previous = radius;
        scalar = next;
    }
    0.0
}

/// Ring rows: each one's `m_radius` and its `scale` field (absent:
/// `default`), the finite ones, by radius.
fn authored_rings(
    rows: Option<&Json>,
    scale: &str,
    default: f64,
    evaluate: &mut impl FnMut(&Json) -> Result<Value, EvalError>,
) -> Result<Vec<(f64, f64)>, EvalError> {
    let mut rings = Vec::new();
    for row in rows
        .and_then(Json::as_array)
        .into_iter()
        .flatten()
        .filter(|row| row.is_object())
    {
        let radius = authored_number(row.get("m_radius"), 0.0, evaluate)?;
        let scalar = authored_number(row.get(scale), default, evaluate)?;
        if radius.is_finite() && scalar.is_finite() {
            rings.push((radius, scalar));
        }
    }
    rings.sort_by(|a, b| a.0.total_cmp(&b.0).then_with(|| a.1.total_cmp(&b.1)));
    Ok(rings)
}

/// The launch class whose `m_0D30E40C` rows lay a shot's projectiles out
/// in rings (`STU_B101467F`: `m_6425FFB9` the ring's share of `m_spread`
/// degrees, `m_B19BD35D` its pellets).
const RING_LAUNCH: &str = "STU_613BDDB0";
const RING_ROW: &str = "STU_B101467F";
/// The most pellets the server flies for one shot (upstream's limit of
/// its collision work, `projectiles.py` `MAX_PELLETS`).
const MAX_PELLETS: f64 = 64.0;
/// The launch class whose spread grows with the shots (`m_AC20B1C1` the
/// ramp; `m_7F0FED02` and `m_429A6C85` the cone in degrees at no and full
/// spread).
const SPREAD_LAUNCH: &str = "STU_239B571E";

/// The pellet rings of a volley that fires several projectiles a shot:
/// each ring's tilt from the aim in radians and its pellets, in authored
/// order. None when the launch has no rings, or they don't add up to the
/// volley's `m_numProjectilesPerShot` (2 to 64), a ring's share is outside
/// 0 to 1, or the spread outside 0° to 180°. Ported from leafall903/ORPHEUS
/// (MIT) `projectiles.py` `launch_directions`, commit 7cb4018 (Doomfist's
/// Hand Cannon: 1, 4 and 6 pellets at 0, 0.5 and 1 of 2.5°). Unlike
/// upstream, this holds for any sync, not only SHOTS_ONLY.
pub fn pellet_rings(
    node: &Node,
    mut evaluate: impl FnMut(&Json) -> Result<Value, EvalError>,
) -> Result<Option<Vec<(f64, u32)>>, EvalError> {
    let launch = match node.field("m_5EC1EB38") {
        Some(launch) if launch.get("$").and_then(Json::as_str) == Some(RING_LAUNCH) => launch,
        _ => return Ok(None),
    };
    // Upstream reads only plain numbers: anything else is no layout.
    let mut number = |config: Option<&Json>| -> Result<Option<f64>, EvalError> {
        let Some(config) = config.filter(|config| !config.is_null()) else {
            return Ok(None);
        };
        Ok(match evaluate(config)? {
            Value::Integer(value) => Some(value as f64),
            Value::Float(value) if value.is_finite() => Some(value),
            _ => None,
        })
    };
    let Some(count) = number(node.field("m_numProjectilesPerShot"))?
        .filter(|count| (2.0..=MAX_PELLETS).contains(count))
    else {
        return Ok(None);
    };
    let Some(rows) = launch
        .get("m_0D30E40C")
        .and_then(Json::as_array)
        .filter(|rows| rows.len() as f64 <= count)
    else {
        return Ok(None);
    };
    let Some(spread) =
        number(launch.get("m_spread"))?.filter(|spread| (0.0..=180.0).contains(spread))
    else {
        return Ok(None);
    };
    let mut rings = Vec::with_capacity(rows.len());
    for row in rows {
        if row.get("$").and_then(Json::as_str) != Some(RING_ROW) {
            return Ok(None);
        }
        let share = number(row.get("m_6425FFB9"))?;
        let pellets = number(row.get("m_B19BD35D"))?;
        let (Some(share), Some(pellets)) = (share, pellets) else {
            return Ok(None);
        };
        if !(0.0..=1.0).contains(&share)
            || !(1.0..=count).contains(&pellets)
            || pellets.fract() != 0.0
        {
            return Ok(None);
        }
        rings.push(((share * spread).to_radians(), pellets as u32));
    }
    let total: f64 = rings.iter().map(|&(_, pellets)| f64::from(pellets)).sum();
    Ok((total == count).then_some(rings))
}

/// The cone of a growing spread at `normalized` (0 to 1): `m_7F0FED02`
/// degrees plus `normalized` of the way to `m_429A6C85`, in radians, at
/// float32 steps. None when the launch isn't `STU_239B571E`. Ported from
/// leafall903/ORPHEUS (MIT) `mccree.py` `fan_direction`, commit c239a84.
pub fn spread_cone(
    node: &Node,
    normalized: f64,
    mut evaluate: impl FnMut(&Json) -> Result<Value, EvalError>,
) -> Result<Option<f32>, EvalError> {
    let launch = match node.field("m_5EC1EB38") {
        Some(launch) if launch.get("$").and_then(Json::as_str) == Some(SPREAD_LAUNCH) => launch,
        _ => return Ok(None),
    };
    let low = authored_number(launch.get("m_7F0FED02"), 0.0, &mut evaluate)?;
    let high = authored_number(launch.get("m_429A6C85"), 0.0, &mut evaluate)?;
    let degrees = (f64::from((high - low) as f32) * normalized) as f32 + low as f32;
    Ok(Some(degrees * (std::f64::consts::PI / 180.0) as f32))
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

    fn volley(fields: Json) -> Node {
        serde_json::from_value(json!({"pos":2,"class":"STUStatescriptStateWeaponVolley",
            "uid":2,"rid":2,"remote":-1,"state":1,"client_only":false,"server_only":false,
            "fields":fields}))
        .unwrap()
    }

    fn rings(count: i64, spread: Json, rows: &[(f64, i64)]) -> Node {
        let rows: Vec<_> = rows
            .iter()
            .map(|&(share, pellets)| {
                json!({"$":"STU_B101467F","m_6425FFB9":{"$":"STUConfigVarFloat","m_value":share},
                    "m_B19BD35D":{"$":"STUConfigVarInt","m_value":pellets}})
            })
            .collect();
        volley(json!({
            "m_numProjectilesPerShot":{"$":"STUConfigVarInt","m_value":count},
            "m_5EC1EB38":{"$":"STU_613BDDB0","m_0D30E40C":rows,"m_spread":spread}
        }))
    }

    /// An explosion's self-damage scale and push, shaped like Pharah's
    /// rockets' (`0023`: 5 and 8 m/s, no `m_9102DA3F` in the rings); a ring
    /// scale left out is 1. A pull (Sigma's `187F`, −0.75) or a push
    /// without rings is none, and an explosion without `m_8A3E3883` doesn't
    /// hurt its shooter.
    #[test]
    fn an_explosion_reads_its_self_damage_and_push() {
        let float = |value: f64| json!({"$":"STUConfigVarFloat","m_value":value});
        let ring = |radius: f64, horizontal: Option<f64>, upward: f64| {
            json!({"$":"STUStatescriptModifyHealthKnockbackRing","m_radius":float(radius),
                "m_579D7CFF":horizontal.map(float),"m_10399AE7":float(upward)})
        };
        let rocket = |modify: Json, self_damage: Option<f64>| {
            let node = volley(json!({"m_projectileSpeed":float(35.0),
                "m_D2D11CE9":{"$":"STU_4AA6CA44","m_8A3E3883":self_damage.map(float),
                    "m_0D30E40C":[{"m_radius":float(2.5),"m_B399CACB":float(1.0)}],
                    "m_modifyHealth":modify}}));
            ProjectileBallistics::from_authored(&node, |value| evaluate(value, &mut EmptyContext))
                .unwrap()
                .unwrap()
                .blast
                .unwrap()
        };
        let pharah = rocket(
            json!({"m_amount":float(120.0),"m_E33EFEE1":float(5.0),"m_10399AE7":float(8.0),
                "m_knockbackRings":[ring(2.5, Some(0.0), 0.0), ring(0.5, None, 0.8)]}),
            Some(0.5),
        );
        assert_eq!(pharah.self_damage, 0.5);
        let knockback = pharah.knockback.unwrap();
        assert_eq!(knockback.horizontal_rings, [(0.5, 1.0), (2.5, 0.0)]);
        // A float config var is a float32.
        let scale = f64::from(0.8f32);
        assert_eq!(knockback.upward_rings, [(0.5, scale), (2.5, 0.0)]);
        assert_eq!(knockback.at(0.5), Some((5.0, 8.0 * scale)));
        assert_eq!(knockback.at(2.5), None);
        let pull = rocket(
            json!({"m_amount":float(50.0),"m_E33EFEE1":float(-0.75),"m_10399AE7":float(1.0),
                "m_knockbackRings":[ring(0.5, Some(0.8), 1.0)]}),
            None,
        );
        assert_eq!((pull.self_damage, pull.knockback), (0.0, None));
        let ringless = rocket(
            json!({"m_amount":float(50.0),"m_E33EFEE1":float(5.0),"m_10399AE7":float(8.0)}),
            None,
        );
        assert_eq!(ringless.knockback, None);
    }

    /// Graph `0D0A` state 2's rings (upstream 7cb4018): 1, 4 and 6 pellets
    /// at 0, 0.5 and 1 of the spread, in degrees; anything that doesn't add
    /// up, or isn't a plain number in range, is no layout.
    #[test]
    fn pellet_rings_read_the_authored_layout() {
        let read = |node: &Node| pellet_rings(node, |value| evaluate(value, &mut EmptyContext));
        let float = |value: f64| json!({"$":"STUConfigVarFloat","m_value":value});
        let hand_cannon = [(0.0, 1), (0.5, 4), (1.0, 6)];
        let layout = read(&rings(11, float(2.5), &hand_cannon)).unwrap().unwrap();
        assert_eq!(
            layout,
            [
                (0.0, 1),
                (1.25f64.to_radians(), 4),
                (2.5f64.to_radians(), 6)
            ]
        );
        for node in [
            rings(12, float(2.5), &hand_cannon),
            rings(11, float(181.0), &hand_cannon),
            rings(11, json!(null), &hand_cannon),
            rings(1, float(2.5), &[(0.0, 1)]),
            rings(11, float(2.5), &[(0.0, 1), (1.5, 4), (1.0, 6)]),
            rings(11, float(2.5), &[(0.0, 1), (0.5, 4), (1.0, 6), (1.0, 0)]),
            volley(json!({"m_numProjectilesPerShot":3,
                "m_5EC1EB38":{"$":"STU_1145C42E","m_spread":9}})),
        ] {
            assert_eq!(read(&node).unwrap(), None);
        }
    }

    /// mccree.py `fan_direction`: low + (high - low) × normalized degrees,
    /// in radians as float32; another launch has no cone.
    #[test]
    fn spread_cone_runs_from_low_to_high() {
        let node = volley(json!({"m_5EC1EB38":{"$":"STU_239B571E",
            "m_7F0FED02":{"$":"STUConfigVarFloat","m_value":0.5},
            "m_429A6C85":{"$":"STUConfigVarFloat","m_value":2.5}}}));
        let cone = |normalized| {
            spread_cone(&node, normalized, |value| {
                evaluate(value, &mut EmptyContext)
            })
            .unwrap()
            .unwrap()
        };
        assert!((cone(0.0) - 0.5f32.to_radians()).abs() < 1e-8);
        assert!((cone(0.25) - 1.0f32.to_radians()).abs() < 1e-8);
        assert!((cone(1.0) - 2.5f32.to_radians()).abs() < 1e-8);
        let other = volley(json!({"m_5EC1EB38":{"$":"STU_613BDDB0"}}));
        assert_eq!(
            spread_cone(&other, 1.0, |_| panic!("no configs")).unwrap(),
            None
        );
    }
}
