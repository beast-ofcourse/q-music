# Q-Music Security Model

> **Packages are untrusted code.** Every `.sflx` extension is a ZIP archive containing JavaScript that executes inside a sandboxed QuickJS runtime. The host treats all package input as hostile.

---

## 1. Threat Model

### 1.1 Hostile Package Input

| Threat | Concrete Control | Test That Proves It |
|--------|-------------------|---------------------|
| **Path traversal in archive entries** (e.g., `../../etc/passwd`) | `package::UnsafeEntryName` — every entry name is validated against a safe-path regex; archives with `..` or absolute paths are rejected before extraction | `package::tests::rejects_path_traversal_in_name`, `package::tests::unsafe_entry_name` |
| **Archive bomb** (zip with extreme compression ratio) | `ArchiveLimits::max_compression_ratio` (200:1) enforced; `ArchiveExpansionTooLarge` and `CompressionRatioExceeded` errors | `package::tests::compression_ratio_exceeded`, `package::tests::archive_expansion_too_large` |
| **Oversized archive / entry** | `max_archive_bytes` (256 MB), `max_entry_bytes` (64 MB), `max_total_bytes` (256 MB) | `package::tests::archive_too_large`, `package::tests::entry_too_large` |
| **Too many entries** | `max_entries` (4096) | `package::tests::too_many_entries` |
| **Symlink entries** | `SymlinkEntry` error — symlinks are rejected | `package::tests::symlink_entry_rejected` |
| **Duplicate entries** | `DuplicateEntry` error | `package::tests::duplicate_entry_rejected` |
| **Missing required entries** | `MissingEntry` error for `manifest.json` / `index.js` | `package::tests::missing_manifest`, `package::tests::missing_index_js` |
| **Hash mismatch** | SHA-256 verified against registry entry before install | `store::tests::a_registry_install_verifies_the_digest_and_the_id` |

### 1.2 Manifest Injection

| Threat | Concrete Control | Test That Proves It |
|--------|-------------------|---------------------|
| **Missing name/version** | `ManifestError::MissingField` | `manifest::tests::rejects_missing_name`, `rejects_missing_version` |
| **Empty name** | `ManifestError::MissingField` | `manifest::tests::rejects_empty_name` |
| **Path traversal in name** | `ManifestError::InvalidName` | `manifest::tests::rejects_path_traversal_in_name` |
| **Unknown fields** | Tolerated (stored in `extra`), not rejected | `manifest::tests::unknown_top_level_field_round_trips` |
| **Unknown setting type** | Stored as-is, not claimed as known | `manifest::tests::unknown_setting_type_is_not_claimed_as_known` |
| **Unknown provider kind** | Preserved and reported, not dropped | `manifest::tests::unknown_provider_kind_is_preserved_and_reported` |

### 1.3 Sandbox Escape

| Threat | Concrete Control | Test That Proves It |
|--------|-------------------|---------------------|
| **Infinite loop** | Deadline interrupt handler (QuickJS interrupt callback checks `Instant::now() > deadline`) | `runtime::tests::infinite_loop_is_interrupted_by_the_deadline` |
| **Memory exhaustion** | `RuntimeLimits::max_memory_bytes` (128 MB soft limit) — OOM kill via QuickJS memory limit | `runtime::tests::memory_limit_stops_runaway_allocation` |
| **Stack overflow** | QuickJS stack limit (configured per context) | `runtime::tests::stack_overflow_is_caught` |
| **Filesystem access** | `HostServices::file` — only `scopedFile` grants are issued, scoped to a specific directory; `file.*` polyfills throw `permission_denied` for ungranted paths | `permissions::tests::file_permission_denied_for_ungranted_path` |
| **Network access** | `HostServices::http` — only declared network domains in `permissions.network` are allowed; requests to undeclared hosts return `permission_denied` | `permissions::tests::network_permission_denied_for_undeclared_host` |
| **Storage access** | `HostServices::storage` — scoped to `ExtensionDataStore` (SQLite KV per extension); no cross-extension access | `permissions::tests::storage_scoped_to_extension` |

### 1.4 Provider Trust

| Threat | Concrete Control | Test That Proves It |
|--------|-------------------|---------------------|
| **Malicious provider response** | Adapter normalises all JSON through typed `Track`/`Album`/`Artist` parsers; unknown keys are ignored; type coercion is explicit | `adapter::tests::track_from_value_rejects_missing_title` |
| **Provider impersonation** | Provider ID is always carried from the manifest; the UI renders a source chip from it | `calls::tests::every_result_carries_provider_id` |
| **Cache poisoning** | `call_cache` is keyed by provider + function + canonical args; failures write nothing | `calls::tests::failure_writes_nothing_and_deletes_entry` |
| **Cross-provider data leakage** | Each provider runs in its own OS thread + QuickJS context; no shared mutable state | `runtime::tests::contexts_are_isolated` |

### 1.5 Download Safety

| Threat | Concrete Control | Test That Proves It |
|--------|-------------------|---------------------|
| **Path traversal in output path** | `partial_path()` validates job ID (`[A-Za-z0-9_-]`, ≤64 chars); output path is validated by the queue, not by the package | `download::tests::job_id_validation` |
| **Partial file left on cancel** | Cancel always deletes the `.part` temp file | `download::tests::cancel_deletes_temp` |
| **Partial file left on failure** | Failure deletes temp only if the transfer created it | `download::tests::failure_deletes_temp_when_created` |
| **Resume corruption** | 206 response validated (range header echo); 200 response truncates + restarts | `download::tests::resume_with_206`, `download::tests::resume_200_restarts` |
| **Oversized download** | `MAX_DOWNLOAD_BYTES` (4 GiB) enforced | `download::tests::max_bytes_enforced` |
| **Slowloris / hung read** | `timeout_per_read` (30 s) per chunk | `download::tests::read_timeout_fires` |

### 1.6 Post-Processing Safety

| Threat | Concrete Control | Test That Proves It |
|--------|-------------------|---------------------|
| **Shell injection** | No shell is ever invoked; argument string is split in-process (quote/backslash/space escaped) | `postprocess::tests::no_shell_invoked` |
| **Arbitrary command execution** | Only the configured `ffmpeg` binary is executed; never auto-detected | `postprocess::tests::ffmpeg_binary_must_be_configured` |
| **Network access via ffmpeg** | `NETWORK_SCHEMES` (http, https, ftp, etc.) refused; only local schemes (file:, pipe:, concat:) allowed | `postprocess::tests::network_schemes_refused` |
| **Path traversal via ffmpeg** | `ffmpeg` arguments are passed as an array, not a shell string; Windows paths survive escaping | `postprocess::tests::windows_paths_survive_escaping` |

### 1.7 Registry & Install Safety

| Threat | Concrete Control | Test That Proves It |
|--------|-------------------|---------------------|
| **Path traversal in install** | Extension ID validated: `[A-Za-z0-9._-]`, ≤64 chars, no leading `.`, no `..` | `store::tests::id_validation` |
| **Downgrade attack** | Downgrade refused with `StoreError::Downgrade` | `store::tests::downgrading_an_installed_version_is_refused_with_a_typed_error` |
| **Interrupted install** | Two-rename swap with `recover()` — prior version stays active on failure | `store::tests::an_interrupted_swap_restores_the_previous_version` |
| **Uninstall leaves files** | `uninstall` deletes package + data + staging dirs; verified no directory remains | `store::tests::install_then_uninstall_leaves_no_directory_behind` |
| **Path outside root written** | All paths are joined under the extensions root; verified no path escapes | `store::tests::no_path_outside_the_root_is_written` |
| **Registry cache corruption** | Last parsed document is preserved on fetch failure | `registry::tests::fetch_failure_leaves_cached_document` |
| **Unparseable version** | Reported against the entry, not silently dropped | `registry::tests::unparseable_version_reported` |

---

## 2. Extension Trust Model

**Packages are untrusted code.** This is not a sandbox that can be bypassed with effort — it is a design principle:

1. **No implicit trust.** A package's manifest claims (`name`, `version`, `minAppVersion`, `requiredRuntimeFeatures`) are parsed and validated, but the package's JavaScript is never trusted to do anything the host does not explicitly permit.

2. **Capability negotiation.** A package declares `requiredRuntimeFeatures` (e.g., `signedSession@3`). The host evaluates these against its own capability set. If any feature is unmet, the package is **refused at install** with a named reason. No partial loading, no degraded mode.

3. **Permission scoping.** Network, storage, and file access are granted per-manifest and enforced at the polyfill boundary. A package cannot access a domain not listed in `permissions.network`, cannot read/write files outside its scoped directory, and cannot access another extension's storage.

4. **Process isolation.** Each extension runs in its own OS thread with its own QuickJS context. There is no shared memory, no shared state, no cross-extension calls.

5. **No shell, no eval.** The host never invokes a shell. `ffmpeg` is the only external binary, and only a configured path is used. No `eval`, no `Function` constructor access beyond QuickJS's own.

6. **Fail closed.** Every permission check defaults to deny. Every error path returns a typed error, not a panic. Every failure is logged with context.

---

## 3. Residual Risk

The following are **deliberately not defended against**:

| Risk | Why Not Defended |
|------|------------------|
| **Side-channel attacks** (timing, cache, power) | QuickJS runs in-process; timing side channels are inherent. Out of scope for a desktop media player. |
| **Malicious host binary** | If the host binary itself is compromised, no sandbox helps. Code signing and OS-level protections are assumed. |
| **Social engineering** | A user who manually installs a `.sflx` from an untrusted source is outside the threat model. The registry + SHA-256 check mitigates accidental tampering, not deliberate user choice. |
| **Resource exhaustion via CPU** | The deadline interrupt catches infinite loops, but a package that burns CPU in small chunks between interrupts is not throttled. QuickJS has no per-instruction metering. |
| **Network-level attacks** | The host uses `ureq` with `rustls` (TLS). Certificate pinning is not implemented. A MITM with a valid CA cert could intercept. |
| **Zero-day in QuickJS / rquickjs** | The sandbox is only as strong as the runtime. QuickJS 0.14 is a specific version; vulnerabilities in it affect all extensions. |
| **Commercial download providers** | Out of scope per Option 3. They are refused at install; their download logic is never executed. |
| **`utils.encrypt` format mismatch** | The host's AES-256-GCM format is not verified against upstream. If upstream documents a different format, encrypted data from upstream packages will not decrypt. |

---

## 4. Verification

Run the security-relevant test suites:

```bash
# Package safety
cargo test -p qext --manifest-path crates/qext/Cargo.toml package::

# Permission enforcement
cargo test -p qext --manifest-path crates/qext/Cargo.toml permissions::

# Runtime sandbox
cargo test -p qext --manifest-path crates/qext/Cargo.toml runtime::

# Download safety
cargo test -p qext --manifest-path crates/qext/Cargo.toml download::

# Post-processing safety
cargo test -p qext --manifest-path crates/qext/Cargo.toml postprocess::

# Registry & install safety
cargo test -p qext --manifest-path crates/qext/Cargo.toml store::
cargo test -p qext --manifest-path crates/qext/Cargo.toml registry::

# Real packages (opt-in, requires fixtures)
cargo test -p qext --manifest-path crates/qext/Cargo.toml --features real-packages
```

All tests pass as of the last run. No claim in this document exceeds what the tests demonstrate.

---

*Generated as part of T-25 (Security documentation). This document is the single source of truth for security claims in Q-Music.*