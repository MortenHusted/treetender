//! Disposable provider fixture for database/storage namespaces, not a real backend.
use anyhow::{Context, Result, bail};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::{self, Read},
    path::PathBuf,
};
use treetender::{AdapterRequest, AdapterResponse, Presence, Proof};

#[derive(Serialize, Deserialize)]
struct Namespace {
    instance: String,
    owner_token: String,
}

fn main() -> Result<()> {
    let mut input = String::new();
    io::stdin().take(1024 * 1024).read_to_string(&mut input)?;
    let request: AdapterRequest = serde_json::from_str(&input)?;
    if request.version != 1 {
        bail!("unsupported request version");
    }
    let root = PathBuf::from(
        std::env::var_os("TREETENDER_DEMO_ROOT")
            .context("set TREETENDER_DEMO_ROOT to a disposable directory")?,
    );
    let observing = request.action == "inspect";
    if observing && !root.exists() {
        println!(
            "{}",
            serde_json::to_string(&AdapterResponse {
                version: 1,
                proof: Proof::missing()
            })?
        );
        return Ok(());
    }
    if !observing {
        fs::create_dir_all(&root)?;
    }
    if fs::symlink_metadata(&root)?.file_type().is_symlink() {
        bail!("demo root cannot be a symlink");
    }
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(!observing)
        .create(!observing)
        .truncate(false)
        .open(root.join(".provider.lock"));
    let _lock = match lock {
        Ok(file) => {
            if observing {
                file.lock_shared()?;
            } else {
                file.lock_exclusive()?;
            }
            Some(file)
        }
        Err(e) if observing && e.kind() == io::ErrorKind::NotFound => None,
        Err(e) => return Err(e.into()),
    };
    let (kind, name) = request
        .target
        .split_once(':')
        .context("target must be database:NAME or storage:NAME")?;
    if !["database", "storage"].contains(&kind)
        || name.is_empty()
        || !name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        bail!("unsafe namespace target");
    }
    let path = root.join(format!("{kind}-{name}.json"));
    if fs::symlink_metadata(&path).is_ok_and(|m| !m.is_file() || m.file_type().is_symlink()) {
        bail!("namespace must be a regular fixture file");
    }
    let mut current = match fs::read(&path) {
        Ok(bytes) => Some(serde_json::from_slice::<Namespace>(&bytes)?),
        Err(e) if e.kind() == io::ErrorKind::NotFound => None,
        Err(e) => return Err(e.into()),
    };
    match request.action.as_str() {
        "inspect" => {}
        "prepare" => {
            if current.is_none() {
                let namespace = Namespace {
                    instance: uuid::Uuid::new_v4().to_string(),
                    owner_token: request.owner_token.clone(),
                };
                let mut file = fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&path)?;
                serde_json::to_writer(&mut file, &namespace)?;
                file.sync_all()?;
                current = Some(namespace);
            }
        }
        "destroy" => {
            if request.workspace.primary {
                bail!("primary namespaces are protected even when invoked directly");
            }
            if let Some(namespace) = &current {
                let prior = request
                    .receipt
                    .as_ref()
                    .context("destroy requires an exact prior receipt")?;
                if namespace.owner_token != request.owner_token
                    || prior.owner_token.as_ref() != Some(&namespace.owner_token)
                    || prior.instance.as_ref() != Some(&namespace.instance)
                {
                    bail!("namespace ownership mismatch");
                }
                fs::remove_file(&path)?;
                current = None;
            }
        }
        _ => bail!("fixture supports inspect/prepare/destroy only"),
    }
    let proof = match current {
        Some(namespace) => Proof {
            presence: Presence::Present,
            instance: Some(namespace.instance),
            owner_token: Some(namespace.owner_token),
            handle: serde_json::json!({"backend":kind,"namespace":name}),
        },
        None => Proof::missing(),
    };
    println!(
        "{}",
        serde_json::to_string(&AdapterResponse { version: 1, proof })?
    );
    Ok(())
}
