//! Anti-drift guard between the inventory schema and the embedded `analyze`
//! guide page.
//!
//! Walks every field name serde emits for the two payload shapes — including
//! nested structs — and asserts each one is documented as a field bullet in
//! `docs/guide/analyze.md`. The guide ships inside the binary via
//! `include_str!`, so a schema change that skips the docs is a compile-clean
//! but semantically stale build; this test is what catches it.
//!
//! The fixtures below are exhaustive struct literals on purpose. Adding a
//! field to any inventory struct breaks their compilation, which forces the
//! author here — and then the key walk forces them into the guide.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::collections::BTreeSet;
use std::path::PathBuf;

use serde_json::Value;

use pycoati::{
    FileRecord, Inventory, Project, SlowTest, SmellHit, Suite, SutCallEntry, SutCalls, TestRecord,
    ToolInfo, TopSuspicious, WorkspaceInventory,
};

/// An inventory with every nested collection non-empty, so the key walk
/// reaches every struct in the schema.
fn populated_inventory() -> Inventory {
    Inventory {
        schema_version: "2".to_string(),
        project: Project { path: PathBuf::from("."), name: "demo".to_string() },
        suite: Suite {
            test_count: Some(3),
            runtime_seconds: Some(1.5),
            line_coverage_pct: Some(80.0),
            slowest_tests: vec![SlowTest {
                nodeid: "tests/test_x.py::test_slow".to_string(),
                seconds: 2.0,
            }],
        },
        files: vec![FileRecord {
            path: PathBuf::from("tests/test_x.py"),
            test_function_count: 1,
            assertion_count: 1,
            mock_construction_count: 1,
            patch_decorator_count: 1,
            stubs_count: 1,
            fixture_count: 1,
            smell_hits: vec![smell_hit(None)],
        }],
        test_functions: vec![TestRecord {
            nodeid: "tests/test_x.py::test_a".to_string(),
            file: PathBuf::from("tests/test_x.py"),
            line: 4,
            assertion_count: 1,
            only_asserts_on_mock: true,
            patch_decorator_count: 1,
            stubs_count: 1,
            setup_to_assertion_ratio: 3.0,
            called_names: vec!["demo.thing".to_string()],
            smell_hits: vec![smell_hit(Some("tests/test_x.py::test_a"))],
            suspicion_score: 0.5,
        }],
        sut_calls: SutCalls {
            by_name: vec![SutCallEntry {
                name: "demo.thing".to_string(),
                test_function_count: 1,
                test_nodeids: vec!["tests/test_x.py::test_a".to_string()],
            }],
            top_called: vec!["demo.thing".to_string()],
        },
        top_suspicious: TopSuspicious {
            test_functions: vec!["tests/test_x.py::test_a".to_string()],
            files: vec!["tests/test_x.py".to_string()],
        },
        tool: tool_info(),
    }
}

fn populated_workspace() -> WorkspaceInventory {
    WorkspaceInventory {
        schema_version: "2".to_string(),
        workspace_root: PathBuf::from("."),
        members: vec![populated_inventory()],
        tool: tool_info(),
    }
}

fn tool_info() -> ToolInfo {
    ToolInfo {
        name: "pycoati".to_string(),
        version: "0.0.0".to_string(),
        ran_pytest: true,
        ran_coverage: true,
    }
}

fn smell_hit(test: Option<&str>) -> SmellHit {
    SmellHit {
        category: "mock_overuse".to_string(),
        test: test.map(str::to_string),
        line: 4,
        evidence: "3 mocks, 1 assertions".to_string(),
    }
}

/// Recursively collect every object key serde emitted.
fn collect_keys(value: &Value, out: &mut BTreeSet<String>) {
    match value {
        Value::Object(map) => {
            for (k, v) in map {
                out.insert(k.clone());
                collect_keys(v, out);
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_keys(item, out);
            }
        }
        _ => {}
    }
}

/// Field names the `analyze` page documents: the backtick-quoted token opening
/// a bullet in the field reference.
///
/// Scoped to the field reference itself, which runs from the top of the page to
/// the `## The suspicion score` heading. Everything after that opens bullets
/// with things that are not field names — smell category values in Part 1's
/// threshold section, expressions like ``- `assertion_count == 0`. …`` in Part
/// 2's taxonomy — and would pollute the set, making the reverse check below
/// meaningless. Both boundary headings are asserted below so a rename fails
/// loudly instead of silently widening the scope.
const FIELD_REFERENCE_END: &str = "## The suspicion score";

fn documented_names(page: &str) -> BTreeSet<String> {
    let field_reference = page.split(FIELD_REFERENCE_END).next().unwrap_or(page);
    field_reference
        .lines()
        .filter_map(|line| line.strip_prefix("- `"))
        .filter_map(|rest| rest.split('`').next())
        // Field-reference bullets are bare field names; anything carrying
        // punctuation or whitespace is prose that happens to lead with a
        // backtick.
        .filter(|token| {
            !token.is_empty() && token.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        })
        .map(str::to_string)
        .collect()
}

#[test]
fn every_serialized_field_is_documented_in_the_analyze_page() {
    let mut emitted = BTreeSet::new();
    collect_keys(
        &serde_json::to_value(populated_inventory()).expect("serialize inventory"),
        &mut emitted,
    );
    collect_keys(
        &serde_json::to_value(populated_workspace()).expect("serialize workspace"),
        &mut emitted,
    );

    let documented = documented_names(pycoati::guide::ANALYZE);
    let missing: Vec<&String> = emitted.difference(&documented).collect();

    assert!(
        missing.is_empty(),
        "docs/guide/analyze.md is out of date with the inventory schema.\n\
         These serialized field names have no `- `<name>` — …` bullet on the page: {missing:?}\n\
         Add one bullet per field, then re-run this test."
    );
}

#[test]
fn the_walk_actually_reaches_nested_structs() {
    // Guards the guard: if `populated_inventory` ever degrades to empty
    // collections, the drift test above would pass vacuously for nested fields.
    let mut emitted = BTreeSet::new();
    collect_keys(
        &serde_json::to_value(populated_inventory()).expect("serialize inventory"),
        &mut emitted,
    );
    for nested in ["seconds", "evidence", "test_nodeids", "suspicion_score", "fixture_count"] {
        assert!(emitted.contains(nested), "key walk never reached nested field `{nested}`");
    }
    assert_eq!(
        emitted.len(),
        38,
        "single-project schema field count changed. If you added or removed an \
         inventory field, update this count AND the field's bullet in \
         docs/guide/analyze.md."
    );
}

#[test]
fn workspace_shape_contributes_its_own_keys() {
    let mut emitted = BTreeSet::new();
    collect_keys(
        &serde_json::to_value(populated_workspace()).expect("serialize workspace"),
        &mut emitted,
    );
    assert!(emitted.contains("workspace_root"));
    assert!(emitted.contains("members"));
}

#[test]
fn the_analyze_page_documents_no_field_that_the_schema_dropped() {
    // The reverse of the guard above. Without this, removing a field from the
    // schema leaves its bullet on the page forever, and the page starts
    // describing an inventory the binary no longer emits.
    let mut emitted = BTreeSet::new();
    collect_keys(
        &serde_json::to_value(populated_inventory()).expect("serialize inventory"),
        &mut emitted,
    );
    collect_keys(
        &serde_json::to_value(populated_workspace()).expect("serialize workspace"),
        &mut emitted,
    );

    let documented = documented_names(pycoati::guide::ANALYZE);
    let stale: Vec<&String> = documented.difference(&emitted).collect();

    assert!(
        stale.is_empty(),
        "docs/guide/analyze.md documents fields the schema no longer emits: {stale:?}\n\
         Remove their bullets from the Part 1 field reference."
    );
}

#[test]
fn the_field_reference_scope_marker_still_exists() {
    // `documented_names` slices the page at this heading. If the heading is
    // renamed, the slice silently becomes the whole page and both drift guards
    // degrade — the forward one to a weaker check, the reverse one to noise.
    let page = pycoati::guide::ANALYZE;
    assert!(
        page.contains(FIELD_REFERENCE_END),
        "docs/guide/analyze.md no longer contains the `{FIELD_REFERENCE_END}` heading that \
         bounds the field reference; update FIELD_REFERENCE_END to match."
    );
    let scoped = documented_names(page);
    assert!(
        scoped.len() < page.matches("- `").count(),
        "the field-reference slice should be narrower than the whole page"
    );
}
