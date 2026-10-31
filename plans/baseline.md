# Baseline — recorded 2026-10-30

Captured before any extension-host work. All green.

| Check | Command | Result |
| --- | --- | --- |
| Core tests | `cargo test --manifest-path crates/qcore/Cargo.toml` | 40 passed, 0 failed (1.99 s) |
| Shell typecheck | `cargo check --manifest-path src-tauri/Cargo.toml` | Finished, no errors or warnings |
| TS typecheck | `npx tsc --noEmit` | exit 0, no output |
| Frontend build | `npx vite build` | built in 632 ms; 311.22 kB JS, 28.13 kB CSS |

Notes carried forward:

- PowerShell reports `Exited with code 1` after successful `cargo` runs because cargo writes
  progress to stderr. The `test result: ok` line is authoritative.
- `src-tauri/target` holds ~1.2 GB of `cargo check` artefacts. First `tauri dev` still requires
  full codegen.
- No git repository. Nothing committed.

Feasibility probe (outside the tree, `%TEMP%\rq-probe`):

- `rquickjs` 0.14 compiles on Windows MSVC with VS Build Tools 2022 and runs QuickJS in 65 s cold.
- All three Mornye fixture packages (`spotify-web`, `amazon`, `tidal-web`) evaluate and call
  `registerExtension()` with stubbed host globals.
- Zero network calls fire during load.
