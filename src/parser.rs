//! Tree-sitter based parsing of Python source files.
//!
//! Walks the syntax tree iteratively using a [`tree_sitter::TreeCursor`] and
//! emits one [`crate::TestRecord`] per pytest-collectable test function:
//!
//! * Top-level `def test_*` and `async def test_*`, with or without
//!   decorators (`@pytest.mark.parametrize`, `@pytest.mark.anyio`, etc.).
//!   Decorated defs live under a `decorated_definition` node which the
//!   walker unwraps.
//! * Methods named `test_*` inside classes whose name starts with `Test`,
//!   matching pytest's default collection rule. Nodeid is
//!   `<file>::<ClassName>::<method>` to align with pytest's output.
//!
//! Functions nested inside other functions are deliberately not collected —
//! pytest does not collect them either.
//!
//! `assertion_count` reflects the number of `assert_statement` nodes inside
//! each test function's body. The [`only_asserts_on_mock`](crate::TestRecord)
//! predicate is `true` when every assert in the test targets a Mock-API
//! attribute.
//!
//! Per-test AST counts (Run 3 phase 1):
//!
//! * `patch_decorator_count` — `@patch`, `@mock.patch`, `@patch.object`, or
//!   any `@<something>.patch` decorator on the test function (or on its
//!   wrapping `decorated_definition`).
//! * `setup_to_assertion_ratio` — `(first_assert_line - def_line) /
//!   max(assertion_count, 1)` as `f64`, using tree-sitter `start_position()`
//!   row deltas. When the body contains no `assert_statement`, the
//!   numerator becomes `last_body_line - def_line` and the denominator is
//!   `1`, so zero-assert setup-heavy tests naturally rank high on the
//!   suspicion-score axis.
//! * `called_names` — raw, sorted, deduped dot-joined attribute chain at
//!   the `function` child of every `call_expression` in the test body,
//!   minus calls whose head chain starts with `self.`. Phase 2 resolves
//!   these against the file's [`crate::sut_calls::ImportMap`] and replaces
//!   the field with the project-internal subset.
//!
//! Per-file AST counts (returned alongside the per-test records via
//! [`ParsedFile`]):
//!
//! * `mock_construction_count` — count of `call_expression` nodes inside
//!   any test body whose head name matches
//!   [`crate::mock_api::MOCK_CONSTRUCTORS`].
//! * `patch_decorator_count` — sum of the per-test counts.
//! * `fixture_count` — number of `@pytest.fixture` / `@fixture` decorators
//!   anywhere in the file (counted on any `decorated_definition`, not just
//!   tests).

use std::collections::BTreeSet;
use std::path::Path;

use anyhow::{Context, Result};
use tree_sitter::{Node, Parser};

use crate::mock_api::{is_mock_api_attribute, is_mock_constructor};
use crate::sut_calls::ImportMap;
use crate::TestRecord;

/// What [`parse_python_file`] returns: the per-test records plus per-file
/// aggregates and the import map needed for Phase 2 sut-call resolution.
#[derive(Debug, Clone, Default)]
pub struct ParsedFile {
    pub test_functions: Vec<TestRecord>,
    /// Sum across every test body of `call_expression` heads matching
    /// [`crate::mock_api::MOCK_CONSTRUCTORS`].
    pub mock_construction_count: u64,
    /// Sum of `patch_decorator_count` across every test record.
    pub patch_decorator_count: u64,
    /// Count of `@pytest.fixture` / `@fixture` decorators anywhere in the file.
    pub fixture_count: u64,
    /// Per-file import map for Phase 2 sut-call resolution.
    pub import_map: ImportMap,
}

/// Build a tree-sitter parser pre-configured with the Python grammar.
fn python_parser() -> Result<Parser> {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_python::LANGUAGE.into())
        .context("failed to load tree-sitter Python grammar")?;
    Ok(parser)
}

/// Parse one Python source file and return the per-test records plus
/// per-file aggregates and the import map.
///
/// The supplied `file_path` is used to build pytest-style nodeids
/// (`<path>::<test_name>` for module-level tests, `<path>::<Class>::<test>`
/// for class-nested tests); it is otherwise opaque to the parser.
pub fn parse_python_file(source: &str, file_path: &Path) -> Result<ParsedFile> {
    let mut parser = python_parser()?;
    let tree = parser.parse(source, None).context("tree-sitter returned no tree")?;
    let root = tree.root_node();
    let bytes = source.as_bytes();

    let mut parsed = ParsedFile::default();
    collect_module_tests(root, bytes, file_path, &mut parsed);
    parsed.fixture_count = count_fixture_decorators(root, bytes);
    parsed.import_map = build_import_map(root, bytes);
    Ok(parsed)
}

/// Iterate the immediate children of the module and dispatch on node kind:
/// bare `function_definition`s (top-level tests), `decorated_definition`s
/// (wrapping a function or a class), and `class_definition`s (pytest test
/// containers when named `Test*`).
fn collect_module_tests(module: Node<'_>, source: &[u8], file_path: &Path, out: &mut ParsedFile) {
    let mut cursor = module.walk();
    for child in module.children(&mut cursor) {
        match child.kind() {
            "function_definition" => {
                try_collect_test_function(child, None, source, file_path, None, out);
            }
            "decorated_definition" => {
                if let Some(inner) = child.child_by_field_name("definition") {
                    match inner.kind() {
                        "function_definition" => {
                            try_collect_test_function(
                                inner,
                                Some(child),
                                source,
                                file_path,
                                None,
                                out,
                            );
                        }
                        "class_definition" => {
                            collect_class_tests(inner, source, file_path, out);
                        }
                        _ => {}
                    }
                }
            }
            "class_definition" => {
                collect_class_tests(child, source, file_path, out);
            }
            _ => {}
        }
    }
}

/// Walk a `class_definition`'s body collecting `test_*` methods. Skips
/// classes whose name does not start with `Test` (pytest's default rule).
/// Nested classes inside the body are intentionally not recursed into —
/// pytest does not collect them, and joining their names would diverge
/// from pytest's nodeid shape.
fn collect_class_tests(
    class_node: Node<'_>,
    source: &[u8],
    file_path: &Path,
    out: &mut ParsedFile,
) {
    let Some(class_name) = node_name(class_node, source) else {
        return;
    };
    if !class_name.starts_with("Test") {
        return;
    }
    let Some(body) = class_node.child_by_field_name("body") else {
        return;
    };
    let mut cursor = body.walk();
    for child in body.children(&mut cursor) {
        match child.kind() {
            "function_definition" => {
                try_collect_test_function(child, None, source, file_path, Some(class_name), out);
            }
            "decorated_definition" => {
                if let Some(inner) = child.child_by_field_name("definition") {
                    if inner.kind() == "function_definition" {
                        try_collect_test_function(
                            inner,
                            Some(child),
                            source,
                            file_path,
                            Some(class_name),
                            out,
                        );
                    }
                }
            }
            _ => {}
        }
    }
}

/// Push one record if the `function_definition` is a `test_*`. When the
/// function is wrapped by a `decorated_definition`, `decorated` carries
/// that wrapper so decorator counts can be extracted.
fn try_collect_test_function(
    func: Node<'_>,
    decorated: Option<Node<'_>>,
    source: &[u8],
    file_path: &Path,
    class_prefix: Option<&str>,
    out: &mut ParsedFile,
) {
    if let Some(name) = node_name(func, source) {
        if name.starts_with("test_") {
            let (record, mock_constructions) =
                build_record(func, decorated, name, source, file_path, class_prefix);
            out.mock_construction_count =
                out.mock_construction_count.saturating_add(mock_constructions);
            out.patch_decorator_count =
                out.patch_decorator_count.saturating_add(record.patch_decorator_count);
            out.test_functions.push(record);
        }
    }
}

/// Extract the identifier text from a node's `name` field — works for
/// both `function_definition` and `class_definition`.
fn node_name<'a>(node: Node<'_>, source: &'a [u8]) -> Option<&'a str> {
    let name_node = node.child_by_field_name("name")?;
    name_node.utf8_text(source).ok()
}

/// Build a [`TestRecord`] from a `function_definition` node plus the
/// surrounding `decorated_definition` (if any).
///
/// Returns the record plus the count of mock-constructor call sites in
/// the test body, which is aggregated at file level (no per-test field
/// exists for it on `TestRecord`).
fn build_record(
    func: Node<'_>,
    decorated: Option<Node<'_>>,
    name: &str,
    source: &[u8],
    file_path: &Path,
    class_prefix: Option<&str>,
) -> (TestRecord, u64) {
    // tree-sitter rows are zero-indexed `usize`. On every supported target
    // `usize` is at most 64 bits, so the cast is exact; `saturating_add`
    // converts to a 1-indexed line number without surfacing a bogus
    // sentinel value at the theoretical `u64::MAX` boundary.
    let row = func.start_position().row as u64;
    let line = row.saturating_add(1);

    let body = func.child_by_field_name("body");

    let mut asserts: Vec<Node<'_>> = Vec::new();
    let mut calls: Vec<Node<'_>> = Vec::new();
    if let Some(body_node) = body {
        collect_asserts(body_node, &mut asserts);
        collect_calls(body_node, &mut calls);
    }

    let assertion_count = asserts.len() as u64;
    let only_asserts_on_mock =
        assertion_count > 0 && asserts.iter().all(|a| assert_targets_mock_api(*a, source));

    let mock_construction_count =
        calls.iter().filter(|c| call_head_is_mock_constructor(**c, source)).count() as u64;

    let called_names = called_names_for_test(&calls, source);

    let patch_decorator_count = decorated.map_or(0, |d| count_patch_decorators(d, source));

    let setup_to_assertion_ratio = compute_setup_to_assertion_ratio(func, body, &asserts);

    let nodeid = match class_prefix {
        Some(cls) => format!("{}::{}::{}", file_path.display(), cls, name),
        None => format!("{}::{}", file_path.display(), name),
    };

    let record = TestRecord {
        nodeid,
        file: file_path.to_path_buf(),
        line,
        assertion_count,
        only_asserts_on_mock,
        patch_decorator_count,
        setup_to_assertion_ratio,
        called_names,
        smell_hits: Vec::new(),
        suspicion_score: 0.0,
    };
    (record, mock_construction_count)
}

/// Compute `setup_to_assertion_ratio` per the locked Run 3 definition.
///
/// With at least one `assert_statement` in the body:
///   `(first_assert_row - def_row) / max(assertion_count, 1)`
///
/// With zero asserts:
///   `(last_body_row - def_row) / 1`
///
/// Row deltas are taken from tree-sitter `start_position().row` (0-indexed);
/// only the delta matters, so the index base is irrelevant.
fn compute_setup_to_assertion_ratio(
    func: Node<'_>,
    body: Option<Node<'_>>,
    asserts: &[Node<'_>],
) -> f64 {
    let def_row = func.start_position().row;
    if let Some(first) = asserts.first() {
        let first_row = first.start_position().row;
        let delta = first_row.saturating_sub(def_row) as f64;
        let denom = asserts.len().max(1) as f64;
        return delta / denom;
    }
    // Zero-assert fallback: body height, divided by 1. "Last body row" is
    // the start row of the body's last named statement — this is the row
    // index a human would point at as "the last line of the function body".
    let Some(body_node) = body else {
        return 0.0;
    };
    let last_body_row = last_named_child_row(body_node).unwrap_or(def_row);
    last_body_row.saturating_sub(def_row) as f64
}

/// Return the `start_position().row` of the last named child of `node`,
/// or `None` if the node has no named children.
fn last_named_child_row(node: Node<'_>) -> Option<usize> {
    // Walk via cursor and track the last named child we see.
    let mut cursor = node.walk();
    let mut last: Option<Node<'_>> = None;
    for child in node.named_children(&mut cursor) {
        last = Some(child);
    }
    last.map(|n| n.start_position().row)
}

/// True iff the `call_expression`'s `function` child has a head name
/// (bare identifier or first segment of an attribute chain) that matches
/// one of the known Mock-API constructors.
fn call_head_is_mock_constructor(call: Node<'_>, source: &[u8]) -> bool {
    let Some(func) = call.child_by_field_name("function") else {
        return false;
    };
    match func.kind() {
        "identifier" => func.utf8_text(source).is_ok_and(is_mock_constructor),
        "attribute" => {
            // For `mock.patch(...)` the head segment is `mock`; for our
            // strict semantics we treat only an exact head match against
            // the constructor set as a mock construction. This keeps
            // `repo.save(...)` out of the count and avoids double-counting
            // `mock.patch(...)` calls (the `@patch` decorator pathway
            // captures decorator usage separately).
            head_identifier(func, source).is_some_and(|h| is_mock_constructor(h) && h != "patch")
        }
        _ => false,
    }
}

/// Walk down an `attribute` node until the leftmost identifier is reached,
/// returning that identifier's text.
fn head_identifier<'a>(mut node: Node<'_>, source: &'a [u8]) -> Option<&'a str> {
    loop {
        match node.kind() {
            "identifier" => return node.utf8_text(source).ok(),
            "attribute" => {
                let object = node.child_by_field_name("object")?;
                node = object;
            }
            _ => return None,
        }
    }
}

/// Compute the dot-joined attribute chain at a `call_expression`'s
/// `function` child. Supports bare identifiers (`Repository`) and
/// attribute chains of arbitrary depth (`uuid.uuid4`, `a.b.c.save`).
/// Returns `None` for callable forms we don't model (calls on subscripts,
/// lambda invocations, chained `().method()`-style suffixes, etc.).
fn call_head_chain(call: Node<'_>, source: &[u8]) -> Option<String> {
    let func = call.child_by_field_name("function")?;
    attribute_chain_text(func, source)
}

/// Recursively render an expression as `"a.b.c"`. Returns `None` if the
/// expression contains anything other than identifiers and `attribute`
/// nodes — calls, subscripts, parentheses, etc. break the chain.
fn attribute_chain_text(node: Node<'_>, source: &[u8]) -> Option<String> {
    match node.kind() {
        "identifier" => node.utf8_text(source).ok().map(String::from),
        "attribute" => {
            let object = node.child_by_field_name("object")?;
            let attr = node.child_by_field_name("attribute")?;
            let lhs = attribute_chain_text(object, source)?;
            let rhs = attr.utf8_text(source).ok()?;
            Some(format!("{lhs}.{rhs}"))
        }
        _ => None,
    }
}

/// Collect, dedupe, sort, and `self.*`-filter the called-name chains for
/// one test body.
fn called_names_for_test(calls: &[Node<'_>], source: &[u8]) -> Vec<String> {
    let mut set = BTreeSet::new();
    for call in calls {
        if let Some(chain) = call_head_chain(*call, source) {
            if chain == "self" || chain.starts_with("self.") {
                continue;
            }
            set.insert(chain);
        }
    }
    set.into_iter().collect()
}

/// Count `@patch`-shaped decorators on a `decorated_definition` node.
///
/// Recognised decorator shapes:
/// - `@patch` (bare identifier)
/// - `@<something>.patch` (any attribute chain ending in `.patch`, e.g.
///   `mock.patch`, `unittest.mock.patch`)
/// - `@patch.object`, `@patch.dict`, … (any attribute chain whose head
///   identifier is `patch`)
/// - the same forms when used as call expressions: `@patch('a')`,
///   `@mock.patch('a')`, `@patch.object(SomeClass, 'method')`.
fn count_patch_decorators(decorated: Node<'_>, source: &[u8]) -> u64 {
    let mut count: u64 = 0;
    let mut cursor = decorated.walk();
    for child in decorated.children(&mut cursor) {
        if child.kind() == "decorator" && decorator_is_patch(child, source) {
            count = count.saturating_add(1);
        }
    }
    count
}

/// Walk a `decorator` node and decide whether its decorator-expression is
/// a `@patch`-shaped form. See [`decorator_target`] for the shared
/// unwrap-bare-or-call-target logic.
fn decorator_is_patch(decorator: Node<'_>, source: &[u8]) -> bool {
    let Some(target) = decorator_target(decorator) else {
        return false;
    };
    match target.kind() {
        "identifier" => target.utf8_text(source).is_ok_and(|t| t == "patch"),
        "attribute" => {
            // Match when the dotted chain ends in `patch` (mock.patch,
            // unittest.mock.patch, …) OR starts with `patch`
            // (patch.object, patch.dict, …). Either pattern qualifies as
            // a "patch-shaped" decorator for counting purposes.
            let Some(chain) = attribute_chain_text(target, source) else {
                return false;
            };
            // Match when either end of the dotted chain is `patch`:
            // - first segment `patch` covers `patch.object`, `patch.dict`,
            //   and the bare `patch` after the early `==` check below.
            // - last segment `patch` covers `mock.patch`,
            //   `unittest.mock.patch`, etc.
            let mut segments = chain.split('.');
            let first = segments.next();
            let last = segments.next_back().or(first);
            first == Some("patch") || last == Some("patch")
        }
        _ => false,
    }
}

/// Count `@pytest.fixture` / `@fixture` decorators on every
/// `decorated_definition` in the file. The fixture decoration can apply
/// to module-level functions, methods inside any class (test class or
/// not), or even nested defs — pytest itself only collects module-level
/// and class-scoped ones, but the spec asks for a coarse file-level
/// count, so we count any `decorated_definition` descendant.
fn count_fixture_decorators(root: Node<'_>, source: &[u8]) -> u64 {
    let mut count: u64 = 0;
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        if node.kind() == "decorated_definition" {
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                if child.kind() == "decorator" && decorator_is_fixture(child, source) {
                    count = count.saturating_add(1);
                }
            }
        }
        // Recurse into every named child — we want to find decorated defs
        // nested arbitrarily deep (inside classes, conditionals, etc.).
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            // Skip the decorators themselves to avoid re-entering them.
            if child.kind() != "decorator" {
                stack.push(child);
            }
        }
    }
    count
}

/// True iff a `decorator` node is `@pytest.fixture` (with or without
/// parens) or `@fixture` (with or without parens). Anything else is
/// not a pytest fixture decoration for counting purposes.
fn decorator_is_fixture(decorator: Node<'_>, source: &[u8]) -> bool {
    let Some(target) = decorator_target(decorator) else {
        return false;
    };
    match target.kind() {
        "identifier" => target.utf8_text(source).is_ok_and(|t| t == "fixture"),
        "attribute" => attribute_chain_text(target, source).is_some_and(|c| c == "pytest.fixture"),
        _ => false,
    }
}

/// Unwrap a `decorator` node to the identifier-or-attribute that names
/// the decorator. For `@foo` and `@a.b` returns the `foo` / `a.b` node;
/// for `@foo(...)` and `@a.b(...)` returns the `function` child of the
/// inner `call`. Returns `None` for shapes we don't classify (subscripts,
/// lambdas, etc.).
fn decorator_target(decorator: Node<'_>) -> Option<Node<'_>> {
    let expr = decorator.named_child(0)?;
    if expr.kind() == "call" {
        expr.child_by_field_name("function")
    } else {
        Some(expr)
    }
}

/// Build the per-file [`ImportMap`] by walking the module's top-level
/// `import_statement` and `import_from_statement` nodes. Phase 1 returns
/// the map for downstream consumption by Phase 2 (`sut_calls` resolution);
/// no resolution logic lives in this module.
fn build_import_map(root: Node<'_>, source: &[u8]) -> ImportMap {
    let mut map = ImportMap::default();
    let mut cursor = root.walk();
    for child in root.children(&mut cursor) {
        match child.kind() {
            "import_statement" => record_import_statement(child, source, &mut map),
            "import_from_statement" => record_import_from(child, source, &mut map),
            _ => {}
        }
    }
    map
}

/// Record one `import_statement` (e.g. `import foo`, `import foo.bar`,
/// `import foo as f`, `import foo.bar as fb`, `import a, b`).
fn record_import_statement(stmt: Node<'_>, source: &[u8], map: &mut ImportMap) {
    let mut cursor = stmt.walk();
    for child in stmt.named_children(&mut cursor) {
        match child.kind() {
            "dotted_name" => {
                if let Some(full) = attribute_or_dotted_text(child, source) {
                    let local = full.split('.').next().unwrap_or(&full).to_string();
                    map.aliases.insert(local, full);
                }
            }
            "aliased_import" => {
                // (aliased_import name: (dotted_name) alias: (identifier))
                if let (Some(name_node), Some(alias_node)) =
                    (child.child_by_field_name("name"), child.child_by_field_name("alias"))
                {
                    if let (Some(full), Ok(alias)) =
                        (attribute_or_dotted_text(name_node, source), alias_node.utf8_text(source))
                    {
                        map.aliases.insert(alias.to_string(), full);
                    }
                }
            }
            _ => {}
        }
    }
}

/// Record one `import_from_statement` (e.g. `from foo import bar`,
/// `from foo import bar as b`, `from foo import *`, `from foo import a, b`).
fn record_import_from(stmt: Node<'_>, source: &[u8], map: &mut ImportMap) {
    // The `module_name` field carries the source module's dotted name.
    let Some(module_node) = stmt.child_by_field_name("module_name") else {
        return;
    };
    let Some(source_module) = attribute_or_dotted_text(module_node, source) else {
        return;
    };

    // Single pass over named children:
    // * `wildcard_import` short-circuits to recording only the source module
    //   (no alias entries; star imports bind an unknown set of names).
    // * `dotted_name` and `aliased_import` produce alias-map entries.
    // tree-sitter-python represents `from m import *` as a `wildcard_import`
    // sibling, and `from m import a, b` as repeated named children.
    let mut buffered: Vec<(String, String)> = Vec::new();
    let mut cursor = stmt.walk();
    for child in stmt.named_children(&mut cursor) {
        if child.id() == module_node.id() {
            continue;
        }
        match child.kind() {
            "wildcard_import" => {
                map.star_sources.insert(source_module);
                return;
            }
            "dotted_name" => {
                if let Some(local) = attribute_or_dotted_text(child, source) {
                    let local_head = local.split('.').next().unwrap_or(&local).to_string();
                    buffered.push((local_head, source_module.clone()));
                }
            }
            "aliased_import" => {
                if let Some(alias_node) = child.child_by_field_name("alias") {
                    if let Ok(alias) = alias_node.utf8_text(source) {
                        buffered.push((alias.to_string(), source_module.clone()));
                    }
                }
            }
            _ => {}
        }
    }
    for (local, src) in buffered {
        map.aliases.insert(local, src);
    }
}

/// Render a `dotted_name` or `attribute` node as its dotted text.
fn attribute_or_dotted_text(node: Node<'_>, source: &[u8]) -> Option<String> {
    match node.kind() {
        "dotted_name" => {
            // `dotted_name` children alternate `identifier` and `.` tokens
            // — collect just the identifiers.
            let mut parts: Vec<&str> = Vec::new();
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                if child.kind() == "identifier" {
                    if let Ok(t) = child.utf8_text(source) {
                        parts.push(t);
                    }
                }
            }
            if parts.is_empty() {
                None
            } else {
                Some(parts.join("."))
            }
        }
        "identifier" => node.utf8_text(source).ok().map(String::from),
        "attribute" => attribute_chain_text(node, source),
        _ => None,
    }
}

/// Collect every `call_expression` node anywhere in the subtree rooted at
/// `node`. Uses the same iterative cursor descent as `collect_asserts`.
fn collect_calls<'a>(node: Node<'a>, out: &mut Vec<Node<'a>>) {
    collect_descendants(node, "call", out);
}

/// Collect every `assert_statement` node anywhere in the subtree rooted at
/// `node`.
fn collect_asserts<'a>(node: Node<'a>, out: &mut Vec<Node<'a>>) {
    collect_descendants(node, "assert_statement", out);
}

/// Iterative tree-sitter cursor descent: collect every descendant of
/// `node` whose `kind() == target_kind`. Walks the subtree iteratively,
/// avoiding the per-node cursor allocations that a children-iterator loop
/// would incur. Bounded to the subtree rooted at `node` — we must not
/// ascend past the starting node when backing out of dead-ends.
fn collect_descendants<'a>(node: Node<'a>, target_kind: &str, out: &mut Vec<Node<'a>>) {
    let mut cursor = node.walk();
    let start_id = node.id();
    loop {
        let current = cursor.node();
        if current.kind() == target_kind {
            out.push(current);
        }

        if cursor.goto_first_child() {
            continue;
        }
        if current.id() != start_id && cursor.goto_next_sibling() {
            continue;
        }
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
        parse_python_file(src, &PathBuf::from("synthetic.py")).expect("parse").test_functions
    }

    fn parse_full(src: &str) -> ParsedFile {
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

    #[test]
    fn decorated_top_level_test_is_detected() {
        let src = "\
@pytest.mark.parametrize('x', [1, 2])
def test_decorated(x):
    assert x > 0
";
        let recs = parse(src);
        assert_eq!(recs.len(), 1);
        assert_eq!(recs[0].nodeid, "synthetic.py::test_decorated");
        assert_eq!(recs[0].assertion_count, 1);
    }

    #[test]
    fn multi_decorator_test_is_detected() {
        let src = "\
@pytest.mark.slow
@pytest.mark.parametrize('x', [1])
def test_stacked(x):
    assert x
";
        let recs = parse(src);
        assert_eq!(recs.len(), 1);
        assert_eq!(recs[0].nodeid, "synthetic.py::test_stacked");
    }

    #[test]
    fn async_def_test_is_detected() {
        let src = "\
async def test_async_thing():
    assert True
";
        let recs = parse(src);
        assert_eq!(recs.len(), 1);
        assert_eq!(recs[0].nodeid, "synthetic.py::test_async_thing");
        assert_eq!(recs[0].assertion_count, 1);
    }

    #[test]
    fn decorated_async_def_test_is_detected() {
        let src = "\
@pytest.mark.anyio
async def test_decorated_async():
    assert mock.called
";
        let recs = parse(src);
        assert_eq!(recs.len(), 1);
        assert_eq!(recs[0].nodeid, "synthetic.py::test_decorated_async");
        assert!(recs[0].only_asserts_on_mock);
    }

    #[test]
    fn class_nested_test_method_uses_pytest_nodeid_format() {
        let src = "\
class TestFoo:
    def test_bar(self):
        assert 1 == 1
";
        let recs = parse(src);
        assert_eq!(recs.len(), 1);
        assert_eq!(recs[0].nodeid, "synthetic.py::TestFoo::test_bar");
        assert_eq!(recs[0].assertion_count, 1);
    }

    #[test]
    fn decorated_class_method_is_detected() {
        let src = "\
class TestFoo:
    @pytest.mark.skip
    def test_bar(self):
        assert mock.called
";
        let recs = parse(src);
        assert_eq!(recs.len(), 1);
        assert_eq!(recs[0].nodeid, "synthetic.py::TestFoo::test_bar");
        assert!(recs[0].only_asserts_on_mock);
    }

    #[test]
    fn multiple_methods_in_test_class_all_collected() {
        let src = "\
class TestThing:
    def test_one(self):
        assert 1
    @pytest.mark.parametrize('x', [1])
    def test_two(self, x):
        assert x
    async def test_three(self):
        assert True
";
        let recs = parse(src);
        assert_eq!(recs.len(), 3);
        let ids: Vec<&str> = recs.iter().map(|r| r.nodeid.as_str()).collect();
        assert!(ids.contains(&"synthetic.py::TestThing::test_one"));
        assert!(ids.contains(&"synthetic.py::TestThing::test_two"));
        assert!(ids.contains(&"synthetic.py::TestThing::test_three"));
    }

    #[test]
    fn non_test_class_is_ignored() {
        let src = "\
class Helper:
    def test_bar(self):
        assert True
";
        let recs = parse(src);
        assert!(recs.is_empty());
    }

    #[test]
    fn decorated_test_class_methods_are_detected() {
        let src = "\
@some_decorator
class TestFoo:
    def test_bar(self):
        assert 1
";
        let recs = parse(src);
        assert_eq!(recs.len(), 1);
        assert_eq!(recs[0].nodeid, "synthetic.py::TestFoo::test_bar");
    }

    #[test]
    fn non_test_function_named_test_is_skipped() {
        let src = "\
@pytest.fixture
def some_fixture():
    return 1

def helper():
    assert False
";
        let recs = parse(src);
        assert!(recs.is_empty());
    }

    // ---- Run 3 Phase 1 additions ------------------------------------------

    #[test]
    fn mock_construction_counted_for_each_constructor_name() {
        let src = "\
def test_x():
    a = Mock()
    b = MagicMock()
    c = AsyncMock()
    d = create_autospec(Service)
    with patch('mod.thing') as p:
        assert a.called
";
        let parsed = parse_full(src);
        // Five constructor calls in the test body: Mock, MagicMock,
        // AsyncMock, create_autospec, patch.
        assert_eq!(parsed.mock_construction_count, 5);
        assert_eq!(parsed.test_functions.len(), 1);
    }

    #[test]
    fn patch_decorators_counted_on_top_level_and_class_methods() {
        let src = "\
@patch('a')
def test_one():
    assert True

@mock.patch('b')
def test_two():
    assert True

@patch.object(Foo, 'bar')
def test_three():
    assert True

@foo.patch
def test_four():
    assert True

class TestK:
    @patch('z')
    def test_method(self):
        assert True
";
        let parsed = parse_full(src);
        let by_name: std::collections::BTreeMap<&str, u64> = parsed
            .test_functions
            .iter()
            .map(|t| (t.nodeid.rsplit("::").next().unwrap_or(""), t.patch_decorator_count))
            .collect();
        assert_eq!(by_name.get("test_one"), Some(&1));
        assert_eq!(by_name.get("test_two"), Some(&1));
        assert_eq!(by_name.get("test_three"), Some(&1));
        assert_eq!(by_name.get("test_four"), Some(&1));
        assert_eq!(by_name.get("test_method"), Some(&1));
        // Five tests, one patch decorator each → file-level sum = 5.
        assert_eq!(parsed.patch_decorator_count, 5);
    }

    #[test]
    fn patch_decorator_count_handles_stacked_decorators() {
        let src = "\
@patch('a')
@patch('b')
def test_x():
    assert True
";
        let parsed = parse_full(src);
        assert_eq!(parsed.test_functions.len(), 1);
        assert_eq!(parsed.test_functions[0].patch_decorator_count, 2);
        assert_eq!(parsed.patch_decorator_count, 2);
    }

    #[test]
    fn fixture_count_at_file_level_counts_only_pytest_fixtures() {
        let src = "\
import pytest
from dataclasses import dataclass

@pytest.fixture
def fix_a():
    return 1

@fixture
def fix_b():
    return 2

@pytest.fixture()
def fix_c():
    return 3

@dataclass
class Foo:
    x: int

class Bar:
    @property
    def value(self):
        return 1

def test_x():
    assert True
";
        let parsed = parse_full(src);
        assert_eq!(parsed.fixture_count, 3);
    }

    #[test]
    fn setup_to_assertion_ratio_lines_between_def_and_first_assert() {
        let src = "\
def test_x():
    a = 1
    b = 2
    c = 3
    assert a == 1
";
        let parsed = parse_full(src);
        let t = &parsed.test_functions[0];
        // def at row 0, first assert at row 4 → delta 4; one assert → 4/1.
        assert!((t.setup_to_assertion_ratio - 4.0).abs() < 1e-9);
    }

    #[test]
    fn setup_to_assertion_ratio_zero_assert_fallback() {
        let src = "\
def test_x():
    a = 1
    b = 2
    c = 3
";
        let parsed = parse_full(src);
        let t = &parsed.test_functions[0];
        // def at row 0; body's last statement at row 3; ratio = 3 / 1.
        assert!((t.setup_to_assertion_ratio - 3.0).abs() < 1e-9);
    }

    #[test]
    fn setup_to_assertion_ratio_multiple_asserts_divides_by_assertion_count() {
        let src = "\
def test_x():
    a = 1
    assert a == 1
    assert a > 0
";
        let parsed = parse_full(src);
        let t = &parsed.test_functions[0];
        // def at row 0, first assert at row 2 → delta 2; two asserts → 2/2.
        assert!((t.setup_to_assertion_ratio - 1.0).abs() < 1e-9);
    }

    #[test]
    fn called_names_emit_dot_joined_head_chains() {
        let src = "\
def test_x():
    repo.save(1)
    Repository()
    uuid.uuid4()
    assert True
";
        let parsed = parse_full(src);
        let names = &parsed.test_functions[0].called_names;
        assert_eq!(
            names,
            &vec!["Repository".to_string(), "repo.save".to_string(), "uuid.uuid4".to_string()]
        );
    }

    #[test]
    fn called_names_filter_self_calls() {
        let src = "\
class TestT:
    def test_x(self):
        self.assertTrue(True)
        self.client.get('/')
        assert True
";
        let parsed = parse_full(src);
        assert_eq!(parsed.test_functions.len(), 1);
        let names = &parsed.test_functions[0].called_names;
        assert!(names.is_empty(), "expected no called_names, got {names:?}");
    }

    #[test]
    fn called_names_dedup_and_sort_within_test() {
        let src = "\
def test_x():
    repo.save(1)
    repo.save(2)
    repo.load(3)
    assert True
";
        let parsed = parse_full(src);
        let names = &parsed.test_functions[0].called_names;
        assert_eq!(names, &vec!["repo.load".to_string(), "repo.save".to_string()]);
    }

    #[test]
    fn called_names_both_nodeid_shapes() {
        let src = "\
class TestT:
    def test_method(self):
        repo.save(1)
        assert True

def test_top_level():
    Service().run()
    assert True
";
        let parsed = parse_full(src);
        let by_id: std::collections::BTreeMap<&str, &Vec<String>> =
            parsed.test_functions.iter().map(|t| (t.nodeid.as_str(), &t.called_names)).collect();
        let cls =
            by_id.get("synthetic.py::TestT::test_method").expect("class-nested record present");
        assert_eq!(*cls, &vec!["repo.save".to_string()]);
        let top = by_id.get("synthetic.py::test_top_level").expect("top-level record present");
        // `Service().run()` chains a call onto another call — only `Service`
        // is a pure attribute chain at the function child of the outermost
        // call expression; the `.run()` callable's function child is itself
        // a `call`, which `call_head_chain` returns `None` for.
        assert_eq!(*top, &vec!["Service".to_string()]);
    }

    // ---- ImportMap construction (optional Phase 1 work) -------------------

    #[test]
    fn import_map_handles_plain_imports() {
        let src = "\
import foo
import bar.baz
";
        let parsed = parse_full(src);
        assert_eq!(parsed.import_map.aliases.get("foo"), Some(&"foo".to_string()));
        assert_eq!(parsed.import_map.aliases.get("bar"), Some(&"bar.baz".to_string()));
    }

    #[test]
    fn import_map_handles_aliases() {
        let src = "\
import foo.bar as fb
import quux as q
";
        let parsed = parse_full(src);
        assert_eq!(parsed.import_map.aliases.get("fb"), Some(&"foo.bar".to_string()));
        assert_eq!(parsed.import_map.aliases.get("q"), Some(&"quux".to_string()));
    }

    #[test]
    fn import_map_handles_from_imports() {
        let src = "\
from foo import bar
from foo import baz as b
";
        let parsed = parse_full(src);
        assert_eq!(parsed.import_map.aliases.get("bar"), Some(&"foo".to_string()));
        assert_eq!(parsed.import_map.aliases.get("b"), Some(&"foo".to_string()));
    }

    #[test]
    fn import_map_records_star_imports() {
        let src = "\
from foo import *
";
        let parsed = parse_full(src);
        assert!(parsed.import_map.star_sources.contains("foo"));
        // No alias-map entry for star imports.
        assert!(parsed.import_map.aliases.is_empty());
    }
}
