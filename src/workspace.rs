//! uv-workspace detection and member resolution.
//!
//! A "uv workspace" is a Python repo whose root `pyproject.toml` declares
//! `[tool.uv.workspace].members = [<glob>, ...]`. pycoati treats each
//! resolved member as its own project for audit purposes, while reusing
//! the workspace's single `.venv` / interpreter for any pytest work.
//!
//! Detection is intentionally narrow:
//!
//! - **Only** `[tool.uv.workspace]` triggers workspace mode. We do not
//!   subdir-scan or sniff anything else.
//! - Globs support a single trailing `*` segment only (`packages/*`).
//!   `**`, `?`, and char classes are rejected outright — the design
//!   docs (`cf/context.md`) capture this scope decision.
//! - The workspace root itself is **not** an implicit member; to audit
//!   the root, list `"."` in `members`.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::pyproject;

/// A resolved uv workspace: the canonical workspace root plus the
/// ordered list of canonical member-directory paths.
#[derive(Debug, Clone)]
pub struct Workspace {
    pub root: PathBuf,
    pub members: Vec<PathBuf>,
}

/// Detect a uv workspace rooted at `root`.
///
/// Returns:
/// - `Ok(None)` iff `<root>/pyproject.toml` does not declare
///   `[tool.uv.workspace]`. Callers fall back to single-project mode.
/// - `Ok(Some(Workspace { members: vec![], .. }))` when the workspace is
///   declared but has no members (either `members = []` or no `members`
///   key at all). Logs a stderr `warn!` for observability.
/// - `Ok(Some(Workspace { members, .. }))` with one entry per resolved,
///   on-disk, deduped member directory.
/// - `Err(...)` on hard failures: unsupported glob syntax, member path
///   escapes the workspace root, etc.
pub fn detect(root: &Path) -> Result<Option<Workspace>> {
    let Some(patterns) = pyproject::read_uv_workspace_members(root) else {
        return Ok(None);
    };

    let canonical_root = std::fs::canonicalize(root)
        .with_context(|| format!("failed to canonicalize workspace root {}", root.display()))?;

    if patterns.is_empty() {
        tracing::warn!(
            workspace_root = %canonical_root.display(),
            "[tool.uv.workspace] declared but no members; emitting empty workspace wrapper"
        );
        return Ok(Some(Workspace { root: canonical_root, members: Vec::new() }));
    }

    let mut members: Vec<PathBuf> = Vec::new();
    for pattern in &patterns {
        let expanded = expand_pattern(&canonical_root, pattern)?;
        for path in expanded {
            // Dedup preserving first-occurrence order. A small linear
            // scan is fine: member lists are tiny in practice.
            if !members.contains(&path) {
                members.push(path);
            }
        }
    }

    Ok(Some(Workspace { root: canonical_root, members }))
}

/// Expand a single workspace-members pattern against `root`.
///
/// Returns canonicalized absolute paths of every member directory the
/// pattern resolves to. Non-existent exact paths and non-directory
/// matches are skipped with a `warn!` — uv itself is lenient here and
/// we mirror that. Patterns that escape the workspace root, however,
/// are a hard error.
fn expand_pattern(root: &Path, pattern: &str) -> Result<Vec<PathBuf>> {
    reject_unsupported_glob(pattern)?;

    // A single trailing `*` segment is the only glob shape we support.
    // Anything else is treated as an exact path.
    if let Some((prefix, suffix)) = split_trailing_star(pattern) {
        return expand_star_pattern(root, prefix, suffix);
    }

    let candidate = root.join(pattern);
    let Ok(canon) = std::fs::canonicalize(&candidate) else {
        tracing::warn!(
            pattern = %pattern,
            path = %candidate.display(),
            "workspace member path does not exist; skipping"
        );
        return Ok(Vec::new());
    };
    validate_member_under_root(root, &canon, pattern)?;
    if !canon.is_dir() {
        tracing::warn!(
            pattern = %pattern,
            path = %canon.display(),
            "workspace member is not a directory; skipping"
        );
        return Ok(Vec::new());
    }
    Ok(vec![canon])
}

/// Hard-error on glob metacharacters we deliberately don't support.
/// `**` would imply recursive descent, `?` and `[` would imply a real
/// glob engine — both are out of scope for v1 and we want a loud
/// failure rather than silent surprise.
fn reject_unsupported_glob(pattern: &str) -> Result<()> {
    if pattern.contains("**") {
        anyhow::bail!(
            "unsupported glob in [tool.uv.workspace].members: `{pattern}` (recursive `**` is not supported)"
        );
    }
    if pattern.contains('?') {
        anyhow::bail!(
            "unsupported glob in [tool.uv.workspace].members: `{pattern}` (`?` wildcards are not supported)"
        );
    }
    if pattern.contains('[') {
        anyhow::bail!(
            "unsupported glob in [tool.uv.workspace].members: `{pattern}` (character classes are not supported)"
        );
    }
    // A bare `*` anywhere other than as the entire trailing segment is
    // unsupported. `packages/*` is fine; `packages*/foo`, `pat*tern/*`,
    // and `foo*` are not — they would silently produce zero members
    // because `read_dir` is called on a literal prefix, so reject them
    // loudly here instead.
    if pattern.contains('*') {
        let mut segments = pattern.split('/').peekable();
        while let Some(segment) = segments.next() {
            let is_trailing = segments.peek().is_none();
            if segment.contains('*') && !(is_trailing && segment == "*") {
                anyhow::bail!(
                    "unsupported glob in [tool.uv.workspace].members: `{pattern}` (only a single trailing `*` segment is supported)"
                );
            }
        }
    }
    Ok(())
}

/// If `pattern` ends with `/*`, return `(prefix, "*")`. Returns `None`
/// otherwise.
fn split_trailing_star(pattern: &str) -> Option<(&str, &str)> {
    let suffix = pattern.rsplit('/').next()?;
    if suffix != "*" {
        return None;
    }
    let prefix_len = pattern.len() - suffix.len();
    // Strip the separating `/` if present.
    let prefix = if prefix_len > 0 && pattern.as_bytes()[prefix_len - 1] == b'/' {
        &pattern[..prefix_len - 1]
    } else {
        &pattern[..prefix_len]
    };
    Some((prefix, suffix))
}

/// Resolve a trailing-`*` pattern by listing the prefix directory and
/// keeping only the entries that are themselves directories. Non-dir
/// matches are skipped with a `warn!`; this matches uv's behaviour
/// when a glob accidentally selects a stray file in `packages/`.
fn expand_star_pattern(root: &Path, prefix: &str, _suffix: &str) -> Result<Vec<PathBuf>> {
    let prefix_dir = if prefix.is_empty() { root.to_path_buf() } else { root.join(prefix) };
    let read = match std::fs::read_dir(&prefix_dir) {
        Ok(r) => r,
        Err(err) => {
            tracing::warn!(
                prefix = %prefix_dir.display(),
                error = %err,
                "failed to list workspace-members prefix directory; skipping"
            );
            return Ok(Vec::new());
        }
    };

    let mut out: Vec<PathBuf> = Vec::new();
    for entry in read {
        let entry = entry
            .with_context(|| format!("failed to read entry under {}", prefix_dir.display()))?;
        let path = entry.path();
        let canon = match std::fs::canonicalize(&path) {
            Ok(p) => p,
            Err(err) => {
                tracing::warn!(
                    path = %path.display(),
                    error = %err,
                    "failed to canonicalize workspace-members entry; skipping"
                );
                continue;
            }
        };
        if !canon.is_dir() {
            tracing::warn!(
                path = %canon.display(),
                "workspace-members glob matched a non-directory; skipping"
            );
            continue;
        }
        validate_member_under_root(root, &canon, prefix)?;
        out.push(canon);
    }
    // Sort for a deterministic order — `read_dir` is filesystem-specific.
    out.sort();
    Ok(out)
}

/// Reject any resolved member path that escapes the workspace root.
/// A `..`-laden pattern like `"../other"` is a hard error: it indicates
/// a configuration the rest of pycoati cannot reason about safely.
fn validate_member_under_root(root: &Path, member: &Path, pattern: &str) -> Result<()> {
    if !member.starts_with(root) {
        anyhow::bail!(
            "workspace member `{pattern}` resolves to {} which is outside workspace root {}",
            member.display(),
            root.display()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

    use super::*;
    use std::fs;

    fn write_workspace_root(dir: &Path, members_literal: &str) {
        let body = format!(
            "[project]\nname = \"root\"\n\n[tool.uv.workspace]\nmembers = {members_literal}\n"
        );
        fs::write(dir.join("pyproject.toml"), body).expect("write root pyproject");
    }

    fn make_member(root: &Path, rel: &str) {
        let dir = root.join(rel);
        fs::create_dir_all(&dir).expect("mkdir member");
        fs::write(dir.join("pyproject.toml"), "[project]\nname = \"x\"\n")
            .expect("write member pyproject");
    }

    #[test]
    fn detect_returns_none_when_no_workspace_section() {
        let tmp = tempfile::tempdir().expect("tempdir");
        fs::write(tmp.path().join("pyproject.toml"), "[project]\nname = \"root\"\n")
            .expect("write");
        let result = detect(tmp.path()).expect("detect ok");
        assert!(result.is_none(), "no [tool.uv.workspace] means no workspace");
    }

    #[test]
    fn detect_returns_workspace_for_exact_path_member() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write_workspace_root(tmp.path(), "[\"pkg_a\"]");
        make_member(tmp.path(), "pkg_a");

        let ws = detect(tmp.path()).expect("detect ok").expect("workspace present");
        assert_eq!(ws.members.len(), 1);
        let canonical_root = std::fs::canonicalize(tmp.path()).unwrap();
        assert_eq!(ws.root, canonical_root);
        assert_eq!(ws.members[0], canonical_root.join("pkg_a"));
    }

    #[test]
    fn detect_expands_star_pattern_to_two_members() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write_workspace_root(tmp.path(), "[\"pkgs/*\"]");
        make_member(tmp.path(), "pkgs/a");
        make_member(tmp.path(), "pkgs/b");

        let ws = detect(tmp.path()).expect("detect ok").expect("workspace present");
        assert_eq!(ws.members.len(), 2);
        let canonical_root = std::fs::canonicalize(tmp.path()).unwrap();
        let names: Vec<String> = ws
            .members
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert!(names.contains(&"a".to_string()));
        assert!(names.contains(&"b".to_string()));
        for m in &ws.members {
            assert!(m.starts_with(&canonical_root));
        }
    }

    #[test]
    fn detect_skips_missing_exact_path_member() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write_workspace_root(tmp.path(), "[\"pkg_a\", \"nope\"]");
        make_member(tmp.path(), "pkg_a");

        let ws = detect(tmp.path()).expect("detect ok").expect("workspace present");
        // Missing member is skipped lenient-style — workspace still has the one.
        assert_eq!(ws.members.len(), 1);
    }

    #[test]
    fn detect_skips_file_matches_under_star_glob() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write_workspace_root(tmp.path(), "[\"pkgs/*\"]");
        make_member(tmp.path(), "pkgs/a");
        // A stray file alongside a member must NOT show up as a member.
        fs::create_dir_all(tmp.path().join("pkgs")).expect("mkdir pkgs");
        fs::write(tmp.path().join("pkgs/stray.txt"), b"not a package").expect("write stray");

        let ws = detect(tmp.path()).expect("detect ok").expect("workspace present");
        assert_eq!(ws.members.len(), 1);
        assert!(ws.members[0].ends_with("a"));
    }

    #[test]
    fn detect_errors_when_member_path_escapes_root() {
        let tmp = tempfile::tempdir().expect("tempdir");
        // Make a sibling directory that the workspace pattern points into.
        let outside = tmp.path().join("outside");
        fs::create_dir_all(&outside).expect("mkdir outside");
        fs::write(outside.join("pyproject.toml"), "[project]\nname = \"x\"\n").expect("write");

        let ws_root = tmp.path().join("ws");
        fs::create_dir_all(&ws_root).expect("mkdir ws");
        write_workspace_root(&ws_root, "[\"../outside\"]");

        let err = detect(&ws_root).expect_err("must reject member outside workspace root");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("outside workspace root"),
            "error must mention 'outside workspace root', got: {msg}"
        );
    }

    #[test]
    fn detect_declared_but_empty_returns_empty_workspace() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write_workspace_root(tmp.path(), "[]");
        let ws = detect(tmp.path()).expect("detect ok").expect("workspace present");
        assert!(ws.members.is_empty(), "empty list yields zero members but still some(workspace)");
    }

    #[test]
    fn detect_rejects_star_in_non_trailing_segment() {
        // Regression: a `*` outside the trailing segment used to slip past
        // `reject_unsupported_glob` and silently expand to zero members
        // (the prefix passed to `read_dir` was treated literally). The
        // contract is "single trailing `*` only" — anything else must
        // fail loudly so the user notices the misconfiguration.
        let tmp = tempfile::tempdir().expect("tempdir");
        write_workspace_root(tmp.path(), "[\"pat*tern/*\"]");
        let err = detect(tmp.path())
            .expect_err("must reject `*` in non-trailing segment, not silently match zero");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("trailing"),
            "error must explain single-trailing-`*` rule, got: {msg}"
        );
    }

    #[test]
    fn detect_rejects_trailing_star_with_extra_chars() {
        // `packages*` (no slash, star not the whole segment) should also
        // be rejected loudly. The previous logic let `foo*` slip through
        // when there was no `/` because `rsplit` returned the whole
        // pattern; the rewritten check now segments on `/` first.
        let tmp = tempfile::tempdir().expect("tempdir");
        write_workspace_root(tmp.path(), "[\"packages*\"]");
        let err = detect(tmp.path()).expect_err("must reject bare `packages*`");
        let msg = format!("{err:#}");
        assert!(msg.contains("trailing"), "unexpected message: {msg}");
    }

    #[test]
    fn detect_rejects_double_star_glob() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write_workspace_root(tmp.path(), "[\"pkgs/**\"]");
        let err = detect(tmp.path()).expect_err("must reject **");
        let msg = format!("{err:#}");
        assert!(msg.contains("recursive `**`") || msg.contains("**"), "unexpected: {msg}");
    }
}
