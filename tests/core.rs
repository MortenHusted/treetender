use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use tempfile::TempDir;
use treetender::*;

fn git(root: &Path, args: &[&str]) {
    let mut cmd = Command::new("git");
    cmd.arg("-C").arg(root).args(args);
    treetender::git::clean_environment(&mut cmd);
    let out = cmd.output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

struct Fixture {
    temp: TempDir,
    primary: PathBuf,
    lane: PathBuf,
    state: PathBuf,
    script: PathBuf,
    python: String,
    engine: Engine,
}
impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let primary = temp.path().join("repo");
        let lane = temp.path().join("lane");
        let state = temp.path().join("state");
        fs::create_dir(&primary).unwrap();
        git(&primary, &["init", "-b", "main"]);
        fs::write(primary.join("template"), "fixture content\n").unwrap();
        git(&primary, &["add", "template"]);
        git(
            &primary,
            &[
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@example.invalid",
                "commit",
                "-m",
                "fixture",
            ],
        );
        git(
            &primary,
            &["worktree", "add", "-b", "lane", lane.to_str().unwrap()],
        );
        let script = temp.path().join("provider.py");
        fs::write(&script, PROVIDER).unwrap();
        let python = String::from_utf8(
            Command::new("python3")
                .args(["-c", "import sys;print(sys.executable)"])
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap()
        .trim()
        .to_owned();
        let engine = Engine::open(&state).unwrap();
        Self {
            temp,
            primary,
            lane,
            state,
            script,
            python,
            engine,
        }
    }
    fn manifest(&self, text: &str) {
        fs::write(
            self.lane.join("treetender.toml"),
            format!("version=1\n{text}"),
        )
        .unwrap();
    }
    fn file(&self, id: &str, source: &str, destination: &str) -> String {
        format!(
            "[[resources]]\nid='{id}'\nkind='file'\n[resources.adapter]\ntype='file'\nsource='{source}'\ndestination='{destination}'\n"
        )
    }
    fn resource(&self, id: &str, kind: &str, target: &str, ports: &[u16]) -> String {
        format!(
            "[[resources]]\nid='{id}'\nkind='{kind}'\ntarget='{target}'\nports={ports:?}\n[resources.adapter]\ntype='script'\ncommand={}\ncapabilities=['inspect','prepare','start','stop','destroy']\ntimeout_secs=3\n",
            serde_json::to_string(&[&self.python, self.script.to_str().unwrap()]).unwrap()
        )
    }
    fn register(&self) -> Workspace {
        self.engine.register(&self.lane).unwrap()
    }
    fn apply(&self, w: &Workspace, op: Operation, selectors: Selectors) -> ApplyResult {
        let plan = self.engine.plan(&w.id, op, selectors).unwrap();
        assert!(plan.blockers.is_empty(), "{:?}", plan.blockers);
        self.engine.apply(&plan).unwrap()
    }
    fn mutations(&self) -> Vec<String> {
        fs::read_to_string(self.temp.path().join("effects"))
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }
    fn only(id: &str) -> Selectors {
        Selectors {
            only: vec![id.into()],
            retain: vec![],
        }
    }
    fn proof_file(&self, target: &str) -> PathBuf {
        use sha2::{Digest, Sha256};
        self.temp
            .path()
            .join("instances")
            .join(format!("{:x}.json", Sha256::digest(target.as_bytes())))
    }
}

const PROVIDER: &str = r#"import sys,json,os,hashlib,uuid,signal
q=json.load(sys.stdin)
base=os.path.dirname(os.path.abspath(__file__))
os.makedirs(base+'/instances',exist_ok=True)
with open(base+'/requests','a') as f:json.dump(dict(q,_cwd=os.getcwd(),_lane=os.environ.get('TREETENDER_WORKSPACE_NAME')),f);f.write('\n')
p=base+'/instances/'+hashlib.sha256(q['target'].encode()).hexdigest()+'.json'
action=q['action']; rid=q['resource_id']
if os.path.exists(base+'/unknown'):
 print(json.dumps({'version':1,'proof':{'presence':'unknown'}}));sys.exit(0)
if action!='inspect':
 with open(base+'/effects','a') as f:f.write(action+':'+rid+'\n')
if action=='prepare' and os.path.exists(base+'/fail_before_'+rid):
 os.unlink(base+'/fail_before_'+rid);sys.exit(17)
if action=='prepare' and not os.path.exists(p):
 with open(p,'w') as f:json.dump({'presence':'present','instance':str(uuid.uuid4()),'owner_token':q['owner_token'],'handle':{'running':False}},f)
if action in ['start','stop']:
 with open(p) as f:v=json.load(f)
 v['handle']['running']=action=='start'
 with open(p,'w') as f:json.dump(v,f)
if action=='destroy' and os.path.exists(p):
 with open(p) as f:v=json.load(f)
 assert v['owner_token']==q['owner_token'] and v['instance']==q['receipt']['instance']
 os.unlink(p)
if action=='destroy' and os.path.exists(base+'/fail_destroy_'+rid):
 os.unlink(base+'/fail_destroy_'+rid);sys.exit(18)
if action=='prepare' and os.path.exists(base+'/fail_after_'+rid):
 os.unlink(base+'/fail_after_'+rid);sys.exit(17)
if action=='prepare' and os.path.exists(base+'/kill_after_'+rid):
 os.unlink(base+'/kill_after_'+rid);os.kill(os.getppid(),signal.SIGKILL);sys.exit(0)
if os.path.exists(p):
 with open(p) as f:proof=json.load(f)
else:proof={'presence':'missing'}
print(json.dumps({'version':1,'proof':proof}))
"#;

#[test]
fn inspect_and_plan_do_not_register_or_allocate() {
    let f = Fixture::new();
    f.manifest(&f.file("context", "template", "context"));
    let inspection = f.engine.inspect(&f.lane).unwrap();
    assert!(inspection.workspace.is_none());
    assert!(!inspection.git_dir.join("treetender-id").exists());
    assert!(
        f.engine
            .plan(
                f.lane.to_str().unwrap(),
                Operation::Prepare,
                Selectors::default()
            )
            .is_err()
    );
    let w = f.register();
    let before = f.engine.history(&w.id).unwrap().len();
    let p = f
        .engine
        .plan(&w.id, Operation::Prepare, Selectors::default())
        .unwrap();
    assert_eq!(p.steps.len(), 1);
    assert!(!f.lane.join("context").exists());
    assert_eq!(before, f.engine.history(&w.id).unwrap().len());
    assert!(f.engine.status(&w.id).unwrap().resources.is_empty());
}

#[test]
fn file_prepare_is_idempotent_stop_retains_clean_is_explicit() {
    let f = Fixture::new();
    f.manifest(&f.file("context", "template", "nested/context"));
    let w = f.register();
    assert!(
        f.apply(&w, Operation::Prepare, Selectors::default())
            .success
    );
    assert_eq!(
        fs::read_to_string(f.lane.join("nested/context")).unwrap(),
        "fixture content\n"
    );
    assert!(
        f.engine
            .plan(&w.id, Operation::Prepare, Selectors::default())
            .unwrap()
            .steps
            .is_empty()
    );
    assert!(f.apply(&w, Operation::Stop, Selectors::default()).success);
    assert!(f.lane.join("nested/context").exists());
    assert!(f.apply(&w, Operation::Clean, Selectors::default()).success);
    assert!(!f.lane.join("nested/context").exists());
    assert!(f.lane.exists());
    assert!(f.engine.status(&w.id).unwrap().resources.is_empty());
}

#[test]
fn modified_replaced_files_and_tampered_plans_are_preserved() {
    let f = Fixture::new();
    f.manifest(&f.file("context", "template", "context"));
    let w = f.register();
    let p = f
        .engine
        .plan(&w.id, Operation::Prepare, Selectors::default())
        .unwrap();
    let mut tampered = p.clone();
    tampered.steps.clear();
    assert_eq!(f.engine.apply(&tampered).unwrap_err().code, "stale_plan");
    fs::write(f.lane.join("template"), "new source").unwrap();
    assert_eq!(f.engine.apply(&p).unwrap_err().code, "stale_plan");
    assert!(
        f.apply(&w, Operation::Prepare, Selectors::default())
            .success
    );
    let clean = f
        .engine
        .plan(&w.id, Operation::Clean, Selectors::default())
        .unwrap();
    fs::rename(f.lane.join("context"), f.lane.join("original")).unwrap();
    fs::write(f.lane.join("context"), "replacement").unwrap();
    assert_eq!(f.engine.apply(&clean).unwrap_err().code, "stale_plan");
    assert!(
        !f.engine
            .plan(&w.id, Operation::Clean, Selectors::default())
            .unwrap()
            .blockers
            .is_empty()
    );
    assert_eq!(
        fs::read_to_string(f.lane.join("context")).unwrap(),
        "replacement"
    );
}

#[test]
fn symlink_parents_and_source_links_never_escape() {
    use std::os::unix::fs::symlink;
    let f = Fixture::new();
    let outside = f.temp.path().join("outside");
    fs::create_dir(&outside).unwrap();
    symlink(&outside, f.lane.join("escape")).unwrap();
    f.manifest(&f.file("context", "template", "escape/secret"));
    let w = f.register();
    assert!(
        !f.engine
            .plan(&w.id, Operation::Prepare, Selectors::default())
            .unwrap()
            .blockers
            .is_empty()
    );
    assert!(!outside.join("secret").exists());
    fs::write(outside.join("source"), "private").unwrap();
    symlink(outside.join("source"), f.lane.join("source")).unwrap();
    f.manifest(&f.file("context", "source", "context"));
    assert!(
        !f.engine
            .plan(&w.id, Operation::Prepare, Selectors::default())
            .unwrap()
            .blockers
            .is_empty()
    );
    assert!(!f.lane.join("context").exists());
}

#[test]
fn primary_source_and_owned_link_preserve_source() {
    let f = Fixture::new();
    fs::write(f.primary.join(".env"), "private fixture").unwrap();
    f.manifest("[[resources]]\nid='env'\nkind='agent'\n[resources.adapter]\ntype='file'\nsource='.env'\ndestination='.env'\nsource_root='primary'\nmode='link'\n");
    let w = f.register();
    assert!(
        f.apply(&w, Operation::Prepare, Selectors::default())
            .success
    );
    assert_eq!(
        fs::read_to_string(f.lane.join(".env")).unwrap(),
        "private fixture"
    );
    assert!(f.apply(&w, Operation::Clean, Selectors::default()).success);
    assert!(!f.lane.join(".env").exists());
    assert_eq!(
        fs::read_to_string(f.primary.join(".env")).unwrap(),
        "private fixture"
    );
}

#[test]
fn primary_checkout_is_protected_after_branch_rename() {
    let f = Fixture::new();
    fs::write(
        f.primary.join("treetender.toml"),
        format!("version=1\n{}", f.file("context", "template", "context")),
    )
    .unwrap();
    let w = f.engine.register(&f.primary).unwrap();
    assert!(w.primary);
    assert!(
        f.apply(&w, Operation::Prepare, Selectors::default())
            .success
    );
    git(&f.primary, &["branch", "-m", "renamed"]);
    let again = f.engine.register(&f.primary).unwrap();
    assert_eq!(w.id, again.id);
    assert!(again.primary);
    let clean = f
        .engine
        .plan(&w.id, Operation::Clean, Selectors::default())
        .unwrap();
    assert!(clean.steps.is_empty());
    assert!(clean.exclusions[0].reason.contains("primary"));
    assert!(f.engine.apply(&clean).unwrap().success);
    assert!(f.primary.join("context").exists());
}

#[test]
fn linked_cannot_claim_primary_target_and_late_primary_declaration_blocks_cleanup() {
    let f = Fixture::new();
    let declaration = f.resource("db", "data", "default-db", &[]);
    fs::write(
        f.primary.join("treetender.toml"),
        format!("version=1\n{declaration}"),
    )
    .unwrap();
    f.manifest(&declaration);
    let w = f.register();
    assert!(
        !f.engine
            .plan(&w.id, Operation::Prepare, Selectors::default())
            .unwrap()
            .blockers
            .is_empty()
    );
    assert!(f.mutations().is_empty());
    let f = Fixture::new();
    f.manifest(&f.resource("db", "data", "late-primary-db", &[]));
    let w = f.register();
    assert!(
        f.apply(&w, Operation::Prepare, Selectors::default())
            .success
    );
    let declaration = f
        .resource("shared", "data", "late-primary-db", &[])
        .replace("kind='data'", "kind='data'\nownership='shared'");
    fs::write(
        f.primary.join("treetender.toml"),
        format!("version=1\n{declaration}"),
    )
    .unwrap();
    assert!(
        !f.engine
            .plan(&w.id, Operation::Clean, Selectors::default())
            .unwrap()
            .blockers
            .is_empty()
    );
    // A protected reference is not an exclusive allocation claim.
    assert!(f.engine.register(&f.primary).unwrap().primary);
    assert!(f.proof_file("late-primary-db").exists());
}

#[test]
fn shared_borrowed_and_stronger_current_protection_survive_clean() {
    let f = Fixture::new();
    f.manifest(&f.resource("db", "data", "db/{workspace_id}", &[]));
    let w = f.register();
    assert!(
        f.apply(&w, Operation::Prepare, Selectors::default())
            .success
    );
    let d = f
        .resource("db", "data", "db/{workspace_id}", &[])
        .replace("kind='data'", "kind='data'\nownership='shared'");
    f.manifest(&d);
    let p = f
        .engine
        .plan(&w.id, Operation::Clean, Selectors::default())
        .unwrap();
    assert!(p.steps.is_empty());
    assert!(p.exclusions.iter().any(|e| e.reason.contains("protected")));
    assert!(f.engine.apply(&p).unwrap().success);
    assert!(f.proof_file(&format!("db/{}", w.id)).exists());
    let borrowed = f
        .file("borrowed", "template", "unowned")
        .replace("kind='file'", "kind='file'\nownership='borrowed'");
    f.manifest(&borrowed);
    let p = f
        .engine
        .plan(&w.id, Operation::Prepare, Selectors::default())
        .unwrap();
    assert!(p.steps.is_empty());
    assert!(!f.lane.join("unowned").exists());
}

#[test]
fn retained_resource_kinds_and_stop_keep_data_and_claims() {
    let f = Fixture::new();
    f.manifest(
        &(f.resource("db", "data", "db/{workspace_id}", &[43210])
            + &f.resource("storage", "custom", "storage/{workspace_id}", &[])),
    );
    let w = f.register();
    assert!(
        f.apply(&w, Operation::Prepare, Selectors::default())
            .success
    );
    assert!(f.apply(&w, Operation::Start, Selectors::default()).success);
    assert!(f.apply(&w, Operation::Stop, Selectors::default()).success);
    assert!(f.proof_file(&format!("db/{}", w.id)).exists());
    assert!(f.proof_file(&format!("storage/{}", w.id)).exists());
    assert!(
        f.apply(
            &w,
            Operation::Clean,
            Selectors {
                only: vec![],
                retain: vec!["kind:data".into()]
            }
        )
        .success
    );
    let s = f.engine.status(&w.id).unwrap();
    assert_eq!(s.resources.len(), 1);
    assert_eq!(s.resources[0].resource_id, "db");
    assert!(!f.proof_file(&format!("storage/{}", w.id)).exists());
    let second = f.temp.path().join("second");
    git(
        &f.primary,
        &["worktree", "add", "-b", "second", second.to_str().unwrap()],
    );
    fs::write(
        second.join("treetender.toml"),
        format!(
            "version=1\n{}",
            f.resource("other", "data", "other-db", &[43210])
        ),
    )
    .unwrap();
    let second = f.engine.register(&second).unwrap();
    let result = f.apply(&second, Operation::Prepare, Selectors::default());
    assert!(!result.success);
    assert_eq!(result.failure.unwrap().code, "port_conflict");
    assert!(!f.proof_file("other-db").exists());
}

#[test]
fn failure_after_effect_retains_receipt_and_retry_reconciles() {
    let f = Fixture::new();
    f.manifest(
        &(f.resource("a", "data", "a/{workspace_id}", &[])
            + &f.resource("b", "custom", "b/{workspace_id}", &[])),
    );
    let w = f.register();
    fs::write(f.temp.path().join("fail_after_b"), "").unwrap();
    let result = f.apply(&w, Operation::Prepare, Selectors::default());
    assert!(!result.success);
    assert_eq!(result.completed, vec!["a"]);
    assert!(!result.recovery.is_empty());
    let status = f.engine.status(&w.id).unwrap();
    assert_eq!(status.resources.len(), 2);
    assert_eq!(
        status
            .resources
            .iter()
            .find(|r| r.resource_id == "b")
            .unwrap()
            .state,
        "preparing"
    );
    assert_eq!(status.operations.last().unwrap().state, "failed");
    let retry = f.apply(&w, Operation::Prepare, Selectors::default());
    assert!(retry.success);
    assert_eq!(retry.completed, vec!["b"]);
    assert_eq!(
        f.mutations()
            .iter()
            .filter(|s| s.as_str() == "prepare:b")
            .count(),
        1
    );
}

#[test]
fn unknown_state_and_changed_owner_block_destroy() {
    let f = Fixture::new();
    f.manifest(&f.resource("db", "data", "owned/{workspace_id}", &[]));
    let w = f.register();
    assert!(
        f.apply(&w, Operation::Prepare, Selectors::default())
            .success
    );
    let p = f
        .engine
        .plan(&w.id, Operation::Clean, Selectors::default())
        .unwrap();
    fs::write(f.temp.path().join("unknown"), "").unwrap();
    assert_eq!(f.engine.apply(&p).unwrap_err().code, "stale_plan");
    fs::remove_file(f.temp.path().join("unknown")).unwrap();
    let path = f.proof_file(&format!("owned/{}", w.id));
    let mut value: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    value["owner_token"] = "replacement".into();
    fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
    assert!(
        !f.engine
            .plan(&w.id, Operation::Clean, Selectors::default())
            .unwrap()
            .blockers
            .is_empty()
    );
    assert!(path.exists());
    assert!(!f.mutations().iter().any(|m| m.starts_with("destroy:")));
}

#[test]
fn task_dag_directory_inputs_artifacts_and_failed_build_never_start_services() {
    let f = Fixture::new();
    fs::create_dir(f.lane.join("src")).unwrap();
    fs::write(f.lane.join("src/lib.rs"), "input").unwrap();
    f.manifest("[[tasks]]\nid='compile'\nkind='build'\ncommand=['/bin/sh','-c','printf artifact > result']\ninputs=['src']\noutputs=['result']\n[[tasks]]\nid='verify'\nkind='check'\ndepends_on=['compile']\ncommand=['/bin/sh','-c','test -s result']\n");
    let w = f.register();
    let p = f
        .engine
        .plan(&w.id, Operation::Build, Selectors::default())
        .unwrap();
    fs::write(f.lane.join("src/lib.rs"), "changed").unwrap();
    assert_eq!(f.engine.apply(&p).unwrap_err().code, "stale_plan");
    assert!(f.apply(&w, Operation::Build, Selectors::default()).success);
    let h = f.engine.history(&w.id).unwrap();
    assert!(
        h.last().unwrap().steps[0]
            .detail
            .as_ref()
            .unwrap()
            .contains("artifacts")
    );
    assert!(f.apply(&w, Operation::Check, Selectors::default()).success);
    f.manifest("[[tasks]]\nid='compile'\nkind='build'\ncommand=['/bin/sh','-c','exit 7']\n");
    assert!(!f.apply(&w, Operation::Build, Selectors::default()).success);
    assert!(f.mutations().is_empty());
}

#[test]
fn overlay_dependencies_merge_before_validation_and_cycles_fail() {
    let f = Fixture::new();
    f.manifest(&(f.file("base", "template", "base") + &f.file("child", "template", "child")));
    fs::write(
        f.lane.join(".treetender.local.toml"),
        format!(
            "version=1\n{}",
            f.file("child", "template", "child")
                .replace("kind='file'", "kind='file'\ndepends_on=['base']")
        ),
    )
    .unwrap();
    let w = f.register();
    let p = f
        .engine
        .plan(&w.id, Operation::Prepare, Fixture::only("child"))
        .unwrap();
    assert_eq!(
        p.steps.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(),
        vec!["base", "child"]
    );
    f.manifest(
        &(f.file("base", "template", "base")
            .replace("kind='file'", "kind='file'\ndepends_on=['child']")
            + &f.file("child", "template", "child")),
    );
    assert_eq!(
        f.engine.inspect(&f.lane).unwrap_err().code,
        "dependency_cycle"
    );
}

#[test]
fn retained_and_shared_dependents_block_clean_dependency() {
    let f = Fixture::new();
    let dependent = f
        .file("dependent", "template", "dependent")
        .replace("kind='file'", "kind='file'\ndepends_on=['base']");
    f.manifest(&(f.file("base", "template", "base") + &dependent));
    let w = f.register();
    assert!(
        f.apply(&w, Operation::Prepare, Selectors::default())
            .success
    );
    assert!(
        !f.engine
            .plan(&w.id, Operation::Clean, Fixture::only("base"))
            .unwrap()
            .blockers
            .is_empty()
    );
    let dependent = dependent.replace("kind='file'", "kind='file'\nownership='borrowed'");
    f.manifest(&(f.file("base", "template", "base") + &dependent));
    assert!(
        !f.engine
            .plan(&w.id, Operation::Clean, Fixture::only("base"))
            .unwrap()
            .blockers
            .is_empty()
    );
}

#[test]
fn linked_relocation_keeps_identity_and_file_receipts() {
    let f = Fixture::new();
    f.manifest(&f.file("context", "template", "context"));
    let w = f.register();
    assert!(
        f.apply(&w, Operation::Prepare, Selectors::default())
            .success
    );
    let moved = f.temp.path().join("moved");
    git(
        &f.primary,
        &[
            "worktree",
            "move",
            f.lane.to_str().unwrap(),
            moved.to_str().unwrap(),
        ],
    );
    let again = f.engine.register(&moved).unwrap();
    assert_eq!(w.id, again.id);
    assert!(
        f.apply(&again, Operation::Clean, Selectors::default())
            .success
    );
    assert!(!moved.join("context").exists());
}

#[test]
fn checkout_removal_keeps_external_provenance_and_receipt_cleanup() {
    let f = Fixture::new();
    f.manifest(&f.resource("db", "data", "removed/{workspace_id}", &[]));
    let w = f.register();
    assert!(
        f.apply(&w, Operation::Prepare, Selectors::default())
            .success
    );
    fs::remove_dir_all(&f.lane).unwrap();
    git(&f.primary, &["worktree", "prune", "--expire", "now"]);
    assert_eq!(f.engine.status(&w.id).unwrap().resources.len(), 1);
    let p = f
        .engine
        .plan(&w.id, Operation::Clean, Selectors::default())
        .unwrap();
    assert!(p.blockers.is_empty(), "{:?}", p.blockers);
    assert!(f.engine.apply(&p).unwrap().success);
    assert!(!f.proof_file(&format!("removed/{}", w.id)).exists());
}

#[test]
fn portman_uses_only_real_framed_readonly_status_and_validates_service() {
    use std::io::{Read, Write};
    use std::os::unix::net::UnixListener;
    let f = Fixture::new();
    let socket = f.temp.path().join("portman.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let thread = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut len = [0; 4];
        stream.read_exact(&mut len).unwrap();
        let mut b = vec![0; u32::from_be_bytes(len) as usize];
        stream.read_exact(&mut b).unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&b).unwrap(),
            serde_json::json!({"kind":"service_status"})
        );
        let bytes=serde_json::to_vec(&serde_json::json!({"kind":"service_statuses","services":[{"name":"root:web","state":"running"},{"name":"other:web","state":"stopped"}]})).unwrap();
        stream
            .write_all(&(bytes.len() as u32).to_be_bytes())
            .unwrap();
        stream.write_all(&bytes).unwrap();
    });
    f.manifest(&format!("[[resources]]\nid='web'\nkind='service'\nownership='borrowed'\ntarget='portman-web'\n[resources.adapter]\ntype='portman'\nservice='root:web'\nsocket='{}'\n",socket.display()));
    let w = f.register();
    let status = f.engine.status(&w.id).unwrap();
    assert_eq!(
        status.observations["web"].instance.as_deref(),
        Some("root:web")
    );
    thread.join().unwrap();
    assert!(
        f.engine
            .plan(&w.id, Operation::Stop, Selectors::default())
            .unwrap()
            .steps
            .is_empty()
    );
    let text = fs::read_to_string(f.lane.join("treetender.toml"))
        .unwrap()
        .replace("service='root:web'", "service=''");
    fs::write(f.lane.join("treetender.toml"), text).unwrap();
    assert!(f.engine.inspect(&f.lane).is_err());
}

#[test]
fn private_registry_permissions_and_symlink_refusal() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let f = Fixture::new();
    assert_eq!(
        fs::metadata(&f.state).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert_eq!(
        fs::metadata(f.state.join("registry.sqlite3"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    let unsafe_dir = f.temp.path().join("unsafe");
    fs::create_dir(&unsafe_dir).unwrap();
    fs::set_permissions(&unsafe_dir, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(Engine::open(&unsafe_dir).is_err());
    let other = f.temp.path().join("other");
    fs::create_dir(&other).unwrap();
    fs::set_permissions(&other, fs::Permissions::from_mode(0o700)).unwrap();
    symlink(
        f.state.join("registry.sqlite3"),
        other.join("registry.sqlite3"),
    )
    .unwrap();
    assert!(Engine::open(&other).is_err());
}

#[test]
fn process_worker() {
    let Ok(state) = std::env::var("TT_TEST_STATE") else {
        return;
    };
    let plan: Plan =
        serde_json::from_slice(&fs::read(std::env::var("TT_TEST_PLAN").unwrap()).unwrap()).unwrap();
    let result = Engine::open(state).and_then(|engine| engine.apply(&plan));
    if let Ok(out) = std::env::var("TT_TEST_OUT") {
        fs::write(
            out,
            match result {
                Ok(r) if r.success => "success".into(),
                Ok(_) => "partial".into(),
                Err(e) => e.code,
            },
        )
        .unwrap();
    }
}
fn worker(f: &Fixture, plan: &Path, out: &Path) -> Command {
    let mut c = Command::new(std::env::current_exe().unwrap());
    c.args(["--exact", "process_worker", "--nocapture"])
        .env("TT_TEST_STATE", &f.state)
        .env("TT_TEST_PLAN", plan)
        .env("TT_TEST_OUT", out);
    c
}

#[test]
fn cross_process_apply_serializes_and_rejects_conflicting_plans() {
    let f = Fixture::new();
    f.manifest(&f.resource("db", "data", "concurrent/{workspace_id}", &[43211]));
    let w = f.register();
    let plan = f
        .engine
        .plan(&w.id, Operation::Prepare, Selectors::default())
        .unwrap();
    let path = f.temp.path().join("plan.json");
    fs::write(&path, serde_json::to_vec(&plan).unwrap()).unwrap();
    let out1 = f.temp.path().join("out1");
    let out2 = f.temp.path().join("out2");
    let mut a = worker(&f, &path, &out1).spawn().unwrap();
    let mut b = worker(&f, &path, &out2).spawn().unwrap();
    assert!(a.wait().unwrap().success());
    assert!(b.wait().unwrap().success());
    let results = [
        fs::read_to_string(out1).unwrap(),
        fs::read_to_string(out2).unwrap(),
    ];
    assert_eq!(
        results.iter().filter(|r| r.as_str() == "success").count(),
        1
    );
    assert!(
        results
            .iter()
            .any(|r| matches!(r.as_str(), "stale_plan" | "registry_busy"))
    );
    assert_eq!(f.mutations().len(), 1);
}

#[test]
fn killed_caller_retains_intent_and_reconciles_exact_provider_token() {
    let f = Fixture::new();
    f.manifest(&f.resource("db", "data", "interrupted/{workspace_id}", &[43212]));
    let w = f.register();
    let plan = f
        .engine
        .plan(&w.id, Operation::Prepare, Selectors::default())
        .unwrap();
    let path = f.temp.path().join("plan.json");
    fs::write(&path, serde_json::to_vec(&plan).unwrap()).unwrap();
    fs::write(f.temp.path().join("kill_after_db"), "").unwrap();
    let status = worker(&f, &path, &f.temp.path().join("unused"))
        .status()
        .unwrap();
    assert!(!status.success());
    let reopened = Engine::open(&f.state).unwrap();
    let status = reopened.status(&w.id).unwrap();
    assert_eq!(status.resources[0].state, "preparing");
    assert_eq!(status.operations[0].state, "interrupted");
    assert_eq!(status.operations[0].steps[0].state, "uncertain");
    let retry = reopened
        .plan(&w.id, Operation::Prepare, Selectors::default())
        .unwrap();
    assert!(retry.blockers.is_empty(), "{:?}", retry.blockers);
    assert!(reopened.apply(&retry).unwrap().success);
    assert_eq!(f.mutations(), vec!["prepare:db"]);
}

#[test]
fn registered_borrowed_reference_protects_another_workspaces_allocation() {
    let f = Fixture::new();
    f.manifest(&f.resource("db", "data", "borrowed-lane-target", &[]));
    let owner = f.register();
    assert!(
        f.apply(&owner, Operation::Prepare, Selectors::default())
            .success
    );
    let other_path = f.temp.path().join("borrower");
    git(
        &f.primary,
        &[
            "worktree",
            "add",
            "-b",
            "borrower",
            other_path.to_str().unwrap(),
        ],
    );
    let borrowed = f
        .resource("reference", "data", "borrowed-lane-target", &[])
        .replace("kind='data'", "kind='data'\nownership='borrowed'");
    fs::write(
        other_path.join("treetender.toml"),
        format!("version=1\n{borrowed}"),
    )
    .unwrap();
    f.engine.register(&other_path).unwrap();
    assert!(
        !f.engine
            .plan(&owner.id, Operation::Clean, Selectors::default())
            .unwrap()
            .blockers
            .is_empty()
    );
    fs::remove_dir_all(&other_path).unwrap();
    git(&f.primary, &["worktree", "prune", "--expire", "now"]);
    assert!(
        !f.engine
            .plan(&owner.id, Operation::Clean, Selectors::default())
            .unwrap()
            .blockers
            .is_empty()
    );
    assert!(f.proof_file("borrowed-lane-target").exists());
}

#[test]
fn unavailable_protected_prerequisite_blocks_dependent_effects() {
    let f = Fixture::new();
    let borrowed = f
        .resource("reference", "data", "missing-reference", &[])
        .replace("kind='data'", "kind='data'\nownership='borrowed'");
    let child = f
        .file("context", "template", "context")
        .replace("kind='file'", "kind='file'\ndepends_on=['reference']");
    f.manifest(&(borrowed + &child));
    let w = f.register();
    let p = f
        .engine
        .plan(&w.id, Operation::Prepare, Selectors::default())
        .unwrap();
    assert!(!p.blockers.is_empty());
    assert!(f.engine.apply(&p).is_err());
    assert!(!f.lane.join("context").exists());
    assert!(f.mutations().is_empty());
}

#[test]
fn changing_reserved_ports_requires_cleaning_original_allocation() {
    let f = Fixture::new();
    f.manifest(&f.resource("db", "data", "ports/{workspace_id}", &[43213]));
    let w = f.register();
    assert!(
        f.apply(&w, Operation::Prepare, Selectors::default())
            .success
    );
    f.manifest(&f.resource("db", "data", "ports/{workspace_id}", &[43214]));
    assert!(
        !f.engine
            .plan(&w.id, Operation::Prepare, Selectors::default())
            .unwrap()
            .blockers
            .is_empty()
    );
    assert_eq!(
        f.engine.status(&w.id).unwrap().resources[0]
            .declaration
            .ports,
        vec![43213]
    );
    assert!(f.apply(&w, Operation::Clean, Selectors::default()).success);
}

#[test]
fn distinct_clones_do_not_share_repository_identity() {
    let f = Fixture::new();
    let original = f.engine.register(&f.primary).unwrap();
    let clone = f.temp.path().join("clone");
    git(
        f.temp.path(),
        &[
            "clone",
            f.primary.to_str().unwrap(),
            clone.to_str().unwrap(),
        ],
    );
    let second = f.engine.register(&clone).unwrap();
    assert_ne!(original.repository_id, second.repository_id);
    assert!(second.primary);
}

fn recycle_lane(f: &Fixture) {
    git(
        &f.primary,
        &["worktree", "remove", "--force", f.lane.to_str().unwrap()],
    );
    git(
        &f.primary,
        &[
            "worktree",
            "add",
            "-b",
            "replacement",
            f.lane.to_str().unwrap(),
        ],
    );
}

#[test]
fn recycled_path_resolves_marker_and_old_receipts_never_touch_new_files() {
    let f = Fixture::new();
    let declaration = f.file("context", "template", "context")
        + &f.resource("db", "data", "recycled-fixed-target", &[]);
    f.manifest(&declaration);
    let old = f.register();
    assert!(
        f.apply(&old, Operation::Prepare, Selectors::default())
            .success
    );
    recycle_lane(&f);
    f.manifest(&declaration);
    fs::write(f.lane.join("context"), "replacement checkout file").unwrap();
    assert!(!treetender::git::is_current(&old).unwrap());
    assert!(f.engine.inspect(&f.lane).unwrap().workspace.is_none());
    assert_eq!(
        f.engine
            .plan(
                f.lane.to_str().unwrap(),
                Operation::Prepare,
                Selectors::default()
            )
            .unwrap_err()
            .code,
        "unregistered"
    );
    let new = f.register();
    assert_ne!(old.id, new.id);
    assert!(treetender::git::is_current(&new).unwrap());
    assert_eq!(
        f.engine.inspect(&f.lane).unwrap().workspace.unwrap().id,
        new.id
    );
    assert_eq!(
        f.engine
            .plan(
                f.lane.to_str().unwrap(),
                Operation::Prepare,
                Selectors::default()
            )
            .unwrap()
            .workspace
            .id,
        new.id
    );
    assert!(
        f.engine
            .status(&new.id)
            .unwrap()
            .issues
            .iter()
            .any(|s| s.contains(&old.id))
    );
    let old_clean = f
        .engine
        .plan(&old.id, Operation::Clean, Selectors::default())
        .unwrap();
    assert!(old_clean.blockers.is_empty(), "{:?}", old_clean.blockers);
    assert!(f.engine.apply(&old_clean).unwrap().success);
    assert_eq!(
        fs::read_to_string(f.lane.join("context")).unwrap(),
        "replacement checkout file"
    );
    assert!(!f.proof_file("recycled-fixed-target").exists());
    assert!(f.engine.status(&old.id).unwrap().resources.is_empty());
    assert!(
        f.apply(&new, Operation::Prepare, Fixture::only("db"))
            .success
    );
    let requests = fs::read_to_string(f.temp.path().join("requests")).unwrap();
    for request in requests
        .lines()
        .map(|s| serde_json::from_str::<serde_json::Value>(s).unwrap())
        .filter(|r| r["workspace"]["id"] == old.id && r["action"] == "destroy")
    {
        assert_eq!(request["workspace"]["path"], old.path.to_str().unwrap());
        assert_eq!(
            Path::new(request["_cwd"].as_str().unwrap()),
            f.state.canonicalize().unwrap().join("adapter-recovery")
        );
    }
}

#[test]
fn stale_record_blocks_replacement_local_scripts_and_absolute_checkout_args() {
    let f = Fixture::new();
    let local = f.lane.join("local.py");
    fs::write(
        &local,
        format!(
            "import runpy\nrunpy.run_path({:?},run_name='__main__')\n",
            f.script.to_str().unwrap()
        ),
    )
    .unwrap();
    let command = serde_json::to_string(&[f.python.as_str(), "local.py"]).unwrap();
    let declaration = f
        .resource("db", "data", "local-script-target", &[])
        .replace(
            &serde_json::to_string(&[f.python.as_str(), f.script.to_str().unwrap()]).unwrap(),
            &command,
        );
    f.manifest(&(declaration + &f.resource("safe", "data", "safe-orphan-target", &[])));
    let old = f.register();
    assert!(
        f.apply(&old, Operation::Prepare, Selectors::default())
            .success
    );
    recycle_lane(&f);
    fs::write(
        &local,
        "raise Exception('replacement script must never run')",
    )
    .unwrap();
    let clean = f
        .engine
        .plan(&old.id, Operation::Clean, Selectors::default())
        .unwrap();
    assert!(
        clean
            .blockers
            .iter()
            .any(|s| s.contains("checkout_unavailable"))
    );
    assert!(f.proof_file("local-script-target").exists());
    assert!(f.engine.apply(&clean).is_err());
    assert!(!f.mutations().iter().any(|e| e.starts_with("destroy")));
    assert!(
        f.apply(&old, Operation::Clean, Fixture::only("safe"))
            .success
    );
    assert!(!f.proof_file("safe-orphan-target").exists());
    // Absolute paths into the old location are equally unsafe after recycling.
    let f = Fixture::new();
    let local = f.lane.join("local.py");
    fs::copy(&f.script, &local).unwrap();
    let command = serde_json::to_string(&[f.python.as_str(), local.to_str().unwrap()]).unwrap();
    let declaration = f.resource("db", "data", "absolute-local", &[]).replace(
        &serde_json::to_string(&[f.python.as_str(), f.script.to_str().unwrap()]).unwrap(),
        &command,
    );
    f.manifest(&declaration);
    let old = f.register();
    assert!(
        f.apply(&old, Operation::Prepare, Selectors::default())
            .success
    );
    recycle_lane(&f);
    fs::write(local, "raise Exception('new script')").unwrap();
    let p = f
        .engine
        .plan(&old.id, Operation::Clean, Selectors::default())
        .unwrap();
    assert!(p.blockers.iter().any(|s| s.contains("absent/replacement")));
}

#[test]
fn removed_checkout_file_receipt_does_not_block_external_cleanup() {
    let f = Fixture::new();
    f.manifest(
        &(f.file("context", "template", "context")
            + &f.resource("db", "data", "mixed-missing/{workspace_id}", &[])),
    );
    let w = f.register();
    assert!(
        f.apply(&w, Operation::Prepare, Selectors::default())
            .success
    );
    fs::remove_dir_all(&f.lane).unwrap();
    assert!(
        f.engine
            .plan(&w.id, Operation::Clean, Selectors::default())
            .unwrap()
            .blockers
            .iter()
            .any(|s| s.contains("checkout_location_unknown"))
    );
    git(&f.primary, &["worktree", "prune", "--expire", "now"]);
    let plan = f
        .engine
        .plan(&w.id, Operation::Clean, Selectors::default())
        .unwrap();
    assert!(plan.blockers.is_empty(), "{:?}", plan.blockers);
    assert!(f.engine.apply(&plan).unwrap().success);
    assert!(f.engine.status(&w.id).unwrap().resources.is_empty());
    assert!(!f.proof_file(&format!("mixed-missing/{}", w.id)).exists());
}

#[test]
fn symlinked_missing_checkout_is_unknown_instead_of_proven_missing() {
    let f = Fixture::new();
    f.manifest(&f.file("context", "template", "context"));
    let w = f.register();
    assert!(
        f.apply(&w, Operation::Prepare, Selectors::default())
            .success
    );
    fs::remove_dir_all(&f.lane).unwrap();
    std::os::unix::fs::symlink(f.temp.path().join("nonexistent"), &f.lane).unwrap();
    assert_eq!(
        treetender::git::is_current(&w).unwrap_err().code,
        "unsafe_path"
    );
    assert_eq!(
        f.engine
            .plan(&w.id, Operation::Clean, Selectors::default())
            .unwrap_err()
            .code,
        "unsafe_path"
    );
}

#[test]
fn inspect_strips_expected_proof_but_effects_keep_exact_allocation_receipt() {
    let f = Fixture::new();
    f.manifest(&f.resource("db", "data", "redacted-inspect/{workspace_id}", &[]));
    let w = f.register();
    assert!(
        f.apply(&w, Operation::Prepare, Selectors::default())
            .success
    );
    let receipt = f.engine.status(&w.id).unwrap().resources.remove(0);
    assert!(f.apply(&w, Operation::Clean, Selectors::default()).success);
    let log = fs::read_to_string(f.temp.path().join("requests")).unwrap();
    let requests: Vec<serde_json::Value> = log
        .lines()
        .map(|s| serde_json::from_str(s).unwrap())
        .collect();
    assert!(
        requests
            .iter()
            .any(|r| r["action"] == "inspect" && r["receipt"].is_object())
    );
    for q in &requests {
        if q["action"] == "inspect" {
            assert_eq!(q["owner_token"], "");
            assert!(q["receipt"].is_null() || q["receipt"]["owner_token"].is_null());
            assert!(q["receipt"].is_null() || q["receipt"]["instance"].is_null());
        } else {
            assert_eq!(q["owner_token"], receipt.owner_token);
            if q["action"] == "destroy" {
                assert_eq!(
                    q["receipt"]["instance"],
                    receipt.proof.instance.clone().unwrap()
                );
                assert_eq!(q["receipt"]["owner_token"], receipt.owner_token);
            }
        }
    }
}

#[test]
fn echoing_inspector_cannot_adopt_a_foreign_instance_after_uncertain_prepare() {
    let f = Fixture::new();
    f.manifest(&f.resource("db", "data", "echo/{workspace_id}", &[]));
    let w = f.register();
    fs::write(f.temp.path().join("fail_after_db"), "").unwrap();
    assert!(
        !f.apply(&w, Operation::Prepare, Selectors::default())
            .success
    );
    let path = f.proof_file(&format!("echo/{}", w.id));
    fs::write(
        &path,
        r#"{"presence":"present","instance":"foreign-instance","owner_token":"foreign-owner"}"#,
    )
    .unwrap();
    fs::write(&f.script,r#"import sys,json
q=json.load(sys.stdin)
assert q['action']=='inspect'
prior=q.get('receipt') or {}
print(json.dumps({'version':1,'proof':{'presence':'present','instance':prior.get('instance') or 'foreign-instance','owner_token':q['owner_token'],'handle':{}}}))
"#).unwrap();
    let p = f
        .engine
        .plan(&w.id, Operation::Prepare, Selectors::default())
        .unwrap();
    assert!(p.blockers.iter().any(|s| s.contains("ownership/instance")));
    assert!(f.engine.apply(&p).is_err());
    assert!(fs::read_to_string(path).unwrap().contains("foreign-owner"));
}

#[test]
fn file_destinations_cannot_modify_declarations_or_cleanup_capture_paths() {
    let f = Fixture::new();
    for destination in [
        "treetender.toml",
        ".treetender.local.toml",
        "a/.treetender-delete-x",
        "treetender.toml/nested",
        "nested/.treetender.local.toml",
    ] {
        f.manifest(&f.file("bad", "template", destination));
        assert_eq!(
            f.engine.inspect(&f.lane).unwrap_err().code,
            "unsafe_path",
            "{destination}"
        );
    }
    fs::write(f.primary.join(".treetender.local.toml"), "version=1\n").unwrap();
    f.manifest(
        &f.file("context", ".treetender.local.toml", "context")
            .replace("type='file'", "type='file'\nsource_root='primary'"),
    );
    let w = f.register();
    assert!(
        f.apply(&w, Operation::Prepare, Selectors::default())
            .success
    );
    assert!(f.primary.join(".treetender.local.toml").exists());
}

#[test]
fn separate_repositories_can_reference_the_same_shared_resource_without_claiming_it() {
    let f = Fixture::new();
    let shared = f
        .resource("pg", "service", "shared-postgres", &[43219])
        .replace("kind='service'", "kind='service'\nownership='shared'");
    fs::write(
        f.primary.join("treetender.toml"),
        format!("version=1\n{shared}"),
    )
    .unwrap();
    let first = f.engine.register(&f.primary).unwrap();
    let other = f.temp.path().join("other");
    fs::create_dir(&other).unwrap();
    git(&other, &["init", "-b", "main"]);
    fs::write(
        other.join("treetender.toml"),
        format!("version=1\n{shared}"),
    )
    .unwrap();
    let second = f.engine.register(&other).unwrap();
    assert!(f.engine.status(&first.id).unwrap().resources.is_empty());
    assert!(f.engine.status(&second.id).unwrap().resources.is_empty());
    assert!(f.mutations().is_empty());
}

#[test]
fn obsolete_primary_reservations_release_target_and_ports_without_forgetting_allocations() {
    let f = Fixture::new();
    let declaration = f.resource("cache", "data", "obsolete-reservation", &[43220]);
    fs::write(
        f.primary.join("treetender.toml"),
        format!("version=1\n{declaration}"),
    )
    .unwrap();
    let primary = f.engine.register(&f.primary).unwrap();
    assert_eq!(
        f.engine.status(&primary.id).unwrap().resources[0].state,
        "reserved"
    );
    fs::write(f.primary.join("treetender.toml"), "version=1\n").unwrap();
    f.engine.register(&f.primary).unwrap();
    assert!(f.engine.status(&primary.id).unwrap().resources.is_empty());
    f.manifest(&f.resource("cache", "data", "obsolete-reservation", &[43220]));
    let w = f.register();
    assert!(
        f.apply(&w, Operation::Prepare, Selectors::default())
            .success
    );
    // Allocations with an actual/uncertain effect survive declaration removal.
    for state in ["prepared", "preparing", "active"] {
        let f = Fixture::new();
        let declaration = f.resource("db", "data", "preserved-primary", &[43221]);
        fs::write(
            f.primary.join("treetender.toml"),
            format!("version=1\n{declaration}"),
        )
        .unwrap();
        let primary = f.engine.register(&f.primary).unwrap();
        if state == "preparing" {
            fs::write(f.temp.path().join("fail_after_db"), "").unwrap();
        }
        let result = f.apply(&primary, Operation::Prepare, Selectors::default());
        assert_eq!(result.success, state != "preparing");
        if state == "active" {
            assert!(
                f.apply(&primary, Operation::Start, Selectors::default())
                    .success
            );
        }
        fs::write(f.primary.join("treetender.toml"), "version=1\n").unwrap();
        f.engine.register(&f.primary).unwrap();
        let resources = f.engine.status(&primary.id).unwrap().resources;
        assert_eq!(resources.len(), 1);
        assert_eq!(resources[0].state, state);
        assert!(f.proof_file("preserved-primary").exists());
    }
}

#[test]
fn bare_repository_worktree_layout_requires_explicit_primary_designation() {
    let f = Fixture::new();
    let bare = f.temp.path().join("bare.git");
    git(
        f.temp.path(),
        &[
            "clone",
            "--bare",
            f.primary.to_str().unwrap(),
            bare.to_str().unwrap(),
        ],
    );
    let main = f.temp.path().join("main");
    git(&bare, &["worktree", "add", main.to_str().unwrap(), "main"]);
    for path in [&main, &bare] {
        let e = f.engine.register(path).unwrap_err();
        assert_eq!(e.code, "unsupported_bare_layout");
        assert!(e.message.contains("primary"));
    }
    assert!(f.engine.list().unwrap().is_empty());
    assert!(!bare.join("treetender-id").exists());
}

fn registry_rows(f: &Fixture) -> Vec<Vec<Vec<rusqlite::types::Value>>> {
    let db = rusqlite::Connection::open(f.state.join("registry.sqlite3")).unwrap();
    [
        "repositories",
        "workspaces",
        "declarations",
        "claims",
        "ports",
        "metadata",
        "operations",
    ]
    .iter()
    .map(|table| {
        let mut q = db
            .prepare(&format!("SELECT * FROM {table} ORDER BY 1"))
            .unwrap();
        let count = q.column_count();
        q.query_map([], |row| (0..count).map(|i| row.get(i)).collect())
            .unwrap()
            .map(|row| row.unwrap())
            .collect()
    })
    .collect()
}

#[test]
fn registration_rolls_back_all_rows_snapshots_claims_ports_and_generation_on_late_conflict() {
    let f = Fixture::new();
    f.manifest(&f.resource("taken", "data", "z-taken", &[]));
    let w = f.register();
    assert!(
        f.apply(&w, Operation::Prepare, Selectors::default())
            .success
    );
    let before = registry_rows(&f);
    let other = f.temp.path().join("other");
    fs::create_dir(&other).unwrap();
    git(&other, &["init", "-b", "main"]);
    fs::write(
        other.join("treetender.toml"),
        format!(
            "version=1\n{}{}",
            f.resource("first", "data", "a-provisional", &[43222]),
            f.resource("last", "data", "z-taken", &[])
        ),
    )
    .unwrap();
    assert_eq!(
        f.engine.register(&other).unwrap_err().code,
        "ownership_conflict"
    );
    assert_eq!(registry_rows(&f), before);
    // Rollback also restores reservations pruned earlier in the same operation.
    let primary = f
        .engine
        .list()
        .unwrap()
        .into_iter()
        .find(|w| w.primary)
        .unwrap();
    fs::write(
        f.primary.join("treetender.toml"),
        format!(
            "version=1\n{}",
            f.resource("reserve", "data", "old-reserved", &[43223])
        ),
    )
    .unwrap();
    f.engine.register(&f.primary).unwrap();
    let before = registry_rows(&f);
    fs::write(
        f.primary.join("treetender.toml"),
        format!("version=1\n{}", f.resource("last", "data", "z-taken", &[])),
    )
    .unwrap();
    assert!(f.engine.register(&f.primary).is_err());
    assert_eq!(registry_rows(&f), before);
    assert!(
        f.engine
            .status(&primary.id)
            .unwrap()
            .resources
            .iter()
            .any(|r| r.target == "external:old-reserved")
    );
}

#[test]
fn moved_checkout_with_live_git_identity_blocks_recovery_without_breaking_unrelated_reads() {
    let f = Fixture::new();
    f.manifest(
        &(f.resource("db", "data", "move-protected", &[])
            + &f.resource("free", "data", "move-independent", &[])),
    );
    let owner = f.register();
    assert!(
        f.apply(&owner, Operation::Prepare, Selectors::default())
            .success
    );
    let borrower = f.temp.path().join("borrower");
    git(
        &f.primary,
        &[
            "worktree",
            "add",
            "-b",
            "borrower",
            borrower.to_str().unwrap(),
        ],
    );
    let reference = f
        .resource("reference", "data", "move-protected", &[])
        .replace("kind='data'", "kind='data'\nownership='borrowed'");
    fs::write(
        borrower.join("treetender.toml"),
        format!("version=1\n{reference}"),
    )
    .unwrap();
    let w = f.engine.register(&borrower).unwrap();
    let moved = f.temp.path().join("moved-manually");
    fs::rename(&borrower, &moved).unwrap();
    let e = treetender::git::is_current(&w).unwrap_err();
    assert_eq!(e.code, "checkout_location_unknown");
    assert!(e.message.contains("git worktree repair"));
    let status = f.engine.status(&w.id).unwrap();
    assert!(
        status
            .issues
            .iter()
            .any(|s| s.contains("checkout_location_unknown"))
    );
    assert!(f.engine.history(&w.id).is_ok());
    let p = f
        .engine
        .plan(&w.id, Operation::Clean, Selectors::default())
        .unwrap();
    assert!(
        p.blockers
            .iter()
            .any(|s| s.contains("checkout_location_unknown"))
    );
    assert!(
        !f.engine
            .plan(&owner.id, Operation::Clean, Fixture::only("db"))
            .unwrap()
            .blockers
            .is_empty()
    );
    assert!(
        f.apply(&owner, Operation::Clean, Fixture::only("free"))
            .success
    );
    assert!(f.proof_file("move-protected").exists());
    git(&f.primary, &["worktree", "repair", moved.to_str().unwrap()]);
    let registered = f.engine.register(&moved).unwrap();
    assert_eq!(registered.id, w.id);
    assert!(treetender::git::is_current(&registered).unwrap());
}

#[test]
fn unrelated_no_effect_operation_does_not_invalidate_reviewed_plan() {
    let f = Fixture::new();
    f.manifest(&f.file("context", "template", "context"));
    let w = f.register();
    let reviewed = f
        .engine
        .plan(&w.id, Operation::Prepare, Selectors::default())
        .unwrap();
    let primary = f.engine.register(&f.primary).unwrap();
    let unrelated = f
        .engine
        .plan(&primary.id, Operation::Prepare, Selectors::default())
        .unwrap();
    assert!(unrelated.steps.is_empty());
    assert!(f.engine.apply(&unrelated).unwrap().success);
    assert!(f.engine.apply(&reviewed).unwrap().success);
    assert!(f.lane.join("context").exists());
}

#[test]
fn busy_registry_returns_bounded_error_instead_of_hanging() {
    use fs2::FileExt;
    let f = Fixture::new();
    let w = f.register();
    let plan = f
        .engine
        .plan(&w.id, Operation::Prepare, Selectors::default())
        .unwrap();
    let path = f.temp.path().join("plan.json");
    let out = f.temp.path().join("out");
    fs::write(&path, serde_json::to_vec(&plan).unwrap()).unwrap();
    let held = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(f.state.join("registry.lock"))
        .unwrap();
    held.lock_exclusive().unwrap();
    let started = std::time::Instant::now();
    assert!(worker(&f, &path, &out).status().unwrap().success());
    assert!(started.elapsed() < std::time::Duration::from_secs(15));
    assert_eq!(fs::read_to_string(out).unwrap(), "registry_busy");
}

#[test]
fn missing_external_allocation_preserves_claim_until_reviewed_forget() {
    let f = Fixture::new();
    f.manifest(&f.resource("storage", "data", "retained-provider", &[]));
    let w = f.register();
    assert!(
        f.apply(&w, Operation::Prepare, Selectors::default())
            .success
    );
    let original = f.proof_file("retained-provider");
    let retained = f.temp.path().join("outside-current-provider.json");
    fs::rename(original, &retained).unwrap();
    let clean = f
        .engine
        .plan(&w.id, Operation::Clean, Selectors::default())
        .unwrap();
    assert!(
        clean
            .blockers
            .iter()
            .any(|b| b.contains("explicitly forget"))
    );
    assert_eq!(f.engine.status(&w.id).unwrap().resources.len(), 1);
    assert!(
        f.apply(&w, Operation::Forget, Fixture::only("storage"))
            .success
    );
    assert!(retained.exists());
    assert!(f.engine.status(&w.id).unwrap().resources.is_empty());
    assert!(
        !f.mutations()
            .iter()
            .any(|effect| effect.starts_with("destroy:"))
    );
    let history = f.engine.history(&w.id).unwrap();
    assert!(
        history.last().unwrap().steps[0]
            .detail
            .as_ref()
            .unwrap()
            .contains("owner_token")
    );
}

#[test]
fn forget_edited_file_is_explicit_and_retains_its_contents() {
    let f = Fixture::new();
    f.manifest(&f.file("context", "template", "context"));
    let w = f.register();
    assert!(
        f.apply(&w, Operation::Prepare, Selectors::default())
            .success
    );
    fs::write(f.lane.join("context"), "user edits").unwrap();
    assert!(
        !f.engine
            .plan(&w.id, Operation::Forget, Selectors::default())
            .unwrap()
            .blockers
            .is_empty()
    );
    assert!(
        f.apply(&w, Operation::Forget, Fixture::only("context"))
            .success
    );
    assert_eq!(
        fs::read_to_string(f.lane.join("context")).unwrap(),
        "user edits"
    );
    assert!(f.engine.status(&w.id).unwrap().resources.is_empty());
    let prepare = f
        .engine
        .plan(&w.id, Operation::Prepare, Selectors::default())
        .unwrap();
    assert!(
        !prepare.blockers.is_empty(),
        "forgotten existing files must not be adopted by name"
    );
}

#[test]
fn retry_reconciles_prepare_before_effect_and_destroy_after_effect() {
    let f = Fixture::new();
    f.manifest(&f.resource("db", "data", "retry-intent", &[]));
    let w = f.register();
    fs::write(f.temp.path().join("fail_before_db"), "fail once").unwrap();
    assert!(
        !f.apply(&w, Operation::Prepare, Selectors::default())
            .success
    );
    assert_eq!(
        f.engine.status(&w.id).unwrap().resources[0].state,
        "preparing"
    );
    assert!(
        f.apply(&w, Operation::Prepare, Selectors::default())
            .success
    );
    fs::write(f.temp.path().join("fail_destroy_db"), "fail after effect").unwrap();
    assert!(!f.apply(&w, Operation::Clean, Selectors::default()).success);
    assert_eq!(
        f.engine.status(&w.id).unwrap().resources[0].state,
        "destroying"
    );
    assert!(!f.proof_file("retry-intent").exists());
    assert!(f.apply(&w, Operation::Clean, Selectors::default()).success);
    assert!(f.engine.status(&w.id).unwrap().resources.is_empty());
    assert_eq!(
        f.mutations()
            .iter()
            .filter(|a| a.starts_with("destroy:"))
            .count(),
        1
    );
}

#[test]
fn forget_rolls_back_claim_release_if_declaration_update_fails() {
    let f = Fixture::new();
    f.manifest(&f.resource("db", "data", "atomic-forget", &[43225]));
    let w = f.register();
    assert!(
        f.apply(&w, Operation::Prepare, Selectors::default())
            .success
    );
    git(
        &f.primary,
        &["worktree", "remove", "--force", f.lane.to_str().unwrap()],
    );
    let plan = f
        .engine
        .plan(&w.id, Operation::Forget, Fixture::only("db"))
        .unwrap();
    let db = rusqlite::Connection::open(f.state.join("registry.sqlite3")).unwrap();
    db.execute_batch("CREATE TRIGGER fail_reference_update BEFORE UPDATE ON declarations BEGIN SELECT RAISE(ABORT, 'fixture abort'); END;").unwrap();
    let result = f.engine.apply(&plan).unwrap();
    assert!(!result.success);
    assert_eq!(f.engine.status(&w.id).unwrap().resources.len(), 1);
    assert!(f.proof_file("atomic-forget").exists());
    db.execute_batch("DROP TRIGGER fail_reference_update;")
        .unwrap();
    assert!(f.apply(&w, Operation::Forget, Fixture::only("db")).success);
    assert!(f.proof_file("atomic-forget").exists());
}

#[test]
fn forget_cannot_release_a_moved_workspace_with_live_git_metadata() {
    let f = Fixture::new();
    f.manifest(&f.file("context", "template", "context"));
    let w = f.register();
    assert!(
        f.apply(&w, Operation::Prepare, Selectors::default())
            .success
    );
    let moved = f.temp.path().join("moved-outside-git");
    fs::rename(&f.lane, &moved).unwrap();
    let plan = f
        .engine
        .plan(&w.id, Operation::Forget, Fixture::only("context"))
        .unwrap();
    assert!(
        plan.blockers
            .iter()
            .any(|b| b.contains("Git identity is still live"))
    );
    assert_eq!(f.engine.apply(&plan).unwrap_err().code, "plan_blocked");
    assert!(moved.join("context").exists());
    assert_eq!(f.engine.status(&w.id).unwrap().resources.len(), 1);
}

#[test]
fn repository_environment_is_scrubbed_while_authentication_context_is_preserved() {
    let mut command = Command::new("/bin/sh");
    command.args([
        "-c",
        "test -z \"$GIT_DIR\" && test \"$GIT_SSH_COMMAND\" = fixture-auth",
    ]);
    command
        .env("GIT_DIR", "foreign-repo")
        .env("GIT_SSH_COMMAND", "fixture-auth");
    treetender::git::clean_environment(&mut command);
    assert!(command.status().unwrap().success());
}

#[test]
fn moving_the_whole_repository_never_implies_its_workspaces_are_retired() {
    let f = Fixture::new();
    f.manifest(&f.resource("db", "data", "whole-repository-move", &[]));
    let w = f.register();
    assert!(
        f.apply(&w, Operation::Prepare, Selectors::default())
            .success
    );
    fs::rename(&f.primary, f.temp.path().join("moved-repository")).unwrap();
    for operation in [Operation::Clean, Operation::Forget] {
        let plan = f
            .engine
            .plan(&w.id, operation, Fixture::only("db"))
            .unwrap();
        assert!(
            plan.blockers
                .iter()
                .any(|b| b.contains("repository location or identity"))
        );
        assert_eq!(f.engine.apply(&plan).unwrap_err().code, "plan_blocked");
    }
    assert!(f.proof_file("whole-repository-move").exists());
    assert_eq!(f.engine.status(&w.id).unwrap().resources.len(), 1);
}

#[test]
fn passive_inventory_has_no_observations_claims_or_lifecycle_effects() {
    let f = Fixture::new();
    f.manifest("[[resources]]\nid='server'\nkind='service'\ntarget='docker:shared-postgres'\nownership='shared'\nmanager='docker compose'\n[[resources]]\nid='database'\nkind='data'\ntarget='postgres:shared/lane_{workspace_id}'\ndepends_on=['server']\ndescription='Test data for this lane'\nmanager='existing workspace scripts'\n");
    let w = f.register();
    let status = f.engine.status(&w.id).unwrap();
    assert!(status.resources.is_empty());
    assert!(status.observations.is_empty());
    assert!(status.issues.is_empty());
    assert_eq!(status.declarations.resources.len(), 2);
    let database = &status.declarations.resources[0];
    assert_eq!(database.target, format!("postgres:shared/lane_{}", w.id));
    assert_eq!(
        database.description.as_deref(),
        Some("Test data for this lane")
    );
    assert_eq!(
        database.manager.as_deref(),
        Some("existing workspace scripts")
    );
    for operation in [
        Operation::Prepare,
        Operation::Build,
        Operation::Check,
        Operation::Start,
        Operation::Stop,
        Operation::Clean,
    ] {
        let plan = f
            .engine
            .plan(&w.id, operation, Selectors::default())
            .unwrap();
        assert!(plan.steps.is_empty(), "{operation:?}");
        assert!(
            plan.blockers.is_empty(),
            "{operation:?}: {:?}",
            plan.blockers
        );
        assert!(f.engine.apply(&plan).unwrap().success);
    }
    assert!(f.engine.status(&w.id).unwrap().resources.is_empty());
    assert!(f.mutations().is_empty());
}

#[test]
fn primary_passive_inventory_does_not_reserve_ports_or_allocate() {
    let f = Fixture::new();
    fs::write(f.primary.join("treetender.toml"), "version=1\n[[resources]]\nid='external'\nkind='service'\ntarget='external:tracked-server'\nports=[4567]\n").unwrap();
    let primary = f.engine.register(&f.primary).unwrap();
    assert!(f.engine.status(&primary.id).unwrap().resources.is_empty());
    f.manifest(&f.resource("owned", "service", "different-allocated-service", &[4567]));
    let w = f.register();
    assert!(
        f.apply(&w, Operation::Prepare, Selectors::default())
            .success
    );
    assert!(f.proof_file("different-allocated-service").exists());
}

#[test]
fn passive_registered_use_protects_another_lane_until_orphan_reference_is_forgotten() {
    let f = Fixture::new();
    f.manifest(&f.resource("db", "data", "shared-passive-target", &[]));
    let owner = f.register();
    assert!(
        f.apply(&owner, Operation::Prepare, Selectors::default())
            .success
    );
    let borrower = f.temp.path().join("borrower");
    git(
        &f.primary,
        &[
            "worktree",
            "add",
            "-b",
            "borrower",
            borrower.to_str().unwrap(),
        ],
    );
    fs::write(
        borrower.join("treetender.toml"),
        "version=1\n[[resources]]\nid='reference'\nkind='data'\ntarget='shared-passive-target'\n",
    )
    .unwrap();
    let w = f.engine.register(&borrower).unwrap();
    let blocked = || {
        f.engine
            .plan(&owner.id, Operation::Clean, Fixture::only("db"))
            .unwrap()
    };
    assert!(!blocked().blockers.is_empty());
    fs::remove_dir_all(&borrower).unwrap();
    git(&f.primary, &["worktree", "prune", "--expire", "now"]);
    assert!(!blocked().blockers.is_empty());
    let forget = f
        .engine
        .plan(&w.id, Operation::Forget, Fixture::only("reference"))
        .unwrap();
    assert!(forget.blockers.is_empty(), "{:?}", forget.blockers);
    assert!(f.engine.apply(&forget).unwrap().success);
    assert!(
        f.engine
            .status(&w.id)
            .unwrap()
            .declarations
            .resources
            .is_empty()
    );
    assert!(f.proof_file("shared-passive-target").exists());
    assert!(f.engine.apply(&blocked()).unwrap().success);
    assert!(!f.proof_file("shared-passive-target").exists());
}

#[test]
fn switching_to_passive_retains_managed_receipt_and_blocks_effects() {
    let f = Fixture::new();
    f.manifest(&f.resource("db", "data", "previously-managed", &[]));
    let w = f.register();
    assert!(
        f.apply(&w, Operation::Prepare, Selectors::default())
            .success
    );
    let receipt = f.engine.status(&w.id).unwrap().resources.remove(0);
    let effects = f.mutations();
    f.manifest("[[resources]]\nid='db'\nkind='data'\ntarget='previously-managed'\nmanager='manual cleanup'\n");
    f.register();
    let status = f.engine.status(&w.id).unwrap();
    assert_eq!(status.resources[0], receipt);
    assert!(status.observations.is_empty());
    assert!(status.declarations.resources[0].is_passive());
    for operation in [
        Operation::Prepare,
        Operation::Start,
        Operation::Stop,
        Operation::Clean,
    ] {
        let plan = f
            .engine
            .plan(&w.id, operation, Selectors::default())
            .unwrap();
        assert!(plan.steps.is_empty());
        assert!(f.engine.apply(&plan).unwrap().success);
    }
    assert_eq!(f.mutations(), effects);
    assert!(f.proof_file("previously-managed").exists());
    assert!(f.apply(&w, Operation::Forget, Fixture::only("db")).success);
    assert!(f.engine.status(&w.id).unwrap().resources.is_empty());
    assert!(f.proof_file("previously-managed").exists());
}

#[test]
fn passive_metadata_cannot_prove_a_managed_prerequisite_ready() {
    let f = Fixture::new();
    let reference =
        "[[resources]]\nid='database'\nkind='data'\ntarget='postgres:external-test-db'\n";
    let child = f
        .file("context", "template", "context")
        .replace("kind='file'", "kind='file'\ndepends_on=['database']");
    f.manifest(&(reference.to_owned() + &child));
    let w = f.register();
    let plan = f
        .engine
        .plan(&w.id, Operation::Prepare, Selectors::default())
        .unwrap();
    assert!(
        plan.blockers
            .iter()
            .any(|b| b.contains("passive resource has no live observation"))
    );
    assert!(f.engine.apply(&plan).is_err());
    assert!(!f.lane.join("context").exists());
    assert!(f.mutations().is_empty());
}

#[test]
fn passive_dependent_protects_parent_until_declarations_are_refreshed() {
    let f = Fixture::new();
    let parent = f.resource("server", "service", "owned-server", &[]);
    f.manifest(&(parent.clone()+"[[resources]]\nid='database'\nkind='data'\ntarget='postgres:owned-server/lane'\ndepends_on=['server']\n"));
    let w = f.register();
    assert!(
        f.apply(&w, Operation::Prepare, Selectors::default())
            .success
    );
    let blocked = || {
        f.engine
            .plan(&w.id, Operation::Clean, Fixture::only("server"))
            .unwrap()
    };
    assert!(
        blocked()
            .blockers
            .iter()
            .any(|b| b.contains("retained database"))
    );
    f.manifest(&parent);
    assert!(
        blocked()
            .blockers
            .iter()
            .any(|b| b.contains("retained database"))
    );
    f.register();
    assert!(f.engine.apply(&blocked()).unwrap().success);
    assert!(!f.proof_file("owned-server").exists());
}

#[test]
fn passive_inventory_requires_a_locator_and_does_not_adopt_on_promotion() {
    let f = Fixture::new();
    f.manifest("[[resources]]\nid='db'\nkind='data'\n");
    assert!(
        f.engine
            .register(&f.lane)
            .unwrap_err()
            .message
            .contains("passive resource needs a target")
    );
    f.manifest(&f.resource("db", "data", "existing-foreign-db", &[]));
    let w = f.register();
    assert!(
        f.apply(&w, Operation::Prepare, Selectors::default())
            .success
    );
    assert!(f.apply(&w, Operation::Forget, Fixture::only("db")).success);
    f.manifest("[[resources]]\nid='db'\nkind='data'\ntarget='existing-foreign-db'\n");
    f.register();
    f.manifest(&f.resource("db", "data", "existing-foreign-db", &[]));
    f.register();
    let plan = f
        .engine
        .plan(&w.id, Operation::Prepare, Selectors::default())
        .unwrap();
    assert!(
        plan.blockers
            .iter()
            .any(|b| b.contains("ownership/instance proof does not match"))
    );
    assert!(f.proof_file("existing-foreign-db").exists());
}

fn directory(id: &str, path: &str) -> String {
    format!(
        "[[resources]]\nid='{id}'\nkind='file'\n[resources.adapter]\ntype='directory'\npath='{path}'\n"
    )
}

#[test]
fn owned_directory_is_created_retained_on_stop_and_removed_only_on_clean() {
    let f = Fixture::new();
    f.manifest(&directory("cargo-target", "target"));
    let w = f.register();
    assert!(
        f.apply(&w, Operation::Prepare, Selectors::default())
            .success
    );
    assert!(f.lane.join("target/.treetender-owner.json").is_file());
    fs::create_dir_all(f.lane.join("target/debug/deps")).unwrap();
    fs::write(f.lane.join("target/debug/deps/libapp.rlib"), "artifact").unwrap();
    assert!(
        f.engine
            .plan(&w.id, Operation::Prepare, Selectors::default())
            .unwrap()
            .steps
            .is_empty()
    );
    assert!(f.apply(&w, Operation::Stop, Selectors::default()).success);
    let keep = Selectors {
        only: vec![],
        retain: vec!["cargo-target".into()],
    };
    assert!(f.apply(&w, Operation::Clean, keep).success);
    assert!(f.lane.join("target/debug/deps/libapp.rlib").exists());
    let usage = f.engine.usage(&w.id).unwrap();
    assert!(usage["cargo-target"] > 0);
    assert!(f.apply(&w, Operation::Clean, Selectors::default()).success);
    assert!(!f.lane.join("target").exists());
    assert!(f.lane.join("template").exists());
    assert!(f.engine.status(&w.id).unwrap().resources.is_empty());
    assert!(f.engine.usage(&w.id).unwrap().is_empty());
}

#[test]
fn existing_build_output_needs_a_reviewed_adopt_of_ignored_state() {
    let f = Fixture::new();
    f.manifest(&directory("cargo-target", "target"));
    fs::create_dir_all(f.lane.join("target/debug")).unwrap();
    fs::write(f.lane.join("target/debug/app"), "binary").unwrap();
    let w = f.register();
    let prepare = f
        .engine
        .plan(&w.id, Operation::Prepare, Selectors::default())
        .unwrap();
    assert!(
        prepare
            .blockers
            .iter()
            .any(|b| b.contains("adopt --only cargo-target")),
        "{:?}",
        prepare.blockers
    );
    let unignored = f
        .engine
        .plan(&w.id, Operation::Adopt, Fixture::only("cargo-target"))
        .unwrap();
    assert!(
        unignored.blockers.iter().any(|b| b.contains("not ignored")),
        "{:?}",
        unignored.blockers
    );
    fs::write(f.lane.join(".gitignore"), "/target\n").unwrap();
    let unselected = f
        .engine
        .plan(&w.id, Operation::Adopt, Selectors::default())
        .unwrap();
    assert!(unselected.blockers.iter().any(|b| b.contains("--only")));
    let adopt = f
        .engine
        .plan(&w.id, Operation::Adopt, Fixture::only("cargo-target"))
        .unwrap();
    assert!(adopt.blockers.is_empty(), "{:?}", adopt.blockers);
    assert_eq!(adopt.steps.len(), 1);
    assert!(!adopt.steps[0].destructive);

    // A directory replaced after review is a different instance.
    fs::rename(f.lane.join("target"), f.lane.join("reviewed")).unwrap();
    fs::create_dir(f.lane.join("target")).unwrap();
    assert_eq!(f.engine.apply(&adopt).unwrap_err().code, "stale_plan");
    assert!(!f.lane.join("target/.treetender-owner.json").exists());
    fs::remove_dir(f.lane.join("target")).unwrap();
    fs::rename(f.lane.join("reviewed"), f.lane.join("target")).unwrap();

    assert!(f.engine.apply(&adopt).unwrap().success);
    assert!(f.lane.join("target/.treetender-owner.json").is_file());
    assert_eq!(
        fs::read_to_string(f.lane.join("target/debug/app")).unwrap(),
        "binary"
    );
    let again = f
        .engine
        .plan(&w.id, Operation::Adopt, Fixture::only("cargo-target"))
        .unwrap();
    assert!(again.blockers.iter().any(|b| b.contains("forget it")));
    assert!(f.apply(&w, Operation::Clean, Selectors::default()).success);
    assert!(!f.lane.join("target").exists());
    assert!(f.lane.join(".gitignore").exists());
}

#[test]
fn adopt_refuses_primary_tracked_and_symlinked_directories() {
    use std::os::unix::fs::symlink;
    let f = Fixture::new();
    fs::write(
        f.primary.join("treetender.toml"),
        format!("version=1\n{}", directory("out", "out")),
    )
    .unwrap();
    fs::write(f.primary.join(".gitignore"), "/out\n").unwrap();
    fs::create_dir(f.primary.join("out")).unwrap();
    let primary = f.engine.register(&f.primary).unwrap();
    let plan = f
        .engine
        .plan(&primary.id, Operation::Adopt, Fixture::only("out"))
        .unwrap();
    assert!(plan.blockers.iter().any(|b| b.contains("primary")));

    f.manifest(&format!(
        "{}{}",
        directory("vendored", "vendor"),
        directory("linked", "linked")
    ));
    fs::write(f.lane.join(".gitignore"), "/vendor\n/linked\n").unwrap();
    fs::create_dir(f.lane.join("vendor")).unwrap();
    fs::write(f.lane.join("vendor/lib.rs"), "tracked").unwrap();
    git(&f.lane, &["add", "-f", "vendor/lib.rs"]);
    let outside = f.temp.path().join("outside");
    fs::create_dir(&outside).unwrap();
    fs::write(outside.join("keep"), "keep").unwrap();
    symlink(&outside, f.lane.join("linked")).unwrap();
    let w = f.register();
    let tracked = f
        .engine
        .plan(&w.id, Operation::Adopt, Fixture::only("vendored"))
        .unwrap();
    assert!(
        tracked.blockers.iter().any(|b| b.contains("tracked")),
        "{:?}",
        tracked.blockers
    );
    let linked = f
        .engine
        .plan(&w.id, Operation::Adopt, Fixture::only("linked"))
        .unwrap();
    assert!(
        linked
            .blockers
            .iter()
            .any(|b| b.contains("not a real directory")),
        "{:?}",
        linked.blockers
    );
    assert!(!outside.join(".treetender-owner.json").exists());
    assert!(f.lane.join("vendor/lib.rs").exists());
}

#[test]
fn foreign_marker_blocks_directory_cleanup_and_keeps_contents() {
    let f = Fixture::new();
    f.manifest(&directory("cargo-target", "target"));
    let w = f.register();
    assert!(
        f.apply(&w, Operation::Prepare, Selectors::default())
            .success
    );
    let clean = f
        .engine
        .plan(&w.id, Operation::Clean, Selectors::default())
        .unwrap();
    // Output copied from another lane carries that lane's owner token.
    let marker = fs::read_to_string(f.lane.join("target/.treetender-owner.json")).unwrap();
    let mut foreign: serde_json::Value = serde_json::from_str(&marker).unwrap();
    foreign["owner_token"] = uuid::Uuid::new_v4().to_string().into();
    fs::rename(f.lane.join("target"), f.lane.join("original")).unwrap();
    fs::create_dir(f.lane.join("target")).unwrap();
    fs::write(
        f.lane.join("target/.treetender-owner.json"),
        foreign.to_string(),
    )
    .unwrap();
    fs::write(f.lane.join("target/foreign"), "keep").unwrap();
    assert_eq!(f.engine.apply(&clean).unwrap_err().code, "stale_plan");
    let fresh = f
        .engine
        .plan(&w.id, Operation::Clean, Selectors::default())
        .unwrap();
    assert!(
        fresh
            .blockers
            .iter()
            .any(|b| b.contains("proof does not match")),
        "{:?}",
        fresh.blockers
    );
    assert!(f.lane.join("target/foreign").exists());
    assert!(f.lane.join("original/.treetender-owner.json").exists());
}

fn ids(manifest: &Manifest) -> Vec<&str> {
    manifest.resources.iter().map(|r| r.id.as_str()).collect()
}

#[test]
fn primary_private_overlay_reaches_linked_lanes_by_scope() {
    let f = Fixture::new();
    fs::write(
        f.primary.join(".treetender.local.toml"),
        "version=1\n[[resources]]\nid='lane-db'\nkind='data'\nscope='linked'\ntarget='db:app_{workspace_ident}'\n[[resources]]\nid='primary-only'\nkind='data'\nscope='primary'\ntarget='db:primary'\n[[resources]]\nid='db-host'\nkind='service'\nownership='shared'\ntarget='docker:db-host'\n",
    )
    .unwrap();
    f.manifest("[[resources]]\nid='lane-file'\nkind='data'\ntarget='lane-file'\n");
    fs::write(
        f.lane.join(".treetender.local.toml"),
        "version=1\n[[resources]]\nid='db-host'\nkind='service'\nownership='shared'\ntarget='docker:lane-host'\n",
    )
    .unwrap();
    let lane = f.engine.inspect(&f.lane).unwrap().manifest;
    assert_eq!(ids(&lane), ["db-host", "lane-db", "lane-file"]);
    assert_eq!(lane.resources[0].target, "docker:lane-host");
    let primary = f.engine.inspect(&f.primary).unwrap().manifest;
    assert_eq!(ids(&primary), ["db-host", "primary-only"]);

    let w = f.engine.register_named(&f.lane, Some("feature-x")).unwrap();
    let declared = f.engine.status(&w.id).unwrap().declarations;
    let db = declared
        .resources
        .iter()
        .find(|r| r.id == "lane-db")
        .unwrap();
    assert_eq!(db.target, "db:app_feature_x");

    // The inherited overlay is plan material for the lane.
    let plan = f
        .engine
        .plan(&w.id, Operation::Prepare, Selectors::default())
        .unwrap();
    let overlay = fs::read_to_string(f.primary.join(".treetender.local.toml")).unwrap();
    fs::write(
        f.primary.join(".treetender.local.toml"),
        format!("{overlay}# reviewed later\n"),
    )
    .unwrap();
    assert_eq!(f.engine.apply(&plan).unwrap_err().code, "stale_plan");
}

#[test]
fn workspace_names_are_fixed_unique_and_survive_moves() {
    let f = Fixture::new();
    f.manifest("[[resources]]\nid='db'\nkind='data'\ntarget='db:app_{workspace_ident}'\n");
    let w = f.register();
    assert_eq!(w.name, "lane");
    assert_eq!(
        f.engine
            .workspace(f.primary.to_str().unwrap())
            .unwrap()
            .name,
        "repo"
    );
    assert_eq!(
        f.engine
            .register_named(&f.lane, Some("other"))
            .unwrap_err()
            .code,
        "name_fixed"
    );
    assert_eq!(f.engine.register_named(&f.lane, Some("lane")).unwrap(), w);

    let nested = f.temp.path().join("nested");
    fs::create_dir(&nested).unwrap();
    let second = nested.join("lane");
    git(
        &f.primary,
        &["worktree", "add", "-b", "second", second.to_str().unwrap()],
    );
    assert_eq!(f.engine.register(&second).unwrap().name, "lane-2");
    let third = f.temp.path().join("Third Lane!");
    git(
        &f.primary,
        &["worktree", "add", "-b", "third", third.to_str().unwrap()],
    );
    assert_eq!(
        f.engine
            .register_named(&third, Some("lane"))
            .unwrap_err()
            .code,
        "name_conflict"
    );
    assert_eq!(
        f.engine
            .register_named(&third, Some("Bad_Name"))
            .unwrap_err()
            .code,
        "invalid_name"
    );
    assert_eq!(f.engine.register(&third).unwrap().name, "third-lane");

    let moved = f.temp.path().join("moved");
    git(
        &f.primary,
        &[
            "worktree",
            "move",
            f.lane.to_str().unwrap(),
            moved.to_str().unwrap(),
        ],
    );
    let again = f.engine.register(&moved).unwrap();
    assert_eq!(
        (again.id.as_str(), again.name.as_str()),
        (w.id.as_str(), "lane")
    );
    let declared = f.engine.status(&w.id).unwrap().declarations;
    assert_eq!(declared.resources[0].target, "db:app_lane");
}

#[test]
fn tasks_and_script_adapters_receive_lane_values() {
    let f = Fixture::new();
    f.manifest(&format!(
        "{}[[tasks]]\nid='env'\nkind='build'\ncommand=['sh','-c','env | grep ^TREETENDER_ | sort > lane.env']\noutputs=['lane.env']\n",
        f.resource("db", "data", "db:{workspace_name}", &[])
    ));
    let w = f.engine.register_named(&f.lane, Some("feature-x")).unwrap();
    assert!(
        f.apply(&w, Operation::Prepare, Selectors::default())
            .success
    );
    assert!(f.apply(&w, Operation::Build, Selectors::default()).success);
    let env = fs::read_to_string(f.lane.join("lane.env")).unwrap();
    for line in [
        format!("TREETENDER_WORKSPACE_ID={}", w.id),
        "TREETENDER_WORKSPACE_NAME=feature-x".into(),
        "TREETENDER_WORKSPACE_IDENT=feature_x".into(),
        format!("TREETENDER_REPOSITORY_ID={}", w.repository_id),
    ] {
        assert!(env.lines().any(|l| l == line), "{line} missing from {env}");
    }
    let requests = fs::read_to_string(f.temp.path().join("requests")).unwrap();
    assert!(requests.lines().all(|line| {
        let request: serde_json::Value = serde_json::from_str(line).unwrap();
        request["target"] == "db:feature-x" && request["_lane"] == "feature-x"
    }));
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

#[test]
fn port_leases_are_durable_distinct_and_released_only_explicitly() {
    let f = Fixture::new();
    let manifest = "version=1\n[ports.web]\nrange=[43100, 43103]\n[[resources]]\nid='web'\nkind='service'\ntarget='http://127.0.0.1:{port:web}'\n";
    fs::write(f.lane.join("treetender.toml"), manifest).unwrap();
    let w = f.register();
    let port = w.ports["web"];
    assert!((43100..=43103).contains(&port));
    assert_eq!(f.register().ports["web"], port);
    assert!(
        w.environment()
            .contains(&("TREETENDER_PORT_WEB".into(), port.to_string()))
    );
    let declared = f.engine.status(&w.id).unwrap().declarations;
    assert_eq!(
        declared.resources[0].target,
        format!("http://127.0.0.1:{port}")
    );

    // Another lane never gets a leased port, even while nothing listens on it.
    let second = f.temp.path().join("second");
    git(
        &f.primary,
        &["worktree", "add", "-b", "second", second.to_str().unwrap()],
    );
    fs::write(second.join("treetender.toml"), manifest).unwrap();
    let other = f.engine.register(&second).unwrap();
    assert_ne!(other.ports["web"], port);

    // Static resource ports cannot take a leased port.
    fs::write(
        second.join("treetender.toml"),
        format!(
            "version=1\n{}",
            f.resource("static", "service", "static", &[port])
        ),
    )
    .unwrap();
    let other = f.engine.register(&second).unwrap();
    assert!(other.ports.is_empty());
    let result = f.apply(&other, Operation::Prepare, Selectors::default());
    assert_eq!(result.failure.unwrap().code, "port_conflict");

    let forget = f
        .engine
        .plan(&w.id, Operation::Forget, Fixture::only("port:web"))
        .unwrap();
    assert!(forget.blockers.is_empty(), "{:?}", forget.blockers);
    assert_eq!(forget.steps[0].target, format!("lease:{}:web", w.id));
    assert!(f.engine.apply(&forget).unwrap().success);
    assert!(f.engine.workspace(&w.id).unwrap().ports.is_empty());
    assert!(f.register().ports.contains_key("web"));
}

#[test]
fn port_leases_skip_listeners_report_exhaustion_and_follow_range_changes() {
    let f = Fixture::new();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let busy = listener.local_addr().unwrap().port();
    f.manifest(&format!("[ports.web]\nrange=[{busy}, {busy}]\n"));
    assert_eq!(
        f.engine.register(&f.lane).unwrap_err().code,
        "port_exhausted"
    );
    assert!(f.engine.list().unwrap().is_empty());
    drop(listener);
    assert_eq!(f.register().ports["web"], busy);

    let moved = free_port();
    f.manifest(&format!("[ports.web]\nrange=[{moved}, {moved}]\n"));
    assert_eq!(f.register().ports["web"], moved);
    f.manifest("");
    assert!(f.register().ports.is_empty());
}

#[test]
fn templates_render_lane_values_and_keep_other_braces() {
    let f = Fixture::new();
    fs::write(
        f.primary.join("database.yml.template"),
        "default: &default\n  pool: <%= ENV.fetch(\"MAX\") { 5 } %>\ndevelopment:\n  database: app_development_{workspace_ident}\nflow: {a: 1}\nhost: {workspace_name}.app.test:{port:web}\n",
    )
    .unwrap();
    let port = free_port();
    f.manifest(&format!(
        "[ports.web]\nrange=[{port}, {port}]\n[[resources]]\nid='database-config'\nkind='file'\n[resources.adapter]\ntype='file'\nsource_root='primary'\nsource='database.yml.template'\ndestination='config/database.yml'\nmode='template'\n"
    ));
    let w = f.engine.register_named(&f.lane, Some("feature-x")).unwrap();
    assert!(
        f.apply(&w, Operation::Prepare, Selectors::default())
            .success
    );
    assert_eq!(
        fs::read_to_string(f.lane.join("config/database.yml")).unwrap(),
        format!(
            "default: &default\n  pool: <%= ENV.fetch(\"MAX\") {{ 5 }} %>\ndevelopment:\n  database: app_development_feature_x\nflow: {{a: 1}}\nhost: feature-x.app.test:{port}\n"
        )
    );
    // Edits to the rendered file are preserved, like copies.
    fs::write(f.lane.join("config/database.yml"), "edited").unwrap();
    let clean = f
        .engine
        .plan(&w.id, Operation::Clean, Selectors::default())
        .unwrap();
    assert!(!clean.blockers.is_empty());
    assert_eq!(
        fs::read_to_string(f.lane.join("config/database.yml")).unwrap(),
        "edited"
    );

    fs::write(f.primary.join("other.template"), "port {port:api}\n").unwrap();
    f.manifest(&format!(
        "[ports.web]\nrange=[{port}, {port}]\n[[resources]]\nid='other'\nkind='file'\n[resources.adapter]\ntype='file'\nsource_root='primary'\nsource='other.template'\ndestination='other'\nmode='template'\n"
    ));
    let undeclared = f
        .engine
        .plan(&w.id, Operation::Prepare, Selectors::default())
        .unwrap();
    assert!(
        undeclared.blockers.iter().any(|b| b.contains("{port:api}")),
        "{:?}",
        undeclared.blockers
    );
}

#[test]
fn directory_links_and_optional_sources_follow_the_primary_checkout() {
    let f = Fixture::new();
    fs::create_dir_all(f.primary.join(".claude/skills/review")).unwrap();
    fs::write(f.primary.join(".claude/skills/review/SKILL.md"), "skill").unwrap();
    let link = |id: &str, path: &str| {
        format!(
            "[[resources]]\nid='{id}'\nkind='agent'\n[resources.adapter]\ntype='file'\nsource_root='primary'\nsource='{path}'\ndestination='{path}'\nmode='link'\noptional=true\n"
        )
    };
    f.manifest(&format!(
        "{}{}",
        link("skills", ".claude/skills"),
        link("codex", ".codex")
    ));
    let w = f.register();
    let plan = f
        .engine
        .plan(&w.id, Operation::Prepare, Selectors::default())
        .unwrap();
    assert!(plan.blockers.is_empty(), "{:?}", plan.blockers);
    assert!(
        plan.exclusions
            .iter()
            .any(|e| e.id == "codex" && e.reason.contains("optional source is absent"))
    );
    assert!(f.engine.apply(&plan).unwrap().success);
    assert_eq!(
        fs::read_to_string(f.lane.join(".claude/skills/review/SKILL.md")).unwrap(),
        "skill"
    );
    assert!(
        fs::symlink_metadata(f.lane.join(".claude/skills"))
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert!(!f.lane.join(".codex").exists());
    assert!(f.apply(&w, Operation::Clean, Selectors::default()).success);
    assert!(fs::symlink_metadata(f.lane.join(".claude/skills")).is_err());
    assert_eq!(
        fs::read_to_string(f.primary.join(".claude/skills/review/SKILL.md")).unwrap(),
        "skill"
    );
}
