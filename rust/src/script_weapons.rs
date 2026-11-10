//! Damage and ballistics read from authored weapon volley nodes in build 104319.

use serde_json::Value as Json;

use crate::{
    script_expr::{EvalError, to_float},
    script_graph::Node,
    script_wire::Value,
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

#[derive(Clone, Debug, PartialEq)]
pub struct ProjectileBallistics {
    pub speed: f64,
    pub lifetime: f64,
    pub definition: Option<u64>,
    pub direct: Option<HitDefinition>,
    pub blast: Option<Blast>,
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
        Ok(Some(Self {
            speed,
            lifetime,
            definition,
            direct,
            blast,
        }))
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
    use crate::script_expr::{EmptyContext, evaluate};
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
}
