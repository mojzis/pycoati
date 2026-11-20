//! `pycoati plan`: how to spread a monorepo's pytest runs over parallel lanes.
//!
//! Given the subprojects a caller wants to run, the plan answers two
//! questions: how many pytest processes are worth running side by side, and
//! which subprojects go in which one. It runs nothing itself.
//!
//! The model is deliberately small:
//!
//! - Each subproject is one pytest invocation with one **cost**: its measured
//!   runtime when a `--timings` inventory supplies one, an estimate from the
//!   measured seconds-per-test rate when only some members were measured, and
//!   its test count when nothing was measured at all.
//!   A seconds cost also carries a fixed start-up allowance per invocation
//!   (`--startup-seconds`): pytest's reported runtime leaves out interpreter
//!   and plugin start-up, which is most of the cost of a small suite.
//! - A **lane** is one parallel slot. The subprojects in a lane run one after
//!   another, so a lane costs the sum of its members and the whole plan takes
//!   as long as its most expensive lane (the wall cost).
//! - For every lane count from one to `--max-jobs` the members are packed to
//!   minimise the wall cost ([`partition`]). The recommendation is the
//!   *smallest* lane count whose wall cost is within `--tolerance` percent of
//!   the best one found: extra lanes that buy nothing are never recommended.
//!
//! Subprojects are never merged into a single pytest process. Two members'
//! `conftest.py` files, rootdirs and same-named test modules routinely clash
//! in one process, and no static check here could promise they do not.

use std::fmt::Write as _;
use std::num::NonZeroUsize;
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::{pyproject, workspace, AcceptOptions, MemberCwd};

/// Schema version of the [`RunPlan`] payload. Independent of the inventory's.
pub const PLAN_SCHEMA_VERSION: &str = "1";

/// Default for [`PlanOptions::tolerance_pct`].
pub const DEFAULT_TOLERANCE_PCT: f64 = 10.0;

/// Default for [`PlanOptions::startup_seconds`]: roughly what an interpreter
/// plus pytest and a few plugins take to reach the first test.
pub const DEFAULT_STARTUP_SECONDS: f64 = 0.5;

/// Default for [`PlanOptions::pytest_cmd`].
pub const DEFAULT_PYTEST_CMD: &str = "python -m pytest";

/// Costs closer than this are treated as equal.
const EPS: f64 = 1e-9;

/// Cap on branch-and-bound nodes per lane count, so a pathological set of
/// costs degrades to the greedy packing instead of an unbounded search.
const SEARCH_NODE_BUDGET: usize = 50_000;

/// What the caller asked for.
#[derive(Debug, Clone)]
pub struct PlanOptions {
    /// Subprojects to run. In a uv workspace: member paths relative to the
    /// root, directory basenames, or `[project].name`s; empty means every
    /// member. Outside one: directories relative to the root (required).
    pub members: Vec<String>,
    /// Upper bound on parallel pytest runs.
    pub max_jobs: NonZeroUsize,
    /// How much slower than the best wall cost (in percent) a plan may be
    /// when that saves parallel runs. `0` demands the best.
    pub tolerance_pct: f64,
    /// Seconds added to every member's cost for process start-up, which
    /// pytest's own runtime figure excludes. Ignored when the plan falls
    /// back to test counts.
    pub startup_seconds: f64,
    /// Inventories (`pycoati <root> -o inventory.json`) to read measured
    /// `suite.runtime_seconds` from.
    pub timings: Vec<PathBuf>,
    /// Where the emitted commands are meant to run from.
    pub member_cwd: MemberCwd,
    /// Command prefix for the emitted commands; test paths are appended.
    pub pytest_cmd: String,
}

/// Unit every `cost` in a plan is expressed in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CostUnit {
    /// Wall-clock seconds, measured or estimated from measurements.
    Seconds,
    /// Test-function counts: the proxy used when nothing was measured.
    Tests,
}

/// Where one member's cost came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CostSource {
    /// `suite.runtime_seconds` from a timings inventory.
    Measured,
    /// Test count times the seconds-per-test rate of the measured members.
    Estimated,
    /// The test count itself.
    TestCount,
}

impl CostSource {
    /// The value this source serializes as.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Measured => "measured",
            Self::Estimated => "estimated",
            Self::TestCount => "test_count",
        }
    }
}

/// The plan payload. `docs/src/guide/plan.md` (shipped as [`crate::guide::PLAN`])
/// is the field reference; `tests/guide_schema_drift.rs` keeps the two in step.
#[derive(Debug, Clone, Serialize)]
pub struct RunPlan {
    /// [`PLAN_SCHEMA_VERSION`].
    pub schema_version: String,
    /// Canonical monorepo root; every `path` and `cwd` is relative to it.
    pub root: PathBuf,
    /// Unit of every cost in the plan.
    pub cost_unit: CostUnit,
    /// Lane cap the plan was computed under.
    pub max_jobs: usize,
    /// Tolerance the recommendation was chosen with.
    pub tolerance_pct: f64,
    /// The per-run allowance folded into every cost; `None` when the plan is
    /// in test counts and no allowance applies.
    pub startup_seconds: Option<f64>,
    /// Subprojects that will run, in the order asked for.
    pub members: Vec<PlanMember>,
    /// Selected subprojects with nothing to run.
    pub skipped: Vec<SkippedMember>,
    /// Sum of all member costs: one lane, no parallelism.
    pub serial_cost: f64,
    /// The most expensive member.
    pub bottleneck: Bottleneck,
    /// The best packing's wall cost per lane count, one lane upwards.
    pub options: Vec<PlanOption>,
    /// The lane count to use and what runs in each lane.
    pub recommended: Recommended,
}

/// One subproject that will be run.
#[derive(Debug, Clone, Serialize)]
pub struct PlanMember {
    /// `[project].name`, else the directory basename.
    pub name: String,
    /// Directory relative to the root, `/`-separated; `.` for the root.
    pub path: String,
    /// Collected count from a timings inventory, else the static count.
    pub test_count: u64,
    /// Runtime as pytest reported it, without the start-up allowance.
    pub runtime_seconds: Option<f64>,
    /// What the packing used, in the plan's [`CostUnit`].
    pub cost: f64,
    /// Where `cost` came from.
    pub cost_source: CostSource,
}

/// A selected subproject with nothing to run.
#[derive(Debug, Clone, Serialize)]
pub struct SkippedMember {
    /// As [`PlanMember::name`].
    pub name: String,
    /// As [`PlanMember::path`].
    pub path: String,
    /// `no tests directory` or `no tests`.
    pub reason: String,
}

/// The most expensive member: no number of lanes finishes sooner than it.
#[derive(Debug, Clone, Serialize)]
pub struct Bottleneck {
    /// The member's [`PlanMember::name`].
    pub member: String,
    /// The member's [`PlanMember::path`].
    pub path: String,
    /// The member's cost.
    pub cost: f64,
    /// True when the recommended plan is already down to this member's cost,
    /// so only speeding up the member itself can go faster.
    pub limits_wall_time: bool,
}

/// The best packing found for one lane count.
#[derive(Debug, Clone, Serialize)]
pub struct PlanOption {
    /// Number of lanes, i.e. pytest processes running at once.
    pub jobs: usize,
    /// Cost of the most expensive lane.
    pub wall_cost: f64,
    /// `serial_cost / wall_cost`.
    pub speedup: f64,
}

/// The lane count to use, and what runs in each lane.
#[derive(Debug, Clone, Serialize)]
pub struct Recommended {
    /// Number of lanes; always `lanes.len()`.
    pub jobs: usize,
    /// As [`PlanOption::wall_cost`].
    pub wall_cost: f64,
    /// As [`PlanOption::speedup`].
    pub speedup: f64,
    /// What to run, most expensive lane first.
    pub lanes: Vec<Lane>,
}

/// One parallel slot. Its runs execute sequentially.
// `lane` is the JSON key consumers index a matrix job by; renaming the field
// to appease the lint would only add a `serde(rename)` back to the same name.
#[allow(clippy::struct_field_names)]
#[derive(Debug, Clone, Serialize)]
pub struct Lane {
    /// 1-based lane number.
    pub lane: usize,
    /// Sum of the lane's run costs.
    pub cost: f64,
    /// The lane's pytest invocations, to run in order, one at a time.
    pub runs: Vec<Run>,
}

/// One pytest invocation.
#[derive(Debug, Clone, Serialize)]
pub struct Run {
    /// The member's [`PlanMember::name`].
    pub member: String,
    /// The member's [`PlanMember::path`].
    pub path: String,
    /// The member's cost.
    pub cost: f64,
    /// Directory to run `command` from, relative to the root.
    pub cwd: String,
    /// `--pytest-cmd` followed by the test paths, shell-quoted where needed.
    pub command: String,
}

// ---------------------------------------------------------------------------
// Packing
// ---------------------------------------------------------------------------

/// Split `costs` into at most `lanes` groups, minimising the largest group
/// sum. Returns the groups as indices into `costs`.
///
/// Starts from the longest-processing-time greedy packing and tightens it
/// with a branch-and-bound search capped at [`SEARCH_NODE_BUDGET`] nodes, so
/// the result is optimal for the member counts a monorepo has and never worse
/// than greedy for the rest.
///
/// The output is deterministic: empty groups are dropped, each group lists
/// its most expensive member first, and groups are ordered by descending sum
/// with ties going to the group holding the earliest index.
pub(crate) fn partition(costs: &[f64], lanes: usize) -> Vec<Vec<usize>> {
    if costs.is_empty() || lanes == 0 {
        return Vec::new();
    }
    let lanes = lanes.min(costs.len());

    let mut order: Vec<usize> = (0..costs.len()).collect();
    order.sort_by(|&a, &b| costs[b].total_cmp(&costs[a]).then(a.cmp(&b)));

    let total: f64 = costs.iter().sum();
    let lower = costs[order[0]].max(total / lanes as f64);

    let mut search = Search {
        costs,
        order: &order,
        loads: vec![0.0; lanes],
        current: vec![0; costs.len()],
        best: Vec::new(),
        best_wall: f64::INFINITY,
        lower,
        nodes: 0,
    };
    search.seed_greedy();
    if search.best_wall > lower + EPS {
        search.descend(0);
    }

    let mut groups: Vec<Vec<usize>> = vec![Vec::new(); lanes];
    for &index in &order {
        groups[search.best[index]].push(index);
    }
    groups.retain(|g| !g.is_empty());
    let sum = |g: &Vec<usize>| group_cost(costs, g);
    let first = |g: &Vec<usize>| g.iter().copied().min().unwrap_or(usize::MAX);
    groups.sort_by(|a, b| {
        let (sa, sb) = (sum(a), sum(b));
        if (sa - sb).abs() <= EPS {
            first(a).cmp(&first(b))
        } else {
            sb.total_cmp(&sa)
        }
    });
    groups
}

/// Sum of the costs of one group of member indices.
fn group_cost(costs: &[f64], group: &[usize]) -> f64 {
    group.iter().map(|&i| costs[i]).sum()
}

/// Cost of the most expensive group in a packing.
fn wall_cost(costs: &[f64], packing: &[Vec<usize>]) -> f64 {
    packing.iter().map(|group| group_cost(costs, group)).fold(0.0, f64::max)
}

/// Branch-and-bound state for [`partition`].
struct Search<'a> {
    costs: &'a [f64],
    /// Indices into `costs`, most expensive first.
    order: &'a [usize],
    loads: Vec<f64>,
    /// Lane of each index on the path being explored.
    current: Vec<usize>,
    /// Lane of each index in the best packing found.
    best: Vec<usize>,
    best_wall: f64,
    /// No packing can beat this, so reaching it ends the search.
    lower: f64,
    nodes: usize,
}

impl Search<'_> {
    /// Longest-processing-time first: each member goes to the emptiest lane.
    fn seed_greedy(&mut self) {
        let mut loads = vec![0.0_f64; self.loads.len()];
        let mut assignment = vec![0; self.costs.len()];
        for &index in self.order {
            let mut lane = 0;
            for (candidate, load) in loads.iter().enumerate() {
                if *load < loads[lane] - EPS {
                    lane = candidate;
                }
            }
            loads[lane] += self.costs[index];
            assignment[index] = lane;
        }
        self.best_wall = loads.iter().copied().fold(0.0, f64::max);
        self.best = assignment;
    }

    fn done(&self) -> bool {
        self.best_wall <= self.lower + EPS || self.nodes >= SEARCH_NODE_BUDGET
    }

    fn descend(&mut self, depth: usize) {
        if self.done() {
            return;
        }
        self.nodes += 1;
        if depth == self.order.len() {
            // Every branch that got here stayed strictly under `best_wall`.
            self.best_wall = self.loads.iter().copied().fold(0.0, f64::max);
            self.best.clone_from(&self.current);
            return;
        }
        let index = self.order[depth];
        let cost = self.costs[index];
        for lane in 0..self.loads.len() {
            let load = self.loads[lane];
            // Lanes with equal loads are interchangeable: try only the first.
            if self.loads[..lane].iter().any(|earlier| (earlier - load).abs() <= EPS) {
                continue;
            }
            if load + cost >= self.best_wall - EPS {
                continue;
            }
            self.loads[lane] = load + cost;
            self.current[index] = lane;
            self.descend(depth + 1);
            self.loads[lane] = load;
            if self.done() {
                return;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Building a plan
// ---------------------------------------------------------------------------

/// A selected subproject before costs are assigned.
struct Candidate {
    name: String,
    /// Path relative to the plan root, `/`-separated; `.` for the root.
    rel: String,
    tests_dirs: Vec<PathBuf>,
    dir: PathBuf,
    test_count: u64,
    runtime_seconds: Option<f64>,
}

/// Build the run plan for the subprojects of `root` named in `opts`.
///
/// # Errors
///
/// When `root` is not a directory, a knob is out of range, a named
/// subproject cannot be resolved, a timings file is unreadable or not an
/// inventory, or no selected subproject has anything to run.
pub fn build_plan(root: &Path, opts: &PlanOptions) -> Result<RunPlan> {
    if !opts.tolerance_pct.is_finite() || opts.tolerance_pct < 0.0 {
        anyhow::bail!("--tolerance must be a non-negative percentage");
    }
    if !opts.startup_seconds.is_finite() || opts.startup_seconds < 0.0 {
        anyhow::bail!("--startup-seconds must be a non-negative number of seconds");
    }
    if !root.is_dir() {
        anyhow::bail!("plan root is not a directory: {}", root.display());
    }
    let root = std::fs::canonicalize(root)
        .with_context(|| format!("failed to canonicalize {}", root.display()))?;

    let dirs = select_members(&root, &opts.members)?;
    let timings = load_timings(&opts.timings)?;

    let mut candidates: Vec<Candidate> = Vec::new();
    let mut skipped: Vec<SkippedMember> = Vec::new();
    for dir in dirs {
        let name = member_name(&dir);
        let rel = slash_path(dir.strip_prefix(&root).unwrap_or(&dir));
        let tests_dirs = crate::resolve_tests_dirs(&dir, None, false).existing;
        if tests_dirs.is_empty() {
            skipped.push(SkippedMember { name, path: rel, reason: "no tests directory".into() });
            continue;
        }
        let timing = timings.lookup(&dir, &name);
        let runtime_seconds = timing.and_then(|t| t.runtime_seconds);
        // A collected count of zero is what a run with broken collection
        // reports; it must not erase a suite the static pass can see.
        let test_count = match timing.and_then(|t| t.test_count).filter(|&n| n > 0) {
            Some(collected) => collected,
            None => static_test_count(&dir)?,
        };
        if test_count == 0 && runtime_seconds.is_none() {
            skipped.push(SkippedMember { name, path: rel, reason: "no tests".into() });
            continue;
        }
        candidates.push(Candidate { name, rel, tests_dirs, dir, test_count, runtime_seconds });
    }

    if candidates.is_empty() && skipped.is_empty() {
        anyhow::bail!("nothing to plan: the uv workspace at {} has no members", root.display());
    }
    if candidates.is_empty() {
        let why: Vec<String> =
            skipped.iter().map(|s| format!("{} ({})", s.path, s.reason)).collect();
        anyhow::bail!("nothing to plan under {}: {}", root.display(), why.join(", "));
    }

    let (cost_unit, costs) = assign_costs(&candidates, opts.startup_seconds);
    let members: Vec<PlanMember> = candidates
        .iter()
        .zip(&costs)
        .map(|(c, &(cost, cost_source))| PlanMember {
            name: c.name.clone(),
            path: c.rel.clone(),
            test_count: c.test_count,
            runtime_seconds: c.runtime_seconds,
            cost,
            cost_source,
        })
        .collect();
    let costs: Vec<f64> = costs.iter().map(|&(cost, _)| cost).collect();

    let serial_cost: f64 = costs.iter().sum();
    let (options, packings) = pack_every_lane_count(&costs, opts.max_jobs.get(), serial_cost);

    let best_wall = options.iter().map(|o| o.wall_cost).fold(f64::INFINITY, f64::min);
    let threshold = best_wall.mul_add(1.0 + opts.tolerance_pct / 100.0, EPS);
    let chosen = options.iter().position(|o| o.wall_cost <= threshold).unwrap_or(0);

    // The first of several equally expensive members is the one reported.
    let top = (0..costs.len()).fold(0, |top, i| if costs[i] > costs[top] { i } else { top });
    let bottleneck = Bottleneck {
        member: members[top].name.clone(),
        path: members[top].path.clone(),
        cost: costs[top],
        limits_wall_time: options[chosen].wall_cost <= costs[top] + EPS,
    };

    let lanes: Vec<Lane> = packings[chosen]
        .iter()
        .enumerate()
        .map(|(position, group)| Lane {
            lane: position + 1,
            cost: round3(group_cost(&costs, group)),
            runs: group.iter().map(|&i| run_for(&root, &candidates[i], costs[i], opts)).collect(),
        })
        .collect();

    Ok(RunPlan {
        schema_version: PLAN_SCHEMA_VERSION.to_string(),
        root,
        cost_unit,
        max_jobs: opts.max_jobs.get(),
        tolerance_pct: opts.tolerance_pct,
        startup_seconds: (cost_unit == CostUnit::Seconds).then_some(opts.startup_seconds),
        members,
        skipped,
        serial_cost: round3(serial_cost),
        bottleneck,
        recommended: Recommended {
            jobs: lanes.len(),
            wall_cost: options[chosen].wall_cost,
            speedup: options[chosen].speedup,
            lanes,
        },
        options,
    })
}

/// One [`PlanOption`] and its packing per lane count, from one lane up to
/// `max_jobs` (never more lanes than members). Wall costs never rise with
/// the lane count: a count whose search came out worse reuses the previous
/// packing, and once the most expensive member alone sets the wall cost the
/// remaining counts reuse it without searching.
fn pack_every_lane_count(
    costs: &[f64],
    max_jobs: usize,
    serial_cost: f64,
) -> (Vec<PlanOption>, Vec<Vec<Vec<usize>>>) {
    let floor = costs.iter().copied().fold(0.0, f64::max);
    let wall_of = |packing: &[Vec<usize>]| wall_cost(costs, packing);

    let mut options: Vec<PlanOption> = Vec::new();
    let mut packings: Vec<Vec<Vec<usize>>> = Vec::new();
    for jobs in 1..=max_jobs.min(costs.len()) {
        let previous = packings.last().map(|p| (p, wall_of(p)));
        let packing = match previous {
            Some((p, wall)) if wall <= floor + EPS => p.clone(),
            Some((p, wall)) => {
                let candidate = partition(costs, jobs);
                if wall_of(&candidate) < wall - EPS {
                    candidate
                } else {
                    p.clone()
                }
            }
            None => partition(costs, jobs),
        };
        let wall = wall_of(&packing);
        options.push(PlanOption {
            jobs,
            wall_cost: round3(wall),
            speedup: if wall > EPS { round2(serial_cost / wall) } else { 1.0 },
        });
        packings.push(packing);
    }
    (options, packings)
}

/// Pick the cost unit and give every candidate a cost in it.
///
/// Seconds whenever the measured members yield a seconds-per-test rate (or
/// every member was measured); test counts otherwise, for all members, so
/// that seconds and counts are never mixed in one plan. A count cost is at
/// least 1: a member that is scheduled is never free. A seconds cost is the
/// runtime plus `startup_seconds`; the rate is derived from runtimes alone.
fn assign_costs(
    candidates: &[Candidate],
    startup_seconds: f64,
) -> (CostUnit, Vec<(f64, CostSource)>) {
    let measured: Vec<(f64, u64)> = candidates
        .iter()
        .filter_map(|c| c.runtime_seconds.map(|seconds| (seconds, c.test_count)))
        .collect();
    let measured_tests: u64 = measured.iter().map(|&(_, tests)| tests).sum();
    let measured_seconds: f64 = measured.iter().map(|&(seconds, _)| seconds).sum();
    let rate = (measured_tests > 0).then(|| measured_seconds / measured_tests as f64);

    let all_measured = measured.len() == candidates.len();
    if !all_measured && rate.is_none() {
        let costs = candidates
            .iter()
            .map(|c| (c.test_count.max(1) as f64, CostSource::TestCount))
            .collect();
        return (CostUnit::Tests, costs);
    }
    let rate = rate.unwrap_or(0.0);
    let costs = candidates
        .iter()
        .map(|c| match c.runtime_seconds {
            Some(seconds) => (round3(seconds + startup_seconds), CostSource::Measured),
            None => (
                round3((c.test_count as f64).mul_add(rate, startup_seconds)),
                CostSource::Estimated,
            ),
        })
        .collect();
    (CostUnit::Seconds, costs)
}

/// The pytest invocation for one member, anchored per `--member-cwd`.
fn run_for(root: &Path, candidate: &Candidate, cost: f64, opts: &PlanOptions) -> Run {
    let (base, cwd) = match opts.member_cwd {
        MemberCwd::Root => (root, ".".to_string()),
        MemberCwd::Member => (candidate.dir.as_path(), candidate.rel.clone()),
    };
    let mut command = opts.pytest_cmd.trim().to_string();
    for dir in &candidate.tests_dirs {
        command.push(' ');
        command.push_str(&shell_quote(&slash_path(dir.strip_prefix(base).unwrap_or(dir))));
    }
    Run { member: candidate.name.clone(), path: candidate.rel.clone(), cost, cwd, command }
}

/// Resolve the requested subprojects to canonical directories, in the order
/// asked for (workspace order when nothing was named), without duplicates.
fn select_members(root: &Path, requested: &[String]) -> Result<Vec<PathBuf>> {
    let mut selected: Vec<PathBuf> = Vec::new();
    let mut push = |dir: PathBuf| {
        if !selected.contains(&dir) {
            selected.push(dir);
        }
    };

    let Some(ws) = workspace::detect(root)? else {
        if requested.is_empty() {
            anyhow::bail!(
                "{} declares no [tool.uv.workspace]; name the subproject directories to plan, \
                 e.g. `pycoati plan . services/api libs/core`",
                root.display()
            );
        }
        for token in requested {
            let dir = std::fs::canonicalize(root.join(token)).with_context(|| {
                format!("subproject `{token}` not found under {}", root.display())
            })?;
            if !dir.is_dir() {
                anyhow::bail!("subproject `{token}` is not a directory");
            }
            if !dir.starts_with(root) {
                anyhow::bail!("subproject `{token}` is outside {}", root.display());
            }
            push(dir);
        }
        return Ok(selected);
    };

    if requested.is_empty() {
        return Ok(ws.members);
    }
    let known: Vec<(String, String, &PathBuf)> = ws
        .members
        .iter()
        .map(|dir| (slash_path(dir.strip_prefix(root).unwrap_or(dir)), member_name(dir), dir))
        .collect();
    let available = || known.iter().map(|(rel, ..)| rel.as_str()).collect::<Vec<_>>().join(", ");
    for token in requested {
        let wanted = token.trim_start_matches("./").trim_end_matches('/');
        let by_path: Vec<&PathBuf> =
            known.iter().filter(|(rel, ..)| rel == wanted).map(|(.., dir)| *dir).collect();
        let matches = if by_path.is_empty() {
            known
                .iter()
                .filter(|(rel, name, _)| name == wanted || rel.rsplit('/').next() == Some(wanted))
                .map(|(.., dir)| *dir)
                .collect()
        } else {
            by_path
        };
        match matches.as_slice() {
            [dir] => push((*dir).clone()),
            [] => anyhow::bail!("`{token}` is not a workspace member; available: {}", available()),
            _ => anyhow::bail!(
                "`{token}` matches more than one workspace member; use its path instead: {}",
                available()
            ),
        }
    }
    Ok(selected)
}

/// `[project].name` when declared, else the directory basename.
fn member_name(dir: &Path) -> String {
    pyproject::read_project_name(dir).unwrap_or_else(|| {
        dir.file_name().map_or_else(|| ".".to_string(), |n| n.to_string_lossy().into_owned())
    })
}

/// Number of test functions the static pass finds in a subproject.
/// Parametrised tests count once, so this undercounts what pytest collects.
fn static_test_count(dir: &Path) -> Result<u64> {
    let accept = AcceptOptions { disabled: true, ..AcceptOptions::default() };
    let inventory = crate::run_static_dir_with_options(dir, None, None, 0, false, &accept)
        .with_context(|| format!("counting tests in {}", dir.display()))?;
    Ok(inventory.test_functions.len() as u64)
}

/// `/`-separated rendering of a relative path; `.` when it is empty.
fn slash_path(path: &Path) -> String {
    let parts: Vec<String> = path
        .components()
        .filter(|c| !matches!(c, Component::CurDir))
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    if parts.is_empty() {
        ".".to_string()
    } else {
        parts.join("/")
    }
}

/// POSIX-shell quoting for a path dropped into a command line: unchanged
/// when it is made of characters no shell interprets, single-quoted otherwise.
fn shell_quote(text: &str) -> String {
    let plain = |c: char| {
        c.is_ascii_alphanumeric()
            || matches!(c, '_' | '.' | '/' | '-' | '+' | '@' | ':' | ',' | '=')
    };
    if !text.is_empty() && text.chars().all(plain) {
        text.to_string()
    } else {
        format!("'{}'", text.replace('\'', "'\\''"))
    }
}

fn round3(value: f64) -> f64 {
    (value * 1000.0).round() / 1000.0
}

fn round2(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

// ---------------------------------------------------------------------------
// Timings
// ---------------------------------------------------------------------------

/// The slice of an inventory payload the plan reads. Both shapes pycoati
/// emits are accepted: a workspace wrapper (`members`) and a bare
/// single-project inventory (`project` + `suite`).
#[derive(Deserialize)]
struct TimingsFile {
    members: Option<Vec<TimedInventory>>,
    #[serde(flatten)]
    single: Option<TimedInventory>,
}

#[derive(Deserialize)]
struct TimedInventory {
    project: TimedProject,
    suite: TimedSuite,
}

#[derive(Deserialize)]
struct TimedProject {
    path: PathBuf,
    name: String,
}

#[derive(Deserialize)]
struct TimedSuite {
    test_count: Option<u64>,
    runtime_seconds: Option<f64>,
}

/// One project's measurements from a timings inventory.
struct Timing {
    path: PathBuf,
    name: String,
    test_count: Option<u64>,
    runtime_seconds: Option<f64>,
}

#[derive(Default)]
struct Timings(Vec<Timing>);

impl Timings {
    /// Measurements for a member: by project path when the inventory was
    /// produced in this checkout, else by project name when exactly one
    /// entry carries it (an inventory from CI has foreign paths).
    fn lookup(&self, dir: &Path, name: &str) -> Option<&Timing> {
        if let Some(found) = self.0.iter().find(|t| t.path == dir) {
            return Some(found);
        }
        let mut named = self.0.iter().filter(|t| t.name == name);
        match (named.next(), named.next()) {
            (Some(only), None) => Some(only),
            (Some(_), Some(_)) => {
                tracing::warn!(
                    project = name,
                    member = %dir.display(),
                    "several timings entries share this project name and none matches the \
                     member's path; ignoring them"
                );
                None
            }
            _ => None,
        }
    }
}

fn load_timings(files: &[PathBuf]) -> Result<Timings> {
    let mut timings = Timings::default();
    for file in files {
        let text = std::fs::read_to_string(file)
            .with_context(|| format!("failed to read timings file {}", file.display()))?;
        let parsed: TimingsFile = serde_json::from_str(&text)
            .with_context(|| format!("{} is not a pycoati inventory", file.display()))?;
        let inventories = match (parsed.members, parsed.single) {
            (Some(members), _) => members,
            (None, Some(single)) => vec![single],
            (None, None) => anyhow::bail!(
                "{} is not a pycoati inventory (no `members`, no `project` + `suite`)",
                file.display()
            ),
        };
        for inv in inventories {
            // The same project measured again — a later `--timings` file, or
            // a newer run — replaces the earlier entry instead of sitting
            // beside it and making the project's name ambiguous.
            timings.0.retain(|earlier| earlier.path != inv.project.path);
            timings.0.push(Timing {
                path: inv.project.path,
                name: inv.project.name,
                test_count: inv.suite.test_count,
                // A negative or non-finite runtime would poison every sum.
                runtime_seconds: inv.suite.runtime_seconds.filter(|s| s.is_finite() && *s >= 0.0),
            });
        }
    }
    Ok(timings)
}

// ---------------------------------------------------------------------------
// Pretty rendering
// ---------------------------------------------------------------------------

/// Breadcrumb closing the pretty view, like the audit's.
const PLAN_FOOTER: &str = "next: run `pycoati guide plan`";

/// Aligned-column terminal view of a plan. Pure: no I/O.
pub fn render_pretty(plan: &RunPlan) -> String {
    let cost = |value: f64| match plan.cost_unit {
        CostUnit::Seconds => format!("{value:.1}s"),
        CostUnit::Tests if (value - 1.0).abs() <= EPS => "1 test".to_string(),
        CostUnit::Tests => format!("{value:.0} tests"),
    };
    let mut out = String::new();
    let basename = plan.root.file_name().map_or_else(
        || plan.root.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    );
    heading(&mut out, &format!("pycoati plan — {basename}"), '=');

    let unit = match (plan.cost_unit, plan.startup_seconds) {
        (CostUnit::Seconds, Some(startup)) if startup > 0.0 => {
            format!("seconds, incl. {startup:.1}s start-up per run")
        }
        (CostUnit::Seconds, _) => "seconds".to_string(),
        (CostUnit::Tests, _) => "test counts; no measured runtimes".to_string(),
    };
    heading(&mut out, &format!("Subprojects (cost in {unit})"), '-');
    let rows: Vec<[String; 4]> = plan
        .members
        .iter()
        .map(|m| {
            [
                cost(m.cost),
                m.test_count.to_string(),
                m.cost_source.as_str().to_string(),
                m.path.clone(),
            ]
        })
        .collect();
    table(&mut out, ["cost", "tests", "source", "path"], &rows);

    if !plan.skipped.is_empty() {
        heading(&mut out, "Skipped", '-');
        for s in &plan.skipped {
            let _ = writeln!(out, "  {} — {}", s.path, s.reason);
        }
        out.push('\n');
    }

    heading(&mut out, &format!("Parallel runs (up to {})", plan.max_jobs), '-');
    let rows: Vec<[String; 4]> = plan
        .options
        .iter()
        .map(|o| {
            let mark = if o.jobs == plan.recommended.jobs { "*" } else { "" };
            [mark.to_string(), o.jobs.to_string(), cost(o.wall_cost), format!("{:.2}x", o.speedup)]
        })
        .collect();
    table(&mut out, ["", "runs", "wall", "speedup"], &rows);

    let rec = &plan.recommended;
    let runs = if rec.jobs == 1 {
        "1 run, no parallelism".to_string()
    } else {
        format!("{} parallel runs", rec.jobs)
    };
    let _ = writeln!(
        out,
        "Recommended: {runs} — wall {}, serial {}, {:.2}x",
        cost(rec.wall_cost),
        cost(plan.serial_cost),
        rec.speedup
    );
    if plan.bottleneck.limits_wall_time {
        let _ = writeln!(
            out,
            "Bottleneck: {} ({}) sets the floor; more runs cannot go faster, splitting it can.",
            plan.bottleneck.path,
            cost(plan.bottleneck.cost)
        );
    }
    out.push('\n');

    for lane in &rec.lanes {
        let _ = writeln!(out, "Lane {} — {}", lane.lane, cost(lane.cost));
        for run in &lane.runs {
            if run.cwd == "." {
                let _ = writeln!(out, "  {}", run.command);
            } else {
                let _ = writeln!(out, "  (cd {} && {})", shell_quote(&run.cwd), run.command);
            }
        }
    }
    out.push('\n');
    out.push_str(PLAN_FOOTER);
    out
}

fn heading(out: &mut String, title: &str, underline: char) {
    let _ = writeln!(out, "{title}");
    let _ = writeln!(out, "{}", underline.to_string().repeat(title.chars().count()));
    if underline == '=' {
        out.push('\n');
    }
}

/// Left-aligned columns padded to their widest cell, two-space indent.
fn table<const N: usize>(out: &mut String, header: [&str; N], rows: &[[String; N]]) {
    let mut widths = header.map(|h| h.chars().count());
    for row in rows {
        for (width, cell) in widths.iter_mut().zip(row) {
            *width = (*width).max(cell.chars().count());
        }
    }
    let mut line = |cells: Vec<&str>| {
        let mut text = String::from(" ");
        for (cell, width) in cells.iter().zip(widths) {
            let _ = write!(text, " {cell:<width$} ");
        }
        let _ = writeln!(out, "{}", text.trim_end());
    };
    line(header.to_vec());
    for row in rows {
        line(row.iter().map(String::as_str).collect());
    }
    out.push('\n');
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::float_cmp)]
mod tests {
    use super::*;

    fn wall(costs: &[f64], groups: &[Vec<usize>]) -> f64 {
        wall_cost(costs, groups)
    }

    fn assert_is_a_partition(costs: &[f64], groups: &[Vec<usize>]) {
        let mut seen: Vec<usize> = groups.iter().flatten().copied().collect();
        seen.sort_unstable();
        assert_eq!(seen, (0..costs.len()).collect::<Vec<_>>(), "every member exactly once");
    }

    #[test]
    fn one_lane_holds_everything() {
        let costs = [3.0, 1.0, 2.0];
        let groups = partition(&costs, 1);
        assert_eq!(groups, vec![vec![0, 2, 1]], "most expensive member first");
    }

    #[test]
    fn beats_greedy_where_greedy_is_suboptimal() {
        // Greedy (longest first, emptiest lane) lands on 3+2+2 | 3+2 = 7.
        // The optimum is 3+3 | 2+2+2 = 6.
        let costs = [3.0, 3.0, 2.0, 2.0, 2.0];
        let groups = partition(&costs, 2);
        assert_is_a_partition(&costs, &groups);
        assert_eq!(wall(&costs, &groups), 6.0);
        assert_eq!(groups, vec![vec![0, 1], vec![2, 3, 4]]);
    }

    #[test]
    fn never_uses_more_lanes_than_members() {
        let costs = [5.0, 4.0];
        let groups = partition(&costs, 8);
        assert_eq!(groups, vec![vec![0], vec![1]]);
    }

    #[test]
    fn a_dominant_member_gets_a_lane_to_itself() {
        let costs = [10.0, 100.0, 20.0, 30.0];
        let groups = partition(&costs, 2);
        assert_eq!(groups, vec![vec![1], vec![3, 2, 0]]);
        assert_eq!(wall(&costs, &groups), 100.0);
    }

    #[test]
    fn empty_input_and_zero_lanes_yield_no_groups() {
        assert!(partition(&[], 3).is_empty());
        assert!(partition(&[1.0], 0).is_empty());
    }

    #[test]
    fn zero_cost_members_are_still_scheduled() {
        let costs = [0.0, 0.0, 4.0];
        let groups = partition(&costs, 3);
        assert_is_a_partition(&costs, &groups);
        assert_eq!(wall(&costs, &groups), 4.0);
    }

    #[test]
    fn a_large_input_stays_within_the_search_budget_and_no_worse_than_greedy() {
        // 40 members with awkward costs: the search may not finish, but the
        // result must be a valid partition at or under the greedy bound of
        // 4/3 of the lower bound.
        let costs: Vec<f64> = (0..40_u32).map(|i| f64::from(17 + (i * 37) % 23)).collect();
        let groups = partition(&costs, 6);
        assert_is_a_partition(&costs, &groups);
        let total: f64 = costs.iter().sum();
        assert!(wall(&costs, &groups) <= total / 6.0 * 4.0 / 3.0);
    }

    #[test]
    fn wall_cost_never_rises_with_more_lanes() {
        let costs: Vec<f64> = (0..15_u32).map(|i| f64::from(5 + (i * 13) % 11)).collect();
        let (options, packings) = pack_every_lane_count(&costs, 8, costs.iter().sum());
        assert_eq!(options.len(), 8);
        for pair in options.windows(2) {
            assert!(pair[1].wall_cost <= pair[0].wall_cost, "{options:?}");
        }
        for packing in &packings {
            assert_is_a_partition(&costs, packing);
        }
    }

    #[test]
    fn slash_path_is_dot_for_the_root_itself() {
        assert_eq!(slash_path(Path::new("")), ".");
        assert_eq!(slash_path(Path::new("packages/a")), "packages/a");
    }

    #[test]
    fn timings_lookup_prefers_path_then_a_unique_name() {
        let timing = |path: &str, name: &str, seconds: f64| Timing {
            path: PathBuf::from(path),
            name: name.to_string(),
            test_count: None,
            runtime_seconds: Some(seconds),
        };
        let timings = Timings(vec![
            timing("/ci/a", "a", 1.0),
            timing("/here/b", "dup", 2.0),
            timing("/ci/c", "dup", 3.0),
        ]);
        let seconds = |dir: &str, name: &str| {
            timings.lookup(Path::new(dir), name).and_then(|t| t.runtime_seconds)
        };
        assert_eq!(seconds("/here/a", "a"), Some(1.0), "unique name matches a foreign path");
        assert_eq!(seconds("/here/b", "dup"), Some(2.0), "path match wins");
        assert_eq!(seconds("/here/x", "dup"), None, "an ambiguous name matches nothing");
    }
}
