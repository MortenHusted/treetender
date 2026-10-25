mod cli;
mod tui;

use anyhow::{Context, Result, bail};
use clap::Parser;
use cli::{AgentCommand, Cli, Command, Lifecycle, Selection};
use serde::Serialize;
use serde_json::{Value, json};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};
use treetender::{Engine, Operation, Plan, Selectors};

fn main() {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(error) => {
            let help = matches!(
                error.kind(),
                clap::error::ErrorKind::DisplayHelp | clap::error::ErrorKind::DisplayVersion
            );
            if !help && std::env::args_os().any(|arg| arg == "--json") {
                eprintln!(
                    "{}",
                    json!({"version":1,"ok":false,"error":{"code":"invalid_arguments","message":error.to_string()}})
                );
                std::process::exit(2);
            }
            error.exit();
        }
    };
    let structured = cli.json;
    if let Err(error) = run(cli) {
        let core = error.downcast_ref::<treetender::Error>();
        if structured {
            eprintln!(
                "{}",
                json!({"version":1,"ok":false,"error":{"code":core.map(|e| e.code.as_str()).unwrap_or("command_failed"),"message":format!("{error:#}")}})
            );
        } else {
            eprintln!("TreeTender: {error:#}");
        }
        std::process::exit(1);
    }
}

fn run(cli: Cli) -> Result<()> {
    match &cli.command {
        Command::Agent { command } => return output(agent_resource(command)?, cli.json),
        Command::Init => {
            let path = cli.repo.join("treetender.toml");
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
                .with_context(|| {
                    format!(
                        "create {} (existing manifests are preserved)",
                        path.display()
                    )
                })?;
            file.write_all(include_bytes!("../examples/minimal.toml"))?;
            return output(
                json!({"manifest":path,"next":"inspect, register, then prepare to review the plan"}),
                cli.json,
            );
        }
        _ => {}
    }
    let state = cli.state_dir.unwrap_or_else(default_state_dir);
    let engine = Engine::open(&state)?;
    let workspace = cli.repo.to_string_lossy().into_owned();
    let value = match cli.command {
        Command::Inspect => serde_json::to_value(engine.inspect(&cli.repo)?)?,
        Command::Register => {
            let registered = engine.register(&cli.repo)?;
            if !cli.json {
                println!(
                    "Registered {}{}\nWorkspace {}",
                    registered.path.display(),
                    if registered.primary {
                        " [primary: protected]"
                    } else {
                        ""
                    },
                    registered.id
                );
                return Ok(());
            }
            serde_json::to_value(registered)?
        }
        Command::Status => {
            let status = engine.status(&workspace)?;
            if !cli.json {
                println!(
                    "{}{}",
                    status.workspace.path.display(),
                    if status.workspace.primary {
                        " [primary: protected]"
                    } else {
                        ""
                    }
                );
                for receipt in &status.resources {
                    println!(
                        "  {} · {} · {:?} · {}",
                        receipt.resource_id,
                        receipt.state,
                        receipt.declaration.ownership,
                        receipt.target
                    );
                }
                for resource in &status.declarations.resources {
                    if resource.is_passive() {
                        println!(
                            "  {} · passive · {:?} · {}",
                            resource.id, resource.ownership, resource.target
                        );
                        if let Some(description) = &resource.description {
                            println!("    purpose: {description}");
                        }
                        if let Some(manager) = &resource.manager {
                            println!("    manager: {manager}");
                        }
                    }
                }
                for (id, proof) in &status.observations {
                    println!("  observed {id}: {:?}", proof.presence);
                }
                for issue in &status.issues {
                    println!("  issue: {issue}");
                }
                for build in &status.builds {
                    println!(
                        "  task {}: {:?} · {}",
                        build.task_id, build.state, build.reason
                    );
                }
                return Ok(());
            }
            serde_json::to_value(status)?
        }
        Command::List => {
            let workspaces = engine.list()?;
            if !cli.json {
                for workspace in &workspaces {
                    println!(
                        "{}  {}{}",
                        workspace.id,
                        workspace.path.display(),
                        if workspace.primary {
                            " [primary: protected]"
                        } else {
                            ""
                        }
                    );
                }
                if workspaces.is_empty() {
                    println!("No workspaces registered. Use inspect, then register.");
                }
                return Ok(());
            }
            serde_json::to_value(workspaces)?
        }
        Command::History => serde_json::to_value(engine.history(&workspace)?)?,
        Command::Plan {
            operation,
            selection,
            out,
        } => {
            let plan = engine.plan(&workspace, operation.into(), selection.into())?;
            if let Some(path) = out {
                let mut file = fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&path)
                    .with_context(|| {
                        format!(
                            "save plan {} (choose a new path to preserve an existing plan)",
                            path.display()
                        )
                    })?;
                serde_json::to_writer_pretty(&mut file, &plan)?;
                file.write_all(b"\n")?;
            }
            return output_plan(&plan, cli.json);
        }
        Command::Apply { plan } => {
            let plan: Plan = serde_json::from_slice(
                &fs::read(&plan).with_context(|| format!("read plan {}", plan.display()))?,
            )?;
            return apply(&engine, &plan, cli.json);
        }
        Command::Prepare(action) => {
            return action_run(&engine, &workspace, Lifecycle::Prepare, action, cli.json);
        }
        Command::Build(action) => {
            return action_run(&engine, &workspace, Lifecycle::Build, action, cli.json);
        }
        Command::Check(action) => {
            return action_run(&engine, &workspace, Lifecycle::Check, action, cli.json);
        }
        Command::Start(action) => {
            return action_run(&engine, &workspace, Lifecycle::Start, action, cli.json);
        }
        Command::Stop(action) => {
            return action_run(&engine, &workspace, Lifecycle::Stop, action, cli.json);
        }
        Command::Clean(action) => {
            return action_run(&engine, &workspace, Lifecycle::Clean, action, cli.json);
        }
        Command::Forget(action) => {
            return action_run(&engine, &workspace, Lifecycle::Forget, action, cli.json);
        }
        Command::Tui => {
            if cli.json {
                bail!("the TUI requires a terminal; use status/plan/apply with --json for agents");
            }
            return tui::run(engine, workspace);
        }
        Command::Agent { .. } | Command::Init => unreachable!(),
    };
    output(value, cli.json)
}

fn action_run(
    engine: &Engine,
    workspace: &str,
    operation: Lifecycle,
    action: cli::Action,
    structured: bool,
) -> Result<()> {
    let plan = engine.plan(workspace, operation.into(), action.selection.into())?;
    if action.apply {
        if action
            .expect_fingerprint
            .as_ref()
            .is_some_and(|expected| expected != &plan.fingerprint)
        {
            return Err(treetender::Error::new(
                "stale_plan",
                "the reviewed fingerprint differs from the current plan; review a new plan",
            )
            .into());
        }
        if matches!(operation, Lifecycle::Clean | Lifecycle::Forget)
            && action.expect_fingerprint.as_deref() != Some(&plan.fingerprint)
        {
            return Err(treetender::Error::new("review_required","review a plan first; use apply FILE, or repeat the command with --apply --expect-fingerprint FINGERPRINT").into());
        }
        apply(engine, &plan, structured)
    } else {
        output_plan(&plan, structured)
    }
}

fn output_plan(plan: &Plan, structured: bool) -> Result<()> {
    if structured {
        return output(plan, true);
    }
    println!(
        "{:?} · {}{}",
        plan.operation,
        plan.workspace.path.display(),
        if plan.workspace.primary {
            " [primary: protected]"
        } else {
            ""
        }
    );
    for step in &plan.steps {
        println!(
            "  {} {} → {}",
            if step.destructive { "REMOVE" } else { "do" },
            step.id,
            step.target
        );
    }
    for exclusion in &plan.exclusions {
        println!("  keep {}: {}", exclusion.id, exclusion.reason);
    }
    for blocker in &plan.blockers {
        println!("  BLOCKED: {blocker}");
    }
    println!(
        "{} steps, {} exclusions, {} blockers",
        plan.steps.len(),
        plan.exclusions.len(),
        plan.blockers.len()
    );
    println!("Fingerprint: {}", plan.fingerprint);
    println!(
        "Plan only. Apply a saved plan, or repeat clean/forget with --apply --expect-fingerprint FINGERPRINT."
    );
    Ok(())
}

fn apply(engine: &Engine, plan: &Plan, structured: bool) -> Result<()> {
    let result = engine.apply(plan)?;
    let success = result.success;
    if structured {
        println!(
            "{}",
            serde_json::to_string(&json!({"version":1,"ok":success,"result":result}))?
        );
    } else {
        output(result, false)?;
    }
    if !success {
        std::process::exit(1);
    }
    Ok(())
}

fn output(value: impl Serialize, structured: bool) -> Result<()> {
    let value = serde_json::to_value(value)?;
    if structured {
        println!(
            "{}",
            serde_json::to_string(&json!({"version":1,"ok":true,"result":value}))?
        );
    } else {
        println!("{}", serde_json::to_string_pretty(&value)?);
    }
    Ok(())
}

fn default_state_dir() -> PathBuf {
    if let Some(path) = std::env::var_os("XDG_STATE_HOME") {
        return PathBuf::from(path).join("treetender");
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(".").to_path_buf());
    if cfg!(target_os = "macos") {
        home.join("Library/Application Support/treetender")
    } else {
        home.join(".local/state/treetender")
    }
}

impl From<Lifecycle> for Operation {
    fn from(operation: Lifecycle) -> Self {
        match operation {
            Lifecycle::Prepare => Self::Prepare,
            Lifecycle::Build => Self::Build,
            Lifecycle::Check => Self::Check,
            Lifecycle::Start => Self::Start,
            Lifecycle::Stop => Self::Stop,
            Lifecycle::Clean => Self::Clean,
            Lifecycle::Forget => Self::Forget,
        }
    }
}

impl From<Selection> for Selectors {
    fn from(selection: Selection) -> Self {
        Self {
            only: selection.only,
            retain: selection.keep,
        }
    }
}

fn agent_resource(command: &AgentCommand) -> Result<Value> {
    match command {
        AgentCommand::Schema => Ok(json!({
            "protocol":"treetender-cli/v1",
            "output":{"version":1,"ok":"boolean","result":"command-specific JSON","error":{"code":"stable error code","message":"human explanation"}},
            "global_flags":["--repo PATH","--state-dir PATH","--json"],
            "read_commands":["inspect","status","list","history","plan OP","agent schema","agent resources","agent read URI"],
            "write_commands":["init","register","apply FILE","OP --apply"],
            "operations":["prepare","build","check","start","stop","clean","forget"],
            "resource_registration":{
                "passive":{"adapter":"omit","required":["id","kind","target"],"kinds":["file","data","service","agent","custom"],"optional":["ownership","depends_on","ports","description","manager"],"live_observation":false,"allocation_claim":false,"lifecycle_authority":false},
                "managed":{"adapter":"explicit configuration","allocation_proof":"required for effects"},
                "refresh":"edit declarations and register again; orphaned references can be explicitly forgotten"
            },
            "selectors":{"only":"repeat --only RESOURCE_OR_TASK","retain":"repeat --keep RESOURCE"},
            "workflow":["inspect","register explicitly","plan OP --out plan.json","review blockers, effects and exclusions","apply plan.json"],
            "safety":{"stop":"retain checkout and persistent data","clean":"verified owned selections only; reviewed fingerprint required for --apply","forget":"explicit --only resources; release claims/ports while retaining instances and audit receipts","primary":"protected","force_bypass":false,"scripts":"trusted executable code; not sandboxed"},
            "resources":"agent resources",
            "adapter_contract":"treetender://adapter"
        })),
        AgentCommand::Resources => Ok(json!([
            {"uri":"treetender://lifecycle","title":"Lifecycle and ownership","mime_type":"text/markdown"},
            {"uri":"treetender://adapter","title":"External adapter protocol","mime_type":"text/markdown"},
            {"uri":"treetender://registry","title":"Registry and resource relationships","mime_type":"text/markdown"},
            {"uri":"treetender://manifest","title":"Minimal portable manifest","mime_type":"text/plain"},
            {"uri":"treetender://inventory","title":"Passive resource inventory example","mime_type":"text/plain"}
        ])),
        AgentCommand::Read { uri } => {
            let text = match uri.as_str() {
                "treetender://lifecycle" => include_str!("../docs/usage.md"),
                "treetender://adapter" => include_str!("../docs/adapters.md"),
                "treetender://registry" => include_str!("../docs/registry.md"),
                "treetender://manifest" => include_str!("../examples/minimal.toml"),
                "treetender://inventory" => include_str!("../examples/inventory.toml"),
                _ => bail!(
                    "unknown resource URI {uri}; discover available URIs with agent resources"
                ),
            };
            Ok(json!({"uri":uri,"text":text}))
        }
    }
}
