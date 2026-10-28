use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Error {
    pub code: String,
    pub message: String,
}
impl Error {
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}
impl std::error::Error for Error {}
impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Self::new("io", e.to_string())
    }
}
impl From<rusqlite::Error> for Error {
    fn from(e: rusqlite::Error) -> Self {
        Self::new("registry", e.to_string())
    }
}
impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Self {
        Self::new("json", e.to_string())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Workspace {
    pub id: String,
    pub repository_id: String,
    pub path: PathBuf,
    pub git_dir: PathBuf,
    pub common_dir: PathBuf,
    pub primary: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Inspection {
    pub path: PathBuf,
    pub git_dir: PathBuf,
    pub common_dir: PathBuf,
    pub primary_path: PathBuf,
    pub primary: bool,
    pub workspace: Option<Workspace>,
    pub manifest: crate::manifest::Manifest,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum Ownership {
    #[default]
    Owned,
    Shared,
    Borrowed,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Operation {
    Prepare,
    Build,
    Check,
    Start,
    Stop,
    Clean,
    Forget,
    Adopt,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Selectors {
    #[serde(default)]
    pub only: Vec<String>,
    #[serde(default)]
    pub retain: Vec<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Presence {
    Present,
    Missing,
    Unknown,
}

/// Provider proof binds an exact instance to the generated allocation token.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Proof {
    pub presence: Presence,
    #[serde(default)]
    pub instance: Option<String>,
    #[serde(default)]
    pub owner_token: Option<String>,
    #[serde(default)]
    pub handle: serde_json::Value,
}
impl Proof {
    pub fn missing() -> Self {
        Self {
            presence: Presence::Missing,
            instance: None,
            owner_token: None,
            handle: serde_json::Value::Null,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Receipt {
    pub resource_id: String,
    pub target: String,
    pub workspace_id: String,
    pub owner_token: String,
    pub proof: Proof,
    pub generation: u64,
    pub state: String,
    pub declaration: crate::manifest::Resource,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Step {
    pub id: String,
    pub action: Operation,
    pub target: String,
    pub destructive: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Exclusion {
    pub id: String,
    pub reason: String,
}

/// A frozen material snapshot. Never construct plans by editing these fields.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Plan {
    pub version: u32,
    pub workspace: Workspace,
    pub operation: Operation,
    pub selectors: Selectors,
    pub steps: Vec<Step>,
    pub exclusions: Vec<Exclusion>,
    pub blockers: Vec<String>,
    pub fingerprint: String,
    pub manifest: crate::manifest::Manifest,
    pub observations: std::collections::BTreeMap<String, Proof>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApplyResult {
    pub operation_id: String,
    pub success: bool,
    pub completed: Vec<String>,
    pub failure: Option<Error>,
    pub recovery: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperationRecord {
    pub id: String,
    pub workspace_id: String,
    pub operation: Operation,
    pub state: String,
    pub steps: Vec<JournalStep>,
    pub failure: Option<Error>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JournalStep {
    pub id: String,
    pub state: String,
    pub detail: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Status {
    pub workspace: Workspace,
    pub resources: Vec<Receipt>,
    pub operations: Vec<OperationRecord>,
    pub observations: std::collections::BTreeMap<String, Proof>,
    pub issues: Vec<String>,
    pub builds: Vec<crate::build::BuildStatus>,
    pub declarations: crate::manifest::Manifest,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdapterRequest {
    pub version: u32,
    pub action: String,
    pub workspace: Workspace,
    pub resource_id: String,
    pub target: String,
    pub owner_token: String,
    pub receipt: Option<Proof>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdapterResponse {
    pub version: u32,
    pub proof: Proof,
}
