// Launcher integration: XDG .desktop entries for installed app packages.
// Dev OS scans $XDG_DATA_HOME/applications through GIO and refreshes live, so
// entries written here show up in the launcher and panel without a restart.
use crate::manager::{Manifest, PackageKind, Result};
use std::{fs, io::Write, path::Path};
use tempfile::NamedTempFile;

pub fn entry_filename(command: &str) -> String {
    format!("dev-{command}.desktop")
}

// Desktop-entry values cannot hold control characters.
fn escape_value(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect::<String>()
        .trim()
        .to_owned()
}

// Quote the dev executable for the Exec line and escape desktop field codes.
fn exec_token(value: &str) -> String {
    let escaped = value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('%', "%%");
    if escaped.contains(char::is_whitespace) {
        format!("\"{escaped}\"")
    } else {
        escaped
    }
}

fn dev_executable() -> String {
    std::env::current_exe()
        .ok()
        .and_then(|path| path.into_os_string().into_string().ok())
        .unwrap_or_else(|| "dev".into())
}

fn entry_content(manifest: &Manifest, command: &str, package_root: &Path) -> String {
    let name = if !manifest.fullname.is_empty() {
        &manifest.fullname
    } else if !manifest.shortname.is_empty() {
        &manifest.shortname
    } else {
        command
    };
    let mut entry = String::new();
    entry.push_str("[Desktop Entry]\n");
    entry.push_str("Type=Application\n");
    entry.push_str(&format!("Name={}\n", escape_value(name)));
    if !manifest.description.is_empty() {
        entry.push_str(&format!(
            "Comment={}\n",
            escape_value(&manifest.description)
        ));
    }
    entry.push_str(&format!(
        "Exec={} --root {} run {command}\n",
        exec_token(&dev_executable()),
        exec_token(
            &package_root
                .parent()
                .and_then(Path::parent)
                .unwrap_or(package_root)
                .to_string_lossy()
        )
    ));
    if !manifest.icon.is_empty() {
        if let Some(icon) = package_root.join(&manifest.icon).to_str() {
            entry.push_str(&format!("Icon={}\n", escape_value(icon)));
        }
    } else {
        entry.push_str("Icon=application-x-executable\n");
    }
    entry.push_str("Terminal=false\n");
    entry.push_str(&format!("X-DevOS-Package={}\n", manifest.name));
    entry
}

fn is_app(manifest: &Manifest) -> bool {
    matches!(manifest.kind, PackageKind::App)
}

pub fn write_entries(applications: &Path, manifest: &Manifest, package_root: &Path) -> Result<()> {
    if !is_app(manifest) {
        return Ok(());
    }
    fs::create_dir_all(applications)?;
    for command in manifest.bin.keys() {
        let content = entry_content(manifest, command, package_root);
        let mut temp = NamedTempFile::new_in(applications)?;
        temp.write_all(content.as_bytes())?;
        temp.persist(applications.join(entry_filename(command)))?;
    }
    Ok(())
}

pub fn remove(applications: &Path, manifest: &Manifest) -> Result<()> {
    if !is_app(manifest) {
        return Ok(());
    }
    for command in manifest.bin.keys() {
        match fs::remove_file(applications.join(entry_filename(command))) {
            Ok(()) => (),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}

// Remove every dev-*.desktop entry stamped with this package's marker, even if
// its command is no longer in the manifest (leftovers from older versions or an
// interrupted upgrade). Files without the exact marker line are never touched.
pub fn remove_package_entries(applications: &Path, package: &str) -> Result<()> {
    let marker = format!("X-DevOS-Package={package}");
    let entries = match fs::read_dir(applications) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e.into()),
    };
    for entry in entries {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if !name.starts_with("dev-") || !name.ends_with(".desktop") || !entry.file_type()?.is_file()
        {
            continue;
        }
        // Entries written by dev are tiny; skip anything unexpectedly large.
        if entry.metadata()?.len() > 64 * 1024 {
            continue;
        }
        if fs::read_to_string(entry.path()).is_ok_and(|text| text.lines().any(|l| l == marker)) {
            fs::remove_file(entry.path())?;
        }
    }
    Ok(())
}

// Swap entries when the active manifest changes (upgrade or rollback).
pub fn refresh(
    applications: &Path,
    previous: Option<&Manifest>,
    current: &Manifest,
    package_root: &Path,
) -> Result<()> {
    if let Some(previous) = previous.filter(|manifest| is_app(manifest)) {
        remove(applications, previous)?;
    }
    write_entries(applications, current, package_root)
}
