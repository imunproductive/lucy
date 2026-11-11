//! `data/game_objectives_174.json`: what the objectives' native components
//! hold that the server reads. For now the capture area's progress marks:
//! entity `0x37.003` is the only entity definition with an
//! `STUCaptureAreaComponent`, and its `m_segments` list the marks (each an
//! `STUCaptureAreaSegment`'s float `m_4A9567C6`, 1/3 and 2/3 in 1.74) that a
//! capture never falls below.

use std::path::Path;

use orpheus_stu::{Asset, Value};
use serde_json::json;

use crate::{Install, Output, Result, ai_spawners::component, asset_error, guid_hex};

pub(crate) const OBJECTIVES: &str = "data/game_objectives_174.json";

/// The capture area entity definition (`0x37.003`).
const CAPTURE_AREA: u64 = 0x0400_0000_0000_0037;

const SOURCE: &str = "Build 1.74.0.0.104319: entity 0x37.003's STUCaptureAreaComponent.m_segments, each \
STUCaptureAreaSegment's m_4A9567C6 (the capture progress marks).";

pub(crate) fn derive(install: &Install, _out: &Path) -> Result<Vec<Output>> {
    let asset = install.stu(CAPTURE_AREA)?;
    let segments = capture_segments(&asset)
        .map_err(|reason| asset_error(format!("{}: {reason}", guid_hex(CAPTURE_AREA))))?;
    Ok(vec![Output::json(
        OBJECTIVES,
        &json!({
            "source": SOURCE,
            "capture_area": {"entity": guid_hex(CAPTURE_AREA), "segments": segments},
        }),
    )])
}

/// The marks of the entity's capture area component, in order.
fn capture_segments(asset: &Asset) -> Result<Vec<serde_json::Value>, String> {
    let component =
        component(asset, "STUCaptureAreaComponent").ok_or("no STUCaptureAreaComponent")?;
    let Some(Value::Array(segments)) = component.get("m_segments") else {
        return Err("the capture area has no m_segments".into());
    };
    if segments.is_empty() {
        return Err("the capture area has no segments".into());
    }
    segments
        .iter()
        .map(|segment| {
            let segment = asset
                .resolve(segment)
                .filter(|segment| segment.class_name() == Some("STUCaptureAreaSegment"))
                .ok_or("a segment is not an STUCaptureAreaSegment")?;
            match segment.get("m_4A9567C6") {
                Some(&Value::Float(mark)) => Ok(float(mark)),
                _ => Err("a segment has no float m_4A9567C6".into()),
            }
        })
        .collect()
}

/// An `f32` as DataTool prints it, read back as a double (as `game_modes`).
fn float(value: f64) -> serde_json::Value {
    let shortest: f64 = (value as f32).to_string().parse().expect("a float");
    json!(shortest)
}
