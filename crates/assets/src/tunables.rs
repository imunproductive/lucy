//! `data/game_tunables_174.json`: every tunable value in the install (type
//! `130`), the values the graphs read through `STU_082FA44E`. A tunable is
//! an `STU_A0E5F3ED` (its float `m_3E5360A1`) or an `STU_AA802F91` (its
//! float `m_value`); the file maps the tunable's GUID to that value. The
//! other `130` assets hold no single value: bare `STU_E35FF0F6`s group
//! tunables (their `m_parent`), and `STU_23102F10` holds several floats no
//! config reads, so they are left out.

use std::path::Path;

use lucy_stu::Value;
use serde_json::{Map, json};

use crate::{
    Install, Output, Result, asset_error,
    game_queues::{Node, key, load, typed},
};

pub(crate) const TUNABLES: &str = "data/game_tunables_174.json";

const TUNABLE: u16 = 0x130;

const SOURCE: &str = "Build 1.74.0.0.104319: every tunable (130): its GUID -> STU_A0E5F3ED.m_3E5360A1 or \
STU_AA802F91.m_value, the float STU_082FA44E reads. Other 130 classes (groups, STU_23102F10) are left out.";

pub(crate) fn derive(install: &Install, _out: &Path) -> Result<Vec<Output>> {
    let mut values = Map::new();
    for (&index, asset) in &load(install, TUNABLE)? {
        let root = Node::root(asset)?;
        let field = match root.class_name() {
            Some("STU_A0E5F3ED") => "m_3E5360A1",
            Some("STU_AA802F91") => "m_value",
            _ => continue,
        };
        let Some(&Value::Float(value)) = root.value(field) else {
            return Err(asset_error(format!(
                "{index:012X}.130: a tunable without the float {field}"
            )));
        };
        values.insert(key(typed(index, TUNABLE)), float(value));
    }
    Ok(vec![Output::json(
        TUNABLES,
        &json!({"source": SOURCE, "tunables": values}),
    )])
}

/// An `f32` as DataTool prints it, read back as a double (as `game_modes`).
fn float(value: f64) -> serde_json::Value {
    let shortest: f64 = (value as f32).to_string().parse().expect("a float");
    json!(shortest)
}
