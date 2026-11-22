//! Deterministic regression for the human table (design §§5–6): a listing
//! renders ONE row per line so a later entry is never hidden behind the
//! per-cell cap, a non-list payload still renders field/value rows, and the
//! per-cell cap (with its escape-safe `...` suffix) survives the expansion.

use serde_json::{json, Value};

use super::write_human_result;

/// Renders one payload through the human table and returns it as text.
fn rendered(command: &str, payload: &Value) -> String {
    let mut sink: Vec<u8> = Vec::new();
    write_human_result(&mut sink, command, payload);
    String::from_utf8(sink).expect("human table is UTF-8")
}

/// Three request rows must appear as `rows[0]`, `rows[1]`, `rows[2]` on
/// SEPARATE lines, and every `request_exec_id` must be visible (mutant: the
/// old single 200-char cell hides rows 2 and 3).
#[test]
fn request_rows_render_one_row_per_line() {
    let payload = json!({
        "kind": "requests",
        "scope": "own",
        "rows": [
            { "request_exec_id": "1", "session_handle": { "handle": "11" } },
            { "request_exec_id": "2", "session_handle": { "handle": "22" } },
            { "request_exec_id": "3", "session_handle": { "handle": "33" } },
        ],
        "rows_returned": "3",
        "rows_truncated": false,
    });
    let text = rendered("activity requests", &payload);
    let lines: Vec<&str> = text.split('\n').collect();

    let mut used: Vec<usize> = Vec::new();
    for index in 0..3 {
        let label = format!("rows[{index}]");
        let hits: Vec<usize> = lines
            .iter()
            .enumerate()
            .filter(|(_, line)| line.contains(label.as_str()))
            .map(|(line_index, _)| line_index)
            .collect();
        assert_eq!(hits.len(), 1, "`{label}` must own exactly one line: {text}");
        assert!(
            !used.contains(&hits[0]),
            "`{label}` must not share a line with another row: {text}"
        );
        used.push(hits[0]);
    }

    for id in 1..=3 {
        let needle = format!("\"request_exec_id\":\"{id}\"");
        assert!(
            text.contains(needle.as_str()),
            "request row {id} must be visible: {text}"
        );
    }
}

/// A NON-array payload still renders as `field  value` rows.
#[test]
fn non_array_payload_renders_field_value_rows() {
    let text = rendered("activity cancel", &json!({ "outcome": "cancelled" }));
    let hits = text
        .split('\n')
        .filter(|line| line.contains("outcome") && line.contains("cancelled"))
        .count();
    assert_eq!(hits, 1, "one `outcome  cancelled` row expected: {text}");
}

/// The expansion keeps the per-cell cap: a 400-char cell is cut with the
/// `...` suffix and stays on ONE line.
#[test]
fn long_row_cell_keeps_the_escape_safe_cap() {
    let payload = json!({
        "rows": [
            { "request_exec_id": "1", "detail": "x".repeat(400) },
        ],
        "rows_returned": "1",
        "rows_truncated": false,
    });
    let text = rendered("activity requests", &payload);
    assert_eq!(
        text.matches("rows[0]").count(),
        1,
        "one `rows[0]` line expected: {text}"
    );
    let cell_line = text
        .split('\n')
        .find(|line| line.contains("rows[0]"))
        .expect("the `rows[0]` line is present");
    assert!(
        cell_line.ends_with("..."),
        "a capped cell keeps its `...` suffix: {cell_line}"
    );
    let x_lines = text.split('\n').filter(|line| line.contains('x')).count();
    assert_eq!(x_lines, 1, "the capped cell stays on one line: {text}");
    assert!(
        cell_line.matches('x').count() < 400,
        "the 400-char value must be capped, not rendered whole: {cell_line}"
    );
}
