use clap::{Args, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(
    version,
    about = "Tend development workspaces without losing their roots"
)]
pub struct Cli {
    /// Workspace checkout (defaults to the current directory).
    #[arg(long, global = true, default_value = ".")]
    pub repo: PathBuf,
    /// Private registry directory. Never commit its contents.
    #[arg(long, global = true, env = "TREETENDER_STATE_DIR")]
    pub state_dir: Option<PathBuf>,
    /// Emit one versioned JSON document; errors go to stderr as JSON.
    #[arg(long, global = true)]
    pub json: bool,
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Write an explicit repository manifest without replacing existing files.
    Init,
    /// Discover Git identity and configuration without registering resources.
    Inspect,
    /// Register this checkout; never claim existing external resources.
    Register,
    /// Inspect the selected workspace and its recorded resources.
    Status,
    /// List centrally registered workspaces, including retained state.
    List,
    /// Inspect durable operation outcomes and incomplete work.
    History,
    /// Produce a side-effect-free lifecycle plan.
    Plan {
        #[arg(value_enum)]
        operation: Lifecycle,
        #[command(flatten)]
        selection: Selection,
        /// Save the full reviewed plan for a later apply.
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Revalidate and execute a saved plan.
    Apply { plan: PathBuf },
    /// Plan preparation; --apply creates declared resources without starting services.
    Prepare(Action),
    /// Plan build/check tasks; --apply executes them without starting services.
    Build(Action),
    /// Plan declared verification tasks; --apply runs them.
    Check(Action),
    /// Plan explicit service startup; --apply starts only declared owned services.
    Start(Action),
    /// Plan service stop; --apply stops and retains data and checkout.
    Stop(Action),
    /// Plan resource cleanup; --apply removes only verified owned selections.
    Clean(Action),
    /// Browse workspaces, review plans and apply them through the same policy.
    Tui,
    /// Discover machine interfaces and agent-readable resources.
    Agent {
        #[command(subcommand)]
        command: AgentCommand,
    },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum Lifecycle {
    Prepare,
    Build,
    Check,
    Start,
    Stop,
    Clean,
}

impl Lifecycle {
    pub fn name(self) -> &'static str {
        match self {
            Self::Prepare => "prepare",
            Self::Build => "build",
            Self::Check => "check",
            Self::Start => "start",
            Self::Stop => "stop",
            Self::Clean => "clean",
        }
    }
}

#[derive(Debug, Clone, Default, Args)]
pub struct Selection {
    /// Select a resource/task by name (repeat to select several).
    #[arg(long = "only")]
    pub only: Vec<String>,
    /// Preserve a resource by name, regardless of kind (repeat as needed).
    #[arg(long = "keep")]
    pub keep: Vec<String>,
}

#[derive(Debug, Args)]
pub struct Action {
    #[command(flatten)]
    pub selection: Selection,
    /// Execute the displayed operation. Without this flag, only plan.
    #[arg(long)]
    pub apply: bool,
}

#[derive(Debug, Subcommand)]
pub enum AgentCommand {
    /// Machine-readable command, plan and adapter contracts.
    Schema,
    /// Discover documentation resources using stable treetender:// URIs.
    Resources,
    /// Read one of the advertised resources.
    Read { uri: String },
}
