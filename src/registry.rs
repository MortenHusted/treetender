use crate::model::*;
use fs2::FileExt;
use rusqlite::{Connection, OptionalExtension, params};
use std::fs::{File, OpenOptions};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

pub(crate) struct Registry {
    pub root: PathBuf,
}
pub(crate) struct Guard {
    _file: File,
    pub db: Connection,
}
impl Registry {
    pub fn open(path: &Path) -> Result<Self> {
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(path)?;
        let metadata = std::fs::symlink_metadata(path)?;
        if !metadata.is_dir()
            || metadata.file_type().is_symlink()
            || metadata.permissions().mode() & 0o077 != 0
        {
            return Err(Error::new(
                "state_permissions",
                "state directory must be a private directory (0700), not a symlink",
            ));
        }
        let root = path.canonicalize()?;
        let registry = Self { root };
        let g = registry.lock()?;
        g.db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;
            CREATE TABLE IF NOT EXISTS metadata (key TEXT PRIMARY KEY, value INTEGER NOT NULL);
            INSERT OR IGNORE INTO metadata VALUES ('generation', 0);
            CREATE TABLE IF NOT EXISTS repositories (id TEXT PRIMARY KEY, common_dir TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS workspaces (id TEXT PRIMARY KEY, body TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS declarations (workspace TEXT PRIMARY KEY, body TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS claims (target TEXT PRIMARY KEY, workspace TEXT NOT NULL, resource TEXT NOT NULL, body TEXT NOT NULL, UNIQUE(workspace,resource));
            CREATE TABLE IF NOT EXISTS ports (port INTEGER PRIMARY KEY, target TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS operations (sequence INTEGER PRIMARY KEY AUTOINCREMENT, id TEXT UNIQUE NOT NULL, workspace TEXT NOT NULL, body TEXT NOT NULL);")?;
        Ok(registry)
    }
    pub fn lock(&self) -> Result<Guard> {
        for name in [
            "registry.lock",
            "registry.sqlite3",
            "registry.sqlite3-wal",
            "registry.sqlite3-shm",
            "registry.sqlite3-journal",
        ] {
            if let Ok(m) = std::fs::symlink_metadata(self.root.join(name))
                && (!m.is_file()
                    || m.file_type().is_symlink()
                    || m.nlink() != 1
                    || m.permissions().mode() & 0o077 != 0)
            {
                return Err(Error::new(
                    "state_permissions",
                    format!("{name} must be a private regular file (0600) with one link"),
                ));
            }
        }
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(self.root.join("registry.lock"))?;
        file.lock_exclusive()?;
        let database = self.root.join("registry.sqlite3");
        if !database.exists() {
            OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW)
                .open(&database)?;
        }
        let db = Connection::open(database)?;
        db.busy_timeout(std::time::Duration::from_secs(30))?;
        db.execute_batch("PRAGMA synchronous=FULL; PRAGMA foreign_keys=ON;")?;
        Ok(Guard { _file: file, db })
    }
}
impl Guard {
    pub fn save_declarations(&self, w: &Workspace, m: &crate::manifest::Manifest) -> Result<()> {
        self.db.execute("INSERT INTO declarations VALUES (?,?) ON CONFLICT(workspace) DO UPDATE SET body=excluded.body",params![w.id,serde_json::to_string(m)?])?;
        self.bump()
    }
    pub fn declarations(&self, id: &str) -> Result<crate::manifest::Manifest> {
        let body: Option<String> = self
            .db
            .query_row(
                "SELECT body FROM declarations WHERE workspace=?",
                [id],
                |r| r.get(0),
            )
            .optional()?;
        Ok(match body {
            Some(body) => serde_json::from_str(&body)?,
            None => crate::manifest::Manifest::default(),
        })
    }
    pub fn generation(&self) -> Result<u64> {
        Ok(self.db.query_row(
            "SELECT value FROM metadata WHERE key='generation'",
            [],
            |r| r.get::<_, i64>(0),
        )? as u64)
    }
    pub fn bump(&self) -> Result<()> {
        self.db.execute(
            "UPDATE metadata SET value=value+1 WHERE key='generation'",
            [],
        )?;
        Ok(())
    }
    pub fn workspaces(&self) -> Result<Vec<Workspace>> {
        let mut q = self.db.prepare("SELECT body FROM workspaces ORDER BY id")?;
        let rows = q.query_map([], |r| r.get::<_, String>(0))?;
        rows.map(|r| Ok(serde_json::from_str(&r?)?)).collect()
    }
    pub fn resolve(&self, input: &Path) -> Result<Workspace> {
        let text = input.to_string_lossy();
        if let Some(w) = self.workspaces()?.into_iter().find(|w| w.id == text) {
            return Ok(w);
        }
        let discovery = crate::git::discover(input)?;
        self.workspaces()?
            .into_iter()
            .find(|w| w.git_dir == discovery.git_dir && w.path == discovery.path)
            .ok_or_else(|| {
                Error::new(
                    "unregistered",
                    "register this checkout first; relocated checkouts require register again",
                )
            })
    }
    pub fn save_workspace(&self, w: &Workspace) -> Result<()> {
        let previous: Option<String> = self
            .db
            .query_row(
                "SELECT common_dir FROM repositories WHERE id=?",
                [&w.repository_id],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(old) = previous
            && Path::new(&old) != w.common_dir
            && Path::new(&old).exists()
        {
            return Err(Error::new(
                "identity_conflict",
                "repository marker also exists at another live location",
            ));
        }
        if let Some(old) = self.workspaces()?.into_iter().find(|old| old.id == w.id)
            && (old.repository_id != w.repository_id
                || (old.git_dir != w.git_dir && old.git_dir.exists())
                || (old.path != w.path && old.path.exists()))
        {
            return Err(Error::new(
                "identity_conflict",
                "workspace identity is already bound to another Git directory",
            ));
        }
        self.db.execute("INSERT INTO repositories VALUES (?,?) ON CONFLICT(id) DO UPDATE SET common_dir=excluded.common_dir", params![w.repository_id,w.common_dir.to_string_lossy()])?;
        self.db.execute(
            "INSERT INTO workspaces VALUES (?,?) ON CONFLICT(id) DO UPDATE SET body=excluded.body",
            params![w.id, serde_json::to_string(w)?],
        )?;
        self.bump()
    }
    pub fn claims(&self, workspace: Option<&str>) -> Result<Vec<Receipt>> {
        let mut q = self.db.prepare(
            "SELECT body FROM claims WHERE (?1 IS NULL OR workspace=?1) ORDER BY resource",
        )?;
        let rows = q.query_map([workspace], |r| r.get::<_, String>(0))?;
        rows.map(|r| Ok(serde_json::from_str(&r?)?)).collect()
    }
    pub fn claim(&self, target: &str) -> Result<Option<Receipt>> {
        Ok(self.claims(None)?.into_iter().find(|c| c.target == target))
    }
    pub fn save_claim(&self, receipt: &Receipt) -> Result<()> {
        if let Some(other) = self.claim(&receipt.target)?
            && (other.workspace_id != receipt.workspace_id
                || other.resource_id != receipt.resource_id)
        {
            return Err(Error::new(
                "ownership_conflict",
                format!("{} is reserved by another allocation", receipt.target),
            ));
        }
        // BEGIN IMMEDIATE makes claim + all port reservations + generation atomic.
        self.db.execute_batch("BEGIN IMMEDIATE")?;
        let result = (|| -> Result<()> {
            for port in &receipt.declaration.ports {
                let other: Option<String> = self
                    .db
                    .query_row("SELECT target FROM ports WHERE port=?", [port], |r| {
                        r.get(0)
                    })
                    .optional()?;
                if other.is_some_and(|t| t != receipt.target) {
                    return Err(Error::new(
                        "port_conflict",
                        format!("port {port} remains claimed, including while stopped"),
                    ));
                }
                self.db.execute(
                    "INSERT OR IGNORE INTO ports VALUES (?,?)",
                    params![port, receipt.target],
                )?;
            }
            self.db.execute("INSERT INTO claims VALUES (?,?,?,?) ON CONFLICT(target) DO UPDATE SET body=excluded.body", params![receipt.target,receipt.workspace_id,receipt.resource_id,serde_json::to_string(receipt)?])?;
            self.bump()?;
            Ok(())
        })();
        match result {
            Ok(()) => {
                self.db.execute_batch("COMMIT")?;
                Ok(())
            }
            Err(e) => {
                self.db.execute_batch("ROLLBACK")?;
                Err(e)
            }
        }
    }
    pub fn delete_claim(&self, target: &str) -> Result<()> {
        self.db.execute_batch("BEGIN IMMEDIATE")?;
        let result = (|| -> Result<()> {
            self.db
                .execute("DELETE FROM ports WHERE target=?", [target])?;
            self.db
                .execute("DELETE FROM claims WHERE target=?", [target])?;
            self.bump()
        })();
        match result {
            Ok(()) => {
                self.db.execute_batch("COMMIT")?;
                Ok(())
            }
            Err(e) => {
                self.db.execute_batch("ROLLBACK")?;
                Err(e)
            }
        }
    }
    pub fn save_operation(&self, op: &OperationRecord) -> Result<()> {
        self.db.execute("INSERT INTO operations(id,workspace,body) VALUES (?,?,?) ON CONFLICT(id) DO UPDATE SET body=excluded.body", params![op.id,op.workspace_id,serde_json::to_string(op)?])?;
        self.bump()
    }
    pub fn history(&self, id: &str) -> Result<Vec<OperationRecord>> {
        let mut q = self
            .db
            .prepare("SELECT body FROM operations WHERE workspace=? ORDER BY sequence")?;
        let rows = q.query_map([id], |r| r.get::<_, String>(0))?;
        rows.map(|r| Ok(serde_json::from_str(&r?)?)).collect()
    }
}
