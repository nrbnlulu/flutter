use std::{
    fs, io,
    path::{Component, Path, PathBuf},
    process::Command,
};

use anyhow::{Context, Result, bail};

static PROTOCOL_OUTPUT: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static BUILD_LOG: std::sync::Mutex<Option<std::sync::Arc<std::sync::Mutex<fs::File>>>> =
    std::sync::Mutex::new(None);
pub fn set_protocol_output(enabled: bool) {
    PROTOCOL_OUTPUT.store(enabled, std::sync::atomic::Ordering::Release);
}
pub fn set_build_log(file: fs::File) {
    *BUILD_LOG.lock().unwrap() = Some(std::sync::Arc::new(std::sync::Mutex::new(file)));
}
fn forward_build(
    mut source: impl io::Read + Send + 'static,
) -> std::thread::JoinHandle<io::Result<()>> {
    use io::Write;
    let log = BUILD_LOG.lock().unwrap().clone();
    std::thread::spawn(move || {
        let mut buffer = [0; 8192];
        let mut failure = None;
        loop {
            let count = source.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            if let Some(log) = &log {
                let mut log = log.lock().unwrap();
                let written = log.metadata().and_then(|metadata| {
                    if metadata.len().saturating_add(count as u64) > 64 * 1024 * 1024 {
                        return Err(io::Error::other(
                            "build log exceeded 64 MiB retention limit",
                        ));
                    }
                    log.write_all(&buffer[..count])
                });
                if let Err(error) = written {
                    failure = Some(error);
                }
            }
            let _ = io::stderr().lock().write_all(&buffer[..count]);
        }
        match failure {
            Some(error) => Err(error),
            None => Ok(()),
        }
    })
}

pub fn describe(command: &Command) -> String {
    let mut parts = vec![command.get_program().to_string_lossy().into_owned()];
    parts.extend(command.get_args().map(|a| a.to_string_lossy().into_owned()));
    parts.join(" ")
}

/// Runs `command` with inherited stdio and fails if it exits unsuccessfully.
pub fn run(command: &mut Command) -> Result<()> {
    eprintln!("$ {}", describe(command));
    let status = if PROTOCOL_OUTPUT.load(std::sync::atomic::Ordering::Acquire) {
        use std::process::Stdio;
        let mut child = command
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| format!("failed to start `{}`", describe(command)))?;
        let stdout = forward_build(child.stdout.take().unwrap());
        let stderr = forward_build(child.stderr.take().unwrap());
        let status = child.wait()?;
        stdout
            .join()
            .map_err(|_| anyhow::anyhow!("build log worker panicked"))??;
        stderr
            .join()
            .map_err(|_| anyhow::anyhow!("build log worker panicked"))??;
        status
    } else {
        command
            .status()
            .with_context(|| format!("failed to start `{}`", describe(command)))?
    };
    if !status.success() {
        bail!("`{}` failed ({status})", describe(command));
    }
    Ok(())
}

/// Points `link` at `target`, replacing whatever is there. A link that already
/// has the right target is left alone so file times stay stable.
#[cfg(unix)]
pub fn replace_link(link: &Path, target: &Path) -> Result<()> {
    match fs::symlink_metadata(link) {
        Ok(meta) => {
            if meta.file_type().is_symlink() && fs::read_link(link)? == target && link.exists() {
                return Ok(());
            }
            remove_path(link)?;
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error).with_context(|| format!("stat {}", link.display())),
    }
    if let Some(parent) = link.parent() {
        fs::create_dir_all(parent)?;
    }
    std::os::unix::fs::symlink(target, link)
        .with_context(|| format!("link {} -> {}", link.display(), target.display()))
}

/// Prefer native links; copy when Windows developer-mode symlinks are unavailable.
#[cfg(windows)]
pub fn replace_link(link: &Path, target: &Path) -> Result<()> {
    replace_link_with(link, target, |target, link| {
        if target.is_dir() {
            std::os::windows::fs::symlink_dir(target, link)
        } else {
            std::os::windows::fs::symlink_file(target, link)
        }
    })
}

#[cfg(windows)]
fn replace_link_with(
    link: &Path,
    target: &Path,
    symlink: impl FnOnce(&Path, &Path) -> io::Result<()>,
) -> Result<()> {
    let target = if target.is_absolute() {
        target.to_owned()
    } else {
        link.parent().context("link has no parent")?.join(target)
    };
    if fs::symlink_metadata(link).is_ok_and(|meta| meta.file_type().is_symlink()) {
        if fs::read_link(link)? == target && link.exists() {
            return Ok(());
        }
        remove_path(link)?;
    }
    if !link.exists() {
        if let Some(parent) = link.parent() {
            fs::create_dir_all(parent)?;
        }
        let result = symlink(&target, link);
        if result.is_ok() {
            return Ok(());
        }
    }
    if target.is_dir() {
        copy_dir(&target, link)
    } else {
        copy_if_changed(&target, link)
    }
}

/// Removes a file, symlink or directory tree; missing paths are fine.
pub fn remove_path(path: &Path) -> Result<()> {
    let meta = match fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error).with_context(|| format!("stat {}", path.display())),
    };
    #[cfg(windows)]
    {
        use std::os::windows::fs::FileTypeExt;
        if meta.file_type().is_symlink_dir() {
            return fs::remove_dir(path).with_context(|| format!("remove {}", path.display()));
        }
    }
    if meta.is_dir() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    }
    .with_context(|| format!("remove {}", path.display()))
}

/// Writes `contents` unless the file already has them.
pub fn write_if_changed(path: &Path, contents: &str) -> Result<bool> {
    if fs::read_to_string(path).is_ok_and(|existing| existing == contents) {
        return Ok(false);
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, contents).with_context(|| format!("write {}", path.display()))?;
    Ok(true)
}

pub fn copy_if_changed(source: &Path, destination: &Path) -> Result<()> {
    use std::io::Read;
    // Compare in bounded chunks: engine libraries can be hundreds of MB.
    if destination.is_file() && fs::metadata(source)?.len() == fs::metadata(destination)?.len() {
        let mut left = io::BufReader::new(fs::File::open(source)?);
        let mut right = io::BufReader::new(fs::File::open(destination)?);
        let mut a = [0; 65536];
        let mut b = [0; 65536];
        loop {
            let count = left.read(&mut a)?;
            if count == 0 {
                return Ok(());
            }
            right.read_exact(&mut b[..count])?;
            if a[..count] != b[..count] {
                break;
            }
        }
    }
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent)?;
    }
    if destination.is_dir() {
        remove_path(destination)?;
    }
    fs::copy(source, destination).with_context(|| format!("copy {}", source.display()))?;
    Ok(())
}

/// Remove entries no longer included in a tool-owned staging directory.
pub fn retain_files(directory: &Path, keep: &[PathBuf]) -> Result<()> {
    if directory.is_dir() {
        for entry in fs::read_dir(directory)? {
            let path = entry?.path();
            if !keep.contains(&path) {
                remove_path(&path)?;
            }
        }
    }
    Ok(())
}

/// Mirrors a directory, retaining unchanged files and deleting obsolete entries.
pub fn copy_dir(source: &Path, destination: &Path) -> Result<()> {
    if destination.exists() && !destination.is_dir() {
        remove_path(destination)?;
    }
    fs::create_dir_all(destination)?;
    let mut keep = Vec::new();
    for entry in fs::read_dir(source).with_context(|| format!("read {}", source.display()))? {
        let entry = entry?;
        let to = destination.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&entry.path(), &to)?;
        } else {
            copy_if_changed(&entry.path(), &to)?;
        }
        keep.push(to);
    }
    retain_files(destination, &keep)
}

/// Resolves `.` and `..` without touching the file system.
pub fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{testing::tempdir, *};

    #[test]
    #[cfg(windows)]
    fn denied_symlinks_copy_and_refresh_without_stale_files() {
        let root = testing::tempdir("link_denied");
        let source = root.join("source");
        let link = root.join("copy");
        fs::create_dir_all(&source).unwrap();
        fs::write(source.join("old"), "old").unwrap();
        let denied = |_: &Path, _: &Path| Err(io::Error::from_raw_os_error(1314));
        replace_link_with(&link, &source, denied).unwrap();
        assert!(!link.is_symlink());
        assert_eq!(fs::read_to_string(link.join("old")).unwrap(), "old");
        fs::remove_file(source.join("old")).unwrap();
        fs::write(source.join("new"), "updated").unwrap();
        replace_link_with(&link, &source, denied).unwrap();
        assert!(!link.join("old").exists());
        assert_eq!(fs::read_to_string(link.join("new")).unwrap(), "updated");
        let file = root.join("file-copy");
        replace_link_with(&file, &source.join("new"), denied).unwrap();
        fs::write(source.join("new"), "changed").unwrap();
        replace_link_with(&file, &source.join("new"), denied).unwrap();
        assert_eq!(fs::read_to_string(file).unwrap(), "changed");
    }

    #[test]
    fn staging_preserves_unchanged_files_and_removes_obsolete_entries() {
        let dir = tempdir("incremental_staging");
        let source = dir.join("source");
        let destination = dir.join("destination");
        fs::create_dir_all(source.join("nested")).unwrap();
        fs::write(source.join("nested/keep"), "same").unwrap();
        fs::write(source.join("change"), "old!").unwrap();
        copy_dir(&source, &destination).unwrap();
        let sentinel = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1000);
        fs::OpenOptions::new()
            .write(true)
            .open(destination.join("nested/keep"))
            .unwrap()
            .set_modified(sentinel)
            .unwrap();
        fs::write(destination.join("obsolete"), "gone").unwrap();
        fs::write(source.join("change"), "new!").unwrap();
        copy_dir(&source, &destination).unwrap();
        assert_eq!(
            fs::metadata(destination.join("nested/keep"))
                .unwrap()
                .modified()
                .unwrap(),
            sentinel
        );
        assert_eq!(
            fs::read_to_string(destination.join("change")).unwrap(),
            "new!"
        );
        assert!(!destination.join("obsolete").exists());
        fs::remove_dir_all(source.join("nested")).unwrap();
        copy_dir(&source, &destination).unwrap();
        assert!(!destination.join("nested").exists());
    }

    #[test]
    fn normalize_resolves_dots() {
        assert_eq!(
            normalize(Path::new("/a/b/../c/./d")),
            PathBuf::from("/a/c/d")
        );
        assert_eq!(normalize(Path::new("/a/b/..")), PathBuf::from("/a"));
    }

    #[test]
    fn replace_link_is_idempotent_and_repoints() {
        let dir = tempdir("replace_link");
        let (a, b) = (dir.join("a"), dir.join("b"));
        fs::write(&a, "a").unwrap();
        fs::write(&b, "b").unwrap();
        let link = dir.join("link");
        replace_link(&link, &a).unwrap();
        replace_link(&link, &a).unwrap();
        assert_eq!(fs::read_to_string(&link).unwrap(), "a");
        replace_link(&link, &b).unwrap();
        assert_eq!(fs::read_to_string(&link).unwrap(), "b");
    }
}

#[cfg(test)]
pub(crate) mod testing {
    use std::{fs, path::PathBuf};

    pub fn tempdir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rlutter-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }
}
