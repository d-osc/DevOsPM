use super::*;
use crate::manager::{InstallOptions, Manager};
use std::fs::File;
use tempfile::TempDir;

fn options(replace: bool) -> InstallOptions {
    InstallOptions {
        name: None,
        version: None,
        sha256: None,
        replace,
        no_desktop: false,
        source_repository: None,
    }
}

fn manifest_value(id: &str, version: &str) -> serde_json::Value {
    serde_json::json!({
        "id": id,
        "name": "Workspace Status",
        "description": "Example Dev OS extension",
        "version": version,
        "apiVersion": 1,
        "kind": "ui",
        "system": false,
        "entry": format!("{id}.js"),
        "order": 100,
        "settingsVersion": 1,
        "enabledByDefault": true,
        "settings": []
    })
}

fn manifest(id: &str, version: &str) -> ExtensionManifest {
    serde_json::from_value(manifest_value(id, version)).unwrap()
}

fn extension_archive(path: &Path, id: &str, version: &str) {
    use flate2::{write::GzEncoder, Compression};
    let gzip = GzEncoder::new(File::create(path).unwrap(), Compression::fast());
    let mut tar = tar::Builder::new(gzip);
    let manifest = serde_json::to_vec_pretty(&manifest_value(id, version)).unwrap();
    let entry = format!("extension/dist/{id}.js");
    for (name, data) in [
        ("extension/extension.json", manifest.as_slice()),
        (entry.as_str(), b"export default {};\n".as_slice()),
    ] {
        let mut header = tar::Header::new_gnu();
        header.set_size(data.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        tar.append_data(&mut header, name, data).unwrap();
    }
    tar.into_inner().unwrap().finish().unwrap();
}

fn manager(temp: &TempDir) -> Manager {
    Manager::open_with(
        Some(temp.path().join("store")),
        Some(temp.path().join("devos")),
        Some(temp.path().join("applications")),
        true,
    )
    .unwrap()
}

#[test]
fn extension_ids_repositories_keys_and_versions_are_validated() {
    assert!(valid_extension_id("org.example.status"));
    assert!(valid_extension_id("a-b"));
    assert!(!valid_extension_id("status"));
    assert!(!valid_extension_id("Org.example.status"));
    assert!(!valid_extension_id("org..example"));
    assert!(valid_repository("example/tools"));
    assert!(valid_repository("local/cli"));
    assert!(!valid_repository("example"));
    assert!(!valid_repository("example/a/b"));
    assert!(valid_key("1.0.0-0123456789ab", "1.0.0"));
    assert!(!valid_key("1.0.0-0123456789a", "1.0.0"));
    assert!(!valid_key("2.0.0-0123456789ab", "1.0.0"));
    assert_eq!(semver("1.2.3"), Some((1, 2, 3)));
    assert_eq!(semver("v1.2.3"), None);
    assert_eq!(semver("1.2"), None);
    assert_eq!(semver("1.2.3-beta"), None);
}

#[test]
fn manifest_validation_mirrors_the_desktop_schema() {
    manifest("org.example.status", "1.0.0").validate().unwrap();
    assert!(manifest("status", "1.0.0").validate().is_err());
    assert!(manifest("org.devos.panel", "1.0.0").validate().is_err());
    assert!(manifest("org.example.status", "1.0").validate().is_err());
    let mut system = manifest_value("org.example.status", "1.0.0");
    system["system"] = serde_json::Value::Bool(true);
    serde_json::from_value::<ExtensionManifest>(system)
        .unwrap()
        .validate()
        .unwrap_err();
    let mut api = manifest_value("org.example.status", "1.0.0");
    api["apiVersion"] = serde_json::json!(2);
    serde_json::from_value::<ExtensionManifest>(api)
        .unwrap()
        .validate()
        .unwrap_err();
    let mut entry = manifest_value("org.example.status", "1.0.0");
    entry["entry"] = serde_json::json!("core.js");
    serde_json::from_value::<ExtensionManifest>(entry)
        .unwrap()
        .validate()
        .unwrap_err();
    let mut nested = manifest_value("org.example.status", "1.0.0");
    nested["entry"] = serde_json::json!("dist/app.js");
    serde_json::from_value::<ExtensionManifest>(nested)
        .unwrap()
        .validate()
        .unwrap_err();
    let mut settings_kind = manifest_value("org.example.status", "1.0.0");
    settings_kind["kind"] = serde_json::json!("settings");
    settings_kind["settings"].as_array_mut().unwrap().clear();
    serde_json::from_value::<ExtensionManifest>(settings_kind)
        .unwrap()
        .validate()
        .unwrap_err();
    let mut unknown = manifest_value("org.example.status", "1.0.0");
    unknown["extra"] = serde_json::json!(true);
    assert!(serde_json::from_value::<ExtensionManifest>(unknown).is_err());
}

#[cfg(unix)]
#[test]
fn extension_install_rollback_remove_follow_the_desktop_layout() {
    let temp = TempDir::new().unwrap();
    let v1 = temp.path().join("extension-1.0.0.tar.gz");
    let v2 = temp.path().join("extension-1.1.0.tar.gz");
    extension_archive(&v1, "org.example.status", "1.0.0");
    extension_archive(&v2, "org.example.status", "1.1.0");
    let mut store = manager(&temp);

    let entry = match store.install(&v1, options(false)).unwrap() {
        crate::manager::InstallOutcome::Extension(entry) => entry,
        _ => panic!("expected an extension installation"),
    };
    assert_eq!(entry.version, "1.0.0");
    let release = temp
        .path()
        .join("devos/extension-store/releases/org.example.status");
    assert!(valid_key(&entry.key, "1.0.0"));
    let link = temp.path().join("devos/extensions/org.example.status");
    let manifest_path = link.join("extension.json");
    assert!(manifest_path.exists());
    let record: InstallRecord =
        serde_json::from_reader(File::open(link.join("store-install.json")).unwrap()).unwrap();
    assert_eq!(record.previous, None);
    assert_eq!(store.verify(None).unwrap(), 1);

    // Reinstalling without --replace is rejected like packages.
    assert!(store.install(&v1, options(false)).is_err());

    match store.install(&v2, options(true)).unwrap() {
        crate::manager::InstallOutcome::Extension(entry) => assert_eq!(entry.version, "1.1.0"),
        _ => panic!("expected an extension installation"),
    }
    let record: InstallRecord =
        serde_json::from_reader(File::open(link.join("store-install.json")).unwrap()).unwrap();
    assert_eq!(record.previous.as_ref().unwrap().version, "1.0.0");
    // Both release directories survive for rollback, like the desktop store.
    assert!(release.join(&record.key).is_dir());
    assert!(release
        .join(record.previous.as_ref().unwrap().key.clone())
        .is_dir());

    assert_eq!(store.rollback("org.example.status").unwrap(), "1.0.0");
    let rolled: ExtensionManifest =
        serde_json::from_reader(File::open(manifest_path).unwrap()).unwrap();
    assert_eq!(rolled.version, "1.0.0");
    assert_eq!(store.verify(None).unwrap(), 1);

    store.remove("org.example.status").unwrap();
    assert!(!link.exists());
    assert!(release.is_dir());
    store
        .remove_extension("org.example.status", true)
        .unwrap_err();
    drop(store);
    // Purging an installed extension removes its cached releases.
    let mut again = manager(&temp);
    match again.install(&v1, options(false)).unwrap() {
        crate::manager::InstallOutcome::Extension(_) => (),
        _ => panic!("expected an extension installation"),
    }
    again.remove_extension("org.example.status", true).unwrap();
    assert!(!release.exists());
}

#[cfg(unix)]
#[test]
fn extension_install_honors_the_shared_lock_and_reuses_cached_releases() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("extension-1.0.0.tar.gz");
    extension_archive(&path, "org.example.status", "1.0.0");
    let store_root = temp.path().join("devos/extension-store");
    fs::create_dir_all(&store_root).unwrap();
    fs::File::create(store_root.join("install.lock")).unwrap();
    let mut store = manager(&temp);
    let error = store
        .install(&path, options(false))
        .unwrap_err()
        .to_string();
    assert!(error.contains("in progress"), "{error}");
    fs::remove_file(store_root.join("install.lock")).unwrap();
    match store.install(&path, options(false)).unwrap() {
        crate::manager::InstallOutcome::Extension(entry) => {
            let release = temp
                .path()
                .join("devos/extension-store/releases/org.example.status")
                .join(&entry.key);
            assert!(release.join("dist/org.example.status.js").exists());
            // Installing the very same archive again reuses the cached release.
            match store.install(&path, options(true)).unwrap() {
                crate::manager::InstallOutcome::Extension(again) => {
                    assert_eq!(again.key, entry.key);
                }
                _ => panic!("expected an extension installation"),
            }
        }
        _ => panic!("expected an extension installation"),
    }
    // The lock is released after each operation.
    assert!(!store_root.join("install.lock").exists());
}

#[test]
fn extension_archives_never_mix_with_package_manifests() {
    let temp = TempDir::new().unwrap();
    let mut store = manager(&temp);
    let path = temp.path().join("mixed.tar.gz");
    use flate2::{write::GzEncoder, Compression};
    let gzip = GzEncoder::new(File::create(&path).unwrap(), Compression::fast());
    let mut tar = tar::Builder::new(gzip);
    let manifest = serde_json::to_vec(&manifest_value("org.example.status", "1.0.0")).unwrap();
    let package = serde_json::to_vec(&serde_json::json!({
        "name": "mixed", "version": "1.0.0", "bin": {"mixed": "bin/app"}
    }))
    .unwrap();
    for (name, data) in [
        ("extension/extension.json", manifest.as_slice()),
        ("package.json", package.as_slice()),
        ("bin/app", b"#!/bin/sh\n".as_slice()),
    ] {
        let mut header = tar::Header::new_gnu();
        header.set_size(data.len() as u64);
        header.set_mode(0o755);
        header.set_cksum();
        tar.append_data(&mut header, name, data).unwrap();
    }
    tar.into_inner().unwrap().finish().unwrap();
    let error = store
        .install(&path, options(false))
        .unwrap_err()
        .to_string();
    assert!(error.contains("mixes"), "{error}");
}

#[cfg(unix)]
#[test]
fn extension_archives_install_from_dpk_and_dpkx_filenames() {
    let temp = TempDir::new().unwrap();
    let dpk = temp.path().join("extension-1.0.0.dpk");
    let dpkx = temp.path().join("extension-1.1.0.dpkx");
    extension_archive(&dpk, "org.example.status", "1.0.0");
    extension_archive(&dpkx, "org.example.status", "1.1.0");
    let mut store = manager(&temp);
    match store.install(&dpk, options(false)).unwrap() {
        crate::manager::InstallOutcome::Extension(entry) => assert_eq!(entry.version, "1.0.0"),
        _ => panic!("expected an extension installation"),
    }
    match store.install(&dpkx, options(true)).unwrap() {
        crate::manager::InstallOutcome::Extension(entry) => assert_eq!(entry.version, "1.1.0"),
        _ => panic!("expected an extension installation"),
    }
    let link = temp.path().join("devos/extensions/org.example.status");
    assert!(link.is_symlink());
    assert_eq!(store.verify(None).unwrap(), 1);
}
