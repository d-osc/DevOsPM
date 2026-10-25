use flate2::{read::MultiGzDecoder, write::GzEncoder, Compression};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    env,
    fs::{self, File, OpenOptions},
    io::{self, BufReader, Read, Write},
    path::{Path, PathBuf},
    process::Command,
};
use tempfile::{Builder, NamedTempFile};

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
const MAX_FILES: usize = 100_000;
const MAX_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const MAX_MANIFEST: u64 = 1024 * 1024;
// Schema 1 had no "extensions" section; it upgrades transparently on save.
const REGISTRY_SCHEMA: u32 = 2;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    #[serde(default)]
    pub name: String,
    #[serde(default, alias = "shotname")]
    pub shortname: String,
    #[serde(default)]
    pub fullname: String,
    #[serde(default)]
    pub homepage: String,
    #[serde(default)]
    pub storepage: StorePage,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub kind: PackageKind,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub icon: String,
    #[serde(default)]
    pub repository: crate::metadata::Repository,
    #[serde(default)]
    pub release: String,
    #[serde(default, alias = "commands", deserialize_with = "deserialize_bin")]
    pub bin: BTreeMap<String, String>,
    #[serde(default)]
    pub contextmenu: Vec<ContextMenuItem>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(untagged)]
pub enum StorePage {
    Reference(String),
    Files(StorePageFiles),
}
impl Default for StorePage {
    fn default() -> Self {
        Self::Reference(String::new())
    }
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct StorePageFiles {
    pub index: String,
    pub files: Vec<String>,
}
impl StorePage {
    fn local_files(&self) -> Vec<&str> {
        match self {
            Self::Reference(path)
                if path.is_empty()
                    || path.starts_with("https://")
                    || path.starts_with("http://") =>
            {
                Vec::new()
            }
            Self::Reference(path) => vec![path.as_str()],
            Self::Files(page) => page.files.iter().map(String::as_str).collect(),
        }
    }
    fn validate(&self) -> Result<()> {
        let index = match self {
            Self::Reference(path)
                if path.is_empty()
                    || path.starts_with("https://")
                    || path.starts_with("http://") =>
            {
                return Ok(())
            }
            Self::Reference(path) => path,
            Self::Files(page) => {
                if !page.files.contains(&page.index) {
                    return Err(error("storepage.files must include index"));
                }
                &page.index
            }
        };
        let extension = Path::new(index)
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if !matches!(extension.as_str(), "html" | "htm" | "md") {
            return Err(error("storepage.index must be an HTML or Markdown file"));
        }
        let mut seen = BTreeSet::new();
        for path in self.local_files() {
            if path.is_empty()
                || safe_path(path)? != path
                || !seen.insert(path.to_ascii_lowercase())
            {
                return Err(error(
                    "storepage files must be unique relative paths inside the package",
                ));
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ContextMenuItem {
    pub id: String,
    pub label: String,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    pub targets: Vec<ContextMenuTarget>,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ContextMenuTarget {
    File,
    Directory,
    Background,
}

fn deserialize_bin<'de, D>(
    deserializer: D,
) -> std::result::Result<BTreeMap<String, String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Bin {
        Path(String),
        Commands(BTreeMap<String, String>),
    }
    match Bin::deserialize(deserializer)? {
        Bin::Path(path) => Ok(BTreeMap::from([(String::new(), path)])),
        Bin::Commands(map) => {
            if map.contains_key("") {
                return Err(serde::de::Error::custom("bin command name cannot be empty"));
            }
            Ok(map)
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct FileRecord {
    pub sha256: String,
    pub size: u64,
    pub executable: bool,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PackageKind {
    #[default]
    App,
    Extension,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Snapshot {
    pub manifest: Manifest,
    #[serde(default)]
    pub kind: PackageKind,
    pub directory: String,
    pub bytes: u64,
    pub files: BTreeMap<String, FileRecord>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Package {
    pub active: Snapshot,
    pub previous: Option<Snapshot>,
}

#[derive(Clone, Deserialize, Serialize)]
pub(crate) struct Registry {
    pub(crate) schema: u32,
    #[serde(default)]
    pub(crate) packages: BTreeMap<String, Package>,
    #[serde(default)]
    pub(crate) extensions: BTreeMap<String, crate::devos::ExtensionEntry>,
}

// What dev install produced: a store package or a Dev OS desktop extension.
// Both arms are boxed so the enum stays small while it travels to the CLI.
#[derive(Clone, Debug)]
pub enum InstallOutcome {
    Package(Box<Snapshot>),
    Extension(Box<crate::devos::ExtensionEntry>),
}

pub struct InstallOptions {
    pub name: Option<String>,
    pub version: Option<String>,
    pub sha256: Option<String>,
    pub replace: bool,
    pub no_desktop: bool,
    // "owner/repo" when the archive came from a GitHub URL; recorded for
    // Dev OS extension install history so the desktop store stays usable.
    pub source_repository: Option<String>,
}

pub struct Manager {
    pub(crate) root: PathBuf,
    pub(crate) registry: Registry,
    // The lock stays held for the whole operation, including dev run.
    _lock: File,
    writable: bool,
    pub(crate) devos: PathBuf,
    pub(crate) applications: PathBuf,
}

pub(crate) fn error(message: impl Into<String>) -> Box<dyn std::error::Error> {
    io::Error::other(message.into()).into()
}

// .dpk is the primary extension, .dpkx marks Dev OS extension packages, and
// .tar.gz stays supported; all three wrap the same checked tar+gzip reader.
// .deb and .rpm are unwrapped to a tar stream by the native module first.
fn package_stem(path: &Path) -> Option<&str> {
    let name = path.file_name()?.to_str()?;
    name.strip_suffix(".dpk")
        .or_else(|| name.strip_suffix(".dpkx"))
        .or_else(|| name.strip_suffix(".tar.gz"))
        .or_else(|| name.strip_suffix(".deb"))
        .or_else(|| name.strip_suffix(".rpm"))
}

pub(crate) fn identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value.as_bytes()[0].is_ascii_alphanumeric()
        && value
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
}

// Validate archive paths explicitly; never delegate extraction to a shell.
pub(crate) fn safe_path(value: &str) -> Result<String> {
    if value.is_empty()
        || value.len() > 1024
        || value.starts_with('/')
        || value.contains('\\')
        || value.contains(':')
        || value.chars().any(char::is_control)
    {
        return Err(error(format!("Unsafe package path: {value:?}")));
    }
    let mut parts = Vec::new();
    for part in value.split('/') {
        if part == "." || part.is_empty() {
            continue;
        }
        if part == ".." || part.ends_with('.') || part.ends_with(' ') {
            return Err(error(format!("Unsafe package path: {value:?}")));
        }
        let base = part.split('.').next().unwrap_or("").to_ascii_uppercase();
        if matches!(base.as_str(), "CON" | "PRN" | "AUX" | "NUL")
            || (base.len() == 4
                && (base.starts_with("COM") || base.starts_with("LPT"))
                && matches!(base.as_bytes()[3], b'1'..=b'9'))
        {
            return Err(error(format!("Reserved package path: {value:?}")));
        }
        parts.push(part);
    }
    if parts.len() > 64 {
        return Err(error("Package path is too deep"));
    }
    Ok(parts.join("/"))
}

fn check_path_case(path: &str, known: &mut BTreeMap<String, String>) -> Result<()> {
    let mut prefix = String::new();
    for part in path.split('/') {
        if !prefix.is_empty() {
            prefix.push('/');
        }
        prefix.push_str(part);
        let folded = prefix.to_ascii_lowercase();
        if let Some(previous) = known.get(&folded) {
            if previous != &prefix {
                return Err(error(format!(
                    "Conflicting path case: {previous} and {prefix}"
                )));
            }
        } else {
            known.insert(folded, prefix.clone());
        }
    }
    Ok(())
}

pub(crate) struct Extracted {
    pub files: BTreeMap<String, FileRecord>,
    pub bytes: u64,
    // Header metadata when the archive was a .deb or .rpm.
    pub native: Option<crate::native::Meta>,
}

// Entries recorded relative to a stripped root keep the root prefix out of the
// stored paths; a directory entry for the root itself yields an empty path.
fn strip_root(normalized: &str, prefix: &str, is_dir: bool) -> Result<String> {
    if let Some(rest) = normalized.strip_prefix(prefix) {
        return Ok(rest.to_owned());
    }
    if is_dir && normalized == prefix.trim_end_matches('/') {
        return Ok(String::new());
    }
    Err(error(format!(
        "Entry outside the {prefix} package root: {normalized}"
    )))
}

// Shared hardened extraction for every archive dev installs: raw tar entries,
// explicit GNU/PAX handling, path validation, and streaming SHA-256.
pub(crate) fn extract_archive(
    archive: &Path,
    destination: &Path,
    strip: Option<&str>,
    manifest_names: &[&str],
    max_files: usize,
    max_bytes: u64,
) -> Result<Extracted> {
    let max_stream = max_bytes + 256 * 1024 * 1024;
    let (source, mut native): (Box<dyn Read>, Option<crate::native::Meta>) =
        match crate::native::Kind::of(archive) {
            None => (
                Box::new(MultiGzDecoder::new(BufReader::with_capacity(
                    128 * 1024,
                    File::open(archive)?,
                ))),
                None,
            ),
            Some(kind) => {
                let scratch = destination.parent().unwrap_or(destination);
                let opened = crate::native::open(archive, kind, scratch, max_bytes, max_files)?;
                (opened.reader, Some(opened.meta))
            }
        };
    let decoder = source.take(max_stream);
    let mut tar = tar::Archive::new(decoder);
    let mut files = BTreeMap::new();
    let mut seen = BTreeSet::new();
    let mut path_case = BTreeMap::new();
    let mut bytes = 0u64;
    let mut pending_name = None;
    let mut pending_size = None;
    let mut pending_extensions = false;
    // Read extension entries explicitly so hostile GNU/PAX metadata cannot
    // make the tar library allocate an unbounded buffer before yielding a file.
    for (index, entry) in tar.entries()?.raw(true).enumerate() {
        if index >= max_files {
            return Err(error("Archive has too many entries"));
        }
        let mut entry = entry?;
        let kind = entry.header().entry_type();
        if kind.is_gnu_longname() || kind.is_pax_local_extensions() {
            if entry.size() > MAX_MANIFEST {
                return Err(error("Tar metadata exceeds 1 MiB"));
            }
            pending_extensions = true;
            if kind.is_gnu_longname() {
                if entry.size() > 1025 || pending_name.is_some() {
                    return Err(error("Invalid GNU long-name metadata"));
                }
                let mut name = Vec::new();
                entry.read_to_end(&mut name)?;
                if name.last() == Some(&0) {
                    name.pop();
                }
                pending_name = Some(
                    String::from_utf8(name).map_err(|_| error("Package paths must be UTF-8"))?,
                );
            } else if let Some(extensions) = entry.pax_extensions()? {
                for extension in extensions {
                    let extension = extension?;
                    let key = extension.key()?;
                    match key {
                        "path" => {
                            if pending_name.is_some() {
                                return Err(error("Conflicting tar path metadata"));
                            }
                            pending_name = Some(extension.value()?.to_owned());
                        }
                        "size" => {
                            if pending_size.is_some() {
                                return Err(error("Conflicting tar size metadata"));
                            }
                            pending_size = Some(extension.value()?.parse::<u64>()?);
                        }
                        "linkpath" => return Err(error("Tar link metadata is unsupported")),
                        key if key.starts_with("GNU.sparse") => {
                            return Err(error("Sparse tar files are unsupported"))
                        }
                        _ => (),
                    }
                }
            }
            continue;
        }
        if pending_size.take().is_some_and(|size| size != entry.size()) {
            return Err(error("PAX size must match the tar header size"));
        }
        pending_extensions = false;
        let original = match pending_name.take() {
            Some(name) => name,
            None => String::from_utf8(entry.path_bytes().into_owned())
                .map_err(|_| error("Package paths must be UTF-8"))?,
        };
        let normalized = safe_path(&original)?;
        if let Some(meta) = native.as_mut() {
            // System packages are full of symlinks; none are recreated.
            if !kind.is_file() && !kind.is_dir() {
                meta.skipped += 1;
                continue;
            }
        }
        if !kind.is_file() && !kind.is_dir() {
            return Err(error(format!(
                "Links and special files are not supported: {original}"
            )));
        }
        if kind.is_dir() && entry.size() != 0 {
            return Err(error("Directory entries must not contain data"));
        }
        let path = match strip {
            Some(prefix) => strip_root(&normalized, prefix, kind.is_dir())?,
            None => normalized,
        };
        if path.is_empty() {
            if kind.is_dir() {
                continue;
            }
            return Err(error("Empty file path in archive"));
        }
        if !seen.insert(path.to_ascii_lowercase()) {
            return Err(error(format!("Duplicate archive path: {path}")));
        }
        check_path_case(&path, &mut path_case)?;
        let target = destination.join(&path);
        if kind.is_dir() {
            fs::create_dir_all(&target)?;
            continue;
        }
        let size = entry.size();
        bytes = bytes
            .checked_add(size)
            .ok_or_else(|| error("Archive is too large"))?;
        if bytes > max_bytes {
            let limit = if max_bytes == MAX_BYTES {
                "2 GiB".to_owned()
            } else if max_bytes == crate::devos::MAX_BYTES {
                "128 MiB".to_owned()
            } else {
                format!("{max_bytes} bytes")
            };
            return Err(error(format!(
                "Archive exceeds the {limit} extracted-size limit"
            )));
        }
        if manifest_names.contains(&path.as_str()) && size > MAX_MANIFEST {
            return Err(error("Package manifest exceeds 1 MiB"));
        }
        fs::create_dir_all(
            target
                .parent()
                .ok_or_else(|| error("Invalid destination"))?,
        )?;
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&target)?;
        let (digest, written) = copy_hashed(&mut entry, &mut output)?;
        if written != size {
            return Err(error(format!("Truncated package file: {path}")));
        }
        let executable = entry.header().mode()? & 0o111 != 0;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            output.set_permissions(fs::Permissions::from_mode(if executable {
                0o755
            } else {
                0o644
            }))?;
        }
        output.sync_all()?;
        files.insert(
            path,
            FileRecord {
                sha256: digest,
                size,
                executable,
            },
        );
    }
    if pending_extensions {
        return Err(error("Dangling tar metadata without a file entry"));
    }
    // tar stops at its end marker; finish reading gzip to verify its CRC/trailer.
    let mut decoder = tar.into_inner();
    let mut tail = [0u8; 32 * 1024];
    loop {
        let count = decoder.read(&mut tail)?;
        if count == 0 {
            break;
        }
        if tail[..count].iter().any(|&b| b != 0) {
            return Err(error("Unexpected data after tar end marker"));
        }
    }
    if decoder.limit() == 0 {
        return Err(error("Archive exceeds the decompression limit"));
    }
    if files.is_empty() {
        return Err(error("Archive contains no files"));
    }
    Ok(Extracted {
        files,
        bytes,
        native,
    })
}

impl Manifest {
    fn normalize_bin(&mut self) {
        if let Some(path) = self.bin.remove("") {
            self.bin.insert(self.name.clone(), path);
        }
    }
    fn validate(&self) -> Result<()> {
        self.resolved_release()?;
        self.storepage.validate()?;
        if !self.icon.is_empty() && safe_path(&self.icon)? != self.icon {
            return Err(error(
                "icon must be a relative file path inside the package",
            ));
        }
        if !identifier(&self.name) {
            return Err(error(
                "Package name must use lowercase letters, numbers, - or _ (max 64 characters)",
            ));
        }
        if self.version.is_empty()
            || self.version.len() > 128
            || !self
                .version
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._+-".contains(&b))
        {
            return Err(error("Invalid package version"));
        }
        for (command, path) in &self.bin {
            if !identifier(command) || safe_path(path)? != *path || path.is_empty() {
                return Err(error(format!(
                    "Invalid command mapping: {command} -> {path}"
                )));
            }
        }
        let mut menu_ids = BTreeSet::new();
        for item in &self.contextmenu {
            if !identifier(&item.id) || !menu_ids.insert(&item.id) {
                return Err(error(
                    "contextmenu IDs must be valid and unique within the package",
                ));
            }
            if item.label.trim().is_empty()
                || item.label.len() > 512
                || item.label.chars().any(char::is_control)
            {
                return Err(error(
                    "contextmenu label must be nonempty text (max 512 bytes)",
                ));
            }
            if !self.bin.contains_key(&item.command) {
                return Err(error(format!(
                    "contextmenu command is missing from bin: {}",
                    item.command
                )));
            }
            if item.targets.is_empty()
                || item.targets.iter().collect::<BTreeSet<_>>().len() != item.targets.len()
            {
                return Err(error("contextmenu targets must be nonempty and unique"));
            }
            if item.args.iter().any(|arg| arg.contains('\0')) {
                return Err(error("contextmenu arguments cannot contain NUL"));
            }
        }
        Ok(())
    }
}

pub(crate) fn check_plain_directory(path: &Path) -> Result<()> {
    let meta = fs::symlink_metadata(path)?;
    if !meta.is_dir() || meta.file_type().is_symlink() {
        return Err(error(format!(
            "Expected a real directory: {}",
            path.display()
        )));
    }
    Ok(())
}

pub(crate) fn sync_directory(path: &Path) -> Result<()> {
    #[cfg(unix)]
    File::open(path)?.sync_all()?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

pub(crate) fn sync_tree(path: &Path) -> Result<()> {
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            sync_tree(&entry.path())?;
        }
    }
    sync_directory(path)
}

fn default_root() -> Result<PathBuf> {
    if let Some(root) = env::var_os("DEV_ROOT") {
        return Ok(root.into());
    }
    if let Some(data) = env::var_os("XDG_DATA_HOME") {
        return Ok(PathBuf::from(data).join("devos"));
    }
    env::var_os("HOME")
        .or_else(|| env::var_os("USERPROFILE"))
        .map(|home| PathBuf::from(home).join(".local/share/devos"))
        .ok_or_else(|| error("Cannot find home directory; use --root or DEV_ROOT"))
}

// Symlink targets and launcher icon paths must stay absolute even when the
// caller passes a relative override.
fn absolutize(path: PathBuf) -> Result<PathBuf> {
    if path.is_absolute() {
        Ok(path)
    } else {
        Ok(env::current_dir()?.join(path))
    }
}

fn data_home() -> Result<PathBuf> {
    if let Some(data) = env::var_os("XDG_DATA_HOME").filter(|value| !value.is_empty()) {
        return Ok(data.into());
    }
    env::var_os("HOME")
        .or_else(|| env::var_os("USERPROFILE"))
        .map(|home| PathBuf::from(home).join(".local/share"))
        .ok_or_else(|| error("Cannot find home directory"))
}

// Dev OS desktop data: extension-store/ and extensions/ live here.
pub fn default_devos_data() -> Result<PathBuf> {
    Ok(data_home()?.join("dev-os"))
}

// XDG applications directory scanned by desktop launchers such as Dev OS.
pub fn default_applications() -> Result<PathBuf> {
    Ok(data_home()?.join("applications"))
}

impl Manager {
    pub fn open_with(
        root: Option<PathBuf>,
        devos_data: Option<PathBuf>,
        applications: Option<PathBuf>,
        writable: bool,
    ) -> Result<Self> {
        let root = match root {
            Some(root) => root,
            None => default_root()?,
        };
        let devos = match devos_data {
            Some(path) => absolutize(path)?,
            None => default_devos_data()?,
        };
        let applications = match applications {
            Some(path) => absolutize(path)?,
            None => default_applications()?,
        };
        fs::create_dir_all(&root)?;
        check_plain_directory(&root)?;
        let root = fs::canonicalize(root)?;
        // A private store protects against other users swapping files during extraction.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&root, fs::Permissions::from_mode(0o700))?;
        }
        let lock_path = root.join(".lock");
        if fs::symlink_metadata(&lock_path)
            .is_ok_and(|m| !m.is_file() || m.file_type().is_symlink())
        {
            return Err(error("Invalid package-store lock file"));
        }
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(lock_path)?;
        let locking = if writable {
            fs2::FileExt::try_lock_exclusive(&lock)
        } else {
            fs2::FileExt::try_lock_shared(&lock)
        };
        locking.map_err(|e| error(format!("Package store is busy or cannot be locked: {e}. Retry after the other dev command finishes.")))?;
        for directory in ["packages", "staging"] {
            let path = root.join(directory);
            fs::create_dir_all(&path)?;
            check_plain_directory(&path)?;
        }
        let db = root.join("registry.json");
        let registry = match fs::symlink_metadata(&db) {
            Ok(meta) => {
                if !meta.is_file()
                    || meta.file_type().is_symlink()
                    || meta.len() > 128 * 1024 * 1024
                {
                    return Err(error("Invalid package registry file"));
                }
                let registry: Registry = serde_json::from_reader(BufReader::new(File::open(&db)?))
                    .map_err(|e| {
                        error(format!(
                            "Registry is corrupt; preserve it for recovery: {e}"
                        ))
                    })?;
                if !matches!(registry.schema, 1 | REGISTRY_SCHEMA) {
                    return Err(error("Unsupported registry schema"));
                }
                let mut generations = BTreeSet::new();
                let mut commands = BTreeSet::new();
                for (name, package) in &registry.packages {
                    for snapshot in std::iter::once(&package.active).chain(package.previous.iter())
                    {
                        snapshot.validate()?;
                        if snapshot.manifest.name != *name {
                            return Err(error("Registry package name mismatch"));
                        }
                        if !generations.insert(&snapshot.directory) {
                            return Err(error("Duplicate generation in registry"));
                        }
                    }
                    for command in package.active.manifest.bin.keys() {
                        if !commands.insert(command) {
                            return Err(error("Duplicate active command in registry"));
                        }
                    }
                }
                for (id, entry) in &registry.extensions {
                    entry
                        .validate()
                        .map_err(|e| error(format!("Invalid extension record for {id}: {e}")))?;
                    if id != &entry.id {
                        return Err(error("Registry extension ID mismatch"));
                    }
                }
                registry
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => Registry {
                schema: REGISTRY_SCHEMA,
                packages: BTreeMap::new(),
                extensions: BTreeMap::new(),
            },
            Err(e) => return Err(e.into()),
        };
        Ok(Self {
            root,
            registry,
            _lock: lock,
            writable,
            devos,
            applications,
        })
    }

    pub(crate) fn save(&self, registry: &mut Registry) -> Result<()> {
        if !self.writable {
            return Err(error("This operation requires an exclusive store lock"));
        }
        registry.schema = REGISTRY_SCHEMA;
        let mut temp = NamedTempFile::new_in(&self.root)?;
        serde_json::to_writer(&mut temp, registry)?;
        temp.write_all(b"\n")?;
        if temp.as_file().metadata()?.len() > 128 * 1024 * 1024 {
            return Err(error("Registry exceeds the 128 MiB limit"));
        }
        temp.as_file().sync_all()?;
        temp.persist(self.root.join("registry.json"))?;
        // After atomic replacement, the new state is committed. Never delete its payload
        // on a directory-sync failure: that would turn a warning into data loss.
        if let Err(e) = sync_directory(&self.root) {
            eprintln!("dev: warning: registry committed but directory sync failed: {e}");
        }
        Ok(())
    }

    pub fn packages(&self) -> &BTreeMap<String, Package> {
        &self.registry.packages
    }

    pub fn package(&self, name: &str) -> Result<&Package> {
        self.registry
            .packages
            .get(name)
            .ok_or_else(|| error(format!("Package is not installed: {name}")))
    }

    pub fn snapshot_path(&self, snapshot: &Snapshot) -> PathBuf {
        self.root.join("packages").join(&snapshot.directory)
    }

    pub fn install(&mut self, archive: &Path, options: InstallOptions) -> Result<InstallOutcome> {
        if !self.writable {
            return Err(error("Install requires an exclusive store lock"));
        }
        if package_stem(archive).is_none() {
            return Err(error(
                "Package filename must end in .dpk, .dpkx, .tar.gz, .deb or .rpm",
            ));
        }
        if let Some(expected) = &options.sha256 {
            if expected.len() != 64 || !expected.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err(error("--sha256 requires a 64-character hexadecimal digest"));
            }
            let actual = hash_reader(File::open(archive)?)?.0;
            if actual != expected.to_ascii_lowercase() {
                return Err(error("Archive SHA-256 mismatch; nothing installed"));
            }
        }
        let stage = Builder::new()
            .prefix("install-")
            .tempdir_in(self.root.join("staging"))?;
        eprintln!("Extracting and checking package files...");
        let extracted = extract_archive(
            archive,
            stage.path(),
            None,
            &["package.json", "dev-package.json"],
            MAX_FILES,
            MAX_BYTES,
        )?;
        let mut files = extracted.files;
        let mut bytes = extracted.bytes;
        let native = extracted.native;
        if let Some(skipped) = native.as_ref().map(|meta| meta.skipped).filter(|&n| n > 0) {
            eprintln!("dev: warning: skipped {skipped} symlink(s)/special file(s) in the package");
        }

        // Dev OS desktop extension packages keep every file under extension/.
        if files.contains_key("extension/extension.json") {
            if files.contains_key("package.json") || files.contains_key("dev-package.json") {
                return Err(error(
                    "Archive mixes a Dev OS extension with package.json manifests",
                ));
            }
            if !files.keys().all(|path| path.starts_with("extension/")) {
                return Err(error(
                    "Dev OS extension archives must keep every file under extension/",
                ));
            }
            if options.name.is_some() || options.version.is_some() {
                return Err(error(
                    "Dev OS extensions carry their own metadata in extension.json",
                ));
            }
            if fs::metadata(archive)?.len() > crate::devos::MAX_TARBALL {
                return Err(error("Dev OS extension archives must not exceed 32 MiB"));
            }
            let manifest =
                crate::devos::read_manifest(&stage.path().join("extension/extension.json"))?;
            manifest.validate()?;
            if self.registry.extensions.contains_key(&manifest.id) && !options.replace {
                return Err(error(format!(
                    "{} is already installed; use --replace to upgrade",
                    manifest.id
                )));
            }
            let repository = options
                .source_repository
                .clone()
                .unwrap_or_else(|| "local/cli".into());
            return self
                .install_devos_extension(archive, &manifest, &repository)
                .map(|entry| InstallOutcome::Extension(Box::new(entry)));
        }
        let has_package = files.contains_key("package.json");
        let has_legacy = files.contains_key("dev-package.json");
        let mut manifest = if has_package {
            let path = stage.path().join("package.json");
            serde_json::from_reader::<_, Manifest>(File::open(path)?)?
        } else if has_legacy {
            serde_json::from_reader::<_, Manifest>(File::open(
                stage.path().join("dev-package.json"),
            )?)?
        } else {
            Manifest {
                name: String::new(),
                shortname: String::new(),
                fullname: String::new(),
                homepage: String::new(),
                storepage: Default::default(),
                version: String::new(),
                kind: PackageKind::App,
                description: String::new(),
                icon: String::new(),
                repository: Default::default(),
                release: String::new(),
                bin: BTreeMap::new(),
                contextmenu: Vec::new(),
            }
        };
        if (!manifest.name.is_empty() && options.name.is_some())
            || (!manifest.version.is_empty() && options.version.is_some())
        {
            return Err(error(
                "Use package metadata; --name/--version are for missing metadata",
            ));
        }
        if let (Some(meta), false) = (&native, has_package || has_legacy) {
            if manifest.description.is_empty() {
                manifest.description = meta.description.clone();
            }
            if manifest.bin.is_empty() {
                manifest.bin = crate::native::detect_commands(&files);
            }
        }
        let meta_name = native
            .as_ref()
            .map(|meta| meta.name.clone())
            .filter(|name| !name.is_empty());
        let meta_version = native
            .as_ref()
            .map(|meta| meta.version.clone())
            .filter(|version| !version.is_empty());
        if manifest.name.is_empty() {
            manifest.name = options
                .name
                .or(meta_name)
                .unwrap_or_else(|| package_stem(archive).unwrap().to_owned());
        }
        if manifest.version.is_empty() {
            manifest.version = options
                .version
                .or(meta_version)
                .unwrap_or_else(|| "0.0.0".into());
        }
        if !has_package && !has_legacy {
            manifest.bin = crate::native::detect_commands(&files);
            // Native GUI packages often put their executable under opt/ rather
            // than a bin directory. Resolve only paths present in this payload.
            for path in files.keys().filter(|path| path.ends_with(".desktop")) {
                if files[path].size > MAX_MANIFEST {
                    continue;
                }
                let Ok(content) = fs::read_to_string(stage.path().join(path)) else {
                    continue;
                };
                let mut desktop_group = false;
                for line in content.lines().map(str::trim) {
                    if line.starts_with('[') {
                        desktop_group = line == "[Desktop Entry]";
                    }
                    if !desktop_group {
                        continue;
                    }
                    if let Some(exec) = line.strip_prefix("Exec=") {
                        let executable = if let Some(quoted) = exec.strip_prefix('"') {
                            quoted.split('"').next().unwrap_or("")
                        } else {
                            exec.split_whitespace().next().unwrap_or("")
                        };
                        let relative = executable.trim_start_matches('/');
                        if files.get(relative).is_some_and(|record| record.executable) {
                            if let Some(command) =
                                relative.rsplit('/').next().filter(|name| identifier(name))
                            {
                                manifest
                                    .bin
                                    .entry(command.into())
                                    .or_insert_with(|| relative.into());
                            }
                        }
                    }
                    if let Some(name) = line.strip_prefix("Name=") {
                        if manifest.fullname.is_empty() {
                            manifest.fullname = name.into();
                        }
                    }
                    if let Some(icon) = line.strip_prefix("Icon=") {
                        let relative = icon.trim_start_matches('/');
                        if files.contains_key(relative) {
                            manifest.icon = relative.into();
                        } else if let Some(path) = files.keys().find(|path| {
                            Path::new(path).file_stem().and_then(|s| s.to_str()) == Some(icon)
                                && matches!(
                                    Path::new(path).extension().and_then(|s| s.to_str()),
                                    Some("png" | "svg" | "xpm")
                                )
                        }) {
                            manifest.icon = path.clone();
                        }
                    }
                }
            }
            // Portable archives commonly keep the main executable at their root.
            if manifest.bin.is_empty() {
                for (path, record) in &files {
                    if record.executable && !path.contains('/') && identifier(path) {
                        manifest.bin.insert(path.clone(), path.clone());
                    }
                }
            }
            if manifest.icon.is_empty() {
                manifest.icon = files
                    .keys()
                    .find(|path| {
                        let file = Path::new(path);
                        file.file_stem().and_then(|s| s.to_str()) == Some(manifest.name.as_str())
                            && matches!(
                                file.extension().and_then(|s| s.to_str()),
                                Some("png" | "svg" | "xpm")
                            )
                    })
                    .cloned()
                    .unwrap_or_default();
            }
        }
        manifest.normalize_bin();
        let kind = manifest.kind.clone();
        if !has_package {
            // Generated package.json must not collide with an existing entry by case.
            if files
                .keys()
                .any(|path| path.eq_ignore_ascii_case("package.json") && path != "package.json")
            {
                return Err(error("Conflicting path case for package.json"));
            }
            let data = serde_json::to_vec_pretty(&manifest)?;
            if files.len() >= MAX_FILES || bytes + data.len() as u64 > MAX_BYTES {
                return Err(error(
                    "No space within package limits for generated package.json",
                ));
            }
            let mut output = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(stage.path().join("package.json"))?;
            let (digest, size) = copy_hashed(&mut &data[..], &mut output)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                output.set_permissions(fs::Permissions::from_mode(0o644))?;
            }
            output.sync_all()?;
            bytes += size;
            files.insert(
                "package.json".into(),
                FileRecord {
                    sha256: digest,
                    size,
                    executable: false,
                },
            );
        }
        manifest.validate()?;
        validate_commands(&manifest, &files)?;
        let existing = self.registry.packages.get(&manifest.name).cloned();
        if existing.is_some() && !options.replace {
            return Err(error(format!(
                "{} is already installed; use --replace to upgrade",
                manifest.name
            )));
        }
        for (name, package) in &self.registry.packages {
            if *name != manifest.name {
                for command in manifest.bin.keys() {
                    if package.active.manifest.bin.contains_key(command) {
                        return Err(error(format!(
                            "Command {command} is already exported by {name}"
                        )));
                    }
                }
            }
        }
        eprintln!(
            "Saving {} {} ({} files)...",
            manifest.name,
            manifest.version,
            files.len()
        );
        collect_files(stage.path())?;
        sync_tree(stage.path())?;
        let directory = format!(
            "{}-{}",
            manifest.name,
            stage.path().file_name().unwrap().to_string_lossy()
        );
        let snapshot = Snapshot {
            manifest,
            kind,
            directory,
            bytes,
            files,
        };
        let destination = self.snapshot_path(&snapshot);
        // Keep the old snapshot intact until the registry atomically points to the new one.
        fs::rename(stage.path(), &destination)?;
        if let Err(e) = sync_directory(&self.root.join("packages")) {
            self.cleanup(&snapshot);
            return Err(e);
        }
        let mut registry = self.registry.clone();
        registry.packages.insert(
            snapshot.manifest.name.clone(),
            Package {
                active: snapshot.clone(),
                previous: existing.as_ref().map(|p| p.active.clone()),
            },
        );
        if let Err(e) = self.save(&mut registry) {
            self.cleanup(&snapshot);
            return Err(e);
        }
        self.registry = registry;
        if !options.no_desktop && matches!(snapshot.kind, PackageKind::App) {
            if snapshot.manifest.bin.is_empty() {
                eprintln!("dev: warning: no launchable command found; Applications entry cannot be created. Add a bin mapping in package.json.");
            } else {
                eprintln!("Registering application launcher entries...");
            }
            let previous = existing.as_ref().map(|package| &package.active.manifest);
            if let Err(e) = crate::desktop::refresh(
                &self.applications,
                previous,
                &snapshot.manifest,
                &self.snapshot_path(&snapshot),
            ) {
                eprintln!("dev: warning: installed, but writing the launcher entry failed: {e}");
            }
        }
        if let Some(old) = existing.and_then(|p| p.previous) {
            self.cleanup(&old);
        }
        Ok(InstallOutcome::Package(Box::new(snapshot)))
    }

    fn cleanup(&self, snapshot: &Snapshot) {
        let path = self.snapshot_path(snapshot);
        // Only remove a single validated generation underneath packages.
        let result = check_plain_directory(&path)
            .and_then(|_| fs::remove_dir_all(&path).map_err(Into::into));
        if let Err(e) = result {
            eprintln!(
                "dev: warning: committed successfully; unused files remain at {}: {e}",
                path.display()
            );
        }
    }

    pub fn remove(&mut self, name: &str) -> Result<()> {
        if !self.registry.packages.contains_key(name) {
            if self.registry.extensions.contains_key(name) {
                return self.remove_devos_extension(name, false);
            }
            return Err(error(format!("Package is not installed: {name}")));
        }
        let package = self.package(name)?.clone();
        let mut registry = self.registry.clone();
        registry.packages.remove(name);
        self.save(&mut registry)?;
        self.registry = registry;
        if let Err(e) = crate::desktop::remove(&self.applications, &package.active.manifest) {
            eprintln!("dev: warning: removed, but launcher entries remain: {e}");
        }
        self.cleanup(&package.active);
        if let Some(previous) = package.previous {
            self.cleanup(&previous);
        }
        Ok(())
    }

    pub fn remove_extension(&mut self, id: &str, purge: bool) -> Result<()> {
        if !self.registry.extensions.contains_key(id) {
            return Err(error(format!("Extension is not installed: {id}")));
        }
        self.remove_devos_extension(id, purge)
    }

    pub fn extensions(&self) -> &BTreeMap<String, crate::devos::ExtensionEntry> {
        &self.registry.extensions
    }

    // Test-facing wrapper that keeps the package-only signature the existing
    // suite was written against.
    #[cfg(test)]
    pub(crate) fn install_package(
        &mut self,
        archive: &Path,
        options: InstallOptions,
    ) -> Result<Snapshot> {
        match self.install(archive, options)? {
            InstallOutcome::Package(snapshot) => Ok(*snapshot),
            InstallOutcome::Extension(_) => Err(error("Expected a package installation")),
        }
    }

    pub fn extension(&self, id: &str) -> Result<&crate::devos::ExtensionEntry> {
        self.registry
            .extensions
            .get(id)
            .ok_or_else(|| error(format!("Extension is not installed: {id}")))
    }

    pub fn extension_path(&self, entry: &crate::devos::ExtensionEntry) -> PathBuf {
        self.devos
            .join("extension-store")
            .join("releases")
            .join(&entry.id)
            .join(&entry.key)
    }

    pub fn rollback(&mut self, name: &str) -> Result<String> {
        if !self.registry.packages.contains_key(name) {
            if self.registry.extensions.contains_key(name) {
                return self.rollback_devos_extension(name);
            }
            return Err(error(format!("Package is not installed: {name}")));
        }
        let package = self.package(name)?.clone();
        let previous = package
            .previous
            .ok_or_else(|| error(format!("No previous version saved for {name}")))?;
        self.verify_snapshot(&previous)?;
        for (other_name, other) in &self.registry.packages {
            if other_name != name {
                for command in previous.manifest.bin.keys() {
                    if other.active.manifest.bin.contains_key(command) {
                        return Err(error(format!(
                            "Cannot roll back: command {command} is now exported by {other_name}"
                        )));
                    }
                }
            }
        }
        let version = previous.manifest.version.clone();
        let mut registry = self.registry.clone();
        registry.packages.insert(
            name.to_owned(),
            Package {
                active: previous.clone(),
                previous: Some(package.active.clone()),
            },
        );
        self.save(&mut registry)?;
        self.registry = registry;
        if let Err(e) = crate::desktop::refresh(
            &self.applications,
            Some(&package.active.manifest),
            &previous.manifest,
            &self.snapshot_path(&previous),
        ) {
            eprintln!("dev: warning: launcher entries may be stale: {e}");
        }
        Ok(version)
    }

    fn verify_snapshot(&self, snapshot: &Snapshot) -> Result<()> {
        let root = self.snapshot_path(snapshot);
        check_plain_directory(&root)?;
        let paths = collect_files(&root)?;
        let actual: BTreeSet<_> = paths.into_iter().collect();
        let expected: BTreeSet<_> = snapshot.files.keys().cloned().collect();
        if actual != expected {
            let missing: Vec<_> = expected.difference(&actual).take(5).collect();
            let unexpected: Vec<_> = actual.difference(&expected).take(5).collect();
            return Err(error(format!(
                "{}: file list changed (missing: {missing:?}, unexpected: {unexpected:?})",
                snapshot.manifest.name
            )));
        }
        for (path, record) in &snapshot.files {
            let full_path = root.join(path);
            let metadata = fs::symlink_metadata(&full_path)?;
            if metadata.len() != record.size
                || hash_reader(File::open(&full_path)?)?.0 != record.sha256
            {
                return Err(error(format!(
                    "Integrity check failed: {} / {path}",
                    snapshot.manifest.name
                )));
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mode = metadata.permissions().mode();
                if (mode & 0o111 != 0) != record.executable || mode & 0o7000 != 0 {
                    return Err(error(format!("Executable permissions changed: {path}")));
                }
            }
        }
        Ok(())
    }

    pub fn verify(&self, name: Option<&str>) -> Result<usize> {
        if let Some(name) = name {
            if let Some(package) = self.registry.packages.get(name) {
                self.verify_snapshot(&package.active)?;
                return Ok(1);
            }
            if let Some(entry) = self.registry.extensions.get(name) {
                self.verify_devos_extension(entry)?;
                return Ok(1);
            }
            return Err(error(format!("Package is not installed: {name}")));
        }
        for package in self.registry.packages.values() {
            self.verify_snapshot(&package.active)?;
        }
        for entry in self.registry.extensions.values() {
            self.verify_devos_extension(entry)?;
        }
        Ok(self.registry.packages.len() + self.registry.extensions.len())
    }

    pub fn run(&self, name: &str, args: &[String]) -> Result<i32> {
        for package in self.registry.packages.values() {
            if let Some(path) = package.active.manifest.bin.get(name) {
                let root = self.snapshot_path(&package.active);
                check_plain_directory(&root)?;
                let mut current = root.clone();
                for component in path.split('/') {
                    current.push(component);
                    if fs::symlink_metadata(&current)?.file_type().is_symlink() {
                        return Err(error("Command path contains a symlink"));
                    }
                }
                // Hash only the executable on launch; full-package validation is dev verify.
                let record = &package.active.files[path];
                if hash_reader(File::open(&current)?)?.0 != record.sha256 {
                    return Err(error(format!(
                        "Command {name} was modified; reinstall the package"
                    )));
                }
                let status = Command::new(current)
                    .args(args)
                    .env("DEV_PACKAGE_ROOT", root)
                    .status()?;
                #[cfg(unix)]
                {
                    use std::os::unix::process::ExitStatusExt;
                    return Ok(status
                        .code()
                        .unwrap_or_else(|| 128 + status.signal().unwrap_or(1)));
                }
                #[cfg(not(unix))]
                return Ok(status.code().unwrap_or(1));
            }
        }
        Err(error(format!(
            "No installed package exports {name}; use dev list or dev info <package>"
        )))
    }
}

impl Snapshot {
    fn validate(&self) -> Result<()> {
        self.manifest.validate()?;
        if self.directory.len() > 128
            || self.directory.contains('/')
            || safe_path(&self.directory)? != self.directory
            || !self
                .directory
                .starts_with(&format!("{}-install-", self.manifest.name))
        {
            return Err(error("Invalid generation directory in registry"));
        }
        let mut bytes = 0u64;
        let mut path_case = BTreeMap::new();
        for (path, record) in &self.files {
            if path.is_empty()
                || safe_path(path)? != *path
                || record.sha256.len() != 64
                || !record.sha256.bytes().all(|b| b.is_ascii_hexdigit())
            {
                return Err(error("Invalid file record in registry"));
            }
            check_path_case(path, &mut path_case)?;
            bytes = bytes
                .checked_add(record.size)
                .ok_or_else(|| error("Invalid registry byte count"))?;
        }
        if bytes != self.bytes || bytes > MAX_BYTES || self.files.len() > MAX_FILES {
            return Err(error("Invalid registry sizes"));
        }
        validate_commands(&self.manifest, &self.files)
    }
}

fn validate_commands(manifest: &Manifest, files: &BTreeMap<String, FileRecord>) -> Result<()> {
    for path in manifest.storepage.local_files() {
        if !files.contains_key(path) {
            return Err(error(format!("Store page file is missing: {path}")));
        }
    }
    if !manifest.icon.is_empty() && !files.contains_key(&manifest.icon) {
        return Err(error(format!("Icon file is missing: {}", manifest.icon)));
    }
    for (command, path) in &manifest.bin {
        let record = files
            .get(path)
            .ok_or_else(|| error(format!("Command {command} points to missing file {path}")))?;
        if !record.executable {
            return Err(error(format!(
                "Command file must be executable in the tar archive: {path}"
            )));
        }
    }
    Ok(())
}

pub(crate) fn copy_hashed(
    reader: &mut impl Read,
    writer: &mut impl Write,
) -> Result<(String, u64)> {
    let mut buffer = [0u8; 128 * 1024];
    let mut digest = Sha256::new();
    let mut count = 0u64;
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        writer.write_all(&buffer[..read])?;
        digest.update(&buffer[..read]);
        count += read as u64;
    }
    Ok((format!("{:x}", digest.finalize()), count))
}

pub(crate) fn hash_reader(mut reader: impl Read) -> Result<(String, u64)> {
    copy_hashed(&mut reader, &mut io::sink())
}

pub(crate) fn collect_files(root: &Path) -> Result<Vec<String>> {
    fn walk(root: &Path, path: &Path, files: &mut Vec<String>, visited: &mut usize) -> Result<()> {
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            *visited += 1;
            if *visited > MAX_FILES {
                return Err(error("Too many package entries"));
            }
            let kind = entry.file_type()?;
            let relative = entry
                .path()
                .strip_prefix(root)?
                .to_str()
                .ok_or_else(|| error("Package paths must be UTF-8"))?
                .replace('\\', "/");
            // Check depth before recursion to bound stack usage.
            safe_path(&relative)?;
            if kind.is_symlink() || (!kind.is_file() && !kind.is_dir()) {
                return Err(error(format!(
                    "Links and special files are unsupported: {relative}"
                )));
            }
            if kind.is_dir() {
                walk(root, &entry.path(), files, visited)?;
            } else {
                files.push(relative);
            }
        }
        Ok(())
    }
    let mut files = Vec::new();
    walk(root, root, &mut files, &mut 0)?;
    files.sort();
    Ok(files)
}

fn read_source_manifest(directory: &Path) -> Result<Manifest> {
    let primary = directory.join("package.json");
    let path = if primary.try_exists()? {
        primary
    } else {
        directory.join("dev-package.json")
    };
    let metadata = fs::symlink_metadata(&path)?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(error("Source manifest must be a regular file"));
    }
    if metadata.len() > MAX_MANIFEST {
        return Err(error("Package manifest exceeds 1 MiB"));
    }
    let mut manifest: Manifest = serde_json::from_reader(File::open(path)?)?;
    manifest.normalize_bin();
    manifest.validate()?;
    Ok(manifest)
}

pub fn default_package_output(directory: &Path) -> Result<PathBuf> {
    check_plain_directory(directory)?;
    let manifest = read_source_manifest(directory)?;
    Ok(PathBuf::from(format!(
        "{}-{}.dpk",
        manifest.name, manifest.version
    )))
}

pub fn pack(directory: &Path, output: &Path) -> Result<()> {
    check_plain_directory(directory)?;
    let directory = fs::canonicalize(directory)?;
    if package_stem(output).is_none() {
        return Err(error("Output filename must end in .dpk, .dpkx or .tar.gz"));
    }
    let parent = output
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let parent = fs::canonicalize(parent)?;
    if parent.starts_with(&directory) {
        return Err(error("Write the archive outside its source directory"));
    }
    let output = parent.join(
        output
            .file_name()
            .ok_or_else(|| error("Invalid output filename"))?,
    );
    if output.exists() {
        return Err(error(
            "Output already exists; choose a new archive filename",
        ));
    }
    let paths = collect_files(&directory)?;
    if !paths
        .iter()
        .any(|p| p == "package.json" || p == "dev-package.json")
    {
        return Err(error("Source directory must contain package.json"));
    }
    let manifest = read_source_manifest(&directory)?;
    for path in manifest.storepage.local_files() {
        if !paths.iter().any(|candidate| candidate == path) {
            return Err(error(format!("Store page file is missing: {path}")));
        }
    }
    if !manifest.icon.is_empty() && !paths.contains(&manifest.icon) {
        return Err(error(format!("Icon file is missing: {}", manifest.icon)));
    }
    for path in manifest.bin.values() {
        if !paths.contains(path) {
            return Err(error(format!("Command file is missing: {path}")));
        }
    }
    let mut temporary = NamedTempFile::new_in(&parent)?;
    {
        let gzip = GzEncoder::new(temporary.as_file_mut(), Compression::fast());
        let mut tar = tar::Builder::new(gzip);
        let mut total = 0u64;
        let mut seen = BTreeSet::new();
        let mut path_case = BTreeMap::new();
        for path in paths {
            if !seen.insert(path.to_ascii_lowercase()) {
                return Err(error(format!("Conflicting package path: {path}")));
            }
            check_path_case(&path, &mut path_case)?;
            let mut file = File::open(directory.join(&path))?;
            let metadata = file.metadata()?;
            total = total
                .checked_add(metadata.len())
                .ok_or_else(|| error("Source package is too large"))?;
            if total > MAX_BYTES {
                return Err(error("Source package exceeds 2 GiB"));
            }
            let mut header = tar::Header::new_gnu();
            header.set_size(metadata.len());
            let command = manifest.bin.values().any(|v| *v == path);
            #[cfg(unix)]
            let executable = {
                use std::os::unix::fs::PermissionsExt;
                command || metadata.permissions().mode() & 0o111 != 0
            };
            #[cfg(not(unix))]
            let executable = command;
            header.set_mode(if executable { 0o755 } else { 0o644 });
            header.set_uid(0);
            header.set_gid(0);
            header.set_mtime(0);
            header.set_cksum();
            tar.append_data(&mut header, &path, &mut file)?;
        }
        tar.into_inner()?.finish()?;
    }
    temporary.as_file().sync_all()?;
    temporary.persist_noclobber(output)?;
    sync_directory(&parent)?;
    Ok(())
}

#[cfg(test)]
mod tests;
