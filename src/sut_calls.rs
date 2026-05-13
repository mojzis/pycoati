//! Cross-phase data carrier for resolving `called_names` against project
//! packages.
//!
//! Phase 1 (parser) constructs an [`ImportMap`] per Python source file from
//! `import_statement` and `import_from_statement` AST nodes. Phase 2 (sut
//! call resolution) consumes the map: it joins the head segment of each
//! `called_name` against `aliases` and `star_sources` to decide whether
//! the call resolves into a project package.
//!
//! No resolution logic lives here in Phase 1 — only the type that crosses
//! the phase boundary.

use std::collections::{BTreeMap, BTreeSet};

/// Per-file import map. Both fields are sorted (BTree-backed) so downstream
/// consumers see deterministic iteration order.
#[derive(Debug, Clone, Default)]
pub struct ImportMap {
    /// `local_name -> source_module`.
    ///
    /// - `import foo` → `foo` → `foo`.
    /// - `import foo.bar` → `foo` → `foo.bar` (the binding is the head
    ///   segment, the source is the full dotted name).
    /// - `import foo.bar as fb` → `fb` → `foo.bar`.
    /// - `from foo import bar` → `bar` → `foo`.
    /// - `from foo import bar as b` → `b` → `foo`.
    pub aliases: BTreeMap<String, String>,
    /// Modules that contributed a `from <module> import *`. The wildcard
    /// import binds an unknown set of names from the source module; Phase
    /// 2 uses this set to opportunistically classify unresolved bare-name
    /// calls as project-internal when one of the star sources is a project
    /// package.
    pub star_sources: BTreeSet<String>,
}
