//! Artwork cache. Files are keyed by a content fingerprint, so two tracks from the
//! same album share one image and re-scanning never duplicates anything.

use std::path::{Path, PathBuf};

use crate::error::{CoreError, CoreResult};
use crate::tags::Picture;

pub struct ArtCache {
    directory: PathBuf,
}

impl ArtCache {
    pub fn new(directory: PathBuf) -> CoreResult<Self> {
        std::fs::create_dir_all(&directory)
            .map_err(|error| CoreError::io(&directory, error))?;
        Ok(Self { directory })
    }

    /// Stores the picture if it is new and returns its key.
    pub fn store(&self, picture: &Picture) -> CoreResult<String> {
        let key = &picture.cache_key;
        if self.find(key).is_some() {
            return Ok(key.clone());
        }

        let extension = extension_for(&picture.mime);
        let path = self.directory.join(format!("{key}.{extension}"));
        std::fs::write(&path, &picture.bytes).map_err(|error| CoreError::io(&path, error))?;
        Ok(key.clone())
    }

    /// Returns where the cached image lives, if it is already on disk.
    pub fn find(&self, key: &str) -> Option<PathBuf> {
        for extension in ["jpg", "jpeg", "png", "webp", "gif", "bmp"] {
            let candidate = self.directory.join(format!("{key}.{extension}"));
            if candidate.is_file() {
                return Some(candidate);
            }
        }
        None
    }

    pub fn read(&self, key: &str) -> Option<(Vec<u8>, String)> {
        let path = self.find(key)?;
        let bytes = std::fs::read(&path).ok()?;
        let mime = match path.extension().and_then(|extension| extension.to_str()) {
            Some("png") => "image/png",
            Some("webp") => "image/webp",
            Some("gif") => "image/gif",
            Some("bmp") => "image/bmp",
            _ => "image/jpeg",
        };
        Some((bytes, mime.to_string()))
    }

    pub fn directory(&self) -> &Path {
        &self.directory
    }

    /// Removes cached images no longer referenced by any track.
    pub fn prune(&self, referenced: &[String]) -> usize {
        let Ok(entries) = std::fs::read_dir(&self.directory) else {
            return 0;
        };
        let mut removed = 0;
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
                continue;
            };
            if !referenced.iter().any(|key| key == stem) && std::fs::remove_file(&path).is_ok() {
                removed += 1;
            }
        }
        removed
    }
}

fn extension_for(mime: &str) -> &'static str {
    match mime.to_ascii_lowercase().as_str() {
        "image/png" => "png",
        "image/webp" => "webp",
        "image/gif" => "gif",
        "image/bmp" => "bmp",
        _ => "jpg",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stores_once_and_reads_back() {
        let directory = tempfile::tempdir().expect("temp dir");
        let cache = ArtCache::new(directory.path().join("art")).expect("cache");
        let picture = Picture {
            bytes: vec![0xff, 0xd8, 0xff, 0x00],
            mime: "image/jpeg".into(),
            cache_key: crate::tags::fingerprint(b"cover-one"),
        };

        let key = cache.store(&picture).expect("store");
        assert_eq!(key, picture.cache_key);
        // Storing the same bytes again is a no-op, not a duplicate.
        cache.store(&picture).expect("store again");
        let entries = std::fs::read_dir(cache.directory()).expect("read dir").count();
        assert_eq!(entries, 1);

        let (bytes, mime) = cache.read(&key).expect("read");
        assert_eq!(bytes, picture.bytes);
        assert_eq!(mime, "image/jpeg");
    }

    #[test]
    fn prune_keeps_referenced_images() {
        let directory = tempfile::tempdir().expect("temp dir");
        let cache = ArtCache::new(directory.path().join("art")).expect("cache");
        let kept = Picture {
            bytes: vec![1, 2, 3],
            mime: "image/png".into(),
            cache_key: "aaaa".into(),
        };
        let dropped = Picture {
            bytes: vec![4, 5, 6],
            mime: "image/png".into(),
            cache_key: "bbbb".into(),
        };
        cache.store(&kept).expect("store");
        cache.store(&dropped).expect("store");

        assert_eq!(cache.prune(&["aaaa".to_string()]), 1);
        assert!(cache.find("aaaa").is_some());
        assert!(cache.find("bbbb").is_none());
    }
}
