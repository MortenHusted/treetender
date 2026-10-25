use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};
use tempfile::TempDir;

fn binary() -> Command {
    Command::new(env!("CARGO_BIN_EXE_treetender"))
}

struct Fixture {
    _temp: TempDir,
    primary: PathBuf,
    lane: PathBuf,
    state: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let primary = temp.path().join("primary");
        let lane = temp.path().join("lane");
        let state = temp.path().join("registry");
        fs::create_dir(&primary).unwrap();
        git(&primary, &["init", "-b", "main"]);
        git(
            &primary,
            &["config", "user.email", "fixture@example.invalid"],
        );
        git(&primary, &["config", "user.name", "TreeTender fixture"]);
        fs::write(primary.join("README.md"), "private fixture input\n").unwrap();
        fs::write(
            primary.join("treetender.toml"),
            r#"version = 1
[[resources]]
id = "context"
kind = "agent"
[resources.adapter]
type = "file"
source = "README.md"
destination = "generated/AGENTS.md"
mode = "copy"

[[tasks]]
id = "compile"
kind = "build"
command = ["sh", "-c", "mkdir -p artifacts && printf built > artifacts/app"]
inputs = ["README.md"]
outputs = ["artifacts/app"]
"#,
        )
        .unwrap();
        git(&primary, &["add", "README.md", "treetender.toml"]);
        git(
            &primary,
            &["-c", "core.hooksPath=/dev/null", "commit", "-m", "fixture"],
        );
        // Disposable Git fixture, deliberately unrelated to the user's managed lanes.
        git(
            &primary,
            &[
                "worktree",
                "add",
                "-b",
                "fixture-lane",
                lane.to_str().unwrap(),
            ],
        );
        Self {
            _temp: temp,
            primary,
            lane,
            state,
        }
    }
    fn call(&self, path: &Path, args: &[&str]) -> Output {
        binary()
            .arg("--state-dir")
            .arg(&self.state)
            .arg("--repo")
            .arg(path)
            .arg("--json")
            .args(args)
            .output()
            .unwrap()
    }
    fn ok(&self, path: &Path, args: &[&str]) -> Value {
        let output = self.call(path, args);
        assert!(
            output.status.success(),
            "{:?}: {} {}",
            args,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice::<Value>(&output.stdout).unwrap()["result"].clone()
    }
    fn reviewed_call(&self, path: &Path, args: &[&str]) -> Output {
        let plan = self.ok(path, args);
        let mut command = args.to_vec();
        command.extend([
            "--apply",
            "--expect-fingerprint",
            plan["fingerprint"].as_str().unwrap(),
        ]);
        self.call(path, &command)
    }
    fn reviewed(&self, path: &Path, args: &[&str]) -> Value {
        let output = self.reviewed_call(path, args);
        assert!(
            output.status.success(),
            "{} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice::<Value>(&output.stdout).unwrap()["result"].clone()
    }
}

fn git(path: &Path, args: &[&str]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(args)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "Git {:?}: {}",
        args,
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn agent_discovery_is_structured_and_does_not_open_registry() {
    let temp = tempfile::tempdir().unwrap();
    let state = temp.path().join("uncreated");
    let output = binary()
        .arg("--state-dir")
        .arg(&state)
        .args(["--json", "agent", "schema"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let json: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["version"], 1);
    assert_eq!(json["result"]["protocol"], "treetender-cli/v1");
    assert!(!state.exists());
    let output = binary()
        .args(["--json", "agent", "read", "treetender://adapter"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let json: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(
        json["result"]["text"]
            .as_str()
            .unwrap()
            .contains("ownership")
    );
}

#[test]
fn invalid_agent_arguments_return_json_and_nonzero_status() {
    let output = binary()
        .args(["--json", "unknown-command"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    let json: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(json["ok"], false);
    assert_eq!(json["error"]["code"], "invalid_arguments");
}

#[test]
fn init_preserves_existing_manifest() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("treetender.toml");
    fs::write(&path, "valuable existing config").unwrap();
    let output = binary()
        .arg("--repo")
        .arg(temp.path())
        .args(["--json", "init"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert_eq!(
        fs::read_to_string(path).unwrap(),
        "valuable existing config"
    );
}

#[test]
fn full_cli_flow_retains_on_stop_and_protects_primary_on_clean() {
    let fixture = Fixture::new();
    let primary = fixture.ok(&fixture.primary, &["register"]);
    assert_eq!(primary["primary"], true);
    fixture.ok(&fixture.primary, &["prepare", "--apply"]);
    let lane = fixture.ok(&fixture.lane, &["register"]);
    assert_eq!(lane["primary"], false);
    let plan = fixture.ok(&fixture.lane, &["prepare"]);
    assert_eq!(plan["operation"], "prepare");
    assert!(!fixture.lane.join("generated/AGENTS.md").exists());
    fixture.ok(&fixture.lane, &["prepare", "--apply"]);
    assert!(fixture.lane.join("generated/AGENTS.md").exists());
    fixture.ok(&fixture.lane, &["build", "--apply"]);
    let status = fixture.ok(&fixture.lane, &["status"]);
    assert_eq!(status["builds"][0]["state"], "fresh");
    assert_eq!(
        fs::read_to_string(fixture.lane.join("artifacts/app")).unwrap(),
        "built"
    );
    fs::write(
        fixture.lane.join("README.md"),
        "updated source after build\n",
    )
    .unwrap();
    let status = fixture.ok(&fixture.lane, &["status"]);
    assert_eq!(status["builds"][0]["state"], "stale");
    fixture.ok(&fixture.lane, &["stop", "--apply"]);
    assert!(fixture.lane.join("generated/AGENTS.md").exists());
    fixture.reviewed(&fixture.lane, &["clean", "--keep", "context"]);
    assert!(fixture.lane.join("generated/AGENTS.md").exists());
    let unreviewed = fixture.call(&fixture.lane, &["clean", "--apply"]);
    assert!(!unreviewed.status.success());
    assert!(fixture.lane.join("generated/AGENTS.md").exists());
    fixture.reviewed(&fixture.lane, &["clean"]);
    assert!(!fixture.lane.join("generated/AGENTS.md").exists());
    fixture.reviewed(&fixture.primary, &["clean"]);
    assert!(fixture.primary.join("generated/AGENTS.md").exists());
    assert_eq!(
        fs::read_to_string(fixture.primary.join("README.md")).unwrap(),
        "private fixture input\n"
    );
    let history = fixture.ok(&fixture.lane, &["history"]);
    assert!(!history.as_array().unwrap().is_empty());
}

#[test]
fn saved_plan_refuses_changed_inputs_and_preserves_edited_output() {
    let fixture = Fixture::new();
    fixture.ok(&fixture.lane, &["register"]);
    let saved = fixture._temp.path().join("prepare.json");
    fixture.ok(
        &fixture.lane,
        &["plan", "prepare", "--out", saved.to_str().unwrap()],
    );
    fs::write(fixture.lane.join("README.md"), "changed after review").unwrap();
    let result = fixture.call(&fixture.lane, &["apply", saved.to_str().unwrap()]);
    assert!(!result.status.success());
    assert!(!fixture.lane.join("generated/AGENTS.md").exists());
    fixture.ok(&fixture.lane, &["prepare", "--apply"]);
    fs::write(fixture.lane.join("generated/AGENTS.md"), "user edits").unwrap();
    let _ = fixture.reviewed_call(&fixture.lane, &["clean"]);
    assert_eq!(
        fs::read_to_string(fixture.lane.join("generated/AGENTS.md")).unwrap(),
        "user edits"
    );
}

#[test]
fn tui_refuses_nonterminal_and_returns_structured_error() {
    let fixture = Fixture::new();
    let output = fixture.call(&fixture.primary, &["tui"]);
    assert!(!output.status.success());
    let json: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(json["ok"], false);
    assert!(
        json["error"]["message"]
            .as_str()
            .unwrap()
            .contains("terminal")
    );
}

#[test]
fn failed_build_returns_nonzero_with_durable_structured_outcome() {
    let fixture = Fixture::new();
    let path = fixture.lane.join("treetender.toml");
    let mut manifest = fs::read_to_string(&path).unwrap();
    manifest
        .push_str("\n[[tasks]]\nid='failed-check'\nkind='check'\ncommand=['sh','-c','exit 17']\n");
    fs::write(path, manifest).unwrap();
    fixture.ok(&fixture.lane, &["register"]);
    let output = fixture.call(&fixture.lane, &["check", "--apply"]);
    assert!(!output.status.success());
    let json: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["ok"], false);
    assert_eq!(json["result"]["success"], false);
    assert_eq!(json["result"]["failure"]["code"], "command_failed");
    let history = fixture.ok(&fixture.lane, &["history"]);
    assert_eq!(
        history.as_array().unwrap().last().unwrap()["state"],
        "failed"
    );
    assert!(!fixture.lane.join("generated/AGENTS.md").exists());
}

#[test]
fn saved_build_plan_rejects_a_different_executable_on_path() {
    use std::os::unix::fs::PermissionsExt;
    let fixture = Fixture::new();
    fs::write(
        fixture.lane.join("treetender.toml"),
        "version=1\n[[tasks]]\nid='build'\nkind='build'\ncommand=['fixture-builder']\n",
    )
    .unwrap();
    fixture.ok(&fixture.lane, &["register"]);
    let first = fixture._temp.path().join("first");
    let second = fixture._temp.path().join("second");
    fs::create_dir(&first).unwrap();
    fs::create_dir(&second).unwrap();
    for (dir, text) in [(&first, "first"), (&second, "second")] {
        let path = dir.join("fixture-builder");
        fs::write(&path, format!("#!/bin/sh\nprintf {text} > ran-builder\n")).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
    }
    let saved = fixture._temp.path().join("build-plan.json");
    let invoke = |directory: &Path, args: &[&str]| {
        let path = std::env::join_paths(
            std::iter::once(directory.to_path_buf())
                .chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
        )
        .unwrap();
        binary()
            .arg("--state-dir")
            .arg(&fixture.state)
            .arg("--repo")
            .arg(&fixture.lane)
            .arg("--json")
            .args(args)
            .env("PATH", path)
            .output()
            .unwrap()
    };
    let plan = invoke(&first, &["plan", "build", "--out", saved.to_str().unwrap()]);
    assert!(
        plan.status.success(),
        "{}",
        String::from_utf8_lossy(&plan.stderr)
    );
    let output = invoke(&second, &["apply", saved.to_str().unwrap()]);
    assert!(!output.status.success());
    let error: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(error["error"]["code"], "stale_plan");
    assert!(!fixture.lane.join("ran-builder").exists());
}
