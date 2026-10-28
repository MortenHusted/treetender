use crate::model::{Error, Ownership, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub version: u32,
    #[serde(default)]
    pub resources: Vec<Resource>,
    #[serde(default)]
    pub tasks: Vec<Task>,
    /// Named host ports leased to each workspace from a range.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub ports: BTreeMap<String, PortRange>,
}
impl Default for Manifest {
    fn default() -> Self {
        Self {
            version: 1,
            resources: vec![],
            tasks: vec![],
            ports: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PortRange {
    /// Inclusive bounds.
    pub range: [u16; 2],
    #[serde(default, skip_serializing_if = "Scope::is_all")]
    pub scope: Scope,
}
impl PortRange {
    pub fn contains(&self, port: u16) -> bool {
        (self.range[0]..=self.range[1]).contains(&port)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Resource {
    pub id: String,
    pub kind: String,
    #[serde(default)]
    pub ownership: Ownership,
    #[serde(default)]
    pub depends_on: Vec<String>,
    #[serde(default)]
    pub target: String,
    #[serde(default)]
    pub ports: Vec<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adapter: Option<Adapter>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manager: Option<String>,
    #[serde(default, skip_serializing_if = "Scope::is_all")]
    pub scope: Scope,
}

/// Which checkouts a declaration applies to. Mostly useful in the primary's
/// private overlay, which linked checkouts inherit.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    #[default]
    All,
    Linked,
    Primary,
}
impl Scope {
    fn is_all(&self) -> bool {
        *self == Scope::All
    }
    fn applies(self, primary: bool) -> bool {
        match self {
            Scope::All => true,
            Scope::Linked => !primary,
            Scope::Primary => primary,
        }
    }
}

impl Resource {
    pub fn is_passive(&self) -> bool {
        self.adapter.is_none()
    }

    pub fn is_reference(&self) -> bool {
        self.is_passive() || self.ownership != Ownership::Owned
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Adapter {
    File {
        source: String,
        destination: String,
        #[serde(default)]
        mode: FileMode,
        #[serde(default)]
        source_root: SourceRoot,
        /// Skip, rather than block on, a source that does not exist.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        optional: bool,
    },
    Script {
        command: Vec<String>,
        capabilities: Vec<String>,
        #[serde(default = "timeout")]
        timeout_secs: u64,
    },
    Portman {
        #[serde(default = "portman")]
        command: Vec<String>,
        service: String,
        socket: String,
    },
    Directory {
        path: String,
    },
    /// One database on a PostgreSQL server, managed through `psql`.
    /// Credentials come from libpq (service file, password file, PG* variables).
    Postgres {
        database: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        service: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        host: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        port: Option<u16>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        user: Option<String>,
        #[serde(default = "psql")]
        command: Vec<String>,
        #[serde(default = "timeout")]
        timeout_secs: u64,
    },
}

impl Adapter {
    /// External providers keep a claim when they unexpectedly report a recorded
    /// allocation missing; local files and directories retire it on clean.
    pub fn is_external(&self) -> bool {
        matches!(self, Adapter::Script { .. } | Adapter::Postgres { .. })
    }
}
fn psql() -> Vec<String> {
    vec!["psql".into()]
}
fn timeout() -> u64 {
    30
}
fn portman() -> Vec<String> {
    vec!["portman".into()]
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum FileMode {
    #[default]
    Copy,
    Link,
    /// A private copy with workspace placeholders substituted.
    Template,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum SourceRoot {
    #[default]
    Workspace,
    Primary,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TaskKind {
    Build,
    Check,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Task {
    pub id: String,
    pub kind: TaskKind,
    pub command: Vec<String>,
    #[serde(default)]
    pub depends_on: Vec<String>,
    #[serde(default)]
    pub inputs: Vec<String>,
    #[serde(default)]
    pub outputs: Vec<String>,
    #[serde(default = "timeout")]
    pub timeout_secs: u64,
    #[serde(default, skip_serializing_if = "Scope::is_all")]
    pub scope: Scope,
}

pub fn relative(path: &str) -> Result<()> {
    if path.is_empty()
        || Path::new(path)
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
    {
        return Err(Error::new(
            "unsafe_path",
            format!("expected safe relative path: {path}"),
        ));
    }
    // Git metadata is never a declared file source or destination.
    if Path::new(path)
        .components()
        .any(|c| c.as_os_str() == ".git")
    {
        return Err(Error::new(
            "unsafe_path",
            "resources cannot address Git metadata",
        ));
    }
    Ok(())
}

pub(crate) fn writable_destination(path: &str) -> Result<()> {
    relative(path)?;
    if Path::new(path).components().any(|part| {
        let part = part.as_os_str().to_string_lossy();
        part == "treetender.toml"
            || part == ".treetender.local.toml"
            || part.starts_with(".treetender-delete-")
    }) {
        return Err(Error::new(
            "unsafe_path",
            "file destinations cannot modify lifecycle declarations or cleanup quarantine paths",
        ));
    }
    Ok(())
}

impl Manifest {
    /// Loads a checkout's declarations. A linked checkout also inherits the
    /// primary checkout's private overlay, layered between its own two files.
    pub fn load(root: &Path, primary_root: &Path) -> Result<Self> {
        let primary = root == primary_root;
        let mut files = vec![root.join("treetender.toml")];
        if !primary {
            files.push(primary_root.join(".treetender.local.toml"));
        }
        files.push(root.join(".treetender.local.toml"));
        let mut result = Self::default();
        for path in files {
            match std::fs::symlink_metadata(&path) {
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => return Err(e.into()),
                Ok(m) if !m.is_file() || m.file_type().is_symlink() => {
                    return Err(Error::new("unsafe_path", "manifest must be a regular file"));
                }
                _ => (),
            }
            let parsed: Self = toml::from_str(&std::fs::read_to_string(path)?)
                .map_err(|e| Error::new("manifest", e.to_string()))?;
            if parsed.version != 1 {
                return Err(Error::new(
                    "manifest_version",
                    "only version 1 is supported",
                ));
            }
            let mut local_ids = BTreeSet::new();
            for id in parsed
                .resources
                .iter()
                .map(|r| &r.id)
                .chain(parsed.tasks.iter().map(|t| &t.id))
            {
                if !local_ids.insert(id) {
                    return Err(Error::new("manifest", format!("duplicate id {id}")));
                }
            }
            result.version = parsed.version;
            for resource in parsed.resources {
                result.resources.retain(|r| r.id != resource.id);
                result.resources.push(resource);
            }
            for task in parsed.tasks {
                result.tasks.retain(|t| t.id != task.id);
                result.tasks.push(task);
            }
            result.ports.extend(parsed.ports);
        }
        result.resources.retain(|r| r.scope.applies(primary));
        result.tasks.retain(|t| t.scope.applies(primary));
        result.ports.retain(|_, p| p.scope.applies(primary));
        result.resources.sort_by(|a, b| a.id.cmp(&b.id));
        result.tasks.sort_by(|a, b| a.id.cmp(&b.id));
        result.validate()?;
        Ok(result)
    }
    pub fn validate(&self) -> Result<()> {
        if self.version != 1 {
            return Err(Error::new(
                "manifest_version",
                "only version 1 is supported",
            ));
        }
        let mut ids = BTreeSet::new();
        for id in self
            .resources
            .iter()
            .map(|r| &r.id)
            .chain(self.tasks.iter().map(|t| &t.id))
        {
            if id.is_empty()
                || !id
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "_-./".contains(c))
                || !ids.insert(id)
            {
                return Err(Error::new(
                    "manifest",
                    format!("invalid or duplicate id {id}"),
                ));
            }
        }
        for r in &self.resources {
            if !["file", "data", "service", "agent", "custom"].contains(&r.kind.as_str()) {
                return Err(Error::new("manifest", format!("invalid kind {}", r.kind)));
            }
            if r.ports.contains(&0) {
                return Err(Error::new("manifest", "port zero cannot be reserved"));
            }
            match r.adapter.as_ref() {
                None => {
                    if r.target.trim().is_empty() {
                        return Err(Error::new("manifest", "passive resource needs a target"));
                    }
                }
                Some(Adapter::File {
                    source,
                    destination,
                    source_root,
                    ..
                }) => {
                    relative(source)?;
                    writable_destination(destination)?;
                    if source == destination && *source_root == SourceRoot::Workspace {
                        return Err(Error::new("manifest", "source equals destination"));
                    }
                }
                Some(Adapter::Script {
                    command,
                    capabilities,
                    timeout_secs,
                }) => {
                    validate_command(command, *timeout_secs)?;
                    if r.target.trim().is_empty()
                        || !capabilities.iter().any(|c| c == "inspect")
                        || capabilities.iter().any(|c| {
                            !["inspect", "prepare", "start", "stop", "destroy"]
                                .contains(&c.as_str())
                        })
                    {
                        return Err(Error::new(
                            "manifest",
                            "script needs target and explicit supported capabilities including inspect",
                        ));
                    }
                }
                Some(Adapter::Directory { path }) => writable_destination(path)?,
                Some(Adapter::Postgres {
                    database,
                    command,
                    timeout_secs,
                    ..
                }) => {
                    validate_command(command, *timeout_secs)?;
                    if database.trim().is_empty() {
                        return Err(Error::new("manifest", "postgres needs a database"));
                    }
                }
                Some(Adapter::Portman {
                    command,
                    service,
                    socket,
                }) => {
                    validate_command(command, 30)?;
                    if service.trim().is_empty()
                        || service.starts_with('-')
                        || service.chars().any(char::is_whitespace)
                    {
                        return Err(Error::new("manifest", "Portman needs one explicit service"));
                    }
                    if r.target.trim().is_empty() {
                        return Err(Error::new("manifest", "Portman needs a target"));
                    }
                    if !Path::new(socket).is_absolute() {
                        return Err(Error::new(
                            "manifest",
                            "Portman requires an explicit absolute socket path (normally in private overlay)",
                        ));
                    }
                    if r.ownership == Ownership::Owned {
                        return Err(Error::new(
                            "manifest",
                            "Portman references existing service registrations; declare borrowed/shared",
                        ));
                    }
                }
            }
        }
        for (name, port) in &self.ports {
            if name.is_empty()
                || name.len() > 32
                || !name
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "_-".contains(c))
            {
                return Err(Error::new(
                    "manifest",
                    format!("port names use up to 32 lowercase letters, digits, - and _: {name}"),
                ));
            }
            if port.range[0] == 0 || port.range[0] > port.range[1] {
                return Err(Error::new(
                    "manifest",
                    format!("port {name} needs a range [low, high] with 0 < low <= high"),
                ));
            }
        }
        for t in &self.tasks {
            validate_command(&t.command, t.timeout_secs)?;
            for path in t.inputs.iter().chain(&t.outputs) {
                relative(path)?;
            }
        }
        self.order()?;
        Ok(())
    }
    pub fn order(&self) -> Result<Vec<String>> {
        let deps: BTreeMap<&str, &[String]> = self
            .resources
            .iter()
            .map(|r| (r.id.as_str(), r.depends_on.as_slice()))
            .chain(
                self.tasks
                    .iter()
                    .map(|t| (t.id.as_str(), t.depends_on.as_slice())),
            )
            .collect();
        let mut done = BTreeSet::new();
        let mut result = vec![];
        while result.len() < deps.len() {
            let mut progress = false;
            for (id, needs) in &deps {
                for need in *needs {
                    if !deps.contains_key(need.as_str()) {
                        return Err(Error::new(
                            "dependency",
                            format!("{id} depends on unknown {need}"),
                        ));
                    }
                }
                if !done.contains(id) && needs.iter().all(|d| done.contains(d.as_str())) {
                    done.insert(*id);
                    result.push((*id).to_owned());
                    progress = true;
                }
            }
            if !progress {
                return Err(Error::new(
                    "dependency_cycle",
                    "resource/task dependencies contain a cycle",
                ));
            }
        }
        Ok(result)
    }
}
pub(crate) fn validate_command(command: &[String], seconds: u64) -> Result<()> {
    if command.is_empty()
        || command[0].trim().is_empty()
        || seconds == 0
        || seconds > 86400
        || command.iter().any(|s| s.contains('\0'))
    {
        return Err(Error::new(
            "manifest",
            "command requires argv and timeout 1..86400 seconds",
        ));
    }
    Ok(())
}
