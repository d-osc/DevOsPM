// Dev OS desktop extension support: recognizes the Extension Store package
// layout (everything under extension/ with extension.json) and installs into
// the same directories the desktop reads, so the Settings store keeps working.
use crate::manager::{
    check_plain_directory, collect_files, error, extract_archive, hash_reader, safe_path,
    sync_directory, sync_tree, FileRecord, Manager, Result,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
};
use tempfile::Builder;

// Limits mirror the desktop Extension Store so both installers agree.
pub const MAX_TARBALL: u64 = 32 * 1024 * 1024;
pub const MAX_BYTES: u64 = 128 * 1024 * 1024;
pub const MAX_FILES: usize = 4_000;
pub const ROOT_PREFIX: &str = "extension/";
pub const MANIFEST_NAME: &str = "extension.json";
pub const ASSET_NAME: &str = "dev-os-extension.tar.gz";
const MAX_MANIFEST: u64 = 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExtensionManifest {
    pub id: String,
    pub name: String,
    pub description: String,
    pub version: String,
    pub api_version: u32,
    pub settings_version: u64,
    pub enabled_by_default: bool,
    pub settings: Vec<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entry: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub order: Option<i64>,
}

impl ExtensionManifest {
    pub fn is_ui(&self) -> bool {
        self.kind.as_deref() == Some("ui")
    }
    pub fn entry_path(&self) -> String {
        format!("dist/{}", self.entry.as_deref().unwrap_or_default())
    }
    pub fn validate(&self) -> Result<()> {
        if !valid_extension_id(&self.id) {
            return Err(error(
                "Dev OS extension IDs are lowercase namespaced names like org.example.status",
            ));
        }
        if self.id.starts_with("org.devos.") {
            return Err(error("Dev OS base packages cannot be replaced"));
        }
        if self.name.trim().is_empty() || self.description.trim().is_empty() {
            return Err(error("Dev OS extensions need a name and description"));
        }
        if self.api_version != 1 {
            return Err(error("Unsupported Dev OS extension API version"));
        }
        if semver(&self.version).is_none() {
            return Err(error(
                "Dev OS extension versions must be stable semver like 1.0.0",
            ));
        }
        match self.kind.as_deref() {
            None | Some("settings") | Some("ui") => (),
            Some(_) => return Err(error("Dev OS extension kind must be ui or settings")),
        }
        if self.is_ui() {
            if self.system != Some(false) {
                return Err(error(
                    "Dev OS extension packages cannot declare system: true",
                ));
            }
            let Some(entry) = &self.entry else {
                return Err(error("UI extensions need an entry filename"));
            };
            if !valid_entry(entry) {
                return Err(error(
                    "UI entry must be a flat JavaScript filename like app.js",
                ));
            }
            if reserved_entry(entry) {
                return Err(error("UI entry uses a reserved shared-runtime name"));
            }
            if self.order.is_none_or(|order| order < 0) {
                return Err(error("UI order must be a non-negative integer"));
            }
        } else {
            if self.system.is_some() || self.entry.is_some() || self.order.is_some() {
                return Err(error("system, entry and order only apply to kind: ui"));
            }
            if self.settings.is_empty() {
                return Err(error("Non-UI extensions must contribute settings sections"));
            }
        }
        validate_settings_shallow(&self.settings)
    }
}

// Registry record for one installed Dev OS extension.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ExtensionEntry {
    pub id: String,
    pub version: String,
    pub key: String,
    pub repository: String,
    pub bytes: u64,
    pub files: BTreeMap<String, FileRecord>,
}
impl ExtensionEntry {
    pub(crate) fn validate(&self) -> Result<()> {
        if !valid_extension_id(&self.id) || !valid_repository(&self.repository) {
            return Err(error("Invalid extension ID or repository"));
        }
        if !valid_key(&self.key, &self.version) {
            return Err(error("Invalid extension release key"));
        }
        let mut bytes = 0u64;
        for (path, record) in &self.files {
            safe_path(path)?;
            if record.sha256.len() != 64 || !record.sha256.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err(error("Invalid file record for an extension"));
            }
            bytes = bytes
                .checked_add(record.size)
                .ok_or_else(|| error("Invalid extension byte count"))?;
        }
        if bytes != self.bytes || bytes > MAX_BYTES || self.files.len() > MAX_FILES {
            return Err(error("Invalid extension sizes"));
        }
        Ok(())
    }
}

// install history kept inside every release directory, exactly like the
// desktop store writes it, so Settings can update and roll back our installs.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct InstallRecord {
    pub key: String,
    pub id: String,
    pub version: String,
    pub repository: String,
    #[serde(default)]
    pub previous: Option<InstallPrevious>,
}
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct InstallPrevious {
    pub key: String,
    pub id: String,
    pub version: String,
    pub repository: String,
}
impl InstallRecord {
    pub fn validate(&self) -> Result<()> {
        if !valid_extension_id(&self.id) || !valid_repository(&self.repository) {
            return Err(error("Invalid extension installation history"));
        }
        if !valid_key(&self.key, &self.version) {
            return Err(error("Invalid extension installation history"));
        }
        if let Some(previous) = &self.previous {
            if !valid_extension_id(&previous.id)
                || !valid_repository(&previous.repository)
                || !valid_key(&previous.key, &previous.version)
            {
                return Err(error("Invalid extension installation history"));
            }
        }
        Ok(())
    }
}

pub fn valid_key(key: &str, version: &str) -> bool {
    let (release, digest) = key.split_once('-').unwrap_or(("", ""));
    release == version && digest.len() == 12 && digest.bytes().all(|b| b.is_ascii_hexdigit())
}

pub fn valid_extension_id(id: &str) -> bool {
    if id.len() > 128 || id.is_empty() {
        return false;
    }
    let bytes = id.as_bytes();
    if !bytes[0].is_ascii_lowercase() {
        return false;
    }
    let mut index = 0;
    while index < bytes.len()
        && (bytes[index].is_ascii_lowercase() || bytes[index].is_ascii_digit())
    {
        index += 1;
    }
    let mut segments = 0;
    while index < bytes.len() {
        let separator = bytes[index];
        if separator != b'.' && separator != b'-' {
            return false;
        }
        index += 1;
        let start = index;
        while index < bytes.len()
            && (bytes[index].is_ascii_lowercase() || bytes[index].is_ascii_digit())
        {
            index += 1;
        }
        if index == start {
            return false;
        }
        segments += 1;
    }
    segments >= 1
}

pub fn valid_repository(value: &str) -> bool {
    if value.len() > 200 {
        return false;
    }
    let Some((owner, repository)) = value.split_once('/') else {
        return false;
    };
    valid_segment(owner, b"_-") && valid_segment(repository, b"_.-")
}
fn valid_segment(segment: &str, extra: &[u8]) -> bool {
    let bytes = segment.as_bytes();
    !bytes.is_empty()
        && bytes[0].is_ascii_alphanumeric()
        && bytes
            .iter()
            .all(|b| b.is_ascii_alphanumeric() || extra.contains(b))
}

fn valid_entry(entry: &str) -> bool {
    let bytes = entry.as_bytes();
    entry.len() > 3
        && entry.ends_with(".js")
        && bytes[0].is_ascii_lowercase()
        && bytes[..bytes.len() - 3]
            .iter()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'.' || *b == b'-')
}

const RESERVED_ENTRIES: &[&str] = &[
    "react-gtk",
    "core",
    "extensions",
    "updates",
    "runtime",
    "services",
    "compat",
    "config",
    "shell",
    "supervisor",
];
fn reserved_entry(entry: &str) -> bool {
    RESERVED_ENTRIES.iter().any(|name| {
        let rest = entry.strip_prefix(name).unwrap_or("");
        rest == ".js" || rest.starts_with('-')
    })
}

fn valid_setting_key(key: &str) -> bool {
    let bytes = key.as_bytes();
    !bytes.is_empty()
        && bytes[0].is_ascii_lowercase()
        && bytes
            .iter()
            .all(|b| b.is_ascii_alphanumeric() || *b == b'_')
}

// The desktop validates the full settings schema at load time; this shallow
// check only rejects obviously corrupt manifests before anything is written.
fn validate_settings_shallow(settings: &[serde_json::Value]) -> Result<()> {
    let mut section_ids = BTreeSet::new();
    let mut field_keys = BTreeSet::new();
    for section in settings {
        let Some(object) = section.as_object() else {
            return Err(error("Settings sections must be objects"));
        };
        let Some(id) = object.get("id").and_then(|value| value.as_str()) else {
            return Err(error("Settings sections need a string id"));
        };
        if id.is_empty() || !section_ids.insert(id.to_owned()) {
            return Err(error("Settings section IDs must be unique"));
        }
        match object.get("fields") {
            None | Some(serde_json::Value::Null) => (),
            Some(serde_json::Value::Array(fields)) => {
                for field in fields {
                    let Some(key) = field
                        .as_object()
                        .and_then(|object| object.get("key"))
                        .and_then(|value| value.as_str())
                    else {
                        return Err(error("Settings fields need a string key"));
                    };
                    if !valid_setting_key(key) || !field_keys.insert(key.to_owned()) {
                        return Err(error("Invalid or duplicate settings field key"));
                    }
                }
            }
            Some(_) => return Err(error("Settings fields must be an array")),
        }
    }
    Ok(())
}

pub fn stable_version(version: &str) -> bool {
    semver(version).is_some()
}

// Stable semver only: no prerelease, no leading v.
pub fn semver(version: &str) -> Option<(u64, u64, u64)> {
    let mut parts = version.split('.');
    let mut numbers = [0u64; 3];
    for slot in &mut numbers {
        let part = parts.next()?;
        if part.is_empty() || !part.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        *slot = part.parse().ok()?;
    }
    if parts.next().is_some() {
        return None;
    }
    Some((numbers[0], numbers[1], numbers[2]))
}

pub fn read_manifest(path: &Path) -> Result<ExtensionManifest> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(error("extension.json must be a regular file"));
    }
    if metadata.len() > MAX_MANIFEST {
        return Err(error("extension.json exceeds 1 MiB"));
    }
    Ok(serde_json::from_reader(File::open(path)?)?)
}

// The desktop store holds install.lock during every write; share it so the
// CLI and Settings never flip the package symlink at the same time.
struct StoreLock(PathBuf);
impl StoreLock {
    fn acquire(path: PathBuf) -> Result<Self> {
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(_) => Ok(Self(path)),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Err(error(
                "Another Dev OS extension installation is in progress",
            )),
            Err(e) => Err(e.into()),
        }
    }
}
impl Drop for StoreLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

fn create_private_dir(path: &Path) -> Result<()> {
    fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn read_selection(link: &Path, id: &str) -> Result<Option<InstallRecord>> {
    let path = link.join("store-install.json");
    if fs::symlink_metadata(&path).is_err() {
        return Ok(None);
    }
    let record: InstallRecord = serde_json::from_reader(File::open(&path)?)?;
    record.validate()?;
    if record.id != id {
        return Err(error("Installed extension ID does not match its history"));
    }
    Ok(Some(record))
}

// Point extensions/<id> at a release directory atomically, like the desktop.
fn select_release(packages: &Path, id: &str, target: &Path) -> Result<()> {
    let link = packages.join(id);
    if let Ok(metadata) = fs::symlink_metadata(&link) {
        if !metadata.file_type().is_symlink() {
            return Err(error("An existing local package owns this extension ID"));
        }
    }
    let temporary = packages.join(format!(".store-{:012x}", rand_suffix()));
    #[cfg(unix)]
    std::os::unix::fs::symlink(target, &temporary)?;
    #[cfg(windows)]
    std::os::windows::fs::symlink_dir(target, &temporary).map_err(|e| {
        error(format!(
            "Linking the extension needs Developer Mode on Windows: {e}"
        ))
    })?;
    if let Err(e) = fs::rename(&temporary, &link) {
        let _ = fs::remove_file(&temporary);
        return Err(e.into());
    }
    sync_directory(packages)
}

fn rand_suffix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|time| time.as_nanos() as u64)
        .unwrap_or(0)
        ^ (std::process::id() as u64) << 32
}

fn record_matches(expected: &FileRecord, actual: &FileRecord) -> bool {
    if expected.sha256 != actual.sha256 || expected.size != actual.size {
        return false;
    }
    #[cfg(unix)]
    {
        expected.executable == actual.executable
    }
    #[cfg(not(unix))]
    {
        true
    }
}

fn hash_tree(root: &Path) -> Result<BTreeMap<String, FileRecord>> {
    let mut files = BTreeMap::new();
    for path in collect_files(root)? {
        let full = root.join(&path);
        let metadata = fs::symlink_metadata(&full)?;
        #[cfg(unix)]
        let executable = {
            use std::os::unix::fs::PermissionsExt;
            metadata.permissions().mode() & 0o111 != 0
        };
        #[cfg(not(unix))]
        let executable = false;
        let (sha256, size) = hash_reader(File::open(&full)?)?;
        files.insert(
            path,
            FileRecord {
                sha256,
                size,
                executable,
            },
        );
    }
    Ok(files)
}

impl Manager {
    pub(crate) fn install_devos_extension(
        &mut self,
        archive: &Path,
        manifest: &ExtensionManifest,
        repository: &str,
    ) -> Result<ExtensionEntry> {
        if !valid_repository(repository) {
            return Err(error("Extension repository must look like owner/repo"));
        }
        manifest.validate()?;
        let store = self.devos.join("extension-store");
        let releases = store.join("releases");
        let packages = self.devos.join("extensions");
        for directory in [&store, &releases, &packages] {
            create_private_dir(directory)?;
        }
        let _lock = StoreLock::acquire(store.join("install.lock"))?;

        let id = manifest.id.clone();
        let release_root = releases.join(&id);
        create_private_dir(&release_root)?;
        let staging = Builder::new()
            .prefix(".staging-")
            .tempdir_in(&release_root)?;
        let extracted = extract_archive(
            archive,
            staging.path(),
            Some(ROOT_PREFIX),
            &[MANIFEST_NAME],
            MAX_FILES,
            MAX_BYTES,
        )?;
        // The archive may have changed since the detection pass.
        let staged = read_manifest(&staging.path().join(MANIFEST_NAME))?;
        if staged != *manifest {
            return Err(error(
                "The extension archive changed while installing; retry",
            ));
        }
        if extracted.files.contains_key("store-install.json") {
            return Err(error("Extension packages cannot ship store-install.json"));
        }
        if manifest.is_ui() && !extracted.files.contains_key(&manifest.entry_path()) {
            return Err(error(format!(
                "Extension entry file is missing: {}",
                manifest.entry_path()
            )));
        }
        let digest = hash_reader(File::open(archive)?)?.0;
        let key = format!("{}-{}", manifest.version, &digest[..12]);

        let link = packages.join(&id);
        let current = read_selection(&link, &id)?;
        let previous = current.as_ref().map(|record| InstallPrevious {
            key: record.key.clone(),
            id: record.id.clone(),
            version: record.version.clone(),
            repository: record.repository.clone(),
        });
        let record = InstallRecord {
            key: key.clone(),
            id: id.clone(),
            version: manifest.version.clone(),
            repository: repository.to_owned(),
            previous,
        };
        let data = serde_json::to_string_pretty(&record)? + "\n";
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(staging.path().join("store-install.json"))?;
        output.write_all(data.as_bytes())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            output.set_permissions(fs::Permissions::from_mode(0o600))?;
        }
        output.sync_all()?;
        let mut files = extracted.files;
        let bytes = extracted.bytes + data.len() as u64;
        files.insert(
            "store-install.json".to_owned(),
            FileRecord {
                sha256: hash_reader(data.as_bytes())?.0,
                size: data.len() as u64,
                executable: false,
            },
        );

        let target = release_root.join(&key);
        if target.exists() {
            // A verified cached release can be reused after a rollback.
            let cached: InstallRecord =
                serde_json::from_reader(File::open(target.join("store-install.json"))?)?;
            if cached.key != key || cached.id != id || cached.repository != repository {
                return Err(error("Invalid cached extension release"));
            }
        } else {
            sync_tree(staging.path())?;
            // The temporary directory moves as a whole; its Drop then no-ops.
            fs::rename(staging.path(), &target)?;
            sync_directory(&release_root)?;
        }
        select_release(&packages, &id, &target)?;

        let entry = ExtensionEntry {
            id: id.clone(),
            version: manifest.version.clone(),
            key,
            repository: repository.to_owned(),
            bytes,
            files,
        };
        let mut registry = self.registry.clone();
        registry.extensions.insert(id, entry.clone());
        self.save(&mut registry)?;
        self.registry = registry;
        Ok(entry)
    }

    pub(crate) fn remove_devos_extension(&mut self, id: &str, purge: bool) -> Result<()> {
        let entry = self.extension(id)?.clone();
        let store = self.devos.join("extension-store");
        let packages = self.devos.join("extensions");
        if !packages.join(id).exists() && !store.join("releases").join(id).exists() {
            return Err(error("Dev OS extension directories are missing"));
        }
        let _lock = StoreLock::acquire(store.join("install.lock"))?;
        let link = packages.join(id);
        if let Ok(metadata) = fs::symlink_metadata(&link) {
            if !metadata.file_type().is_symlink() {
                return Err(error(
                    "Cannot remove an unmanaged local extension directory",
                ));
            }
            fs::remove_file(&link)?;
        }
        if purge {
            let directory = store.join("releases").join(id);
            if fs::symlink_metadata(&directory).is_ok() {
                check_plain_directory(&directory)?;
                fs::remove_dir_all(&directory)?;
            }
        }
        let mut registry = self.registry.clone();
        registry.extensions.remove(&entry.id);
        self.save(&mut registry)?;
        self.registry = registry;
        Ok(())
    }

    pub(crate) fn rollback_devos_extension(&mut self, id: &str) -> Result<String> {
        let entry = self.extension(id)?.clone();
        let store = self.devos.join("extension-store");
        let packages = self.devos.join("extensions");
        let _lock = StoreLock::acquire(store.join("install.lock"))?;
        let link = packages.join(id);
        let current =
            read_selection(&link, id)?.ok_or_else(|| error("Extension history is missing"))?;
        let previous = current
            .previous
            .ok_or_else(|| error(format!("No previous version saved for {id}")))?;
        let target = store.join("releases").join(id).join(&previous.key);
        check_plain_directory(&target)?;
        let manifest = read_manifest(&target.join(MANIFEST_NAME))?;
        if manifest.id != entry.id || manifest.version != previous.version {
            return Err(error(
                "Previous extension release does not match its history",
            ));
        }
        select_release(&packages, id, &target)?;
        let files = hash_tree(&target)?;
        let bytes = files.values().map(|record| record.size).sum();
        let mut registry = self.registry.clone();
        registry.extensions.insert(
            entry.id.clone(),
            ExtensionEntry {
                id: entry.id.clone(),
                version: previous.version.clone(),
                key: previous.key.clone(),
                repository: previous.repository.clone(),
                bytes,
                files,
            },
        );
        self.save(&mut registry)?;
        self.registry = registry;
        Ok(previous.version)
    }

    pub(crate) fn verify_devos_extension(&self, entry: &ExtensionEntry) -> Result<()> {
        let root = self.extension_path(entry);
        check_plain_directory(&root)?;
        let actual = hash_tree(&root)?;
        let intact = actual.len() == entry.files.len()
            && actual.iter().all(|(path, record)| {
                entry
                    .files
                    .get(path)
                    .is_some_and(|expected| record_matches(expected, record))
            });
        if !intact {
            return Err(error(format!(
                "Integrity check failed: {} / Dev OS extension",
                entry.id
            )));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
