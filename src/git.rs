use crate::model::{Error, Result};
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
    for (key, _) in std::env::vars_os() {
        if key.to_string_lossy().starts_with("GIT_") {
            command.env_remove(key);
        }
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
    Ok(Discovery {
        path: root,
        git_dir,
        common_dir,
        primary_path: PathBuf::from(first),
    })
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
