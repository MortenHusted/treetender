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
fn prerequisite(
    w: &Workspace,
    r: &crate::manifest::Resource,
    receipt: Option<&Receipt>,
) -> Result<Proof> {
    let proof = adapter::inspect(
        w,
        r,
        receipt.map(|c| c.owner_token.as_str()).unwrap_or(""),
        receipt.map(|c| &c.proof),
    )?;
    if proof.presence != Presence::Present {
        return Err(Error::new("prerequisite", "provider must prove present"));
    }
    if r.ownership == Ownership::Owned
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
            primary_path: d.primary_path,
            primary: d.git_dir == d.common_dir,
            workspace: g
                .workspaces()?
                .into_iter()
                .find(|w| w.git_dir == d.git_dir && w.path == d.path),
            manifest: Manifest::load(&d.path)?,
        })
    }
    pub fn register(&self, path: &Path) -> Result<Workspace> {
        let g = self.registry.lock()?;
        let d = git::discover(path)?;
        // Validate before writing any Git identity markers.
        Manifest::load(&d.path)?;
        let repo = git::identity(&d.common_dir, true)?.unwrap();
        let primary = Workspace {
            id: repo.clone(),
            repository_id: repo.clone(),
            path: d.primary_path.clone(),
            git_dir: d.common_dir.clone(),
            common_dir: d.common_dir.clone(),
            primary: true,
        };
        g.save_workspace(&primary)?;
        let manifest = self.resolved_manifest(&primary)?;
        g.save_declarations(&primary, &manifest)?;
        for r in &manifest.resources {
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
            return Ok(primary);
        }
        let id = git::identity(&d.git_dir, true)?.unwrap();
        let w = Workspace {
            id,
            repository_id: repo,
            path: d.path,
            git_dir: d.git_dir,
            common_dir: d.common_dir,
            primary: false,
        };
        g.save_workspace(&w)?;
        g.save_declarations(&w, &self.resolved_manifest(&w)?)?;
        Ok(w)
    }
    pub fn list(&self) -> Result<Vec<Workspace>> {
        self.registry.lock()?.workspaces()
    }
    pub fn status(&self, workspace: &str) -> Result<Status> {
        let g = self.registry.lock()?;
        let w = g.resolve(Path::new(workspace))?;
        let resources = g.claims(Some(&w.id))?;
        let mut observations = BTreeMap::new();
        let mut issues = vec![];
        let mut manifest = self.resolved_manifest(&w)?;
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
            let receipt = resources.iter().find(|c| c.resource_id == r.id);
            match adapter::inspect(
                &w,
                r,
                receipt.map(|c| c.owner_token.as_str()).unwrap_or(""),
                receipt.map(|c| &c.proof),
            ) {
                Ok(p) => {
                    observations.insert(r.id.clone(), p);
                }
                Err(e) => issues.push(format!("{}: {e}", r.id)),
            }
        }
        Ok(Status {
            resources,
            operations: g.history(&w.id)?,
            workspace: w,
            observations,
            issues,
        })
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
    fn resolved_manifest(&self, w: &Workspace) -> Result<Manifest> {
        let mut m = if w.path.exists() {
            Manifest::load(&w.path)?
        } else {
            Manifest::default()
        };
        for r in &mut m.resources {
            r.target = r
                .target
                .replace("{workspace_id}", &w.id)
                .replace("{repository_id}", &w.repository_id);
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
        let mut blockers = vec![];
        let receipts = g.claims(Some(&w.id))?;
        let mut manifest = self.resolved_manifest(&w)?;
        let protected_declarations: BTreeSet<String> = manifest
            .resources
            .iter()
            .filter(|r| r.ownership != Ownership::Owned)
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
        if matches!(operation, Operation::Clean | Operation::Stop) {
            for other in g.workspaces()? {
                if other.id == w.id {
                    continue;
                }
                let stored = g.declarations(&other.id)?;
                let current = self.resolved_manifest(&other)?;
                reference_material.insert(other.id.clone(), (stored.clone(), current.clone()));
                for r in stored.resources.iter().chain(&current.resources) {
                    if other.primary || r.ownership != Ownership::Owned {
                        primary_targets.insert(adapter::target(&other, r));
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
        if !matches!(operation, Operation::Clean | Operation::Stop) {
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
            .filter(|r| wanted.contains(&r.id))
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
            registry_generation: g.generation()?,
        };
        let mut material = BTreeMap::<String, String>::new();
        if w.path.exists() {
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
            // Primary declarations are material safety inputs even for linked checkouts.
            if !w.primary && d.primary_path.exists() {
                material.insert(
                    "primary_manifest".into(),
                    serde_json::to_string(&Manifest::load(&d.primary_path)?)?,
                );
            }
        } else if !matches!(operation, Operation::Clean | Operation::Stop) {
            blockers
                .push("checkout is missing; only receipt-based cleanup/stop is available".into());
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
                } else if w.primary && matches!(operation, Operation::Clean | Operation::Stop) {
                    Some("primary workspace is protected")
                } else if r.ownership != Ownership::Owned {
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
                        match prerequisite(&w, r, receipt) {
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
                if primary_targets.contains(&target) {
                    blockers.push(format!(
                        "{} overlaps a current primary resource declaration",
                        r.id
                    ));
                    continue;
                }
                if matches!(operation, Operation::Build | Operation::Check) {
                    match prerequisite(&w, r, receipt) {
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
                let proof = match adapter::inspect(&w, r, token, receipt.map(|c| &c.proof)) {
                    Ok(p) => p,
                    Err(e) => {
                        blockers.push(format!("{} inspection failed: {e}", r.id));
                        continue;
                    }
                };
                plan.observations.insert(id.clone(), proof.clone());
                if proof.presence == Presence::Unknown {
                    blockers.push(format!("{} provider state is unknown", r.id));
                    continue;
                }
                if proof.presence == Presence::Present
                    && receipt.is_none_or(|c| !proof_matches(&proof, c))
                {
                    blockers.push(format!("{} ownership/instance proof does not match", r.id));
                    continue;
                }
                if operation == Operation::Prepare && proof.presence == Presence::Present {
                    if receipt.is_some_and(|c| c.state == "uncertain" || c.state == "intent") {
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
                    && receipt.is_some_and(|c| c.state == "uncertain" || c.state == "intent")
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
                    && let Adapter::File {
                        source,
                        source_root,
                        ..
                    } = &r.adapter
                {
                    let source_path = match source_root {
                        SourceRoot::Workspace => w.path.clone(),
                        SourceRoot::Primary => git::discover(&w.path)?.primary_path,
                    };
                    match adapter::file_snapshot(&source_path, source) {
                        Ok(hash) => {
                            material.insert(format!("source:{id}:{}", source_path.display()), hash);
                        }
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
                        r.ownership != Ownership::Owned
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
            if let Adapter::Script { command, .. } = &r.adapter {
                self.command_material(&w.path, command, &mut material)?;
            }
        }
        for t in &manifest.tasks {
            self.command_material(&w.path, &t.command, &mut material)?;
        }
        plan.blockers = blockers;
        plan.fingerprint = adapter::digest(&serde_json::to_vec(&(
            &plan,
            &material,
            &reference_material,
        ))?);
        Ok(plan)
    }
    fn command_material(
        &self,
        root: &Path,
        argv: &[String],
        material: &mut BTreeMap<String, String>,
    ) -> Result<()> {
        for arg in argv {
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
                detail: None,
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
                    result.recovery.push(format!("Inspect {} and create a fresh plan. Uncertain allocations retain their target/port claims; matching provider token can be reconciled, unknown state blocks cleanup.",step.id));
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
        if let Some(task) = plan.manifest.tasks.iter().find(|t| t.id == step.id) {
            adapter::run(&task.command, &w.path, None, task.timeout_secs)?;
            let mut artifacts = BTreeMap::new();
            for output in &task.outputs {
                artifacts.insert(output, adapter::tree_snapshot(&w.path, output)?);
            }
            return Ok(Some(serde_json::to_string(
                &serde_json::json!({"artifacts":artifacts,"kind":task.kind}),
            )?));
        }
        let resource = plan
            .manifest
            .resources
            .iter()
            .find(|r| r.id == step.id)
            .ok_or_else(|| Error::new("plan", "missing resource"))?;
        if resource.ownership != Ownership::Owned
            || (w.primary && matches!(step.action, Operation::Stop | Operation::Clean))
        {
            return Err(Error::new("protected", "resource is protected"));
        }
        let mut receipt = match g.claim(&step.target)? {
            Some(r) => r,
            None if step.action == Operation::Prepare => Receipt {
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
        let current = adapter::inspect(w, resource, &receipt.owner_token, Some(&receipt.proof))?;
        if current.presence == Presence::Unknown
            || (current.presence == Presence::Present && !proof_matches(&current, &receipt))
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
        receipt.state = "uncertain".into();
        receipt.generation += 1;
        g.save_claim(&receipt)?;
        let proof = adapter::invoke(
            w,
            resource,
            action(step.action),
            &receipt.owner_token,
            Some(&current),
        )?;
        match step.action {
            Operation::Clean => {
                if proof.presence != Presence::Missing {
                    return Err(Error::new("adapter_proof", "destroy must report missing"));
                }
                let checked = adapter::inspect(w, resource, &receipt.owner_token, Some(&current))?;
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
                    || (current.presence == Presence::Present && proof.instance != current.instance)
                {
                    return Err(Error::new(
                        "adapter_proof",
                        "effect did not return matching owner and exact instance",
                    ));
                }
                receipt.proof = proof;
                let checked =
                    adapter::inspect(w, resource, &receipt.owner_token, Some(&receipt.proof))?;
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
