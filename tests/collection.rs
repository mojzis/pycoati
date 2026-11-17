//! Collection against `tests/fixtures/collection/`. The expected set is what
//! `pytest --collect-only -q` lists for that file (unittest orders methods
//! alphabetically, so the comparison is on sets).

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::collections::BTreeSet;
use std::path::PathBuf;

#[test]
fn collected_nodeids_match_pytest() {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.push("tests/fixtures/collection/test_collection.py");
    let inv = pycoati::run_static(&path).expect("run_static on collection fixture");
    let collected: BTreeSet<String> = inv
        .test_functions
        .iter()
        .map(|t| t.nodeid.split_once("::").map_or("", |(_, rest)| rest).to_string())
        .collect();
    let expected: BTreeSet<String> = [
        "CartChecks::testEmptyTotal",
        "CartChecks::test_total",
        "InheritedChecks::test_inherited_setup",
        "AsyncChecks::test_async_total",
        "TestOuter::test_outer",
        "TestOuter::TestInner::test_inner",
    ]
    .into_iter()
    .map(String::from)
    .collect();
    assert_eq!(collected, expected);
}
