use crate::model::{Error, Result, Workspace};
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug)]
pub struct Discovery {
    pub path: PathBuf,
    pub git_dir: PathBuf,
    pub common_dir: PathBuf,
    pub primary_path: PathBuf,
}
pub fn clean_environment(command: &mut Command) {
    for key in [
        "GIT_ALTERNATE_OBJECT_DIRECTORIES",
        "GIT_CONFIG",
        "GIT_CONFIG_PARAMETERS",
        "GIT_CONFIG_COUNT",
        "GIT_OBJECT_DIRECTORY",
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_IMPLICIT_WORK_TREE",
        "GIT_GRAFT_FILE",
        "GIT_INDEX_FILE",
        "GIT_NO_REPLACE_OBJECTS",
        "GIT_REPLACE_REF_BASE",
        "GIT_PREFIX",
        "GIT_SHALLOW_FILE",
        "GIT_COMMON_DIR",
        "GIT_CEILING_DIRECTORIES",
        "GIT_DISCOVERY_ACROSS_FILESYSTEM",
    ] {
        command.env_remove(key);
    }
}
fn git(path: &Path, args: &[&str]) -> Result<String> {
    let mut command = Command::new("git");
    command.arg("-C").arg(path).args(args);
    clean_environment(&mut command);
    let output = command.output()?;
    if !output.status.success() {
        return Err(Error::new(
            "git",
            String::from_utf8_lossy(&output.stderr).trim(),
        ));
    }
    String::from_utf8(output.stdout)
        .map(|s| s.trim_end_matches('\n').to_owned())
        .map_err(|e| Error::new("git", e.to_string()))
}
pub fn discover(path: &Path) -> Result<Discovery> {
    if git(path, &["rev-parse", "--is-bare-repository"])? == "true" {
        return Err(bare_layout());
    }
    let root = PathBuf::from(git(path, &["rev-parse", "--show-toplevel"])?).canonicalize()?;
    let git_dir =
        PathBuf::from(git(&root, &["rev-parse", "--absolute-git-dir"])?).canonicalize()?;
    let common = git(
        &root,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )?;
    let common_dir = PathBuf::from(common).canonicalize()?;
    let listing = git(
        &root,
        &[
            "-c",
            "core.quotePath=false",
            "worktree",
            "list",
            "--porcelain",
            "-z",
        ],
    )?;
    let first = listing
        .split('\0')
        .next()
        .and_then(|s| s.strip_prefix("worktree "))
        .ok_or_else(|| Error::new("git", "cannot determine primary checkout"))?;
    if listing
        .split('\0')
        .take_while(|part| !part.is_empty())
        .any(|part| part == "bare")
    {
        return Err(bare_layout());
    }
    Ok(Discovery {
        path: root,
        git_dir,
        common_dir,
        primary_path: PathBuf::from(first),
    })
}

fn bare_layout() -> Error {
    Error::new(
        "unsupported_bare_layout",
        "bare repository worktree layouts need an explicit primary checkout designation and are not supported yet",
    )
}

/// Whether this exact checkout still occupies its registered location.
/// False means the old Git identity is gone/replaced. A missing/rebound path with
/// live old Git metadata is ambiguous (possibly moved), so it returns an error.
/// Unsafe paths also return errors instead of proving disappearance.
pub fn is_current(workspace: &Workspace) -> Result<bool> {
    if !safe_directory_exists(&workspace.common_dir)?
        || identity(&workspace.common_dir, false)?.as_deref() != Some(&workspace.repository_id)
    {
        return Err(Error::new(
            "checkout_location_unknown",
            "repository location or identity is unavailable; it may have moved or been deleted; repair and register the repository at its current location before cleanup or releasing references",
        ));
    }
    let path_exists = safe_directory_exists(&workspace.path)?;
    if !safe_directory_exists(&workspace.git_dir)?
        || identity(&workspace.git_dir, false)?.as_deref() != Some(&workspace.id)
    {
        return Ok(false);
    }
    if !path_exists {
        return Err(location_unknown());
    }
    match std::fs::symlink_metadata(workspace.path.join(".git")) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(location_unknown()),
        Err(e) => return Err(e.into()),
        Ok(m) if m.file_type().is_symlink() => {
            return Err(Error::new(
                "unsafe_path",
                "checkout Git metadata cannot be a symlink",
            ));
        }
        _ => (),
    }
    let d = discover(&workspace.path)?;
    let current = d.path == workspace.path
        && d.git_dir == workspace.git_dir
        && d.common_dir == workspace.common_dir
        && (d.git_dir == d.common_dir) == workspace.primary
        && identity(&d.git_dir, false)?.as_deref() == Some(&workspace.id)
        && identity(&d.common_dir, false)?.as_deref() == Some(&workspace.repository_id);
    if current {
        Ok(true)
    } else {
        Err(location_unknown())
    }
}

fn location_unknown() -> Error {
    Error::new(
        "checkout_location_unknown",
        "recorded location is missing or rebound while its Git identity is still live; if moved, run git worktree repair <new-path> then register it; if deleted, git worktree prune --expire now explicitly asserts deletion",
    )
}

fn safe_directory_exists(path: &Path) -> Result<bool> {
    if !path.is_absolute() {
        return Err(Error::new("unsafe_path", "checkout path must be absolute"));
    }
    let mut prefix = PathBuf::from("/");
    for part in path.components().skip(1) {
        if !matches!(part, std::path::Component::Normal(_)) {
            return Err(Error::new(
                "unsafe_path",
                "checkout path must be normalized",
            ));
        }
        prefix.push(part);
        match std::fs::symlink_metadata(&prefix) {
            Ok(m) if m.is_dir() && !m.file_type().is_symlink() => (),
            Ok(_) => {
                return Err(Error::new(
                    "unsafe_path",
                    format!(
                        "checkout path contains a symlink or non-directory: {}",
                        prefix.display()
                    ),
                ));
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(e) => return Err(e.into()),
        }
    }
    Ok(true)
}

pub(crate) fn identity(directory: &Path, create: bool) -> Result<Option<String>> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let path = directory.join("treetender-id");
    let mut options = std::fs::OpenOptions::new();
    options.read(true).custom_flags(libc::O_NOFOLLOW);
    match options.open(&path) {
        Ok(mut file) => {
            use std::io::Read;
            let mut id = String::new();
            file.read_to_string(&mut id)?;
            uuid::Uuid::parse_str(&id)
                .map_err(|_| Error::new("identity", "invalid Git identity marker"))?;
            Ok(Some(id))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound && create => {
            let id = uuid::Uuid::new_v4().to_string();
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW)
                .open(path)?;
            file.write_all(id.as_bytes())?;
            file.sync_all()?;
            std::fs::File::open(directory)?.sync_all()?;
            Ok(Some(id))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}
