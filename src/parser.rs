//! Tree-sitter based parsing of Python source files.
//!
//! Walks the syntax tree iteratively using a [`tree_sitter::TreeCursor`] and
//! emits one [`crate::TestRecord`] per top-level `def test_*` function. The
//! `assertion_count` reflects the number of `assert_statement` nodes inside
//! each test function's body.
//!
//! Phase 2 adds the [`only_asserts_on_mock`](crate::TestRecord) predicate —
//! `true` when every assert in the test targets a Mock-API attribute.

use std::path::Path;

use anyhow::{Context, Result};
use tree_sitter::{Node, Parser};

use crate::mock_api::is_mock_api_attribute;
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
                    out.push(build_record(child, name, source, file_path));
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
fn build_record(func: Node<'_>, name: &str, source: &[u8], file_path: &Path) -> TestRecord {
    // tree-sitter rows are zero-indexed `usize`. On every supported target
    // `usize` is at most 64 bits, so the cast is exact; `saturating_add`
    // converts to a 1-indexed line number without surfacing a bogus
    // sentinel value at the theoretical `u64::MAX` boundary.
    let row = func.start_position().row as u64;
    let line = row.saturating_add(1);

    let mut asserts: Vec<Node<'_>> = Vec::new();
    if let Some(body) = func.child_by_field_name("body") {
        collect_asserts(body, &mut asserts);
    }

    let assertion_count = asserts.len() as u64;
    let only_asserts_on_mock =
        assertion_count > 0 && asserts.iter().all(|a| assert_targets_mock_api(*a, source));

    TestRecord {
        nodeid: format!("{}::{}", file_path.display(), name),
        file: file_path.to_path_buf(),
        line,
        assertion_count,
        only_asserts_on_mock,
        patch_decorator_count: 0,
        setup_to_assertion_ratio: 0.0,
        called_names: Vec::new(),
        smell_hits: Vec::new(),
        suspicion_score: 0.0,
    }
}

/// Collect every `assert_statement` node anywhere in the subtree rooted at
/// `node`. Walks the subtree iteratively with a single
/// [`tree_sitter::TreeCursor`] — the idiomatic tree-sitter descent pattern,
/// avoiding the per-node cursor allocations that a children-iterator loop
/// would incur.
fn collect_asserts<'a>(node: Node<'a>, out: &mut Vec<Node<'a>>) {
    let mut cursor = node.walk();
    // Bound the traversal to the subtree rooted at `node` — we must not
    // ascend past the starting node when backing out of dead-ends.
    let start_id = node.id();
    loop {
        let current = cursor.node();
        if current.kind() == "assert_statement" {
            out.push(current);
        }

        if cursor.goto_first_child() {
            continue;
        }
        if current.id() != start_id && cursor.goto_next_sibling() {
            continue;
        }
        // Back out until we find a parent with another sibling, or until we
        // would ascend past the starting node.
        loop {
            if !cursor.goto_parent() {
                return;
            }
            if cursor.node().id() == start_id {
                return;
            }
            if cursor.goto_next_sibling() {
                break;
            }
        }
    }
}

/// Decide whether an `assert_statement` targets a Mock-API attribute.
///
/// `assert_statement` in tree-sitter-python has `assert` followed by the
/// asserted expression(s). We look at the first non-keyword child — the
/// expression being asserted — and decide whether its **outermost** value
/// shape is `<receiver>.<mock_api_attribute>` (possibly chained, possibly
/// called).
///
/// Conservative classification: anything we cannot positively identify as a
/// mock-API attribute access (bare names, comparisons, `isinstance(...)`,
/// arithmetic, etc.) is treated as non-mock. The optional `msg` argument
/// after a comma is ignored — only the truth-y expression matters.
fn assert_targets_mock_api(assert_stmt: Node<'_>, source: &[u8]) -> bool {
    let Some(expr) = first_asserted_expression(assert_stmt) else {
        return false;
    };
    last_attribute_name(expr, source).is_some_and(is_mock_api_attribute)
}

/// Return the first asserted expression of an `assert_statement`. The first
/// child is the `assert` keyword; the next named child is the expression.
fn first_asserted_expression(assert_stmt: Node<'_>) -> Option<Node<'_>> {
    // `named_child` is index-based and avoids the lifetime gymnastics of
    // borrowing a cursor that would have to outlive the returned Node.
    assert_stmt.named_child(0)
}

/// Determine the "last attribute" of an expression for Mock-API matching.
///
/// Recognised shapes (and only these):
///
/// * `(attribute object: <X> attribute: (identifier) @last)` → returns `@last`
/// * `(call function: (attribute ... attribute: (identifier) @last) ...)` → returns `@last`
/// * `(parenthesized_expression <inner>)` → recurses into `<inner>`. Parens
///   do not change semantics, so `assert (mock.called)` matches just like
///   `assert mock.called`.
///
/// Everything else — bare identifiers, comparisons, calls without an
/// attribute head, unary/boolean operators, comprehensions — returns
/// `None` (i.e. "not a mock-API assert"). Conservative is correct: when
/// uncertain, callers treat the result as non-mock.
fn last_attribute_name<'a>(expr: Node<'_>, source: &'a [u8]) -> Option<&'a str> {
    let attribute_node = match expr.kind() {
        "attribute" => expr,
        "call" => {
            let func = expr.child_by_field_name("function")?;
            if func.kind() == "attribute" {
                func
            } else {
                return None;
            }
        }
        "parenthesized_expression" => {
            let inner = expr.named_child(0)?;
            return last_attribute_name(inner, source);
        }
        _ => return None,
    };
    let last = attribute_node.child_by_field_name("attribute")?;
    last.utf8_text(source).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn parse(src: &str) -> Vec<TestRecord> {
        parse_python_file(src, &PathBuf::from("synthetic.py")).expect("parse")
    }

    #[test]
    fn only_mock_asserts_predicate_is_true_for_pure_mock_test() {
        let src = "\
def test_a():
    assert mock.assert_called_once_with(1)
    assert repo.save.assert_called()
";
        let recs = parse(src);
        assert_eq!(recs.len(), 1);
        assert_eq!(recs[0].assertion_count, 2);
        assert!(recs[0].only_asserts_on_mock);
    }

    #[test]
    fn mixed_asserts_yield_false() {
        let src = "\
def test_b():
    assert x == 1
    assert mock.assert_called()
";
        let recs = parse(src);
        assert_eq!(recs.len(), 1);
        assert_eq!(recs[0].assertion_count, 2);
        assert!(!recs[0].only_asserts_on_mock);
    }

    #[test]
    fn isinstance_assert_is_non_mock() {
        let src = "\
def test_c():
    assert isinstance(x, Foo)
";
        let recs = parse(src);
        assert!(!recs[0].only_asserts_on_mock);
    }

    #[test]
    fn zero_asserts_yield_false() {
        let src = "\
def test_d():
    pass
";
        let recs = parse(src);
        assert_eq!(recs[0].assertion_count, 0);
        assert!(!recs[0].only_asserts_on_mock);
    }

    #[test]
    fn chained_attribute_uses_last_segment() {
        // The final attribute in the chain decides — even though `save` is
        // not a mock-API name, the outermost `.assert_called_once_with` is.
        let src = "\
def test_e():
    assert a.b.c.save.assert_called_once_with(1)
";
        let recs = parse(src);
        assert!(recs[0].only_asserts_on_mock);
    }

    #[test]
    fn bare_attribute_without_call_is_recognised() {
        // `.called` is a property, not a method — no parentheses.
        let src = "\
def test_f():
    assert mock.called
";
        let recs = parse(src);
        assert!(recs[0].only_asserts_on_mock);
    }

    #[test]
    fn non_attribute_call_is_non_mock() {
        let src = "\
def test_g():
    assert add(2, 3) == 5
";
        let recs = parse(src);
        assert!(!recs[0].only_asserts_on_mock);
    }

    #[test]
    fn parenthesized_mock_attribute_is_recognised() {
        // Parens do not change semantics; `(mock.called)` is still a
        // mock-API attribute access.
        let src = "\
def test_h():
    assert (mock.called)
    assert (repo.save.assert_called_once_with(1))
";
        let recs = parse(src);
        assert_eq!(recs[0].assertion_count, 2);
        assert!(recs[0].only_asserts_on_mock);
    }

    #[test]
    fn unary_negation_is_conservative_non_mock() {
        // `not mock.called` is NOT an attribute access at the outer level —
        // the spec says "Conservative is correct: when uncertain, mark as
        // non-mock." Run-3 may revisit if a clear policy emerges.
        let src = "\
def test_i():
    assert not mock.called
";
        let recs = parse(src);
        assert!(!recs[0].only_asserts_on_mock);
    }

    #[test]
    fn nested_asserts_in_if_block_are_collected() {
        // Exercises the iterative cursor-descent: asserts nested inside
        // control flow must still be discovered.
        let src = "\
def test_j():
    if True:
        assert mock.assert_called()
        if x:
            assert repo.save.assert_called()
";
        let recs = parse(src);
        assert_eq!(recs[0].assertion_count, 2);
        assert!(recs[0].only_asserts_on_mock);
    }
}
