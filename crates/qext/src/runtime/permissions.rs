//! Capability enforcement: what an extension may reach and where it may write.
//!
//! Two independent gates, both checked on the host side so a package cannot
//! skip them by calling a different API:
//!
//! - [`NetworkAllowlist`] — every request, including each redirect hop
//! - [`FileSandbox`] — every relative path, resolved and re-checked after
//!   symlink resolution
//!
//! Wildcards are deliberately narrow. `*.audio.tidal.com` matches
//! `a.audio.tidal.com` but not `audio.tidal.com`, and never matches
//! `tidal.com.evil.net`: suffix matching always requires a dot separator.

use crate::manifest::Manifest;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};
use thiserror::Error;
use url::Url;

/// Windows device names that a file operation must never target.
const RESERVED_DEVICE_NAMES: [&str; 22] = [
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

#[derive(Debug, Error, PartialEq, Eq)]
pub enum PermissionError {
    #[error("network access denied: {host} is not declared in permissions.network")]
    NetworkDenied { host: String },

    #[error("network access denied: {url:?} is not a valid URL")]
    InvalidUrl { url: String },

    #[error("file access denied: absolute paths are not allowed ({path:?})")]
    AbsolutePathNotAllowed { path: String },

    #[error("file access denied: {path:?} is outside the extension sandbox")]
    OutsideSandbox { path: String },

    #[error("file access denied: {path:?} is not a safe relative path")]
    UnsafePath { path: String },

    #[error("file access denied: extension does not have the 'file' permission")]
    FilePermissionMissing,

    #[error("storage access denied: extension does not have the 'storage' permission")]
    StoragePermissionMissing,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum NetworkRule {
    /// Matches one host exactly.
    Exact(String),
    /// Matches any subdomain of `suffix`, requiring the separating dot.
    Suffix(String),
}

/// The hosts a single extension may contact.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NetworkAllowlist {
    rules: Vec<NetworkRule>,
}

impl NetworkAllowlist {
    pub fn from_patterns(patterns: &[String]) -> Self {
        let rules = patterns
            .iter()
            .filter_map(|pattern| build_rule(pattern))
            .collect();
        Self { rules }
    }

    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    pub fn rule_count(&self) -> usize {
        self.rules.len()
    }

    /// True when `host` is covered by a declared rule.
    pub fn allows_host(&self, host: &str) -> bool {
        let host = normalise_host(host);
        if host.is_empty() {
            return false;
        }

        self.rules.iter().any(|rule| match rule {
            NetworkRule::Exact(pattern) => host == *pattern,
            NetworkRule::Suffix(suffix) => host
                .strip_suffix(suffix.as_str())
                .is_some_and(|prefix| prefix.ends_with('.') && prefix.len() > 1),
        })
    }

    /// Parse `raw` and check its host. Returns the parsed URL so callers can
    /// reuse it without parsing twice.
    pub fn check_url(&self, raw: &str) -> Result<Url, PermissionError> {
        let url = Url::parse(raw).map_err(|_| PermissionError::InvalidUrl {
            url: raw.to_string(),
        })?;

        if !matches!(url.scheme(), "http" | "https") {
            return Err(PermissionError::InvalidUrl {
                url: raw.to_string(),
            });
        }

        let host = url.host_str().unwrap_or_default();
        if !self.allows_host(host) {
            return Err(PermissionError::NetworkDenied {
                host: normalise_host(host),
            });
        }

        Ok(url)
    }
}

fn build_rule(pattern: &str) -> Option<NetworkRule> {
    let trimmed = pattern.trim();
    if trimmed.is_empty() {
        return None;
    }

    // Tolerate a package that writes a full URL instead of a host.
    let host_part = match trimmed.split_once("://") {
        Some((_, rest)) => rest,
        None => trimmed,
    };
    let host_part = host_part
        .split(['/', '?', '#'])
        .next()
        .unwrap_or(host_part);
    let host_part = host_part.rsplit_once('@').map_or(host_part, |(_, host)| host);
    let host_part = host_part.split(':').next().unwrap_or(host_part);

    let host = normalise_host(host_part);
    if host.is_empty() {
        return None;
    }

    if let Some(suffix) = host.strip_prefix("*.") {
        if suffix.is_empty() {
            return None;
        }
        // Stored bare: `allows_host` supplies the separating dot itself, so
        // `audio.tidal.com` matches `a.audio.tidal.com` and never
        // `notaudio.tidal.com`.
        Some(NetworkRule::Suffix(suffix.to_string()))
    } else {
        Some(NetworkRule::Exact(host))
    }
}

fn normalise_host(host: &str) -> String {
    host.trim()
        .trim_end_matches('.')
        .to_ascii_lowercase()
}

/// Everything a manifest grants, resolved once at install time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Permissions {
    pub network: NetworkAllowlist,
    pub storage: bool,
    pub file: bool,
}

impl Permissions {
    pub fn from_manifest(manifest: &Manifest) -> Self {
        Self {
            network: NetworkAllowlist::from_patterns(&manifest.permissions.network),
            storage: manifest.permissions.storage,
            file: manifest.permissions.file,
        }
    }

    /// Reject before doing any work if the manifest is missing a grant.
    pub fn require_file(&self) -> Result<(), PermissionError> {
        if self.file {
            Ok(())
        } else {
            Err(PermissionError::FilePermissionMissing)
        }
    }

    pub fn require_storage(&self) -> Result<(), PermissionError> {
        if self.storage {
            Ok(())
        } else {
            Err(PermissionError::StoragePermissionMissing)
        }
    }
}

/// Resolves extension-supplied paths inside one directory.
///
/// The root is canonicalised at construction. Every resolved path is checked
/// twice: lexically (segments must be plain names) and physically (the
/// canonical parent must still sit under the root, which catches a symlink
/// planted earlier).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileSandbox {
    root: PathBuf,
}

impl FileSandbox {
    pub fn new(root: impl AsRef<Path>) -> io::Result<Self> {
        let root = root.as_ref();
        fs::create_dir_all(root)?;
        let canonical = fs::canonicalize(root)?;
        Ok(Self { root: canonical })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Resolve a relative path declared by an extension.
    pub fn resolve(&self, requested: &str) -> Result<PathBuf, PermissionError> {
        if requested.trim().is_empty() {
            return Err(PermissionError::UnsafePath {
                path: requested.to_string(),
            });
        }

        if requested.contains('\0') || requested.contains('\\') {
            return Err(PermissionError::UnsafePath {
                path: requested.to_string(),
            });
        }

        // Nothing in this pipeline percent-decodes, so `..%2f` is currently an
        // odd but harmless filename. It is rejected anyway: a future layer that
        // decodes before reaching here would turn it into traversal, and no
        // legitimate extension data file needs an encoded separator.
        if contains_encoded_path_character(requested) {
            return Err(PermissionError::UnsafePath {
                path: requested.to_string(),
            });
        }

        let candidate = Path::new(requested);
        if candidate.is_absolute() || has_windows_prefix(requested) {
            return Err(PermissionError::AbsolutePathNotAllowed {
                path: requested.to_string(),
            });
        }

        // `Path::components()` normalises `.` away and drops it entirely, so
        // `a/./b` and `./a` would otherwise resolve. The raw segments are
        // checked first: only plain names may appear between separators.
        for segment in requested.trim_end_matches('/').split('/') {
            if segment.is_empty() || segment == "." || segment == ".." {
                return Err(PermissionError::UnsafePath {
                    path: requested.to_string(),
                });
            }
        }

        for component in candidate.components() {
            match component {
                Component::Normal(name) => {
                    let name = name.to_string_lossy();
                    if is_reserved_device_name(&name) {
                        return Err(PermissionError::UnsafePath {
                            path: requested.to_string(),
                        });
                    }
                }
                _ => {
                    return Err(PermissionError::UnsafePath {
                        path: requested.to_string(),
                    })
                }
            }
        }

        let joined = self.root.join(candidate);
        self.ensure_inside(&joined, requested)?;
        Ok(joined)
    }

    /// Accept a path the host itself produced, such as a queued download
    /// target. Absolute is fine here; escaping the download directory is not.
    pub fn resolve_host_path(&self, path: &Path) -> Result<PathBuf, PermissionError> {
        if !path.is_absolute() {
            return Err(PermissionError::UnsafePath {
                path: path.display().to_string(),
            });
        }

        if let Some(name) = path.file_name().map(|name| name.to_string_lossy()) {
            if is_reserved_device_name(&name) {
                return Err(PermissionError::UnsafePath {
                    path: path.display().to_string(),
                });
            }
        }

        self.ensure_inside(path, &path.display().to_string())?;
        Ok(path.to_path_buf())
    }

    fn ensure_inside(&self, candidate: &Path, requested: &str) -> Result<(), PermissionError> {
        // Check the nearest existing ancestor: the file itself may not exist
        // yet, but the directory it will live in does.
        let mut probe = candidate;
        while !probe.exists() {
            match probe.parent() {
                Some(parent) => probe = parent,
                None => {
                    return Err(PermissionError::OutsideSandbox {
                        path: requested.to_string(),
                    })
                }
            }
        }

        let canonical = fs::canonicalize(probe).map_err(|_| PermissionError::OutsideSandbox {
            path: requested.to_string(),
        })?;

        if !canonical.starts_with(&self.root) {
            return Err(PermissionError::OutsideSandbox {
                path: requested.to_string(),
            });
        }

        Ok(())
    }
}

fn has_windows_prefix(path: &str) -> bool {
    let bytes = path.as_bytes();
    if bytes.len() >= 2 && bytes[1] == b':' && bytes[0].is_ascii_alphabetic() {
        return true;
    }
    path.starts_with("//") || path.starts_with("~")
}

fn is_reserved_device_name(name: &str) -> bool {
    let stem = name.split('.').next().unwrap_or(name).to_ascii_uppercase();
    RESERVED_DEVICE_NAMES.contains(&stem.as_str())
}

/// True when the path carries a percent-encoded dot or separator.
fn contains_encoded_path_character(path: &str) -> bool {
    let lowered = path.to_ascii_lowercase();
    ["%2e", "%2f", "%5c"].iter().any(|needle| lowered.contains(needle))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn allowlist(patterns: &[&str]) -> NetworkAllowlist {
        let owned: Vec<String> = patterns.iter().map(|value| value.to_string()).collect();
        NetworkAllowlist::from_patterns(&owned)
    }

    #[test]
    fn exact_host_is_allowed_and_subdomains_are_not() {
        let list = allowlist(&["tidal.com"]);

        assert!(list.allows_host("tidal.com"));
        assert!(list.allows_host("TIDAL.COM"));
        assert!(list.allows_host("tidal.com."));
        assert!(!list.allows_host("evil.tidal.com"));
        assert!(!list.allows_host("www.tidal.com"));
    }

    #[test]
    fn wildcard_matches_child_hosts_only() {
        let list = allowlist(&["*.audio.tidal.com"]);

        assert!(list.allows_host("a.audio.tidal.com"));
        assert!(list.allows_host("a.b.audio.tidal.com"));
        assert!(list.allows_host("A.AUDIO.TIDAL.COM"));

        assert!(!list.allows_host("audio.tidal.com"));
        assert!(!list.allows_host("audio.tidal.com.evil.net"));
        assert!(!list.allows_host("notaudio.tidal.com"));
    }

    #[test]
    fn wildcard_never_widens_to_a_different_domain() {
        let list = allowlist(&["*.tidal.com"]);

        assert!(list.allows_host("api.tidal.com"));
        assert!(!list.allows_host("tidal.com.evil.net"));
        assert!(!list.allows_host("eviltidal.com"));
        assert!(!list.allows_host("tidal.community"));
    }

    #[test]
    fn declared_tidal_hosts_behave_as_the_manifest_expects() {
        let patterns = [
            "tidal.com",
            "www.tidal.com",
            "listen.tidal.com",
            "resources.tidal.com",
            "api.zarz.moe",
            "*.audio.tidal.com",
            "*.manifest.tidal.com",
        ];
        let list = allowlist(&patterns);

        for host in [
            "tidal.com",
            "www.tidal.com",
            "listen.tidal.com",
            "resources.tidal.com",
            "api.zarz.moe",
            "sp.audio.tidal.com",
            "sp.manifest.tidal.com",
        ] {
            assert!(list.allows_host(host), "{host} should be allowed");
        }

        // `sp-audio.tidal.com` is a sibling label, not a subdomain of
        // `audio.tidal.com`, so the wildcard must not cover it.
        for host in [
            "evil.tidal.com",
            "tidal.com.evil.net",
            "zarz.moe",
            "audio.tidal.com",
            "manifest.tidal.com",
            "sp-audio.tidal.com",
            "sp-manifest.tidal.com",
        ] {
            assert!(!list.allows_host(host), "{host} should be denied");
        }
    }

    #[test]
    fn urls_are_checked_by_host_and_scheme() {
        let list = allowlist(&["api.example.com"]);

        let url = list
            .check_url("https://api.example.com/v1/tracks?limit=10")
            .expect("allowed");
        assert_eq!(url.path(), "/v1/tracks");

        let error = list
            .check_url("https://api.example.com.evil.net/v1")
            .expect_err("denied");
        assert!(matches!(error, PermissionError::NetworkDenied { .. }));

        let error = list
            .check_url("ftp://api.example.com/file")
            .expect_err("scheme denied");
        assert!(matches!(error, PermissionError::InvalidUrl { .. }));

        let error = list.check_url("not a url").expect_err("invalid");
        assert!(matches!(error, PermissionError::InvalidUrl { .. }));
    }

    #[test]
    fn url_check_ignores_port_and_credentials() {
        let list = allowlist(&["api.example.com"]);
        assert!(list.check_url("https://api.example.com:8443/x").is_ok());
        assert!(list.check_url("https://user:pass@api.example.com/x").is_ok());
    }

    #[test]
    fn patterns_written_as_urls_are_normalised() {
        let list = allowlist(&["https://api.example.com/v1", "http://cdn.example.net:8080"]);
        assert!(list.allows_host("api.example.com"));
        assert!(list.allows_host("cdn.example.net"));
    }

    #[test]
    fn empty_patterns_are_ignored() {
        let list = allowlist(&["", "   "]);
        assert!(list.is_empty());
        assert!(!list.allows_host("example.com"));
    }

    #[test]
    fn relative_paths_resolve_inside_the_sandbox() {
        let temp = tempfile::tempdir().expect("tempdir");
        let sandbox = FileSandbox::new(temp.path().join("ext")).expect("sandbox");

        let resolved = sandbox.resolve("cache/data.json").expect("resolves");
        assert!(resolved.starts_with(sandbox.root()));
        assert!(resolved.ends_with("cache/data.json"));

        let resolved = sandbox.resolve("track.flac").expect("resolves");
        assert_eq!(resolved.parent(), Some(sandbox.root()));
    }

    #[test]
    fn traversal_is_rejected() {
        let temp = tempfile::tempdir().expect("tempdir");
        let sandbox = FileSandbox::new(temp.path().join("ext")).expect("sandbox");

        for path in [
            "../other-extension/x",
            "a/../../x",
            "..",
            "a/./b",
            "./a",
            "..%2fetc%2fpasswd",
            "%2e%2e/escape",
            "a/%2E%2E/b",
        ] {
            let error = sandbox.resolve(path).expect_err("must be rejected");
            assert!(
                matches!(
                    error,
                    PermissionError::UnsafePath { .. }
                        | PermissionError::AbsolutePathNotAllowed { .. }
                        | PermissionError::OutsideSandbox { .. }
                ),
                "path {path:?} produced {error:?}"
            );
        }
    }

    #[test]
    fn absolute_and_windows_paths_are_rejected() {
        let temp = tempfile::tempdir().expect("tempdir");
        let sandbox = FileSandbox::new(temp.path().join("ext")).expect("sandbox");

        for path in [
            "/etc/passwd",
            "C:/Windows/System32/evil.dll",
            "c:evil",
            "\\\\server\\share\\file",
            "~/.ssh/id_rsa",
            "a\\b",
        ] {
            let error = sandbox.resolve(path).expect_err("must be rejected");
            assert!(
                matches!(
                    error,
                    PermissionError::AbsolutePathNotAllowed { .. }
                        | PermissionError::UnsafePath { .. }
                ),
                "path {path:?} produced {error:?}"
            );
        }
    }

    #[test]
    fn device_names_are_rejected() {
        let temp = tempfile::tempdir().expect("tempdir");
        let sandbox = FileSandbox::new(temp.path().join("ext")).expect("sandbox");

        for path in ["CON", "nul.txt", "COM1", "lpt9.flac"] {
            let error = sandbox.resolve(path).expect_err("must be rejected");
            assert!(
                matches!(error, PermissionError::UnsafePath { .. }),
                "path {path:?} produced {error:?}"
            );
        }
    }

    #[test]
    fn host_paths_must_be_absolute_and_inside() {
        let temp = tempfile::tempdir().expect("tempdir");
        let downloads = temp.path().join("downloads");
        let sandbox = FileSandbox::new(&downloads).expect("sandbox");

        let inside = downloads.join("track.flac");
        assert_eq!(
            sandbox.resolve_host_path(&inside).expect("allowed"),
            inside
        );

        let relative = Path::new("track.flac");
        assert!(sandbox.resolve_host_path(relative).is_err());

        let outside = temp.path().join("elsewhere").join("track.flac");
        fs::create_dir_all(outside.parent().expect("parent")).expect("mkdir");
        assert!(matches!(
            sandbox.resolve_host_path(&outside),
            Err(PermissionError::OutsideSandbox { .. })
        ));
    }

    #[test]
    fn symlink_pointing_outside_is_rejected() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join("ext");
        let sandbox = FileSandbox::new(&root).expect("sandbox");

        let outside = temp.path().join("outside");
        fs::create_dir_all(&outside).expect("outside dir");

        // Creating a symlink needs elevation or developer mode on Windows; if
        // the OS refuses, the environment cannot exercise this control.
        let link = root.join("escape");
        if create_dir_symlink(&outside, &link).is_err() {
            return;
        }

        let error = sandbox.resolve("escape/secret.txt").expect_err("must be rejected");
        assert!(
            matches!(error, PermissionError::OutsideSandbox { .. }),
            "got {error:?}"
        );
    }

    #[cfg(windows)]
    fn create_dir_symlink(target: &Path, link: &Path) -> io::Result<()> {
        std::os::windows::fs::symlink_dir(target, link)
    }

    #[cfg(not(windows))]
    fn create_dir_symlink(target: &Path, link: &Path) -> io::Result<()> {
        std::os::unix::fs::symlink(target, link)
    }

    #[test]
    fn manifest_grants_are_read_from_permissions() {
        let manifest = Manifest::parse(
            r#"{
                "name": "grant-test",
                "version": "1.0.0",
                "permissions": {
                    "network": ["api.example.com", "*.cdn.example.com"],
                    "storage": true,
                    "file": false
                }
            }"#,
        )
        .expect("manifest");

        let permissions = Permissions::from_manifest(&manifest);
        assert!(permissions.network.allows_host("api.example.com"));
        assert!(permissions.network.allows_host("a.cdn.example.com"));
        assert!(permissions.require_storage().is_ok());
        assert_eq!(permissions.require_file(), Err(PermissionError::FilePermissionMissing));
    }
}
