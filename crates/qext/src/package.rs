//! Extension package (`.sflx`) reading, integrity checking and safe extraction.
//!
//! A package is a plain ZIP. That "plain ZIP" is exactly why this module is
//! strict: extraction is the one place where hostile input reaches the
//! filesystem, so every entry is checked against a limit before a single byte
//! is written.
//!
//! Order of operations in [`extract_into`]:
//! 1. validate the whole archive (names, limits, symlinks, duplicates)
//! 2. require `manifest.json` and `index.js` at the root
//! 3. only then write files
//!
//! A package that fails validation never creates a file.

use crate::manifest::{Manifest, ManifestError};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::{Cursor, Read, Write};
use std::path::{Path, PathBuf};
use thiserror::Error;
use zip::ZipArchive;

/// Entry names that must exist at the archive root.
pub const REQUIRED_ROOT_ENTRIES: [&str; 2] = ["manifest.json", "index.js"];

#[derive(Debug, Error)]
pub enum PackageError {
    #[error("cannot read package {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("package is not a readable ZIP archive: {0}")]
    Zip(#[from] zip::result::ZipError),
    #[error("package structure is malformed: {0}")]
    MalformedArchive(String),
    #[error("package is {size} bytes, above the {limit} byte archive limit")]
    ArchiveTooLarge { size: u64, limit: u64 },
    #[error("package has {count} entries, above the {limit} entry limit")]
    TooManyEntries { count: usize, limit: usize },
    #[error("entry {entry:?} has an unsafe path")]
    UnsafeEntryName { entry: String },
    #[error("entry {entry:?} is {size} bytes, above the {limit} byte entry limit")]
    EntryTooLarge {
        entry: String,
        size: u64,
        limit: u64,
    },
    #[error("package expands to {size} bytes, above the {limit} byte total limit")]
    ArchiveExpansionTooLarge { size: u64, limit: u64 },
    #[error("entry {entry:?} compresses {ratio}:1, above the {limit}:1 ratio limit")]
    CompressionRatioExceeded {
        entry: String,
        ratio: u64,
        limit: u64,
    },
    #[error("entry {entry:?} is a symlink; packages may not contain links")]
    SymlinkEntry { entry: String },
    #[error("entry {entry:?} appears more than once in the archive")]
    DuplicateEntry { entry: String },
    #[error("package is missing required root entry {name:?}")]
    MissingEntry { name: &'static str },
    #[error("package integrity check failed: expected {expected}, got {actual}")]
    HashMismatch { expected: String, actual: String },
    #[error("package integrity check is {actual}, expected {expected}")]
    HashMismatchReported { expected: String, actual: String },
    #[error("invalid manifest: {0}")]
    Manifest(#[from] ManifestError),
}

/// Archive limits. Injectable so tests can exercise the rules with small
/// inputs instead of gigabyte fixtures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArchiveLimits {
    pub max_archive_bytes: u64,
    pub max_entries: usize,
    pub max_entry_bytes: u64,
    pub max_total_bytes: u64,
    pub max_compression_ratio: u64,
    /// Ratios are only enforced above this uncompressed size: tiny files
    /// legitimately compress hundreds to one.
    pub ratio_check_floor_bytes: u64,
}

impl Default for ArchiveLimits {
    fn default() -> Self {
        Self {
            max_archive_bytes: 256 * 1024 * 1024,
            max_entries: 4096,
            max_entry_bytes: 64 * 1024 * 1024,
            max_total_bytes: 256 * 1024 * 1024,
            max_compression_ratio: 200,
            ratio_check_floor_bytes: 1024 * 1024,
        }
    }
}

/// What a validated package contains, before or after extraction.
#[derive(Debug, Clone, PartialEq)]
pub struct PackageInfo {
    pub sha256: String,
    pub entry_names: Vec<String>,
    pub manifest: Manifest,
    pub index_js: String,
}

/// Lowercase hex SHA-256 of `bytes`.
pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

impl PackageInfo {
    /// Read and validate a package without touching the filesystem beyond the
    /// source file itself.
    pub fn inspect(path: &Path) -> Result<Self, PackageError> {
        Self::inspect_with(path, &ArchiveLimits::default())
    }

    pub fn inspect_with(path: &Path, limits: &ArchiveLimits) -> Result<Self, PackageError> {
        let bytes = read_archive_bytes(path, limits)?;
        let sha256 = sha256_hex(&bytes);
        let mut archive = ZipArchive::new(Cursor::new(&bytes))?;

        let entry_names = validate_entries(&bytes, &mut archive, limits)?;
        let manifest_json = read_entry(&mut archive, "manifest.json")?;
        let index_js = read_entry(&mut archive, "index.js")?;
        let manifest = Manifest::parse(&manifest_json)?;

        Ok(Self {
            sha256,
            entry_names,
            manifest,
            index_js,
        })
    }

    /// Check a package against a known digest before doing anything else.
    pub fn verify_sha256(path: &Path, expected: &str) -> Result<String, PackageError> {
        let bytes = read_archive_bytes(path, &ArchiveLimits::default())?;
        let actual = sha256_hex(&bytes);
        if !actual.eq_ignore_ascii_case(expected) {
            return Err(PackageError::HashMismatch {
                expected: expected.to_ascii_lowercase(),
                actual,
            });
        }
        Ok(actual)
    }

    /// Validate, then extract into `destination`.
    ///
    /// `destination` must not already contain package files. On failure the
    /// files written by this call are removed; the directory itself is left to
    /// the caller, which normally owns a temporary directory.
    pub fn extract_into(path: &Path, destination: &Path) -> Result<Self, PackageError> {
        Self::extract_into_with(path, destination, &ArchiveLimits::default())
    }

    pub fn extract_into_with(
        path: &Path,
        destination: &Path,
        limits: &ArchiveLimits,
    ) -> Result<Self, PackageError> {
        // Validate fully before writing anything.
        let bytes = read_archive_bytes(path, limits)?;
        let sha256 = sha256_hex(&bytes);
        let mut archive = ZipArchive::new(Cursor::new(&bytes))?;
        let entry_names = validate_entries(&bytes, &mut archive, limits)?;

        let manifest_json = read_entry(&mut archive, "manifest.json")?;
        let index_js = read_entry(&mut archive, "index.js")?;
        let manifest = Manifest::parse(&manifest_json)?;

        fs::create_dir_all(destination).map_err(|source| PackageError::Io {
            path: destination.to_path_buf(),
            source,
        })?;

        let mut written: Vec<PathBuf> = Vec::new();
        let write_result = extract_all(&mut archive, destination, &mut written);

        if let Err(error) = write_result {
            for relative in written.iter().rev() {
                let _ = fs::remove_file(destination.join(relative));
            }
            return Err(error);
        }

        Ok(Self {
            sha256,
            entry_names,
            manifest,
            index_js,
        })
    }

    /// Read one file back out of the source archive (used for icons).
    ///
    /// Bypasses full package validation, so the entry size limit is applied
    /// here too: an icon is a convenience, never a reason to trust a size.
    pub fn read_file(path: &Path, entry: &str) -> Result<Vec<u8>, PackageError> {
        let limits = ArchiveLimits::default();
        let bytes = read_archive_bytes(path, &limits)?;
        let mut archive = ZipArchive::new(Cursor::new(&bytes))?;
        let reader = archive.by_name(entry)?;

        if reader.size() > limits.max_entry_bytes {
            return Err(PackageError::EntryTooLarge {
                entry: entry.to_string(),
                size: reader.size(),
                limit: limits.max_entry_bytes,
            });
        }

        let mut buffer = Vec::new();
        let mut limited = reader.take(limits.max_entry_bytes + 1);
        limited
            .read_to_end(&mut buffer)
            .map_err(|source| PackageError::Io {
                path: PathBuf::from(entry),
                source,
            })?;

        if buffer.len() as u64 > limits.max_entry_bytes {
            return Err(PackageError::EntryTooLarge {
                entry: entry.to_string(),
                size: buffer.len() as u64,
                limit: limits.max_entry_bytes,
            });
        }

        Ok(buffer)
    }
}

fn read_archive_bytes(path: &Path, limits: &ArchiveLimits) -> Result<Vec<u8>, PackageError> {
    let metadata = fs::metadata(path).map_err(|source| PackageError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    if metadata.len() > limits.max_archive_bytes {
        return Err(PackageError::ArchiveTooLarge {
            size: metadata.len(),
            limit: limits.max_archive_bytes,
        });
    }
    fs::read(path).map_err(|source| PackageError::Io {
        path: path.to_path_buf(),
        source,
    })
}

/// One central-directory record, read straight from the archive bytes.
///
/// The `zip` crate collapses entries with identical names while indexing and
/// normalises Unix modes for some source systems, so neither duplicate nor
/// symlink detection can be trusted from its reader alone. Both are decided
/// here, from the bytes that will actually be extracted.
#[derive(Debug, Clone, PartialEq, Eq)]
struct CentralEntry {
    name: String,
    unix_mode: u32,
}

const EOCD_SIGNATURE: u32 = 0x0605_4b50;
const CENTRAL_SIGNATURE: u32 = 0x0201_4b50;
const EOCD_MIN_LEN: usize = 22;
const MAX_COMMENT_LEN: usize = 0xFFFF;
const ZIP64_MARKER: u32 = 0xFFFF_FFFF;
const SYSTEM_UNIX: u8 = 3;

fn scan_central_directory(bytes: &[u8]) -> Result<Vec<CentralEntry>, PackageError> {
    let malformed = |detail: &str| PackageError::MalformedArchive(detail.to_string());

    if bytes.len() < EOCD_MIN_LEN {
        return Err(malformed("archive is too short to contain an end record"));
    }

    // The end record sits at the tail, followed by an optional comment.
    let search_start = bytes.len().saturating_sub(EOCD_MIN_LEN + MAX_COMMENT_LEN);
    let mut eocd = None;
    for offset in (search_start..=bytes.len() - EOCD_MIN_LEN).rev() {
        if read_u32(bytes, offset) == Some(EOCD_SIGNATURE) {
            eocd = Some(offset);
            break;
        }
    }
    let eocd = eocd.ok_or_else(|| malformed("end of central directory record not found"))?;

    let declared_count = read_u16(bytes, eocd + 10).unwrap_or(0) as usize;
    let directory_offset = read_u32(bytes, eocd + 16)
        .ok_or_else(|| malformed("central directory offset missing"))?;

    if directory_offset == ZIP64_MARKER {
        return Err(malformed("ZIP64 archives are not supported"));
    }

    let mut entries = Vec::with_capacity(declared_count.min(1024));
    let mut cursor = directory_offset as usize;

    for _ in 0..declared_count {
        if read_u32(bytes, cursor) != Some(CENTRAL_SIGNATURE) {
            return Err(malformed("central directory record signature mismatch"));
        }

        let made_by = read_u16(bytes, cursor + 4)
            .ok_or_else(|| malformed("central directory record truncated"))?;
        let external_attributes = read_u32(bytes, cursor + 38)
            .ok_or_else(|| malformed("central directory attributes missing"))?;
        let name_len = read_u16(bytes, cursor + 28)
            .ok_or_else(|| malformed("central directory name length missing"))?
            as usize;
        let extra_len = read_u16(bytes, cursor + 30).unwrap_or(0) as usize;
        let comment_len = read_u16(bytes, cursor + 32).unwrap_or(0) as usize;

        let name_start = cursor + 46;
        let name_end = name_start
            .checked_add(name_len)
            .ok_or_else(|| malformed("entry name length overflow"))?;
        let name_bytes = bytes
            .get(name_start..name_end)
            .ok_or_else(|| malformed("entry name extends past the archive"))?;
        let name = std::str::from_utf8(name_bytes)
            .map_err(|_| malformed("entry name is not valid UTF-8"))?
            .to_string();

        let unix_mode = if (made_by >> 8) as u8 == SYSTEM_UNIX {
            external_attributes >> 16
        } else {
            0
        };

        entries.push(CentralEntry { name, unix_mode });

        cursor = name_end
            .checked_add(extra_len)
            .and_then(|next| next.checked_add(comment_len))
            .ok_or_else(|| malformed("central directory offset overflow"))?;
    }

    Ok(entries)
}

fn read_u16(bytes: &[u8], offset: usize) -> Option<u16> {
    let slice = bytes.get(offset..offset + 2)?;
    Some(u16::from_le_bytes([slice[0], slice[1]]))
}

fn read_u32(bytes: &[u8], offset: usize) -> Option<u32> {
    let slice = bytes.get(offset..offset + 4)?;
    Some(u32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]))
}

/// Check every entry name and size, returning the normalised name list.
fn validate_entries<R: Read + std::io::Seek>(
    bytes: &[u8],
    archive: &mut ZipArchive<R>,
    limits: &ArchiveLimits,
) -> Result<Vec<String>, PackageError> {
    let central = scan_central_directory(bytes)?;

    if central.len() > limits.max_entries {
        return Err(PackageError::TooManyEntries {
            count: central.len(),
            limit: limits.max_entries,
        });
    }

    // Name, duplicate and symlink rules run over the central directory, which
    // still contains records the reader discarded as duplicates.
    let mut seen: Vec<String> = Vec::with_capacity(central.len());
    for entry in &central {
        if entry.name.ends_with('/') {
            validate_name(entry.name.trim_end_matches('/'))?;
            continue;
        }

        let name = validate_name(&entry.name)?;

        if entry.unix_mode & 0o170000 == 0o120000 {
            return Err(PackageError::SymlinkEntry { entry: name });
        }

        let duplicate_key = name.to_ascii_lowercase();
        if seen.contains(&duplicate_key) {
            return Err(PackageError::DuplicateEntry { entry: name });
        }
        seen.push(duplicate_key);
    }

    let mut names = Vec::with_capacity(central.len());
    let mut total: u64 = 0;

    for index in 0..archive.len() {
        let entry = archive.by_index(index)?;
        if entry.is_dir() {
            continue;
        }

        let name = validate_name(&entry.name()?)?;

        let size = entry.size();
        if size > limits.max_entry_bytes {
            return Err(PackageError::EntryTooLarge {
                entry: name,
                size,
                limit: limits.max_entry_bytes,
            });
        }

        let ratio = size / entry.compressed_size().max(1);
        if size > limits.ratio_check_floor_bytes && ratio > limits.max_compression_ratio {
            return Err(PackageError::CompressionRatioExceeded {
                entry: name,
                ratio,
                limit: limits.max_compression_ratio,
            });
        }

        total = total.saturating_add(size);
        if total > limits.max_total_bytes {
            return Err(PackageError::ArchiveExpansionTooLarge {
                size: total,
                limit: limits.max_total_bytes,
            });
        }

        names.push(name);
    }

    for required in REQUIRED_ROOT_ENTRIES {
        if !names.iter().any(|name| name == required) {
            return Err(PackageError::MissingEntry { name: required });
        }
    }

    Ok(names)
}

/// Reject anything that could escape the destination directory.
fn validate_name(raw: &str) -> Result<String, PackageError> {
    let unsafe_entry = || PackageError::UnsafeEntryName {
        entry: raw.to_string(),
    };

    if raw.is_empty() || raw.contains('\0') || raw.contains('\\') {
        return Err(unsafe_entry());
    }

    // Drive-qualified (`C:`) and UNC-style names.
    let bytes = raw.as_bytes();
    if bytes.len() >= 2 && bytes[1] == b':' && bytes[0].is_ascii_alphabetic() {
        return Err(unsafe_entry());
    }

    if raw.starts_with('/') || raw.starts_with('~') {
        return Err(unsafe_entry());
    }

    for segment in raw.split('/') {
        if segment.is_empty() || segment == "." || segment == ".." {
            return Err(unsafe_entry());
        }
        if segment.chars().any(|ch| ch.is_control()) {
            return Err(unsafe_entry());
        }
    }

    Ok(raw.to_string())
}

fn read_entry<R: Read + std::io::Seek>(
    archive: &mut ZipArchive<R>,
    name: &str,
) -> Result<String, PackageError> {
    let mut reader = archive.by_name(name)?;
    let mut buffer = String::new();
    reader
        .read_to_string(&mut buffer)
        .map_err(|source| PackageError::Io {
            path: PathBuf::from(name),
            source,
        })?;
    Ok(buffer)
}

fn extract_all<R: Read + std::io::Seek>(
    archive: &mut ZipArchive<R>,
    destination: &Path,
    written: &mut Vec<PathBuf>,
) -> Result<(), PackageError> {
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index)?;
        if entry.is_dir() {
            continue;
        }

        let name = validate_name(&entry.name()?)?;
        let relative = PathBuf::from(&name);
        let target = destination.join(&relative);

        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(|source| PackageError::Io {
                path: parent.to_path_buf(),
                source,
            })?;
        }

        let mut file = fs::File::create(&target).map_err(|source| PackageError::Io {
            path: target.clone(),
            source,
        })?;
        std::io::copy(&mut entry, &mut file).map_err(|source| PackageError::Io {
            path: target.clone(),
            source,
        })?;
        file.flush().map_err(|source| PackageError::Io {
            path: target.clone(),
            source,
        })?;

        written.push(relative);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use zip::write::SimpleFileOptions;

    const SPOTIFY_SHA: &str = "5ccae8a8c236c97500328a22d69ac039c779179a0c9386e8e3b89e152389dbad";

    fn fixture_archive(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures")
            .join("archives")
            .join(format!("{name}.sflx"))
    }

    fn minimal_manifest() -> &'static str {
        r#"{ "name": "test-ext", "version": "1.0.0", "type": ["metadata_provider"] }"#
    }

    /// Build a zip at `dir/name` from (entry name, contents, unix mode) triples.
    fn build_zip(
        dir: &Path,
        file_name: &str,
        entries: &[(&str, &[u8], Option<u32>)],
    ) -> PathBuf {
        let path = dir.join(file_name);
        let file = fs::File::create(&path).expect("create zip");
        let mut writer = zip::ZipWriter::new(file);

        for (name, contents, mode) in entries {
            let mut options = SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored);
            if let Some(mode) = mode {
                options = options.unix_permissions(*mode);
            }
            writer.start_file(*name, options).expect("start entry");
            writer.write_all(contents).expect("write entry");
        }

        writer.finish().expect("finish zip");
        path
    }

    fn default_entries() -> Vec<(&'static str, &'static [u8], Option<u32>)> {
        vec![
            ("manifest.json", minimal_manifest().as_bytes(), None),
            ("index.js", b"registerExtension({});".as_slice(), None),
        ]
    }

    // The `zip` writer refuses duplicate names and normalises symlink modes, so
    // hostile archives that a writer cannot produce are assembled by hand.
    // Method is Stored and CRCs are real, so the reader accepts them.

    fn crc32(data: &[u8]) -> u32 {
        let mut crc: u32 = 0xFFFF_FFFF;
        for byte in data {
            crc ^= u32::from(*byte);
            for _ in 0..8 {
                let mask = (crc & 1).wrapping_neg();
                crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
            }
        }
        !crc
    }

    fn push_u16(out: &mut Vec<u8>, value: u16) {
        out.extend_from_slice(&value.to_le_bytes());
    }

    fn push_u32(out: &mut Vec<u8>, value: u32) {
        out.extend_from_slice(&value.to_le_bytes());
    }

    /// `mode` is the full 16-bit Unix mode including the file-type bits.
    fn craft_zip(entries: &[(&str, &[u8], u32)]) -> Vec<u8> {
        const SIG_LOCAL: u32 = 0x0403_4b50;
        const SIG_CENTRAL: u32 = 0x0201_4b50;
        const SIG_EOCD: u32 = 0x0605_4b50;
        const VERSION_NEEDED: u16 = 20;
        const MADE_BY_UNIX_V2: u16 = 0x0314;
        const DOS_DATE_1980_01_01: u16 = 0x0021;

        let mut out = Vec::new();
        let mut central = Vec::new();

        for (name, data, mode) in entries {
            let offset = out.len() as u32;
            let crc = crc32(data);
            let name_bytes = name.as_bytes();
            let size = data.len() as u32;

            push_u32(&mut out, SIG_LOCAL);
            push_u16(&mut out, VERSION_NEEDED);
            push_u16(&mut out, 0);
            push_u16(&mut out, 0);
            push_u16(&mut out, 0);
            push_u16(&mut out, DOS_DATE_1980_01_01);
            push_u32(&mut out, crc);
            push_u32(&mut out, size);
            push_u32(&mut out, size);
            push_u16(&mut out, name_bytes.len() as u16);
            push_u16(&mut out, 0);
            out.extend_from_slice(name_bytes);
            out.extend_from_slice(data);

            push_u32(&mut central, SIG_CENTRAL);
            push_u16(&mut central, MADE_BY_UNIX_V2);
            push_u16(&mut central, VERSION_NEEDED);
            push_u16(&mut central, 0);
            push_u16(&mut central, 0);
            push_u16(&mut central, 0);
            push_u16(&mut central, DOS_DATE_1980_01_01);
            push_u32(&mut central, crc);
            push_u32(&mut central, size);
            push_u32(&mut central, size);
            push_u16(&mut central, name_bytes.len() as u16);
            push_u16(&mut central, 0);
            push_u16(&mut central, 0);
            push_u16(&mut central, 0);
            push_u16(&mut central, 0);
            push_u32(&mut central, mode << 16);
            push_u32(&mut central, offset);
            central.extend_from_slice(name_bytes);
        }

        let central_offset = out.len() as u32;
        let central_size = central.len() as u32;
        out.extend_from_slice(&central);

        push_u32(&mut out, SIG_EOCD);
        push_u16(&mut out, 0);
        push_u16(&mut out, 0);
        push_u16(&mut out, entries.len() as u16);
        push_u16(&mut out, entries.len() as u16);
        push_u32(&mut out, central_size);
        push_u32(&mut out, central_offset);
        push_u16(&mut out, 0);

        out
    }

    fn write_crafted(dir: &Path, file_name: &str, bytes: &[u8]) -> PathBuf {
        let path = dir.join(file_name);
        fs::write(&path, bytes).expect("write crafted archive");
        path
    }

    #[test]
    fn inspects_real_spotify_package() {
        let info = PackageInfo::inspect(&fixture_archive("spotify-web")).expect("inspect");

        assert_eq!(info.sha256, SPOTIFY_SHA);
        assert_eq!(info.manifest.name, "spotify-web");
        assert_eq!(info.manifest.version, "1.10.4");
        assert!(info.index_js.contains("registerExtension"));
        assert_eq!(info.entry_names.len(), 5);
        assert!(info.entry_names.contains(&"manifest.json".to_string()));
        assert!(info.entry_names.contains(&"index.js".to_string()));
    }

    #[test]
    fn verifies_registry_digest() {
        let actual =
            PackageInfo::verify_sha256(&fixture_archive("spotify-web"), SPOTIFY_SHA).expect("ok");
        assert_eq!(actual, SPOTIFY_SHA);
    }

    #[test]
    fn rejects_digest_mismatch() {
        let wrong = "0".repeat(64);
        let error = PackageInfo::verify_sha256(&fixture_archive("spotify-web"), &wrong)
            .expect_err("mismatch rejected");
        assert!(matches!(error, PackageError::HashMismatch { .. }));
        assert!(error.to_string().contains(&wrong));
    }

    #[test]
    fn extracts_real_package() {
        let staging = tempfile::tempdir().expect("tempdir");
        let destination = staging.path().join("spotify-web");

        let info =
            PackageInfo::extract_into(&fixture_archive("spotify-web"), &destination).expect("extract");

        assert_eq!(info.sha256, SPOTIFY_SHA);
        assert!(destination.join("manifest.json").is_file());
        assert!(destination.join("index.js").is_file());

        let written = fs::read_to_string(destination.join("manifest.json")).expect("read back");
        assert!(written.contains("spotify-web"));
    }

    #[test]
    fn rejects_parent_traversal_entry() {
        let staging = tempfile::tempdir().expect("tempdir");
        let mut entries = default_entries();
        entries.push(("../evil.js", b"pwned".as_slice(), None));
        let archive = build_zip(staging.path(), "traversal.sflx", &entries);

        let error = PackageInfo::inspect(&archive).expect_err("traversal rejected");
        assert!(matches!(error, PackageError::UnsafeEntryName { .. }));
    }

    #[test]
    fn rejects_absolute_and_drive_paths() {
        for name in ["/etc/passwd", "C:/Windows/evil.dll", "c:evil", "\\\\server\\share\\x"] {
            let staging = tempfile::tempdir().expect("tempdir");
            let mut entries = default_entries();
            entries.push((name, b"x".as_slice(), None));
            let archive = build_zip(staging.path(), "absolute.sflx", &entries);

            let error = PackageInfo::inspect(&archive)
                .expect_err("unsafe path rejected");
            assert!(
                matches!(error, PackageError::UnsafeEntryName { .. }),
                "expected unsafe-name error for {name:?}, got {error}"
            );
        }
    }

    #[test]
    fn rejects_symlink_entry() {
        let staging = tempfile::tempdir().expect("tempdir");
        let bytes = craft_zip(&[
            ("manifest.json", minimal_manifest().as_bytes(), 0o100_644),
            ("index.js", b"registerExtension({});".as_slice(), 0o100_644),
            ("link.flac", b"/etc/passwd".as_slice(), 0o120_777),
        ]);
        let archive = write_crafted(staging.path(), "symlink.sflx", &bytes);

        let error = PackageInfo::inspect(&archive).expect_err("symlink rejected");
        assert!(matches!(error, PackageError::SymlinkEntry { .. }));
        assert!(error.to_string().contains("link.flac"));
    }

    #[test]
    fn rejects_duplicate_entries() {
        let staging = tempfile::tempdir().expect("tempdir");
        let bytes = craft_zip(&[
            ("manifest.json", minimal_manifest().as_bytes(), 0o100_644),
            ("index.js", b"registerExtension({});".as_slice(), 0o100_644),
            ("index.js", b"second copy".as_slice(), 0o100_644),
        ]);
        let archive = write_crafted(staging.path(), "duplicate.sflx", &bytes);

        let error = PackageInfo::inspect(&archive).expect_err("duplicate rejected");
        assert!(matches!(error, PackageError::DuplicateEntry { .. }));
        assert!(error.to_string().contains("index.js"));
    }

    #[test]
    fn rejects_duplicate_entries_differing_only_by_case() {
        let staging = tempfile::tempdir().expect("tempdir");
        let mut entries = default_entries();
        entries.push(("INDEX.JS", b"second copy".as_slice(), None));
        let archive = build_zip(staging.path(), "duplicate-case.sflx", &entries);

        let error = PackageInfo::inspect(&archive).expect_err("case duplicate rejected");
        assert!(matches!(error, PackageError::DuplicateEntry { .. }));
    }

    #[test]
    fn rejects_entry_above_limit() {
        let staging = tempfile::tempdir().expect("tempdir");
        let payload = vec![b'a'; 2048];
        let mut entries = default_entries();
        entries.push(("big.bin", payload.as_slice(), None));
        let archive = build_zip(staging.path(), "big.sflx", &entries);

        let limits = ArchiveLimits {
            max_entry_bytes: 1024,
            ..ArchiveLimits::default()
        };
        let error = PackageInfo::inspect_with(&archive, &limits).expect_err("entry too large");
        assert!(matches!(error, PackageError::EntryTooLarge { .. }));
    }

    #[test]
    fn rejects_total_expansion_above_limit() {
        let staging = tempfile::tempdir().expect("tempdir");
        let first = vec![b'a'; 800];
        let second = vec![b'b'; 800];
        let mut entries = default_entries();
        entries.push(("one.bin", first.as_slice(), None));
        entries.push(("two.bin", second.as_slice(), None));
        let archive = build_zip(staging.path(), "total.sflx", &entries);

        let limits = ArchiveLimits {
            max_total_bytes: 1500,
            ..ArchiveLimits::default()
        };
        let error =
            PackageInfo::inspect_with(&archive, &limits).expect_err("total too large");
        assert!(matches!(error, PackageError::ArchiveExpansionTooLarge { .. }));
    }

    #[test]
    fn rejects_too_many_entries() {
        let staging = tempfile::tempdir().expect("tempdir");
        let mut names: Vec<String> = vec!["a".into(), "b".into(), "c".into()];
        let payload = b"x";
        let mut owned: Vec<(String, &[u8])> = Vec::new();
        for name in names.drain(..) {
            owned.push((name, payload.as_slice()));
        }
        let mut entries = default_entries();
        for (name, payload) in &owned {
            entries.push((name.as_str(), *payload, None));
        }
        let archive = build_zip(staging.path(), "many.sflx", &entries);

        let limits = ArchiveLimits {
            max_entries: 3,
            ..ArchiveLimits::default()
        };
        let error = PackageInfo::inspect_with(&archive, &limits).expect_err("too many entries");
        assert!(matches!(error, PackageError::TooManyEntries { .. }));
    }

    #[test]
    fn rejects_compression_ratio_above_limit() {
        let staging = tempfile::tempdir().expect("tempdir");
        let payload = vec![0u8; 64 * 1024];
        let path = staging.path().join("ratio.sflx");

        let file = fs::File::create(&path).expect("create");
        let mut writer = zip::ZipWriter::new(file);
        let stored = SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        writer.start_file("manifest.json", stored).expect("manifest");
        writer
            .write_all(minimal_manifest().as_bytes())
            .expect("manifest body");
        writer.start_file("index.js", stored).expect("index");
        writer.write_all(b"registerExtension({});").expect("index body");

        let deflated = SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        writer.start_file("bomb.bin", deflated).expect("bomb");
        writer.write_all(&payload).expect("bomb body");
        writer.finish().expect("finish");

        let limits = ArchiveLimits {
            max_compression_ratio: 10,
            ratio_check_floor_bytes: 1024,
            ..ArchiveLimits::default()
        };
        let error = PackageInfo::inspect_with(&path, &limits).expect_err("ratio rejected");
        assert!(matches!(error, PackageError::CompressionRatioExceeded { .. }));
    }

    #[test]
    fn rejects_archive_missing_index_js() {
        let staging = tempfile::tempdir().expect("tempdir");
        let entries: Vec<(&str, &[u8], Option<u32>)> =
            vec![("manifest.json", minimal_manifest().as_bytes(), None)];
        let archive = build_zip(staging.path(), "no-index.sflx", &entries);

        let error = PackageInfo::inspect(&archive).expect_err("index.js required");
        assert!(matches!(
            error,
            PackageError::MissingEntry { name: "index.js" }
        ));
    }

    #[test]
    fn rejected_archive_leaves_no_files_behind() {
        let staging = tempfile::tempdir().expect("tempdir");
        let mut entries = default_entries();
        entries.push(("../escape.js", b"pwned".as_slice(), None));
        let archive = build_zip(staging.path(), "traversal.sflx", &entries);

        let destination = staging.path().join("out");
        let error = PackageInfo::extract_into(&archive, &destination).expect_err("rejected");
        assert!(matches!(error, PackageError::UnsafeEntryName { .. }));

        assert!(
            !destination.exists(),
            "validation must run before the destination is created"
        );
        assert!(!staging.path().join("escape.js").exists());
    }

    #[test]
    fn rejects_archive_above_archive_size_limit() {
        let staging = tempfile::tempdir().expect("tempdir");
        let archive = build_zip(staging.path(), "small.sflx", &default_entries());

        let limits = ArchiveLimits {
            max_archive_bytes: 10,
            ..ArchiveLimits::default()
        };
        let error = PackageInfo::inspect_with(&archive, &limits).expect_err("archive too large");
        assert!(matches!(error, PackageError::ArchiveTooLarge { .. }));
    }

    #[test]
    fn reads_a_single_file_back_out() {
        let bytes = PackageInfo::read_file(&fixture_archive("spotify-web"), "manifest.json")
            .expect("read manifest");
        let text = String::from_utf8(bytes).expect("utf8");
        assert!(text.contains("\"name\": \"spotify-web\""));
    }
}


