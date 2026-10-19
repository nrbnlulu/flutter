use std::{
    fs, io,
    os::unix::fs::symlink,
    path::{Component, Path, PathBuf},
    process::Command,
};

use anyhow::{Context, Result, bail};

pub fn describe(command: &Command) -> String {
    let mut parts = vec![command.get_program().to_string_lossy().into_owned()];
    parts.extend(command.get_args().map(|a| a.to_string_lossy().into_owned()));
    parts.join(" ")
}

/// Runs `command` with inherited stdio and fails if it exits unsuccessfully.
pub fn run(command: &mut Command) -> Result<()> {
    eprintln!("$ {}", describe(command));
    let status = command
        .status()
        .with_context(|| format!("failed to start `{}`", describe(command)))?;
    if !status.success() {
        bail!("`{}` failed ({status})", describe(command));
    }
    Ok(())
}

/// Points `link` at `target`, replacing whatever is there. A link that already
/// has the right target is left alone so file times stay stable.
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
    symlink(target, link)
        .with_context(|| format!("link {} -> {}", link.display(), target.display()))
}

/// Removes a file, symlink or directory tree; missing paths are fine.
pub fn remove_path(path: &Path) -> Result<()> {
    let meta = match fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error).with_context(|| format!("stat {}", path.display())),
    };
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
    let contents = fs::read(source).with_context(|| format!("read {}", source.display()))?;
    if fs::read(destination).is_ok_and(|existing| existing == contents) {
        return Ok(());
    }
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(destination, contents).with_context(|| format!("write {}", destination.display()))
}

/// Recursively copies `source` into `destination`, replacing it.
pub fn copy_dir(source: &Path, destination: &Path) -> Result<()> {
    remove_path(destination)?;
    fs::create_dir_all(destination)?;
    for entry in fs::read_dir(source).with_context(|| format!("read {}", source.display()))? {
        let entry = entry?;
        let to = destination.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&entry.path(), &to)?;
        } else {
            fs::copy(entry.path(), &to)
                .with_context(|| format!("copy {}", entry.path().display()))?;
        }
    }
    Ok(())
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
