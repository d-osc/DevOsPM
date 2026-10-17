// dev update: check each package's GitHub latest stable release, verify the
// asset digest from the API, then install over the current version.
// dev self-update applies the same flow to the dev binary itself.
use crate::manager::{copy_hashed, error, InstallOptions, InstallOutcome, Manager, Result};
use serde_json::Value;
use std::path::PathBuf;
use std::time::Duration;
use tempfile::NamedTempFile;

pub struct Asset {
    pub name: String,
    pub size: u64,
    pub state: String,
    pub sha256: String,
    pub url: String,
}
pub struct Release {
    pub tag: String,
    pub version: String,
    pub assets: Vec<Asset>,
}

// Overridable for tests and mirrors; the real endpoint is GitHub's API.
fn api_base() -> String {
    std::env::var("DEV_UPDATE_API")
        .ok()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "https://api.github.com".into())
}

// Asset URLs must sit on github.com unless a test mirror serves the API.
fn enforce_github_urls() -> bool {
    api_base() == "https://api.github.com"
}

fn client() -> Result<reqwest::blocking::Client> {
    Ok(reqwest::blocking::Client::builder()
        .connect_timeout(Duration::from_secs(30))
        .timeout(Duration::from_secs(60))
        .user_agent(concat!("dev/", env!("CARGO_PKG_VERSION")))
        .build()?)
}

fn get_json(url: &str, accept: &str) -> Result<Value> {
    let response = client()?
        .get(url)
        .header("Accept", accept)
        .send()
        .map_err(|err| err.without_url())?;
    let status = response.status();
    if status.as_u16() == 404 {
        return Err(error("No public stable release found"));
    }
    if !status.is_success() {
        return Err(error(format!("GitHub returned HTTP {}", status.as_u16())));
    }
    let body = response.text().map_err(|err| err.without_url())?;
    Ok(serde_json::from_str(&body)?)
}

pub fn fetch_latest(repository: &str) -> Result<Release> {
    let value = get_json(
        &format!("{}/repos/{repository}/releases/latest", api_base()),
        "application/vnd.github+json",
    )?;
    parse_release(&value)
}

pub fn parse_release(value: &Value) -> Result<Release> {
    let object = value
        .as_object()
        .ok_or_else(|| error("Invalid release data"))?;
    if object.get("draft").and_then(Value::as_bool) == Some(true)
        || object.get("prerelease").and_then(Value::as_bool) == Some(true)
    {
        return Err(error("This release is not on the stable channel"));
    }
    let tag = object
        .get("tag_name")
        .and_then(Value::as_str)
        .ok_or_else(|| error("Release has no tag"))?
        .to_owned();
    let version = tag.strip_prefix('v').unwrap_or(&tag).to_owned();
    if !crate::devos::stable_version(&version) {
        return Err(error("Release tag must be stable semver like v1.2.0"));
    }
    let mut assets = Vec::new();
    if let Some(list) = object.get("assets").and_then(Value::as_array) {
        for asset in list {
            let Some(name) = asset.get("name").and_then(Value::as_str) else {
                continue;
            };
            let digest = asset.get("digest").and_then(Value::as_str).unwrap_or("");
            assets.push(Asset {
                name: name.to_owned(),
                size: asset.get("size").and_then(Value::as_u64).unwrap_or(0),
                state: asset
                    .get("state")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned(),
                sha256: digest
                    .strip_prefix("sha256:")
                    .unwrap_or("")
                    .to_ascii_lowercase(),
                url: asset
                    .get("browser_download_url")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned(),
            });
        }
    }
    Ok(Release {
        tag,
        version,
        assets,
    })
}

pub fn pick_asset<'a>(release: &'a Release, repository: &str, filename: &str) -> Result<&'a Asset> {
    let asset = release
        .assets
        .iter()
        .find(|asset| asset.name == filename)
        .ok_or_else(|| {
            let found: Vec<&str> = release
                .assets
                .iter()
                .map(|a| a.name.as_str())
                .take(5)
                .collect();
            error(format!(
                "Release has no {filename} asset (found: {found:?})"
            ))
        })?;
    if asset.state != "uploaded" {
        return Err(error("Release asset has not finished uploading"));
    }
    if asset.size == 0 {
        return Err(error("Release asset is empty"));
    }
    if asset.sha256.len() != 64 || !asset.sha256.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(error(
            "This release has no SHA-256 digest. Re-upload its package to GitHub Releases.",
        ));
    }
    if enforce_github_urls() {
        let expected = format!(
            "https://github.com/{repository}/releases/download/{}/{filename}",
            release.tag
        );
        if asset.url != expected {
            return Err(error(
                "Package URL does not belong to the selected GitHub release",
            ));
        }
    } else if let Ok(parsed) = reqwest::Url::parse(&asset.url) {
        if !matches!(parsed.scheme(), "http" | "https")
            || !parsed.username().is_empty()
            || parsed.password().is_some()
        {
            return Err(error("Package URL must be HTTP(S) without credentials"));
        }
    } else {
        return Err(error("Package URL is invalid"));
    }
    Ok(asset)
}

pub fn is_newer(new: &str, current: &str) -> bool {
    match (crate::devos::semver(new), crate::devos::semver(current)) {
        (Some(new), Some(current)) => new > current,
        _ => new != current,
    }
}

pub const UPSTREAM_REPOSITORY: &str = "d-osc/DevOsPM";
const MAX_BINARY: u64 = 64 * 1024 * 1024;

pub fn arch_asset_name(arch: &str) -> Option<&'static str> {
    match arch {
        "x86_64" => Some("dev-linux-x86_64"),
        "aarch64" => Some("dev-linux-arm64"),
        _ => None,
    }
}

// Stream the release asset to a temporary file, hashing as it is written.
fn download_verified(url: &str, expected: &str, destination: &mut NamedTempFile) -> Result<()> {
    let response = client()?
        .get(url)
        .send()
        .map_err(|err| err.without_url())?
        .error_for_status()
        .map_err(|err| err.without_url())?;
    // Response also implements Iterator, so name Read::take explicitly.
    let mut stream = std::io::Read::take(response, MAX_BINARY);
    let (digest, size) = copy_hashed(&mut stream, destination)?;
    if size == 0 {
        return Err(error("Downloaded dev binary is empty"));
    }
    if digest != expected.to_ascii_lowercase() {
        return Err(error(
            "SHA-256 verification failed. The current dev version was kept.",
        ));
    }
    Ok(())
}

// Replace the running dev executable with the newest upstream release. The
// replacement happens only after the downloaded file runs and reports the
// expected version, and the swap itself is one atomic rename.
pub fn self_update() -> Result<i32> {
    let current = env!("CARGO_PKG_VERSION");
    let architecture = std::env::consts::ARCH;
    let filename = arch_asset_name(architecture)
        .ok_or_else(|| error(format!("No release asset for architecture {architecture}")))?;
    let release = fetch_latest(UPSTREAM_REPOSITORY)?;
    let asset = pick_asset(&release, UPSTREAM_REPOSITORY, filename)?;
    if !is_newer(&release.version, current) {
        println!("dev {current} is up to date");
        return Ok(0);
    }
    println!("dev {current} -> {}", release.version);
    let executable = std::env::current_exe()?;
    let directory = executable
        .parent()
        .ok_or_else(|| error("Cannot locate the directory of the dev executable"))?;
    let mut temp = NamedTempFile::new_in(directory)?;
    download_verified(&asset.url, &asset.sha256, &mut temp)?;
    temp.as_file().sync_all()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        temp.as_file()
            .set_permissions(std::fs::Permissions::from_mode(0o755))?;
    }
    // Linux refuses to execute a file that is still open for writing, so
    // close the handle before the verification run and swap by path.
    let (file, staged) = temp
        .keep()
        .map_err(|e| error(format!("Cannot finalize the downloaded binary: {e}")))?;
    drop(file);
    let outcome = verify_and_swap(&staged, &executable, &release.version);
    if outcome.is_err() {
        let _ = std::fs::remove_file(&staged);
    }
    outcome?;
    println!("Updated dev to {}", release.version);
    Ok(0)
}

// The staged binary must run and report the release version before it is
// allowed to replace the current executable; the swap is one atomic rename.
fn verify_and_swap(
    staged: &std::path::Path,
    executable: &std::path::Path,
    version: &str,
) -> Result<()> {
    let check = std::process::Command::new(staged)
        .arg("--version")
        .output()
        .map_err(|e| error(format!("The downloaded dev binary does not run: {e}")))?;
    let reported = String::from_utf8_lossy(&check.stdout).trim().to_owned();
    if !check.status.success() || reported != format!("dev {version}") {
        return Err(error(format!(
            "The downloaded dev binary reported {reported:?} instead of dev {version}"
        )));
    }
    std::fs::rename(staged, executable)
        .map_err(|e| error(format!("Cannot replace {}: {e}", executable.display())))
}

// "owner/repo" out of an https://github.com/owner/repo URL.
pub fn repository_slug(url: &str) -> Option<String> {
    let parsed = reqwest::Url::parse(url).ok()?;
    if parsed.scheme() != "https" || parsed.host_str() != Some("github.com") {
        return None;
    }
    let path = parsed.path().trim_matches('/').trim_end_matches(".git");
    let mut parts = path.split('/');
    let owner = parts.next()?;
    let repository = parts.next()?;
    if parts.next().is_some() || owner.is_empty() || repository.is_empty() {
        return None;
    }
    Some(format!("{owner}/{repository}"))
}

// Expand {name}/{version} release filename templates like metadata.rs does.
fn expand_filename(template: &str, name: &str, version: &str) -> Result<String> {
    let value = template
        .replace("{version}", version)
        .replace("{name}", name);
    if value.contains(['{', '}']) {
        return Err(error(
            "Unknown release filename template; use {version} and {name}",
        ));
    }
    Ok(value)
}

enum Target {
    Package {
        name: String,
        version: String,
        repository: String,
        template: String,
        dpk: bool,
    },
    Extension {
        id: String,
        version: String,
        repository: String,
    },
}

fn collect_targets(manager: &Manager, names: &[String]) -> Result<Vec<Target>> {
    let mut targets = Vec::new();
    for (name, package) in manager.packages() {
        let manifest = &package.active.manifest;
        if let crate::metadata::Repository::Details(details) = &manifest.repository {
            if let Some(release) = &details.release {
                let Some(slug) = repository_slug(&details.url) else {
                    continue;
                };
                targets.push(Target::Package {
                    name: name.clone(),
                    version: manifest.version.clone(),
                    repository: slug,
                    template: release.filename.clone(),
                    dpk: release.kind != "tar.gz",
                });
            }
        }
    }
    for (id, entry) in manager.extensions() {
        if entry.repository != "local/cli" {
            targets.push(Target::Extension {
                id: id.clone(),
                version: entry.version.clone(),
                repository: entry.repository.clone(),
            });
        }
    }
    if names.is_empty() {
        return Ok(targets);
    }
    let mut selected = Vec::new();
    for name in names {
        let found = targets
            .iter()
            .position(|target| match target {
                Target::Package { name: n, .. } => n == name,
                Target::Extension { id, .. } => id == name,
            })
            .ok_or_else(|| error(format!("No updatable package or extension named {name}")))?;
        selected.push(targets.remove(found));
    }
    Ok(selected)
}

pub fn run(
    root: Option<PathBuf>,
    devos_data: Option<PathBuf>,
    applications: Option<PathBuf>,
    names: &[String],
) -> Result<i32> {
    let targets = {
        let manager = Manager::open_with(
            root.clone(),
            devos_data.clone(),
            applications.clone(),
            false,
        )?;
        collect_targets(&manager, names)?
    };
    if targets.is_empty() {
        println!("Nothing to update. Configure repository.release in package.json.");
        return Ok(0);
    }
    let mut failures = 0;
    for target in targets {
        if let Err(e) = update_one(
            &target,
            root.clone(),
            devos_data.clone(),
            applications.clone(),
        ) {
            failures += 1;
            let label = match &target {
                Target::Package { name, .. } => name.clone(),
                Target::Extension { id, .. } => id.clone(),
            };
            eprintln!("dev: {label}: {e}");
        }
    }
    if failures > 0 {
        eprintln!("dev: {failures} update(s) failed");
        return Ok(1);
    }
    Ok(0)
}

fn update_one(
    target: &Target,
    root: Option<PathBuf>,
    devos_data: Option<PathBuf>,
    applications: Option<PathBuf>,
) -> Result<()> {
    match target {
        Target::Package {
            name,
            version,
            repository,
            template,
            dpk,
        } => {
            let release = fetch_latest(repository)?;
            let mut filename = expand_filename(template, name, &release.version)?;
            let suffix = if *dpk { ".dpk" } else { ".tar.gz" };
            if !filename.ends_with(suffix) {
                filename.push_str(suffix);
            }
            let asset = pick_asset(&release, repository, &filename)?;
            if !is_newer(&release.version, version) {
                println!("{name} {version} is up to date");
                return Ok(());
            }
            println!("{name} {version} -> {}", release.version);
            let downloaded = crate::download::resolve(&asset.url)?
                .ok_or_else(|| error("Expected a URL download"))?;
            let mut manager = Manager::open_with(root, devos_data, applications, true)?;
            let outcome = manager.install(
                downloaded.path(),
                InstallOptions {
                    name: None,
                    version: None,
                    sha256: Some(asset.sha256.clone()),
                    replace: true,
                    no_desktop: false,
                    source_repository: Some(repository.clone()),
                },
            )?;
            match outcome {
                InstallOutcome::Package(snapshot) => {
                    println!(
                        "Updated {} {}",
                        snapshot.manifest.name, snapshot.manifest.version
                    )
                }
                InstallOutcome::Extension(_) => {
                    return Err(error("Expected a package archive for this update"));
                }
            }
            Ok(())
        }
        Target::Extension {
            id,
            version,
            repository,
        } => {
            let release = fetch_latest(repository)?;
            let asset = pick_asset(&release, repository, crate::devos::ASSET_NAME)?;
            if !release
                .assets
                .iter()
                .any(|asset| asset.name == crate::devos::MANIFEST_NAME)
            {
                return Err(error("Release is missing its extension.json asset"));
            }
            let manifest_url = release
                .assets
                .iter()
                .find(|asset| asset.name == crate::devos::MANIFEST_NAME)
                .map(|asset| asset.url.clone())
                .ok_or_else(|| error("Release is missing its extension.json asset"))?;
            let manifest_value = get_json(&manifest_url, "application/octet-stream")?;
            let manifest: crate::devos::ExtensionManifest = serde_json::from_value(manifest_value)?;
            if !is_newer(&manifest.version, version) {
                println!("{id} {version} is up to date");
                return Ok(());
            }
            println!("{id} {version} -> {}", manifest.version);
            let downloaded = crate::download::resolve(&asset.url)?
                .ok_or_else(|| error("Expected a URL download"))?;
            let mut manager = Manager::open_with(root, devos_data, applications, true)?;
            match manager.install(
                downloaded.path(),
                InstallOptions {
                    name: None,
                    version: None,
                    sha256: Some(asset.sha256.clone()),
                    replace: true,
                    no_desktop: false,
                    source_repository: Some(repository.clone()),
                },
            )? {
                InstallOutcome::Extension(entry) => {
                    println!("Updated {} {}", entry.id, entry.version)
                }
                InstallOutcome::Package(_) => {
                    return Err(error("Expected a Dev OS extension archive for this update"));
                }
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn release() -> Value {
        json!({
            "tag_name": "v1.3.0",
            "draft": false,
            "prerelease": false,
            "assets": [
                {"name": "hello-1.2.0.dpk", "size": 10, "state": "uploaded",
                 "digest": format!("sha256:{}", "a".repeat(64)),
                 "browser_download_url": "https://github.com/example/tools/releases/download/v1.3.0/hello-1.2.0.dpk"},
                {"name": "hello-1.3.0.dpk", "size": 11, "state": "uploaded",
                 "digest": format!("sha256:{}", "b".repeat(64)),
                 "browser_download_url": "https://github.com/example/tools/releases/download/v1.3.0/hello-1.3.0.dpk"}
            ]
        })
    }

    #[test]
    fn release_parsing_rejects_unstable_and_bad_assets() {
        let parsed = parse_release(&release()).unwrap();
        assert_eq!(parsed.version, "1.3.0");
        assert_eq!(parsed.assets.len(), 2);
        let asset = pick_asset(&parsed, "example/tools", "hello-1.3.0.dpk").unwrap();
        assert_eq!(asset.sha256, "b".repeat(64));
        assert!(pick_asset(&parsed, "example/tools", "missing.dpk").is_err());
        for key in ["draft", "prerelease"] {
            let mut unstable = release();
            unstable[key] = json!(true);
            assert!(parse_release(&unstable).is_err());
        }
        let mut no_digest = release();
        no_digest["assets"][1]["digest"] = json!(null);
        let parsed = parse_release(&no_digest).unwrap();
        assert!(pick_asset(&parsed, "example/tools", "hello-1.3.0.dpk").is_err());
        let mut bad_url = release();
        bad_url["assets"][1]["browser_download_url"] = json!("https://evil.example/x");
        let parsed = parse_release(&bad_url).unwrap();
        assert!(pick_asset(&parsed, "example/tools", "hello-1.3.0.dpk").is_err());
        let mut tag = release();
        tag["tag_name"] = json!("nightly");
        assert!(parse_release(&tag).is_err());
    }

    #[test]
    fn versions_compare_and_repository_slugs_parse() {
        assert!(is_newer("1.3.0", "1.2.0"));
        assert!(!is_newer("1.2.0", "1.3.0"));
        assert!(!is_newer("1.2.0", "1.2.0"));
        assert!(is_newer("2026.10", "2026.9"));
        assert_eq!(
            repository_slug("https://github.com/example/tools"),
            Some("example/tools".to_owned())
        );
        assert_eq!(
            repository_slug("https://github.com/example/tools.git"),
            Some("example/tools".to_owned())
        );
        assert_eq!(repository_slug("https://gitlab.com/example/tools"), None);
        assert_eq!(
            repository_slug("https://github.com/example/tools/path"),
            None
        );
    }

    #[test]
    fn release_assets_cover_supported_architectures() {
        assert_eq!(arch_asset_name("x86_64"), Some("dev-linux-x86_64"));
        assert_eq!(arch_asset_name("aarch64"), Some("dev-linux-arm64"));
        assert_eq!(arch_asset_name("riscv64"), None);
    }
}
