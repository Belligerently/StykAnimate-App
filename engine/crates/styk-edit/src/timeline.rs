//! The editor's timeline (`editor.timeline`) on native documents: one row per keyframe.

use styk_doc::Value;
pub(crate) use styk_eval::eval::Row;

/// `timeline(report)` for a document without a layer sheet or property channels.
pub(crate) fn rows(report: &Value) -> Vec<Row> {
    let version = report.get("version").and_then(Value::as_f64).unwrap_or(5.0) as u32;
    let durations: Vec<u32> = report
        .get("keyframes")
        .and_then(Value::elements)
        .map(|frames| frames.iter().map(|k| k.get("duration_raw").and_then(Value::as_f64).map(|v| v as i64).unwrap_or(1).max(0) as u32).collect())
        .unwrap_or_default();
    styk_eval::eval::timeline_rows(version, durations.into_iter())
}

/// `editor.totalTicks`.
pub(crate) fn total_ticks(rows: &[Row]) -> u32 {
    rows.last().map(|r| r.start + r.count).unwrap_or(0)
}
