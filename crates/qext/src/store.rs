//! Installing, updating, disabling and removing extensions.
//!
//! The filesystem is the source of truth. An extension is installed when its
//! directory exists and holds a readable manifest, which means a crash cannot
//! leave the host believing in an extension that is not there.
//!
//! Layout under one root:
//!
//! ```text
//! <root>/<id>/manifest.json      the package, exactly as published
//! <root>/<id>/provenance.json    version, digest, install time, enabled flag
//! <root>/.data/<id>/             extension storage; survives updates
//! <root>/.staging/<id>/new       a package being installed
//! <root>/.staging/<id>/previous  the version it is replacing
//! ```
//!
//! The swap is two renames. Between them the extension is missing from `<root>`
//! but its previous version is still on disk, so [`ExtensionStore::open`] puts
//! it back. That is the whole crash-safety story: the only state that can be
//! half-written is the staging directory, and staging is never served.
//!
//! Data lives outside the package directory on purpose. An update replaces the
//! package wholesale, and an extension that stores credentials in its data
//! directory must not lose them because the publisher bumped a patch version.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::manifest::{Manifest, ManifestError};
use crate::package::{PackageError, PackageInfo};
use crate::registry::{RegistryEntry, Version};
use crate::runtime::polyfills::HostServices;
use crate::runtime::{ExtensionRuntime, RuntimeError, RuntimeLimits};

/// Directory holding per-extension storage.
const DATA_DIR: &str = ".data";
/// Directory holding in-flight installs.
const STAGING_DIR: &str = ".staging";
/// File written next to the package describing how it got there.
const PROVENANCE_FILE: &str = "provenance.json";

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("cannot use {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("the package is not installable: {0}")]
    Package(#[from] PackageError),

    #[error("the package manifest is not usable: {0}")]
    Manifest(#[from] ManifestError),

    #[error("{value:?} is not a usable extension id")]
    InvalidId { value: String },

    #[error("{id} is not installed")]
    NotInstalled { id: String },

    #[error("{id} {version} is already installed; reinstalling is not an update")]
    AlreadyInstalled { id: String, version: String },

    #[error("cannot downgrade {id} from {installed} to {offered}")]
    Downgrade {
        id: String,
        installed: String,
        offered: String,
    },

    #[error("{id} declares version {value:?}, which is not a version this host can compare")]
    InvalidVersion { id: String, value: String },

    #[error("{id}: the package digest does not match the registry")]
    ChecksumMismatch {
        id: String,
        expected: String,
        actual: String,
    },

    #[error("{id}: the package names itself {actual:?}, not {expected:?}")]
    IdMismatch {
        id: String,
        expected: String,
        actual: String,
    },

    #[error("{id}: the package changed while it was being installed")]
    PackageChanged { id: String },

    #[error("{id} is refused: {reason}")]
    Refused { id: String, reason: String },

    #[error("{id} is disabled; enable it before starting it")]
    Disabled { id: String },

    #[error("the extension runtime could not start: {0}")]
    Runtime(#[from] RuntimeError),
}

/// A package that is installed and ready to run.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstalledExtension {
    pub id: String,
    pub version: Version,
    pub display_name: Option<String>,
    /// Digest of the archive this version was installed from.
    pub sha256: String,
    pub installed_at: i64,
    pub enabled: bool,
    pub directory: PathBuf,
    pub data_directory: PathBuf,
}

impl InstalledExtension {
    /// The `manifest.json` this package ships.
    pub fn manifest_path(&self) -> PathBuf {
        self.directory.join("manifest.json")
    }

    pub fn index_path(&self) -> PathBuf {
        self.directory.join("index.js")
    }
}

/// A directory under the root that is not a usable package.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BrokenExtension {
    pub id: String,
    pub path: PathBuf,
    pub reason: String,
}

/// Everything the root currently holds.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StoreScan {
    pub installed: Vec<InstalledExtension>,
    /// Directories that exist but cannot be served. Reported rather than
    /// skipped: a package the host silently forgets is indistinguishable from
    /// one that was never installed.
    pub broken: Vec<BrokenExtension>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Provenance {
    id: String,
    version: String,
    #[serde(default)]
    display_name: Option<String>,
    sha256: String,
    installed_at: i64,
    enabled: bool,
}

/// Owns the extensions root and the runtimes started from it.
pub struct ExtensionStore {
    root: PathBuf,
    staging_root: PathBuf,
    data_root: PathBuf,
    limits: RuntimeLimits,
    runtimes: Mutex<HashMap<String, Arc<ExtensionRuntime>>>,
}

impl std::fmt::Debug for ExtensionStore {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ExtensionStore")
            .field("root", &self.root)
            .field("limits", &self.limits)
            .finish()
    }
}

impl ExtensionStore {
    /// Open (or create) a store, recovering any install that was interrupted.
    pub fn open(root: impl Into<PathBuf>) -> Result<Self, StoreError> {
        Self::open_with(root, RuntimeLimits::default())
    }

    pub fn open_with(root: impl Into<PathBuf>, limits: RuntimeLimits) -> Result<Self, StoreError> {
        let root = root.into();
        create_dir(&root)?;

        let staging_root = root.join(STAGING_DIR);
        let data_root = root.join(DATA_DIR);
        create_dir(&staging_root)?;
        create_dir(&data_root)?;

        let store = Self {
            root,
            staging_root,
            data_root,
            limits,
            runtimes: Mutex::new(HashMap::new()),
        };

        store.recover()?;
        Ok(store)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn data_root(&self) -> &Path {
        &self.data_root
    }

    pub fn limits(&self) -> RuntimeLimits {
        self.limits
    }

    /// Every installed package, plus the directories that cannot be served.
    pub fn scan(&self) -> StoreScan {
        let mut scan = StoreScan::default();
        let Ok(entries) = fs::read_dir(&self.root) else {
            return scan;
        };

        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let Some(id) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            if id == DATA_DIR || id == STAGING_DIR {
                continue;
            }

            match self.read_package(id, &path) {
                Ok(installed) => scan.installed.push(installed),
                Err(reason) => scan.broken.push(BrokenExtension {
                    id: id.to_string(),
                    path,
                    reason,
                }),
            }
        }

        scan.installed.sort_by(|left, right| left.id.cmp(&right.id));
        scan.broken.sort_by(|left, right| left.id.cmp(&right.id));
        scan
    }

    pub fn installed(&self, id: &str) -> Option<InstalledExtension> {
        let path = self.package_dir(id).ok()?;
        if !path.is_dir() {
            return None;
        }
        self.read_package(id, &path).ok()
    }

    /// Install a local package. Refuses a downgrade and an equal version.
    pub fn install(&mut self, package: &Path) -> Result<InstalledExtension, StoreError> {
        let info = PackageInfo::inspect(package)?;
        self.install_inspected(package, info)
    }

    /// Install a package that came from the registry, verifying the published
    /// digest and the id before anything is written.
    pub fn install_from_registry(
        &mut self,
        entry: &RegistryEntry,
        package: &Path,
    ) -> Result<InstalledExtension, StoreError> {
        if !entry.status.is_supported() {
            return Err(StoreError::Refused {
                id: entry.id.clone(),
                reason: entry.status.code().to_string(),
            });
        }

        let info = PackageInfo::inspect(package)?;

        if !info.sha256.eq_ignore_ascii_case(&entry.sha256) {
            return Err(StoreError::ChecksumMismatch {
                id: entry.id.clone(),
                expected: entry.sha256.clone(),
                actual: info.sha256.clone(),
            });
        }

        if info.manifest.name != entry.id {
            return Err(StoreError::IdMismatch {
                id: entry.id.clone(),
                expected: entry.id.clone(),
                actual: info.manifest.name.clone(),
            });
        }

        self.install_inspected(package, info)
    }

    fn install_inspected(
        &mut self,
        package: &Path,
        info: PackageInfo,
    ) -> Result<InstalledExtension, StoreError> {
        let id = info.manifest.name.clone();
        // Validates the id, so every join below stays inside the root.
        self.package_dir(&id)?;

        let offered =
            Version::parse(&info.manifest.version).ok_or_else(|| StoreError::InvalidVersion {
                id: id.clone(),
                value: info.manifest.version.clone(),
            })?;

        let existing = self.installed(&id);
        if let Some(installed) = &existing {
            if offered < installed.version {
                return Err(StoreError::Downgrade {
                    id,
                    installed: installed.version.to_string(),
                    offered: offered.to_string(),
                });
            }
            if offered == installed.version {
                return Err(StoreError::AlreadyInstalled {
                    id,
                    version: offered.to_string(),
                });
            }
        }

        // An interrupted swap for this id must be resolved before its staging
        // directory is reused, or the previous version would be deleted.
        self.recover_id(&id)?;

        let staging = self.staging_root.join(&id);
        let fresh = staging.join("new");
        let previous = staging.join("previous");
        remove_dir_all(&staging)?;
        create_dir(&staging)?;

        let extracted = PackageInfo::extract_into(package, &fresh)?;

        if extracted.sha256 != info.sha256 {
            remove_dir_all(&staging)?;
            return Err(StoreError::PackageChanged { id });
        }

        let provenance = Provenance {
            id: id.clone(),
            version: offered.to_string(),
            display_name: info.manifest.display_name.clone(),
            sha256: info.sha256.clone(),
            installed_at: now_seconds(),
            // A fresh package starts enabled; an update keeps the user's choice.
            enabled: existing
                .as_ref()
                .map(|installed| installed.enabled)
                .unwrap_or(true),
        };
        write_provenance(&fresh, &provenance)?;

        let data_directory = self.data_root.join(&id);
        create_dir(&data_directory)?;

        // A runtime is serving the old bytes; it has to go before they move.
        self.deactivate(&id);

        let active = self.root.join(&id);
        if active.is_dir() {
            fs::rename(&active, &previous).map_err(|source| StoreError::Io {
                path: active.clone(),
                source,
            })?;
        }

        if let Err(source) = fs::rename(&fresh, &active) {
            // Put the previous version back so a failed update is a no-op.
            if previous.is_dir() {
                let _ = fs::rename(&previous, &active);
            }
            let _ = remove_dir_all(&staging);
            return Err(StoreError::Io { path: active, source });
        }

        let _ = remove_dir_all(&staging);

        self.installed(&id).ok_or(StoreError::PackageChanged { id })
    }

    /// Remove a package and its data. The runtime is stopped first.
    pub fn uninstall(&mut self, id: &str) -> Result<(), StoreError> {
        let active = self.package_dir(id)?;
        if !active.is_dir() {
            return Err(StoreError::NotInstalled {
                id: id.to_string(),
            });
        }

        self.deactivate(id);
        remove_dir_all(&active)?;

        let data = self.data_root.join(id);
        if data.is_dir() {
            remove_dir_all(&data)?;
        }

        // A staging entry left by a failed install would otherwise outlive the
        // extension it belongs to.
        let _ = remove_dir_all(&self.staging_root.join(id));

        Ok(())
    }

    /// Turn an installed extension on or off without touching its files.
    pub fn set_enabled(
        &mut self,
        id: &str,
        enabled: bool,
    ) -> Result<InstalledExtension, StoreError> {
        let installed = self.installed(id).ok_or_else(|| StoreError::NotInstalled {
            id: id.to_string(),
        })?;

        if !enabled {
            self.deactivate(id);
        }

        let provenance = Provenance {
            id: installed.id.clone(),
            version: installed.version.to_string(),
            display_name: installed.display_name.clone(),
            sha256: installed.sha256.clone(),
            installed_at: installed.installed_at,
            enabled,
        };
        write_provenance(&installed.directory, &provenance)?;

        Ok(InstalledExtension {
            enabled,
            ..installed
        })
    }

    /// Start an extension in its own runtime thread.
    ///
    /// `host` is supplied by the caller because network policy, app version and
    /// the data directory are host decisions, not storage decisions.
    pub fn activate(
        &mut self,
        id: &str,
        host: Arc<HostServices>,
    ) -> Result<(), StoreError> {
        let installed = self.installed(id).ok_or_else(|| StoreError::NotInstalled {
            id: id.to_string(),
        })?;

        if !installed.enabled {
            return Err(StoreError::Disabled {
                id: id.to_string(),
            });
        }

        let source = fs::read_to_string(installed.index_path()).map_err(|source| StoreError::Io {
            path: installed.index_path(),
            source,
        })?;

        let runtime =
            ExtensionRuntime::load_with_host(id, &source, self.limits, host)?;

        let mut runtimes = self
            .runtimes
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        runtimes.insert(id.to_string(), Arc::new(runtime));

        Ok(())
    }

    /// Stop one runtime. Returns whether one was running.
    pub fn deactivate(&mut self, id: &str) -> bool {
        let mut runtimes = self
            .runtimes
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        runtimes.remove(id).is_some()
    }

    pub fn is_active(&self, id: &str) -> bool {
        self.runtimes
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .contains_key(id)
    }

    pub fn active_count(&self) -> usize {
        self.runtimes
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .len()
    }

    /// Borrow one running runtime without exposing the lock.
    pub fn with_runtime<T>(&self, id: &str, action: impl FnOnce(&ExtensionRuntime) -> T) -> Option<T> {
        let runtimes = self
            .runtimes
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        runtimes.get(id).map(|runtime| action(runtime))
    }

    /// Clone a shared handle to one running runtime.
    ///
    /// The lock is held only for the lookup: a caller that keeps the handle can
    /// issue calls from its own thread for as long as the runtime lives, which is
    /// what a long download needs without pinning the store's other callers.
    /// `with_runtime` is still the right tool for short, borrowed calls.
    pub fn runtime_of(&self, id: &str) -> Option<Arc<ExtensionRuntime>> {
        self.runtimes
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .get(id)
            .cloned()
    }

    /// `<root>/<id>`, with the id checked so the result is always a direct child.
    fn package_dir(&self, id: &str) -> Result<PathBuf, StoreError> {
        validate_id(id)?;
        Ok(self.root.join(id))
    }

    fn read_package(&self, id: &str, directory: &Path) -> Result<InstalledExtension, String> {
        let manifest_text = fs::read_to_string(directory.join("manifest.json"))
            .map_err(|error| format!("manifest.json is unreadable: {error}"))?;
        let manifest = Manifest::parse(&manifest_text)
            .map_err(|error| format!("manifest.json is not valid: {error}"))?;

        if manifest.name != id {
            return Err(format!(
                "the directory is {id:?} but the manifest names {:?}",
                manifest.name
            ));
        }

        let version = Version::parse(&manifest.version)
            .ok_or_else(|| format!("version {:?} is not comparable", manifest.version))?;

        // Provenance adds the digest, the install time and the enabled flag.
        // Its absence is not fatal: the package itself still describes what it
        // is, and losing those three fields should not hide an installation.
        let provenance = fs::read_to_string(directory.join(PROVENANCE_FILE))
            .ok()
            .and_then(|text| serde_json::from_str::<Provenance>(&text).ok())
            .filter(|provenance| provenance.version == manifest.version);

        Ok(InstalledExtension {
            id: id.to_string(),
            version,
            display_name: manifest
                .display_name
                .clone()
                .or_else(|| provenance.as_ref().and_then(|value| value.display_name.clone())),
            sha256: provenance
                .as_ref()
                .map(|value| value.sha256.clone())
                .unwrap_or_default(),
            installed_at: provenance
                .as_ref()
                .map(|value| value.installed_at)
                .unwrap_or_default(),
            enabled: provenance.map(|value| value.enabled).unwrap_or(true),
            directory: directory.to_path_buf(),
            data_directory: self.data_root.join(id),
        })
    }

    /// Restore every interrupted swap. Called once when the store opens.
    fn recover(&self) -> Result<(), StoreError> {
        let Ok(entries) = fs::read_dir(&self.staging_root) else {
            return Ok(());
        };

        for entry in entries.flatten() {
            let Some(name) = entry.file_name().to_str().map(str::to_string) else {
                continue;
            };
            self.recover_id(&name)?;
        }

        Ok(())
    }

    fn recover_id(&self, id: &str) -> Result<(), StoreError> {
        let staging = self.staging_root.join(id);
        if !staging.is_dir() {
            return Ok(());
        }

        let previous = staging.join("previous");
        let active = self.root.join(id);

        // The only window the swap opens is between "move the old version
        // aside" and "move the new one in". If the old version is still here
        // and nothing is active, the install did not finish.
        if previous.is_dir() && !active.exists() {
            fs::rename(&previous, &active).map_err(|source| StoreError::Io {
                path: active.clone(),
                source,
            })?;
        }

        remove_dir_all(&staging)?;
        Ok(())
    }
}

fn validate_id(id: &str) -> Result<(), StoreError> {
    let valid = !id.is_empty()
        && id.len() <= 64
        && !id.starts_with('.')
        && !id.contains("..")
        && id
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.'));

    if valid {
        Ok(())
    } else {
        Err(StoreError::InvalidId {
            value: id.to_string(),
        })
    }
}

fn write_provenance(directory: &Path, provenance: &Provenance) -> Result<(), StoreError> {
    let path = directory.join(PROVENANCE_FILE);
    let text = serde_json::to_string_pretty(provenance).map_err(|error| StoreError::Io {
        path: path.clone(),
        source: std::io::Error::other(error),
    })?;
    fs::write(&path, text).map_err(|source| StoreError::Io { path, source })
}

fn create_dir(path: &Path) -> Result<(), StoreError> {
    fs::create_dir_all(path).map_err(|source| StoreError::Io {
        path: path.to_path_buf(),
        source,
    })
}

fn remove_dir_all(path: &Path) -> Result<(), StoreError> {
    match fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        // Nothing to remove is the desired end state, not a failure.
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(StoreError::Io {
            path: path.to_path_buf(),
            source,
        }),
    }
}

fn now_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or_default()
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::Registry;
    use std::io::Write;
    use zip::write::SimpleFileOptions;

    const ID: &str = "test-ext";

    /// A manifest the validator accepts: safe name, comparable version, one kind.
    fn manifest_json(id: &str, version: &str) -> String {
        format!(
            r#"{{ "name": "{id}", "version": "{version}", "displayName": "Test Ext", "type": ["metadata_provider"] }}"#
        )
    }

    /// Build a zip at `directory/file_name` from (entry name, contents, unix mode).
    fn build_zip(
        directory: &Path,
        file_name: &str,
        entries: &[(&str, &[u8], Option<u32>)],
    ) -> PathBuf {
        let path = directory.join(file_name);
        let file = fs::File::create(&path).expect("create zip");
        let mut writer = zip::ZipWriter::new(file);

        for (name, contents, mode) in entries {
            let mut options =
                SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
            if let Some(mode) = mode {
                options = options.unix_permissions(*mode);
            }
            writer.start_file(*name, options).expect("start entry");
            writer.write_all(contents).expect("write entry");
        }

        writer.finish().expect("finish zip");
        path
    }

    fn build_package(
        directory: &Path,
        file_name: &str,
        id: &str,
        version: &str,
    ) -> PathBuf {
        build_zip(
            directory,
            file_name,
            &[
                ("manifest.json", manifest_json(id, version).as_bytes(), None),
                (
                    "index.js",
                    b"registerExtension({ ping: function () { return 1; } });".as_slice(),
                    None,
                ),
            ],
        )
    }

    fn registry_document(id: &str, sha256: &str) -> String {
        serde_json::json!({
            "version": 1,
            "extensions": [{
                "id": id,
                "name": id,
                "version": "1.0.0",
                "download_url": "https://example.test/ext.sflx",
                "sha256": sha256,
            }]
        })
        .to_string()
    }

    fn host_version() -> Version {
        Version::new(4, 10, 0)
    }

    /// Write `manifest.json` and `index.js` directly, for states the store has to
    /// recover from rather than states it produced.
    fn write_package_files(directory: &Path, id: &str, version: &str) {
        fs::create_dir_all(directory).expect("creates package dir");
        fs::write(directory.join("manifest.json"), manifest_json(id, version))
            .expect("writes manifest");
        fs::write(
            directory.join("index.js"),
            "registerExtension({ ping: function () { return 1; } });",
        )
        .expect("writes index");
    }

    #[test]
    fn install_then_uninstall_leaves_no_directory_behind() {
        let root = tempfile::tempdir().expect("tempdir");
        let packages = tempfile::tempdir().expect("tempdir");
        let package = build_package(packages.path(), "ext.sflx", ID, "1.10.4");

        let mut store = ExtensionStore::open(root.path()).expect("opens");
        let installed = store.install(&package).expect("installs");

        assert_eq!(installed.id, ID);
        assert_eq!(installed.version, Version::new(1, 10, 4));
        assert!(installed.enabled);
        assert_eq!(installed.directory, root.path().join(ID));
        assert_eq!(installed.data_directory, root.path().join(".data").join(ID));
        assert!(installed.index_path().is_file());

        store.uninstall(ID).expect("uninstalls");

        assert!(!root.path().join(ID).exists(), "package directory survived");
        assert!(
            !root.path().join(".data").join(ID).exists(),
            "data directory survived"
        );
        assert!(
            !root.path().join(".staging").join(ID).exists(),
            "staging directory survived"
        );
        assert!(store.installed(ID).is_none());

        let scan = store.scan();
        assert!(scan.installed.is_empty(), "{scan:?}");
        assert!(
            scan.broken.is_empty(),
            ".data and .staging must not be reported as broken: {scan:?}"
        );
    }

    #[test]
    fn no_path_outside_the_root_is_written() {
        let parent = tempfile::tempdir().expect("tempdir");
        let root = parent.path().join("extensions");
        let packages = tempfile::tempdir().expect("tempdir");
        let package = build_package(packages.path(), "ext.sflx", ID, "1.0.0");
        let marker = parent.path().join("keep.txt");
        fs::write(&marker, "untouched").expect("writes marker");

        let mut store = ExtensionStore::open(&root).expect("opens");
        store.install(&package).expect("installs");
        store.uninstall(ID).expect("uninstalls");

        assert_eq!(fs::read_to_string(&marker).expect("readable"), "untouched");

        let siblings: Vec<String> = fs::read_dir(parent.path())
            .expect("reads parent")
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().to_string())
            .collect();
        assert_eq!(
            siblings.len(),
            2,
            "the store wrote outside its root: {siblings:?}"
        );
        assert!(siblings.contains(&"keep.txt".to_string()), "{siblings:?}");
        assert!(siblings.contains(&"extensions".to_string()), "{siblings:?}");
    }

    #[test]
    fn a_name_that_is_a_path_never_becomes_a_directory() {
        let root = tempfile::tempdir().expect("tempdir");
        let packages = tempfile::tempdir().expect("tempdir");
        let mut store = ExtensionStore::open(root.path()).expect("opens");

        // The manifest validator rejects the name before anything is written.
        let package = build_package(packages.path(), "evil.sflx", "../escape", "1.0.0");
        let error = store.install(&package).expect_err("refused");
        assert!(
            matches!(error, StoreError::Package(_)),
            "expected the package validator to refuse the name, got {error}"
        );

        // And the store's own id check refuses it even when the manifest does not
        // reach validation, so `uninstall` cannot be aimed outside the root.
        let error = store.uninstall("../escape").expect_err("refused");
        assert!(matches!(error, StoreError::InvalidId { .. }), "{error}");
        assert!(!root.path().parent().unwrap().join("escape").exists());
    }

    #[test]
    fn downgrading_an_installed_version_is_refused_with_a_typed_error() {
        let root = tempfile::tempdir().expect("tempdir");
        let packages = tempfile::tempdir().expect("tempdir");
        let mut store = ExtensionStore::open(root.path()).expect("opens");

        store
            .install(&build_package(packages.path(), "new.sflx", ID, "1.10.4"))
            .expect("installs 1.10.4");

        let error = store
            .install(&build_package(packages.path(), "old.sflx", ID, "1.10.3"))
            .expect_err("downgrade refused");

        match error {
            StoreError::Downgrade {
                installed, offered, ..
            } => {
                assert_eq!(installed, "1.10.4");
                assert_eq!(offered, "1.10.3");
            }
            other => panic!("expected a typed downgrade refusal, got {other}"),
        }

        assert_eq!(
            store.installed(ID).expect("still installed").version,
            Version::new(1, 10, 4),
            "the refused install must not have touched the package"
        );
    }

    #[test]
    fn reinstalling_the_same_version_is_not_an_update() {
        let root = tempfile::tempdir().expect("tempdir");
        let packages = tempfile::tempdir().expect("tempdir");
        let mut store = ExtensionStore::open(root.path()).expect("opens");

        store
            .install(&build_package(packages.path(), "first.sflx", ID, "1.10.4"))
            .expect("installs");

        let error = store
            .install(&build_package(packages.path(), "second.sflx", ID, "1.10.4"))
            .expect_err("equal version refused");

        assert!(
            matches!(error, StoreError::AlreadyInstalled { .. }),
            "expected AlreadyInstalled, got {error}"
        );
    }

    #[test]
    fn an_update_replaces_the_package_and_keeps_its_data() {
        let root = tempfile::tempdir().expect("tempdir");
        let packages = tempfile::tempdir().expect("tempdir");
        let mut store = ExtensionStore::open(root.path()).expect("opens");

        let installed = store
            .install(&build_package(packages.path(), "v1.sflx", ID, "1.0.0"))
            .expect("installs 1.0.0");

        // A credential an extension saved must survive the publisher's bump.
        let credential = installed.data_directory.join("session.json");
        fs::write(&credential, r#"{"token":"kept"}"#).expect("writes data");

        let updated = store
            .install(&build_package(packages.path(), "v2.sflx", ID, "1.1.0"))
            .expect("updates");

        assert_eq!(updated.version, Version::new(1, 1, 0));
        assert_eq!(updated.directory, root.path().join(ID));
        assert_eq!(
            fs::read_to_string(&credential).expect("data survives the update"),
            r#"{"token":"kept"}"#
        );

        let scan = store.scan();
        assert_eq!(scan.installed.len(), 1, "{scan:?}");
        assert_eq!(scan.installed[0].version, Version::new(1, 1, 0));
        assert!(scan.broken.is_empty(), "{scan:?}");
    }

    #[test]
    fn an_update_keeps_the_extension_disabled_when_the_user_turned_it_off() {
        let root = tempfile::tempdir().expect("tempdir");
        let packages = tempfile::tempdir().expect("tempdir");
        let mut store = ExtensionStore::open(root.path()).expect("opens");

        store
            .install(&build_package(packages.path(), "v1.sflx", ID, "1.0.0"))
            .expect("installs");
        store.set_enabled(ID, false).expect("disables");

        let updated = store
            .install(&build_package(packages.path(), "v2.sflx", ID, "1.1.0"))
            .expect("updates");

        assert!(!updated.enabled, "an update must not re-enable a package");
    }

    #[test]
    fn an_interrupted_swap_restores_the_previous_version() {
        let root = tempfile::tempdir().expect("tempdir");
        let packages = tempfile::tempdir().expect("tempdir");

        {
            let mut store = ExtensionStore::open(root.path()).expect("opens");
            store
                .install(&build_package(packages.path(), "v1.sflx", ID, "1.0.0"))
                .expect("installs");
        }

        // The crash window: the old version has been moved aside and the new one
        // has not arrived. Nothing is serving the package at this instant.
        let active = root.path().join(ID);
        let previous = root.path().join(".staging").join(ID).join("previous");
        fs::create_dir_all(previous.parent().expect("parent")).expect("creates staging");
        fs::rename(&active, &previous).expect("moves the old version aside");
        assert!(!active.exists());

        let store = ExtensionStore::open(root.path()).expect("reopens");

        let restored = store.installed(ID).expect("the old version is back");
        assert_eq!(restored.version, Version::new(1, 0, 0));
        assert!(restored.index_path().is_file());
        assert!(
            !root.path().join(".staging").join(ID).exists(),
            "staging must be cleared after recovery"
        );
        assert!(store.scan().broken.is_empty());
    }

    #[test]
    fn a_staged_install_that_never_swapped_is_discarded() {
        let root = tempfile::tempdir().expect("tempdir");

        // A fresh install that died mid-extraction: files exist under staging,
        // nothing is active. It must not become a half-enabled extension.
        let staged = root.path().join(".staging").join(ID).join("new");
        write_package_files(&staged, ID, "1.0.0");

        let store = ExtensionStore::open(root.path()).expect("opens");

        assert!(store.installed(ID).is_none(), "a staged package was served");
        assert!(
            !root.path().join(".staging").join(ID).exists(),
            "the aborted install was not cleaned up"
        );
        assert!(store.scan().installed.is_empty());
    }

    #[test]
    fn a_committed_new_version_survives_a_crash_before_cleanup() {
        let root = tempfile::tempdir().expect("tempdir");
        let packages = tempfile::tempdir().expect("tempdir");

        {
            let mut store = ExtensionStore::open(root.path()).expect("opens");
            store
                .install(&build_package(packages.path(), "v1.sflx", ID, "1.0.0"))
                .expect("installs");
        }

        // The other crash window: the new version is in place and the old one is
        // still parked under staging. Recovery must keep the new one.
        let active = root.path().join(ID);
        let previous = root.path().join(".staging").join(ID).join("previous");
        fs::create_dir_all(previous.parent().expect("parent")).expect("creates staging");
        write_package_files(&previous, ID, "0.9.0");
        assert!(active.is_dir());

        let store = ExtensionStore::open(root.path()).expect("reopens");

        assert_eq!(
            store.installed(ID).expect("still installed").version,
            Version::new(1, 0, 0),
            "recovery must not roll a committed install back"
        );
        assert!(!root.path().join(".staging").join(ID).exists());
    }

    #[test]
    fn uninstall_stops_the_runtime_before_deleting_files() {
        let root = tempfile::tempdir().expect("tempdir");
        let packages = tempfile::tempdir().expect("tempdir");
        let mut store = ExtensionStore::open(root.path()).expect("opens");

        store
            .install(&build_package(packages.path(), "ext.sflx", ID, "1.0.0"))
            .expect("installs");
        store
            .activate(ID, Arc::new(HostServices::default()))
            .expect("starts");
        assert!(store.is_active(ID));

        store.uninstall(ID).expect("uninstalls");

        // The runtime is gone before its bytes are, so no thread is left reading
        // a deleted package.
        assert!(!store.is_active(ID));
        assert_eq!(store.active_count(), 0);
        assert!(!root.path().join(ID).exists());
        assert!(store.with_runtime(ID, |_| ()).is_none());
    }

    #[test]
    fn disabling_keeps_the_files_and_refuses_activation() {
        let root = tempfile::tempdir().expect("tempdir");
        let packages = tempfile::tempdir().expect("tempdir");
        let mut store = ExtensionStore::open(root.path()).expect("opens");

        store
            .install(&build_package(packages.path(), "ext.sflx", ID, "1.0.0"))
            .expect("installs");
        store
            .activate(ID, Arc::new(HostServices::default()))
            .expect("starts");

        let disabled = store.set_enabled(ID, false).expect("disables");
        assert!(!disabled.enabled);
        // Disabling stops a running runtime without deleting the package.
        assert!(!store.is_active(ID));
        assert!(disabled.index_path().is_file(), "disable deleted files");

        let error = store
            .activate(ID, Arc::new(HostServices::default()))
            .expect_err("disabled extension must not start");
        assert!(matches!(error, StoreError::Disabled { .. }), "{error}");

        store.set_enabled(ID, true).expect("re-enables");
        store
            .activate(ID, Arc::new(HostServices::default()))
            .expect("starts again");
        assert!(store.is_active(ID));

        // The flag survives a reopen because it lives in provenance.json.
        drop(store);
        let store = ExtensionStore::open(root.path()).expect("reopens");
        assert!(store.installed(ID).expect("installed").enabled);
    }

    #[test]
    fn scan_reports_a_broken_package_without_hiding_the_good_one() {
        let root = tempfile::tempdir().expect("tempdir");
        let packages = tempfile::tempdir().expect("tempdir");
        let mut store = ExtensionStore::open(root.path()).expect("opens");

        store
            .install(&build_package(packages.path(), "good.sflx", "good-ext", "1.0.0"))
            .expect("installs");

        let broken = root.path().join("broken-ext");
        fs::create_dir_all(&broken).expect("creates dir");
        fs::write(broken.join("manifest.json"), "{ not json").expect("writes manifest");

        let scan = store.scan();

        assert_eq!(scan.installed.len(), 1, "{scan:?}");
        assert_eq!(scan.installed[0].id, "good-ext");
        assert_eq!(scan.broken.len(), 1, "{scan:?}");
        assert_eq!(scan.broken[0].id, "broken-ext");
        assert!(
            !scan.broken[0].reason.is_empty(),
            "a broken entry must say why"
        );
        assert!(store.installed("broken-ext").is_none());
    }

    #[test]
    fn a_directory_whose_manifest_names_something_else_is_broken_not_installed() {
        let root = tempfile::tempdir().expect("tempdir");
        let store = ExtensionStore::open(root.path()).expect("opens");

        write_package_files(&root.path().join("dir-name"), "other-name", "1.0.0");

        let scan = store.scan();
        assert!(scan.installed.is_empty(), "{scan:?}");
        assert_eq!(scan.broken.len(), 1, "{scan:?}");
        assert_eq!(scan.broken[0].id, "dir-name");
    }

    #[test]
    fn uninstalling_something_that_is_not_installed_is_a_typed_error() {
        let root = tempfile::tempdir().expect("tempdir");
        let mut store = ExtensionStore::open(root.path()).expect("opens");

        let error = store.uninstall("missing-ext").expect_err("refused");
        assert!(matches!(error, StoreError::NotInstalled { .. }), "{error}");

        let error = store.set_enabled("missing-ext", false).expect_err("refused");
        assert!(matches!(error, StoreError::NotInstalled { .. }), "{error}");
    }

    #[test]
    fn a_registry_install_verifies_the_digest_and_the_id() {
        let root = tempfile::tempdir().expect("tempdir");
        let packages = tempfile::tempdir().expect("tempdir");
        let package = build_package(packages.path(), "ext.sflx", ID, "1.0.0");
        let digest = PackageInfo::inspect(&package).expect("inspects").sha256;

        let registry =
            Registry::parse(&registry_document(ID, &digest), &host_version()).expect("parses");
        let entry = registry.entry(ID).expect("entry");

        let mut store = ExtensionStore::open(root.path()).expect("opens");
        let installed = store
            .install_from_registry(entry, &package)
            .expect("registry install");

        assert_eq!(installed.id, ID);
        assert_eq!(installed.sha256, digest);
        assert!(installed.index_path().is_file());
    }

    #[test]
    fn a_checksum_that_does_not_match_the_registry_stops_the_install() {
        let root = tempfile::tempdir().expect("tempdir");
        let packages = tempfile::tempdir().expect("tempdir");
        let package = build_package(packages.path(), "ext.sflx", ID, "1.0.0");
        let published = "a".repeat(64);

        let registry =
            Registry::parse(&registry_document(ID, &published), &host_version()).expect("parses");
        let entry = registry.entry(ID).expect("entry");

        let mut store = ExtensionStore::open(root.path()).expect("opens");
        let error = store
            .install_from_registry(entry, &package)
            .expect_err("refused");

        match error {
            StoreError::ChecksumMismatch { expected, .. } => assert_eq!(expected, published),
            other => panic!("expected a checksum refusal, got {other}"),
        }
        assert!(store.installed(ID).is_none());
        assert!(!root.path().join(ID).exists(), "nothing may be written");
        assert!(!root.path().join(".staging").join(ID).exists());
    }

    #[test]
    fn a_package_that_names_a_different_id_than_the_registry_stops_the_install() {
        let root = tempfile::tempdir().expect("tempdir");
        let packages = tempfile::tempdir().expect("tempdir");
        let package = build_package(packages.path(), "ext.sflx", ID, "1.0.0");
        let digest = PackageInfo::inspect(&package).expect("inspects").sha256;

        let registry = Registry::parse(
            &registry_document("other-ext", &digest),
            &host_version(),
        )
        .expect("parses");
        let entry = registry.entry("other-ext").expect("entry");

        let mut store = ExtensionStore::open(root.path()).expect("opens");
        let error = store
            .install_from_registry(entry, &package)
            .expect_err("refused");

        match error {
            StoreError::IdMismatch {
                expected, actual, ..
            } => {
                assert_eq!(expected, "other-ext");
                assert_eq!(actual, ID);
            }
            other => panic!("expected an id mismatch, got {other}"),
        }
    }

    #[test]
    fn a_registry_entry_this_host_cannot_run_is_refused_before_the_digest_check() {
        let root = tempfile::tempdir().expect("tempdir");
        let packages = tempfile::tempdir().expect("tempdir");
        let package = build_package(packages.path(), "ext.sflx", ID, "1.0.0");

        // A version this host can never satisfy, published with a sound digest.
        let digest = PackageInfo::inspect(&package).expect("inspects").sha256;
        let document = serde_json::json!({
            "version": 1,
            "extensions": [{
                "id": ID,
                "name": ID,
                "version": "1.0.0",
                "min_app_version": "9.0.0",
                "download_url": "https://example.test/ext.sflx",
                "sha256": digest,
            }]
        })
        .to_string();

        let registry = Registry::parse(&document, &host_version()).expect("parses");
        let entry = registry.entry(ID).expect("entry");
        assert!(!entry.status.is_supported());

        let mut store = ExtensionStore::open(root.path()).expect("opens");
        let error = store
            .install_from_registry(entry, &package)
            .expect_err("refused");

        match error {
            StoreError::Refused { id, reason } => {
                assert_eq!(id, ID);
                assert_eq!(reason, "host_too_old");
            }
            other => panic!("expected a refusal, got {other}"),
        }
        assert!(!root.path().join(ID).exists());
    }

    #[test]
    fn a_package_version_that_is_not_comparable_is_refused() {
        let root = tempfile::tempdir().expect("tempdir");
        let packages = tempfile::tempdir().expect("tempdir");

        // The manifest validator accepts any non-empty version string, so the
        // store is the layer that must refuse one it cannot order.
        let package = build_package(packages.path(), "odd.sflx", ID, "1.10.4-ko");

        let mut store = ExtensionStore::open(root.path()).expect("opens");
        let installed = store
            .install(&package)
            .expect("a prerelease is a comparable version");
        assert_eq!(installed.version.to_string(), "1.10.4-ko");

        let package = build_zip(
            packages.path(),
            "bad.sflx",
            &[
                ("manifest.json", manifest_json("bad-ext", "nightly").as_bytes(), None),
                ("index.js", b"registerExtension({});".as_slice(), None),
            ],
        );
        let error = store.install(&package).expect_err("refused");
        assert!(
            matches!(error, StoreError::InvalidVersion { .. }),
            "expected InvalidVersion, got {error}"
        );
        assert!(!root.path().join("bad-ext").exists());
    }

    #[test]
    fn a_reopen_finds_what_was_installed() {
        let root = tempfile::tempdir().expect("tempdir");
        let packages = tempfile::tempdir().expect("tempdir");

        let digest = {
            let mut store = ExtensionStore::open(root.path()).expect("opens");
            store
                .install(&build_package(packages.path(), "ext.sflx", ID, "1.10.4"))
                .expect("installs");
            store.installed(ID).expect("installed").sha256
        };

        let store = ExtensionStore::open(root.path()).expect("reopens");
        let installed = store.installed(ID).expect("still installed");

        assert_eq!(installed.version, Version::new(1, 10, 4));
        assert_eq!(installed.display_name.as_deref(), Some("Test Ext"));
        assert_eq!(installed.sha256, digest, "provenance survives a reopen");
        assert!(installed.installed_at > 0);
        assert!(installed.enabled);
    }

    #[test]
    fn a_package_without_provenance_is_still_installed() {
        let root = tempfile::tempdir().expect("tempdir");
        let store = ExtensionStore::open(root.path()).expect("opens");

        // An install placed there by hand (or by an older host) has no digest and
        // no install time, but it is a real package and must not be hidden.
        write_package_files(&root.path().join(ID), ID, "2.0.0");

        let installed = store.installed(ID).expect("installed");
        assert_eq!(installed.version, Version::new(2, 0, 0));
        assert_eq!(installed.sha256, "");
        assert!(installed.enabled);
        assert!(store.scan().broken.is_empty());
    }

    #[test]
    fn the_data_directory_is_created_outside_the_package_directory() {
        let root = tempfile::tempdir().expect("tempdir");
        let packages = tempfile::tempdir().expect("tempdir");
        let mut store = ExtensionStore::open(root.path()).expect("opens");

        let installed = store
            .install(&build_package(packages.path(), "ext.sflx", ID, "1.0.0"))
            .expect("installs");

        assert!(installed.data_directory.is_dir());
        assert!(
            !installed.data_directory.starts_with(&installed.directory),
            "the data directory must survive a package replacement"
        );
        assert_eq!(installed.data_directory, store.data_root().join(ID));
    }
}