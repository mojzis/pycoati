//! Tree-sitter based parsing of Python source files.
//!
//! Walks the syntax tree iteratively using a [`tree_sitter::TreeCursor`] and
//! emits one [`crate::TestRecord`] per top-level `def test_*` function. The
//! `assertion_count` reflects the number of `assert_statement` nodes inside
//! each test function's body.
//!
//! Phase 1 hardcodes `only_asserts_on_mock = false` — the mock-API detector
//! lands in Phase 2.

use std::path::Path;

use anyhow::{Context, Result};
use tree_sitter::{Node, Parser};

use crate::TestRecord;

/// Build a tree-sitter parser pre-configured with the Python grammar.
fn python_parser() -> Result<Parser> {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_python::LANGUAGE.into())
        .context("failed to load tree-sitter Python grammar")?;
    Ok(parser)
}

/// Parse one Python source file and return one record per `test_*` function.
///
/// The supplied `file_path` is used to build a pytest-style `nodeid` (i.e.
/// `<path>::<test_name>`); it is otherwise opaque to the parser. Non-test
/// functions and any nested test-named functions are ignored.
pub fn parse_python_file(source: &str, file_path: &Path) -> Result<Vec<TestRecord>> {
    let mut parser = python_parser()?;
    let tree = parser.parse(source, None).context("tree-sitter returned no tree")?;
    let root = tree.root_node();

    let mut records = Vec::new();
    collect_top_level_test_functions(root, source.as_bytes(), file_path, &mut records);
    Ok(records)
}

/// Iterate the immediate children of the module looking for top-level
/// `def test_*` definitions. Nested defs are deliberately not collected —
/// they aren't pytest tests.
fn collect_top_level_test_functions(
    module: Node<'_>,
    source: &[u8],
    file_path: &Path,
    out: &mut Vec<TestRecord>,
) {
    let mut cursor = module.walk();
    for child in module.children(&mut cursor) {
        if child.kind() == "function_definition" {
            if let Some(name) = function_name(child, source) {
                if name.starts_with("test_") {
                    out.push(build_record(child, name, file_path));
                }
            }
        }
    }
}

/// Extract the identifier text of a `function_definition` node, if present.
fn function_name<'a>(func: Node<'_>, source: &'a [u8]) -> Option<&'a str> {
    let name_node = func.child_by_field_name("name")?;
    name_node.utf8_text(source).ok()
}

/// Build a [`TestRecord`] from a `function_definition` node, counting
/// `assert_statement` descendants in its body.
fn build_record(func: Node<'_>, name: &str, file_path: &Path) -> TestRecord {
    // tree-sitter rows are zero-indexed `usize`. On every supported target
    // `usize` is at most 64 bits, so the cast is exact; `saturating_add`
    // converts to a 1-indexed line number without surfacing a bogus
    // sentinel value at the theoretical `u64::MAX` boundary.
    let row = func.start_position().row as u64;
    let line = row.saturating_add(1);
    let body = func.child_by_field_name("body");
    let assertion_count = body.map_or(0, |b| count_asserts(b));

    TestRecord {
        nodeid: format!("{}::{}", file_path.display(), name),
        file: file_path.to_path_buf(),
        line,
        assertion_count,
        only_asserts_on_mock: false,
        patch_decorator_count: 0,
        setup_to_assertion_ratio: 0.0,
        called_names: Vec::new(),
        smell_hits: Vec::new(),
        suspicion_score: 0.0,
    }
}

/// Count every `assert_statement` node anywhere in the subtree rooted at
/// `node`. Uses an explicit stack instead of recursion to keep the function
/// well within the cognitive-complexity threshold.
fn count_asserts(node: Node<'_>) -> u64 {
    let mut count: u64 = 0;
    let mut stack: Vec<Node<'_>> = vec![node];

    while let Some(current) = stack.pop() {
        if current.kind() == "assert_statement" {
            count += 1;
        }
        let mut cursor = current.walk();
        for child in current.children(&mut cursor) {
            stack.push(child);
        }
    }

    count
}
