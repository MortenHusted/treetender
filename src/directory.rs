//! Owned directories inside a checkout, such as build output and dependency
//! installs. Ownership is the marker this adapter writes into the directory;
//! a matching path alone is never proof.
use crate::adapter::{anchored_parent, cstring, dir_names, fd_file, parent};
use crate::model::*;
use serde::{Deserialize, Serialize};
use std::ffi::CString;
use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::MetadataExt;
use std::path::Path;

const MARKER: &str = ".treetender-owner.json";
// Each level holds a descriptor open; stay well below common descriptor limits.
const MAX_DEPTH: usize = 128;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Marker {
    version: u32,
    workspace_id: String,
    resource_id: String,
    owner_token: String,
    instance: String,
}

fn open_directory(dir: &File, name: &CString, path: &str) -> Result<Option<File>> {
    // SAFETY: anchored open that refuses a symlink or non-directory leaf.
    let fd = unsafe {
        libc::openat(
            dir.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        let e = std::io::Error::last_os_error();
        if e.kind() == std::io::ErrorKind::NotFound {
            return Ok(None);
        }
        return Err(Error::new(
            "unsafe_path",
            format!("{path} is not a real directory: {e}"),
        ));
    }
    fd_file(fd).map(Some)
}

fn read_marker(directory: &File) -> Result<Option<Marker>> {
    let name = cstring(MARKER)?;
    // SAFETY: the marker leaf is opened without following symlinks.
    let fd = unsafe {
        libc::openat(
            directory.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        let e = std::io::Error::last_os_error();
        if e.kind() == std::io::ErrorKind::NotFound {
            return Ok(None);
        }
        return Err(e.into());
    }
    let file = fd_file(fd)?;
    if !file.metadata()?.is_file() {
        return Err(Error::new(
            "ownership",
            "ownership marker is not a regular file",
        ));
    }
    let mut bytes = vec![];
    file.take(64 * 1024 + 1).read_to_end(&mut bytes)?;
    let marker: Marker = serde_json::from_slice(&bytes)
        .ok()
        .filter(|m: &Marker| bytes.len() <= 64 * 1024 && m.version == 1)
        .ok_or_else(|| Error::new("ownership", "unreadable ownership marker"))?;
    Ok(Some(marker))
}

fn write_marker(directory: &File, marker: &Marker) -> Result<()> {
    let name = cstring(MARKER)?;
    // SAFETY: O_EXCL refuses an existing marker or symlink atomically.
    let mut file = fd_file(unsafe {
        libc::openat(
            directory.as_raw_fd(),
            name.as_ptr(),
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            0o600,
        )
    })?;
    file.write_all(&serde_json::to_vec_pretty(marker)?)?;
    file.sync_all()?;
    directory.sync_all()?;
    Ok(())
}

fn proof_at(dir: &File, name: &CString, path: &str) -> Result<Proof> {
    let Some(directory) = open_directory(dir, name, path)? else {
        return Ok(Proof::missing());
    };
    let meta = directory.metadata()?;
    let marker = read_marker(&directory)?;
    Ok(Proof {
        presence: Presence::Present,
        handle: serde_json::json!({
            "path": path,
            "device": meta.dev(),
            "inode": meta.ino(),
            "marked": marker.is_some(),
        }),
        instance: marker.as_ref().map(|m| m.instance.clone()),
        owner_token: marker.map(|m| m.owner_token),
    })
}

fn open_capture(dir: &File, token: &str) -> Result<Option<File>> {
    let quarantine = cstring(&format!(".treetender-delete-{token}"))?;
    open_directory(dir, &quarantine, "cleanup capture")
}

/// Reads ownership from the marker, never from the request. An interrupted
/// destroy keeps the same instance inside its private capture.
pub(crate) fn proof(root: &Path, path: &str, token: &str) -> Result<Proof> {
    let Some((dir, name)) = anchored_parent(root, path)? else {
        return Ok(Proof::missing());
    };
    if uuid::Uuid::parse_str(token).is_ok()
        && let Some(capture) = open_capture(&dir, token)?
    {
        let captured = proof_at(&capture, &cstring("resource")?, path)?;
        if captured.presence == Presence::Present {
            return Ok(captured);
        }
    }
    proof_at(&dir, &name, path)
}

pub(crate) fn prepare(w: &Workspace, resource_id: &str, path: &str, token: &str) -> Result<Proof> {
    let (dir, name) = parent(&w.path, path, true)?;
    // SAFETY: creates one checked leaf; an existing entry is never claimed here.
    if unsafe { libc::mkdirat(dir.as_raw_fd(), name.as_ptr(), 0o755) } < 0 {
        let e = std::io::Error::last_os_error();
        if e.kind() == std::io::ErrorKind::AlreadyExists {
            return Err(Error::new(
                "ownership",
                format!("{path} already exists; adopt it explicitly if it is disposable"),
            ));
        }
        return Err(e.into());
    }
    dir.sync_all()?;
    let directory = open_directory(&dir, &name, path)?
        .ok_or_else(|| Error::new("ownership", format!("{path} disappeared during creation")))?;
    write_marker(
        &directory,
        &Marker {
            version: 1,
            workspace_id: w.id.clone(),
            resource_id: resource_id.into(),
            owner_token: token.into(),
            instance: uuid::Uuid::new_v4().to_string(),
        },
    )?;
    proof(&w.path, path, token)
}

/// Adoption claims only ignored, untracked output.
pub(crate) fn adoptable(w: &Workspace, path: &str) -> Result<()> {
    if crate::git::tracked(&w.path, path)? {
        return Err(Error::new(
            "ownership",
            format!("{path} contains tracked files"),
        ));
    }
    if !crate::git::ignored(&w.path, path)? {
        return Err(Error::new(
            "ownership",
            format!("{path} is not ignored by Git; only ignored output can be adopted"),
        ));
    }
    Ok(())
}

/// Marks the exact directory observed in the reviewed plan.
pub(crate) fn adopt(
    w: &Workspace,
    resource_id: &str,
    path: &str,
    token: &str,
    observed: Option<&Proof>,
) -> Result<Proof> {
    adoptable(w, path)?;
    let (dir, name) = parent(&w.path, path, false)?;
    let directory = open_directory(&dir, &name, path)?
        .ok_or_else(|| Error::new("ownership", format!("{path} disappeared since review")))?;
    let meta = directory.metadata()?;
    if observed.is_none_or(|p| {
        p.instance.is_some()
            || p.handle.get("device") != Some(&meta.dev().into())
            || p.handle.get("inode") != Some(&meta.ino().into())
    }) {
        return Err(Error::new(
            "ownership",
            format!("{path} changed since the adopt plan was reviewed"),
        ));
    }
    write_marker(
        &directory,
        &Marker {
            version: 1,
            workspace_id: w.id.clone(),
            resource_id: resource_id.into(),
            owner_token: token.into(),
            instance: uuid::Uuid::new_v4().to_string(),
        },
    )?;
    proof(&w.path, path, token)
}

#[cfg(target_os = "macos")]
fn rename_noreplace(from: &File, old: &CString, to: &File, new: &CString) -> std::io::Result<()> {
    // SAFETY: anchored descriptors; RENAME_EXCL refuses an existing destination.
    let result = unsafe {
        libc::renameatx_np(
            from.as_raw_fd(),
            old.as_ptr(),
            to.as_raw_fd(),
            new.as_ptr(),
            libc::RENAME_EXCL,
        )
    };
    if result < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn rename_noreplace(from: &File, old: &CString, to: &File, new: &CString) -> std::io::Result<()> {
    // SAFETY: anchored descriptors; RENAME_NOREPLACE refuses an existing destination.
    let result = unsafe {
        libc::renameat2(
            from.as_raw_fd(),
            old.as_ptr(),
            to.as_raw_fd(),
            new.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    if result < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

/// Captures the directory into private storage, verifies the captured marker
/// against the receipt, then removes only that tree. A mismatch is restored
/// without overwriting anything that appeared at the path meanwhile.
pub(crate) fn destroy(
    root: &Path,
    path: &str,
    token: &str,
    receipt: Option<&Proof>,
) -> Result<Proof> {
    uuid::Uuid::parse_str(token)
        .map_err(|_| Error::new("ownership", "invalid allocation token"))?;
    if crate::git::tracked(root, path)? {
        return Err(Error::new(
            "ownership",
            format!("{path} contains tracked files; refusing to remove it"),
        ));
    }
    let (dir, name) = parent(root, path, false)?;
    let quarantine = cstring(&format!(".treetender-delete-{token}"))?;
    // SAFETY: creation occurs in an anchored directory; existing captures resume below.
    if unsafe { libc::mkdirat(dir.as_raw_fd(), quarantine.as_ptr(), 0o700) } < 0
        && std::io::Error::last_os_error().kind() != std::io::ErrorKind::AlreadyExists
    {
        return Err(std::io::Error::last_os_error().into());
    }
    let capture = open_capture(&dir, token)?
        .ok_or_else(|| Error::new("unsafe_path", "cleanup capture disappeared"))?;
    let entry = cstring("resource")?;
    if proof_at(&capture, &entry, path)?.presence == Presence::Missing {
        // SAFETY: atomic rename captures the directory itself, never a link target.
        if unsafe {
            libc::renameat(
                dir.as_raw_fd(),
                name.as_ptr(),
                capture.as_raw_fd(),
                entry.as_ptr(),
            )
        } < 0
        {
            return Err(std::io::Error::last_os_error().into());
        }
        dir.sync_all()?;
        capture.sync_all()?;
    }
    let captured = proof_at(&capture, &entry, path)?;
    if Some(&captured) != receipt {
        let restored = rename_noreplace(&capture, &entry, &dir, &name).is_ok();
        if restored {
            // SAFETY: removes only our now-empty private directory.
            unsafe {
                libc::unlinkat(dir.as_raw_fd(), quarantine.as_ptr(), libc::AT_REMOVEDIR);
            }
            dir.sync_all()?;
        }
        let preserved = if restored {
            root.join(path)
        } else {
            root.join(path)
                .parent()
                .unwrap()
                .join(quarantine.to_string_lossy().as_ref())
                .join("resource")
        };
        return Err(Error::new(
            "ownership",
            format!(
                "captured directory differs from receipt; preserved {}",
                preserved.display()
            ),
        ));
    }
    remove_tree(&capture, &entry, None, 0)?;
    capture.sync_all()?;
    // SAFETY: removes only our now-empty private directory.
    if unsafe { libc::unlinkat(dir.as_raw_fd(), quarantine.as_ptr(), libc::AT_REMOVEDIR) } < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    dir.sync_all()?;
    proof(root, path, token)
}

fn is_directory(dir: &File, name: &CString) -> Result<bool> {
    let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
    // SAFETY: stat points at writable storage; AT_SYMLINK_NOFOLLOW observes the leaf.
    if unsafe {
        libc::fstatat(
            dir.as_raw_fd(),
            name.as_ptr(),
            stat.as_mut_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    } < 0
    {
        return Err(std::io::Error::last_os_error().into());
    }
    // SAFETY: successful fstatat initialized stat.
    Ok(unsafe { stat.assume_init() }.st_mode & libc::S_IFMT == libc::S_IFDIR)
}

/// Descriptor-relative removal that never follows symlinks, never crosses into
/// another filesystem and stops at nested Git checkouts. The marker goes last
/// so an interrupted removal still proves the same instance.
fn remove_tree(parent: &File, name: &CString, device: Option<u64>, depth: usize) -> Result<()> {
    if depth > MAX_DEPTH {
        return Err(Error::new(
            "input_limit",
            "directory tree is too deep to remove safely",
        ));
    }
    let directory = open_directory(parent, name, "owned directory")?
        .ok_or_else(|| Error::new("unsafe_path", "directory disappeared during removal"))?;
    let meta = directory.metadata()?;
    let device = device.unwrap_or(meta.dev());
    if meta.dev() != device {
        return Err(Error::new(
            "unsafe_path",
            "refusing to remove across a filesystem boundary",
        ));
    }
    let names = dir_names(&directory)?;
    if names.iter().any(|n| n == b".git") {
        return Err(Error::new(
            "unsafe_path",
            "refusing to remove a nested Git checkout",
        ));
    }
    for child in names {
        if depth == 0 && child == MARKER.as_bytes() {
            continue;
        }
        let child =
            CString::new(child).map_err(|_| Error::new("unsafe_path", "invalid filename"))?;
        if is_directory(&directory, &child)? {
            remove_tree(&directory, &child, Some(device), depth + 1)?;
        // SAFETY: removes one non-directory entry, including a symlink itself.
        } else if unsafe { libc::unlinkat(directory.as_raw_fd(), child.as_ptr(), 0) } < 0 {
            return Err(std::io::Error::last_os_error().into());
        }
    }
    if depth == 0 {
        let marker = cstring(MARKER)?;
        // SAFETY: removes the verified marker of the captured directory.
        if unsafe { libc::unlinkat(directory.as_raw_fd(), marker.as_ptr(), 0) } < 0
            && std::io::Error::last_os_error().kind() != std::io::ErrorKind::NotFound
        {
            return Err(std::io::Error::last_os_error().into());
        }
    }
    drop(directory);
    // SAFETY: removes the now-empty directory entry under its anchored parent.
    if unsafe { libc::unlinkat(parent.as_raw_fd(), name.as_ptr(), libc::AT_REMOVEDIR) } < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(())
}

/// Allocated bytes below the directory, for status reporting only. Volatile, so
/// it never enters proofs, plans or fingerprints.
pub(crate) fn usage(root: &Path, path: &str) -> Result<Option<u64>> {
    let Some((dir, name)) = anchored_parent(root, path)? else {
        return Ok(None);
    };
    let Some(directory) = open_directory(&dir, &name, path)? else {
        return Ok(None);
    };
    let device = directory.metadata()?.dev();
    tree_bytes(&directory, device, 0).map(Some)
}

fn tree_bytes(directory: &File, device: u64, depth: usize) -> Result<u64> {
    let mut total = directory.metadata()?.blocks() * 512;
    if depth > MAX_DEPTH {
        return Ok(total);
    }
    for child in dir_names(directory)? {
        let child =
            CString::new(child).map_err(|_| Error::new("unsafe_path", "invalid filename"))?;
        if is_directory(directory, &child)? {
            if let Some(sub) = open_directory(directory, &child, "owned directory")?
                && sub.metadata()?.dev() == device
            {
                total += tree_bytes(&sub, device, depth + 1)?;
            }
            continue;
        }
        let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
        // SAFETY: stat points at writable storage; the leaf is never followed.
        if unsafe {
            libc::fstatat(
                directory.as_raw_fd(),
                child.as_ptr(),
                stat.as_mut_ptr(),
                libc::AT_SYMLINK_NOFOLLOW,
            )
        } == 0
        {
            // SAFETY: successful fstatat initialized stat.
            total += unsafe { stat.assume_init() }.st_blocks as u64 * 512;
        }
    }
    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workspace(root: &Path) -> Workspace {
        Workspace {
            id: uuid::Uuid::new_v4().to_string(),
            repository_id: uuid::Uuid::new_v4().to_string(),
            path: root.to_path_buf(),
            git_dir: root.join(".git"),
            common_dir: root.join(".git"),
            primary: false,
            name: "lane".into(),
            ports: Default::default(),
        }
    }

    fn checkout() -> (tempfile::TempDir, std::path::PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let mut git = std::process::Command::new("git");
        git.arg("-C").arg(&root).args(["init", "-q"]);
        crate::git::clean_environment(&mut git);
        assert!(git.status().unwrap().success());
        (temp, root)
    }

    #[test]
    fn interrupted_removal_resumes_from_the_capture() {
        let (_temp, root) = checkout();
        let w = workspace(&root);
        let token = uuid::Uuid::new_v4().to_string();
        let receipt = prepare(&w, "out", "out", &token).unwrap();
        std::fs::create_dir_all(root.join("out/deep/er")).unwrap();
        std::fs::write(root.join("out/deep/er/artifact"), "bytes").unwrap();
        let quarantine = root.join(format!(".treetender-delete-{token}"));
        std::fs::create_dir(&quarantine).unwrap();
        std::fs::rename(root.join("out"), quarantine.join("resource")).unwrap();
        std::fs::remove_file(quarantine.join("resource/deep/er/artifact")).unwrap();
        assert_eq!(proof(&root, "out", &token).unwrap(), receipt);
        let outcome = destroy(&root, "out", &token, Some(&receipt)).unwrap();
        assert_eq!(outcome.presence, Presence::Missing);
        assert!(!quarantine.exists());
    }

    #[test]
    fn mismatched_capture_is_restored_without_overwriting() {
        let (_temp, root) = checkout();
        let w = workspace(&root);
        let token = uuid::Uuid::new_v4().to_string();
        let receipt = prepare(&w, "out", "out", &token).unwrap();
        std::fs::rename(root.join("out"), root.join("original")).unwrap();
        std::fs::create_dir(root.join("out")).unwrap();
        std::fs::write(root.join("out/foreign"), "keep").unwrap();
        assert_eq!(
            destroy(&root, "out", &token, Some(&receipt))
                .unwrap_err()
                .code,
            "ownership"
        );
        assert_eq!(
            std::fs::read_to_string(root.join("out/foreign")).unwrap(),
            "keep"
        );
        assert!(root.join("original").join(MARKER).exists());
        assert!(!root.join(format!(".treetender-delete-{token}")).exists());
    }

    #[test]
    fn removal_never_follows_symlinks_or_enters_nested_checkouts() {
        let (_temp, root) = checkout();
        let w = workspace(&root);
        std::fs::create_dir(root.join("outside")).unwrap();
        std::fs::write(root.join("outside/keep"), "keep").unwrap();
        let token = uuid::Uuid::new_v4().to_string();
        let receipt = prepare(&w, "out", "out", &token).unwrap();
        std::os::unix::fs::symlink(root.join("outside"), root.join("out/link")).unwrap();
        assert_eq!(
            destroy(&root, "out", &token, Some(&receipt))
                .unwrap()
                .presence,
            Presence::Missing
        );
        assert_eq!(
            std::fs::read_to_string(root.join("outside/keep")).unwrap(),
            "keep"
        );
        let token = uuid::Uuid::new_v4().to_string();
        let receipt = prepare(&w, "out", "out", &token).unwrap();
        std::fs::create_dir_all(root.join("out/vendor/.git")).unwrap();
        assert_eq!(
            destroy(&root, "out", &token, Some(&receipt))
                .unwrap_err()
                .code,
            "unsafe_path"
        );
        assert!(
            root.join(format!(".treetender-delete-{token}/resource/vendor/.git"))
                .exists()
        );
    }
}
