//! Trusted external command boundary and descriptor-relative file operations.
use crate::manifest::{Adapter, FileMode, Resource, SourceRoot};
use crate::model::*;
use sha2::{Digest, Sha256};
use std::ffi::CString;
use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::fs::MetadataExt;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub(crate) fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

pub(crate) fn cstring(s: &str) -> Result<CString> {
    CString::new(s).map_err(|_| Error::new("unsafe_path", "path contains NUL"))
}
pub(crate) fn fd_file(fd: i32) -> Result<File> {
    if fd < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    // SAFETY: a successful open/openat returns a fresh descriptor owned here.
    Ok(unsafe { File::from_raw_fd(fd) })
}
fn root_fd(root: &Path) -> Result<File> {
    use std::os::unix::ffi::OsStrExt;
    if !root.is_absolute() {
        return Err(Error::new("unsafe_path", "root must be absolute"));
    }
    // SAFETY: the literal is a valid C string; open returns a new descriptor.
    let mut dir = fd_file(unsafe {
        libc::open(
            c"/".as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    })?;
    for component in root.components().skip(1) {
        if !matches!(component, std::path::Component::Normal(_)) {
            return Err(Error::new("unsafe_path", "non-normal root component"));
        }
        let name = CString::new(component.as_os_str().as_bytes())
            .map_err(|_| Error::new("unsafe_path", "NUL in path"))?;
        // SAFETY: dir is anchored, and no intermediate symlink may be followed.
        dir = fd_file(unsafe {
            libc::openat(
                dir.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        })?;
    }
    Ok(dir)
}
pub(crate) fn parent(root: &Path, relative: &str, create: bool) -> Result<(File, CString)> {
    crate::manifest::relative(relative)?;
    let mut dir = root_fd(root)?;
    let mut parts = relative.split('/').peekable();
    while let Some(part) = parts.next() {
        let name = cstring(part)?;
        if parts.peek().is_none() {
            return Ok((dir, name));
        }
        // SAFETY: dir and name remain valid for the syscall.
        let mut fd = unsafe {
            libc::openat(
                dir.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if fd < 0
            && create
            && std::io::Error::last_os_error().kind() == std::io::ErrorKind::NotFound
        {
            // SAFETY: this creates only one checked path component under dir.
            let made = unsafe { libc::mkdirat(dir.as_raw_fd(), name.as_ptr(), 0o700) };
            if made < 0
                && std::io::Error::last_os_error().kind() != std::io::ErrorKind::AlreadyExists
            {
                return Err(std::io::Error::last_os_error().into());
            }
            dir.sync_all()?;
            // SAFETY: opening without following links also handles concurrent creators.
            fd = unsafe {
                libc::openat(
                    dir.as_raw_fd(),
                    name.as_ptr(),
                    libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                )
            };
        }
        dir = fd_file(fd)
            .map_err(|e| Error::new("unsafe_path", format!("unsafe parent of {relative}: {e}")))?;
    }
    Err(Error::new("unsafe_path", "empty path"))
}
fn read_regular(root: &Path, path: &str) -> Result<File> {
    let (dir, name) = parent(root, path, false)?;
    // SAFETY: name and dir are valid; leaf symlinks are explicitly refused.
    let f = fd_file(unsafe {
        libc::openat(
            dir.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
        )
    })?;
    if !f.metadata()?.is_file() {
        return Err(Error::new(
            "unsafe_path",
            format!("{path} is not a regular file"),
        ));
    }
    Ok(f)
}
pub(crate) fn file_snapshot(root: &Path, path: &str) -> Result<String> {
    let mut file = read_regular(root, path)?;
    let meta = file.metadata()?;
    let mut hash = Sha256::new();
    std::io::copy(&mut file, &mut hash)?;
    Ok(format!(
        "{}:{}:{:x}",
        meta.dev(),
        meta.ino(),
        hash.finalize()
    ))
}
/// Deterministic file/tree fingerprint, with descriptor-relative traversal and
/// no symlinks, special files or unbounded recursion.
pub(crate) fn tree_snapshot(root: &Path, path: &str) -> Result<String> {
    let (dir, name) = parent(root, path, false)?;
    // SAFETY: openat anchors traversal and refuses symlink leaves.
    let file = fd_file(unsafe {
        libc::openat(
            dir.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
        )
    })?;
    let mut count = 0;
    snapshot_open(file, 0, &mut count)
}
/// Sorted entry names of an open directory, excluding `.` and `..`.
pub(crate) fn dir_names(directory: &File) -> Result<Vec<Vec<u8>>> {
    // SAFETY: dup gives fdopendir a descriptor it may own/close.
    let fd = unsafe { libc::dup(directory.as_raw_fd()) };
    if fd < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    // SAFETY: fd is an owned directory descriptor. On success closedir owns it.
    let stream = unsafe { libc::fdopendir(fd) };
    if stream.is_null() {
        // SAFETY: failed fdopendir did not consume fd.
        unsafe {
            libc::close(fd);
        }
        return Err(std::io::Error::last_os_error().into());
    }
    // SAFETY: the duplicate shares its offset with the original descriptor.
    unsafe { libc::rewinddir(stream) };
    let mut names = vec![];
    loop {
        // SAFETY: stream remains open and this thread owns it.
        let entry = unsafe { libc::readdir(stream) };
        if entry.is_null() {
            break;
        }
        // SAFETY: d_name is a NUL-terminated name valid until next readdir.
        let name = unsafe { std::ffi::CStr::from_ptr((*entry).d_name.as_ptr()) }
            .to_bytes()
            .to_vec();
        if name != b"." && name != b".." {
            names.push(name);
        }
    }
    // SAFETY: closes precisely the stream allocated above.
    unsafe {
        libc::closedir(stream);
    }
    names.sort();
    Ok(names)
}
fn snapshot_open(mut file: File, depth: usize, count: &mut usize) -> Result<String> {
    use std::os::unix::ffi::OsStrExt;
    *count += 1;
    if depth > 128 || *count > 100_000 {
        return Err(Error::new("input_limit", "input tree too large/deep"));
    }
    let meta = file.metadata()?;
    if meta.is_file() {
        let mut hash = Sha256::new();
        std::io::copy(&mut file, &mut hash)?;
        return Ok(format!("file:{:x}", hash.finalize()));
    }
    if !meta.is_dir() {
        return Err(Error::new("unsafe_path", "input contains a special file"));
    }
    let names = dir_names(&file)?;
    let mut entries = vec![];
    for name in names {
        let c = CString::new(name.clone())
            .map_err(|_| Error::new("unsafe_path", "invalid filename"))?;
        // SAFETY: file anchors the directory and children cannot follow symlinks.
        let child = fd_file(unsafe {
            libc::openat(
                file.as_raw_fd(),
                c.as_ptr(),
                libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
            )
        })?;
        entries.push((
            std::ffi::OsStr::from_bytes(&name).as_bytes().to_vec(),
            snapshot_open(child, depth + 1, count)?,
        ));
    }
    Ok(format!("tree:{}", digest(&serde_json::to_vec(&entries)?)))
}
/// The anchored parent of a relative path, or None when a parent is missing.
/// A missing parent proves absence; a symlinked parent never does.
pub(crate) fn anchored_parent(root: &Path, relative: &str) -> Result<Option<(File, CString)>> {
    match parent(root, relative, false) {
        Ok(p) => Ok(Some(p)),
        Err(e) if e.code == "unsafe_path" && !root.join(relative).exists() => {
            let mut p = root.to_path_buf();
            for part in relative.split('/').take(relative.split('/').count() - 1) {
                p.push(part);
                match std::fs::symlink_metadata(&p) {
                    Ok(m) if m.is_dir() && !m.file_type().is_symlink() => (),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
                    _ => return Err(e),
                }
            }
            Err(e)
        }
        Err(e) => Err(e),
    }
}
fn file_proof(root: &Path, destination: &str, mode: FileMode, token: &str) -> Result<Proof> {
    let Some((dir, name)) = anchored_parent(root, destination)? else {
        return Ok(Proof::missing());
    };
    if uuid::Uuid::parse_str(token).is_ok() {
        let quarantine = cstring(&format!(".treetender-delete-{token}"))?;
        // SAFETY: existing private captures are read without following symlinks.
        let fd = unsafe {
            libc::openat(
                dir.as_raw_fd(),
                quarantine.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if fd >= 0 {
            let capture = fd_file(fd)?;
            let proof = proof_at(&capture, &cstring("resource")?, destination, mode, token)?;
            if proof.presence == Presence::Present {
                return Ok(proof);
            }
        } else if std::io::Error::last_os_error().kind() != std::io::ErrorKind::NotFound {
            return Err(std::io::Error::last_os_error().into());
        }
    }
    proof_at(&dir, &name, destination, mode, token)
}
// libc device/inode field widths differ between macOS and Linux.
#[allow(clippy::unnecessary_cast)]
fn proof_at(
    dir: &File,
    name: &CString,
    destination: &str,
    mode: FileMode,
    token: &str,
) -> Result<Proof> {
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
        let e = std::io::Error::last_os_error();
        if e.kind() == std::io::ErrorKind::NotFound {
            return Ok(Proof::missing());
        }
        return Err(e.into());
    }
    // SAFETY: successful fstatat initialized stat.
    let stat = unsafe { stat.assume_init() };
    let content = match mode {
        FileMode::Copy => {
            // SAFETY: anchored leaf is opened without following symlinks.
            let mut file = fd_file(unsafe {
                libc::openat(
                    dir.as_raw_fd(),
                    name.as_ptr(),
                    libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
                )
            })?;
            let meta = file.metadata()?;
            if !meta.is_file()
                || meta.dev() != stat.st_dev as u64
                || meta.ino() != stat.st_ino as u64
            {
                return Err(Error::new("ownership", "file changed during inspection"));
            }
            let mut hash = Sha256::new();
            std::io::copy(&mut file, &mut hash)?;
            format!("{}:{}:{:x}", meta.dev(), meta.ino(), hash.finalize())
        }
        FileMode::Link => {
            if stat.st_mode & libc::S_IFMT != libc::S_IFLNK {
                return Err(Error::new("ownership", "expected owned symlink"));
            }
            let mut buf = vec![0u8; 16384];
            // SAFETY: buffer is valid and length bounded; readlinkat never follows the leaf.
            let n = unsafe {
                libc::readlinkat(
                    dir.as_raw_fd(),
                    name.as_ptr(),
                    buf.as_mut_ptr().cast(),
                    buf.len(),
                )
            };
            if n < 0 {
                return Err(std::io::Error::last_os_error().into());
            }
            if n as usize == buf.len() {
                return Err(Error::new("unsafe_path", "link target too long"));
            }
            digest(&buf[..n as usize])
        }
    };
    Ok(Proof {
        presence: Presence::Present,
        instance: Some(format!("{}:{}:{content}", stat.st_dev, stat.st_ino)),
        owner_token: Some(token.into()),
        handle: serde_json::json!({"destination":destination, "mode":mode}),
    })
}

pub(crate) fn target(w: &Workspace, resource: &Resource) -> String {
    match resource.adapter.as_ref() {
        Some(Adapter::File { destination, .. }) => format!("file:{}:{destination}", w.id),
        Some(Adapter::Directory { path }) => format!("dir:{}:{path}", w.id),
        _ => format!("external:{}", resource.target),
    }
}

fn quarantine_delete(
    root: &Path,
    destination: &str,
    mode: FileMode,
    token: &str,
    receipt: Option<&Proof>,
) -> Result<Proof> {
    uuid::Uuid::parse_str(token)
        .map_err(|_| Error::new("ownership", "invalid allocation token"))?;
    let (dir, name) = parent(root, destination, false)?;
    // Deterministic private capture path is recoverable from the durable intent.
    let quarantine = cstring(&format!(".treetender-delete-{token}"))?;
    // SAFETY: creation occurs in an anchored directory; existing captures resume below.
    if unsafe { libc::mkdirat(dir.as_raw_fd(), quarantine.as_ptr(), 0o700) } < 0
        && std::io::Error::last_os_error().kind() != std::io::ErrorKind::AlreadyExists
    {
        return Err(std::io::Error::last_os_error().into());
    }
    // SAFETY: the quarantine cannot be a symlink.
    let capture = fd_file(unsafe {
        libc::openat(
            dir.as_raw_fd(),
            quarantine.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    })?;
    let entry = cstring("resource")?;
    let existing = proof_at(&capture, &entry, destination, mode, token);
    if existing
        .as_ref()
        .is_ok_and(|p| p.presence == Presence::Missing)
    {
        // SAFETY: atomic rename captures the leaf (including a raced replacement),
        // never its target. Proof is checked only after capture in private storage.
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
    let proof = proof_at(&capture, &entry, destination, mode, token);
    if proof.as_ref().ok() != receipt {
        // linkat has no overwrite behavior; on a concurrently created destination
        // leave the captured replacement in quarantine and report its exact path.
        // SAFETY: flags 0 hardlinks the leaf itself, including a symlink leaf.
        let restored = unsafe {
            libc::linkat(
                capture.as_raw_fd(),
                entry.as_ptr(),
                dir.as_raw_fd(),
                name.as_ptr(),
                0,
            )
        } == 0;
        if restored {
            // SAFETY: original is restored and this removes only the extra link.
            unsafe {
                libc::unlinkat(capture.as_raw_fd(), entry.as_ptr(), 0);
                libc::unlinkat(dir.as_raw_fd(), quarantine.as_ptr(), libc::AT_REMOVEDIR);
            }
            dir.sync_all()?;
        }
        return Err(Error::new(
            "ownership",
            format!(
                "captured file differs from receipt; preserved {}",
                if restored {
                    root.join(destination)
                } else {
                    root.join(destination)
                        .parent()
                        .unwrap()
                        .join(quarantine.to_string_lossy().as_ref())
                        .join("resource")
                }
                .display()
            ),
        ));
    }
    // SAFETY: only the captured, verified instance is removed. A replacement now
    // at destination remains untouched and is detected by post-effect inspection.
    if unsafe { libc::unlinkat(capture.as_raw_fd(), entry.as_ptr(), 0) } < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    capture.sync_all()?;
    // SAFETY: this removes only our now-empty private directory.
    if unsafe { libc::unlinkat(dir.as_raw_fd(), quarantine.as_ptr(), libc::AT_REMOVEDIR) } < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    dir.sync_all()?;
    file_proof(root, destination, mode, token)
}

pub(crate) fn supports(resource: &Resource, action: &str) -> bool {
    match resource.adapter.as_ref() {
        None => false,
        Some(Adapter::File { .. }) => ["inspect", "prepare", "destroy"].contains(&action),
        Some(Adapter::Script { capabilities, .. }) => capabilities.iter().any(|c| c == action),
        Some(Adapter::Portman { .. }) => action == "inspect",
        Some(Adapter::Directory { .. }) => {
            ["inspect", "prepare", "destroy", "adopt"].contains(&action)
        }
    }
}

/// Read-only checks an adapter requires before existing state may be adopted.
pub(crate) fn adoptable(w: &Workspace, resource: &Resource) -> Result<()> {
    match resource.adapter.as_ref() {
        Some(Adapter::Directory { path }) => crate::directory::adoptable(w, path),
        _ => Err(Error::new(
            "capability",
            "adapter cannot adopt existing state",
        )),
    }
}

pub(crate) fn inspect(
    w: &Workspace,
    resource: &Resource,
    token: &str,
    receipt: Option<&Proof>,
    recovery_dir: &Path,
) -> Result<Proof> {
    match resource.adapter.as_ref() {
        None => Err(Error::new(
            "passive_resource",
            "passive resource has no live observation; configure an observer to verify a prerequisite",
        )),
        Some(Adapter::File {
            destination, mode, ..
        }) => {
            if !crate::git::is_current(w)? {
                return Ok(Proof::missing());
            }
            crate::manifest::writable_destination(destination)?;
            let mut proof = file_proof(&w.path, destination, *mode, token)?;
            if proof.presence == Presence::Present
                && receipt.is_none_or(|old| old.instance != proof.instance)
            {
                proof.owner_token = None;
            }
            Ok(proof)
        }
        Some(Adapter::Directory { path }) => {
            if !crate::git::is_current(w)? {
                return Ok(Proof::missing());
            }
            crate::manifest::writable_destination(path)?;
            crate::directory::proof(&w.path, path, token)
        }
        _ => invoke(w, resource, "inspect", token, receipt, recovery_dir),
    }
}
pub(crate) fn invoke(
    w: &Workspace,
    resource: &Resource,
    action: &str,
    token: &str,
    receipt: Option<&Proof>,
    recovery_dir: &Path,
) -> Result<Proof> {
    if !supports(resource, action) {
        return Err(Error::new(
            "capability",
            format!("{} lacks {action}", resource.id),
        ));
    }
    match resource.adapter.as_ref() {
        None => Err(Error::new(
            "passive_resource",
            "passive resource has no live observation; configure an observer to verify a prerequisite",
        )),
        Some(Adapter::File {
            source,
            destination,
            mode,
            source_root,
        }) => {
            crate::manifest::writable_destination(destination)?;
            if action != "inspect" && !crate::git::is_current(w)? {
                return Err(Error::new(
                    "checkout_unavailable",
                    "file effects require the original current checkout",
                ));
            }
            match action {
                "inspect" => return inspect(w, resource, token, receipt, recovery_dir),
                "prepare" => {
                    let source_path = match source_root {
                        SourceRoot::Workspace => w.path.clone(),
                        SourceRoot::Primary => crate::git::discover(&w.path)?.primary_path,
                    };
                    let mut source_file = read_regular(&source_path, source)?;
                    let (dir, name) = parent(&w.path, destination, true)?;
                    match mode {
                        FileMode::Copy => {
                            // SAFETY: O_EXCL refuses existing files or symlinks atomically.
                            let mut out = fd_file(unsafe {
                                libc::openat(
                                    dir.as_raw_fd(),
                                    name.as_ptr(),
                                    libc::O_WRONLY
                                        | libc::O_CREAT
                                        | libc::O_EXCL
                                        | libc::O_NOFOLLOW
                                        | libc::O_CLOEXEC,
                                    0o600,
                                )
                            })?;
                            std::io::copy(&mut source_file, &mut out)?;
                            out.sync_all()?;
                        }
                        FileMode::Link => {
                            let link = cstring(&source_path.join(source).to_string_lossy())?;
                            // SAFETY: both strings and the anchored parent descriptor are valid.
                            if unsafe {
                                libc::symlinkat(link.as_ptr(), dir.as_raw_fd(), name.as_ptr())
                            } < 0
                            {
                                return Err(std::io::Error::last_os_error().into());
                            }
                        }
                    }
                    dir.sync_all()?;
                }
                "destroy" => {
                    return quarantine_delete(&w.path, destination, *mode, token, receipt);
                }
                _ => return Err(Error::new("capability", "file action unsupported")),
            }
            file_proof(&w.path, destination, *mode, token)
        }
        Some(Adapter::Script {
            command,
            timeout_secs,
            ..
        }) => {
            let inspecting = action == "inspect";
            let input = AdapterRequest {
                version: 1,
                action: action.into(),
                workspace: w.clone(),
                resource_id: resource.id.clone(),
                target: resource.target.clone(),
                owner_token: if inspecting {
                    String::new()
                } else {
                    token.into()
                },
                receipt: receipt.cloned().map(|mut proof| {
                    if inspecting {
                        proof.instance = None;
                        proof.owner_token = None;
                    }
                    proof
                }),
            };
            let checkout_current = crate::git::is_current(w)?;
            let recovery_argv = if checkout_current {
                None
            } else {
                Some(recovery_command(w, command)?)
            };
            let bytes = run(
                recovery_argv.as_deref().unwrap_or(command),
                if checkout_current {
                    &w.path
                } else {
                    recovery_dir
                },
                Some(serde_json::to_vec(&input)?),
                *timeout_secs,
            )?;
            let response: AdapterResponse = serde_json::from_slice(&bytes)?;
            if response.version != 1 {
                return Err(Error::new(
                    "adapter_version",
                    "expected adapter response version 1",
                ));
            }
            if response.proof.presence == Presence::Present
                && response
                    .proof
                    .instance
                    .as_ref()
                    .is_none_or(|s| s.is_empty())
            {
                return Err(Error::new(
                    "adapter_proof",
                    "present requires exact nonempty instance",
                ));
            }
            Ok(response.proof)
        }
        Some(Adapter::Portman {
            service, socket, ..
        }) => portman_status(socket, service),
        Some(Adapter::Directory { path }) => {
            crate::manifest::writable_destination(path)?;
            if action != "inspect" && !crate::git::is_current(w)? {
                return Err(Error::new(
                    "checkout_unavailable",
                    "directory effects require the original current checkout",
                ));
            }
            match action {
                "inspect" => crate::directory::proof(&w.path, path, token),
                "prepare" => crate::directory::prepare(w, &resource.id, path, token),
                "adopt" => crate::directory::adopt(w, &resource.id, path, token, receipt),
                "destroy" => crate::directory::destroy(&w.path, path, token, receipt),
                _ => Err(Error::new("capability", "directory action unsupported")),
            }
        }
    }
}

/// Receipt-only recovery cannot resolve executable or file arguments from a
/// replacement checkout. Bare non-option arguments are ambiguous relative paths;
/// adapters needing literals can use option flags or an absolute wrapper script.
pub(crate) fn recovery_command(w: &Workspace, argv: &[String]) -> Result<Vec<String>> {
    use std::os::unix::fs::PermissionsExt;
    let program = argv
        .first()
        .ok_or_else(|| Error::new("adapter_unavailable", "empty adapter command"))?;
    let mut result = argv.to_vec();
    let executable = if Path::new(program).is_absolute() {
        Path::new(program).to_path_buf()
    } else if program.contains('/') {
        return Err(Error::new(
            "checkout_unavailable",
            "receipt-only cleanup cannot run a repository-relative executable",
        ));
    } else {
        let mut found = None;
        for dir in std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()) {
            if !dir.is_absolute() {
                return Err(Error::new(
                    "checkout_unavailable",
                    "receipt-only cleanup requires absolute PATH entries",
                ));
            }
            let candidate = dir.join(program);
            if std::fs::metadata(&candidate)
                .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
            {
                found = Some(candidate);
                break;
            }
        }
        found.ok_or_else(|| {
            Error::new(
                "adapter_unavailable",
                format!("executable not found: {program}"),
            )
        })?
    };
    let paths = std::iter::once(executable.as_path()).chain(
        argv.iter()
            .skip(1)
            .filter(|arg| !arg.starts_with('-'))
            .map(Path::new),
    );
    for path in paths {
        if !path.is_absolute() {
            return Err(Error::new(
                "checkout_unavailable",
                "receipt-only cleanup requires absolute file arguments; repository-relative/ambiguous arguments are blocked",
            ));
        }
        if path.starts_with(&w.path) || path.canonicalize().is_ok_and(|p| p.starts_with(&w.path)) {
            return Err(Error::new(
                "checkout_unavailable",
                "adapter command references the absent/replacement checkout",
            ));
        }
    }
    result[0] = executable.to_string_lossy().into_owned();
    Ok(result)
}

/// Execute argv as trusted code. Output is bounded and process groups time out.
pub(crate) fn run(
    argv: &[String],
    cwd: &Path,
    input: Option<Vec<u8>>,
    seconds: u64,
) -> Result<Vec<u8>> {
    const LIMIT: u64 = 1024 * 1024;
    let mut cmd = Command::new(&argv[0]);
    cmd.args(&argv[1..])
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0);
    crate::git::clean_environment(&mut cmd);
    let mut child = cmd
        .spawn()
        .map_err(|e| Error::new("adapter_unavailable", e.to_string()))?;
    let pid = child.id();
    let stdin = child.stdin.take().expect("piped stdin");
    std::thread::spawn(move || {
        let mut stdin = stdin;
        if let Some(input) = input {
            let _ = stdin.write_all(&input);
        }
    });
    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");
    let (tx, rx) = std::sync::mpsc::channel();
    for (index, stream) in [
        (0, Box::new(stdout) as Box<dyn Read + Send>),
        (1, Box::new(stderr) as Box<dyn Read + Send>),
    ] {
        let tx = tx.clone();
        std::thread::spawn(move || {
            let mut bytes = vec![];
            let result = stream.take(LIMIT + 1).read_to_end(&mut bytes);
            let _ = tx.send((index, result, bytes));
        });
    }
    drop(tx);
    let deadline = Instant::now() + Duration::from_secs(seconds);
    let mut output = [None, None];
    let mut status = None;
    loop {
        while let Ok((i, result, bytes)) = rx.try_recv() {
            result?;
            if bytes.len() as u64 > LIMIT {
                // SAFETY: negative pid addresses only the group created for this child.
                unsafe {
                    libc::kill(-(pid as i32), libc::SIGKILL);
                }
                let _ = child.wait();
                return Err(Error::new(
                    "adapter_output",
                    "command exceeded 1 MiB output limit",
                ));
            }
            output[i] = Some(bytes);
        }
        if status.is_none() {
            status = child.try_wait()?;
        }
        if let Some(status) = status
            && output.iter().all(Option::is_some)
        {
            let stdout = output[0].take().unwrap();
            let stderr = output[1].take().unwrap();
            if !status.success() {
                return Err(Error::new(
                    "command_failed",
                    format!(
                        "exit {status}: {}",
                        String::from_utf8_lossy(&stderr[..stderr.len().min(2048)])
                    ),
                ));
            }
            return Ok(stdout);
        }
        if Instant::now() >= deadline {
            // SAFETY: this is the process group started above, including pipe-holding children.
            unsafe {
                libc::kill(-(pid as i32), libc::SIGKILL);
            }
            let _ = child.wait();
            return Err(Error::new(
                "adapter_timeout",
                format!("command exceeded {seconds}s"),
            ));
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

// Portman protocol: four-byte big-endian length + tagged JSON, one request per
// connection. Only ServiceStatus is used; no config sync or implicit adoption.
fn portman_status(socket: &str, service: &str) -> Result<Proof> {
    use std::os::unix::net::UnixStream;
    let mut stream = UnixStream::connect(socket)
        .map_err(|e| Error::new("adapter_unavailable", e.to_string()))?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    let request = br#"{"kind":"service_status"}"#;
    stream.write_all(&(request.len() as u32).to_be_bytes())?;
    stream.write_all(request)?;
    let mut size = [0u8; 4];
    stream.read_exact(&mut size)?;
    let size = u32::from_be_bytes(size) as usize;
    if size > 1024 * 1024 {
        return Err(Error::new("adapter_output", "Portman frame exceeds 1 MiB"));
    }
    let mut body = vec![0u8; size];
    stream.read_exact(&mut body)?;
    let value: serde_json::Value = serde_json::from_slice(&body)?;
    if value.get("kind").and_then(|v| v.as_str()) != Some("service_statuses") {
        return Err(Error::new(
            "adapter_proof",
            "Portman did not return service_statuses (authenticated brokers require a trusted wrapper)",
        ));
    }
    let services = value
        .get("services")
        .and_then(|v| v.as_array())
        .ok_or_else(|| Error::new("adapter_proof", "Portman omitted services"))?;
    let found: Vec<_> = services
        .iter()
        .filter(|s| s.get("name").and_then(|v| v.as_str()) == Some(service))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn captured_replacement_is_restored_without_destroying_it() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let token = uuid::Uuid::new_v4().to_string();
        std::fs::write(root.join("owned"), "original").unwrap();
        let receipt = file_proof(&root, "owned", FileMode::Copy, &token).unwrap();
        std::fs::rename(root.join("owned"), root.join("original")).unwrap();
        std::fs::write(root.join("owned"), "replacement").unwrap();
        assert_eq!(
            quarantine_delete(&root, "owned", FileMode::Copy, &token, Some(&receipt))
                .unwrap_err()
                .code,
            "ownership"
        );
        assert_eq!(
            std::fs::read_to_string(root.join("owned")).unwrap(),
            "replacement"
        );
        assert_eq!(
            std::fs::read_to_string(root.join("original")).unwrap(),
            "original"
        );
    }

    #[test]
    fn interrupted_capture_resumes_without_touching_new_destination() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let token = uuid::Uuid::new_v4().to_string();
        std::fs::write(root.join("owned"), "original").unwrap();
        let receipt = file_proof(&root, "owned", FileMode::Copy, &token).unwrap();
        let quarantine = root.join(format!(".treetender-delete-{token}"));
        std::fs::create_dir(&quarantine).unwrap();
        std::fs::rename(root.join("owned"), quarantine.join("resource")).unwrap();
        std::fs::write(root.join("owned"), "replacement").unwrap();
        assert_eq!(
            file_proof(&root, "owned", FileMode::Copy, &token).unwrap(),
            receipt
        );
        let outcome =
            quarantine_delete(&root, "owned", FileMode::Copy, &token, Some(&receipt)).unwrap();
        assert_ne!(outcome.instance, receipt.instance);
        assert_eq!(
            std::fs::read_to_string(root.join("owned")).unwrap(),
            "replacement"
        );
        assert!(!quarantine.exists());
    }

    #[test]
    fn commands_bound_output_and_timeout_pipe_holding_children() {
        let temp = tempfile::tempdir().unwrap();
        let output = run(
            &["/bin/sh".into(), "-c".into(), "yes x".into()],
            temp.path(),
            None,
            3,
        )
        .unwrap_err();
        assert_eq!(output.code, "adapter_output");
        let timeout = run(
            &["/bin/sh".into(), "-c".into(), "sleep 5 & exit 0".into()],
            temp.path(),
            None,
            1,
        )
        .unwrap_err();
        assert_eq!(timeout.code, "adapter_timeout");
    }

    #[test]
    fn tree_inputs_refuse_symlink_children() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        std::fs::create_dir(root.join("src")).unwrap();
        std::fs::write(root.join("real"), "input").unwrap();
        std::os::unix::fs::symlink(root.join("real"), root.join("src/link")).unwrap();
        assert!(tree_snapshot(&root, "src").is_err());
    }
}
