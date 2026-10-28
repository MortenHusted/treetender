use crate::adapter;
use crate::git;
use crate::manifest::{Adapter, Manifest, SourceRoot, TaskKind};
use crate::model::*;
use crate::registry::{Guard, Registry};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

pub struct Engine {
    registry: Registry,
}

fn action(op: Operation) -> &'static str {
    match op {
        Operation::Prepare => "prepare",
        Operation::Build => "build",
        Operation::Check => "check",
        Operation::Start => "start",
        Operation::Stop => "stop",
        Operation::Clean => "destroy",
        Operation::Forget => "forget",
        Operation::Adopt => "adopt",
    }
}
fn matches(selector: &str, id: &str, kind: &str) -> bool {
    selector == id || selector.strip_prefix("kind:") == Some(kind)
}
fn selected(selectors: &[String], id: &str, kind: &str) -> bool {
    selectors.iter().any(|s| matches(s, id, kind))
}
fn proof_matches(proof: &Proof, receipt: &Receipt) -> bool {
    proof.presence == Presence::Present
        && proof.owner_token.as_deref() == Some(&receipt.owner_token)
        && proof.instance.as_ref().is_some_and(|s| !s.is_empty())
        && (receipt.proof.presence != Presence::Present || proof.instance == receipt.proof.instance)
}

fn missing_external_blocks(receipt: &Receipt, operation: Operation) -> bool {
    if receipt.state == "reserved" {
        return false;
    }
    if operation == Operation::Clean && receipt.state == "destroying" {
        return false;
    }
    !(matches!(operation, Operation::Prepare | Operation::Clean)
        && receipt.state == "preparing"
        && receipt.proof.presence != Presence::Present)
}

fn pending(receipt: &Receipt) -> bool {
    matches!(
        receipt.state.as_str(),
        "intent" | "uncertain" | "preparing" | "starting" | "stopping" | "destroying"
    )
}
fn prerequisite(
    w: &Workspace,
    r: &crate::manifest::Resource,
    receipt: Option<&Receipt>,
    recovery_dir: &Path,
) -> Result<Proof> {
    let proof = adapter::inspect(
        w,
        r,
        receipt.map(|c| c.owner_token.as_str()).unwrap_or(""),
        receipt.map(|c| &c.proof),
        recovery_dir,
    )?;
    if proof.presence != Presence::Present {
        return Err(Error::new("prerequisite", "provider must prove present"));
    }
    if !r.is_reference()
        && receipt.is_none_or(|c| {
            !matches!(c.state.as_str(), "prepared" | "active") || !proof_matches(&proof, c)
        })
    {
        return Err(Error::new(
            "prerequisite",
            "owned dependency must be prepared separately with matching proof",
        ));
    }
    Ok(proof)
}

/// Leases each declared port name to the workspace. An existing lease inside
/// its range is kept even while its listener runs; leases for names no longer
/// declared, or outside a changed range, are released.
fn lease_ports(
    g: &Guard,
    w: &Workspace,
    declared: &BTreeMap<String, crate::manifest::PortRange>,
) -> Result<BTreeMap<String, u16>> {
    let mut leases = g.leases(&w.id)?;
    for (name, port) in leases.clone() {
        if declared
            .get(&name)
            .is_none_or(|range| !range.contains(port))
        {
            g.release(&format!("lease:{}:{name}", w.id))?;
            leases.remove(&name);
        }
    }
    for (name, range) in declared {
        if leases.contains_key(name) {
            continue;
        }
        let [low, high] = range.range;
        let span = u32::from(high - low) + 1;
        let seed = adapter::digest(format!("{}:{name}", w.id).as_bytes());
        let start = u32::from_str_radix(&seed[..8], 16).unwrap_or(0) % span;
        let port = (0..span)
            .map(|offset| low + ((start + offset) % span) as u16)
            .find(|&port| !g.port_taken(port).unwrap_or(true) && !listening(port))
            .ok_or_else(|| {
                Error::new(
                    "port_exhausted",
                    format!("no free port in {low}-{high} for {name}"),
                )
            })?;
        g.lease(&format!("lease:{}:{name}", w.id), port)?;
        leases.insert(name.clone(), port);
    }
    Ok(leases)
}

/// A port is busy when any local listener prevents binding it.
fn listening(port: u16) -> bool {
    ["127.0.0.1", "0.0.0.0"]
        .iter()
        .any(|host| std::net::TcpListener::bind((*host, port)).is_err())
}

fn valid_name(name: &str) -> Result<()> {
    let mut chars = name.chars();
    if name.len() > 40
        || !chars
            .next()
            .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        || !chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        || name.ends_with('-')
    {
        return Err(Error::new(
            "invalid_name",
            "workspace names use 1-40 lowercase letters, digits and inner hyphens",
        ));
    }
    Ok(())
}

/// A workspace keeps its first name, so names derived from it survive moves.
/// New names are unique within the repository.
fn workspace_name(
    known: &[Workspace],
    id: &str,
    repository_id: &str,
    path: &Path,
    requested: Option<&str>,
) -> Result<String> {
    if let Some(existing) = known.iter().find(|w| w.id == id && !w.name.is_empty()) {
        if requested.is_some_and(|name| name != existing.name) {
            return Err(Error::new(
                "name_fixed",
                format!(
                    "workspace is already named {}; names are fixed at first registration",
                    existing.name
                ),
            ));
        }
        return Ok(existing.name.clone());
    }
    let taken = |name: &str| {
        known
            .iter()
            .any(|w| w.id != id && w.repository_id == repository_id && w.name == name)
    };
    if let Some(name) = requested {
        if taken(name) {
            return Err(Error::new(
                "name_conflict",
                format!("another workspace of this repository is named {name}"),
            ));
        }
        return Ok(name.into());
    }
    let mut base = String::new();
    for c in path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .chars()
    {
        if c.is_ascii_alphanumeric() {
            base.push(c.to_ascii_lowercase());
        } else if !base.is_empty() && !base.ends_with('-') {
            base.push('-');
        }
    }
    let base = match base.trim_end_matches('-') {
        "" => "workspace",
        trimmed => trimmed,
    };
    let truncated = |len: usize| {
        base.chars()
            .take(len)
            .collect::<String>()
            .trim_end_matches('-')
            .to_owned()
    };
    let mut candidate = truncated(40);
    let mut suffix = 2;
    while taken(&candidate) {
        let tail = format!("-{suffix}");
        candidate = format!("{}{tail}", truncated(40 - tail.len()));
        suffix += 1;
    }
    Ok(candidate)
}

impl Engine {
    /// Initialize private state. This does not discover or register any checkout.
    pub fn open(state_dir: impl AsRef<Path>) -> Result<Self> {
        Ok(Self {
            registry: Registry::open(state_dir.as_ref())?,
        })
    }
    pub fn inspect(&self, path: &Path) -> Result<Inspection> {
        let d = git::discover(path)?;
        let g = self.registry.lock()?;
        Ok(Inspection {
            path: d.path.clone(),
            git_dir: d.git_dir.clone(),
            common_dir: d.common_dir.clone(),
            primary_path: d.primary_path.clone(),
            primary: d.git_dir == d.common_dir,
            workspace: g.current_workspace(&d)?,
            manifest: Manifest::load(&d.path, &d.primary_path)?,
        })
    }
    pub fn register(&self, path: &Path) -> Result<Workspace> {
        self.register_named(path, None)
    }
    /// Registers a checkout, naming it on first registration. A name is fixed
    /// afterwards; omit `name` to derive one from the checkout directory.
    pub fn register_named(&self, path: &Path, name: Option<&str>) -> Result<Workspace> {
        let g = self.registry.lock()?;
        let d = git::discover(path)?;
        // Validate before writing any Git identity markers.
        Manifest::load(&d.path, &d.primary_path)?;
        if let Some(name) = name {
            valid_name(name)?;
        }
        let transaction =
            rusqlite::Transaction::new_unchecked(&g.db, rusqlite::TransactionBehavior::Immediate)?;
        let known = g.workspaces()?;
        let repo = git::identity(&d.common_dir, true)?.unwrap();
        let registering_primary = d.git_dir == d.common_dir;
        let mut primary = Workspace {
            id: repo.clone(),
            repository_id: repo.clone(),
            path: d.primary_path.clone(),
            git_dir: d.common_dir.clone(),
            common_dir: d.common_dir.clone(),
            primary: true,
            name: workspace_name(
                &known,
                &repo,
                &repo,
                &d.primary_path,
                name.filter(|_| registering_primary),
            )?,
            ports: BTreeMap::new(),
        };
        primary.ports = match self.loaded_manifest(&primary)? {
            Some(m) => lease_ports(&g, &primary, &m.ports)?,
            None => g.leases(&primary.id)?,
        };
        let manifest = self.resolved_manifest(&primary)?;
        for old in g.claims(Some(&primary.id))? {
            if old.state == "reserved"
                && !manifest.resources.iter().any(|r| {
                    !r.is_reference()
                        && old.declaration == *r
                        && old.target == adapter::target(&primary, r)
                })
            {
                g.delete_claim(&old.target)?;
            }
        }
        g.save_workspace(&primary)?;
        g.save_declarations(&primary, &manifest)?;
        for r in manifest.resources.iter().filter(|r| !r.is_reference()) {
            let target = adapter::target(&primary, r);
            if let Some(other) = g.claim(&target)? {
                if other.workspace_id != primary.id || other.resource_id != r.id {
                    return Err(Error::new(
                        "ownership_conflict",
                        format!(
                            "primary target {target} is already claimed by {}",
                            other.workspace_id
                        ),
                    ));
                }
            } else {
                let receipt = Receipt {
                    resource_id: r.id.clone(),
                    target,
                    workspace_id: primary.id.clone(),
                    owner_token: uuid::Uuid::new_v4().to_string(),
                    proof: Proof {
                        presence: Presence::Unknown,
                        ..Proof::missing()
                    },
                    generation: 0,
                    state: "reserved".into(),
                    declaration: r.clone(),
                };
                g.save_claim(&receipt)?;
            }
        }
        if d.git_dir == d.common_dir {
            transaction.commit()?;
            return Ok(primary);
        }
        let id = git::identity(&d.git_dir, true)?.unwrap();
        let mut w = Workspace {
            name: workspace_name(&known, &id, &repo, &d.path, name)?,
            id,
            repository_id: repo,
            path: d.path,
            git_dir: d.git_dir,
            common_dir: d.common_dir,
            primary: false,
            ports: BTreeMap::new(),
        };
        if let Some(m) = self.loaded_manifest(&w)? {
            w.ports = lease_ports(&g, &w, &m.ports)?;
        }
        g.save_workspace(&w)?;
        g.save_declarations(&w, &self.resolved_manifest(&w)?)?;
        transaction.commit()?;
        Ok(w)
    }
    pub fn list(&self) -> Result<Vec<Workspace>> {
        self.registry.lock()?.workspaces()
    }
    /// The registered record for a checkout path or workspace ID.
    pub fn workspace(&self, workspace: &str) -> Result<Workspace> {
        self.registry.lock()?.resolve(Path::new(workspace))
    }
    pub fn status(&self, workspace: &str) -> Result<Status> {
        let g = self.registry.lock()?;
        let w = g.resolve(Path::new(workspace))?;
        let resources = g.claims(Some(&w.id))?;
        let mut observations = BTreeMap::new();
        let mut issues = vec![];
        let checkout_current = match git::is_current(&w) {
            Ok(current) => current,
            Err(e) => {
                issues.push(e.to_string());
                false
            }
        };
        if !checkout_current {
            issues.push("recorded checkout is missing or replaced; only receipt-based operations are available".into());
        }
        for stale in g
            .workspaces()?
            .iter()
            .filter(|old| checkout_current && old.id != w.id && old.path == w.path)
        {
            issues.push(format!(
                "replaces stale workspace {}; its receipts remain accessible by ID",
                stale.id
            ));
        }
        let mut manifest = match self.resolved_manifest(&w) {
            Ok(m) => m,
            Err(e) => {
                issues.push(e.to_string());
                Manifest::default()
            }
        };
        for receipt in &resources {
            if !manifest
                .resources
                .iter()
                .any(|r| r.id == receipt.resource_id)
            {
                manifest.resources.push(receipt.declaration.clone());
            }
        }
        for r in &manifest.resources {
            if r.is_passive() {
                continue;
            }
            let receipt = resources.iter().find(|c| c.resource_id == r.id);
            match adapter::inspect(
                &w,
                r,
                receipt.map(|c| c.owner_token.as_str()).unwrap_or(""),
                receipt.map(|c| &c.proof),
                &self.registry.recovery_dir,
            ) {
                Ok(p) => {
                    observations.insert(r.id.clone(), p);
                }
                Err(e) => issues.push(format!("{}: {e}", r.id)),
            }
        }
        let operations = g.history(&w.id)?;
        let builds = crate::build::inspect(&w.path, &manifest.tasks, &operations);
        Ok(Status {
            declarations: g.declarations(&w.id)?,
            resources,
            operations,
            workspace: w,
            observations,
            issues,
            builds,
        })
    }
    /// Allocated bytes of present directory resources. Walks whole trees, so
    /// callers request it explicitly; it never enters plans or proofs.
    pub fn usage(&self, workspace: &str) -> Result<BTreeMap<String, u64>> {
        // The walk can be slow; never hold the registry lock during it.
        let w = self.registry.lock()?.resolve(Path::new(workspace))?;
        let mut usage = BTreeMap::new();
        if !git::is_current(&w)? {
            return Ok(usage);
        }
        for r in self.resolved_manifest(&w)?.resources {
            if let Some(Adapter::Directory { path }) = &r.adapter
                && let Some(bytes) = crate::directory::usage(&w.path, path)?
            {
                usage.insert(r.id.clone(), bytes);
            }
        }
        Ok(usage)
    }
    pub fn history(&self, workspace: &str) -> Result<Vec<OperationRecord>> {
        let g = self.registry.lock()?;
        let w = g.resolve(Path::new(workspace))?;
        g.history(&w.id)
    }
    pub fn plan(
        &self,
        workspace: &str,
        operation: Operation,
        selectors: Selectors,
    ) -> Result<Plan> {
        let g = self.registry.lock()?;
        let w = g.resolve(Path::new(workspace))?;
        self.plan_locked(&g, w, operation, selectors)
    }
    /// The checkout's own declarations, or None when it is not where it was registered.
    fn loaded_manifest(&self, w: &Workspace) -> Result<Option<Manifest>> {
        let current = match git::is_current(w) {
            Ok(current) => current,
            Err(e) if e.code == "checkout_location_unknown" => false,
            Err(e) => return Err(e),
        };
        if !current {
            return Ok(None);
        }
        Manifest::load(&w.path, &git::discover(&w.path)?.primary_path).map(Some)
    }
    fn resolved_manifest(&self, w: &Workspace) -> Result<Manifest> {
        let mut m = self.loaded_manifest(w)?.unwrap_or_default();
        for r in &mut m.resources {
            if let Some(Adapter::Postgres { database, .. }) = &mut r.adapter {
                *database = w.substitute(database)?;
                crate::postgres::valid_database(database)?;
            }
            r.target = w.substitute(&r.target)?;
            if r.target.contains(['{', '}']) {
                return Err(Error::new("manifest", "unknown target placeholder"));
            }
        }
        Ok(m)
    }
    fn plan_locked(
        &self,
        g: &Guard,
        w: Workspace,
        operation: Operation,
        selectors: Selectors,
    ) -> Result<Plan> {
        if operation == Operation::Forget {
            return self.plan_forget(g, w, selectors);
        }
        let mut blockers = vec![];
        if operation == Operation::Adopt {
            if w.primary {
                blockers.push("primary workspace resources cannot be adopted".into());
            }
            if selectors.only.is_empty() {
                blockers.push(
                    "adopt requires explicit --only RESOURCE selection of existing disposable state"
                        .into(),
                );
            }
        }
        let checkout_current = match git::is_current(&w) {
            Ok(current) => current,
            Err(e) if e.code == "checkout_location_unknown" => {
                blockers.push(e.to_string());
                false
            }
            Err(e) => return Err(e),
        };
        let receipts = g.claims(Some(&w.id))?;
        let mut manifest = self.resolved_manifest(&w)?;
        let stored_declarations = g.declarations(&w.id)?;
        let protected_declarations: BTreeSet<String> = manifest
            .resources
            .iter()
            .chain(&stored_declarations.resources)
            .filter(|r| r.is_reference())
            .map(|r| r.id.clone())
            .collect();
        let mut primary_targets = BTreeSet::new();
        if !w.primary
            && let Some(primary) = g
                .workspaces()?
                .into_iter()
                .find(|p| p.primary && p.repository_id == w.repository_id)
        {
            for r in self.resolved_manifest(&primary)?.resources {
                primary_targets.insert(adapter::target(&primary, &r));
            }
        }
        if matches!(operation, Operation::Clean | Operation::Stop) {
            // Registered uses keep their dependencies protected until registration refreshes them.
            for reference in stored_declarations
                .resources
                .iter()
                .filter(|r| r.is_reference())
            {
                if let Some(current) = manifest.resources.iter_mut().find(|r| r.id == reference.id)
                {
                    current.depends_on.extend(reference.depends_on.clone());
                    current.depends_on.sort();
                    current.depends_on.dedup();
                } else {
                    manifest.resources.push(reference.clone());
                }
            }
            for receipt in &receipts {
                // Cleanup always uses the allocating adapter, never a changed declaration.
                let mut original = receipt.declaration.clone();
                if let Some(current) = manifest
                    .resources
                    .iter()
                    .find(|r| r.id == receipt.resource_id)
                {
                    original.depends_on.extend(current.depends_on.clone());
                    original.depends_on.sort();
                    original.depends_on.dedup();
                }
                manifest.resources.retain(|r| r.id != receipt.resource_id);
                manifest.resources.push(original);
            }
            // Removed declaration dependencies may be absent; retain their receipts too.
            let known: BTreeSet<String> = manifest
                .resources
                .iter()
                .map(|r| r.id.clone())
                .chain(manifest.tasks.iter().map(|t| t.id.clone()))
                .collect();
            for r in &mut manifest.resources {
                r.depends_on.retain(|d| known.contains(d));
            }
        }
        // Persisted references remain protective after a checkout disappears.
        // Re-registration explicitly refreshes the snapshot; planning never does.
        let mut reference_material = BTreeMap::new();
        let scoped_targets: BTreeSet<_> = manifest
            .resources
            .iter()
            .map(|r| adapter::target(&w, r))
            .collect();
        if matches!(operation, Operation::Clean | Operation::Stop) {
            for other in g.workspaces()? {
                if other.id == w.id {
                    continue;
                }
                let stored = g.declarations(&other.id)?;
                let current = self.resolved_manifest(&other)?;
                for r in stored.resources.iter().chain(&current.resources) {
                    let target = adapter::target(&other, r);
                    if (other.primary || r.is_reference()) && scoped_targets.contains(&target) {
                        primary_targets.insert(target.clone());
                        reference_material
                            .entry(other.id.clone())
                            .or_insert_with(BTreeSet::new)
                            .insert((r.id.clone(), target));
                    }
                }
            }
        }
        manifest.resources.sort_by(|a, b| a.id.cmp(&b.id));
        let order = manifest.order()?;
        for s in selectors.only.iter().chain(&selectors.retain) {
            if !manifest
                .resources
                .iter()
                .any(|r| matches(s, &r.id, &r.kind))
                && !manifest.tasks.iter().any(|t| {
                    matches(
                        s,
                        &t.id,
                        if t.kind == TaskKind::Build {
                            "build"
                        } else {
                            "check"
                        },
                    )
                })
            {
                return Err(Error::new("selector", format!("unknown selector {s}")));
            }
        }
        let mut wanted = BTreeSet::new();
        for r in &manifest.resources {
            if !matches!(operation, Operation::Build | Operation::Check)
                && (selectors.only.is_empty() || selected(&selectors.only, &r.id, &r.kind))
            {
                wanted.insert(r.id.clone());
            }
        }
        for t in &manifest.tasks {
            if ((operation == Operation::Build && t.kind == TaskKind::Build)
                || (operation == Operation::Check && t.kind == TaskKind::Check))
                && (selectors.only.is_empty()
                    || selected(
                        &selectors.only,
                        &t.id,
                        if t.kind == TaskKind::Build {
                            "build"
                        } else {
                            "check"
                        },
                    ))
            {
                wanted.insert(t.id.clone());
            }
        }
        if !matches!(
            operation,
            Operation::Clean | Operation::Stop | Operation::Adopt
        ) {
            loop {
                let before = wanted.len();
                for r in &manifest.resources {
                    if wanted.contains(&r.id) {
                        wanted.extend(r.depends_on.clone());
                    }
                }
                for t in &manifest.tasks {
                    if wanted.contains(&t.id) {
                        wanted.extend(t.depends_on.clone());
                    }
                }
                if wanted.len() == before {
                    break;
                }
            }
        }
        let prerequisites: BTreeSet<String> = manifest
            .resources
            .iter()
            .filter(|r| wanted.contains(&r.id) && !r.is_passive())
            .flat_map(|r| r.depends_on.clone())
            .chain(
                manifest
                    .tasks
                    .iter()
                    .filter(|t| wanted.contains(&t.id))
                    .flat_map(|t| t.depends_on.clone()),
            )
            .collect();
        let mut plan = Plan {
            version: 1,
            workspace: w.clone(),
            operation,
            selectors,
            steps: vec![],
            exclusions: vec![],
            blockers: vec![],
            fingerprint: String::new(),
            manifest: manifest.clone(),
            observations: BTreeMap::new(),
        };
        let mut material = BTreeMap::<String, String>::new();
        if checkout_current {
            let d = git::discover(&w.path)?;
            if d.git_dir != w.git_dir
                || d.common_dir != w.common_dir
                || git::identity(&d.git_dir, false)?.as_deref() != Some(&w.id)
                || git::identity(&d.common_dir, false)?.as_deref() != Some(&w.repository_id)
                || (d.git_dir == d.common_dir) != w.primary
            {
                blockers.push("Git identity/location changed; register explicitly".into());
            }
            for name in ["treetender.toml", ".treetender.local.toml"] {
                if w.path.join(name).exists() {
                    material.insert(name.into(), adapter::file_snapshot(&w.path, name)?);
                }
            }
            let overlay = ".treetender.local.toml";
            if !w.primary && d.primary_path.join(overlay).exists() {
                material.insert(
                    format!("primary:{overlay}"),
                    adapter::file_snapshot(&d.primary_path, overlay)?,
                );
            }
        } else if !matches!(operation, Operation::Clean | Operation::Stop) {
            blockers.push(
                "checkout is missing or replaced; only receipt-based cleanup/stop is available"
                    .into(),
            );
        }
        let mut execution_order = order;
        if matches!(operation, Operation::Stop | Operation::Clean) {
            execution_order.reverse();
        }
        for id in execution_order {
            if !wanted.contains(&id) {
                continue;
            }
            if let Some(r) = manifest.resources.iter().find(|r| r.id == id) {
                let target = adapter::target(&w, r);
                let receipt = receipts.iter().find(|c| c.resource_id == id);
                let exclude = if selected(&plan.selectors.retain, &r.id, &r.kind) {
                    Some("explicitly retained")
                } else if r.is_passive() {
                    Some("passive resource: lifecycle is externally managed")
                } else if w.primary && matches!(operation, Operation::Clean | Operation::Stop) {
                    Some("primary workspace is protected")
                } else if r.is_reference() {
                    Some("shared/borrowed reference is protected")
                } else if protected_declarations.contains(&r.id) {
                    Some("current declaration is a protected reference")
                } else {
                    None
                };
                if let Some(reason) = exclude {
                    if !matches!(operation, Operation::Stop | Operation::Clean)
                        && prerequisites.contains(&id)
                    {
                        match prerequisite(&w, r, receipt, &self.registry.recovery_dir) {
                            Ok(proof) => {
                                plan.observations.insert(id.clone(), proof);
                            }
                            Err(e) => {
                                blockers.push(format!("{} prerequisite unavailable: {e}", r.id))
                            }
                        }
                    }
                    plan.exclusions.push(Exclusion {
                        id,
                        reason: reason.into(),
                    });
                    continue;
                }
                if matches!(operation, Operation::Clean | Operation::Stop) && receipt.is_none() {
                    plan.exclusions.push(Exclusion {
                        id,
                        reason: "no owned allocation".into(),
                    });
                    continue;
                }
                if primary_targets.contains(&target) {
                    blockers.push(format!(
                        "{} overlaps a current primary resource declaration",
                        r.id
                    ));
                    continue;
                }
                if matches!(operation, Operation::Build | Operation::Check) {
                    match prerequisite(&w, r, receipt, &self.registry.recovery_dir) {
                        Ok(proof) => {
                            plan.observations.insert(id.clone(), proof);
                        }
                        Err(e) => blockers.push(format!("{} prerequisite unavailable: {e}", r.id)),
                    }
                    continue;
                }
                if let Some(other) = g.claim(&target)?
                    && (other.workspace_id != w.id || other.resource_id != r.id)
                {
                    blockers.push(format!(
                        "{} target is claimed by protected/other workspace {}",
                        r.id, other.workspace_id
                    ));
                    continue;
                }
                // A target change cannot silently strand an old allocation.
                if receipt.is_some_and(|c| c.target != target) {
                    blockers.push(format!(
                        "{} has an existing receipt for another target; clean it first",
                        r.id
                    ));
                    continue;
                }
                if receipt.is_some_and(|c| {
                    c.declaration.adapter != r.adapter
                        || c.declaration.ownership != r.ownership
                        || c.declaration.ports != r.ports
                }) && !matches!(operation, Operation::Clean | Operation::Stop)
                {
                    blockers.push(format!(
                        "{} allocating adapter/ownership/ports changed; clean original first",
                        r.id
                    ));
                    continue;
                }
                let token = receipt.map(|c| c.owner_token.as_str()).unwrap_or("");
                let proof = match adapter::inspect(
                    &w,
                    r,
                    token,
                    receipt.map(|c| &c.proof),
                    &self.registry.recovery_dir,
                ) {
                    Ok(p) => p,
                    Err(e) => {
                        blockers.push(format!("{} inspection failed: {e}", r.id));
                        continue;
                    }
                };
                plan.observations.insert(id.clone(), proof.clone());
                if operation == Operation::Adopt {
                    // Adoption claims existing unmarked state, never a recorded allocation.
                    let refusal = if receipt.is_some() {
                        Some("already has an allocation receipt; forget it before adopting".into())
                    } else if !adapter::supports(r, "adopt") {
                        Some("lacks adopt capability".into())
                    } else if proof.presence != Presence::Present || proof.instance.is_some() {
                        Some("adopt needs existing state without an ownership marker".into())
                    } else {
                        adapter::adoptable(&w, r).err().map(|e| e.message)
                    };
                    match refusal {
                        Some(reason) => blockers.push(format!("{} {reason}", r.id)),
                        None => plan.steps.push(Step {
                            id,
                            action: operation,
                            target,
                            destructive: false,
                        }),
                    }
                    continue;
                }
                if proof.presence == Presence::Missing
                    && r.adapter.as_ref().is_some_and(Adapter::is_external)
                    && receipt.is_some_and(|c| missing_external_blocks(c, operation))
                    && matches!(operation, Operation::Prepare | Operation::Clean)
                {
                    let advice = if receipt.is_some_and(|c| c.state == "destroying") {
                        "run clean to finish the interrupted destroy"
                    } else {
                        "verify provider context, then explicitly forget it before recreating or retiring ownership"
                    };
                    blockers.push(format!(
                        "{} provider reports missing for a recorded allocation; {advice}",
                        r.id
                    ));
                    continue;
                }
                if proof.presence == Presence::Unknown {
                    blockers.push(format!("{} provider state is unknown", r.id));
                    continue;
                }
                if proof.presence == Presence::Present
                    && receipt.is_none_or(|c| !proof_matches(&proof, c))
                {
                    if receipt.is_none()
                        && proof.instance.is_none()
                        && adapter::supports(r, "adopt")
                    {
                        blockers.push(format!(
                            "{} exists without a TreeTender ownership marker; if it is disposable, review adopt --only {}",
                            r.id, r.id
                        ));
                    } else {
                        blockers.push(format!("{} ownership/instance proof does not match", r.id));
                    }
                    continue;
                }
                if operation == Operation::Prepare && proof.presence == Presence::Present {
                    if receipt.is_some_and(|c| {
                        matches!(c.state.as_str(), "preparing" | "uncertain" | "intent")
                    }) {
                        plan.steps.push(Step {
                            id: id.clone(),
                            action: operation,
                            target,
                            destructive: false,
                        });
                    } else {
                        plan.exclusions.push(Exclusion {
                            id: id.clone(),
                            reason: "already prepared".into(),
                        });
                    }
                    continue;
                }
                if operation != Operation::Prepare && receipt.is_none() {
                    plan.exclusions.push(Exclusion {
                        id,
                        reason: "no owned allocation".into(),
                    });
                    continue;
                }
                if matches!(operation, Operation::Start | Operation::Stop)
                    && proof.presence == Presence::Missing
                {
                    blockers.push(format!("{} is missing; prepare first", r.id));
                    continue;
                }
                if matches!(operation, Operation::Start | Operation::Stop)
                    && receipt.is_some_and(|c| {
                        pending(c)
                            && !((operation == Operation::Start && c.state == "starting")
                                || (operation == Operation::Stop && c.state == "stopping"))
                    })
                {
                    blockers.push(format!(
                        "{} has uncertain lifecycle state; reconcile with prepare first",
                        r.id
                    ));
                    continue;
                }
                if matches!(operation, Operation::Start | Operation::Stop)
                    && !adapter::supports(r, action(operation))
                {
                    plan.exclusions.push(Exclusion {
                        id,
                        reason: "no service lifecycle capability".into(),
                    });
                    continue;
                }
                if !adapter::supports(r, action(operation)) {
                    blockers.push(format!("{} lacks {} capability", r.id, action(operation)));
                    continue;
                }
                if operation == Operation::Prepare
                    && let Some(Adapter::File {
                        source,
                        source_root,
                        mode,
                        optional,
                        ..
                    }) = &r.adapter
                {
                    let source_path = match source_root {
                        SourceRoot::Workspace => w.path.clone(),
                        SourceRoot::Primary => git::discover(&w.path)?.primary_path,
                    };
                    match adapter::source_material(&w, &source_path, source, *mode) {
                        Ok(Some(hash)) => {
                            material.insert(format!("source:{id}:{}", source_path.display()), hash);
                        }
                        Ok(None) if *optional => {
                            plan.exclusions.push(Exclusion {
                                id,
                                reason: "optional source is absent".into(),
                            });
                            continue;
                        }
                        Ok(None) => blockers.push(format!("{id} source {source} is missing")),
                        Err(e) => blockers.push(format!("{id} source unavailable: {e}")),
                    }
                }
                plan.steps.push(Step {
                    id,
                    action: operation,
                    target,
                    destructive: operation == Operation::Clean,
                });
            } else if let Some(t) = manifest.tasks.iter().find(|t| t.id == id) {
                if !matches!(operation, Operation::Build | Operation::Check) {
                    blockers.push(format!(
                        "{} task must be run separately before resource operation",
                        t.id
                    ));
                    continue;
                }
                if selected(
                    &plan.selectors.retain,
                    &t.id,
                    if t.kind == TaskKind::Build {
                        "build"
                    } else {
                        "check"
                    },
                ) {
                    plan.exclusions.push(Exclusion {
                        id,
                        reason: "explicitly retained".into(),
                    });
                    continue;
                }
                for input in &t.inputs {
                    match adapter::tree_snapshot(&w.path, input) {
                        Ok(hash) => {
                            material.insert(format!("input:{id}:{input}"), hash);
                        }
                        Err(e) => blockers.push(format!("{id} input unavailable: {e}")),
                    }
                }
                plan.steps.push(Step {
                    id,
                    action: operation,
                    target: "task".into(),
                    destructive: false,
                });
            }
        }
        if operation == Operation::Clean || operation == Operation::Stop {
            let removed: BTreeSet<_> = plan.steps.iter().map(|s| s.id.as_str()).collect();
            for r in &manifest.resources {
                if !removed.contains(r.id.as_str())
                    && (if operation == Operation::Stop {
                        receipts
                            .iter()
                            .any(|c| c.resource_id == r.id && c.state == "active")
                    } else {
                        r.is_reference()
                            || receipts
                                .iter()
                                .any(|c| c.resource_id == r.id && c.state != "reserved")
                    })
                    && r.depends_on.iter().any(|d| removed.contains(d.as_str()))
                {
                    blockers.push(format!(
                        "retained {} depends on a selected resource; retain its dependencies too",
                        r.id
                    ));
                }
            }
        }
        // Freeze adapter executables and workspace-local script arguments when available.
        for r in &manifest.resources {
            if !checkout_current
                && !plan.steps.iter().any(|step| step.id == r.id)
                && !plan.observations.contains_key(&r.id)
            {
                continue;
            }
            if let Some(Adapter::Postgres { command, .. }) = &r.adapter {
                self.command_material(&self.registry.recovery_dir, command, &mut material)?;
            }
            if let Some(Adapter::Script { command, .. }) = &r.adapter {
                if checkout_current {
                    self.command_material(&w.path, command, &mut material)?;
                } else {
                    match adapter::recovery_command(&w, command) {
                        Ok(argv) => self.command_material(
                            &self.registry.recovery_dir,
                            &argv,
                            &mut material,
                        )?,
                        Err(e) => blockers.push(format!("{}: {e}", r.id)),
                    }
                }
            }
        }
        for t in &manifest.tasks {
            if checkout_current {
                self.command_material(&w.path, &t.command, &mut material)?;
            }
        }
        plan.blockers = blockers;
        let claims = plan
            .steps
            .iter()
            .map(|step| Ok((step.target.clone(), g.claim(&step.target)?)))
            .collect::<Result<BTreeMap<_, _>>>()?;
        plan.fingerprint = adapter::digest(&serde_json::to_vec(&(
            &plan,
            &material,
            &reference_material,
            &receipts,
            &claims,
        ))?);
        Ok(plan)
    }
    fn checkout_absent(&self, w: &Workspace) -> Result<bool> {
        git::is_current(w).map(|current| !current)
    }

    fn plan_forget(&self, g: &Guard, w: Workspace, selectors: Selectors) -> Result<Plan> {
        let receipts = g.claims(Some(&w.id))?;
        let mut manifest = g.declarations(&w.id)?;
        for receipt in &receipts {
            manifest.resources.retain(|r| r.id != receipt.resource_id);
            manifest.resources.push(receipt.declaration.clone());
        }
        manifest.resources.sort_by(|a, b| a.id.cmp(&b.id));
        let mut location_issue = None;
        let orphaned = match self.checkout_absent(&w) {
            Ok(orphaned) => orphaned,
            Err(error) if error.code == "checkout_location_unknown" => {
                location_issue = Some(error.message);
                false
            }
            Err(error) => return Err(error),
        };
        let mut plan = Plan {
            version: 1,
            workspace: w.clone(),
            operation: Operation::Forget,
            selectors,
            steps: vec![],
            exclusions: vec![],
            blockers: vec![],
            fingerprint: String::new(),
            manifest,
            observations: BTreeMap::new(),
        };
        if let Some(issue) = location_issue {
            plan.blockers.push(issue);
        }
        if w.primary {
            plan.blockers
                .push("primary workspace ownership records are protected".into());
        }
        if plan.selectors.only.is_empty() {
            plan.blockers.push("forget requires explicit --only RESOURCE selection; instances are retained and claims/ports may be released".into());
        }
        for selector in plan.selectors.only.iter().chain(&plan.selectors.retain) {
            if !plan
                .manifest
                .resources
                .iter()
                .any(|r| matches(selector, &r.id, &r.kind))
                && !selector
                    .strip_prefix("port:")
                    .is_some_and(|name| w.ports.contains_key(name))
            {
                return Err(Error::new(
                    "selector",
                    format!("unknown resource selector {selector}"),
                ));
            }
        }
        for name in w.ports.keys() {
            let id = format!("port:{name}");
            if !plan.selectors.only.contains(&id) {
                continue;
            }
            if plan.selectors.retain.contains(&id) {
                plan.exclusions.push(Exclusion {
                    id,
                    reason: "explicitly retained".into(),
                });
                continue;
            }
            plan.steps.push(Step {
                id,
                action: Operation::Forget,
                target: format!("lease:{}:{name}", w.id),
                destructive: false,
            });
        }
        for resource in &plan.manifest.resources {
            if !selected(&plan.selectors.only, &resource.id, &resource.kind) {
                continue;
            }
            if selected(&plan.selectors.retain, &resource.id, &resource.kind) {
                plan.exclusions.push(Exclusion {
                    id: resource.id.clone(),
                    reason: "explicitly retained".into(),
                });
                continue;
            }
            let receipt = receipts.iter().find(|r| r.resource_id == resource.id);
            if receipt.is_none() && (!orphaned || !resource.is_reference()) {
                plan.blockers.push(format!(
                    "{} has no owned claim; edit and re-register a live reference instead",
                    resource.id
                ));
                continue;
            }
            plan.steps.push(Step {
                id: resource.id.clone(),
                action: Operation::Forget,
                target: receipt
                    .map(|r| r.target.clone())
                    .unwrap_or_else(|| format!("reference:{}:{}", w.id, resource.id)),
                destructive: false,
            });
            if let Some(receipt) = receipt {
                plan.observations
                    .insert(resource.id.clone(), receipt.proof.clone());
            }
        }
        plan.fingerprint = adapter::digest(&serde_json::to_vec(&(&plan, &receipts, orphaned))?);
        Ok(plan)
    }

    fn command_material(
        &self,
        root: &Path,
        argv: &[String],
        material: &mut BTreeMap<String, String>,
    ) -> Result<()> {
        use std::os::unix::fs::PermissionsExt;
        if let Some(program) = argv.first() {
            let declared = Path::new(program);
            let executable = if declared.is_absolute() {
                Some(declared.to_path_buf())
            } else if program.contains('/') {
                Some(root.join(declared))
            } else {
                std::env::var_os("PATH").and_then(|paths| {
                    std::env::split_paths(&paths)
                        .map(|directory| {
                            if directory.is_absolute() {
                                directory.join(program)
                            } else {
                                root.join(directory).join(program)
                            }
                        })
                        .find(|candidate| {
                            candidate
                                .metadata()
                                .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
                        })
                })
            };
            match executable.and_then(|path| path.canonicalize().ok()) {
                Some(path) => {
                    material.insert(
                        format!("executable:{program}:{}", path.display()),
                        adapter::digest(&std::fs::read(path)?),
                    );
                }
                None => {
                    material.insert(format!("executable:{program}"), "unavailable".into());
                }
            }
        }
        for arg in argv.iter().skip(1) {
            let path = Path::new(arg);
            let resolved = if path.is_absolute() {
                path.to_path_buf()
            } else {
                root.join(path)
            };
            if resolved.is_file() {
                if path.is_absolute() {
                    material.insert(
                        format!("command:{}", resolved.display()),
                        adapter::digest(&std::fs::read(&resolved)?),
                    );
                } else if crate::manifest::relative(arg).is_ok() {
                    material.insert(
                        format!("command:{}", resolved.display()),
                        adapter::file_snapshot(root, arg)?,
                    );
                }
            }
        }
        Ok(())
    }
    pub fn apply(&self, plan: &Plan) -> Result<ApplyResult> {
        let g = self.registry.lock()?;
        let w = g.resolve(Path::new(&plan.workspace.id))?;
        let fresh = self.plan_locked(&g, w, plan.operation, plan.selectors.clone())?;
        if &fresh != plan {
            return Err(Error::new(
                "stale_plan",
                "material inputs, ownership, plan contents or registry generation changed; review a new plan",
            ));
        }
        if !plan.blockers.is_empty() {
            return Err(Error::new("plan_blocked", plan.blockers.join("; ")));
        }
        let mut record = OperationRecord {
            id: uuid::Uuid::new_v4().to_string(),
            workspace_id: plan.workspace.id.clone(),
            operation: plan.operation,
            state: "running".into(),
            steps: vec![],
            failure: None,
        };
        g.save_operation(&record)?;
        let mut result = ApplyResult {
            operation_id: record.id.clone(),
            success: true,
            completed: vec![],
            failure: None,
            recovery: vec![],
        };
        for step in &plan.steps {
            record.steps.push(JournalStep {
                id: step.id.clone(),
                state: "intent".into(),
                detail: if plan.operation == Operation::Forget {
                    Some(serde_json::to_string(&(
                        g.claim(&step.target)?,
                        plan.manifest.resources.iter().find(|r| r.id == step.id),
                    ))?)
                } else {
                    None
                },
            });
            g.save_operation(&record)?;
            match self.execute_step(&g, plan, step) {
                Ok(detail) => {
                    let journal = record.steps.last_mut().unwrap();
                    journal.state = "completed".into();
                    journal.detail = detail;
                    result.completed.push(step.id.clone());
                    g.save_operation(&record)?;
                }
                Err(e) => {
                    record.steps.last_mut().unwrap().state = "uncertain".into();
                    record.steps.last_mut().unwrap().detail = Some(e.to_string());
                    record.state = "failed".into();
                    record.failure = Some(e.clone());
                    g.save_operation(&record)?;
                    result.success = false;
                    result.failure = Some(e);
                    result.recovery.push(format!("Inspect {} and create a fresh plan with the original selectors {}. Allocation intent and target/port claims are retained; retry the intended operation, while unknown state blocks cleanup.",step.id,serde_json::to_string(&plan.selectors)?));
                    return Ok(result);
                }
            }
        }
        record.state = "completed".into();
        g.save_operation(&record)?;
        Ok(result)
    }
    fn execute_step(&self, g: &Guard, plan: &Plan, step: &Step) -> Result<Option<String>> {
        let w = &plan.workspace;
        if step.action == Operation::Forget {
            let orphaned = self.checkout_absent(w)?;
            let transaction = rusqlite::Transaction::new_unchecked(
                &g.db,
                rusqlite::TransactionBehavior::Immediate,
            )?;
            if step.target.starts_with("lease:") {
                g.release(&step.target)?;
                transaction.commit()?;
                return Ok(Some(serde_json::to_string(
                    &serde_json::json!({"released_lease":step.target,"port":w.ports.get(&step.id["port:".len()..])}),
                )?));
            }
            let receipt = g.claim(&step.target)?;
            if let Some(receipt) = &receipt {
                if receipt.workspace_id != w.id || receipt.resource_id != step.id {
                    return Err(Error::new(
                        "ownership",
                        "claim belongs to another workspace",
                    ));
                }
                g.delete_claim(&step.target)?;
            }
            let mut declarations = g.declarations(&w.id)?;
            if orphaned {
                declarations
                    .resources
                    .retain(|r| r.id != step.id || !r.is_reference());
                g.save_declarations(w, &declarations)?;
            }
            transaction.commit()?;
            return Ok(Some(serde_json::to_string(
                &serde_json::json!({"forgotten_receipt":receipt,"released_declaration":plan.manifest.resources.iter().find(|r|r.id == step.id),"instance_retained":true,"released_target":step.target}),
            )?));
        }
        if let Some(task) = plan.manifest.tasks.iter().find(|t| t.id == step.id) {
            let inputs = crate::build::input_fingerprints(&w.path, task)?;
            adapter::run(
                &task.command,
                &w.path,
                &w.environment(),
                None,
                task.timeout_secs,
            )?;
            return Ok(Some(crate::build::completed(&w.path, task, inputs)?));
        }
        let resource = plan
            .manifest
            .resources
            .iter()
            .find(|r| r.id == step.id)
            .ok_or_else(|| Error::new("plan", "missing resource"))?;
        if resource.is_reference()
            || (w.primary && matches!(step.action, Operation::Stop | Operation::Clean))
        {
            return Err(Error::new("protected", "resource is protected"));
        }
        let recorded = g.claim(&step.target)?;
        let previously_claimed = recorded.is_some();
        let mut receipt = match recorded {
            Some(r) => r,
            None if matches!(step.action, Operation::Prepare | Operation::Adopt) => Receipt {
                resource_id: step.id.clone(),
                target: step.target.clone(),
                workspace_id: w.id.clone(),
                owner_token: uuid::Uuid::new_v4().to_string(),
                proof: Proof::missing(),
                generation: 0,
                state: "intent".into(),
                declaration: resource.clone(),
            },
            None => return Err(Error::new("ownership", "no recorded allocation")),
        };
        if receipt.workspace_id != w.id || receipt.resource_id != step.id {
            return Err(Error::new(
                "ownership",
                "allocation belongs to another workspace",
            ));
        }
        let current = adapter::inspect(
            w,
            resource,
            &receipt.owner_token,
            Some(&receipt.proof),
            &self.registry.recovery_dir,
        )?;
        if current.presence == Presence::Missing
            && previously_claimed
            && missing_external_blocks(&receipt, step.action)
            && resource.adapter.as_ref().is_some_and(Adapter::is_external)
            && matches!(step.action, Operation::Clean | Operation::Prepare)
        {
            return Err(Error::new(
                "provider_missing",
                "recorded external allocation reports missing; preserve claim and explicitly forget after verifying provider context",
            ));
        }

        if step.action == Operation::Adopt
            && (previously_claimed
                || current.presence != Presence::Present
                || current.instance.is_some())
        {
            return Err(Error::new(
                "ownership",
                "adopt target changed since review or is already claimed",
            ));
        }
        if current.presence == Presence::Unknown
            || (step.action != Operation::Adopt
                && current.presence == Presence::Present
                && !proof_matches(&current, &receipt))
        {
            return Err(Error::new(
                "ownership",
                "current exact instance/owner proof unavailable or mismatched",
            ));
        }
        if step.action == Operation::Clean && current.presence == Presence::Missing {
            g.delete_claim(&receipt.target)?;
            return Ok(Some("provider proves missing; retired receipt".into()));
        }
        if step.action == Operation::Prepare && current.presence == Presence::Present {
            receipt.proof = current;
            receipt.state = "prepared".into();
            receipt.generation += 1;
            g.save_claim(&receipt)?;
            return Ok(Some("reconciled existing exact allocation".into()));
        }
        // Allocation/ownership intent is durable before any external effect.
        receipt.state = match step.action {
            Operation::Prepare | Operation::Adopt => "preparing",
            Operation::Start => "starting",
            Operation::Stop => "stopping",
            Operation::Clean => "destroying",
            _ => return Err(Error::new("operation", "unexpected allocation operation")),
        }
        .into();
        receipt.generation += 1;
        g.save_claim(&receipt)?;
        let proof = adapter::invoke(
            w,
            resource,
            action(step.action),
            &receipt.owner_token,
            Some(&current),
            &self.registry.recovery_dir,
        )?;
        match step.action {
            Operation::Clean => {
                if proof.presence != Presence::Missing {
                    return Err(Error::new("adapter_proof", "destroy must report missing"));
                }
                let checked = adapter::inspect(
                    w,
                    resource,
                    &receipt.owner_token,
                    Some(&current),
                    &self.registry.recovery_dir,
                )?;
                if checked.presence != Presence::Missing {
                    return Err(Error::new(
                        "adapter_proof",
                        "provider did not confirm destruction",
                    ));
                }
                g.delete_claim(&receipt.target)?;
            }
            _ => {
                if proof.presence != Presence::Present
                    || proof.owner_token.as_deref() != Some(&receipt.owner_token)
                    || proof.instance.as_ref().is_none_or(|s| s.is_empty())
                    || (current.presence == Presence::Present
                        && step.action != Operation::Adopt
                        && proof.instance != current.instance)
                {
                    return Err(Error::new(
                        "adapter_proof",
                        "effect did not return matching owner and exact instance",
                    ));
                }
                receipt.proof = proof;
                let checked = adapter::inspect(
                    w,
                    resource,
                    &receipt.owner_token,
                    Some(&receipt.proof),
                    &self.registry.recovery_dir,
                )?;
                if !proof_matches(&checked, &receipt) {
                    return Err(Error::new(
                        "adapter_proof",
                        "inspection after effect failed ownership validation",
                    ));
                }
                receipt.proof = checked;
                receipt.state = if step.action == Operation::Start {
                    "active"
                } else {
                    "prepared"
                }
                .into();
                g.save_claim(&receipt)?;
            }
        }
        Ok(None)
    }
}
