//! Runs against a disposable PostgreSQL cluster when TREETENDER_TEST_POSTGRES_BIN
//! names a directory with initdb, pg_ctl, postgres and psql. Skipped otherwise.
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use tempfile::TempDir;
use treetender::*;

fn run(program: &Path, args: &[&str]) -> String {
    let output = Command::new(program).args(args).output().unwrap();
    assert!(
        output.status.success(),
        "{} {args:?}: {}",
        program.display(),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

fn git(root: &Path, args: &[&str]) {
    let mut command = Command::new("git");
    command.arg("-C").arg(root).args(args);
    treetender::git::clean_environment(&mut command);
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// A cluster listening only on a Unix socket in a private temporary directory.
struct Cluster {
    bin: PathBuf,
    data: PathBuf,
    socket: PathBuf,
    _dir: TempDir,
}

impl Cluster {
    fn start() -> Option<Self> {
        let Some(bin) = std::env::var_os("TREETENDER_TEST_POSTGRES_BIN").map(PathBuf::from) else {
            eprintln!("TREETENDER_TEST_POSTGRES_BIN is unset; skipping PostgreSQL tests");
            return None;
        };
        // Unix socket paths are short-limited; keep the directory near the root.
        let dir = tempfile::Builder::new()
            .prefix("ttpg")
            .tempdir_in("/tmp")
            .unwrap();
        let data = dir.path().join("data");
        let socket = dir.path().join("s");
        fs::create_dir(&socket).unwrap();
        run(
            &bin.join("initdb"),
            &[
                "-D",
                data.to_str().unwrap(),
                "-U",
                "postgres",
                "-A",
                "trust",
                "--no-sync",
            ],
        );
        let options = format!("-k {} -c listen_addresses=''", socket.display());
        run(
            &bin.join("pg_ctl"),
            &[
                "-D",
                data.to_str().unwrap(),
                "-o",
                &options,
                "-l",
                dir.path().join("log").to_str().unwrap(),
                "-w",
                "start",
            ],
        );
        Some(Self {
            bin,
            data,
            socket,
            _dir: dir,
        })
    }

    fn sql(&self, statement: &str) -> String {
        run(
            &self.bin.join("psql"),
            &[
                "-X",
                "-q",
                "-A",
                "-t",
                "-v",
                "ON_ERROR_STOP=1",
                "-h",
                self.socket.to_str().unwrap(),
                "-U",
                "postgres",
                "-d",
                "postgres",
                "-c",
                statement,
            ],
        )
        .trim()
        .to_owned()
    }

    fn exists(&self, database: &str) -> bool {
        self.sql(&format!(
            "SELECT count(*) FROM pg_database WHERE datname = '{database}'"
        )) == "1"
    }

    fn adapter(&self, id: &str, database: &str) -> String {
        format!(
            "[[resources]]\nid='{id}'\nkind='data'\n[resources.adapter]\ntype='postgres'\ndatabase='{database}'\nhost='{}'\nuser='postgres'\ncommand=['{}']\n",
            self.socket.display(),
            self.bin.join("psql").display()
        )
    }
}

impl Drop for Cluster {
    fn drop(&mut self) {
        let _ = Command::new(self.bin.join("pg_ctl"))
            .args(["-D", self.data.to_str().unwrap(), "-m", "immediate", "stop"])
            .output();
    }
}

struct Lane {
    temp: TempDir,
    lane: PathBuf,
    engine: Engine,
}

impl Lane {
    fn new(manifest: &str) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let primary = temp.path().join("repo");
        let lane = temp.path().join("feature-x");
        fs::create_dir(&primary).unwrap();
        git(&primary, &["init", "-q", "-b", "main"]);
        git(
            &primary,
            &[
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@example.invalid",
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                "fixture",
            ],
        );
        git(
            &primary,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "lane",
                lane.to_str().unwrap(),
            ],
        );
        fs::write(
            lane.join("treetender.toml"),
            format!("version=1\n{manifest}"),
        )
        .unwrap();
        let engine = Engine::open(temp.path().join("state")).unwrap();
        Self { temp, lane, engine }
    }

    fn apply(&self, w: &Workspace, operation: Operation, selectors: Selectors) -> ApplyResult {
        let plan = self.engine.plan(&w.id, operation, selectors).unwrap();
        assert!(plan.blockers.is_empty(), "{:?}", plan.blockers);
        self.engine.apply(&plan).unwrap()
    }
}

fn only(id: &str) -> Selectors {
    Selectors {
        only: vec![id.into()],
        retain: vec![],
    }
}

#[test]
fn lane_database_is_created_retained_and_dropped_with_its_marker() {
    let Some(cluster) = Cluster::start() else {
        return;
    };
    cluster.sql("CREATE DATABASE app_development");
    let lane = Lane::new(&cluster.adapter("db", "app_development_{workspace_ident}"));
    let w = lane.engine.register(&lane.lane).unwrap();
    assert_eq!(w.name, "feature-x");
    assert!(
        lane.apply(&w, Operation::Prepare, Selectors::default())
            .success
    );
    assert!(cluster.exists("app_development_feature_x"));
    let comment = cluster.sql(
        "SELECT shobj_description(oid, 'pg_database') FROM pg_database WHERE datname = 'app_development_feature_x'",
    );
    assert!(comment.contains(&w.id), "{comment}");
    assert!(
        lane.engine
            .plan(&w.id, Operation::Prepare, Selectors::default())
            .unwrap()
            .steps
            .is_empty()
    );
    assert!(
        lane.apply(&w, Operation::Stop, Selectors::default())
            .success
    );
    assert!(cluster.exists("app_development_feature_x"));
    assert!(
        lane.apply(&w, Operation::Clean, Selectors::default())
            .success
    );
    assert!(!cluster.exists("app_development_feature_x"));
    assert!(cluster.exists("app_development"));
    drop(lane.temp);
}

#[test]
fn existing_lane_database_is_adopted_only_after_review_and_with_lane_naming() {
    let Some(cluster) = Cluster::start() else {
        return;
    };
    cluster.sql("CREATE DATABASE app_test_feature_x");
    cluster.sql("CREATE DATABASE app_test");
    let lane = Lane::new(&format!(
        "{}{}",
        cluster.adapter("test-db", "app_test_{workspace_ident}"),
        cluster.adapter("shared", "app_test")
    ));
    let w = lane.engine.register(&lane.lane).unwrap();
    let prepare = lane
        .engine
        .plan(&w.id, Operation::Prepare, only("test-db"))
        .unwrap();
    assert!(
        prepare
            .blockers
            .iter()
            .any(|b| b.contains("adopt --only test-db")),
        "{:?}",
        prepare.blockers
    );
    let shared = lane
        .engine
        .plan(&w.id, Operation::Adopt, only("shared"))
        .unwrap();
    assert!(
        shared.blockers.iter().any(|b| b.contains("lane-specific")),
        "{:?}",
        shared.blockers
    );
    let adopt = lane
        .engine
        .plan(&w.id, Operation::Adopt, only("test-db"))
        .unwrap();
    assert!(adopt.blockers.is_empty(), "{:?}", adopt.blockers);
    // A database recreated after review is a different instance.
    cluster.sql("DROP DATABASE app_test_feature_x");
    cluster.sql("CREATE DATABASE app_test_feature_x");
    assert_eq!(lane.engine.apply(&adopt).unwrap_err().code, "stale_plan");
    assert!(lane.apply(&w, Operation::Adopt, only("test-db")).success);
    assert!(lane.apply(&w, Operation::Clean, only("test-db")).success);
    assert!(!cluster.exists("app_test_feature_x"));
    assert!(cluster.exists("app_test"));
}

#[test]
fn replaced_or_vanished_databases_are_never_dropped_blindly() {
    let Some(cluster) = Cluster::start() else {
        return;
    };
    let lane = Lane::new(&cluster.adapter("db", "app_{workspace_ident}"));
    let w = lane.engine.register(&lane.lane).unwrap();
    assert!(
        lane.apply(&w, Operation::Prepare, Selectors::default())
            .success
    );
    let clean = lane
        .engine
        .plan(&w.id, Operation::Clean, Selectors::default())
        .unwrap();
    // Someone recreated the database: same name, no marker, other data.
    cluster.sql("DROP DATABASE app_feature_x");
    cluster.sql("CREATE DATABASE app_feature_x");
    assert_eq!(lane.engine.apply(&clean).unwrap_err().code, "stale_plan");
    let fresh = lane
        .engine
        .plan(&w.id, Operation::Clean, Selectors::default())
        .unwrap();
    assert!(!fresh.blockers.is_empty());
    assert!(cluster.exists("app_feature_x"));

    // A recorded database that vanished keeps its claim until a reviewed forget.
    cluster.sql("DROP DATABASE app_feature_x");
    let vanished = lane
        .engine
        .plan(&w.id, Operation::Clean, Selectors::default())
        .unwrap();
    assert!(
        vanished
            .blockers
            .iter()
            .any(|b| b.contains("reports missing")),
        "{:?}",
        vanished.blockers
    );
    assert!(lane.apply(&w, Operation::Forget, only("db")).success);
    assert!(lane.engine.status(&w.id).unwrap().resources.is_empty());
}
