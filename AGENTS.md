# crumBS

Lists installed apps across pacman, dpkg, rpm, Flatpak, Snap, AppImages and unmanaged `.desktop` entries, finds their leftovers and reclaimable space, and removes what the user picks (to the trash where possible, with a history and `crumbs restore`). CLI plus an MCP server (`crumbs mcp`). Rust 2024, workspace of two crates, GPL-3.0-or-later. `README.md` is the user manual; `docs/json-schema.md` is the JSON contract; the design lives in `docs/superpowers/specs/`.

## Commands
- Format: `cargo fmt --all` (CI runs `cargo fmt --check`).
- Lint: `cargo clippy --workspace --all-targets --all-features -- -D warnings`.
- Test: `cargo test --workspace --all-features --locked`. Everything runs inside fixture trees (`crumbs_core::testing::FixtureTree`); no test touches the real home or package database.
- Release build check: `cargo build --workspace --release --locked` (release compiles out the test hooks).
- Licences: `scripts/check-licenses.sh`. Other distros: `scripts/distro-check.sh` (podman).
- Release artifacts: `scripts/release.sh <version>`; see README, "Releasing". Never tag, push a tag, publish a release or push to the AUR unless the user asks.

## Layout
- `crates/crumbs-core`: inventory (`sources/`), rules (`rules.rs`, `data/rules/apps.toml`), leftovers and orphans (`leftovers/`), bloat scanners (`bloat/`), planning (`plan.rs`, `removal/`), safety checks (`safety.rs`), execution (`executor/`), privilege batch (`privilege/`), history and restore.
- `crates/crumbs-cli`: commands in `src/cmd/` (shared confirm-and-execute flow in `cmd/act/`), MCP server in `src/mcp/`, integration tests in `tests/cli.rs` and `tests/mcp.rs` with insta snapshots in `tests/snapshots/`.
- `packaging/`: AppImage, AUR (`crum-bs`, `crum-bs-bin`), deb, rpm. `scripts/`: build and release.

## Conventions
- Minimum Rust is `rust-version` in `Cargo.toml` (1.99); CI pins the same toolchain.
- JSON output is versioned: within `schema_version` 1 fields may be added, never removed or renamed. Document every new field and exit-code case in `docs/json-schema.md`, and user-facing behaviour in `README.md` and `CHANGELOG.md` (`## Unreleased`).
- Every change to the filesystem goes through a plan, the safety checks and the history. Never add a path that deletes outside `safety.rs`'s scopes, and never weaken a protected path.
- Root steps run in one batch through run0, pkexec or sudo; the MCP server never uses sudo.
- Pin GitHub Actions by commit SHA with the version in a comment.

## uQ integration
crumBS ships as part of the uQ desktop (installed by bindle with uq, scdr and noti-fi).
- uQ's, scdr's, noti-fi's and appOFholding's own state are claimed by rules in `data/rules/apps.toml`, so they are never offered as leftovers or orphans. Keep those rules when the apps move their files.
- `model::UQ_PACKAGES` are the desktop's own packages: `App::part_of` is `"uQ"` for them and `uninstall` removes them only with a second confirmation or `--desktop` (`desktop: true` over MCP).
- appOFholding owns the AppImages in its library (`~/Applications` once its database exists): crumBS does not list, clean or uninstall them (`sources/appimage.rs`, `appofholding_library`).
- crumBS is GPL-3.0-or-later; uq and noti-fi are MIT and scdr is MPL-2.0. Do not copy crumBS code into them. uQ talks to crumBS through its CLI and JSON (`scan --json`, `clean --json`) and, later, a launcher provider.
