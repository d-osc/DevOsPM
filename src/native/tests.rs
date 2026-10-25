use super::*;
use crate::manager::{InstallOptions, InstallOutcome, Manager, Snapshot};
use flate2::{write::GzEncoder, Compression};
use std::{fs, io::Write, path::PathBuf};
use tempfile::TempDir;

type Compressor = fn(&[u8]) -> Vec<u8>;

fn gzip(data: &[u8]) -> Vec<u8> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::fast());
    encoder.write_all(data).unwrap();
    encoder.finish().unwrap()
}

fn xz(data: &[u8]) -> Vec<u8> {
    let mut writer =
        lzma_rust2::XzWriter::new(Vec::new(), lzma_rust2::XzOptions::with_preset(1)).unwrap();
    writer.write_all(data).unwrap();
    writer.finish().unwrap()
}

fn zstd(data: &[u8]) -> Vec<u8> {
    ruzstd::encoding::compress_to_vec(data, ruzstd::encoding::CompressionLevel::Fastest)
}

fn plain(data: &[u8]) -> Vec<u8> {
    data.to_vec()
}

fn tar(entries: &[(&str, &[u8], u32, tar::EntryType)]) -> Vec<u8> {
    let mut builder = tar::Builder::new(Vec::new());
    for (name, data, mode, kind) in entries {
        let mut header = tar::Header::new_gnu();
        header.set_size(data.len() as u64);
        header.set_mode(*mode);
        header.set_entry_type(*kind);
        if kind.is_symlink() {
            header.set_link_name("target").unwrap();
        }
        builder.append_data(&mut header, name, *data).unwrap();
    }
    builder.into_inner().unwrap()
}

fn ar_member(out: &mut Vec<u8>, name: &str, data: &[u8]) {
    out.extend(
        format!(
            "{name:<16}{:<12}{:<6}{:<6}{:<8}{:<10}`\n",
            0,
            0,
            0,
            "100644",
            data.len()
        )
        .as_bytes(),
    );
    out.extend(data);
    if data.len() % 2 == 1 {
        out.push(b'\n');
    }
}

const CONTROL: &str = "Package: Hello-Tool\nVersion: 1:2.3-1~ubuntu1\nArchitecture: amd64\nDescription: Says hello\n A longer description.\n";

fn data_tar() -> Vec<u8> {
    tar(&[
        ("./", b"", 0o755, tar::EntryType::Directory),
        ("./usr/", b"", 0o755, tar::EntryType::Directory),
        ("./usr/bin/", b"", 0o755, tar::EntryType::Directory),
        (
            "./usr/bin/hello-tool",
            b"#!/bin/sh\necho hi\n",
            0o755,
            tar::EntryType::Regular,
        ),
        ("./usr/bin/hello-alias", b"", 0o777, tar::EntryType::Symlink),
        (
            "./usr/share/doc/hello/copyright",
            b"MIT",
            0o644,
            tar::EntryType::Regular,
        ),
    ])
}

fn control_tar() -> Vec<u8> {
    tar(&[(
        "./control",
        CONTROL.as_bytes(),
        0o644,
        tar::EntryType::Regular,
    )])
}

fn deb(dir: &TempDir, compress: Compressor, suffix: &str) -> PathBuf {
    let mut out = b"!<arch>\n".to_vec();
    ar_member(&mut out, "debian-binary", b"2.0\n");
    ar_member(
        &mut out,
        &format!("control.tar{suffix}"),
        &compress(&control_tar()),
    );
    ar_member(
        &mut out,
        &format!("data.tar{suffix}"),
        &compress(&data_tar()),
    );
    let path = dir.path().join("hello-tool_2.3_amd64.deb");
    fs::write(&path, out).unwrap();
    path
}

fn manager(dir: &TempDir) -> Manager {
    Manager::open_with(
        Some(dir.path().join("root")),
        Some(dir.path().join("devos")),
        Some(dir.path().join("apps")),
        true,
    )
    .unwrap()
}

fn options() -> InstallOptions {
    InstallOptions {
        name: None,
        version: None,
        sha256: None,
        replace: false,
        no_desktop: false,
        source_repository: None,
    }
}

fn install(dir: &TempDir, path: &Path, options: InstallOptions) -> Box<Snapshot> {
    match manager(dir).install(path, options).unwrap() {
        InstallOutcome::Package(snapshot) => snapshot,
        InstallOutcome::Extension(_) => panic!("expected a package"),
    }
}

#[test]
fn deb_formats_install_and_verify() {
    for (compress, suffix) in [
        (gzip as Compressor, ".gz"),
        (xz, ".xz"),
        (zstd, ".zst"),
        (plain, ""),
    ] {
        let dir = TempDir::new().unwrap();
        let path = deb(&dir, compress, suffix);
        let snapshot = install(&dir, &path, options());
        assert_eq!(snapshot.manifest.name, "hello-tool", "{suffix}");
        assert_eq!(snapshot.manifest.version, "1.2.3-1.ubuntu1");
        assert_eq!(snapshot.manifest.description, "Says hello");
        assert_eq!(
            snapshot.manifest.bin.get("hello-tool").map(String::as_str),
            Some("usr/bin/hello-tool")
        );
        assert!(snapshot.files.contains_key("usr/share/doc/hello/copyright"));
        // The symlink is dropped rather than recreated.
        assert!(!snapshot.files.contains_key("usr/bin/hello-alias"));
        assert!(snapshot.files["usr/bin/hello-tool"].executable);
        let launcher = fs::read_to_string(dir.path().join("apps/dev-hello-tool.desktop")).unwrap();
        assert!(launcher.contains("Icon=application-x-executable\n"));
        assert!(launcher.contains(" run hello-tool\n"));
        manager(&dir).verify(Some("hello-tool")).unwrap();
    }
}

#[test]
fn name_and_version_flags_override_deb_control() {
    let dir = TempDir::new().unwrap();
    let path = deb(&dir, gzip, ".gz");
    let snapshot = install(
        &dir,
        &path,
        InstallOptions {
            name: Some("renamed".into()),
            version: Some("9.9.9".into()),
            ..options()
        },
    );
    assert_eq!(snapshot.manifest.name, "renamed");
    assert_eq!(snapshot.manifest.version, "9.9.9");
}

#[test]
fn multi_frame_zstd_is_fully_decoded() {
    let mut joined = zstd(b"first frame, ");
    joined.extend(zstd(b"second frame"));
    let mut output = Vec::new();
    decompress(io::Cursor::new(joined))
        .unwrap()
        .read_to_end(&mut output)
        .unwrap();
    assert_eq!(output, b"first frame, second frame");
}

#[test]
fn corrupt_and_unsupported_debs_are_rejected() {
    let dir = TempDir::new().unwrap();
    let good = fs::read(deb(&dir, gzip, ".gz")).unwrap();
    for (name, bytes) in [
        ("truncated.deb", good[..good.len() - 40].to_vec()),
        ("garbage.deb", b"not an ar archive".to_vec()),
    ] {
        let path = dir.path().join(name);
        fs::write(&path, bytes).unwrap();
        assert!(manager(&dir).install(&path, options()).is_err(), "{name}");
    }
    let mut bzip = b"!<arch>\n".to_vec();
    ar_member(&mut bzip, "control.tar.gz", &gzip(&control_tar()));
    ar_member(&mut bzip, "data.tar.bz2", b"BZh91AY&SY");
    let path = dir.path().join("bzip.deb");
    fs::write(&path, bzip).unwrap();
    assert!(manager(&dir).install(&path, options()).is_err());
}

fn cpio_entry(out: &mut Vec<u8>, name: &str, mode: u32, links: u32, inode: u32, data: &[u8]) {
    let name = format!("{name}\0");
    out.extend(
        format!(
            "070701{inode:08x}{mode:08x}{:08x}{:08x}{links:08x}{:08x}{:08x}{:08x}{:08x}{:08x}{:08x}{:08x}{:08x}",
            0,
            0,
            0,
            data.len(),
            0,
            0,
            0,
            0,
            name.len(),
            0
        )
        .as_bytes(),
    );
    out.extend(name.as_bytes());
    out.resize(out.len().next_multiple_of(4), 0);
    out.extend(data);
    out.resize(out.len().next_multiple_of(4), 0);
}

fn rpm_header(entries: &[(u32, u32, &str)]) -> Vec<u8> {
    let mut store = Vec::new();
    let mut index = Vec::new();
    for (tag, kind, value) in entries {
        index.extend(tag.to_be_bytes());
        index.extend(kind.to_be_bytes());
        index.extend((store.len() as u32).to_be_bytes());
        index.extend(1u32.to_be_bytes());
        store.extend(value.as_bytes());
        store.push(0);
    }
    let mut out = vec![0x8e, 0xad, 0xe8, 0x01, 0, 0, 0, 0];
    out.extend((entries.len() as u32).to_be_bytes());
    out.extend((store.len() as u32).to_be_bytes());
    out.extend(index);
    out.extend(store);
    out
}

fn rpm(dir: &TempDir, compress: Compressor) -> PathBuf {
    let mut cpio = Vec::new();
    cpio_entry(&mut cpio, "./usr", 0o040755, 1, 1, b"");
    cpio_entry(&mut cpio, "./usr/bin", 0o040755, 1, 2, b"");
    cpio_entry(
        &mut cpio,
        "./usr/bin/hello-tool",
        0o100755,
        1,
        3,
        b"#!/bin/sh\necho hi\n",
    );
    // Two hard links: RPM stores the data with the last entry only.
    cpio_entry(&mut cpio, "./usr/share/a.txt", 0o100644, 2, 4, b"");
    cpio_entry(&mut cpio, "./usr/share/b.txt", 0o100644, 2, 4, b"shared");
    cpio_entry(
        &mut cpio,
        "./usr/bin/hello-alias",
        0o120777,
        1,
        5,
        b"hello-tool",
    );
    cpio_entry(&mut cpio, "TRAILER!!!", 0, 1, 0, b"");
    let mut out = vec![0xed, 0xab, 0xee, 0xdb];
    out.resize(96, 0);
    // The signature header is padded to 8 bytes, which the reader must honour.
    out.extend(rpm_header(&[(62, 6, "sig")]));
    out.resize(out.len().next_multiple_of(8), 0);
    out.extend(rpm_header(&[
        (1000, 6, "hello-tool"),
        (1001, 6, "2.3"),
        (1002, 6, "4.fc40"),
        (1004, 9, "Says hello"),
    ]));
    out.extend(compress(&cpio));
    let path = dir.path().join("hello-tool-2.3-4.fc40.x86_64.rpm");
    fs::write(&path, out).unwrap();
    path
}

#[test]
fn rpm_formats_install_with_hard_links_and_metadata() {
    for compress in [gzip as Compressor, xz, zstd] {
        let dir = TempDir::new().unwrap();
        let path = rpm(&dir, compress);
        let snapshot = install(&dir, &path, options());
        assert_eq!(snapshot.manifest.name, "hello-tool");
        assert_eq!(snapshot.manifest.version, "2.3-4.fc40");
        assert_eq!(snapshot.manifest.description, "Says hello");
        assert_eq!(
            snapshot.manifest.bin.get("hello-tool").map(String::as_str),
            Some("usr/bin/hello-tool")
        );
        assert_eq!(snapshot.files["usr/share/a.txt"].size, 6);
        assert_eq!(snapshot.files["usr/share/b.txt"].size, 6);
        assert!(!snapshot.files.contains_key("usr/bin/hello-alias"));
        manager(&dir).verify(Some("hello-tool")).unwrap();
    }
}

#[test]
fn corrupt_rpm_is_rejected() {
    let dir = TempDir::new().unwrap();
    let good = fs::read(rpm(&dir, gzip)).unwrap();
    let mut bad_header = good.clone();
    bad_header[96] ^= 0xff;
    for (name, bytes) in [
        ("short.rpm", good[..60].to_vec()),
        ("bad-header.rpm", bad_header),
        ("truncated.rpm", good[..good.len() - 12].to_vec()),
    ] {
        let path = dir.path().join(name);
        fs::write(&path, bytes).unwrap();
        assert!(manager(&dir).install(&path, options()).is_err(), "{name}");
    }
}
