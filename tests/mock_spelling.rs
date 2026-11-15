//! Mock counting against `tests/fixtures/mock_spelling/`, a pytest-valid
//! file whose test pairs install the same doubles with different spellings.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::collections::BTreeMap;
use std::path::PathBuf;

use pycoati::TestRecord;

fn records() -> BTreeMap<String, TestRecord> {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.push("tests/fixtures/mock_spelling/test_mock_spelling.py");
    let inv = pycoati::run_static(&path).expect("run_static on mock_spelling fixture");
    inv.test_functions
        .into_iter()
        .map(|t| (t.nodeid.rsplit("::").next().unwrap_or("").to_string(), t))
        .collect()
}

#[test]
fn constructor_spellings_count_the_same() {
    let recs = records();
    let by_name = &recs["test_constructors_imported_by_name"];
    let by_module = &recs["test_constructors_through_module_alias"];
    assert_eq!(by_name.mock_construction_count, 3);
    assert_eq!(by_module.mock_construction_count, 3);
}

#[test]
fn decorator_and_context_manager_patches_score_the_same() {
    let recs = records();
    let decorated = &recs["test_patches_as_decorators"];
    let with_block = &recs["test_patches_as_context_managers"];
    assert_eq!(decorated.patch_decorator_count, 2);
    assert_eq!(decorated.patch_call_count, 0);
    assert_eq!(with_block.patch_decorator_count, 0);
    assert_eq!(with_block.patch_call_count, 2);
    // Same assertions, same patch footprint: only the setup term may differ,
    // and both bodies put their first assertion close to the `def`.
    let patch_term = |t: &TestRecord| t.patch_decorator_count + t.patch_call_count;
    assert_eq!(patch_term(decorated), patch_term(with_block));
    // Each patch is in the `mock_overuse` sum exactly once.
    let mocks =
        |t: &TestRecord| t.mock_construction_count + t.patch_decorator_count + t.stubs_count;
    assert_eq!(mocks(decorated), 2);
    assert_eq!(mocks(with_block), 2);
}

#[test]
fn an_attribute_named_mock_is_not_a_constructor() {
    // `MM()` counts; `factory.Mock()` on that double does not.
    assert_eq!(records()["test_unrelated_mock_attribute"].mock_construction_count, 1);
}
