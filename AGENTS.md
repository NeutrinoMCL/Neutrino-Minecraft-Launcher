# Neutrino Minecraft Launcher — Agent Notes

Rust 2024 + Slint 1.18 Windows desktop Minecraft launcher in a single crate: shared library
(`src/lib.rs`), GUI binary, and the `neutrino-install` CLI. Read `README.md` for behavior/config
and `CONTRIBUTING.md` for PR rules; this file records what is easy to get wrong.

## Commands

```sh
cargo run                                   # GUI (blocks while the window is open)
cargo run --bin neutrino-install -- --list  # CLI installer; see --help
cargo run --bin neutrino-install -- 1.20.4 --dry-run

cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features
cargo test <name>                           # single test; tests use fixtures and need no network
```

CI (`.github/workflows/ci.yml`, `windows-latest`) runs exactly fmt → clippy → test. There is no
`tests/` directory; add `#[cfg(test)]` modules beside the code. After `.slint` edits, use
`slint-viewer --check ui/app-window.slint` or run the app — compiling is not enough for layout changes.

## Wiring

- `src/lib.rs` exposes `core`, `java`, `instances`. The GUI imports `java` / `instances` from the
  library (`use neutrino_minecraft_launcher::{instances, java};`); do not declare duplicate
  `mod` entries in `src/main.rs`, or those files compile (and their tests run) twice.
- `src/desktop.rs` (`InstallController`) is the main GUI↔core bridge for version lists, instance
  creation, discover, installs, and downloads. Slint properties are UI-thread only: background work
  must return via `Weak::upgrade_in_event_loop`, and progress callbacks capture the controller
  (`Arc<Mutex<_>>`), not a bare `Weak`. `main.rs` also calls `core::launch` / `core::paths` directly.
  Discover search is stored on the controller (`Actions.search-projects`); Modrinth receives `query`,
  CurseForge `searchFilter`, combined with the existing filters/sort, 50 results per request. Project
  rows/detail headers show `ProjectInfo.tags` (`project_tags` on the controller: client/server
  support, API categories, recognized loaders). Project versions are grouped by game version
  (`ProjectVersionGroup`); loader tags come from `loader_tags` (mod loaders, or Iris/OptiFine/Canvas
  for shaders). `Actions.open-instance-download` pre-filters the download dialog by the instance's
  game version/loader; `load_versions` sorts groups by manifest rank and marks a `recommended`
  entry (UI shows it in `AppState.recommended-version`) when the download target is an instance. The
  `image` crate must keep the `webp` feature enabled because most Modrinth project icons are WebP.
- Networking is blocking (`ureq` / `Downloader`) and background jobs are plain `std::thread`s; there
  is no async runtime. Don't introduce one without discussing it first.
- Instance content scanning (`InstanceContentLoader` in `src/main.rs`) is deliberately two-phase on a
  background thread (names first, then archive metadata/icons); do not move that IO back into the
  `load-instance-content` callback, or selecting an instance freezes the window. `instances.rs`
  picks icons from metadata (`fabric.mod.json` `icon`, Forge `logoFile`) before falling back to
  `pack.png`, root `*-icon.png` / `logo.png`, and `assets/<namespace>/icon.*`; PNG/WebP/JPEG/GIF
  names are accepted. The same loader also
  publishes instance loader metadata (from `versions/<id>/neutrino.json`, falling back to Fabric/Forge
  library coordinates). The mods tab shows that loader above the list; `LoaderSwitchDialog` drives
  `refresh-instance-loader-versions` / `switch-instance-loader`. Switching re-runs `install_instance`
  for the same instance directory (single-task guarded) and must not touch `mods/`, saves or config.
- Version-install writes go through `core/install.rs`: `.part` → SHA-1 verify → rename, skipping
  valid files, so re-runs are idempotent. Reuse that path for new install writes (Forge's external
  installer is the exception). `install_instance` also writes `versions/<id>/neutrino.json` with the
  base game version, loader key and loader version; keep that in sync if the install options change.
  Discover content is downloaded separately by `desktop.rs`.
  `core/paths.rs` owns the `.minecraft` layout; never hand-join `versions/...`.
- `core/loader.rs`: Fabric reads Fabric Meta (no Java needed); Forge runs the official installer, so
  it needs a detected Java, temporarily installs the base version, then cleans it up.
  `core/launch.rs` accepts only merged self-contained instance JSON (it rejects `inheritsFrom`);
  `--gameDir`/cwd is `versions/<id>/` while `libraries/` and `assets/` stay shared; game output goes
  to `versions/<id>/logs/launcher.log`.
- `build.rs` compiles only `ui/app-window.slint`. New `.slint` files must be imported transitively
  from it or explicitly added, or they are never compiled. Reusable widgets (message strip, tag,
  action/refresh buttons, empty hint) live in `ui/components/common.slint` and are imported by
  views. Rust only sees globals/types exported from `app-window.slint` via
  `slint::include_modules!()`. `AppState` properties are `in`-only (Rust sets them); keep view-local
  state on the view component.
- `slint` and `slint-build` are pinned to 1.18.1 and must stay in sync. The 3D preview needs
  `unstable-wgpu-30`; keep the `wgpu` crate version and Slint feature aligned. For `.slint` work,
  use the vendored skill in `.agents/skills/slint/`.

## Runtime and repo gotchas

- `conf.toml` lookup order: `NEUTRINO_CONFIG` → `./conf.toml` (only if it already exists) →
  `%APPDATA%/Neutrino/conf.toml`; the settings page writes back to that same file. `./conf.toml` is
  not gitignored. Accounts/tokens live in `%APPDATA%/Neutrino/account.toml` (TOML; legacy
  `accounts.json` is migrated automatically on first load and removed after a successful write).
  Custom skins and offline capes are copied to `%APPDATA%/Neutrino/skins` and `.../capes`.
- `.env` (copy `.env.example`; ignored) supplies `NEUTRINO_MSA_CLIENT_ID` for Microsoft login and
  `CURSEFORGE_API_KEY` / legacy `NEUTRINO_CURSEFORGE_API_KEY`. The CurseForge key is read literally
  from the `.env` file, bypassing dotenvy's expansion (it would expand `$` inside keys); an external
  env var wins. Don't replace that reader with plain dotenvy.
- 3D preview selects a manual wgpu 30 backend at startup; if unavailable the app still runs without
  preview. `main.rs` intentionally `mem::forget`s the `Device`/`Queue` at shutdown to avoid a wgpu 30
  Windows panic — don't "fix" the leak without testing shutdown.
- **Do not re-enable Slint's `accessibility` feature.** `Cargo.toml` uses `default-features = false`;
  on some Windows 11 + accesskit_windows 0.34 setups window creation panics (`SetPropW` /
  `ERROR_NOT_ENOUGH_MEMORY`).
- Keep `.cargo/config.toml`: it raises the MSVC stack to 8 MB.
- `.delta/` is an ignored local worktree/clone area with stale copies; skip it in searches and edits.
- Installs and discover downloads each have a single-task guard and no cancellation. Discover
  downloads go to `default_game_dir()` (`%APPDATA%/.minecraft`) `mods/` / `shaderpacks/` /
  `resourcepacks/`, unless the instance download dialog set a target via
  `Actions.set-download-target`; then they go into that instance's folder. Instance downloads are
  recorded in `<instance>/neutrino-content.json` (source, project/version id), and
  `Actions.check-content-updates` / `update-content` use that file to offer updates; only tracked
  files can be updated. Modpack install is browse-only.
- The verified target is Windows MSVC and CI is Windows-only. When launching the GUI from a script,
  start it detached and confirm with `tasklist`; a direct `cargo run` blocks and `(./exe &)` may die
  with the shell.

## Conventions

- rustfmt defaults; Clippy warnings are errors — fix them rather than adding `#[allow]` without
  justification. `.editorconfig`: UTF-8, LF, final newline, 4-space Rust / 2-space Slint.
  Identifiers in English; user-facing UI text and docs in Chinese.
- Conventional Commits (`feat:`, `fix:`, `docs:`, `refactor:`, `chore:`); `main` is protected and all
  changes land via PR. Keep unrelated formatting out of feature commits.
