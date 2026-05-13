//! Resolve raw `called_names` against project packages and aggregate
//! call-site frequency.
//!
//! Phase 1 (parser) emits, for each test, a sorted+deduped list of dot-joined
//! head chains for every `call_expression` in the body. Phase 1 also produces
//! a per-file [`ImportMap`] from `import_statement` / `import_from_statement`
//! nodes (alias map + star-import sources).
//!
//! Phase 2 (this module):
//! - resolves each raw call name against the import map and the project's
//!   declared packages, keeping only project-internal names (canonicalised
//!   to their dotted form);
//! - aggregates the resolved names into [`crate::SutCalls`].

use std::collections::{BTreeMap, BTreeSet};

use crate::{SutCallEntry, SutCalls, TestRecord};

/// Per-file import map. Both fields are sorted (BTree-backed) so downstream
/// consumers see deterministic iteration order.
///
/// The struct lives in the crate-private `sut_calls` module — its `pub`
/// fields are reachable only from inside the crate. It is the
/// parser→aggregator hand-off contract; nothing outside the crate has a
/// reason to construct or inspect an `ImportMap` (the public output is
/// `SutCalls`).
#[derive(Debug, Clone, Default)]
pub struct ImportMap {
    /// `local_name -> canonical_full_dotted_name`.
    ///
    /// The value is what the local binding semantically points at:
    ///
    /// - `import foo` → `foo` → `foo`.
    /// - `import foo.bar` → `foo` → `foo.bar` (the binding is the head
    ///   segment, the canonical is the full dotted name).
    /// - `import foo.bar as fb` → `fb` → `foo.bar`.
    /// - `from foo import bar` → `bar` → `foo.bar`.
    /// - `from foo import bar as b` → `b` → `foo.bar` (the alias keys the
    ///   map; the originally imported name is the suffix of the canonical).
    /// - `from myproj.repository import Repository` → `Repository` →
    ///   `myproj.repository.Repository`.
    pub aliases: BTreeMap<String, String>,
    /// Modules that contributed a `from <module> import *`. The wildcard
    /// import binds an unknown set of names from the source module; Phase
    /// 2 uses this set to opportunistically classify unresolved bare-name
    /// calls as project-internal when one of the star sources is a project
    /// package.
    ///
    /// **Policy — deliberately over-inclusive:** a bare call like
    /// `compute()` in a file with `from myproj import *` is counted as
    /// `myproj.compute` even when `compute` is actually a stdlib builtin or
    /// a local helper. Resolving a star import correctly would require
    /// reading the source module's `__all__` (or executing it), which is
    /// out of scope for a static analyzer. False positives are intentional
    /// in v1; the alternative — silently dropping calls under a star import
    /// — is worse because it would hide genuine SUT calls. A future
    /// revision may flag these as "low-confidence" rather than dropping
    /// them.
    pub star_sources: BTreeSet<String>,
}

/// Resolve one raw `called_name` against an import map and project package
/// list. Returns `Some(canonical_dotted_name)` if the call resolves into
/// a project package, `None` otherwise.
///
/// A name is "project-internal" if:
/// - its head segment IS a project package name (direct hit), or
/// - its head segment resolves through the alias map to a module starting
///   with a project package name (aliased import), or
/// - its head segment is unresolved and the file has a star import from a
///   project package (the over-include path).
///
/// Canonical form: when resolved via the alias map, the head segment is
/// replaced by its full dotted source module. The trailing attribute (if
/// any) is preserved.
///
/// `self.*` names are not expected at this stage (the parser already filters
/// them) — but if one slips through, it never matches a project package and
/// is dropped.
pub fn resolve_called_name(
    raw: &str,
    imports: &ImportMap,
    project_packages: &BTreeSet<String>,
) -> Option<String> {
    if raw.is_empty() || raw == "self" || raw.starts_with("self.") {
        return None;
    }
    let (head, tail) = split_head(raw);

    // 1. Head IS a project package: keep as-is.
    if project_packages.contains(head) {
        return Some(raw.to_string());
    }

    // 2. Head resolves through the alias map.
    if let Some(source_module) = imports.aliases.get(head) {
        if head_under_project(source_module, project_packages) {
            // Canonical form replaces the head with the full source-module
            // dotted name. If the source_module IS the head (`import foo`
            // produced alias `foo -> foo`), this still produces the same
            // canonical form. The trailing attribute is preserved.
            let canonical = match tail {
                Some(rest) => format!("{source_module}.{rest}"),
                None => source_module.clone(),
            };
            return Some(canonical);
        }
        // Aliased, but not to a project package — third-party / stdlib.
        return None;
    }

    // 3. Star import from a project package: opportunistically include.
    for star_source in &imports.star_sources {
        if head_under_project(star_source, project_packages) {
            let canonical = format!("{star_source}.{raw}");
            return Some(canonical);
        }
    }

    None
}

/// Mutate each test record's `called_names` in place: replace the raw list
/// with the resolved+filtered list. Then aggregate across tests and return
/// the populated [`SutCalls`].
pub fn aggregate(
    tests: &mut [TestRecord],
    imports_per_file: &BTreeMap<std::path::PathBuf, ImportMap>,
    project_packages: &BTreeSet<String>,
) -> SutCalls {
    // First pass: resolve per test, writing back into the record.
    for t in tests.iter_mut() {
        let empty_map = ImportMap::default();
        let imports = imports_per_file.get(&t.file).unwrap_or(&empty_map);
        let mut resolved: BTreeSet<String> = BTreeSet::new();
        for raw in &t.called_names {
            if let Some(canonical) = resolve_called_name(raw, imports, project_packages) {
                resolved.insert(canonical);
            }
        }
        t.called_names = resolved.into_iter().collect();
    }

    // Second pass: aggregate.
    let mut by_name: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for t in tests.iter() {
        for name in &t.called_names {
            by_name.entry(name.clone()).or_default().insert(t.nodeid.clone());
        }
    }

    let mut entries: Vec<SutCallEntry> = by_name
        .into_iter()
        .map(|(name, nodeids)| {
            let mut sorted: Vec<String> = nodeids.into_iter().collect();
            sorted.sort();
            let count = sorted.len() as u64;
            SutCallEntry { name, test_function_count: count, test_nodeids: sorted }
        })
        .collect();
    // Stable by name (already true from BTreeMap iteration order).
    entries.sort_by(|a, b| a.name.cmp(&b.name));

    // Top called: descending by count, tiebreak by name ascending.
    let mut top: Vec<(String, u64)> =
        entries.iter().map(|e| (e.name.clone(), e.test_function_count)).collect();
    top.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    let top_called: Vec<String> = top.into_iter().take(20).map(|(name, _)| name).collect();

    SutCalls { by_name: entries, top_called }
}

/// Return the head segment of a dot-joined identifier chain — the substring
/// before the first `.`, or the whole string if there is no `.`.
///
/// Shared between the parser (when recording import bindings — the head of
/// `foo.bar` is the local name bound by `import foo.bar`) and the resolver
/// (when checking whether a call's head identifies a project package). One
/// helper avoids two slightly-different implementations drifting apart and
/// keeps the "we treat dotted chains as `head . tail`" rule in one place.
pub fn dotted_head(name: &str) -> &str {
    name.split_once('.').map_or(name, |(h, _)| h)
}

/// Split a dot-joined chain into `(head, optional_rest)`. Built on
/// [`dotted_head`] to keep the split semantics consistent.
fn split_head(raw: &str) -> (&str, Option<&str>) {
    match raw.split_once('.') {
        Some((h, rest)) => (h, Some(rest)),
        None => (raw, None),
    }
}

/// True iff the head segment of `module` is a project package.
fn head_under_project(module: &str, project_packages: &BTreeSet<String>) -> bool {
    project_packages.contains(dotted_head(module))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn pkgs<I, S>(iter: I) -> BTreeSet<String>
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        iter.into_iter().map(Into::into).collect()
    }

    fn make_record(nodeid: &str, called: &[&str]) -> TestRecord {
        TestRecord {
            nodeid: nodeid.to_string(),
            file: PathBuf::from("tests/test_x.py"),
            line: 1,
            assertion_count: 0,
            only_asserts_on_mock: false,
            patch_decorator_count: 0,
            setup_to_assertion_ratio: 0.0,
            called_names: called.iter().map(|s| (*s).to_string()).collect(),
            smell_hits: Vec::new(),
            suspicion_score: 0.0,
        }
    }

    // ---- ImportMap construction is in parser.rs; these tests cross-check
    // the Phase-1 invariant from the consumer's side.

    #[test]
    fn import_map_handles_plain_import() {
        let src = "import foo\n";
        let parsed =
            crate::parser::parse_python_file(src, &PathBuf::from("synthetic.py")).expect("parse");
        assert_eq!(parsed.import_map.aliases.get("foo"), Some(&"foo".to_string()));
    }

    #[test]
    fn import_map_handles_aliased_import() {
        let src = "import foo as f\n";
        let parsed =
            crate::parser::parse_python_file(src, &PathBuf::from("synthetic.py")).expect("parse");
        assert_eq!(parsed.import_map.aliases.get("f"), Some(&"foo".to_string()));
    }

    #[test]
    fn import_map_handles_from_import() {
        // The alias-map value is the canonical full dotted name of the
        // local binding (`source_module + . + imported_name`).
        let src = "from foo import bar\n";
        let parsed =
            crate::parser::parse_python_file(src, &PathBuf::from("synthetic.py")).expect("parse");
        assert_eq!(parsed.import_map.aliases.get("bar"), Some(&"foo.bar".to_string()));
    }

    #[test]
    fn import_map_handles_from_import_with_alias() {
        // For an aliased from-import the alias keys the map and the
        // originally imported name is the suffix of the canonical value.
        let src = "from foo import bar as b\n";
        let parsed =
            crate::parser::parse_python_file(src, &PathBuf::from("synthetic.py")).expect("parse");
        assert_eq!(parsed.import_map.aliases.get("b"), Some(&"foo.bar".to_string()));
    }

    #[test]
    fn import_map_handles_dotted_module_with_alias() {
        let src = "import foo.bar as fb\n";
        let parsed =
            crate::parser::parse_python_file(src, &PathBuf::from("synthetic.py")).expect("parse");
        assert_eq!(parsed.import_map.aliases.get("fb"), Some(&"foo.bar".to_string()));
    }

    #[test]
    fn import_map_records_star_imports() {
        let src = "from foo import *\n";
        let parsed =
            crate::parser::parse_python_file(src, &PathBuf::from("synthetic.py")).expect("parse");
        assert!(parsed.import_map.star_sources.contains("foo"));
        assert!(parsed.import_map.aliases.is_empty());
    }

    // ---- resolve_called_name --------------------------------------------

    #[test]
    fn resolve_keeps_project_internal_via_alias() {
        // `from myproj import Repo` produces alias `Repo -> myproj.Repo`
        // (canonical form). `Repo.save` resolves to `myproj.Repo.save`.
        let mut imports = ImportMap::default();
        imports.aliases.insert("Repo".to_string(), "myproj.Repo".to_string());
        let resolved = resolve_called_name("Repo.save", &imports, &pkgs(["myproj"]));
        assert_eq!(resolved, Some("myproj.Repo.save".to_string()));
    }

    #[test]
    fn resolve_keeps_aliased_from_dotted_module() {
        // `from myproj.repository import Repository` produces alias
        // `Repository -> myproj.repository.Repository`.
        let mut imports = ImportMap::default();
        imports
            .aliases
            .insert("Repository".to_string(), "myproj.repository.Repository".to_string());
        let resolved = resolve_called_name("Repository.save", &imports, &pkgs(["myproj"]));
        assert_eq!(resolved, Some("myproj.repository.Repository.save".to_string()));
    }

    #[test]
    fn resolve_drops_stdlib() {
        // `import uuid` → alias `uuid -> uuid`. Calling `uuid.uuid4` resolves
        // canonically to `uuid.uuid4`; its head `uuid` is not a project
        // package → dropped.
        let mut imports = ImportMap::default();
        imports.aliases.insert("uuid".to_string(), "uuid".to_string());
        let resolved = resolve_called_name("uuid.uuid4", &imports, &pkgs(["myproj"]));
        assert_eq!(resolved, None);
    }

    #[test]
    fn resolve_drops_third_party() {
        // `from pytest import fixture` → alias `fixture -> pytest.fixture`.
        // Canonical head `pytest` is not project → dropped.
        let mut imports = ImportMap::default();
        imports.aliases.insert("fixture".to_string(), "pytest.fixture".to_string());
        let resolved = resolve_called_name("fixture", &imports, &pkgs(["myproj"]));
        assert_eq!(resolved, None);
    }

    #[test]
    fn resolve_drops_self_calls() {
        // Phase 1 filters these — confirm we don't reintroduce them.
        let imports = ImportMap::default();
        assert_eq!(resolve_called_name("self.foo", &imports, &pkgs(["myproj"])), None);
        assert_eq!(resolve_called_name("self", &imports, &pkgs(["myproj"])), None);
    }

    #[test]
    fn star_import_keeps_unresolved_bare_names_under_project_package() {
        let mut imports = ImportMap::default();
        imports.star_sources.insert("myproj".to_string());
        let resolved = resolve_called_name("compute", &imports, &pkgs(["myproj"]));
        assert_eq!(resolved, Some("myproj.compute".to_string()));
    }

    #[test]
    fn resolve_keeps_name_with_head_equal_to_project_package() {
        let imports = ImportMap::default();
        let resolved = resolve_called_name("myproj.greet", &imports, &pkgs(["myproj"]));
        assert_eq!(resolved, Some("myproj.greet".to_string()));
    }

    #[test]
    fn resolve_drops_unresolved_when_no_star_import() {
        let imports = ImportMap::default();
        let resolved = resolve_called_name("compute", &imports, &pkgs(["myproj"]));
        assert_eq!(resolved, None);
    }

    // ---- aggregate -------------------------------------------------------

    #[test]
    fn aggregator_dedupes_test_nodeids() {
        // Same raw name appearing twice in a single test's called_names
        // resolves to the same canonical and must produce a single by_name
        // entry with `test_function_count = 1` and one nodeid.
        let mut imports = ImportMap::default();
        imports
            .aliases
            .insert("Repository".to_string(), "myproj.repository.Repository".to_string());
        let mut tests =
            vec![make_record("tests/test_x.py::test_a", &["Repository.save", "Repository.save"])];
        let mut imports_per_file = BTreeMap::new();
        imports_per_file.insert(PathBuf::from("tests/test_x.py"), imports);
        let sc = aggregate(&mut tests, &imports_per_file, &pkgs(["myproj"]));
        assert_eq!(sc.by_name.len(), 1);
        assert_eq!(sc.by_name[0].name, "myproj.repository.Repository.save");
        assert_eq!(sc.by_name[0].test_function_count, 1);
        assert_eq!(sc.by_name[0].test_nodeids, vec!["tests/test_x.py::test_a".to_string()]);
    }

    #[test]
    fn aggregator_top_called_sorted_desc_then_name_asc() {
        // Counts: a=2, b=2, c=3 → top_called = ["c", "a", "b"].
        let mut imports = ImportMap::default();
        imports.star_sources.insert("myproj".to_string());
        let mut tests = vec![
            make_record("tests/test_x.py::t1", &["a", "b", "c"]),
            make_record("tests/test_x.py::t2", &["a", "b", "c"]),
            make_record("tests/test_x.py::t3", &["c"]),
        ];
        let mut imports_per_file = BTreeMap::new();
        imports_per_file.insert(PathBuf::from("tests/test_x.py"), imports);
        let sc = aggregate(&mut tests, &imports_per_file, &pkgs(["myproj"]));
        assert_eq!(
            sc.top_called,
            vec!["myproj.c".to_string(), "myproj.a".to_string(), "myproj.b".to_string()]
        );
    }

    #[test]
    fn aggregator_top_called_caps_at_twenty() {
        // 25 unique called names all with `test_function_count = 1` — the
        // tiebreaker (name ascending) picks the lexicographically-smallest
        // 20. Names are zero-padded so the lex order matches numeric.
        let mut imports = ImportMap::default();
        imports.star_sources.insert("myproj".to_string());
        let names: Vec<String> = (0..25).map(|i| format!("n{i:02}")).collect();
        let calls: Vec<&str> = names.iter().map(String::as_str).collect();
        let mut tests = vec![make_record("tests/test_x.py::t1", &calls)];
        let mut imports_per_file = BTreeMap::new();
        imports_per_file.insert(PathBuf::from("tests/test_x.py"), imports);
        let sc = aggregate(&mut tests, &imports_per_file, &pkgs(["myproj"]));
        assert_eq!(sc.top_called.len(), 20);
        assert_eq!(sc.top_called.first().map(String::as_str), Some("myproj.n00"));
        assert_eq!(sc.top_called.last().map(String::as_str), Some("myproj.n19"));
    }

    #[test]
    fn aggregator_writes_resolved_names_back_to_record() {
        let mut imports = ImportMap::default();
        imports
            .aliases
            .insert("Repository".to_string(), "myproj.repository.Repository".to_string());
        let mut tests = vec![make_record("tests/test_x.py::t", &["Repository.save", "uuid.uuid4"])];
        let mut imports_per_file = BTreeMap::new();
        imports_per_file.insert(PathBuf::from("tests/test_x.py"), imports);
        let _ = aggregate(&mut tests, &imports_per_file, &pkgs(["myproj"]));
        assert_eq!(tests[0].called_names, vec!["myproj.repository.Repository.save".to_string()]);
    }
}
