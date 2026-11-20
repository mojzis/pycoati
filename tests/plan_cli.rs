//! CLI-level tests for `pycoati plan`: which subprojects end up in the plan,
//! where their costs come from, how many parallel runs are recommended, and
//! the commands the plan hands back.
//!
//! Workspaces with specific runtimes are built in temp dirs: the plan never
//! runs pytest, so a member is just a `pyproject.toml` plus a test file, and
//! the runtimes arrive through a hand-written `--timings` inventory.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

mod common;

use std::fs;
use std::path::Path;

use assert_cmd::Command;
use predicates::prelude::*;
use serde_json::{json, Value};
use tempfile::TempDir;

use common::fixture_path;

/// `pycoati plan <args…>`, asserted successful, stdout parsed as JSON.
fn plan_json(args: &[&str]) -> Value {
    let assert =
        Command::cargo_bin("pycoati").expect("binary built").arg("plan").args(args).assert();
    let assert = assert.success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).expect("utf-8 stdout");
    serde_json::from_str(&stdout).expect("plan stdout must be valid JSON")
}

/// `pycoati plan <args…> --format pretty`, asserted successful.
fn plan_pretty(args: &[&str]) -> String {
    let assert = Command::cargo_bin("pycoati")
        .expect("binary built")
        .arg("plan")
        .args(args)
        .args(["--format", "pretty"])
        .assert()
        .success();
    String::from_utf8(assert.get_output().stdout.clone()).expect("utf-8 stdout")
}

/// `pycoati plan <args…>` must fail with every `needle` on stderr.
fn plan_fails(args: &[&str], needles: &[&str]) {
    let assert =
        Command::cargo_bin("pycoati").expect("binary built").arg("plan").args(args).assert();
    let assert = assert.failure();
    let stderr = String::from_utf8_lossy(&assert.get_output().stderr).into_owned();
    for needle in needles {
        assert!(stderr.contains(needle), "stderr for {args:?} lacks `{needle}`: {stderr}");
    }
}

/// Create `<root>/<rel>` as a member holding `tests` trivial test functions.
fn make_member(root: &Path, rel: &str, tests: usize) {
    let dir = root.join(rel);
    let name = Path::new(rel).file_name().unwrap().to_string_lossy().into_owned();
    fs::create_dir_all(dir.join("tests")).expect("mkdir member tests");
    fs::write(dir.join("pyproject.toml"), format!("[project]\nname = \"{name}\"\n"))
        .expect("write member pyproject");
    let body = (0..tests)
        .map(|i| format!("def test_case_{i}():\n    assert {i} == {i}\n"))
        .collect::<Vec<_>>()
        .join("\n");
    fs::write(dir.join("tests").join(format!("test_{name}.py")), body).expect("write test file");
}

/// A uv workspace under `packages/*` with one member per `(name, tests)`.
fn make_workspace(members: &[(&str, usize)]) -> TempDir {
    let tmp = TempDir::new().expect("tempdir");
    fs::write(
        tmp.path().join("pyproject.toml"),
        "[project]\nname = \"mono\"\n\n[tool.uv.workspace]\nmembers = [\"packages/*\"]\n",
    )
    .expect("write root pyproject");
    for (name, tests) in members {
        make_member(tmp.path(), &format!("packages/{name}"), *tests);
    }
    tmp
}

/// Write a workspace-shaped timings inventory naming members by project name
/// only (the path is deliberately foreign, as it is for a CI artifact).
fn write_timings(dir: &Path, runtimes: &[(&str, f64, u64)]) -> String {
    let members: Vec<Value> = runtimes
        .iter()
        .map(|(name, seconds, tests)| {
            json!({
                "project": {"path": format!("/ci/checkout/packages/{name}"), "name": name},
                "suite": {"test_count": tests, "runtime_seconds": seconds},
            })
        })
        .collect();
    let payload =
        json!({"schema_version": "3", "workspace_root": "/ci/checkout", "members": members});
    let path = dir.join("timings.json");
    fs::write(&path, serde_json::to_string(&payload).unwrap()).expect("write timings");
    path.to_string_lossy().into_owned()
}

fn lane_members(plan: &Value) -> Vec<Vec<String>> {
    plan["recommended"]["lanes"]
        .as_array()
        .expect("lanes array")
        .iter()
        .map(|lane| {
            lane["runs"]
                .as_array()
                .expect("runs array")
                .iter()
                .map(|run| run["member"].as_str().unwrap().to_string())
                .collect()
        })
        .collect()
}

// --- static (no timings) -----------------------------------------------------

#[test]
fn static_plan_counts_tests_and_balances_on_them() {
    let ws = fixture_path("tests/fixtures/uv_workspace");
    let plan = plan_json(&[ws.to_str().unwrap(), "--max-jobs", "2", "--tolerance", "0"]);

    assert_eq!(plan["schema_version"], "1");
    assert_eq!(plan["cost_unit"], "tests");
    assert_eq!(plan["max_jobs"], 2);

    let members = plan["members"].as_array().expect("members array");
    assert_eq!(members.len(), 2);
    assert_eq!(members[0]["name"], "pkg_a");
    assert_eq!(members[0]["path"], "packages/pkg_a");
    assert_eq!(members[0]["test_count"], 2);
    assert_eq!(members[0]["runtime_seconds"], Value::Null);
    assert_eq!(members[0]["cost"], 2.0);
    assert_eq!(members[0]["cost_source"], "test_count");
    assert_eq!(members[1]["name"], "pkg_b");
    assert_eq!(members[1]["test_count"], 1);

    assert_eq!(plan["serial_cost"], 3.0);
    assert_eq!(plan["recommended"]["jobs"], 2);
    assert_eq!(plan["recommended"]["wall_cost"], 2.0);
    assert_eq!(plan["recommended"]["speedup"], 1.5);
    assert_eq!(lane_members(&plan), vec![vec!["pkg_a"], vec!["pkg_b"]]);
}

#[test]
fn runs_carry_a_ready_command_from_the_workspace_root() {
    let ws = fixture_path("tests/fixtures/uv_workspace");
    let plan = plan_json(&[ws.to_str().unwrap(), "--max-jobs", "2", "--tolerance", "0"]);
    let run = &plan["recommended"]["lanes"][0]["runs"][0];
    assert_eq!(run["member"], "pkg_a");
    assert_eq!(run["path"], "packages/pkg_a");
    assert_eq!(run["cwd"], ".");
    assert_eq!(run["command"], "python -m pytest packages/pkg_a/tests");
}

#[test]
fn member_cwd_member_anchors_each_command_in_its_member() {
    let ws = fixture_path("tests/fixtures/uv_workspace");
    let plan = plan_json(&[
        ws.to_str().unwrap(),
        "--member-cwd",
        "member",
        "--pytest-cmd",
        "uv run pytest -q",
    ]);
    let run = &plan["recommended"]["lanes"][0]["runs"][0];
    assert_eq!(run["cwd"], "packages/pkg_a");
    assert_eq!(run["command"], "uv run pytest -q tests");
}

// --- selecting subprojects ---------------------------------------------------

#[test]
fn naming_members_restricts_the_plan_to_them() {
    let ws = make_workspace(&[("api", 4), ("core", 3), ("web", 2), ("cli", 1)]);
    let plan = plan_json(&[
        ws.path().to_str().unwrap(),
        "core",
        "packages/cli",
        "--max-jobs",
        "4",
        "--tolerance",
        "0",
    ]);
    let names: Vec<&str> =
        plan["members"].as_array().unwrap().iter().map(|m| m["name"].as_str().unwrap()).collect();
    assert_eq!(names, vec!["core", "cli"], "members keep the order they were asked for in");
    assert_eq!(plan["serial_cost"], 4.0);
    assert_eq!(plan["recommended"]["jobs"], 2);
    assert_eq!(plan["options"].as_array().unwrap().len(), 2, "never more lanes than members");
}

#[test]
fn an_unknown_member_fails_and_lists_the_real_ones() {
    let ws = fixture_path("tests/fixtures/uv_workspace");
    Command::cargo_bin("pycoati")
        .expect("binary built")
        .args(["plan", ws.to_str().unwrap(), "pkg_zzz"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("pkg_zzz"))
        .stderr(predicate::str::contains("packages/pkg_a"))
        .stderr(predicate::str::contains("packages/pkg_b"));
}

#[test]
fn a_member_without_tests_is_reported_as_skipped_not_scheduled() {
    let ws = fixture_path("tests/fixtures/uv_workspace_partial");
    let plan = plan_json(&[ws.to_str().unwrap()]);
    let members = plan["members"].as_array().unwrap();
    assert_eq!(members.len(), 1);
    assert_eq!(members[0]["name"], "pkg_a");
    let skipped = plan["skipped"].as_array().unwrap();
    assert_eq!(skipped.len(), 1);
    assert_eq!(skipped[0]["path"], "packages/pkg_b");
    assert_eq!(skipped[0]["reason"], "no tests directory");
    assert_eq!(lane_members(&plan), vec![vec!["pkg_a"]]);
}

#[test]
fn a_plan_with_nothing_to_run_is_an_error() {
    let ws = fixture_path("tests/fixtures/uv_workspace_partial");
    Command::cargo_bin("pycoati")
        .expect("binary built")
        .args(["plan", ws.to_str().unwrap(), "pkg_b"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("no tests"));
}

#[test]
fn a_plain_monorepo_takes_subproject_directories() {
    // No [tool.uv.workspace]: the subprojects are whatever directories the
    // caller names.
    let tmp = TempDir::new().expect("tempdir");
    make_member(tmp.path(), "services/api", 5);
    make_member(tmp.path(), "libs/core", 2);
    let plan = plan_json(&[
        tmp.path().to_str().unwrap(),
        "services/api",
        "libs/core",
        "--max-jobs",
        "2",
        "--tolerance",
        "0",
    ]);
    assert_eq!(plan["members"][0]["path"], "services/api");
    assert_eq!(plan["members"][0]["test_count"], 5);
    assert_eq!(plan["recommended"]["jobs"], 2);
    assert_eq!(
        plan["recommended"]["lanes"][1]["runs"][0]["command"],
        "python -m pytest libs/core/tests"
    );
}

#[test]
fn a_plain_monorepo_without_named_subprojects_is_an_error() {
    let tmp = TempDir::new().expect("tempdir");
    make_member(tmp.path(), "services/api", 1);
    Command::cargo_bin("pycoati")
        .expect("binary built")
        .args(["plan", tmp.path().to_str().unwrap()])
        .assert()
        .failure()
        .stderr(predicate::str::contains("tool.uv.workspace"))
        .stderr(predicate::str::contains("name the subproject directories"));
}

// --- measured runtimes -------------------------------------------------------

#[test]
fn measured_runtimes_drive_the_plan_and_the_longest_member_caps_it() {
    let ws = make_workspace(&[("a", 1), ("b", 1), ("c", 1), ("d", 1)]);
    let timings = write_timings(
        ws.path(),
        &[("a", 60.0, 10), ("b", 30.0, 10), ("c", 20.0, 10), ("d", 10.0, 10)],
    );
    let plan = plan_json(&[
        ws.path().to_str().unwrap(),
        "--timings",
        &timings,
        "--startup-seconds",
        "0",
        "--max-jobs",
        "4",
        "--tolerance",
        "0",
    ]);

    assert_eq!(plan["cost_unit"], "seconds");
    assert_eq!(plan["members"][0]["cost_source"], "measured");
    assert_eq!(plan["members"][0]["runtime_seconds"], 60.0);
    assert_eq!(plan["members"][0]["test_count"], 10, "collected count wins over the static one");
    assert_eq!(plan["serial_cost"], 120.0);

    // 60 | 30+20+10: two lanes already reach the 60s floor set by `a`, so
    // lanes three and four would buy nothing.
    let walls: Vec<f64> = plan["options"]
        .as_array()
        .unwrap()
        .iter()
        .map(|o| o["wall_cost"].as_f64().unwrap())
        .collect();
    assert_eq!(walls, vec![120.0, 60.0, 60.0, 60.0]);
    assert_eq!(plan["recommended"]["jobs"], 2);
    assert_eq!(plan["recommended"]["wall_cost"], 60.0);
    assert_eq!(plan["recommended"]["speedup"], 2.0);
    assert_eq!(lane_members(&plan), vec![vec!["a"], vec!["b", "c", "d"]]);
    assert_eq!(plan["recommended"]["lanes"][1]["cost"], 60.0);

    assert_eq!(plan["bottleneck"]["member"], "a");
    assert_eq!(plan["bottleneck"]["cost"], 60.0);
    assert_eq!(plan["bottleneck"]["limits_wall_time"], true);
}

#[test]
fn tolerance_trades_a_little_wall_time_for_fewer_runs() {
    // Best splits: 1 lane 150s, 2 lanes 80s, 3+ lanes 50s.
    let ws = make_workspace(&[("a", 1), ("b", 1), ("c", 1), ("d", 1), ("e", 1)]);
    let timings = write_timings(
        ws.path(),
        &[("a", 50.0, 1), ("b", 40.0, 1), ("c", 30.0, 1), ("d", 20.0, 1), ("e", 10.0, 1)],
    );
    let root = ws.path().to_str().unwrap();

    let base = [root, "--timings", &timings, "--startup-seconds", "0", "--max-jobs", "5"];
    let strict = plan_json(&[&base[..], &["--tolerance", "0"]].concat());
    assert_eq!(strict["recommended"]["jobs"], 3);
    assert_eq!(strict["recommended"]["wall_cost"], 50.0);

    let loose = plan_json(&[&base[..], &["--tolerance", "60"]].concat());
    assert_eq!(loose["tolerance_pct"], 60.0);
    assert_eq!(loose["recommended"]["jobs"], 2, "80s is within 60% of the 50s optimum");
    assert_eq!(loose["recommended"]["wall_cost"], 80.0);
    assert_eq!(loose["bottleneck"]["limits_wall_time"], false);
}

#[test]
fn max_jobs_caps_the_options_considered() {
    let ws = make_workspace(&[("a", 1), ("b", 1), ("c", 1)]);
    let timings = write_timings(ws.path(), &[("a", 30.0, 1), ("b", 30.0, 1), ("c", 30.0, 1)]);
    let plan = plan_json(&[
        ws.path().to_str().unwrap(),
        "--timings",
        &timings,
        "--startup-seconds",
        "0",
        "--max-jobs",
        "2",
        "--tolerance",
        "0",
    ]);
    assert_eq!(plan["options"].as_array().unwrap().len(), 2);
    assert_eq!(plan["recommended"]["jobs"], 2);
    assert_eq!(plan["recommended"]["wall_cost"], 60.0);
}

#[test]
fn an_unmeasured_member_is_estimated_from_the_measured_rate() {
    // `a` and `b` ran at 2s per test overall; `c` has 5 tests and no timing.
    let ws = make_workspace(&[("a", 1), ("b", 1), ("c", 5)]);
    let timings = write_timings(ws.path(), &[("a", 30.0, 10), ("b", 10.0, 10)]);
    let plan =
        plan_json(&[ws.path().to_str().unwrap(), "--timings", &timings, "--startup-seconds", "0"]);

    assert_eq!(plan["cost_unit"], "seconds");
    let c = &plan["members"][2];
    assert_eq!(c["name"], "c");
    assert_eq!(c["runtime_seconds"], Value::Null);
    assert_eq!(c["test_count"], 5);
    assert_eq!(c["cost_source"], "estimated");
    assert_eq!(c["cost"], 10.0);
    assert_eq!(plan["serial_cost"], 50.0);
}

#[test]
fn every_run_is_charged_a_startup_allowance_on_top_of_its_runtime() {
    // pytest's own runtime excludes interpreter and plugin start-up, which is
    // what dominates a monorepo of many small suites. Four members measured
    // at ~0s must not collapse into "one lane, zero seconds".
    let ws = make_workspace(&[("a", 1), ("b", 1), ("c", 1), ("d", 1)]);
    let timings =
        write_timings(ws.path(), &[("a", 0.0, 1), ("b", 0.0, 1), ("c", 0.0, 1), ("d", 1.0, 1)]);
    let root = ws.path().to_str().unwrap();

    let plan = plan_json(&[root, "--timings", &timings, "--max-jobs", "4", "--tolerance", "0"]);
    assert_eq!(plan["startup_seconds"], 0.5, "the default allowance");
    assert_eq!(plan["members"][0]["runtime_seconds"], 0.0, "the measurement is reported as is");
    assert_eq!(plan["members"][0]["cost"], 0.5);
    assert_eq!(plan["members"][3]["cost"], 1.5);
    assert_eq!(plan["serial_cost"], 3.0);
    // d (1.5) | a+b+c (1.5): two lanes reach the floor.
    assert_eq!(plan["recommended"]["jobs"], 2);
    assert_eq!(plan["recommended"]["wall_cost"], 1.5);

    let tuned =
        plan_json(&[root, "--timings", &timings, "--startup-seconds", "2", "--max-jobs", "4"]);
    assert_eq!(tuned["startup_seconds"], 2.0);
    assert_eq!(tuned["members"][3]["cost"], 3.0);
}

#[test]
fn a_test_count_plan_carries_no_startup_allowance() {
    // Counts and seconds are never mixed: with nothing measured there is no
    // unit to express the allowance in.
    let ws = fixture_path("tests/fixtures/uv_workspace");
    let plan = plan_json(&[ws.to_str().unwrap(), "--startup-seconds", "3"]);
    assert_eq!(plan["cost_unit"], "tests");
    assert_eq!(plan["startup_seconds"], Value::Null);
    assert_eq!(plan["members"][0]["cost"], 2.0);
}

#[test]
fn a_timings_file_that_is_not_an_inventory_is_an_error() {
    let ws = make_workspace(&[("a", 1)]);
    let bogus = ws.path().join("bogus.json");
    fs::write(&bogus, r#"{"hello": "world"}"#).expect("write bogus");
    Command::cargo_bin("pycoati")
        .expect("binary built")
        .args(["plan", ws.path().to_str().unwrap(), "--timings", bogus.to_str().unwrap()])
        .assert()
        .failure()
        .stderr(predicate::str::contains("bogus.json"))
        .stderr(predicate::str::contains("inventory"));
}

#[test]
fn zero_max_jobs_is_rejected() {
    let ws = fixture_path("tests/fixtures/uv_workspace");
    plan_fails(&[ws.to_str().unwrap(), "--max-jobs", "0"], &["--max-jobs"]);
}

#[test]
fn negative_or_non_finite_knobs_are_rejected() {
    let ws = fixture_path("tests/fixtures/uv_workspace");
    let root = ws.to_str().unwrap();
    plan_fails(&[root, "--tolerance=-1"], &["--tolerance"]);
    plan_fails(&[root, "--tolerance", "nan"], &["--tolerance"]);
    plan_fails(&[root, "--startup-seconds=-0.5"], &["--startup-seconds"]);
    plan_fails(&[root, "--startup-seconds", "inf"], &["--startup-seconds"]);
}

// --- timings edge cases ------------------------------------------------------

#[test]
fn the_same_project_in_two_timings_files_takes_the_later_measurement() {
    // Regression: two inventories naming the same project by foreign path
    // used to make the name ambiguous, silently dropping both measurements
    // and degrading the plan to test counts.
    let ws = make_workspace(&[("a", 1), ("b", 1)]);
    let old_dir = TempDir::new().expect("tempdir");
    let old = write_timings(old_dir.path(), &[("a", 99.0, 1), ("b", 99.0, 1)]);
    let new = write_timings(ws.path(), &[("a", 7.0, 1), ("b", 3.0, 1)]);
    let plan = plan_json(&[
        ws.path().to_str().unwrap(),
        "--timings",
        &old,
        "--timings",
        &new,
        "--startup-seconds",
        "0",
    ]);
    assert_eq!(plan["cost_unit"], "seconds");
    assert_eq!(plan["members"][0]["cost_source"], "measured");
    assert_eq!(plan["members"][0]["runtime_seconds"], 7.0);
    assert_eq!(plan["members"][1]["runtime_seconds"], 3.0);
}

#[test]
fn a_local_inventory_matches_members_by_path_even_when_names_collide() {
    // Two members share a `[project].name`; only the path tells them apart.
    let ws = make_workspace(&[("a", 1), ("b", 1)]);
    for member in ["a", "b"] {
        fs::write(
            ws.path().join("packages").join(member).join("pyproject.toml"),
            "[project]\nname = \"twin\"\n",
        )
        .expect("rewrite member pyproject");
    }
    let root = fs::canonicalize(ws.path()).expect("canonical root");
    let entry = |member: &str, seconds: f64| {
        json!({
            "project": {"path": root.join("packages").join(member), "name": "twin"},
            "suite": {"test_count": 1, "runtime_seconds": seconds},
        })
    };
    let payload = json!({
        "schema_version": "3",
        "workspace_root": root,
        "members": [entry("a", 4.0), entry("b", 6.0)],
    });
    let timings = ws.path().join("timings.json");
    fs::write(&timings, payload.to_string()).expect("write timings");

    let plan = plan_json(&[
        ws.path().to_str().unwrap(),
        "--timings",
        timings.to_str().unwrap(),
        "--startup-seconds",
        "0",
    ]);
    assert_eq!(plan["members"][0]["path"], "packages/a");
    assert_eq!(plan["members"][0]["runtime_seconds"], 4.0);
    assert_eq!(plan["members"][1]["runtime_seconds"], 6.0);
}

#[test]
fn a_single_project_inventory_is_accepted_as_timings() {
    let ws = make_workspace(&[("a", 1), ("b", 1)]);
    let timings = ws.path().join("a.json");
    let payload = json!({
        "schema_version": "3",
        "project": {"path": "/ci/checkout/packages/a", "name": "a"},
        "suite": {"test_count": 4, "runtime_seconds": 8.0, "line_coverage_pct": null},
        "files": [],
    });
    fs::write(&timings, payload.to_string()).expect("write timings");
    let plan = plan_json(&[
        ws.path().to_str().unwrap(),
        "--timings",
        timings.to_str().unwrap(),
        "--startup-seconds",
        "0",
    ]);
    assert_eq!(plan["members"][0]["cost_source"], "measured");
    assert_eq!(plan["members"][0]["cost"], 8.0);
    assert_eq!(plan["members"][1]["cost_source"], "estimated");
    assert_eq!(plan["members"][1]["cost"], 2.0, "1 test at the measured 2s per test");
}

#[test]
fn a_collected_count_of_zero_does_not_erase_a_suite_that_has_tests() {
    // An inventory from a run whose collection broke reports zero tests and
    // no runtime. The static count must still keep the member in the plan.
    let ws = make_workspace(&[("a", 2), ("b", 1)]);
    let timings = ws.path().join("timings.json");
    let payload = json!({"members": [{
        "project": {"path": "/ci/a", "name": "a"},
        "suite": {"test_count": 0, "runtime_seconds": null},
    }]});
    fs::write(&timings, payload.to_string()).expect("write timings");
    let plan = plan_json(&[ws.path().to_str().unwrap(), "--timings", timings.to_str().unwrap()]);
    assert_eq!(plan["skipped"].as_array().unwrap().len(), 0);
    assert_eq!(plan["members"][0]["name"], "a");
    assert_eq!(plan["members"][0]["test_count"], 2);
}

#[test]
fn a_scheduled_member_never_costs_nothing_in_a_test_count_plan() {
    // `a` was measured but against zero collected tests, so there is no rate
    // to convert `b` with and the plan falls back to counts. `a` still runs,
    // so it must not be packed as free.
    let ws = make_workspace(&[("a", 0), ("b", 3)]);
    let timings = ws.path().join("timings.json");
    let payload = json!({"members": [{
        "project": {"path": "/ci/a", "name": "a"},
        "suite": {"test_count": 0, "runtime_seconds": 5.0},
    }]});
    fs::write(&timings, payload.to_string()).expect("write timings");
    let plan = plan_json(&[ws.path().to_str().unwrap(), "--timings", timings.to_str().unwrap()]);
    assert_eq!(plan["cost_unit"], "tests");
    assert_eq!(plan["members"][0]["name"], "a");
    assert_eq!(plan["members"][0]["cost"], 1.0);
    assert_eq!(plan["members"][1]["cost"], 3.0);
}

#[test]
fn an_unreadable_or_malformed_timings_file_is_an_error() {
    let ws = make_workspace(&[("a", 1)]);
    let root = ws.path().to_str().unwrap();
    plan_fails(&[root, "--timings", "/no/such/timings.json"], &["timings.json", "failed to read"]);
    let broken = ws.path().join("broken.json");
    fs::write(&broken, "{ not json").expect("write broken");
    plan_fails(&[root, "--timings", broken.to_str().unwrap()], &["broken.json", "inventory"]);
}

// --- selection edge cases ----------------------------------------------------

#[test]
fn a_name_matching_two_members_asks_for_a_path() {
    let tmp = TempDir::new().expect("tempdir");
    fs::write(
        tmp.path().join("pyproject.toml"),
        "[tool.uv.workspace]\nmembers = [\"apps/*\", \"libs/*\"]\n",
    )
    .expect("write root pyproject");
    make_member(tmp.path(), "apps/core", 1);
    make_member(tmp.path(), "libs/core", 2);
    let root = tmp.path().to_str().unwrap();

    plan_fails(&[root, "core"], &["more than one", "apps/core", "libs/core"]);
    let plan = plan_json(&[root, "libs/core"]);
    assert_eq!(plan["members"].as_array().unwrap().len(), 1);
    assert_eq!(plan["members"][0]["path"], "libs/core");
}

#[test]
fn naming_a_member_twice_schedules_it_once() {
    let ws = fixture_path("tests/fixtures/uv_workspace");
    let plan = plan_json(&[ws.to_str().unwrap(), "pkg_a", "packages/pkg_a", "./packages/pkg_a/"]);
    assert_eq!(plan["members"].as_array().unwrap().len(), 1);
    assert_eq!(plan["serial_cost"], 2.0);
}

#[test]
fn a_plain_monorepo_rejects_missing_and_outside_subprojects() {
    let tmp = TempDir::new().expect("tempdir");
    let root = tmp.path().join("mono");
    make_member(&root, "services/api", 1);
    make_member(tmp.path(), "elsewhere", 1);
    let root = root.to_str().unwrap();
    plan_fails(&[root, "services/nope"], &["services/nope"]);
    plan_fails(&[root, "../elsewhere"], &["../elsewhere", "outside"]);
    plan_fails(&[root, "services/api/pyproject.toml"], &["not a directory"]);
}

#[test]
fn a_workspace_with_no_members_says_so() {
    let tmp = TempDir::new().expect("tempdir");
    fs::write(tmp.path().join("pyproject.toml"), "[tool.uv.workspace]\nmembers = []\n")
        .expect("write root pyproject");
    plan_fails(&[tmp.path().to_str().unwrap()], &["no members"]);
}

#[test]
fn paths_that_need_quoting_are_quoted_in_commands() {
    let tmp = TempDir::new().expect("tempdir");
    make_member(tmp.path(), "libs/my pkg", 1);
    let root = tmp.path().to_str().unwrap();

    let plan = plan_json(&[root, "libs/my pkg"]);
    let run = &plan["recommended"]["lanes"][0]["runs"][0];
    assert_eq!(run["path"], "libs/my pkg", "data fields stay raw");
    assert_eq!(run["command"], "python -m pytest 'libs/my pkg/tests'");

    let pretty = plan_pretty(&[root, "libs/my pkg", "--member-cwd", "member"]);
    assert!(pretty.contains("(cd 'libs/my pkg' && python -m pytest tests)"), "{pretty}");
}

// --- output routing ----------------------------------------------------------

#[test]
fn pretty_format_shows_the_options_table_and_the_commands() {
    let ws = make_workspace(&[("a", 1), ("b", 1), ("c", 1)]);
    let timings = write_timings(ws.path(), &[("a", 60.0, 1), ("b", 30.0, 1), ("c", 30.0, 1)]);
    let assert = Command::cargo_bin("pycoati")
        .expect("binary built")
        .args([
            "plan",
            ws.path().to_str().unwrap(),
            "--timings",
            &timings,
            "--startup-seconds",
            "0",
            "--max-jobs",
            "3",
            "--format",
            "pretty",
        ])
        .assert()
        .success();
    let out = String::from_utf8(assert.get_output().stdout.clone()).expect("utf-8 stdout");
    assert!(out.contains("pycoati plan"), "missing title: {out}");
    assert!(out.contains("Recommended: 2 parallel runs"), "missing recommendation: {out}");
    assert!(out.contains("120.0s"), "missing serial cost: {out}");
    assert!(out.contains("2.00x"), "missing speedup: {out}");
    assert!(out.contains("Lane 1"), "missing lane heading: {out}");
    assert!(out.contains("python -m pytest packages/a/tests"), "missing command: {out}");
    assert!(out.contains("next: run `pycoati guide plan`"), "missing breadcrumb: {out}");
}

#[test]
fn output_flag_writes_the_plan_to_a_file() {
    let ws = fixture_path("tests/fixtures/uv_workspace");
    let tmp = TempDir::new().expect("tempdir");
    let out = tmp.path().join("plan.json");
    Command::cargo_bin("pycoati")
        .expect("binary built")
        .args(["plan", ws.to_str().unwrap(), "--output", out.to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicate::str::is_empty());
    let v: Value = serde_json::from_str(&fs::read_to_string(&out).expect("read plan")).unwrap();
    assert_eq!(v["schema_version"], "1");
}

#[test]
fn pretty_format_covers_test_counts_skips_and_the_single_lane_case() {
    let partial = fixture_path("tests/fixtures/uv_workspace_partial");
    let out = plan_pretty(&[partial.to_str().unwrap()]);
    assert!(out.contains("cost in test counts; no measured runtimes"), "{out}");
    assert!(out.contains("packages/pkg_b — no tests directory"), "{out}");
    assert!(out.contains("Recommended: 1 run, no parallelism"), "{out}");
    assert!(out.contains("wall 1 test,"), "singular unit: {out}");

    let ws = fixture_path("tests/fixtures/uv_workspace");
    let out = plan_pretty(&[ws.to_str().unwrap(), "--max-jobs", "2", "--member-cwd", "member"]);
    assert!(out.contains("Lane 1 — 2 tests"), "{out}");
    assert!(out.contains("(cd packages/pkg_a && python -m pytest tests)"), "{out}");
}
