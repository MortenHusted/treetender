//! Portman integration. The daemon records which config root synced each
//! service; a lane root owns exactly the services registered under its path.
use crate::model::*;
use serde_json::{Value, json};
use std::io::{Read, Write};
use std::path::Path;
use std::time::Duration;

/// One request on the daemon socket: a four-byte big-endian length and tagged
/// JSON, one request per connection.
fn request(socket: &str, body: &Value) -> Result<Value> {
    use std::os::unix::net::UnixStream;
    let mut stream = UnixStream::connect(socket)
        .map_err(|e| Error::new("adapter_unavailable", e.to_string()))?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    let request = serde_json::to_vec(body)?;
    stream.write_all(&(request.len() as u32).to_be_bytes())?;
    stream.write_all(&request)?;
    let mut size = [0u8; 4];
    stream.read_exact(&mut size)?;
    let size = u32::from_be_bytes(size) as usize;
    if size > 1024 * 1024 {
        return Err(Error::new("adapter_output", "Portman frame exceeds 1 MiB"));
    }
    let mut body = vec![0u8; size];
    stream.read_exact(&mut body)?;
    let value: Value = serde_json::from_slice(&body)?;
    if value.get("kind").and_then(Value::as_str) == Some("err") {
        return Err(Error::new(
            "adapter_failed",
            format!(
                "Portman: {}",
                value
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("error")
            ),
        ));
    }
    Ok(value)
}

fn services(socket: &str) -> Result<Vec<Value>> {
    let value = request(socket, &json!({"kind": "service_status"}))?;
    if value.get("kind").and_then(Value::as_str) != Some("service_statuses") {
        return Err(Error::new(
            "adapter_proof",
            "Portman did not return service_statuses (authenticated brokers require a trusted wrapper)",
        ));
    }
    value
        .get("services")
        .and_then(Value::as_array)
        .cloned()
        .ok_or_else(|| Error::new("adapter_proof", "Portman omitted services"))
}

/// Read-only observation of one explicitly named service.
pub(crate) fn status(socket: &str, service: &str) -> Result<Proof> {
    let services = services(socket)?;
    let found: Vec<_> = services
        .iter()
        .filter(|s| s.get("name").and_then(Value::as_str) == Some(service))
        .collect();
    match found.as_slice() {
        [] => Ok(Proof::missing()),
        [value] => Ok(Proof {
            presence: Presence::Present,
            instance: Some(service.into()),
            owner_token: None,
            handle: (*value).clone(),
        }),
        _ => Err(Error::new(
            "adapter_proof",
            "Portman returned duplicate exact service names",
        )),
    }
}

/// The daemon socket Portman's CLI uses for the current user.
pub(crate) fn default_socket() -> Result<String> {
    let home = std::env::var_os("HOME")
        .map(std::path::PathBuf::from)
        .ok_or_else(|| Error::new("adapter_unavailable", "HOME is unset; set socket"))?;
    let data = if cfg!(target_os = "macos") {
        home.join("Library/Application Support")
    } else {
        std::env::var_os("XDG_DATA_HOME")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| home.join(".local/share"))
    };
    Ok(data
        .join("portman/portman.sock")
        .to_string_lossy()
        .into_owned())
}

/// Services synced from exactly this root. Roots compare as exact paths.
fn owned(socket: &str, root: &Path) -> Result<Vec<String>> {
    Ok(services(socket)?
        .iter()
        .filter(|s| s.get("root").and_then(Value::as_str).map(Path::new) == Some(root))
        .filter_map(|s| s.get("name").and_then(Value::as_str).map(String::from))
        .collect())
}

/// Syncs and starts the root's own config. The CLI walks up from its working
/// directory, so a config must sit in the root itself, never only in an ancestor.
pub(crate) fn start(command: &[String], root: &Path, w: &Workspace, timeout: u64) -> Result<()> {
    if !["portman.toml", "portman.local.toml"]
        .iter()
        .any(|name| std::fs::symlink_metadata(root.join(name)).is_ok_and(|m| m.is_file()))
    {
        return Err(Error::new(
            "portman_config",
            format!(
                "{} has no portman.toml or portman.local.toml; prepare its config first",
                root.display()
            ),
        ));
    }
    let mut argv = command.to_vec();
    argv.push("up".into());
    crate::adapter::run(&argv, root, &w.environment(), None, timeout)?;
    Ok(())
}

/// Stops only the services this root owns, by explicit name.
pub(crate) fn stop(socket: &str, root: &Path) -> Result<()> {
    let names = owned(socket, root)?;
    if !names.is_empty() {
        request(socket, &json!({"kind": "service_down", "names": names}))?;
    }
    Ok(())
}

/// Stops and unregisters everything the root owns, then confirms none remain.
pub(crate) fn forget(
    command: &[String],
    socket: &str,
    root: &Path,
    w: &Workspace,
    cwd: &Path,
    timeout: u64,
) -> Result<()> {
    let mut argv = command.to_vec();
    argv.extend(["down".into(), "--forget".into(), "--root".into()]);
    argv.push(root.to_string_lossy().into_owned());
    crate::adapter::run(&argv, cwd, &w.environment(), None, timeout)?;
    let remaining = owned(socket, root)?;
    if !remaining.is_empty() {
        return Err(Error::new(
            "adapter_proof",
            format!(
                "Portman still lists {} for {}",
                remaining.join(", "),
                root.display()
            ),
        ));
    }
    Ok(())
}
