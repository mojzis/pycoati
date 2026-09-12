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
//! `assertion_count` reflects the number of effective assertions in each
//! test function's body — `assert_statement` nodes plus `with pytest.raises(...)`
//! / `with raises(...)` blocks. A raises block is a non-mock effective
//! assertion: it contributes to the count and disqualifies
//! [`only_asserts_on_mock`](crate::TestRecord). The `only_asserts_on_mock`
//! predicate is `true` when every `assert_statement` targets a Mock-API
//! attribute *and* the test contains no raises blocks.
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
//!   suspicion-score axis — unless the body carries external verification,
//!   in which case the first verification call site plays the assertion's
//!   role.
//! * `external_verification_count` — call sites that check a child
//!   process's exit status, so that a failure outside this process still
//!   fails the test. See [`crate::verification`] for the matched shapes and
//!   for why the inference stops where it does.
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

use crate::mock_api::{
    chain_constructs_a_mock, is_mock_api_attribute, is_mock_constructor, is_stub_call_head,
};
use crate::sut_calls::{canonicalize_called_name, dotted_head, is_at_or_under, ImportMap};
use crate::verification::{
    is_checkable_subprocess_call, is_checked_subprocess_call, is_returncode_check,
    is_subprocess_name, is_suppress_context_manager, CHECK_KEYWORD, TRUE_LITERAL,
};
use crate::TestRecord;

/// What [`parse_python_file`] returns: the per-test records plus per-file
/// aggregates and the import map needed for Phase 2 sut-call resolution.
///
/// Lives in the crate-private `parser` module; nothing outside the crate
/// touches the parser's structured output — the public entry point is
/// [`crate::run_static`], which returns a fully-populated
/// [`crate::Inventory`].
#[derive(Debug, Clone, Default)]
pub struct ParsedFile {
    pub test_functions: Vec<TestRecord>,
    /// Sum across every test body of `call_expression` heads matching
    /// [`crate::mock_api::MOCK_CONSTRUCTORS`].
    pub mock_construction_count: u64,
    /// Sum of `patch_decorator_count` across every test record.
    pub patch_decorator_count: u64,
    /// Sum across every test body of `call_expression` heads matching
    /// [`crate::mock_api::STUB_HEADS`] — fixture-driven patching via
    /// `monkeypatch.*` or `mocker.*`.
    pub stubs_count: u64,
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

    // The import map is built before the tests are walked: resolving a call
    // head against it (`sp.run` → `subprocess.run`) is part of building each
    // record, not a later pass.
    let import_map = build_import_map(root, bytes);

    let mut parsed = ParsedFile::default();
    collect_module_tests(root, bytes, file_path, &import_map, &mut parsed);
    parsed.fixture_count = count_fixture_decorators(root, bytes);
    parsed.import_map = import_map;
    Ok(parsed)
}

/// What an enclosing `class` contributes to one of its test methods:
/// pytest's nodeid prefix, plus the names the class replaced with a test
/// double for every method in it — a class-level `@patch`, or a `setUp`
/// that starts a patcher. Module-level tests carry [`ClassContext::NONE`].
#[derive(Clone, Copy)]
struct ClassContext<'a> {
    prefix: Option<&'a str>,
    /// Dotted names the class installed doubles for. Borrowed because the
    /// set is computed once per class and shared by every method in it.
    shadows: &'a BTreeSet<String>,
}

impl ClassContext<'_> {
    /// A module-level test: no nodeid prefix, no class-installed doubles.
    const NONE: Self = Self { prefix: None, shadows: &BTreeSet::new() };
}

/// Iterate the immediate children of the module and dispatch on node kind:
/// bare `function_definition`s (top-level tests), `decorated_definition`s
/// (wrapping a function or a class), and `class_definition`s (pytest test
/// containers when named `Test*`).
fn collect_module_tests(
    module: Node<'_>,
    source: &[u8],
    file_path: &Path,
    imports: &ImportMap,
    out: &mut ParsedFile,
) {
    let mut cursor = module.walk();
    for child in module.children(&mut cursor) {
        match child.kind() {
            "function_definition" => {
                try_collect_test_function(
                    child,
                    None,
                    source,
                    file_path,
                    ClassContext::NONE,
                    imports,
                    out,
                );
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
                                ClassContext::NONE,
                                imports,
                                out,
                            );
                        }
                        "class_definition" => {
                            collect_class_tests(
                                inner,
                                Some(child),
                                source,
                                file_path,
                                imports,
                                out,
                            );
                        }
                        _ => {}
                    }
                }
            }
            "class_definition" => {
                collect_class_tests(child, None, source, file_path, imports, out);
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
    class_decorated: Option<Node<'_>>,
    source: &[u8],
    file_path: &Path,
    imports: &ImportMap,
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
    let shadows = class_installed_doubles(body, class_decorated, source, imports);
    let class = ClassContext { prefix: Some(class_name), shadows: &shadows };
    let mut cursor = body.walk();
    for child in body.children(&mut cursor) {
        match child.kind() {
            "function_definition" => {
                try_collect_test_function(child, None, source, file_path, class, imports, out);
            }
            "decorated_definition" => {
                if let Some(inner) = child.child_by_field_name("definition") {
                    if inner.kind() == "function_definition" {
                        try_collect_test_function(
                            inner,
                            Some(child),
                            source,
                            file_path,
                            class,
                            imports,
                            out,
                        );
                    }
                }
            }
            _ => {}
        }
    }
}

/// Lifecycle hooks that run before a test method and therefore install
/// doubles on its behalf. `unittest`'s `setUp` / `setUpClass` and pytest's
/// xunit-style `setup_method` / `setup_class`; `conftest.py` fixtures are
/// out of reach of a single-file parser.
const SETUP_METHODS: &[&str] = &["setUp", "setUpClass", "setup_method", "setup_class"];

/// Names a class replaced with a test double for all of its methods: the
/// targets of a class-level `@patch`, and of any patch-shaped call in a
/// setup hook (`self.patcher = patch("a.b"); self.patcher.start()`).
fn class_installed_doubles(
    class_body: Node<'_>,
    class_decorated: Option<Node<'_>>,
    source: &[u8],
    imports: &ImportMap,
) -> BTreeSet<String> {
    let mut names = BTreeSet::new();

    if let Some(decorated) = class_decorated {
        names.extend(decorator_patch_targets(decorated, source, imports));
    }

    let mut cursor = class_body.walk();
    for child in class_body.children(&mut cursor) {
        let Some(func) = setup_method(child, source) else {
            continue;
        };
        let Some(hook_body) = func.child_by_field_name("body") else {
            continue;
        };
        let mut calls: Vec<Node<'_>> = Vec::new();
        collect_calls(hook_body, &mut calls);
        names.extend(patch_call_targets(&calls, source, imports));
    }

    names
}

/// The `function_definition` behind a class-body child when it is a setup
/// hook, unwrapping a `decorated_definition` first.
fn setup_method<'a>(child: Node<'a>, source: &[u8]) -> Option<Node<'a>> {
    let func = match child.kind() {
        "function_definition" => child,
        "decorated_definition" => child
            .child_by_field_name("definition")
            .filter(|inner| inner.kind() == "function_definition")?,
        _ => return None,
    };
    let name = node_name(func, source)?;
    SETUP_METHODS.contains(&name).then_some(func)
}

/// Push one record if the `function_definition` is a `test_*`. When the
/// function is wrapped by a `decorated_definition`, `decorated` carries
/// that wrapper so decorator counts can be extracted.
fn try_collect_test_function(
    func: Node<'_>,
    decorated: Option<Node<'_>>,
    source: &[u8],
    file_path: &Path,
    class: ClassContext<'_>,
    imports: &ImportMap,
    out: &mut ParsedFile,
) {
    if let Some(name) = node_name(func, source) {
        if name.starts_with("test_") {
            let (record, mock_constructions) =
                build_record(func, decorated, name, source, file_path, class, imports);
            out.mock_construction_count =
                out.mock_construction_count.saturating_add(mock_constructions);
            out.patch_decorator_count =
                out.patch_decorator_count.saturating_add(record.patch_decorator_count);
            // `stubs_count` is per-test (lives on `TestRecord`); the file
            // aggregate is just the sum of the per-test counts. Mirrors the
            // `patch_decorator_count` aggregation above.
            out.stubs_count = out.stubs_count.saturating_add(record.stubs_count);
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
    class: ClassContext<'_>,
    imports: &ImportMap,
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
    let mut raises_blocks: Vec<Node<'_>> = Vec::new();
    let mut unittest_asserts: Vec<Node<'_>> = Vec::new();
    if let Some(body_node) = body {
        collect_asserts(body_node, &mut asserts);
        collect_calls(body_node, &mut calls);
        collect_raises_blocks(body_node, source, &mut raises_blocks);
        // Reuse the `calls` vector already walked above instead of rewalking
        // the body — keeps the parse pass single-walk per node kind.
        collect_unittest_asserts(&calls, class.prefix, source, &mut unittest_asserts);
    }

    // `with pytest.raises(...)` blocks are non-mock effective assertions: they
    // count toward `assertion_count` and disqualify `only_asserts_on_mock`.
    // `self.assertXxx(...)` method calls inside a `unittest.TestCase` subclass
    // are also non-mock effective assertions: each call site counts once.
    let assertion_count = (asserts.len() + raises_blocks.len() + unittest_asserts.len()) as u64;
    let only_asserts_on_mock = !asserts.is_empty()
        && raises_blocks.is_empty()
        && unittest_asserts.is_empty()
        && asserts.iter().all(|a| assert_targets_mock_api(*a, source));

    let mock_construction_count =
        calls.iter().filter(|c| call_head_is_mock_constructor(**c, source)).count() as u64;

    // Fixture-driven stub call sites in this test's body. Walks the same
    // `calls` vector — no extra tree walk.
    let stubs_count = calls
        .iter()
        .filter_map(|c| call_head_chain(*c, source))
        .filter(|h| is_stub_call_head(h))
        .count() as u64;

    let called_names = called_names_for_test(&calls, source);

    // Verification the body performs outside this process. Walks the same
    // `calls` vector as the counts above — no extra tree walk. Names the
    // test replaced with a double are excluded: checking a double's exit
    // status is not evidence.
    let shadowed = shadowed_names(func, decorated, &calls, source, imports, class.shadows);
    let verification_sites = external_verification_sites(&calls, body, source, imports, &shadowed);
    let external_verification_count = verification_sites.len() as u64;

    let patch_decorator_count = decorated.map_or(0, |d| count_patch_decorators(d, source));

    let setup_to_assertion_ratio =
        compute_setup_to_assertion_ratio(func, body, &asserts, &raises_blocks, &verification_sites);

    let nodeid = match class.prefix {
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
        stubs_count,
        setup_to_assertion_ratio,
        external_verification_count,
        called_names,
        smell_hits: Vec::new(),
        suspicion_score: 0.0,
    };
    (record, mock_construction_count)
}

/// Compute `setup_to_assertion_ratio` per the locked Run 3 definition,
/// extended to treat `with pytest.raises(...)` blocks as effective assertions.
///
/// With at least one effective assertion (assert statement or raises block):
///   `(first_effective_row - def_row) / effective_count`
///
/// With zero effective assertions but at least one external-verification
/// call site (see [`crate::verification`]), that call site takes the place
/// of the first assertion:
///   `(first_verification_row - def_row) / verification_count`
///
/// With neither:
///   `(last_body_row - def_row) / 1`
///
/// Row deltas are taken from tree-sitter `start_position().row` (0-indexed);
/// only the delta matters, so the index base is irrelevant.
fn compute_setup_to_assertion_ratio(
    func: Node<'_>,
    body: Option<Node<'_>>,
    asserts: &[Node<'_>],
    raises_blocks: &[Node<'_>],
    verification_sites: &[Node<'_>],
) -> f64 {
    let def_row = func.start_position().row;
    let effective_count = asserts.len() + raises_blocks.len();
    let first_row =
        asserts.iter().chain(raises_blocks.iter()).map(|n| n.start_position().row).min();
    if let Some(first) = first_row {
        let delta = first.saturating_sub(def_row) as f64;
        let denom = effective_count.max(1) as f64;
        return delta / denom;
    }
    // No local assertion, but the body checks a child process: the body is
    // not "all setup and no verification", so the body-height fallback below
    // would overstate it. Measure to the first verification call site
    // instead, exactly as the assert branch does.
    let first_verification = verification_sites.iter().map(|n| n.start_position().row).min();
    if let Some(first) = first_verification {
        let delta = first.saturating_sub(def_row) as f64;
        let denom = verification_sites.len().max(1) as f64;
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

/// Select the call sites in `calls` that verify an outcome outside this
/// process, in source order.
///
/// Three shapes qualify, and only these (see [`crate::verification`] for
/// why the inference stops here):
///
/// * a call canonicalizing to `subprocess.check_call` / `check_output`,
///   which raise on a non-zero exit status unconditionally;
/// * a call canonicalizing to `subprocess.run` that passes `check=True`;
/// * `<receiver>.check_returncode()`, the explicit check on a
///   `CompletedProcess`.
///
/// Call heads are canonicalized through the file's import map first, so
/// aliases (`import subprocess as sp`, `from subprocess import run as r`)
/// resolve. Three things are rejected, all so the count never credits
/// evidence the test manufactured or never runs:
///
/// * a call whose name the test replaced with a double (`@patch`,
///   `monkeypatch.setattr`, a fixture parameter shadowing the module) —
///   a double never exits non-zero, so checking it proves nothing;
/// * a call nested inside a `def` or `lambda` in the body, which the test
///   may never invoke;
/// * a head that does not resolve (`from subprocess import *`), matched as
///   written and therefore missed.
///
/// The last two are silent under-counts, which is the safe direction for a
/// signal that suppresses another one.
fn external_verification_sites<'a>(
    calls: &[Node<'a>],
    body: Option<Node<'_>>,
    source: &[u8],
    imports: &ImportMap,
    shadowed: &BTreeSet<String>,
) -> Vec<Node<'a>> {
    calls
        .iter()
        .copied()
        .filter(|call| {
            let Some(chain) = call_head_chain(*call, source) else {
                return false;
            };
            if verification_is_unreliable(*call, body, source, imports)
                || head_is_shadowed(&chain, shadowed)
            {
                return false;
            }
            if is_returncode_check(&chain) {
                // A `CompletedProcess` can only come from a subprocess call,
                // so if the test replaced that boundary the receiver is a
                // double and checking it is vacuous. The receiver itself is
                // a local name, which `head_is_shadowed` above only catches
                // when it was assigned from something the test replaced.
                return !subprocess_boundary_shadowed(shadowed);
            }
            let canonical = canonicalize_called_name(&chain, imports);
            if is_shadowed(&canonical, shadowed) {
                return false;
            }
            is_checked_subprocess_call(&canonical)
                || (is_checkable_subprocess_call(&canonical)
                    && call_passes_check_true(*call, source))
        })
        .collect()
}

/// True iff `name` is, or lives under, a name the test replaced.
/// `subprocess` shadows `subprocess.run`; `subprocess.run` shadows itself.
fn is_shadowed(name: &str, shadowed: &BTreeSet<String>) -> bool {
    shadowed.iter().any(|s| is_at_or_under(name, s))
}

/// True iff the test replaced the `subprocess` module or any name under it.
fn subprocess_boundary_shadowed(shadowed: &BTreeSet<String>) -> bool {
    shadowed.iter().any(|s| is_subprocess_name(s))
}

/// True iff the head segment of a raw call chain is a shadowed name. Checked
/// before canonicalization so a parameter binding (`def test(subprocess)`)
/// wins over the module the file imported under the same name.
fn head_is_shadowed(chain: &str, shadowed: &BTreeSet<String>) -> bool {
    shadowed.contains(dotted_head(chain))
}

/// True iff `call` sits somewhere the test cannot rely on its failure:
///
/// * inside a `def`, `lambda` or generator expression nested in the body,
///   which the test may define and never invoke;
/// * inside a `try` block that has an `except` clause, which may swallow
///   the `CalledProcessError` the whole signal rests on — or inside the
///   `except` clause itself, which only runs on the error path;
/// * inside `with contextlib.suppress(...)`, which discards it outright.
///
/// One ancestor scan up to `body`; a missing body is treated as reliable,
/// matching how the other per-test counts degrade on a bodyless function.
fn verification_is_unreliable(
    call: Node<'_>,
    body: Option<Node<'_>>,
    source: &[u8],
    imports: &ImportMap,
) -> bool {
    let Some(body_node) = body else {
        return false;
    };
    let mut current = call;
    while let Some(parent) = current.parent() {
        if parent.id() == body_node.id() {
            return false;
        }
        if matches!(
            parent.kind(),
            "function_definition" | "lambda" | "generator_expression" | "except_clause"
        ) {
            return true;
        }
        if parent.kind() == "try_statement" && try_guards_child(parent, current) {
            return true;
        }
        if parent.kind() == "with_statement" && with_suppresses_errors(parent, source, imports) {
            return true;
        }
        current = parent;
    }
    false
}

/// True iff a `with_statement` opens `contextlib.suppress(...)` on one of
/// its own items.
///
/// Scoped to the statement's single `with_clause`, never a subtree walk: an
/// unrelated `with contextlib.suppress(...)` further down the same block
/// must not disarm evidence that sits above it.
fn with_suppresses_errors(with_stmt: Node<'_>, source: &[u8], imports: &ImportMap) -> bool {
    let mut cursor = with_stmt.walk();
    let Some(clause) = with_stmt.children(&mut cursor).find(|c| c.kind() == "with_clause") else {
        return false;
    };
    let mut item_cursor = clause.walk();
    let suppresses = clause
        .named_children(&mut item_cursor)
        .filter(|item| item.kind() == "with_item")
        .any(|item| {
            item.child_by_field_name("value")
                .filter(|value| value.kind() == "call")
                .and_then(|value| call_head_chain(value, source))
                .is_some_and(|chain| {
                    is_suppress_context_manager(&canonicalize_called_name(&chain, imports))
                })
        });
    // Bound rather than returned directly: the iterator borrows `item_cursor`.
    suppresses
}

/// True iff `child` is the guarded block of a `try_statement` that has an
/// `except` clause. A `try`/`finally` with no `except` swallows nothing, so
/// verification inside it still propagates.
fn try_guards_child(try_stmt: Node<'_>, child: Node<'_>) -> bool {
    if try_stmt.child_by_field_name("body").is_none_or(|b| b.id() != child.id()) {
        return false;
    }
    let mut cursor = try_stmt.walk();
    let has_except = try_stmt
        .children(&mut cursor)
        .any(|n| matches!(n.kind(), "except_clause" | "except_group_clause"));
    // Bound rather than returned directly: the iterator borrows `cursor`.
    has_except
}

/// Names this test replaced with a test double, canonicalized through the
/// import map where they are dotted.
///
/// Three sources, matching how a pytest test installs a double:
///
/// * the target of a `@patch`-shaped decorator — `@patch("subprocess.run")`;
/// * the target of a patch-shaped call in the body — `patch("a.b")` as a
///   context manager, `mocker.patch("a.b")`, and the two-argument form
///   `patch.object(mod, "attr")` / `monkeypatch.setattr(mod, "attr", …)`;
/// * the test's own parameter names, which shadow anything the module
///   imported under the same name (a `subprocess` fixture, say).
fn shadowed_names(
    func: Node<'_>,
    decorated: Option<Node<'_>>,
    calls: &[Node<'_>],
    source: &[u8],
    imports: &ImportMap,
    class_shadows: &BTreeSet<String>,
) -> BTreeSet<String> {
    // Seeded with what the enclosing class installed for every method: a
    // class-level `@patch`, or a patcher started in `setUp`.
    let mut names = class_shadows.clone();

    if let Some(params) = func.child_by_field_name("parameters") {
        let mut cursor = params.walk();
        for param in params.named_children(&mut cursor) {
            if let Some(name) = parameter_name(param, source) {
                // `self` binds the test instance, never a double, and every
                // other consumer of a call chain already special-cases it.
                if name == "self" {
                    continue;
                }
                names.insert(name.to_string());
            }
        }
    }

    if let Some(decorated_node) = decorated {
        names.extend(decorator_patch_targets(decorated_node, source, imports));
    }

    names.extend(patch_call_targets(calls, source, imports));

    // Last, because it reads the set built above: a local bound to a mock
    // constructor — or to anything already shadowed — is itself a double.
    // This is what disqualifies `completed = MagicMock()` followed by
    // `completed.check_returncode()`.
    if let Some(body) = func.child_by_field_name("body") {
        for (target, value) in double_bindings(body) {
            if binds_a_double(value, source, imports, &names) {
                if let Ok(name) = target.utf8_text(source) {
                    names.insert(name.to_string());
                }
            }
        }
    }

    names
}

/// Every `(bound_identifier, call_expression)` pair in a test body, across
/// the binding forms that can put a test double in a local:
/// `m = Mock()`, `with patch(...) as m:`, and `(m := Mock())`. Tuple
/// assignments are zipped positionally when both sides have equal arity.
fn double_bindings<'a>(body: Node<'a>) -> Vec<(Node<'a>, Node<'a>)> {
    let mut pairs = Vec::new();

    let mut assignments: Vec<Node<'a>> = Vec::new();
    collect_descendants(body, "assignment", &mut assignments);
    for assignment in assignments {
        let (Some(target), Some(value)) =
            (assignment.child_by_field_name("left"), assignment.child_by_field_name("right"))
        else {
            continue;
        };
        if target.kind() == "identifier" {
            if value.kind() == "call" {
                pairs.push((target, value));
            }
            continue;
        }
        pairs.extend(zip_tuple_binding(target, value));
    }

    let mut named: Vec<Node<'a>> = Vec::new();
    collect_descendants(body, "named_expression", &mut named);
    for expr in named {
        if let (Some(target), Some(value)) =
            (expr.child_by_field_name("name"), expr.child_by_field_name("value"))
        {
            if target.kind() == "identifier" && value.kind() == "call" {
                pairs.push((target, value));
            }
        }
    }

    let mut items: Vec<Node<'a>> = Vec::new();
    collect_descendants(body, "with_item", &mut items);
    for item in items {
        // `with patch(...) as m:` parses the item value as an `as_pattern`
        // whose first child is the call and whose `alias` field is the name.
        let Some(pattern) = item.named_child(0).filter(|v| v.kind() == "as_pattern") else {
            continue;
        };
        let (Some(value), Some(alias)) =
            (pattern.named_child(0), pattern.child_by_field_name("alias"))
        else {
            continue;
        };
        let target = if alias.kind() == "identifier" { Some(alias) } else { alias.named_child(0) };
        if let Some(target) = target.filter(|t| t.kind() == "identifier") {
            if value.kind() == "call" {
                pairs.push((target, value));
            }
        }
    }

    pairs
}

/// Zip a tuple assignment positionally: `a, m = 1, Mock()` binds `m` to the
/// second value. Mismatched arities bind nothing — a starred target or an
/// unpacked call makes the correspondence unknowable.
fn zip_tuple_binding<'a>(target: Node<'a>, value: Node<'a>) -> Vec<(Node<'a>, Node<'a>)> {
    if !matches!(target.kind(), "pattern_list" | "tuple_pattern")
        || !matches!(value.kind(), "expression_list" | "tuple")
    {
        return Vec::new();
    }
    let mut target_cursor = target.walk();
    let targets: Vec<Node<'a>> = target.named_children(&mut target_cursor).collect();
    let mut value_cursor = value.walk();
    let values: Vec<Node<'a>> = value.named_children(&mut value_cursor).collect();
    if targets.len() != values.len() {
        return Vec::new();
    }
    targets
        .into_iter()
        .zip(values)
        .filter(|(t, v)| t.kind() == "identifier" && v.kind() == "call")
        .collect()
}

/// Targets of every `@patch`-shaped decorator on a `decorated_definition`.
///
/// Matched with the same predicate as [`patch_call_targets`], which is
/// broader than the one `count_patch_decorators` uses: it also catches
/// `@unittest.mock.patch.object(...)`, which that counter misses (a
/// pre-existing gap in a scored field, left alone here). Deliberate — a
/// shadow that is too wide costs a missed piece of evidence, while one that
/// is too narrow credits a test double.
fn decorator_patch_targets(decorated: Node<'_>, source: &[u8], imports: &ImportMap) -> Vec<String> {
    let mut targets = Vec::new();
    for decorator in decorator_nodes(decorated) {
        let Some(target) = decorator_target(decorator) else {
            continue;
        };
        if !attribute_chain_text(target, source).is_some_and(|c| head_patches_a_name(&c)) {
            continue;
        }
        // `decorator_target` unwraps `@patch("a.b")` to the `patch`
        // expression; the arguments hang off its parent call node.
        if let Some(call) = target.parent().filter(|p| p.kind() == "call") {
            targets.extend(patched_target(call, source, imports));
        }
    }
    targets
}

/// Targets of every patch-shaped call in `calls`.
fn patch_call_targets(calls: &[Node<'_>], source: &[u8], imports: &ImportMap) -> Vec<String> {
    calls
        .iter()
        .filter(|call| call_head_chain(**call, source).is_some_and(|h| head_patches_a_name(&h)))
        .flat_map(|call| patched_target(*call, source, imports))
        .collect()
}

/// True iff the call on the right-hand side of an assignment yields a test
/// double: a Mock-API constructor, or a call to a name the test replaced.
fn binds_a_double(
    call: Node<'_>,
    source: &[u8],
    imports: &ImportMap,
    shadowed: &BTreeSet<String>,
) -> bool {
    call_head_chain(call, source).is_some_and(|chain| {
        // `chain_constructs_a_mock`, not `call_head_is_mock_constructor`:
        // `mock.MagicMock()` must count here even though it deliberately
        // does not count toward `mock_construction_count`.
        chain_constructs_a_mock(&chain)
            || head_is_shadowed(&chain, shadowed)
            || is_shadowed(&canonicalize_called_name(&chain, imports), shadowed)
    })
}

/// Every `decorator` child of a `decorated_definition`, in source order.
/// Shared by the two predicates that classify decorators.
fn decorator_nodes(decorated: Node<'_>) -> Vec<Node<'_>> {
    let mut cursor = decorated.walk();
    decorated.children(&mut cursor).filter(|c| c.kind() == "decorator").collect()
}

/// Extract the identifier bound by one parameter node, across the shapes
/// tree-sitter emits (`x`, `x: T`, `x=1`, `*args`, `**kwargs`).
fn parameter_name<'a>(param: Node<'_>, source: &'a [u8]) -> Option<&'a str> {
    match param.kind() {
        "identifier" => param.utf8_text(source).ok(),
        "typed_parameter" | "list_splat_pattern" | "dictionary_splat_pattern" => {
            param.named_child(0).and_then(|n| parameter_name(n, source))
        }
        "default_parameter" | "typed_default_parameter" => {
            param.child_by_field_name("name").and_then(|n| parameter_name(n, source))
        }
        _ => None,
    }
}

/// True iff a dotted call-head installs a double at a named target: any
/// chain carrying a whole `patch` segment (`patch`, `mock.patch`,
/// `patch.object`, `mocker.patch.dict`), or `monkeypatch.setattr`.
///
/// Segment equality, not a substring match: `helpers.patch_config` binds
/// nothing and must not shadow anything. An unrelated `.patch` — an HTTP
/// client's `requests.patch(url)` — is indistinguishable from `mock.patch`
/// by shape and does contribute its first argument to the shadow set; that
/// entry is inert, since nothing it can name is at or under `subprocess`.
fn head_patches_a_name(head: &str) -> bool {
    head == "monkeypatch.setattr" || head.split('.').any(|segment| segment == "patch")
}

/// Read the target name(s) out of a patch-shaped call.
///
/// `patch("a.b")` names `a.b` directly. `patch.object(mod, "attr")` and
/// `monkeypatch.setattr(mod, "attr", …)` name the attribute relative to a
/// first argument that is itself a dotted name. A first argument that is
/// neither — `patch(some_variable)` — is treated as a receiver expression
/// and shadows the variable's own name, which is usually nothing useful: a
/// computed target is a known miss, and the call site keeps whatever
/// evidence it had.
fn patched_target(call: Node<'_>, source: &[u8], imports: &ImportMap) -> Vec<String> {
    let Some(args) = call.child_by_field_name("arguments") else {
        return Vec::new();
    };
    let positional: Vec<Node<'_>> = {
        let mut cursor = args.walk();
        args.named_children(&mut cursor).filter(|a| a.kind() != "keyword_argument").collect()
    };
    let Some(first) = positional.first() else {
        return Vec::new();
    };
    if let Some(literal) = string_literal_text(*first, source) {
        return vec![canonicalize_called_name(literal, imports)];
    }
    let Some(receiver) = attribute_chain_text(*first, source) else {
        return Vec::new();
    };
    let attr = positional.get(1).and_then(|n| string_literal_text(*n, source));
    let canonical_receiver = canonicalize_called_name(&receiver, imports);
    match attr {
        Some(name) => vec![format!("{canonical_receiver}.{name}")],
        // `patch.object(mod)` is not a shape that patches one attribute;
        // shadow the whole receiver rather than guess.
        None => vec![canonical_receiver],
    }
}

/// Text inside a Python string literal, without its quotes. Returns `None`
/// for f-strings and any other non-plain string node.
fn string_literal_text<'a>(node: Node<'_>, source: &'a [u8]) -> Option<&'a str> {
    if node.kind() != "string" {
        return None;
    }
    let mut cursor = node.walk();
    let content: Vec<Node<'_>> =
        node.named_children(&mut cursor).filter(|c| c.kind() == "string_content").collect();
    match content.as_slice() {
        [single] => single.utf8_text(source).ok(),
        _ => None,
    }
}

/// True iff the call passes the literal `check=True` keyword argument.
///
/// Only the `True` literal counts: `check=strict` may well be true at
/// runtime, but a static reader cannot know that, and the whole point of
/// the signal is that the failure path is provable from the source.
fn call_passes_check_true(call: Node<'_>, source: &[u8]) -> bool {
    let Some(args) = call.child_by_field_name("arguments") else {
        return false;
    };
    let mut cursor = args.walk();
    let found = args.named_children(&mut cursor).any(|arg| {
        if arg.kind() != "keyword_argument" {
            return false;
        }
        let name = arg.child_by_field_name("name").and_then(|n| n.utf8_text(source).ok());
        let value = arg.child_by_field_name("value").and_then(|n| n.utf8_text(source).ok());
        name == Some(CHECK_KEYWORD) && value == Some(TRUE_LITERAL)
    });
    // Bound rather than returned directly: the iterator borrows `cursor`,
    // which must outlive it.
    found
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
    for decorator in decorator_nodes(decorated) {
        if decorator_is_patch(decorator, source) {
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
                    let local = dotted_head(&full).to_string();
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
///
/// The alias-map value is the **canonical full dotted name** of the local
/// binding — i.e. `source_module + . + imported_name`. This lets Phase 2
/// produce the canonical form `myproj.repository.Repository.save` from the
/// raw call chain `Repository.save`. For aliased imports
/// (`from foo import bar as b`), the alias-map key is the alias and the
/// value is `foo.bar`; the originally imported name (`bar`) is the suffix
/// of the canonical, not the alias.
fn record_import_from(stmt: Node<'_>, source: &[u8], map: &mut ImportMap) {
    // The `module_name` field carries the source module's dotted name.
    let Some(module_node) = stmt.child_by_field_name("module_name") else {
        return;
    };
    let Some(source_module) = attribute_or_dotted_text(module_node, source) else {
        return;
    };

    // Single pass over named children. Two-phase to avoid holding the cursor
    // across mutations on `map`.
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
                if let Some(imported) = attribute_or_dotted_text(child, source) {
                    let imported_head = dotted_head(&imported).to_string();
                    let canonical = format!("{source_module}.{imported_head}");
                    buffered.push((imported_head, canonical));
                }
            }
            "aliased_import" => {
                // (aliased_import name: (dotted_name) alias: (identifier))
                if let (Some(name_node), Some(alias_node)) =
                    (child.child_by_field_name("name"), child.child_by_field_name("alias"))
                {
                    if let (Some(imported), Ok(alias)) =
                        (attribute_or_dotted_text(name_node, source), alias_node.utf8_text(source))
                    {
                        let canonical = format!("{source_module}.{imported}");
                        buffered.push((alias.to_string(), canonical));
                    }
                }
            }
            _ => {}
        }
    }
    for (local, canonical) in buffered {
        map.aliases.insert(local, canonical);
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

/// Collect every `with_statement` in the subtree whose context manager is a
/// call to `pytest.raises(...)` or bare `raises(...)`. These are treated as
/// effective assertions (a pytest test passes when the expected exception is
/// raised inside the block, so the `with` line is the assertion site).
///
/// Bare `raises` matches `from pytest import raises` usage; the tiny
/// false-positive risk (an unrelated `raises()` context manager) is accepted
/// for the UX win on the very common pytest pattern.
fn collect_raises_blocks<'a>(node: Node<'a>, source: &[u8], out: &mut Vec<Node<'a>>) {
    let mut withs: Vec<Node<'a>> = Vec::new();
    collect_descendants(node, "with_statement", &mut withs);
    for w in withs {
        if with_statement_is_pytest_raises(w, source) {
            out.push(w);
        }
    }
}

/// Collect every `call_expression` in `calls` whose dotted call-head matches
/// the strict `camelCase` `self.assert<Upper…>` shape used by
/// `unittest.TestCase` assertion methods (`self.assertEqual`,
/// `self.assertTrue`, `self.assertIn`, the bare `self.assert`, …). These
/// count as effective assertions just like a bare `assert`.
///
/// Gated on `class_prefix.is_some()` because outside a class context
/// `self.*` cannot bind to a `TestCase` receiver. Inside `Test*` classes
/// pytest happily collects unittest subclasses, so we accept any
/// `camelCase` `self.assert*` head without further static checks.
///
/// The `camelCase` rule deliberately rejects `snake_case` lookalikes like
/// `self.assert_called_with` (which belongs to `Mock`'s API, not unittest)
/// and prefix collisions like `self.assertion_count(...)` or
/// `self.assert_logged(...)` (user-defined helpers that are not effective
/// assertions). Callers pass the body's `calls` vector that was already
/// collected once during `build_record` — this function does not rewalk
/// the tree.
fn collect_unittest_asserts<'a>(
    calls: &[Node<'a>],
    class_prefix: Option<&str>,
    source: &[u8],
    out: &mut Vec<Node<'a>>,
) {
    if class_prefix.is_none() {
        return;
    }
    for &call in calls {
        if let Some(head) = call_head_chain(call, source) {
            if let Some(method) = head.strip_prefix("self.") {
                if is_unittest_assert_method(method) {
                    out.push(call);
                }
            }
        }
    }
}

/// Strict `camelCase` predicate: `assert` followed by either nothing
/// (the bare `assert` method) or an uppercase ASCII letter. Rejects
/// `assert_called_with` (Mock API), `assertion_count` (user helper),
/// `asserter` (anything starting with `assert<lowercase>`).
fn is_unittest_assert_method(name: &str) -> bool {
    let Some(rest) = name.strip_prefix("assert") else {
        return false;
    };
    match rest.as_bytes().first() {
        None => true,
        Some(b) => b.is_ascii_uppercase(),
    }
}

/// Decide whether a `with_statement` opens a `pytest.raises(...)` or bare
/// `raises(...)` context manager. Examines the first `with_item` only —
/// chained items like `with pytest.raises(X), pytest.raises(Y):` would each
/// be a single block; multiple `with`-statements are how pytest tests
/// actually express multiple expected exceptions in series.
fn with_statement_is_pytest_raises(with_stmt: Node<'_>, source: &[u8]) -> bool {
    let mut cursor = with_stmt.walk();
    let Some(clause) = with_stmt.named_children(&mut cursor).find(|c| c.kind() == "with_clause")
    else {
        return false;
    };
    let mut clause_cursor = clause.walk();
    let Some(item) = clause.named_children(&mut clause_cursor).find(|c| c.kind() == "with_item")
    else {
        return false;
    };
    let Some(value) = item.named_child(0) else {
        return false;
    };
    // `with raises(X) as ei:` wraps the call in an `as_pattern`.
    let call_node = match value.kind() {
        "call" => value,
        "as_pattern" => match value.named_child(0) {
            Some(n) if n.kind() == "call" => n,
            _ => return false,
        },
        _ => return false,
    };
    let Some(func) = call_node.child_by_field_name("function") else {
        return false;
    };
    call_function_is_pytest_raises(func, source)
}

/// Match `pytest.raises` (attribute) or bare `raises` (identifier).
fn call_function_is_pytest_raises(func: Node<'_>, source: &[u8]) -> bool {
    match func.kind() {
        "identifier" => func.utf8_text(source).ok() == Some("raises"),
        "attribute" => {
            let obj = func.child_by_field_name("object");
            let attr = func.child_by_field_name("attribute");
            matches!(
                (
                    obj.and_then(|n| n.utf8_text(source).ok()),
                    attr.and_then(|n| n.utf8_text(source).ok())
                ),
                (Some("pytest"), Some("raises"))
            )
        }
        _ => false,
    }
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

    /// A checked subprocess call is verification evidence: the child's
    /// non-zero exit status raises `CalledProcessError` in the parent.
    #[test]
    fn checked_subprocess_run_is_external_verification() {
        let src = "\
import subprocess

def test_a():
    subprocess.run([\"prog\"], check=True)
";
        let recs = parse(src);
        assert_eq!(recs[0].assertion_count, 0, "assertion_count stays syntactic");
        assert_eq!(recs[0].external_verification_count, 1);
    }

    #[test]
    fn unchecked_subprocess_run_is_not_verification() {
        let src = "\
import subprocess

def test_a():
    subprocess.run([\"prog\"])
";
        let recs = parse(src);
        assert_eq!(recs[0].external_verification_count, 0);
    }

    #[test]
    fn check_false_is_not_verification() {
        let src = "\
import subprocess

def test_a():
    subprocess.run([\"prog\"], check=False)
";
        let recs = parse(src);
        assert_eq!(recs[0].external_verification_count, 0);
    }

    #[test]
    fn non_literal_check_value_is_not_verification() {
        // `check=strict` may be true at runtime; the source does not say so,
        // and the signal has to be provable from the source.
        let src = "\
import subprocess

def test_a(strict):
    subprocess.run([\"prog\"], check=strict)
";
        let recs = parse(src);
        assert_eq!(recs[0].external_verification_count, 0);
    }

    #[test]
    fn check_call_and_check_output_need_no_keyword() {
        let src = "\
import subprocess

def test_a():
    subprocess.check_call([\"prog\"])
    subprocess.check_output([\"prog\"])
";
        let recs = parse(src);
        assert_eq!(recs[0].external_verification_count, 2);
    }

    #[test]
    fn completed_process_check_returncode_is_verification() {
        let src = "\
import subprocess

def test_a():
    completed = subprocess.run([\"prog\"])
    completed.check_returncode()
";
        let recs = parse(src);
        assert_eq!(
            recs[0].external_verification_count, 1,
            "the unchecked run is not evidence; the explicit check is"
        );
    }

    #[test]
    fn aliased_subprocess_names_resolve_through_the_import_map() {
        let src = "\
import subprocess as sp
from subprocess import check_call
from subprocess import run as run_child

def test_module_alias():
    sp.run([\"prog\"], check=True)

def test_function_alias():
    run_child([\"prog\"], check=True)

def test_direct_import():
    check_call([\"prog\"])
";
        let recs = parse(src);
        assert_eq!(recs.len(), 3);
        for r in &recs {
            assert_eq!(r.external_verification_count, 1, "{} did not resolve", r.nodeid);
        }
    }

    #[test]
    fn check_hidden_in_a_kwargs_splat_is_not_matched() {
        // `**kwargs` may or may not carry `check=True`; the source does not
        // say. Pinned as a deliberate under-count, like the star import.
        let src = "\
import subprocess

def test_a(kwargs):
    subprocess.run([\"prog\"], **kwargs)
";
        let recs = parse(src);
        assert_eq!(recs[0].external_verification_count, 0);
    }

    #[test]
    fn a_real_assertion_wins_the_ratio_but_keeps_the_evidence() {
        let src = "\
import subprocess

def test_a():
    completed = subprocess.run([\"prog\"], check=True)
    assert completed.stdout == \"ok\"
";
        let recs = parse(src);
        assert_eq!(recs[0].assertion_count, 1);
        assert_eq!(recs[0].external_verification_count, 1);
        assert!(
            (recs[0].setup_to_assertion_ratio - 2.0).abs() < f64::EPSILON,
            "the assert branch owns the ratio, got {}",
            recs[0].setup_to_assertion_ratio
        );
    }

    #[test]
    fn a_suppress_elsewhere_in_the_block_does_not_disarm_the_evidence() {
        // Only the `with` that opens `suppress` disarms what it wraps; a
        // later cleanup block in the same body must not reach backwards.
        let src = "\
import contextlib
import subprocess

def test_a():
    with open(\"f\") as fh:
        subprocess.check_call([\"prog\"])
        with contextlib.suppress(ValueError):
            pass
";
        let recs = parse(src);
        assert_eq!(recs[0].external_verification_count, 1);
    }

    #[test]
    fn a_call_inside_suppress_is_disarmed() {
        let src = "\
import contextlib
import subprocess

def test_a():
    with contextlib.suppress(subprocess.CalledProcessError):
        subprocess.check_call([\"prog\"])
";
        let recs = parse(src);
        assert_eq!(recs[0].external_verification_count, 0);
    }

    #[test]
    fn star_imported_run_is_not_matched() {
        // `from subprocess import *` binds an unknown set of names, so the
        // head does not canonicalize and the evidence is missed. Pinned
        // because under-counting here is a deliberate choice, not an
        // oversight — see the `verification` module docs.
        let src = "\
from subprocess import *

def test_a():
    run([\"prog\"], check=True)
";
        let recs = parse(src);
        assert_eq!(recs[0].external_verification_count, 0);
    }

    #[test]
    fn patched_subprocess_is_not_verification() {
        let src = "\
import subprocess
from unittest.mock import patch

@patch(\"subprocess.run\")
def test_a(run):
    subprocess.run([\"prog\"], check=True)
";
        let recs = parse(src);
        assert_eq!(
            recs[0].external_verification_count, 0,
            "a double never exits non-zero, so checking it is not evidence"
        );
    }

    #[test]
    fn monkeypatched_attribute_is_not_verification() {
        let src = "\
import subprocess as sp

def test_a(monkeypatch):
    monkeypatch.setattr(sp, \"check_call\", lambda *a: None)
    sp.check_call([\"prog\"])
";
        let recs = parse(src);
        assert_eq!(recs[0].external_verification_count, 0, "the patch target canonicalizes too");
    }

    #[test]
    fn context_manager_patch_shadows_the_target() {
        let src = "\
import subprocess
from unittest.mock import patch

def test_a():
    with patch(\"subprocess.check_call\"):
        subprocess.check_call([\"prog\"])
";
        let recs = parse(src);
        assert_eq!(recs[0].external_verification_count, 0);
    }

    #[test]
    fn parameter_shadowing_beats_the_module_import() {
        let src = "\
import subprocess

def test_a(subprocess):
    subprocess.run([\"prog\"], check=True)
";
        let recs = parse(src);
        assert_eq!(recs[0].external_verification_count, 0);
    }

    #[test]
    fn verification_in_an_uncalled_nested_def_is_not_counted() {
        let src = "\
import subprocess

def test_a():
    def helper():
        subprocess.check_call([\"prog\"])
";
        let recs = parse(src);
        assert_eq!(recs[0].external_verification_count, 0);
    }

    #[test]
    fn patching_something_else_leaves_the_evidence_alone() {
        let src = "\
import subprocess
from unittest.mock import patch

@patch(\"myproj.thing\")
def test_a(thing):
    subprocess.run([\"prog\"], check=True)
";
        let recs = parse(src);
        assert_eq!(recs[0].external_verification_count, 1);
    }

    #[test]
    fn unrelated_run_with_check_true_is_not_verification() {
        // A local collaborator that happens to expose `run(check=...)` is
        // not a subprocess: the head does not canonicalize to `subprocess`.
        let src = "\
def test_a(runner):
    runner.run([\"prog\"], check=True)
";
        let recs = parse(src);
        assert_eq!(recs[0].external_verification_count, 0);
    }

    #[test]
    fn verification_is_counted_inside_test_classes() {
        let src = "\
import subprocess

class TestThing:
    def test_a(self):
        subprocess.check_call([\"prog\"])
";
        let recs = parse(src);
        assert_eq!(recs.len(), 1);
        assert_eq!(recs[0].external_verification_count, 1);
    }

    #[test]
    fn verification_site_replaces_the_body_height_in_the_setup_ratio() {
        // Same body twice; only the `check=True` differs. The unchecked one
        // falls back to the body height (4 rows below the `def`), the checked
        // one measures to its verification call site (row 1 below the `def`).
        let src = "\
import subprocess

def test_checked():
    cmd = [\"prog\"]
    subprocess.run(cmd, check=True)
    cleanup = True
    del cleanup

def test_unchecked():
    cmd = [\"prog\"]
    subprocess.run(cmd)
    cleanup = True
    del cleanup
";
        let recs = parse(src);
        let checked = recs.iter().find(|r| r.nodeid.ends_with("test_checked")).expect("checked");
        let unchecked =
            recs.iter().find(|r| r.nodeid.ends_with("test_unchecked")).expect("unchecked");
        assert!(
            (checked.setup_to_assertion_ratio - 2.0).abs() < f64::EPSILON,
            "expected the rows up to the verification call site, got {}",
            checked.setup_to_assertion_ratio
        );
        assert!(
            (unchecked.setup_to_assertion_ratio - 4.0).abs() < f64::EPSILON,
            "expected the body height, got {}",
            unchecked.setup_to_assertion_ratio
        );
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
        // The alias-map value is the canonical full dotted name of the
        // local binding: `source_module + . + imported_name`. For an
        // aliased from-import, the alias is the KEY and the imported name
        // (`baz`) is the suffix of the canonical value.
        let src = "\
from foo import bar
from foo import baz as b
";
        let parsed = parse_full(src);
        assert_eq!(parsed.import_map.aliases.get("bar"), Some(&"foo.bar".to_string()));
        assert_eq!(parsed.import_map.aliases.get("b"), Some(&"foo.baz".to_string()));
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

    #[test]
    fn with_pytest_raises_counts_as_assertion() {
        let src = "\
def test_raises_value():
    with pytest.raises(ValueError):
        do_thing()
";
        let recs = parse(src);
        assert_eq!(recs.len(), 1);
        assert_eq!(recs[0].assertion_count, 1);
        assert!(!recs[0].only_asserts_on_mock);
    }

    #[test]
    fn bare_raises_counts_as_assertion() {
        let src = "\
def test_raises_bare():
    with raises(KeyError) as ei:
        do_thing()
";
        let recs = parse(src);
        assert_eq!(recs[0].assertion_count, 1);
    }

    #[test]
    fn raises_block_disqualifies_only_asserts_on_mock() {
        // Mock assert plus a pytest.raises block: the raises block is a
        // non-mock effective assertion, so only_asserts_on_mock must be false.
        let src = "\
def test_mixed():
    assert mock.called
    with pytest.raises(ValueError):
        boom()
";
        let recs = parse(src);
        assert_eq!(recs[0].assertion_count, 2);
        assert!(!recs[0].only_asserts_on_mock);
    }

    #[test]
    fn multiple_raises_blocks_each_count() {
        let src = "\
def test_two_raises():
    with pytest.raises(ValueError):
        a()
    with pytest.raises(KeyError):
        b()
";
        let recs = parse(src);
        assert_eq!(recs[0].assertion_count, 2);
    }

    #[test]
    fn unrelated_with_block_is_not_an_assertion() {
        // `with open(...)` is not an assertion; only pytest.raises / raises.
        let src = "\
def test_opens_file():
    with open('x') as f:
        f.read()
";
        let recs = parse(src);
        assert_eq!(recs[0].assertion_count, 0);
    }

    #[test]
    fn nested_with_pytest_raises_is_collected() {
        // Mirrors `nested_asserts_in_if_block_are_collected`: a raises block
        // buried in control flow must still be discovered.
        let src = "\
def test_nested():
    if cond:
        with pytest.raises(ValueError):
            boom()
";
        let recs = parse(src);
        assert_eq!(recs[0].assertion_count, 1);
    }

    // ---- Phase 2 stubs_count -----------------------------------------------

    #[test]
    fn monkeypatch_calls_increment_stubs_count() {
        let src = "\
def test_x(monkeypatch):
    monkeypatch.setattr(os, 'getcwd', lambda: '/x')
    monkeypatch.setenv('FOO', 'bar')
    monkeypatch.delenv('BAZ', raising=False)
    assert True
";
        let parsed = parse_full(src);
        let t = &parsed.test_functions[0];
        assert_eq!(t.stubs_count, 3);
        assert_eq!(parsed.stubs_count, 3);
    }

    #[test]
    fn mocker_calls_increment_stubs_count() {
        let src = "\
def test_x(mocker):
    mocker.patch('mod.thing')
    mocker.patch.object(Foo, 'bar')
    mocker.spy(Foo, 'baz')
    mocker.MagicMock()
    assert True
";
        let parsed = parse_full(src);
        let t = &parsed.test_functions[0];
        assert_eq!(t.stubs_count, 4);
        assert_eq!(parsed.stubs_count, 4);
    }

    #[test]
    fn stubs_count_is_per_test_and_aggregated_to_file() {
        let src = "\
def test_one(monkeypatch):
    monkeypatch.setattr(os, 'getcwd', lambda: '/x')
    assert True

def test_two(mocker):
    mocker.patch('a')
    mocker.patch('b')
    assert True
";
        let parsed = parse_full(src);
        let by_name: std::collections::BTreeMap<&str, u64> = parsed
            .test_functions
            .iter()
            .map(|t| (t.nodeid.rsplit("::").next().unwrap_or(""), t.stubs_count))
            .collect();
        assert_eq!(by_name.get("test_one"), Some(&1));
        assert_eq!(by_name.get("test_two"), Some(&2));
        assert_eq!(parsed.stubs_count, 3);
    }

    #[test]
    fn unrelated_calls_do_not_increment_stubs_count() {
        // `mock.patch` (Mock-API module) and `monkeypatch.foobar` (unknown
        // method) must not count — STUB_HEADS is a closed list of exact
        // dotted call-heads.
        let src = "\
def test_x(mocker):
    mock.patch('a')
    monkeypatch.foobar()
    repo.save(1)
    assert True
";
        let parsed = parse_full(src);
        assert_eq!(parsed.test_functions[0].stubs_count, 0);
        assert_eq!(parsed.stubs_count, 0);
    }

    #[test]
    fn stubs_count_disjoint_from_mock_construction_count() {
        // A test body that mixes `Mock()` (constructor) with
        // `monkeypatch.setattr` (stub head) reports each on its own axis —
        // the two counts must not collide.
        let src = "\
def test_x(monkeypatch):
    m = Mock()
    monkeypatch.setattr(os, 'getcwd', lambda: '/x')
    assert True
";
        let parsed = parse_full(src);
        assert_eq!(parsed.mock_construction_count, 1);
        assert_eq!(parsed.stubs_count, 1);
        assert_eq!(parsed.test_functions[0].stubs_count, 1);
    }
}
