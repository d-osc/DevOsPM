// Debian (.deb) and RPM (.rpm) support. Both formats are unwrapped into a plain
// tar stream that the shared hardened extractor in manager.rs consumes, so path
// validation, size limits, hashing and transactional install behave exactly as
// they do for .dpk and .tar.gz. Nothing is run: maintainer scripts and
// dependencies are ignored, and symlinks/special files are skipped.
use crate::manager::{error, identifier, FileRecord, Result};
use flate2::read::MultiGzDecoder;
use ruzstd::decoding::{FrameDecoder, StreamingDecoder};
use std::{
    collections::BTreeMap,
    fs::File,
    io::{self, BufRead, BufReader, Read, Seek, SeekFrom},
    path::Path,
};

const MAX_META: u64 = 1024 * 1024;
const XZ_MEMORY_LIMIT_KB: u32 = 512 * 1024;
const RPM_MAX_INDEX: u32 = 0xffff;
const RPM_MAX_STORE: u32 = 64 * 1024 * 1024;
const COMMAND_DIRECTORIES: [&str; 6] = [
    "usr/local/bin",
    "usr/local/sbin",
    "usr/bin",
    "usr/sbin",
    "bin",
    "sbin",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Deb,
    Rpm,
}

impl Kind {
    pub(crate) fn of(path: &Path) -> Option<Self> {
        let name = path.file_name()?.to_str()?;
        if name.ends_with(".deb") {
            Some(Self::Deb)
        } else if name.ends_with(".rpm") {
            Some(Self::Rpm)
        } else {
            None
        }
    }
}

// Metadata read from the package header, already reduced to what Dev OS accepts.
#[derive(Clone, Debug, Default)]
pub(crate) struct Meta {
    pub name: String,
    pub version: String,
    pub description: String,
    // Symlinks, hard links and special files dropped by the converter.
    pub skipped: usize,
}

pub(crate) struct Source {
    pub reader: Box<dyn Read>,
    pub meta: Meta,
}

// Open the payload of a native package as a tar stream. `scratch` is the store
// staging directory, used for the temporary tar built from an RPM cpio payload.
pub(crate) fn open(
    path: &Path,
    kind: Kind,
    scratch: &Path,
    max_bytes: u64,
    max_files: usize,
) -> Result<Source> {
    match kind {
        Kind::Deb => open_deb(path),
        Kind::Rpm => open_rpm(path, scratch, max_bytes, max_files),
    }
}

// Commands are the executables in the standard bin directories, matching what
// `dev run` can export; earlier directories win when a name appears twice.
pub(crate) fn detect_commands(files: &BTreeMap<String, FileRecord>) -> BTreeMap<String, String> {
    let mut commands = BTreeMap::new();
    for directory in COMMAND_DIRECTORIES {
        for (path, record) in files {
            if !record.executable {
                continue;
            }
            if let Some((parent, base)) = path.rsplit_once('/') {
                if parent == directory && identifier(base) {
                    commands
                        .entry(base.to_owned())
                        .or_insert_with(|| path.clone());
                }
            }
        }
    }
    commands
}

fn package_name(raw: &str) -> String {
    raw.trim()
        .chars()
        .map(|c| {
            let c = c.to_ascii_lowercase();
            if c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' {
                c
            } else {
                '-'
            }
        })
        .take(64)
        .collect()
}

// Debian epochs (1:2.0) and tildes are not valid in Dev OS versions.
fn package_version(raw: &str) -> String {
    raw.trim()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || "._+-".contains(c) {
                c
            } else {
                '.'
            }
        })
        .take(128)
        .collect()
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

// Pick a decompressor from the stream's magic bytes. Anything unrecognised is
// passed through as-is (an uncompressed tar or cpio).
fn decompress(reader: impl Read + 'static) -> Result<Box<dyn Read>> {
    let mut buffered = BufReader::with_capacity(128 * 1024, reader);
    let head = buffered.fill_buf()?;
    if head.starts_with(&[0x1f, 0x8b]) {
        Ok(Box::new(MultiGzDecoder::new(buffered)))
    } else if head.starts_with(&[0xfd, b'7', b'z', b'X', b'Z', 0]) {
        Ok(Box::new(lzma_rust2::XzReader::new_mem_limit(
            buffered,
            true,
            XZ_MEMORY_LIMIT_KB,
        )))
    } else if head.starts_with(&[0x28, 0xb5, 0x2f, 0xfd]) {
        Ok(Box::new(ZstdFrames::new(buffered)?))
    } else if head.starts_with(b"BZh") {
        Err(error("bzip2-compressed packages are not supported"))
    } else {
        Ok(Box::new(buffered))
    }
}

// ruzstd decodes one frame per decoder; encoders such as pzstd emit several.
enum ZstdState<R: Read> {
    Frame(Box<StreamingDecoder<BufReader<R>, FrameDecoder>>),
    Done,
}

struct ZstdFrames<R: Read> {
    state: ZstdState<R>,
}

impl<R: Read> ZstdFrames<R> {
    fn new(source: BufReader<R>) -> Result<Self> {
        Ok(Self {
            state: ZstdState::Frame(Box::new(StreamingDecoder::new(source)?)),
        })
    }
}

impl<R: Read> Read for ZstdFrames<R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        loop {
            let ZstdState::Frame(decoder) = &mut self.state else {
                return Ok(0);
            };
            let count = decoder.read(buffer)?;
            if count > 0 || buffer.is_empty() {
                return Ok(count);
            }
            let ZstdState::Frame(decoder) = std::mem::replace(&mut self.state, ZstdState::Done)
            else {
                return Ok(0);
            };
            let mut source = decoder.into_parts().0;
            if source.fill_buf()?.is_empty() {
                return Ok(0);
            }
            self.state = ZstdState::Frame(Box::new(
                StreamingDecoder::new(source).map_err(io::Error::other)?,
            ));
        }
    }
}

// ---------------------------------------------------------------- Debian ----

fn open_deb(path: &Path) -> Result<Source> {
    let mut file = File::open(path)?;
    let length = file.metadata()?.len();
    let mut magic = [0u8; 8];
    file.read_exact(&mut magic)
        .map_err(|_| error("Not a Debian package"))?;
    if &magic != b"!<arch>\n" {
        return Err(error("Not a Debian package: missing ar header"));
    }
    let mut control = None;
    let mut data = None;
    let mut position = 8u64;
    while position + 60 <= length {
        file.seek(SeekFrom::Start(position))?;
        let mut header = [0u8; 60];
        file.read_exact(&mut header)?;
        if &header[58..60] != b"`\n" {
            return Err(error("Corrupt ar member header in Debian package"));
        }
        let name = text(&header[..16]);
        let name = name.trim_end().trim_end_matches('/');
        let size: u64 = text(&header[48..58])
            .trim()
            .parse()
            .map_err(|_| error("Corrupt ar member size in Debian package"))?;
        let start = position + 60;
        if start.checked_add(size).is_none_or(|end| end > length) {
            return Err(error("Truncated Debian package"));
        }
        if name.starts_with("control.tar") && control.is_none() {
            control = Some((start, size));
        } else if name.starts_with("data.tar") && data.is_none() {
            data = Some((start, size));
        }
        position = start + size + (size & 1);
    }
    let (control, data) = control
        .zip(data)
        .ok_or_else(|| error("Debian package needs control.tar and data.tar members"))?;
    let meta = read_control(member(path, control.0, control.1)?)?;
    Ok(Source {
        reader: member(path, data.0, data.1)?,
        meta,
    })
}

fn member(path: &Path, start: u64, size: u64) -> Result<Box<dyn Read>> {
    let mut file = File::open(path)?;
    file.seek(SeekFrom::Start(start))?;
    decompress(file.take(size))
}

fn read_control(reader: impl Read) -> Result<Meta> {
    let mut archive = tar::Archive::new(reader);
    for entry in archive.entries()? {
        let mut entry = entry?;
        let is_control = entry
            .path()?
            .to_string_lossy()
            .trim_start_matches("./")
            == "control";
        if !is_control {
            continue;
        }
        let mut content = String::new();
        (&mut entry).take(MAX_META).read_to_string(&mut content)?;
        return Ok(parse_control(&content));
    }
    Err(error("Debian package has no control file"))
}

fn parse_control(content: &str) -> Meta {
    let mut meta = Meta::default();
    for line in content.lines() {
        if line.starts_with([' ', '\t']) {
            continue;
        }
        match line.split_once(':') {
            Some(("Package", value)) => meta.name = package_name(value),
            Some(("Version", value)) => meta.version = package_version(value),
            Some(("Description", value)) => meta.description = value.trim().to_owned(),
            _ => (),
        }
    }
    meta
}

// ------------------------------------------------------------------- RPM ----

struct RpmHeader {
    index: Vec<(u32, u32, u32)>, // tag, type, offset
    store: Vec<u8>,
}

impl RpmHeader {
    fn string(&self, tag: u32) -> Option<String> {
        let (_, kind, offset) = *self.index.iter().find(|entry| entry.0 == tag)?;
        if kind != 6 && kind != 9 {
            return None;
        }
        let rest = self.store.get(offset as usize..)?;
        let end = rest.iter().position(|&b| b == 0)?;
        Some(text(&rest[..end]))
    }
}

fn read_rpm_header(reader: &mut impl Read, align: bool) -> Result<RpmHeader> {
    let mut head = [0u8; 16];
    reader
        .read_exact(&mut head)
        .map_err(|_| error("Truncated RPM header"))?;
    if head[..4] != [0x8e, 0xad, 0xe8, 0x01] {
        return Err(error("Corrupt RPM header magic"));
    }
    let count = u32::from_be_bytes(head[8..12].try_into().unwrap());
    let size = u32::from_be_bytes(head[12..16].try_into().unwrap());
    if count > RPM_MAX_INDEX || size > RPM_MAX_STORE {
        return Err(error("RPM header exceeds sanity limits"));
    }
    let mut raw = vec![0u8; count as usize * 16];
    reader.read_exact(&mut raw)?;
    let index = raw
        .chunks_exact(16)
        .map(|entry| {
            (
                u32::from_be_bytes(entry[0..4].try_into().unwrap()),
                u32::from_be_bytes(entry[4..8].try_into().unwrap()),
                u32::from_be_bytes(entry[8..12].try_into().unwrap()),
            )
        })
        .collect();
    let mut store = vec![0u8; size as usize];
    reader.read_exact(&mut store)?;
    if align {
        // The signature header is padded to an 8-byte boundary.
        let padding = (8 - (16 + u64::from(count) * 16 + u64::from(size)) % 8) % 8;
        io::copy(&mut reader.take(padding), &mut io::sink())?;
    }
    Ok(RpmHeader { index, store })
}

fn open_rpm(path: &Path, scratch: &Path, max_bytes: u64, max_files: usize) -> Result<Source> {
    let mut reader = BufReader::with_capacity(128 * 1024, File::open(path)?);
    let mut lead = [0u8; 96];
    reader
        .read_exact(&mut lead)
        .map_err(|_| error("Not an RPM package"))?;
    if lead[..4] != [0xed, 0xab, 0xee, 0xdb] {
        return Err(error("Not an RPM package: bad lead magic"));
    }
    read_rpm_header(&mut reader, true)?;
    let header = read_rpm_header(&mut reader, false)?;
    let mut meta = Meta::default();
    if let Some(name) = header.string(1000) {
        meta.name = package_name(&name);
    }
    if let Some(version) = header.string(1001) {
        meta.version = match header.string(1002) {
            Some(release) => package_version(&format!("{version}-{release}")),
            None => package_version(&version),
        };
    }
    if let Some(summary) = header.string(1004) {
        meta.description = summary.trim().to_owned();
    }
    let scratch_file = tempfile::tempfile_in(scratch)?;
    let (mut tar_file, skipped) =
        cpio_to_tar(decompress(reader)?, scratch_file, max_bytes, max_files)?;
    tar_file.seek(SeekFrom::Start(0))?;
    meta.skipped = skipped;
    Ok(Source {
        reader: Box::new(tar_file),
        meta,
    })
}

fn cpio_field(header: &[u8], index: usize) -> Result<u64> {
    let start = 6 + index * 8;
    std::str::from_utf8(&header[start..start + 8])
        .ok()
        .and_then(|digits| u64::from_str_radix(digits, 16).ok())
        .ok_or_else(|| error("Corrupt cpio header in RPM payload"))
}

fn skip(reader: &mut impl Read, count: u64) -> Result<()> {
    if io::copy(&mut reader.take(count), &mut io::sink())? != count {
        return Err(error("Truncated RPM payload"));
    }
    Ok(())
}

// Rewrite a newc cpio payload as a tar. Directories and regular files are kept;
// RPM stores a hard-linked file's data on its last entry, so earlier links are
// held back and written as copies once the data arrives.
fn cpio_to_tar(
    mut source: impl Read,
    output: File,
    max_bytes: u64,
    max_files: usize,
) -> Result<(File, usize)> {
    let mut builder = tar::Builder::new(output);
    let mut pending: BTreeMap<(u64, u64, u64), Vec<(String, u64)>> = BTreeMap::new();
    let (mut total, mut entries, mut skipped) = (0u64, 0usize, 0usize);
    loop {
        let mut head = [0u8; 110];
        source
            .read_exact(&mut head)
            .map_err(|_| error("Truncated RPM payload"))?;
        if &head[..6] != b"070701" && &head[..6] != b"070702" {
            return Err(error("Unsupported RPM payload format (expected newc cpio)"));
        }
        let mode = cpio_field(&head, 1)?;
        let links = cpio_field(&head, 4)?;
        let size = cpio_field(&head, 6)?;
        let inode = (
            cpio_field(&head, 0)?,
            cpio_field(&head, 7)?,
            cpio_field(&head, 8)?,
        );
        let name_length = cpio_field(&head, 11)?;
        if name_length == 0 || name_length > 4096 {
            return Err(error("Invalid file name length in RPM payload"));
        }
        let mut name = vec![0u8; name_length as usize];
        source.read_exact(&mut name)?;
        skip(&mut source, (4 - (110 + name_length) % 4) % 4)?;
        if name.last() == Some(&0) {
            name.pop();
        }
        let name = String::from_utf8(name).map_err(|_| error("Package paths must be UTF-8"))?;
        if name == "TRAILER!!!" {
            break;
        }
        entries += 1;
        if entries > max_files {
            return Err(error("Archive has too many entries"));
        }
        let padding = (4 - size % 4) % 4;
        let path = name.trim_start_matches("./").trim_start_matches('/');
        let kind = mode & 0o170000;
        if path.is_empty() || (kind != 0o100000 && kind != 0o040000) {
            if path.is_empty() && kind == 0o040000 {
                continue;
            }
            skip(&mut source, size + padding)?;
            skipped += 1;
            continue;
        }
        let mut header = tar::Header::new_gnu();
        header.set_mode((mode & 0o7777) as u32);
        header.set_mtime(0);
        if kind == 0o040000 {
            header.set_entry_type(tar::EntryType::Directory);
            header.set_size(0);
            builder.append_data(&mut header, path, io::empty())?;
            skip(&mut source, size + padding)?;
            continue;
        }
        if links > 1 && size == 0 {
            pending
                .entry(inode)
                .or_default()
                .push((path.to_owned(), mode));
            continue;
        }
        let copies = pending.remove(&inode).unwrap_or_default();
        total = total
            .checked_add(size * (1 + copies.len() as u64))
            .ok_or_else(|| error("Archive is too large"))?;
        if total > max_bytes {
            return Err(error("Archive exceeds the extracted-size limit"));
        }
        header.set_entry_type(tar::EntryType::Regular);
        header.set_size(size);
        if copies.is_empty() {
            let mut limited = (&mut source).take(size);
            builder.append_data(&mut header, path, &mut limited)?;
            if limited.limit() != 0 {
                return Err(error("Truncated RPM payload"));
            }
        } else {
            let mut data = Vec::new();
            (&mut source).take(size).read_to_end(&mut data)?;
            if data.len() as u64 != size {
                return Err(error("Truncated RPM payload"));
            }
            for (copy, copy_mode) in copies {
                let mut copy_header = header.clone();
                copy_header.set_mode((copy_mode & 0o7777) as u32);
                builder.append_data(&mut copy_header, &copy, &data[..])?;
            }
            builder.append_data(&mut header, path, &data[..])?;
        }
        skip(&mut source, padding)?;
    }
    if !pending.is_empty() {
        return Err(error("RPM hard link has no data entry"));
    }
    Ok((builder.into_inner()?, skipped))
}

#[cfg(test)]
mod tests;
