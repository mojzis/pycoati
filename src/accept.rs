//! Accepted-findings baseline: reviewed, per-test suppressions.
//!
//! A periodic audit re-surfaces the same legitimate tests on every run. The
//! baseline is how a reviewer records that judgement once, with a reason, so
//! the next audit does not make them reconstruct it.
//!
//! ## What acceptance is, and is not
//!
//! Acceptance is an **analysis** decision, not an execution one. An accepted
//! test is still discovered, still parsed, still counted in
//! `files[].test_function_count`, and still run by pytest — so `suite.*`
//! (test count, runtime, coverage) is byte-identical with and without a
//! baseline. Nothing here touches pytest's arguments; this is not
//! deselection.
//!
//! Acceptance is also **not** a mute button on a test. It is scoped to one
//! (test identity, signal) pair. A test whose `zero_asserts` finding is
//! accepted still reaches the shortlist the moment it picks up a
//! `mock_overuse` hit, because that signal was never reviewed.
//!
//! ## The file
//!
//! [`DEFAULT_ACCEPT_FILENAME`] at the project root, or an explicit path via
//! `--accept-file`:
//!
//! ```toml
//! schema_version = "1"
//!
//! [[accept]]
//! test = "tests/test_packaging.py::test_wheel_installs"
//! signals = ["zero_asserts", "high_setup_ratio"]
//! reason = "assertions run in a child interpreter; the parent propagates
//!           failure via subprocess.run(check=True)"
//! reviewed = "2026-09-12"
//! fingerprint = "3f0a1c7d9b2e4a56"
//! ```
//!
//! `reason` is required and must be non-empty — an entry without one is a
//! parse error, not a warning. `signal = "<one>"` is accepted as a
//! single-signal shorthand for `signals`.
//!
//! ## Invalidation
//!
//! An entry stops suppressing — and is reported under `accepted.stale[]` —
//! when the test it names is gone ([`Status::UnknownTest`]), when the signal
//! no longer fires ([`Status::SignalNotActive`]), or when the test's content
//! changed since review ([`Status::ContentChanged`]). The last one is opt-in:
//! it only applies to entries that recorded a `fingerprint`. Every test
//! record carries its current [`fingerprint`] in the inventory so a reviewer
//! can copy the value in when they want that guarantee.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::TestRecord;

/// Baseline filename looked for at the project root when `--accept-file` is
/// not passed. A missing file is not an error — it means no acceptances.
pub const DEFAULT_ACCEPT_FILENAME: &str = ".pycoati-accept.toml";

/// The only `schema_version` a baseline may declare.
///
/// Required rather than defaulted: the file is a reviewed artifact with a
/// long lifetime, so it states its contract rather than relying on a default
/// that a future release would have to guess at.
pub const ACCEPT_SCHEMA_VERSION: &str = "1";

/// `setup_to_assertion_ratio` at or above which [`Signal::HighSetupRatio`] fires.
///
/// An alias, not a second copy: it *is* the sigmoid inflection point of the
/// suspicion score's setup term. Accepting `high_setup_ratio` has to mean
/// "accepting the thing that pushed this test up the ranking", so retuning
/// the score moves both together.
pub const HIGH_SETUP_RATIO: f64 = crate::suspicion::SETUP_RATIO_INFLECTION;

/// A finding that can be accepted.
///
/// A closed set on purpose: there is no wildcard, so accepting a test's
/// current finding never accepts a future one. The first two mirror
/// [`crate::SmellHit::category`] one-for-one; the last two are the
/// score-bearing signals that produce no smell hit of their own but do push
/// a test onto `top_suspicious.test_functions`.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Signal {
    /// Every assertion in the test targets the Mock API.
    MockOnlyAssertions,
    /// The test patches/stubs far more than it asserts.
    MockOveruse,
    /// The test body contains no effective assertion.
    ZeroAsserts,
    /// `setup_to_assertion_ratio >= ` [`HIGH_SETUP_RATIO`].
    HighSetupRatio,
}

impl Signal {
    /// Every signal, in the order they are listed in the guide.
    pub const ALL: [Self; 4] =
        [Self::MockOnlyAssertions, Self::MockOveruse, Self::ZeroAsserts, Self::HighSetupRatio];

    /// The name used in the baseline file and in the serialized inventory.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::MockOnlyAssertions => "mock_only_assertions",
            Self::MockOveruse => "mock_overuse",
            Self::ZeroAsserts => "zero_asserts",
            Self::HighSetupRatio => "high_setup_ratio",
        }
    }

    /// Parse a signal name. `None` for anything outside [`Signal::ALL`] —
    /// callers turn that into an error naming the valid set.
    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|s| s.as_str() == name)
    }

    /// Comma-separated list of every valid name, for error messages.
    pub fn all_names() -> String {
        Self::ALL.map(Self::as_str).join(", ")
    }
}

impl std::fmt::Display for Signal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Signals currently firing on `test`.
///
/// This is the set an entry is matched against: an entry naming a signal
/// absent from this set is stale, and a test is only dropped from the
/// shortlist when this whole set is accepted.
pub fn active_signals(test: &TestRecord) -> BTreeSet<Signal> {
    let mut out = BTreeSet::new();
    for hit in &test.smell_hits {
        if let Some(signal) = Signal::parse(&hit.category) {
            out.insert(signal);
        }
    }
    if test.assertion_count == 0 {
        out.insert(Signal::ZeroAsserts);
    }
    if test.setup_to_assertion_ratio >= HIGH_SETUP_RATIO {
        out.insert(Signal::HighSetupRatio);
    }
    out
}

/// A parsed baseline file: where it came from, and one [`Entry`] per
/// (test, signal) pair it accepts.
#[derive(Debug, Clone)]
pub(crate) struct Baseline {
    pub path: PathBuf,
    pub entries: Vec<Entry>,
}

/// One reviewed acceptance, flattened to a single signal. A `[[accept]]`
/// block listing three signals produces three entries sharing one reason.
#[derive(Debug, Clone)]
pub(crate) struct Entry {
    /// Nodeid of the accepted test, as it appears in
    /// `test_functions[].nodeid`.
    pub test: String,
    pub signal: Signal,
    pub reason: String,
    /// Free-form review date/author note. Never interpreted.
    pub reviewed: Option<String>,
    /// [`fingerprint`] of the test at review time. When present and no longer
    /// matching, the acceptance lapses until the reviewer looks again.
    pub fingerprint: Option<String>,
}

/// Why an entry is not suppressing anything.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum Status {
    /// No test in the inventory carries the entry's nodeid.
    UnknownTest,
    /// The test is there, but the accepted signal no longer fires.
    SignalNotActive,
    /// The test's content changed since the recorded fingerprint.
    ContentChanged,
}

impl Status {
    const fn as_str(self) -> &'static str {
        match self {
            Self::UnknownTest => "unknown_test",
            Self::SignalNotActive => "signal_not_active",
            Self::ContentChanged => "content_changed",
        }
    }
}

impl std::fmt::Display for Status {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The serialized `accepted` block of the inventory.
///
/// Findings live here and *only* here — `test_functions[].smell_hits` and
/// every count stay raw, so the inventory never loses evidence to a
/// suppression.
#[derive(Debug, Clone, Serialize)]
pub struct Accepted {
    /// Baseline file in effect, or `null` when none was found or
    /// `--no-accept` was passed.
    pub path: Option<PathBuf>,
    /// Whether accepted findings were left in `top_suspicious`
    /// (`--include-accepted`).
    pub included_in_shortlist: bool,
    /// Entries that matched a live finding.
    pub findings: Vec<AcceptedFinding>,
    /// Entries that matched nothing, with why.
    pub stale: Vec<StaleAcceptance>,
}

impl Accepted {
    /// The block emitted when no baseline is in effect.
    pub(crate) fn none(included_in_shortlist: bool) -> Self {
        Self { path: None, included_in_shortlist, findings: Vec::new(), stale: Vec::new() }
    }
}

/// One live acceptance and the reason recorded for it.
#[derive(Debug, Clone, Serialize)]
pub struct AcceptedFinding {
    pub test: String,
    pub signal: String,
    pub reason: String,
    pub reviewed: Option<String>,
    pub fingerprint: Option<String>,
}

/// One acceptance that no longer applies.
#[derive(Debug, Clone, Serialize)]
pub struct StaleAcceptance {
    pub test: String,
    pub signal: String,
    pub reason: String,
    /// One of [`Status::as_str`].
    pub status: String,
    /// Human-readable explanation of the mismatch. Display it; do not parse it.
    pub detail: String,
}

/// How one run treats the accepted-findings baseline.
///
/// [`Default`] is the no-flags behaviour: discover the project's own
/// [`DEFAULT_ACCEPT_FILENAME`] if it exists, and keep accepted findings out
/// of the shortlist.
#[derive(Debug, Clone, Default)]
pub struct AcceptOptions {
    /// Explicit baseline path (`--accept-file`). A path that does not exist
    /// is an error — the user asked for a specific file.
    pub file: Option<PathBuf>,
    /// `--no-accept`: run as if no baseline existed. Nothing is read, and
    /// `accepted.path` serializes as `null`.
    pub disabled: bool,
    /// `--include-accepted`: keep accepted findings on
    /// `top_suspicious.test_functions`. The full-audit view.
    pub include_accepted: bool,
}

impl AcceptOptions {
    /// Resolve the baseline for a project-root scan: the explicit
    /// `--accept-file`, else [`DEFAULT_ACCEPT_FILENAME`] in `project_root`,
    /// else none.
    pub(crate) fn resolve(&self, project_root: &Path) -> Result<Option<Baseline>> {
        self.resolve_with(|| discover(project_root))
    }

    /// Resolve the baseline for a single-file scan.
    ///
    /// Walks up from the file's directory instead of looking in exactly one
    /// place: single-file mode has no project root of its own — `start_dir`
    /// is just whichever directory happens to hold the file — so requiring
    /// the baseline to sit next to `tests/test_x.py` would put it somewhere
    /// nobody would think to look. The nearest ancestor wins, which makes
    /// the documented "at the project root" placement work for
    /// `pycoati tests/test_x.py`.
    pub(crate) fn resolve_from_file(&self, start_dir: &Path) -> Result<Option<Baseline>> {
        self.resolve_with(|| start_dir.ancestors().find_map(discover))
    }

    fn resolve_with(&self, find: impl FnOnce() -> Option<PathBuf>) -> Result<Option<Baseline>> {
        if self.disabled {
            return Ok(None);
        }
        let path = match self.file.as_deref() {
            Some(explicit) => {
                if !explicit.is_file() {
                    anyhow::bail!("accept file not found: {}", explicit.display());
                }
                explicit.to_path_buf()
            }
            None => match find() {
                Some(found) => found,
                None => return Ok(None),
            },
        };
        load(&path).map(Some)
    }
}

/// Look for the default baseline next to the project root. Returns `None`
/// when the file is absent — that is the common case and not an error.
pub(crate) fn discover(project_root: &Path) -> Option<PathBuf> {
    let candidate = project_root.join(DEFAULT_ACCEPT_FILENAME);
    candidate.is_file().then_some(candidate)
}

/// Read and validate a baseline file. Every failure is hard: a baseline the
/// reviewer cannot trust is worse than no baseline.
pub(crate) fn load(path: &Path) -> Result<Baseline> {
    let contents = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read accept file {}", path.display()))?;
    parse(&contents, path)
}

/// TOML shape of the baseline file, before validation.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawBaseline {
    schema_version: Option<String>,
    #[serde(default)]
    accept: Vec<RawEntry>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawEntry {
    test: Option<String>,
    signal: Option<String>,
    signals: Option<Vec<String>>,
    reason: Option<String>,
    reviewed: Option<String>,
    fingerprint: Option<String>,
}

/// Parse baseline file contents. `path` is used for error messages only.
pub(crate) fn parse(contents: &str, path: &Path) -> Result<Baseline> {
    let raw: RawBaseline = toml::from_str(contents)
        .with_context(|| format!("failed to parse accept file {}", path.display()))?;

    let declared = raw.schema_version.as_deref().unwrap_or_default();
    if declared != ACCEPT_SCHEMA_VERSION {
        let found = if declared.is_empty() {
            "no schema_version key".to_string()
        } else {
            format!("{declared:?}")
        };
        anyhow::bail!(
            "{}: expected schema_version = \"{ACCEPT_SCHEMA_VERSION}\", found {found}",
            path.display()
        );
    }

    let mut entries: Vec<Entry> = Vec::new();
    let mut seen: BTreeSet<(String, Signal)> = BTreeSet::new();
    for (index, raw_entry) in raw.accept.iter().enumerate() {
        // 1-indexed so the number matches "the third [[accept]] block".
        let position = index + 1;
        let test = required(raw_entry.test.as_deref(), "test", path, position)?;
        let reason = required(raw_entry.reason.as_deref(), "reason", path, position)?;

        let names = signal_names(raw_entry, path, position, &test)?;
        for name in names {
            let Some(signal) = Signal::parse(&name) else {
                anyhow::bail!(
                    "{}: accept entry {position} ({test}) names unknown signal {name:?}; valid signals are: {}",
                    path.display(),
                    Signal::all_names()
                );
            };
            if !seen.insert((test.clone(), signal)) {
                anyhow::bail!(
                    "{}: accept entry {position} repeats an acceptance already recorded for {test} / {name}; \
                     keep one entry per test and signal",
                    path.display()
                );
            }
            entries.push(Entry {
                test: test.clone(),
                signal,
                reason: reason.clone(),
                reviewed: raw_entry.reviewed.clone(),
                fingerprint: raw_entry.fingerprint.clone(),
            });
        }
    }

    Ok(Baseline { path: path.to_path_buf(), entries })
}

/// Pull a required, non-blank string field off a raw entry.
fn required(value: Option<&str>, field: &str, path: &Path, position: usize) -> Result<String> {
    match value.map(str::trim) {
        Some(v) if !v.is_empty() => Ok(v.to_string()),
        Some(_) => anyhow::bail!(
            "{}: accept entry {position} has an empty `{field}`; every acceptance must record one",
            path.display()
        ),
        None => anyhow::bail!(
            "{}: accept entry {position} is missing `{field}`; every acceptance must record one",
            path.display()
        ),
    }
}

/// Resolve the `signal` / `signals` pair into a non-empty list of names.
/// Exactly one of the two keys must be present.
fn signal_names(raw: &RawEntry, path: &Path, position: usize, test: &str) -> Result<Vec<String>> {
    match (raw.signal.as_deref(), raw.signals.as_deref()) {
        (Some(_), Some(_)) => anyhow::bail!(
            "{}: accept entry {position} ({test}) sets both `signal` and `signals`; use one or the other",
            path.display()
        ),
        (Some(one), None) => Ok(vec![one.trim().to_string()]),
        (None, Some(many)) if !many.is_empty() => {
            Ok(many.iter().map(|s| s.trim().to_string()).collect())
        }
        _ => anyhow::bail!(
            "{}: accept entry {position} ({test}) must name a signal via `signal` or a non-empty `signals`; \
             valid signals are: {}",
            path.display(),
            Signal::all_names()
        ),
    }
}

/// Match a baseline against the current inventory.
///
/// Mutates `tests`: every test with a live acceptance gets the signal name
/// pushed onto its `accepted_signals`. Returns the serialized `accepted`
/// block, and warns once per stale entry so a reviewer notices without
/// reading the JSON.
pub(crate) fn apply(
    tests: &mut [TestRecord],
    baseline: Option<Baseline>,
    include_accepted: bool,
) -> Accepted {
    let Some(baseline) = baseline else {
        return Accepted::none(include_accepted);
    };

    // Index once: a baseline with many entries against a large suite would
    // otherwise be a linear scan per entry.
    let positions: std::collections::BTreeMap<&str, usize> =
        tests.iter().enumerate().map(|(i, t)| (t.nodeid.as_str(), i)).collect();
    // Collected as owned indices so the borrow of `positions` (which borrows
    // `tests`) ends before the mutable pass below.
    let resolved: Vec<Option<usize>> =
        baseline.entries.iter().map(|e| positions.get(e.test.as_str()).copied()).collect();

    let mut findings: Vec<AcceptedFinding> = Vec::new();
    let mut stale: Vec<StaleAcceptance> = Vec::new();
    let mut touched: Vec<usize> = Vec::new();

    for (entry, position) in baseline.entries.into_iter().zip(resolved) {
        let Some(position) = position else {
            stale.push(stale_entry(
                entry,
                Status::UnknownTest,
                "no test function with this nodeid in the current inventory".to_string(),
            ));
            continue;
        };
        let test = &mut tests[position];

        // Content check first: a test that changed since review must be
        // looked at again whatever its signals currently say.
        if let Some(recorded) = entry.fingerprint.as_deref() {
            let current = test.fingerprint.as_deref();
            if current != Some(recorded) {
                let detail = current.map_or_else(
                    || {
                        format!(
                            "this test has no fingerprint in this run, so the recorded {recorded} \
                         cannot be confirmed; re-review and update the entry"
                        )
                    },
                    |current| {
                        format!(
                            "test content changed since review (recorded {recorded}, current \
                         {current}); re-review and update the entry"
                        )
                    },
                );
                stale.push(stale_entry(entry, Status::ContentChanged, detail));
                continue;
            }
        }

        if !active_signals(test).contains(&entry.signal) {
            let detail = format!("`{}` does not fire on this test in this run", entry.signal);
            stale.push(stale_entry(entry, Status::SignalNotActive, detail));
            continue;
        }

        test.accepted_signals.push(entry.signal.to_string());
        touched.push(position);
        findings.push(AcceptedFinding {
            test: entry.test,
            signal: entry.signal.to_string(),
            reason: entry.reason,
            reviewed: entry.reviewed,
            fingerprint: entry.fingerprint,
        });
    }

    // Sorted so `accepted_signals` is deterministic regardless of the order
    // entries appear in the file. No dedup: `parse` rejects a repeated
    // (test, signal) pair outright, so there is nothing to collapse.
    touched.sort_unstable();
    touched.dedup();
    for position in touched {
        tests[position].accepted_signals.sort();
    }

    for s in &stale {
        tracing::warn!(
            accept_file = %baseline.path.display(),
            test = %s.test,
            signal = %s.signal,
            status = %s.status,
            "stale acceptance; the finding is actionable again"
        );
    }

    Accepted { path: Some(baseline.path), included_in_shortlist: include_accepted, findings, stale }
}

fn stale_entry(entry: Entry, status: Status, detail: String) -> StaleAcceptance {
    StaleAcceptance {
        test: entry.test,
        signal: entry.signal.to_string(),
        reason: entry.reason,
        status: status.to_string(),
        detail,
    }
}

/// Content fingerprint of one test function's source text.
///
/// FNV-1a over the source with trailing whitespace stripped from each line
/// and blank lines dropped, rendered as 16 lowercase hex digits. Reflowing
/// whitespace therefore does not invalidate an acceptance, but any change to
/// what the test says — including a comment — does.
///
/// FNV-1a rather than a cryptographic digest: this detects an edit made by a
/// colleague, not one made by an adversary, and it keeps the dependency list
/// unchanged. The algorithm is spelled out here rather than taken from
/// `DefaultHasher`, whose output is explicitly not stable across Rust
/// releases — a fingerprint that changed on a toolchain bump would lapse
/// every acceptance in the file.
pub fn fingerprint(source: &str) -> String {
    const OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;

    let mut hash = OFFSET_BASIS;
    for line in source.lines() {
        let trimmed = line.trim_end();
        if trimmed.is_empty() {
            continue;
        }
        for byte in trimmed.bytes() {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(PRIME);
        }
        // Separator, so `["ab", "c"]` and `["a", "bc"]` differ.
        hash ^= u64::from(b'\n');
        hash = hash.wrapping_mul(PRIME);
    }
    format!("{hash:016x}")
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::SmellHit;
    use std::path::PathBuf;

    fn baseline_path() -> PathBuf {
        PathBuf::from(".pycoati-accept.toml")
    }

    fn make_test(nodeid: &str) -> TestRecord {
        TestRecord {
            nodeid: nodeid.to_string(),
            file: PathBuf::from("tests/test_x.py"),
            line: 1,
            assertion_count: 1,
            only_asserts_on_mock: false,
            patch_decorator_count: 0,
            stubs_count: 0,
            setup_to_assertion_ratio: 0.0,
            called_names: Vec::new(),
            smell_hits: Vec::new(),
            suspicion_score: 0.0,
            fingerprint: Some(fingerprint("def test_a():\n    assert 1\n")),
            accepted_signals: Vec::new(),
        }
    }

    fn smell(category: &str) -> SmellHit {
        SmellHit { category: category.to_string(), test: None, line: 1, evidence: String::new() }
    }

    fn parse_ok(contents: &str) -> Baseline {
        parse(contents, &baseline_path()).expect("baseline should parse")
    }

    fn parse_err(contents: &str) -> String {
        let err = parse(contents, &baseline_path()).expect_err("baseline should be rejected");
        format!("{err:#}")
    }

    #[test]
    fn every_signal_round_trips_through_its_name() {
        for signal in Signal::ALL {
            assert_eq!(Signal::parse(signal.as_str()), Some(signal));
        }
        assert_eq!(Signal::parse("not_a_signal"), None);
    }

    #[test]
    fn active_signals_reads_smell_categories() {
        let mut t = make_test("a::t");
        t.smell_hits = vec![smell("mock_overuse"), smell("mock_only_assertions")];
        // Set equality, not containment: a spurious extra signal would widen
        // what one accept entry suppresses.
        assert_eq!(
            active_signals(&t),
            BTreeSet::from([Signal::MockOveruse, Signal::MockOnlyAssertions])
        );
    }

    #[test]
    fn every_smell_category_the_detector_emits_is_a_known_signal() {
        // `active_signals` matches by string, and `Signal::parse` returns
        // `None` for an unrecognised category — which it then drops in
        // silence. Renaming a category in `smells.rs` would compile clean,
        // pass every other test, and quietly stop matching acceptances.
        let mut t = make_test("a::t");
        t.only_asserts_on_mock = true;
        t.assertion_count = 1;
        t.patch_decorator_count = 4;
        let hits =
            crate::smells::derive_test_smells(&t, &crate::smells::MockSmellConfig::default());
        assert_eq!(hits.len(), 2, "fixture must fire both test-scope smells: {hits:?}");
        for hit in &hits {
            assert!(
                Signal::parse(&hit.category).is_some(),
                "smell category {:?} has no matching Signal; accepting it would be impossible",
                hit.category
            );
        }
    }

    #[test]
    fn high_setup_ratio_tracks_the_suspicion_score_inflection() {
        // The two constants must stay one number, or an accepted
        // `high_setup_ratio` would stop describing what ranked the test.
        assert!((HIGH_SETUP_RATIO - crate::suspicion::SETUP_RATIO_INFLECTION).abs() < f64::EPSILON);
    }

    #[test]
    fn active_signals_fires_zero_asserts_only_at_zero() {
        let mut t = make_test("a::t");
        t.assertion_count = 0;
        assert!(active_signals(&t).contains(&Signal::ZeroAsserts));
        t.assertion_count = 1;
        assert!(!active_signals(&t).contains(&Signal::ZeroAsserts));
    }

    #[test]
    fn active_signals_high_setup_ratio_is_inclusive_at_the_threshold() {
        let mut t = make_test("a::t");
        t.setup_to_assertion_ratio = HIGH_SETUP_RATIO;
        assert!(active_signals(&t).contains(&Signal::HighSetupRatio));
        t.setup_to_assertion_ratio = HIGH_SETUP_RATIO - 0.1;
        assert!(!active_signals(&t).contains(&Signal::HighSetupRatio));
    }

    #[test]
    fn parse_reads_a_multi_signal_entry() {
        let b = parse_ok(
            r#"
schema_version = "1"

[[accept]]
test = "tests/test_pkg.py::test_wheel"
signals = ["zero_asserts", "high_setup_ratio"]
reason = "assertions run in a child interpreter"
reviewed = "2026-09-12"
fingerprint = "0123456789abcdef"
"#,
        );
        assert_eq!(b.entries.len(), 2, "one block per signal");
        assert_eq!(b.entries[0].signal, Signal::ZeroAsserts);
        assert_eq!(b.entries[1].signal, Signal::HighSetupRatio);
        // The reason and provenance are shared by both flattened entries.
        for entry in &b.entries {
            assert_eq!(entry.test, "tests/test_pkg.py::test_wheel");
            assert_eq!(entry.reason, "assertions run in a child interpreter");
            assert_eq!(entry.reviewed.as_deref(), Some("2026-09-12"));
            assert_eq!(entry.fingerprint.as_deref(), Some("0123456789abcdef"));
        }
    }

    #[test]
    fn parse_accepts_the_single_signal_shorthand() {
        let b = parse_ok(
            "schema_version = \"1\"\n\n[[accept]]\ntest = \"a::t\"\nsignal = \"mock_overuse\"\nreason = \"boundary stubs\"\n",
        );
        assert_eq!(b.entries.len(), 1);
        assert_eq!(b.entries[0].signal, Signal::MockOveruse);
        assert_eq!(b.entries[0].reviewed, None);
        assert_eq!(b.entries[0].fingerprint, None);
    }

    #[test]
    fn parse_accepts_a_baseline_with_no_entries() {
        let b = parse_ok("schema_version = \"1\"\n");
        assert!(b.entries.is_empty());
        assert_eq!(b.path, baseline_path());
    }

    #[test]
    fn parse_rejects_a_missing_reason() {
        let err = parse_err(
            "schema_version = \"1\"\n\n[[accept]]\ntest = \"a::t\"\nsignal = \"zero_asserts\"\n",
        );
        assert!(err.contains("missing `reason`"), "error should name the field: {err}");
        assert!(err.contains("accept entry 1"), "error should locate the entry: {err}");
    }

    #[test]
    fn parse_rejects_a_blank_reason() {
        let err = parse_err(
            "schema_version = \"1\"\n\n[[accept]]\ntest = \"a::t\"\nsignal = \"zero_asserts\"\nreason = \"   \"\n",
        );
        assert!(err.contains("empty `reason`"), "error should name the field: {err}");
    }

    #[test]
    fn parse_rejects_a_missing_test() {
        let err = parse_err(
            "schema_version = \"1\"\n\n[[accept]]\nsignal = \"zero_asserts\"\nreason = \"r\"\n",
        );
        assert!(err.contains("missing `test`"), "{err}");
    }

    #[test]
    fn parse_rejects_an_unknown_signal_and_lists_the_valid_ones() {
        let err = parse_err(
            "schema_version = \"1\"\n\n[[accept]]\ntest = \"a::t\"\nsignal = \"slow\"\nreason = \"r\"\n",
        );
        assert!(err.contains("unknown signal \"slow\""), "{err}");
        assert!(err.contains("zero_asserts"), "error should list valid signals: {err}");
    }

    #[test]
    fn parse_rejects_an_entry_with_no_signal_at_all() {
        let err =
            parse_err("schema_version = \"1\"\n\n[[accept]]\ntest = \"a::t\"\nreason = \"r\"\n");
        assert!(err.contains("must name a signal"), "{err}");
    }

    #[test]
    fn parse_rejects_an_empty_signals_list() {
        let err = parse_err(
            "schema_version = \"1\"\n\n[[accept]]\ntest = \"a::t\"\nsignals = []\nreason = \"r\"\n",
        );
        assert!(err.contains("must name a signal"), "{err}");
    }

    #[test]
    fn parse_rejects_both_signal_and_signals() {
        let err = parse_err(
            "schema_version = \"1\"\n\n[[accept]]\ntest = \"a::t\"\nsignal = \"zero_asserts\"\nsignals = [\"mock_overuse\"]\nreason = \"r\"\n",
        );
        assert!(err.contains("both `signal` and `signals`"), "{err}");
    }

    #[test]
    fn parse_rejects_a_duplicate_test_signal_pair() {
        let err = parse_err(
            "schema_version = \"1\"\n\n[[accept]]\ntest = \"a::t\"\nsignal = \"zero_asserts\"\nreason = \"one\"\n\n[[accept]]\ntest = \"a::t\"\nsignal = \"zero_asserts\"\nreason = \"two\"\n",
        );
        assert!(err.contains("repeats an acceptance"), "{err}");
    }

    #[test]
    fn parse_allows_the_same_test_under_two_different_signals() {
        let b = parse_ok(
            "schema_version = \"1\"\n\n[[accept]]\ntest = \"a::t\"\nsignal = \"zero_asserts\"\nreason = \"one\"\n\n[[accept]]\ntest = \"a::t\"\nsignal = \"mock_overuse\"\nreason = \"two\"\n",
        );
        assert_eq!(b.entries.len(), 2);
    }

    #[test]
    fn parse_rejects_a_missing_schema_version() {
        let err =
            parse_err("[[accept]]\ntest = \"a::t\"\nsignal = \"zero_asserts\"\nreason = \"r\"\n");
        assert!(err.contains("no schema_version key"), "{err}");
    }

    #[test]
    fn parse_rejects_a_future_schema_version() {
        let err = parse_err("schema_version = \"2\"\n");
        assert!(err.contains("expected schema_version = \"1\""), "{err}");
        assert!(err.contains("found \"2\""), "{err}");
    }

    #[test]
    fn parse_rejects_an_unknown_key() {
        // A typo like `resaon = ...` must not silently produce an entry with
        // no reason — `deny_unknown_fields` is what makes that a parse error.
        let err = parse_err(
            "schema_version = \"1\"\n\n[[accept]]\ntest = \"a::t\"\nsignal = \"zero_asserts\"\nreason = \"r\"\nexpires = \"never\"\n",
        );
        assert!(err.contains("expires"), "{err}");
    }

    #[test]
    fn apply_without_a_baseline_accepts_nothing() {
        let mut tests = vec![make_test("a::t")];
        let accepted = apply(&mut tests, None, false);
        assert_eq!(accepted.path, None);
        assert!(accepted.findings.is_empty());
        assert!(accepted.stale.is_empty());
        assert!(tests[0].accepted_signals.is_empty());
    }

    #[test]
    fn apply_marks_a_live_acceptance_and_records_its_reason() {
        let mut t = make_test("a::t");
        t.assertion_count = 0;
        let mut tests = vec![t];
        let baseline = parse_ok(
            "schema_version = \"1\"\n\n[[accept]]\ntest = \"a::t\"\nsignal = \"zero_asserts\"\nreason = \"child interpreter\"\n",
        );

        let accepted = apply(&mut tests, Some(baseline), false);

        assert_eq!(accepted.findings.len(), 1);
        assert_eq!(accepted.findings[0].signal, "zero_asserts");
        assert_eq!(accepted.findings[0].reason, "child interpreter");
        assert!(accepted.stale.is_empty());
        assert_eq!(tests[0].accepted_signals, vec!["zero_asserts".to_string()]);
    }

    #[test]
    fn apply_leaves_raw_evidence_untouched() {
        let mut t = make_test("a::t");
        t.assertion_count = 0;
        t.smell_hits = vec![smell("mock_overuse")];
        t.suspicion_score = 0.42;
        let mut tests = vec![t];
        let baseline = parse_ok(
            "schema_version = \"1\"\n\n[[accept]]\ntest = \"a::t\"\nsignal = \"zero_asserts\"\nreason = \"r\"\n",
        );

        apply(&mut tests, Some(baseline), false);

        assert_eq!(tests[0].assertion_count, 0, "counts must survive acceptance");
        assert_eq!(tests[0].smell_hits.len(), 1, "smell hits must survive acceptance");
        assert!((tests[0].suspicion_score - 0.42).abs() < f64::EPSILON);
    }

    #[test]
    fn apply_reports_an_entry_for_a_test_that_is_gone() {
        let mut tests = vec![make_test("a::t")];
        let baseline = parse_ok(
            "schema_version = \"1\"\n\n[[accept]]\ntest = \"a::deleted\"\nsignal = \"zero_asserts\"\nreason = \"r\"\n",
        );

        let accepted = apply(&mut tests, Some(baseline), false);

        assert!(accepted.findings.is_empty());
        assert_eq!(accepted.stale.len(), 1);
        assert_eq!(accepted.stale[0].status, "unknown_test");
        assert_eq!(accepted.stale[0].test, "a::deleted");
    }

    #[test]
    fn apply_reports_an_entry_whose_signal_stopped_firing() {
        // The test now asserts, so `zero_asserts` no longer applies.
        let mut tests = vec![make_test("a::t")];
        let baseline = parse_ok(
            "schema_version = \"1\"\n\n[[accept]]\ntest = \"a::t\"\nsignal = \"zero_asserts\"\nreason = \"r\"\n",
        );

        let accepted = apply(&mut tests, Some(baseline), false);

        assert!(accepted.findings.is_empty());
        assert_eq!(accepted.stale.len(), 1);
        assert_eq!(accepted.stale[0].status, "signal_not_active");
        assert!(tests[0].accepted_signals.is_empty());
    }

    #[test]
    fn apply_lapses_a_pinned_acceptance_when_the_test_has_no_fingerprint() {
        // A null fingerprint (unreadable source) must not be treated as
        // "matches anything" — that is the failure the pin exists to stop.
        let mut t = make_test("a::t");
        t.assertion_count = 0;
        t.fingerprint = None;
        let mut tests = vec![t];
        let baseline = parse_ok(
            "schema_version = \"1\"\n\n[[accept]]\ntest = \"a::t\"\nsignal = \"zero_asserts\"\nreason = \"r\"\nfingerprint = \"0000000000000000\"\n",
        );

        let accepted = apply(&mut tests, Some(baseline), false);

        assert!(accepted.findings.is_empty());
        assert_eq!(accepted.stale[0].status, "content_changed");
        assert!(tests[0].accepted_signals.is_empty());
    }

    #[test]
    fn apply_lapses_an_acceptance_when_the_test_content_changed() {
        let mut t = make_test("a::t");
        t.assertion_count = 0;
        t.fingerprint = Some("ffffffffffffffff".to_string());
        let mut tests = vec![t];
        let baseline = parse_ok(
            "schema_version = \"1\"\n\n[[accept]]\ntest = \"a::t\"\nsignal = \"zero_asserts\"\nreason = \"r\"\nfingerprint = \"0000000000000000\"\n",
        );

        let accepted = apply(&mut tests, Some(baseline), false);

        assert!(accepted.findings.is_empty(), "a changed test must be re-reviewed");
        assert_eq!(accepted.stale[0].status, "content_changed");
        assert!(
            accepted.stale[0].detail.contains("ffffffffffffffff"),
            "detail should carry the current fingerprint so it can be copied in: {}",
            accepted.stale[0].detail
        );
        assert!(tests[0].accepted_signals.is_empty());
    }

    #[test]
    fn apply_keeps_an_acceptance_whose_fingerprint_still_matches() {
        let mut t = make_test("a::t");
        t.assertion_count = 0;
        t.fingerprint = Some("0000000000000000".to_string());
        let mut tests = vec![t];
        let baseline = parse_ok(
            "schema_version = \"1\"\n\n[[accept]]\ntest = \"a::t\"\nsignal = \"zero_asserts\"\nreason = \"r\"\nfingerprint = \"0000000000000000\"\n",
        );

        let accepted = apply(&mut tests, Some(baseline), false);

        assert_eq!(accepted.findings.len(), 1);
        assert!(accepted.stale.is_empty());
    }

    #[test]
    fn apply_accepts_one_signal_and_leaves_the_other_active() {
        let mut t = make_test("a::t");
        t.assertion_count = 0;
        t.smell_hits = vec![smell("mock_overuse")];
        let mut tests = vec![t];
        let baseline = parse_ok(
            "schema_version = \"1\"\n\n[[accept]]\ntest = \"a::t\"\nsignal = \"zero_asserts\"\nreason = \"r\"\n",
        );

        apply(&mut tests, Some(baseline), false);

        assert_eq!(tests[0].accepted_signals, vec!["zero_asserts".to_string()]);
        assert!(
            !tests[0].accepted_in_full(),
            "an unreviewed mock_overuse hit must keep the test on the shortlist"
        );
    }

    #[test]
    fn accepted_in_full_requires_every_active_signal() {
        let mut t = make_test("a::t");
        t.assertion_count = 0;
        t.setup_to_assertion_ratio = 20.0;
        let mut tests = vec![t];
        let baseline = parse_ok(
            "schema_version = \"1\"\n\n[[accept]]\ntest = \"a::t\"\nsignals = [\"zero_asserts\", \"high_setup_ratio\"]\nreason = \"r\"\n",
        );

        apply(&mut tests, Some(baseline), false);

        assert!(tests[0].accepted_in_full());
    }

    #[test]
    fn accepted_in_full_is_false_for_a_test_with_no_signals() {
        // Nothing was flagged, so nothing was accepted — such a test must not
        // be treated as suppressed.
        let t = make_test("a::t");
        assert!(active_signals(&t).is_empty());
        assert!(!t.accepted_in_full());
    }

    #[test]
    fn fingerprint_ignores_trailing_whitespace_and_blank_lines() {
        let a = "def test_a():\n    assert 1\n";
        let b = "def test_a():   \n\n    assert 1\t\n\n";
        assert_eq!(fingerprint(a), fingerprint(b));
    }

    #[test]
    fn fingerprint_changes_when_the_body_changes() {
        assert_ne!(
            fingerprint("def test_a():\n    assert 1\n"),
            fingerprint("def test_a():\n    assert 2\n")
        );
    }

    #[test]
    fn fingerprint_changes_when_a_comment_changes() {
        // A comment can carry the reason a test looks the way it does, so an
        // edit to one is an edit worth re-reviewing.
        assert_ne!(
            fingerprint("def test_a():\n    # boundary\n    assert 1\n"),
            fingerprint("def test_a():\n    # rewritten\n    assert 1\n")
        );
    }

    #[test]
    fn fingerprint_is_sixteen_hex_digits() {
        let fp = fingerprint("def test_a():\n    assert 1\n");
        assert_eq!(fp.len(), 16, "got {fp}");
        assert!(fp.chars().all(|c| c.is_ascii_hexdigit() && !c.is_uppercase()), "got {fp}");
    }

    #[test]
    fn fingerprint_of_empty_source_is_the_offset_basis() {
        // Guards the "no lines contributed" path against an accidental
        // panic or a differently-seeded hash.
        assert_eq!(fingerprint(""), "cbf29ce484222325");
    }

    #[test]
    fn fingerprint_separates_line_boundaries() {
        assert_ne!(fingerprint("ab\nc"), fingerprint("a\nbc"));
    }

    #[test]
    fn discover_finds_the_default_file_and_nothing_else() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert_eq!(discover(dir.path()), None);
        let path = dir.path().join(DEFAULT_ACCEPT_FILENAME);
        std::fs::write(&path, "schema_version = \"1\"\n").expect("write baseline");
        assert_eq!(discover(dir.path()), Some(path));
    }

    #[test]
    fn discover_ignores_a_directory_with_the_baseline_name() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir(dir.path().join(DEFAULT_ACCEPT_FILENAME)).expect("mkdir");
        assert_eq!(discover(dir.path()), None);
    }
}
