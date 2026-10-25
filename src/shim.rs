// Command shims: a tiny executable per exported command so `code` works as well
// as `dev run code`. Shims live next to the dev binary (already on PATH) or in
// $DEV_BIN_DIR, and carry a marker line so dev only ever rewrites or removes
// files it created itself.
use crate::manager::Result;
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
};
use tempfile::NamedTempFile;

const MARKER: &str = "# dev-shim: package=";

// Where shims go by default: $DEV_BIN_DIR, else the directory holding `dev`.
pub fn default_dir() -> Option<PathBuf> {
    if !cfg!(unix) {
        return None;
    }
    if let Some(dir) = std::env::var_os("DEV_BIN_DIR").filter(|value| !value.is_empty()) {
        return Some(dir.into());
    }
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf))
}

fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn content(package: &str, command: &str, store: &Path) -> String {
    let dev = std::env::current_exe()
        .ok()
        .and_then(|path| path.into_os_string().into_string().ok())
        .unwrap_or_else(|| "dev".into());
    format!(
        "#!/bin/sh\n{MARKER}{package}\nexec {} --root {} run {} \"$@\"\n",
        quote(&dev),
        quote(&store.to_string_lossy()),
        quote(command)
    )
}

// The package a shim file belongs to, or None if the file is not a dev shim.
pub fn owner(path: &Path) -> Option<String> {
    let meta = fs::symlink_metadata(path).ok()?;
    if !meta.is_file() {
        return None;
    }
    let mut head = String::new();
    fs::File::open(path)
        .ok()?
        .take(1024)
        .read_to_string(&mut head)
        .ok()?;
    head.lines()
        .find_map(|line| line.strip_prefix(MARKER))
        .map(str::to_owned)
}

// Write a shim per command. Returns the commands skipped because a file that dev
// does not manage already uses the name.
pub fn write<'a>(
    dir: &Path,
    package: &str,
    store: &Path,
    commands: impl Iterator<Item = &'a String>,
) -> Result<Vec<String>> {
    let mut skipped = Vec::new();
    for command in commands {
        let path = dir.join(command);
        let taken = fs::symlink_metadata(&path).is_ok();
        if taken && owner(&path).as_deref() != Some(package) {
            skipped.push(command.clone());
            continue;
        }
        fs::create_dir_all(dir)?;
        let mut temp = NamedTempFile::new_in(dir)?;
        temp.write_all(content(package, command, store).as_bytes())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            temp.as_file()
                .set_permissions(fs::Permissions::from_mode(0o755))?;
        }
        temp.persist(&path)?;
    }
    Ok(skipped)
}

// Remove this package's shims for the given commands; foreign files are kept.
pub fn remove<'a>(
    dir: &Path,
    package: &str,
    commands: impl Iterator<Item = &'a String>,
) -> Result<()> {
    for command in commands {
        let path = dir.join(command);
        if owner(&path).as_deref() == Some(package) {
            fs::remove_file(path)?;
        }
    }
    Ok(())
}
