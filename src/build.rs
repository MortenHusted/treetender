//! Freshness covers declared inputs, command definitions and artifact contents.
use crate::{
    adapter,
    manifest::{Task, TaskKind},
    model::{Operation, OperationRecord, Result},
};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::Path};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BuildState {
    NeverBuilt,
    Fresh,
    Stale,
    Failed,
    Interrupted,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuildStatus {
    pub task_id: String,
    pub kind: TaskKind,
    pub state: BuildState,
    pub operation_id: Option<String>,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct BuildRecord {
    version: u32,
    task_definition: String,
    inputs: BTreeMap<String, String>,
    artifacts: BTreeMap<String, String>,
}

pub(crate) fn input_fingerprints(root: &Path, task: &Task) -> Result<BTreeMap<String, String>> {
    task.inputs
        .iter()
        .map(|path| Ok((path.clone(), adapter::tree_snapshot(root, path)?)))
        .collect()
}

pub(crate) fn completed(
    root: &Path,
    task: &Task,
    inputs: BTreeMap<String, String>,
) -> Result<String> {
    let artifacts = task
        .outputs
        .iter()
        .map(|path| Ok((path.clone(), adapter::tree_snapshot(root, path)?)))
        .collect::<Result<BTreeMap<_, _>>>()?;
    Ok(serde_json::to_string(&BuildRecord {
        version: 1,
        task_definition: adapter::digest(&serde_json::to_vec(task)?),
        inputs,
        artifacts,
    })?)
}

pub(crate) fn inspect(
    root: &Path,
    tasks: &[Task],
    history: &[OperationRecord],
) -> Vec<BuildStatus> {
    tasks.iter().map(|task| {
        let mut status = BuildStatus {task_id:task.id.clone(),kind:task.kind,state:BuildState::NeverBuilt,operation_id:None,reason:"No recorded attempt for this task".into()};
        let attempt = history.iter().rev().filter(|op|matches!(op.operation,Operation::Build|Operation::Check)).find_map(|op|op.steps.iter().find(|step|step.id == task.id).map(|step|(op,step)));
        let Some((operation,step)) = attempt else { return status; };
        status.operation_id = Some(operation.id.clone());
        if step.state != "completed" {
            status.state = if matches!(operation.state.as_str(),"running"|"interrupted") { BuildState::Interrupted } else { BuildState::Failed };
            status.reason = step.detail.clone().unwrap_or_else(||"The last task attempt did not complete".into());
            return status;
        }
        let Some(record) = step.detail.as_deref().and_then(|text|serde_json::from_str::<BuildRecord>(text).ok()).filter(|r|r.version == 1) else {
            status.state = BuildState::Unknown;
            status.reason = "This outcome has no comparable freshness record; run the task again".into();
            return status;
        };
        match (|| -> Result<bool> {
            if record.task_definition != adapter::digest(&serde_json::to_vec(task)?) || record.inputs != input_fingerprints(root,task)? { return Ok(false); }
            let current = task.outputs.iter().map(|path|Ok((path.clone(),adapter::tree_snapshot(root,path)?))).collect::<Result<BTreeMap<_,_>>>()?;
            Ok(record.artifacts == current)
        })() {
            Ok(true) => { status.state = BuildState::Fresh; status.reason = "Declared inputs, task definition and artifacts match the successful attempt".into(); },
            Ok(false) => { status.state = BuildState::Stale; status.reason = "Declared inputs, task definition or artifacts changed".into(); },
            Err(error) => { status.state = BuildState::Stale; status.reason = format!("Inputs or artifacts cannot be verified: {error}"); },
        }
        status
    }).collect()
}
