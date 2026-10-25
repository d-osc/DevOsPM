use super::*;
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

fn archive(path: &Path, entries: &[(&str, &[u8], u32, tar::EntryType)]) {
    let gzip = GzEncoder::new(File::create(path).unwrap(), Compression::fast());
    let mut tar = tar::Builder::new(gzip);
    for (name, data, mode, kind) in entries {
        let mut header = tar::Header::new_gnu();
        header.set_size(data.len() as u64);
        header.set_mode(*mode);
        header.set_entry_type(*kind);
        // Raw names let us test malicious archives that tar::Builder normally rejects.
        header.as_mut_bytes()[..name.len()].copy_from_slice(name.as_bytes());
        if kind.is_symlink() || kind.is_hard_link() {
            header.set_link_name("../../outside").unwrap();
        }
        header.set_cksum();
        tar.append(&header, *data).unwrap();
    }
    tar.into_inner().unwrap().finish().unwrap();
}

fn package(
    temp: &TempDir,
    filename: &str,
    name: &str,
    version: &str,
    command: Option<&str>,
) -> PathBuf {
    let path = temp.path().join(filename);
    let commands: BTreeMap<String, String> = command
        .into_iter()
        .map(|c| (c.into(), "bin/hello".into()))
        .collect();
    let manifest = serde_json::to_vec(&Manifest {
        name: name.into(),
        shortname: String::new(),
        fullname: String::new(),
        homepage: String::new(),
        storepage: Default::default(),
        version: version.into(),
        description: "Example".into(),
        icon: String::new(),
        repository: Default::default(),
        release: String::new(),
        kind: PackageKind::App,
        bin: commands,
        contextmenu: Vec::new(),
    })
    .unwrap();
    archive(
        &path,
        &[
            (
                "dev-package.json",
                &manifest,
                0o644,
                tar::EntryType::Regular,
            ),
            (
                "bin/hello",
                b"#!/bin/sh\nprintf 'hello %s\\n' \"$1\"\nexit 7\n",
                0o755,
                tar::EntryType::Regular,
            ),
            (
                "share/version",
                version.as_bytes(),
                0o644,
                tar::EntryType::Regular,
            ),
        ],
    );
    path
}

fn manager(temp: &TempDir) -> Manager {
    Manager::open_with(
        Some(temp.path().join("store")),
        Some(temp.path().join("devos")),
        Some(temp.path().join("applications")),
        true,
    )
    .unwrap_or_else(|e| panic!("manager({}): {e}", temp.path().display()))
}

#[test]
fn install_reopen_verify_and_remove() {
    let temp = TempDir::new().unwrap();
    let path = package(&temp, "hello.tar.gz", "hello", "1.0.0", Some("hello"));
    let mut store = manager(&temp);
    let snapshot = store.install_package(&path, options(false)).unwrap();
    assert_eq!(store.verify(None).unwrap(), 1);
    assert_eq!(snapshot.files.len(), 4);
    let installed = store.snapshot_path(&snapshot);
    assert_eq!(fs::read(installed.join("share/version")).unwrap(), b"1.0.0");
    drop(store);
    let mut reopened = manager(&temp);
    assert_eq!(
        reopened.package("hello").unwrap().active.manifest.version,
        "1.0.0"
    );
    reopened.remove("hello").unwrap();
    assert!(reopened.packages().is_empty());
    assert!(!installed.exists());
    drop(reopened);
    assert!(manager(&temp).packages().is_empty());
}

#[test]
fn missing_kind_manifest_defaults_to_app_and_is_verified() {
    let temp = TempDir::new().unwrap();
    let path = package(&temp, "hello.dpk", "hello", "1", None);
    let original = fs::read(&path).unwrap();
    let mut store = manager(&temp);
    let snapshot = store.install_package(&path, options(false)).unwrap();
    assert!(matches!(snapshot.kind, PackageKind::App));
    let manifest = store.snapshot_path(&snapshot).join("package.json");
    let json: serde_json::Value = serde_json::from_slice(&fs::read(&manifest).unwrap()).unwrap();
    assert_eq!(json["kind"], "app");
    assert!(snapshot.files.contains_key("package.json"));
    assert_eq!(fs::read(path).unwrap(), original);
    store.verify(None).unwrap();
    fs::write(manifest, b"{\"kind\":\"extension\"}").unwrap();
    assert!(store.verify(None).is_err());
}

#[test]
fn kind_manifest_preserves_extension_and_rejects_invalid_metadata() {
    for contents in [
        r#"{"kind":"extension","description":"editor"}"#,
        r#"{"kind":"app"}"#,
        r#"{"kind":"invalid"}"#,
        "null",
        "broken",
    ] {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("example.tar.gz");
        archive(
            &path,
            &[(
                "package.json",
                contents.as_bytes(),
                0o644,
                tar::EntryType::Regular,
            )],
        );
        let mut store = manager(&temp);
        let result = store.install_package(&path, options(false));
        if contents.contains("extension") || contents.contains("app") {
            let snapshot = result.unwrap();
            assert_eq!(
                fs::read_to_string(store.snapshot_path(&snapshot).join("package.json")).unwrap(),
                contents
            );
            assert_eq!(
                matches!(snapshot.kind, PackageKind::Extension),
                contents.contains("extension")
            );
            store.verify(None).unwrap();
        } else {
            assert!(result.is_err());
            assert!(store.packages().is_empty());
            assert_eq!(fs::read_dir(store.root.join("staging")).unwrap().count(), 0);
        }
    }
}

#[test]
fn generated_kind_manifest_rejects_conflicting_paths() {
    for path in ["Package.json", "package.json/child"] {
        let temp = TempDir::new().unwrap();
        let archive_path = temp.path().join("example.dpk");
        archive(
            &archive_path,
            &[(path, b"data", 0o644, tar::EntryType::Regular)],
        );
        let mut store = manager(&temp);
        assert!(store
            .install_package(&archive_path, options(false))
            .is_err());
        assert!(store.packages().is_empty());
    }
}

#[test]
fn shared_package_metadata_supports_app_and_extension_pack_install() {
    for kind in ["app", "extension"] {
        let temp = TempDir::new().unwrap();
        let source = temp.path().join("source");
        fs::create_dir(&source).unwrap();
        let data = format!(
            r#"{{"name":"shared","version":"2","kind":"{kind}","homepage":"https://example.com/shared","storepage":"https://example.com/store/shared","repository":"https://github.com/example/shared","release":"https://example.com/releases/latest/shared.dpk","bin":{{"shared":"entry"}}}}"#
        );
        fs::write(source.join("package.json"), &data).unwrap();
        fs::write(source.join("entry"), b"#!/bin/sh\nexit 0\n").unwrap();
        assert_eq!(
            default_package_output(&source).unwrap(),
            PathBuf::from("shared-2.dpk")
        );
        let output = temp.path().join("different.dpk");
        pack(&source, &output).unwrap();
        let mut store = manager(&temp);
        let snapshot = store.install_package(&output, options(false)).unwrap();
        assert_eq!(snapshot.manifest.name, "shared");
        assert_eq!(snapshot.manifest.version, "2");
        assert_eq!(snapshot.manifest.homepage, "https://example.com/shared");
        assert_eq!(
            serde_json::to_value(&snapshot.manifest.storepage).unwrap(),
            "https://example.com/store/shared"
        );
        assert_eq!(
            snapshot.manifest.release,
            "https://example.com/releases/latest/shared.dpk"
        );
        assert_eq!(
            serde_json::to_value(&snapshot.manifest.repository).unwrap(),
            "https://github.com/example/shared"
        );
        assert_eq!(
            matches!(snapshot.kind, PackageKind::Extension),
            kind == "extension"
        );
        assert!(!snapshot.files.contains_key("dev-package.json"));
        assert_eq!(
            fs::read_to_string(store.snapshot_path(&snapshot).join("package.json")).unwrap(),
            data
        );
        store.verify(None).unwrap();
        drop(store);
        let reopened = manager(&temp);
        assert_eq!(
            reopened.package("shared").unwrap().active.manifest.homepage,
            "https://example.com/shared"
        );
        assert_eq!(
            serde_json::to_value(
                &reopened
                    .package("shared")
                    .unwrap()
                    .active
                    .manifest
                    .storepage
            )
            .unwrap(),
            "https://example.com/store/shared"
        );
        assert_eq!(
            reopened.package("shared").unwrap().active.manifest.release,
            "https://example.com/releases/latest/shared.dpk"
        );
        assert_eq!(
            serde_json::to_value(
                &reopened
                    .package("shared")
                    .unwrap()
                    .active
                    .manifest
                    .repository
            )
            .unwrap(),
            "https://github.com/example/shared"
        );
    }
}

#[test]
fn npm_bin_string_and_object_pack_install_and_reopen() {
    for bin in [r#""bin/hello""#, r#"{"hello":"bin/hello"}"#] {
        let temp = TempDir::new().unwrap();
        let source = temp.path().join("source");
        fs::create_dir_all(source.join("bin")).unwrap();
        fs::write(
            source.join("package.json"),
            format!(r#"{{"name":"hello","version":"1","kind":"app","bin":{bin}}}"#),
        )
        .unwrap();
        fs::write(source.join("bin/hello"), b"#!/bin/sh\nexit 0\n").unwrap();
        let output = temp.path().join("other.dpk");
        pack(&source, &output).unwrap();
        let mut store = manager(&temp);
        let snapshot = store.install_package(&output, options(false)).unwrap();
        assert_eq!(snapshot.manifest.bin["hello"], "bin/hello");
        let json = serde_json::to_value(&snapshot.manifest).unwrap();
        assert_eq!(json["bin"]["hello"], "bin/hello");
        assert!(json.get("commands").is_none());
        store.verify(None).unwrap();
        #[cfg(unix)]
        assert_eq!(store.run("hello", &[]).unwrap(), 0);
        drop(store);
        let reopened = manager(&temp);
        assert_eq!(
            reopened.package("hello").unwrap().active.manifest.bin["hello"],
            "bin/hello"
        );
    }
}

#[test]
fn malformed_bin_is_rejected_and_legacy_commands_remain_readable() {
    for bin in ["42", "null", "[]", r#"{"":"entry"}"#, r#"{"app":42}"#] {
        let data = format!(r#"{{"name":"app","version":"1","bin":{bin}}}"#);
        assert!(serde_json::from_str::<Manifest>(&data).is_err());
    }
    let legacy: Manifest =
        serde_json::from_str(r#"{"name":"app","version":"1","commands":{"app":"entry"}}"#).unwrap();
    assert_eq!(legacy.bin["app"], "entry");
    assert!(serde_json::from_str::<Manifest>(
        r#"{"name":"app","version":"1","bin":{},"commands":{}}"#
    )
    .is_err());
}

#[test]
fn repository_release_tracks_package_version() {
    let data = r#"{"name":"extension","version":"1.2.3","kind":"extension","repository":{"type":"git","url":"https://github.com/example/project.git","directory":"packages/extension","release":{"type":"dpk","filename":"{name}-{version}","tag":"v{version}"}}}"#;
    let mut manifest: Manifest = serde_json::from_str(data).unwrap();
    manifest.validate().unwrap();
    let first = manifest.resolved_release().unwrap().unwrap();
    assert_eq!(
        first.url,
        "https://github.com/example/project/releases/download/v1.2.3/extension-1.2.3.dpk"
    );
    manifest.version = "2.0.0".into();
    let second = manifest.resolved_release().unwrap().unwrap();
    assert_eq!(second.version, "2.0.0");
    assert_eq!(second.tag, "v2.0.0");
    assert_eq!(second.filename, "extension-2.0.0.dpk");
    assert_eq!(
        second.url,
        "https://github.com/example/project/releases/download/v2.0.0/extension-2.0.0.dpk"
    );
    let temp = TempDir::new().unwrap();
    let source = temp.path().join("source");
    fs::create_dir(&source).unwrap();
    fs::write(source.join("package.json"), data).unwrap();
    let archive = temp.path().join("extension.dpk");
    pack(&source, &archive).unwrap();
    let mut store = manager(&temp);
    store.install_package(&archive, options(false)).unwrap();
    drop(store);
    let reopened = manager(&temp);
    assert_eq!(
        reopened
            .package("extension")
            .unwrap()
            .active
            .manifest
            .resolved_release()
            .unwrap()
            .unwrap()
            .url,
        first.url
    );
    reopened.verify(None).unwrap();
}

#[test]
fn icon_roundtrip_reopen_and_tampering_detection() {
    let temp = TempDir::new().unwrap();
    let source = temp.path().join("source");
    fs::create_dir_all(source.join("share")).unwrap();
    fs::write(
        source.join("package.json"),
        br#"{"name":"icon-example","version":"1","kind":"extension","icon":"share/icon.svg"}"#,
    )
    .unwrap();
    fs::write(
        source.join("share/icon.svg"),
        b"<svg xmlns=\"http://www.w3.org/2000/svg\"/>",
    )
    .unwrap();
    let output = temp.path().join("icon-example.dpk");
    pack(&source, &output).unwrap();
    let mut store = manager(&temp);
    let snapshot = store.install_package(&output, options(false)).unwrap();
    assert_eq!(snapshot.manifest.icon, "share/icon.svg");
    assert!(snapshot.files.contains_key("share/icon.svg"));
    store.verify(None).unwrap();
    drop(store);
    let reopened = manager(&temp);
    assert_eq!(
        reopened
            .package("icon-example")
            .unwrap()
            .active
            .manifest
            .icon,
        "share/icon.svg"
    );
    fs::write(
        reopened.snapshot_path(&snapshot).join("share/icon.svg"),
        b"modified",
    )
    .unwrap();
    assert!(reopened.verify(None).is_err());
}

#[test]
fn icon_paths_and_missing_files_fail_before_commit() {
    for icon in ["../outside.svg", "/outside.svg", "share/missing.svg"] {
        let temp = TempDir::new().unwrap();
        let source = temp.path().join("source");
        fs::create_dir(&source).unwrap();
        let manifest = format!(r#"{{"name":"icon-example","version":"1","icon":"{icon}"}}"#);
        fs::write(source.join("package.json"), &manifest).unwrap();
        let output = temp.path().join("icon-example.dpk");
        assert!(pack(&source, &output).is_err());
        archive(
            &output,
            &[(
                "package.json",
                manifest.as_bytes(),
                0o644,
                tar::EntryType::Regular,
            )],
        );
        let mut store = manager(&temp);
        assert!(store.install_package(&output, options(false)).is_err());
        assert!(store.packages().is_empty());
        assert_eq!(fs::read_dir(store.root.join("staging")).unwrap().count(), 0);
    }
}

#[test]
fn display_names_and_shotname_alias_roundtrip() {
    for field in ["shortname", "shotname"] {
        let temp = TempDir::new().unwrap();
        let source = temp.path().join("source");
        fs::create_dir(&source).unwrap();
        let json = format!(
            r#"{{"name":"display-example","version":"1","{field}":"ชื่อย่อ","fullname":"ชื่อเต็มของแอป"}}"#
        );
        fs::write(source.join("package.json"), &json).unwrap();
        let output = temp.path().join("display-example.dpk");
        pack(&source, &output).unwrap();
        let mut store = manager(&temp);
        store.install_package(&output, options(false)).unwrap();
        drop(store);
        let reopened = manager(&temp);
        let manifest = &reopened.package("display-example").unwrap().active.manifest;
        assert_eq!(manifest.name, "display-example");
        assert_eq!(manifest.shortname, "ชื่อย่อ");
        assert_eq!(manifest.fullname, "ชื่อเต็มของแอป");
        let serialized = serde_json::to_value(manifest).unwrap();
        assert!(serialized.get("shotname").is_none());
        assert_eq!(serialized["shortname"], "ชื่อย่อ");
        reopened.verify(None).unwrap();
    }
    assert!(serde_json::from_str::<Manifest>(
        r#"{"name":"app","version":"1","shortname":"A","shotname":"B"}"#
    )
    .is_err());
}

#[test]
fn contextmenu_pack_install_and_reopen_preserves_arguments() {
    for kind in ["app", "extension"] {
        let temp = TempDir::new().unwrap();
        let source = temp.path().join("source");
        fs::create_dir(&source).unwrap();
        let data = format!(
            r#"{{"name":"menu-example","version":"1","kind":"{kind}","bin":{{"menu-command":"entry"}},"contextmenu":[{{"id":"greet","label":"ทักทาย","command":"menu-command","args":["--selected","{{path}}","argument with spaces"],"targets":["file","directory","background"]}}]}}"#
        );
        fs::write(source.join("package.json"), &data).unwrap();
        fs::write(source.join("entry"), b"#!/bin/sh\nexit 0\n").unwrap();
        let output = temp.path().join("menu-example.dpk");
        pack(&source, &output).unwrap();
        let mut store = manager(&temp);
        store.install_package(&output, options(false)).unwrap();
        drop(store);
        let reopened = manager(&temp);
        let item = &reopened
            .package("menu-example")
            .unwrap()
            .active
            .manifest
            .contextmenu[0];
        assert_eq!(item.id, "greet");
        assert_eq!(item.label, "ทักทาย");
        assert_eq!(item.args, ["--selected", "{path}", "argument with spaces"]);
        assert_eq!(item.targets.len(), 3);
        reopened.verify(None).unwrap();
    }
}

#[test]
fn invalid_contextmenu_fails_before_commit() {
    let valid = serde_json::json!({"id":"greet","label":"Hello","command":"hello","args":["{path}"],"targets":["file"]});
    let mut variants = Vec::new();
    for (field, value) in [
        ("id", serde_json::json!("../invalid")),
        ("label", serde_json::json!(" ")),
        ("command", serde_json::json!("missing")),
        ("targets", serde_json::json!([])),
        ("targets", serde_json::json!(["file", "file"])),
        ("args", serde_json::json!(["\0"])),
    ] {
        let mut item = valid.clone();
        item[field] = value;
        variants.push(serde_json::json!([item]));
    }
    variants.push(serde_json::json!([valid.clone(), valid]));
    for menu in variants {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("menu-example.dpk");
        let manifest = serde_json::to_vec(&serde_json::json!({"name":"menu-example","version":"1","bin":{"hello":"entry"},"contextmenu":menu})).unwrap();
        archive(
            &path,
            &[
                ("package.json", &manifest, 0o644, tar::EntryType::Regular),
                ("entry", b"#!/bin/sh\n", 0o755, tar::EntryType::Regular),
            ],
        );
        let mut store = manager(&temp);
        assert!(store.install_package(&path, options(false)).is_err());
        assert!(store.packages().is_empty());
        assert_eq!(fs::read_dir(store.root.join("staging")).unwrap().count(), 0);
    }
}

#[test]
fn html_markdown_and_short_storepages_roundtrip() {
    for page in [
        serde_json::json!({"index":"index.html","files":["index.html","style.css"]}),
        serde_json::json!({"index":"index.md","files":["index.md","style.css"]}),
        serde_json::json!("index.md"),
    ] {
        let temp = TempDir::new().unwrap();
        let source = temp.path().join("source");
        fs::create_dir(&source).unwrap();
        let manifest = serde_json::json!({"name":"store-example","version":"1","storepage":page});
        fs::write(
            source.join("package.json"),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
        fs::write(source.join("index.html"), b"<h1>Example</h1>").unwrap();
        fs::write(source.join("index.md"), b"# Example\n").unwrap();
        fs::write(source.join("style.css"), b"body { color: blue; }").unwrap();
        let output = temp.path().join("store-example.dpk");
        pack(&source, &output).unwrap();
        let mut store = manager(&temp);
        let snapshot = store.install_package(&output, options(false)).unwrap();
        drop(store);
        let reopened = manager(&temp);
        assert_eq!(
            serde_json::to_value(
                &reopened
                    .package("store-example")
                    .unwrap()
                    .active
                    .manifest
                    .storepage
            )
            .unwrap(),
            page
        );
        reopened.verify(None).unwrap();
        fs::write(
            reopened.snapshot_path(&snapshot).join("style.css"),
            b"modified",
        )
        .unwrap();
        assert!(reopened.verify(None).is_err());
    }
}

#[test]
fn invalid_or_missing_storepage_files_fail_before_commit() {
    for page in [
        serde_json::json!({"index":"index.html","files":[]}),
        serde_json::json!({"index":"index.html","files":["index.html","index.html"]}),
        serde_json::json!({"index":"../index.md","files":["../index.md"]}),
        serde_json::json!({"index":"index.md","files":["index.md","/outside"]}),
        serde_json::json!({"index":"index.exe","files":["index.exe"]}),
        serde_json::json!({"index":"missing.md","files":["missing.md"]}),
        serde_json::json!("missing.md"),
    ] {
        let temp = TempDir::new().unwrap();
        let source = temp.path().join("source");
        fs::create_dir(&source).unwrap();
        let manifest = serde_json::to_vec(
            &serde_json::json!({"name":"store-example","version":"1","storepage":page}),
        )
        .unwrap();
        fs::write(source.join("package.json"), &manifest).unwrap();
        let output = temp.path().join("store-example.dpk");
        assert!(pack(&source, &output).is_err());
        archive(
            &output,
            &[("package.json", &manifest, 0o644, tar::EntryType::Regular)],
        );
        let mut store = manager(&temp);
        assert!(store.install_package(&output, options(false)).is_err());
        assert!(store.packages().is_empty());
        assert_eq!(fs::read_dir(store.root.join("staging")).unwrap().count(), 0);
    }
}

#[test]
fn custom_release_url_templates_and_invalid_templates() {
    let data = r#"{"name":"app","version":"1","repository":{"type":"git","url":"https://gitlab.com/example/project.git","release":{"type":"tar.gz","filename":"{name}-{version}.tar.gz","url":"https://example.com/{tag}/{filename}"}}}"#;
    let manifest: Manifest = serde_json::from_str(data).unwrap();
    assert_eq!(
        manifest.resolved_release().unwrap().unwrap().url,
        "https://example.com/v1/app-1.tar.gz"
    );
    for bad in [
        data.replace("{name}", "{unknown}"),
        data.replace("tar.gz", "zip"),
        data.replace("{name}-{version}.tar.gz", "../app.tar.gz"),
        data.replace("https://example.com", "file:///tmp"),
    ] {
        let manifest: Manifest = serde_json::from_str(&bad).unwrap();
        assert!(manifest.validate().is_err());
    }
}

#[test]
fn upgrade_and_rollback_preserve_both_generations() {
    let temp = TempDir::new().unwrap();
    let v1 = package(&temp, "v1.tar.gz", "hello", "1", Some("hello"));
    let v2 = package(&temp, "v2.tar.gz", "hello", "2", Some("hello"));
    let mut store = manager(&temp);
    let old = store.install_package(&v1, options(false)).unwrap();
    assert!(store.install_package(&v2, options(false)).is_err());
    let new = store.install_package(&v2, options(true)).unwrap();
    assert_eq!(store.rollback("hello").unwrap(), "1");
    assert_eq!(store.verify(None).unwrap(), 1);
    assert!(store.snapshot_path(&old).exists());
    assert!(store.snapshot_path(&new).exists());
    assert_eq!(store.rollback("hello").unwrap(), "2");
    store.remove("hello").unwrap();
    assert!(!store.snapshot_path(&old).exists());
    assert!(!store.snapshot_path(&new).exists());
}

#[test]
fn dpk_and_legacy_archives_share_upgrades_and_rollback() {
    let temp = TempDir::new().unwrap();
    let mut store = manager(&temp);
    let dpk = package(&temp, "hello.dpk", "hello", "1", Some("hello"));
    let legacy = package(&temp, "hello.tar.gz", "hello", "2", Some("hello"));
    let latest = package(&temp, "latest.dpk", "hello", "3", Some("hello"));
    store.install_package(&dpk, options(false)).unwrap();
    store.install_package(&legacy, options(true)).unwrap();
    assert_eq!(store.rollback("hello").unwrap(), "1");
    store.install_package(&latest, options(true)).unwrap();
    assert_eq!(store.package("hello").unwrap().active.manifest.version, "3");
    assert_eq!(store.verify(None).unwrap(), 1);
    assert_eq!(store.rollback("hello").unwrap(), "1");
    drop(store);
    assert_eq!(manager(&temp).verify(None).unwrap(), 1);
}

#[test]
fn dpk_requires_valid_archive_contents() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("invalid.dpk");
    fs::write(&path, b"This is not a gzip archive").unwrap();
    let mut store = manager(&temp);
    assert!(store.install_package(&path, options(false)).is_err());
    assert!(store.packages().is_empty());
    assert_eq!(fs::read_dir(store.root.join("staging")).unwrap().count(), 0);
}

#[test]
fn third_upgrade_cleans_only_oldest_generation() {
    let temp = TempDir::new().unwrap();
    let mut store = manager(&temp);
    let first = store
        .install_package(
            &package(&temp, "one.tar.gz", "hello", "1", None),
            options(false),
        )
        .unwrap();
    let second = store
        .install_package(
            &package(&temp, "two.tar.gz", "hello", "2", None),
            options(true),
        )
        .unwrap();
    let third = store
        .install_package(
            &package(&temp, "three.tar.gz", "hello", "3", None),
            options(true),
        )
        .unwrap();
    assert!(!store.snapshot_path(&first).exists());
    assert!(store.snapshot_path(&second).exists());
    assert!(store.snapshot_path(&third).exists());
    assert_eq!(store.rollback("hello").unwrap(), "2");
}

#[test]
fn plain_archive_supports_name_version_and_inference() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("plain.dpk");
    archive(
        &path,
        &[("./share/data", b"plain", 0o644, tar::EntryType::Regular)],
    );
    let mut store = manager(&temp);
    let plain = store.install_package(&path, options(false)).unwrap();
    assert_eq!(plain.manifest.name, "plain");
    assert_eq!(plain.manifest.version, "0.0.0");
    let mut custom = options(false);
    custom.name = Some("custom".into());
    custom.version = Some("2026.10".into());
    assert_eq!(
        store
            .install_package(&path, custom)
            .unwrap()
            .manifest
            .version,
        "2026.10"
    );
}

#[test]
fn sha256_is_checked_before_install() {
    let temp = TempDir::new().unwrap();
    let path = package(&temp, "hello.tar.gz", "hello", "1", None);
    let mut store = manager(&temp);
    let mut bad = options(false);
    bad.sha256 = Some("00".repeat(32));
    assert!(store
        .install_package(&path, bad)
        .unwrap_err()
        .to_string()
        .contains("mismatch"));
    assert!(store.packages().is_empty());
    let mut valid = options(false);
    valid.sha256 = Some(
        hash_reader(File::open(&path).unwrap())
            .unwrap()
            .0
            .to_uppercase(),
    );
    store.install_package(&path, valid).unwrap();
}

#[test]
fn hostile_paths_never_escape_staging() {
    for name in [
        "../outside",
        "/absolute",
        "bin/../../outside",
        "C:/outside",
        "bin\\outside",
        "foo:stream",
        "NUL",
        "foo.",
    ] {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("unsafe.dpk");
        archive(&path, &[(name, b"attack", 0o644, tar::EntryType::Regular)]);
        let mut store = manager(&temp);
        assert!(
            store.install_package(&path, options(false)).is_err(),
            "accepted {name}"
        );
        assert!(!temp.path().join("outside").exists());
        assert_eq!(fs::read_dir(store.root.join("staging")).unwrap().count(), 0);
        assert!(store.packages().is_empty());
    }
}

#[test]
fn rejects_links_and_special_files() {
    for kind in [
        tar::EntryType::Symlink,
        tar::EntryType::Link,
        tar::EntryType::Fifo,
        tar::EntryType::Char,
    ] {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("unsafe.tar.gz");
        archive(&path, &[("link", b"", 0o644, kind)]);
        assert!(manager(&temp)
            .install_package(&path, options(false))
            .is_err());
    }
}

#[test]
fn rejects_duplicate_and_case_conflicting_entries() {
    for second in ["data", "DATA"] {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("duplicate.tar.gz");
        archive(
            &path,
            &[
                ("data", b"a", 0o644, tar::EntryType::Regular),
                (second, b"b", 0o644, tar::EntryType::Regular),
            ],
        );
        assert!(manager(&temp)
            .install_package(&path, options(false))
            .unwrap_err()
            .to_string()
            .contains("Duplicate"));
    }
}

#[test]
fn implicit_parent_directory_case_collisions_are_rejected() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("case.tar.gz");
    archive(
        &path,
        &[
            ("bin/first", b"a", 0o644, tar::EntryType::Regular),
            ("BIN/second", b"b", 0o644, tar::EntryType::Regular),
        ],
    );
    assert!(manager(&temp)
        .install_package(&path, options(false))
        .unwrap_err()
        .to_string()
        .contains("path case"));
}

#[test]
fn oversized_payload_is_rejected_before_writing() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("huge.tar.gz");
    let mut gzip = GzEncoder::new(File::create(&path).unwrap(), Compression::fast());
    let mut header = tar::Header::new_gnu();
    header.set_path("huge").unwrap();
    header.set_mode(0o644);
    header.set_size(MAX_BYTES + 1);
    header.set_cksum();
    gzip.write_all(header.as_bytes()).unwrap();
    gzip.finish().unwrap();
    let mut store = manager(&temp);
    assert!(store
        .install_package(&path, options(false))
        .unwrap_err()
        .to_string()
        .contains("2 GiB"));
    assert!(store.packages().is_empty());
}

fn pax_field(key: &str, value: &str) -> Vec<u8> {
    let body = format!(" {key}={value}\n");
    let mut length = body.len() + 1;
    loop {
        let next = body.len() + length.to_string().len();
        if next == length {
            return format!("{length}{body}").into_bytes();
        }
        length = next;
    }
}

#[test]
fn local_pax_paths_are_supported_and_validated() {
    for name in ["share/renamed", "../outside"] {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("pax.tar.gz");
        let metadata = pax_field("path", name);
        archive(
            &path,
            &[
                ("PaxHeader", &metadata, 0o644, tar::EntryType::XHeader),
                ("original", b"data", 0o644, tar::EntryType::Regular),
            ],
        );
        let mut store = manager(&temp);
        if name.starts_with("..") {
            assert!(store.install_package(&path, options(false)).is_err());
        } else {
            let installed = store.install_package(&path, options(false)).unwrap();
            assert!(installed.files.contains_key(name));
            store.verify(None).unwrap();
        }
    }
}

#[test]
fn oversized_tar_metadata_is_rejected_without_allocation() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("metadata.tar.gz");
    let mut gzip = GzEncoder::new(File::create(&path).unwrap(), Compression::fast());
    let mut header = tar::Header::new_gnu();
    header.set_path("PaxHeader").unwrap();
    header.set_entry_type(tar::EntryType::XHeader);
    header.set_size(MAX_MANIFEST + 1);
    header.set_mode(0o644);
    header.set_cksum();
    gzip.write_all(header.as_bytes()).unwrap();
    gzip.finish().unwrap();
    assert!(manager(&temp)
        .install_package(&path, options(false))
        .unwrap_err()
        .to_string()
        .contains("metadata exceeds"));
}

#[test]
fn conflicting_pax_size_and_dangling_metadata_are_rejected() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("size.tar.gz");
    let metadata = pax_field("size", "99");
    archive(
        &path,
        &[
            ("PaxHeader", &metadata, 0o644, tar::EntryType::XHeader),
            ("data", b"data", 0o644, tar::EntryType::Regular),
        ],
    );
    let mut store = manager(&temp);
    assert!(store
        .install_package(&path, options(false))
        .unwrap_err()
        .to_string()
        .contains("PAX size"));
    archive(
        &path,
        &[(
            "PaxHeader",
            &pax_field("path", "dangling"),
            0o644,
            tar::EntryType::XHeader,
        )],
    );
    assert!(store
        .install_package(&path, options(false))
        .unwrap_err()
        .to_string()
        .contains("Dangling"));
}

#[test]
fn corrupt_gzip_upgrade_keeps_original_and_cleans_stage() {
    let temp = TempDir::new().unwrap();
    let good = package(&temp, "good.tar.gz", "hello", "1", None);
    let bad = package(&temp, "bad.dpk", "hello", "2", None);
    let mut bytes = fs::read(&bad).unwrap();
    let index = bytes.len() - 8; // CRC bytes, after the tar end marker.
    bytes[index] ^= 0xff;
    fs::write(&bad, bytes).unwrap();
    let mut store = manager(&temp);
    store.install_package(&good, options(false)).unwrap();
    assert!(store.install_package(&bad, options(true)).is_err());
    assert_eq!(store.package("hello").unwrap().active.manifest.version, "1");
    assert_eq!(store.verify(None).unwrap(), 1);
    assert_eq!(fs::read_dir(store.root.join("staging")).unwrap().count(), 0);
    assert_eq!(
        fs::read_dir(store.root.join("packages")).unwrap().count(),
        1
    );
}

#[test]
fn truncated_gzip_is_rejected() {
    let temp = TempDir::new().unwrap();
    let path = package(&temp, "broken.tar.gz", "hello", "1", None);
    let bytes = fs::read(&path).unwrap();
    fs::write(&path, &bytes[..bytes.len() - 5]).unwrap();
    assert!(manager(&temp)
        .install_package(&path, options(false))
        .is_err());
}

#[test]
fn failed_registry_commit_preserves_active_payload() {
    let temp = TempDir::new().unwrap();
    let mut store = manager(&temp);
    let old = store
        .install_package(
            &package(&temp, "one.tar.gz", "hello", "1", None),
            options(false),
        )
        .unwrap();
    let registry = store.root.join("registry.json");
    let backup = store.root.join("saved.json");
    fs::rename(&registry, &backup).unwrap();
    fs::create_dir(&registry).unwrap(); // Force atomic commit failure.
    let new = package(&temp, "two.tar.gz", "hello", "2", None);
    assert!(store.install_package(&new, options(true)).is_err());
    assert_eq!(store.package("hello").unwrap().active.manifest.version, "1");
    assert!(store.snapshot_path(&old).exists());
    assert_eq!(
        fs::read_dir(store.root.join("packages")).unwrap().count(),
        1
    );
    fs::remove_dir(&registry).unwrap();
    fs::rename(&backup, &registry).unwrap();
    drop(store);
    assert_eq!(manager(&temp).verify(None).unwrap(), 1);
}

#[test]
fn verify_detects_modified_missing_and_unexpected_files() {
    let temp = TempDir::new().unwrap();
    let mut store = manager(&temp);
    let installed = store
        .install_package(
            &package(&temp, "hello.tar.gz", "hello", "1", None),
            options(false),
        )
        .unwrap();
    let root = store.snapshot_path(&installed);
    fs::write(root.join("share/version"), b"X").unwrap();
    assert!(store
        .verify(None)
        .unwrap_err()
        .to_string()
        .contains("Integrity"));
    fs::write(root.join("share/version"), b"1").unwrap();
    fs::write(root.join("extra"), b"unexpected").unwrap();
    assert!(store.verify(None).is_err());
    fs::remove_file(root.join("extra")).unwrap();
    fs::remove_file(root.join("share/version")).unwrap();
    assert!(store.verify(None).is_err());
}

#[test]
fn cannot_rollback_to_damaged_version() {
    let temp = TempDir::new().unwrap();
    let mut store = manager(&temp);
    let old = store
        .install_package(
            &package(&temp, "one.tar.gz", "hello", "1", None),
            options(false),
        )
        .unwrap();
    store
        .install_package(
            &package(&temp, "two.tar.gz", "hello", "2", None),
            options(true),
        )
        .unwrap();
    fs::write(store.snapshot_path(&old).join("share/version"), b"damaged").unwrap();
    assert!(store.rollback("hello").is_err());
    assert_eq!(store.package("hello").unwrap().active.manifest.version, "2");
}

#[test]
fn command_collisions_and_rollback_collisions_are_rejected() {
    let temp = TempDir::new().unwrap();
    let mut store = manager(&temp);
    store
        .install_package(
            &package(&temp, "one.tar.gz", "one", "1", Some("hello")),
            options(false),
        )
        .unwrap();
    let other = package(&temp, "two.tar.gz", "two", "1", Some("hello"));
    assert!(store.install_package(&other, options(false)).is_err());
    store
        .install_package(
            &package(&temp, "upgrade.tar.gz", "one", "2", Some("new-hello")),
            options(true),
        )
        .unwrap();
    store.install_package(&other, options(false)).unwrap();
    assert!(store
        .rollback("one")
        .unwrap_err()
        .to_string()
        .contains("now exported"));
    assert_eq!(store.package("one").unwrap().active.manifest.version, "2");
}

#[test]
fn writer_lock_excludes_readers_and_writers() {
    let temp = TempDir::new().unwrap();
    let writer = manager(&temp);
    let root = temp.path().join("store");
    assert!(Manager::open_with(Some(root.clone()), None, None, true).is_err());
    assert!(Manager::open_with(Some(root.clone()), None, None, false).is_err());
    drop(writer);
    let first = Manager::open_with(Some(root.clone()), None, None, false).unwrap();
    let second = Manager::open_with(Some(root.clone()), None, None, false).unwrap();
    assert!(Manager::open_with(Some(root), None, None, true).is_err());
    drop((first, second));
    manager(&temp);
}

#[test]
fn corrupt_registry_is_never_silently_reset() {
    let temp = TempDir::new().unwrap();
    let store = manager(&temp);
    fs::write(store.root.join("registry.json"), b"{ corrupt").unwrap();
    drop(store);
    assert!(
        Manager::open_with(Some(temp.path().join("store")), None, None, true)
            .err()
            .unwrap()
            .to_string()
            .contains("corrupt")
    );
    assert_eq!(
        fs::read(temp.path().join("store/registry.json")).unwrap(),
        b"{ corrupt"
    );
}

#[test]
fn registry_paths_cannot_escape_store() {
    let temp = TempDir::new().unwrap();
    let mut store = manager(&temp);
    store
        .install_package(
            &package(&temp, "hello.tar.gz", "hello", "1", None),
            options(false),
        )
        .unwrap();
    let mut corrupted = store.registry.clone();
    corrupted
        .packages
        .get_mut("hello")
        .unwrap()
        .active
        .directory = "../../outside".into();
    fs::write(
        store.root.join("registry.json"),
        serde_json::to_vec(&corrupted).unwrap(),
    )
    .unwrap();
    drop(store);
    assert!(Manager::open_with(Some(temp.path().join("store")), None, None, true).is_err());
}

#[test]
fn pack_roundtrip_is_reproducible_and_does_not_overwrite() {
    let temp = TempDir::new().unwrap();
    let source = temp.path().join("source");
    fs::create_dir_all(source.join("bin")).unwrap();
    fs::write(
        source.join("dev-package.json"),
        br#"{"name":"hello","version":"1","commands":{"hello":"bin/hello"}}"#,
    )
    .unwrap();
    fs::write(source.join("bin/hello"), b"#!/bin/sh\necho hello\n").unwrap();
    fs::write(source.join("a".repeat(120)), b"GNU long-name fixture").unwrap();
    let first = temp.path().join("first.dpk");
    let second = temp.path().join("second.tar.gz");
    pack(&source, &first).unwrap();
    pack(&source, &second).unwrap();
    assert_eq!(
        default_package_output(&source).unwrap(),
        PathBuf::from("hello-1.dpk")
    );
    assert_eq!(fs::read(&first).unwrap(), fs::read(&second).unwrap());
    assert!(pack(&source, &first).is_err());
    assert!(pack(&source, &source.join("nested.tar.gz")).is_err());
    let mut store = manager(&temp);
    store.install_package(&first, options(false)).unwrap();
    assert_eq!(store.verify(None).unwrap(), 1);
}

#[test]
fn invalid_manifest_and_missing_command_fail_before_commit() {
    for manifest in [
        br#"{"name":"../unsafe","version":"1"}"#.as_slice(),
        br#"{"name":"hello","version":"1","commands":{"hello":"missing"}}"#.as_slice(),
        br#"{"name":"hello","version":"1","scripts":{"postinstall":"danger"}}"#.as_slice(),
    ] {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("invalid.tar.gz");
        archive(
            &path,
            &[("dev-package.json", manifest, 0o644, tar::EntryType::Regular)],
        );
        let mut store = manager(&temp);
        assert!(store.install_package(&path, options(false)).is_err());
        assert!(store.packages().is_empty());
    }
}

#[cfg(unix)]
#[test]
fn run_forwards_arguments_and_exit_code_and_rejects_tampering() {
    let temp = TempDir::new().unwrap();
    let mut store = manager(&temp);
    let installed = store
        .install_package(
            &package(&temp, "hello.tar.gz", "hello", "1", Some("hello")),
            options(false),
        )
        .unwrap();
    assert_eq!(
        store.run("hello", &["world with spaces".into()]).unwrap(),
        7
    );
    fs::write(
        store.snapshot_path(&installed).join("bin/hello"),
        b"#!/bin/sh\nexit 0\n",
    )
    .unwrap();
    assert!(store
        .run("hello", &[])
        .unwrap_err()
        .to_string()
        .contains("modified"));
}

#[cfg(unix)]
#[test]
fn symlink_payload_and_changed_executable_mode_fail_verification() {
    use std::os::unix::{fs::symlink, fs::PermissionsExt};
    let temp = TempDir::new().unwrap();
    let mut store = manager(&temp);
    let installed = store
        .install_package(
            &package(&temp, "hello.tar.gz", "hello", "1", Some("hello")),
            options(false),
        )
        .unwrap();
    let root = store.snapshot_path(&installed);
    fs::set_permissions(root.join("bin/hello"), fs::Permissions::from_mode(0o644)).unwrap();
    assert!(store.verify(None).is_err());
    fs::set_permissions(root.join("bin/hello"), fs::Permissions::from_mode(0o755)).unwrap();
    symlink("/tmp", root.join("link")).unwrap();
    assert!(store.verify(None).is_err());
    assert!(pack(&root, &temp.path().join("unsafe.tar.gz")).is_err());
}

#[test]
fn desktop_entries_follow_install_remove_and_rollback() {
    let temp = TempDir::new().unwrap();
    let v1 = package(&temp, "hello.dpk", "hello", "1", Some("hello"));
    let v2 = package(&temp, "hello-v2.dpk", "hello", "2", Some("hello2"));
    let applications = temp.path().join("applications");
    let first = applications.join("dev-hello.desktop");
    let second = applications.join("dev-hello2.desktop");
    let mut store = manager(&temp);
    store.install_package(&v1, options(false)).unwrap();
    let content = fs::read_to_string(&first).unwrap();
    assert!(content.contains("Type=Application"));
    assert!(content.contains("Name=hello"));
    assert!(content.contains(" run hello\n"));
    assert!(content.contains("X-DevOS-Package=hello"));
    assert!(!second.exists());

    // Upgrading with a different command set swaps the launcher entries.
    store.install_package(&v2, options(true)).unwrap();
    assert!(!first.exists());
    assert!(second.exists());

    store.rollback("hello").unwrap();
    assert!(first.exists());
    assert!(!second.exists());

    store.remove("hello").unwrap();
    assert!(!first.exists());
}

#[test]
fn remove_sweeps_stale_entries_of_the_package_only() {
    let temp = TempDir::new().unwrap();
    let path = package(&temp, "hello.dpk", "hello", "1", Some("hello"));
    let applications = temp.path().join("applications");
    let mut store = manager(&temp);
    store.install_package(&path, options(false)).unwrap();
    // A leftover from an older command set, an unrelated dev entry and a
    // foreign file that merely mentions the package name.
    let stale = applications.join("dev-old.desktop");
    let other = applications.join("dev-other.desktop");
    let foreign = applications.join("hello.desktop");
    fs::write(&stale, "[Desktop Entry]\nX-DevOS-Package=hello\n").unwrap();
    fs::write(&other, "[Desktop Entry]\nX-DevOS-Package=hello-two\n").unwrap();
    fs::write(&foreign, "[Desktop Entry]\nX-DevOS-Package=hello\n").unwrap();

    store.remove("hello").unwrap();
    assert!(!applications.join("dev-hello.desktop").exists());
    assert!(!stale.exists());
    assert!(other.exists());
    assert!(foreign.exists());
}

#[test]
fn command_shims_follow_install_upgrade_rollback_and_remove() {
    let temp = TempDir::new().unwrap();
    let bin = temp.path().join("bin");
    fs::create_dir_all(&bin).unwrap();
    let v1 = package(&temp, "hello.dpk", "hello", "1", Some("hello"));
    let v2 = package(&temp, "hello-v2.dpk", "hello", "2", Some("hello2"));
    let mut store = manager(&temp);
    store.bin_dir = Some(bin.clone());

    store.install_package(&v1, options(false)).unwrap();
    let shim = fs::read_to_string(bin.join("hello")).unwrap();
    assert!(shim.starts_with("#!/bin/sh
# dev-shim: package=hello
"));
    assert!(shim.contains(" run 'hello' \"$@\""));
    assert!(shim.contains("--root"));
    assert!(store.shim_path("hello", "hello").is_some());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(bin.join("hello")).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o755);
    }

    // Upgrading with another command set swaps the shims; rollback swaps back.
    store.install_package(&v2, options(true)).unwrap();
    assert!(!bin.join("hello").exists());
    assert!(bin.join("hello2").exists());
    store.rollback("hello").unwrap();
    assert!(bin.join("hello").exists());
    assert!(!bin.join("hello2").exists());

    store.remove("hello").unwrap();
    assert!(!bin.join("hello").exists());
}

#[test]
fn command_shims_never_overwrite_or_remove_foreign_files() {
    let temp = TempDir::new().unwrap();
    let bin = temp.path().join("bin");
    fs::create_dir_all(&bin).unwrap();
    fs::write(bin.join("hello"), "#!/bin/sh
echo system hello
").unwrap();
    let path = package(&temp, "hello.dpk", "hello", "1", Some("hello"));
    let mut store = manager(&temp);
    store.bin_dir = Some(bin.clone());

    store.install_package(&path, options(false)).unwrap();
    assert_eq!(
        fs::read_to_string(bin.join("hello")).unwrap(),
        "#!/bin/sh
echo system hello
"
    );
    assert!(store.shim_path("hello", "hello").is_none());
    store.remove("hello").unwrap();
    assert!(bin.join("hello").exists());
}

#[test]
fn no_desktop_flag_skips_command_shims() {
    let temp = TempDir::new().unwrap();
    let bin = temp.path().join("bin");
    let path = package(&temp, "hello.dpk", "hello", "1", Some("hello"));
    let mut store = manager(&temp);
    store.bin_dir = Some(bin.clone());
    let mut skip = options(false);
    skip.no_desktop = true;
    store.install_package(&path, skip).unwrap();
    assert!(!bin.join("hello").exists());
}

#[test]
fn no_desktop_flag_skips_launcher_entries() {
    let temp = TempDir::new().unwrap();
    let path = package(&temp, "hello.dpk", "hello", "1.0.0", Some("hello"));
    let mut store = manager(&temp);
    let mut skip = options(false);
    skip.no_desktop = true;
    store.install_package(&path, skip).unwrap();
    assert!(!temp.path().join("applications/dev-hello.desktop").exists());
}

#[test]
fn extension_kind_packages_get_no_launcher_entries() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("extension-package.dpk");
    let manifest = serde_json::to_vec(&serde_json::json!({
        "name": "widget", "version": "1.0.0", "kind": "extension",
        "bin": {"widget": "bin/widget"}
    }))
    .unwrap();
    archive(
        &path,
        &[
            ("package.json", &manifest, 0o644, tar::EntryType::Regular),
            ("bin/widget", b"#!/bin/sh\n", 0o755, tar::EntryType::Regular),
        ],
    );
    let mut store = manager(&temp);
    store.install_package(&path, options(false)).unwrap();
    assert!(!temp.path().join("applications/dev-widget.desktop").exists());
}

#[test]
fn manifestless_archives_register_gui_launchers() {
    for filename in ["viewer.dpk", "viewer.tar.gz"] {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join(filename);
        archive(&path, &[
            ("opt/viewer/viewer", b"#!/bin/sh\n", 0o755, tar::EntryType::Regular),
            ("usr/share/applications/viewer.desktop", b"[Desktop Entry]\nName=Image Viewer\nExec=/opt/viewer/viewer %U\nIcon=viewer\n", 0o644, tar::EntryType::Regular),
            ("usr/share/icons/hicolor/48x48/apps/viewer.png", b"icon", 0o644, tar::EntryType::Regular),
        ]);
        let mut store = manager(&temp);
        let snapshot = store.install_package(&path, options(false)).unwrap();
        assert_eq!(snapshot.manifest.bin["viewer"], "opt/viewer/viewer");
        let entry =
            fs::read_to_string(temp.path().join("applications/dev-viewer.desktop")).unwrap();
        assert!(entry.contains("Name=Image Viewer\n"));
        assert!(entry.contains(" --root "));
        assert!(entry.contains("/viewer.png\n") || entry.contains("\\viewer.png\n"));
        assert!(entry.contains(" run viewer\n"));
        store.verify(None).unwrap();
        store.remove("viewer").unwrap();
        assert!(!temp.path().join("applications/dev-viewer.desktop").exists());
    }
}
