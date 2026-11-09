# crumBS Milestone 6 (Packaging) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** crumbs can be installed without `cargo` as any of:
- a static x86_64 binary;
- an AppImage that needs no `libfuse2`;
- `.deb` and `.rpm` packages;
- the AUR packages `crumbs` (source build) and `crumbs-bin`.

Every artifact comes out of one local script, and a tag-triggered GitHub workflow produces a release with SHA256 sums.

**Architecture:** packaging is driven by shell scripts under `scripts/`, so it runs the same locally and in CI.
- `build-static.sh` builds the musl binary.
- The AppImage, `.deb` and `.rpm` all wrap that one binary.
- `release.sh` assembles `dist/` and `SHA256SUMS`.
- The workflow calls these scripts.

Package metadata lives in `crates/crumbs-cli/Cargo.toml` (cargo-deb, cargo-generate-rpm) and `packaging/`. Every artifact is checked by running it in a container.

**Tech Stack:**
- build: the M5 stack, plus the `x86_64-unknown-linux-musl` Rust target;
- packaging (build-time only): `appimagetool` (continuous build, static type-2 runtime), `cargo-deb` and `cargo-generate-rpm`;
- AUR files: `makepkg` and `namcap`;
- verification: podman.

**Spec:** `docs/superpowers/specs/2026-11-08-crumbs-design.md` §12 (packaging and release) and §13 (milestone 6).

## Decisions this plan makes (the spec leaves them open)

1. **License and URL.**
   - **License:** `GPL-3.0-or-later` (the user's choice). `LICENSE` is the SPDX text from `/usr/share/licenses/spdx/GPL-3.0-or-later.txt`. Every package records the SPDX id.
   - **Project URL:** `https://github.com/asafelobotomy/crumbs`, which goes into `repository` and `homepage` and the AUR `url`.
   - **Check:** all dependencies are MIT, Apache-2.0 or similar, which is compatible. A `cargo metadata` check in Task 1 lists any dependency license that is not one of `MIT`, `Apache-2.0`, `BSD-*`, `ISC`, `Unicode-3.0`, `Zlib` or `Unlicense`, or a combination of those.
2. **Release profile.**
   - `[profile.release]` sets `lto = "thin"`, `codegen-units = 1` and `strip = true`.
   - `panic` stays `unwind`: the MCP server turns a panic in a tool into an error and keeps serving.
   - The `mcp` feature stays on in every artifact.
3. **One binary inside every package.** The AppImage, the `.deb`, the `.rpm` and `crumbs-bin` all ship the same stripped `x86_64-unknown-linux-musl` binary, `dist/crumbs-<version>-x86_64-linux-musl`. Only the AUR `crumbs` package builds from source, against the system toolchain (glibc).
4. **AppImage.**
   - **AppDir layout:**
     - `AppRun`, a POSIX `sh` script: `exec "$APPDIR/usr/bin/crumbs" "$@"`;
     - `usr/bin/crumbs`;
     - `crumbs.desktop`: `Type=Application`, `Name=crumBS`, `Exec=crumbs`, `Icon=crumbs`, `Terminal=true`, `Categories=System;Utility;`;
     - `crumbs.svg`, plus a `.DirIcon` symlink to it;
     - `usr/share/metainfo/io.github.asafelobotomy.crumbs.metainfo.xml`.
   - **Build:** `appimagetool` with its default static runtime. The output is `dist/crumbs-<version>-x86_64.AppImage`.
   - **No sandboxing.** crumbs has to see the host.
5. **`.deb` and `.rpm`.**
   - Built with `cargo deb --no-build` and `cargo generate-rpm` from the prebuilt musl binary.
   - Both install `/usr/bin/crumbs`, `/usr/share/doc/crumbs/README.md` and `/usr/share/licenses/crumbs/LICENSE` (the `.deb` puts the licence at `/usr/share/doc/crumbs/copyright`, per Debian policy).
   - **No dependencies:** the binary is static.
   - Package name `crumbs`. Section `utils` / group `Applications/System`.
6. **AUR.**
   - `packaging/aur/crumbs/PKGBUILD` builds from the GitHub tag tarball:
     - `depends=()`, `makedepends=(cargo)`;
     - `build()`: `cargo build --release --locked --frozen` after `prepare()` runs `cargo fetch --locked`;
     - `check()`: `cargo test --release --locked`;
     - `package()` installs the binary, the licence and the README.
   - `packaging/aur/crumbs-bin/PKGBUILD` takes the release's musl binary, `provides=(crumbs)` and `conflicts=(crumbs)`.
   - Both get a `.SRCINFO` from `makepkg --printsrcinfo`. Their checksums are filled in by `release.sh` from the built artifacts; in the repo they are `SKIP`, marked `# filled in by scripts/release.sh`.
7. **Release.**
   - `scripts/release.sh <version>` checks three things:
     - the working tree is clean;
     - `<version>` equals the workspace `version`;
     - `CHANGELOG.md` has a `## <version>` section.
   - It then builds every artifact into `dist/` and writes `dist/SHA256SUMS`, plus the AUR files with real checksums under `dist/aur/`.
   - It never tags, pushes or uploads.
   - `.github/workflows/release.yml` runs on a `v*` tag. It runs `release.sh "${GITHUB_REF_NAME#v}"` in an `archlinux` container, then attaches `dist/*` to a GitHub release, with the notes taken from the changelog section.
   - aarch64 waits until x86_64 is stable (spec §12).
8. **Out of scope.**
   - Publishing to the AUR (needs the user's AUR account).
   - Pushing to GitHub or creating a release or tag. These are outward-facing, so they are the user's step; the README says how.
   - Signing artifacts (a follow-up).

## Global Constraints

- **Inherited constraints:** all M1–M5 global constraints still apply. See the M5 plan, "Global Constraints".
- **No new runtime dependencies.** Build-only tools are installed with `cargo install --locked` or `rustup target add`, never vendored into the repo. They are `cargo-deb`, `cargo-generate-rpm` and the musl target.
- **Never** create a git tag, push, create a GitHub release, or upload to the AUR. Those steps belong to the user.
- **Never** install a built package on the host. Install it only inside a disposable podman container.
- **On the host,** the built binaries may run only these read-only commands:
  - `--version` and `doctor --json`;
  - `apps list --json`;
  - `scan --json`;
  - `crumbs mcp` with read tools only.

  The same no-escalation and `--dry-run`-only rules apply as before.
- **`dist/`** is git-ignored.
- **Disk space:** `/tmp` is a small tmpfs, so put scratch and `CARGO_TARGET_DIR` under `~/.cache`.

## Review Focus

1. **A host without `libfuse2`, or a container without `/dev/fuse`.** The AppImage must not need `libfuse2`, and it must still run via `--appimage-extract-and-run`, or `APPIMAGE_EXTRACT_AND_RUN=1`, when FUSE is unavailable. Pinned by `appimage_runs_without_libfuse2` (Task 3).
2. **`crumbs mcp` started through the AppImage.** The AppImage runtime and `AppRun` must not write to stdout, so the MCP stream stays pure. Pinned by `appimage_mcp_stdout_is_protocol_only` (Task 3).
3. **An AppImage in a path with spaces or quotes** (`~/My Apps/crumbs.AppImage`). `AppRun` quotes `$APPDIR` and passes every argument through unchanged. Pinned by `appimage_in_path_with_spaces` (Task 3).
4. **The release musl binary keeps every release-build guard.** It refuses `CRUMBS_ROOT` and `CRUMBS_TEST_*` with exit 2, and it never honours the debug test hooks. Pinned by `static_binary_keeps_release_guards` (Task 2).
5. **Installing and then removing the `.deb` or `.rpm`.** It leaves nothing behind: `dpkg -r`/`dpkg -P` and `rpm -e` remove every file the package added. The installed `crumbs` lists itself as an app from its own package database (`dpkg:crumbs` / `rpm:crumbs`), so `crumbs uninstall crumbs` can remove it. Pinned by `deb_and_rpm_install_run_and_remove_cleanly` (Task 4).

---

### Task 1: Licence, metadata and release profile

**Files:**
- Create: `LICENSE` (copied from `/usr/share/licenses/spdx/GPL-3.0-or-later.txt`), `CHANGELOG.md` (`## 0.1.0` with a summary of M1–M5), `scripts/check-licenses.sh`.
- Modify: `Cargo.toml` (`[workspace.package]`: `license`, `repository`, `homepage`, `description`, `authors = ["asafelobotomy"]`; `[profile.release]`), `crates/crumbs-cli/Cargo.toml` and `crates/crumbs-core/Cargo.toml` (`license.workspace = true` etc., plus `readme`, `keywords`, `categories`), `.gitignore` (`/dist`).
- Test: `crates/crumbs-cli/tests/cli.rs`.

**Interfaces:**
- `scripts/check-licenses.sh` prints every dependency whose `license` field contains an id outside the allowed set in Decision 1, and exits 1 if there is any. It uses `cargo metadata --format-version 1` and `python3`.

- [ ] **Step 1: Write the failing test.** `version_flag_prints_name_and_version` asserts that `crumbs --version` prints exactly `crumbs 0.1.0\n` and exits 0. Check whether clap already prints this. If the test passes at once, keep it as a regression test and note that in the ledger.
- [ ] **Step 2: Implement** the metadata, `LICENSE`, `CHANGELOG.md`, the release profile and `check-licenses.sh`.
- [ ] **Step 3: Run** `scripts/check-licenses.sh`. Expected: no output, exit 0.

  Run `cargo test --workspace`. Expected: PASS.

  Run `cargo build --release -p crumbs-cli`. Expected: the build succeeds, the binary is stripped (`file` says `stripped`), and `crumbs --version` prints the version.
- [ ] **Step 4: Commit** `chore: GPL-3.0-or-later licence, package metadata, release profile`.

### Task 2: Static musl binary

**Files:**
- Create: `scripts/build-static.sh`.
- Test: `scripts/tests/static.sh`, a shell test run by `scripts/test-packaging.sh` (created here; later tasks add to it).

**Interfaces:**
- `scripts/build-static.sh` takes no arguments. It:
  - runs `rustup target add x86_64-unknown-linux-musl` if the target is missing;
  - runs `cargo build --release --locked --target x86_64-unknown-linux-musl -p crumbs-cli`;
  - copies the binary to `dist/crumbs-<version>-x86_64-linux-musl`;
  - prints that path.
  - It reads `<version>` from `cargo metadata`.
- `scripts/test-packaging.sh [name…]` runs `scripts/tests/<name>.sh` (all of them by default). It prints `PASS <name>` or `FAIL <name>: <reason>`, and exits non-zero on any failure.

- [ ] **Step 1: Write the failing tests** in `scripts/tests/static.sh`:
  - `static_binary_is_statically_linked`: `file` reports `statically linked` (or `static-pie linked`), and `ldd` prints `not a dynamic executable` or `statically linked`.
  - `static_binary_runs_in_every_distro`: `podman run --rm --network=none -v dist:/dist:ro <image> /dist/crumbs-… --version` prints `crumbs <version>` for archlinux, debian:trixie, ubuntu and fedora.
  - `static_binary_keeps_release_guards`: `CRUMBS_ROOT=/nonexistent dist/crumbs-… scan` exits 2 with `honoured only by debug builds`. `CRUMBS_TEST_PRIV_RUNNER=sh dist/crumbs-… doctor` also exits 2.
- [ ] **Step 2: Run** `scripts/test-packaging.sh static`. Expected: FAIL (no binary yet).
- [ ] **Step 3: Implement** `build-static.sh`.
- [ ] **Step 4: Run** `scripts/build-static.sh && scripts/test-packaging.sh static`. Expected: `PASS static`.
- [ ] **Step 5: Commit** `build: static musl binary`.

### Task 3: AppImage

**Files:**
- Create: `packaging/appimage/AppRun`, `packaging/appimage/crumbs.desktop`, `packaging/appimage/crumbs.svg`, `packaging/appimage/io.github.asafelobotomy.crumbs.metainfo.xml`, `scripts/build-appimage.sh` and `scripts/tests/appimage.sh`.

**Interfaces:**
- `crumbs.svg`: a simple 256×256 icon. Use a crumb or broom shape in two colours. It must have no external references and no fonts.
- The metainfo file is a `console-application` component with these fields:
  - `id` `io.github.asafelobotomy.crumbs`;
  - `metadata_license` `CC0-1.0`;
  - `project_license` `GPL-3.0-or-later`;
  - `name` `crumBS`;
  - `summary` `Find and remove app leftovers, caches and system bloat`;
  - `url type="homepage"` set to the project URL;
  - a `provides`/`binary` entry of `crumbs`;
  - a `releases` entry for `0.1.0`.
- `scripts/build-appimage.sh [static-binary]` builds the AppDir under `~/.cache/crumbs-appdir`, then runs `appimagetool` to write `dist/crumbs-<version>-x86_64.AppImage`. It prints the path.
  - It uses `appimagetool` from PATH, or the one named by `APPIMAGETOOL`.
  - It fails with a clear message if appimagetool is missing.

- [ ] **Step 1: Write the failing tests** in `scripts/tests/appimage.sh`:
  - `appimage_lists_expected_files`: `--appimage-extract` into a temp dir yields `AppRun` (executable), `usr/bin/crumbs`, `crumbs.desktop`, `crumbs.svg`, `.DirIcon` and the metainfo file.
  - `appimage_desktop_entry_is_valid`: `desktop-file-validate` passes, if it is installed; otherwise the required keys are checked with grep. `Terminal=true` is present.
  - `appimage_runs_without_libfuse2`:
    - In `debian:trixie`, which has no libfuse2 and no `/dev/fuse`, `podman run --rm --network=none` with the AppImage mounted runs `APPIMAGE_EXTRACT_AND_RUN=1 /dist/…AppImage --version`, and that prints the version.
    - Also run `/dist/…AppImage --appimage-extract-and-run --version`.
    - The runtime's own linkage is static: `readelf -d` on the AppImage shows no `NEEDED` entries.
  - `appimage_in_path_with_spaces`: copy the AppImage to `~/.cache/crumbs-test/My Apps/crumbs's app.AppImage`. Run it with `--version`, then with `apps show "a b"` (an exit 2 with "no app" is fine). The second argument must arrive unsplit, which means the error names `a b`.
  - `appimage_mcp_stdout_is_protocol_only`:
    1. Pipe `initialize`, `notifications/initialized` and `tools/list` into `APPIMAGE_EXTRACT_AND_RUN=1 …AppImage mcp`.
    2. Keep stdin open for about 10 s.
    3. Every stdout line must parse as JSON-RPC 2.0, and `tools/list` must list `apply`.
    4. Use the host's `python3` to drive and check it.
- [ ] **Step 2: Run** `scripts/test-packaging.sh appimage`. Expected: FAIL.
- [ ] **Step 3: Implement** the AppDir files and `build-appimage.sh`.
- [ ] **Step 4: Run** `scripts/build-appimage.sh && scripts/test-packaging.sh appimage`. Expected: `PASS appimage`.
- [ ] **Step 5: Commit** `build: AppImage with static runtime`.

### Task 4: `.deb` and `.rpm`

**Files:**
- Modify: `crates/crumbs-cli/Cargo.toml`. Add `[package.metadata.deb]`:
  - `name = "crumbs"`, `section = "utils"`, `priority = "optional"`, `depends = ""`;
  - `assets`: the musl binary to `usr/bin/crumbs` (mode 755), the README to `usr/share/doc/crumbs/README.md`, and LICENSE to `usr/share/doc/crumbs/copyright`;
  - `extended-description` from the README's first paragraph.

  Add `[package.metadata.generate-rpm]` with the same assets (LICENSE to `/usr/share/licenses/crumbs/LICENSE`) and `name = "crumbs"`.
- Create: `scripts/build-packages.sh` and `scripts/tests/packages.sh`.

**Interfaces:**
- `scripts/build-packages.sh [static-binary]`:
  - installs `cargo-deb` and `cargo-generate-rpm` with `cargo install --locked` into `~/.cache/crumbs-tools` if they are missing;
  - stages the static binary where the asset paths expect it, at `target/x86_64-unknown-linux-musl/release/crumbs`;
  - runs `cargo deb -p crumbs-cli --no-build --target x86_64-unknown-linux-musl --output dist/` and `cargo generate-rpm -p crates/crumbs-cli --target x86_64-unknown-linux-musl -o dist/`;
  - renames the outputs to `dist/crumbs_<version>_amd64.deb` and `dist/crumbs-<version>-1.x86_64.rpm`, and prints both paths.

- [ ] **Step 1: Write the failing tests** in `scripts/tests/packages.sh`:
  - `deb_metadata`: `dpkg-deb --info` shows `Package: crumbs`, `Version: <version>`, no `Depends:` line, and `Section: utils`. `dpkg-deb --contents` lists `./usr/bin/crumbs`.
  - `rpm_metadata`: `rpm -qip` shows `Name : crumbs`, `License : GPL-3.0-or-later`, and the URL. `rpm -qlp` lists `/usr/bin/crumbs`.
  - `deb_and_rpm_install_run_and_remove_cleanly`:
    - In `debian:trixie`, run `dpkg -i /dist/…deb`, then `crumbs --version`.
    - Then run `crumbs apps list --all --json` and check that it lists `dpkg:crumbs`.
    - Then run `dpkg -P crumbs` and check that `/usr/bin/crumbs` is gone.
    - The same in `fedora:latest` with `rpm -i`, `rpm:crumbs` and `rpm -e`.
    - All of this runs inside containers with `--network=none`.
- [ ] **Step 2: Run** `scripts/test-packaging.sh packages`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `scripts/build-packages.sh && scripts/test-packaging.sh packages`. Expected: `PASS packages`.
- [ ] **Step 5: Commit** `build: .deb and .rpm packages`.

### Task 5: AUR packages

**Files:**
- Create: `packaging/aur/crumbs/PKGBUILD`, `packaging/aur/crumbs-bin/PKGBUILD`, their `.SRCINFO`, and `scripts/tests/aur.sh`.

**Interfaces:**
- The fields follow Decision 6, with `pkgver=0.1.0`, `pkgrel=1`, `arch=('x86_64')`, `license=('GPL-3.0-or-later')` and `url` set to the project URL.
- **`crumbs`:**
  - `source=("$pkgname-$pkgver.tar.gz::$url/archive/refs/tags/v$pkgver.tar.gz")`;
  - `options=('!lto')`, because Rust LTO is set in the profile.
- **`crumbs-bin`:**
  - `source=("crumbs-$pkgver::$url/releases/download/v$pkgver/crumbs-$pkgver-x86_64-linux-musl" "LICENSE::…/v$pkgver/LICENSE")`.
  - The release also uploads LICENSE.
- **Checksums:** `sha256sums=('SKIP' …)` in the repo, each with the comment `# filled in by scripts/release.sh`.

- [ ] **Step 1: Write the failing tests** in `scripts/tests/aur.sh`. The source build runs in `archlinux:latest` as a non-root user created in the container (makepkg refuses root), with network access for `cargo fetch`.
  - `aur_srcinfo_up_to_date`: `makepkg --printsrcinfo` for each PKGBUILD equals the committed `.SRCINFO`.
  - `aur_namcap_clean`: if `namcap` is installed on the host or in the container, `namcap PKGBUILD` reports no errors. Warnings are allowed.
  - `aur_source_package_builds`:
    1. Create a tarball named as the PKGBUILD expects with `git archive --prefix=crumbs-<version>/ HEAD`.
    2. Place it in SRCDEST and run `makepkg --skipchecksums`.
    3. Install the result with `pacman -U`.
    4. Check that `crumbs --version` prints the version.
  - `aur_bin_package_builds`: the same with the static binary from `dist/`, placed under the expected source names.
- [ ] **Step 2: Run** `scripts/test-packaging.sh aur`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `scripts/test-packaging.sh aur`. Expected: `PASS aur`.
- [ ] **Step 5: Commit** `build: AUR packages (crumbs, crumbs-bin)`.

### Task 6: Release script and workflow

**Files:**
- Create: `scripts/release.sh`, `.github/workflows/release.yml` and `scripts/tests/release.sh`.

**Interfaces:**
- `scripts/release.sh <version> [--allow-dirty]` follows Decision 7. Its output in `dist/`:
  - `crumbs-<v>-x86_64-linux-musl`;
  - `crumbs-<v>-x86_64.AppImage`;
  - `crumbs_<v>_amd64.deb`;
  - `crumbs-<v>-1.x86_64.rpm`;
  - `LICENSE`;
  - `SHA256SUMS`, in `sha256sum` format over every file above;
  - `dist/aur/{crumbs,crumbs-bin}/PKGBUILD` and `.SRCINFO`, with real `sha256sums`. For `crumbs`, the sum is that of the `git archive` tarball, which matches GitHub's tag tarball only if the tag points at the same commit. The release notes say so, and the user re-checks after tagging.
  - `dist/RELEASE_NOTES.md`, the changelog section.
- `--allow-dirty` exists for tests only. Without it, a dirty tree exits 2 with `refusing: the working tree has uncommitted changes`.
- `release.yml`:
  - **Trigger:** `on: push: tags: ['v*']`. **Permissions:** `contents: write`.
  - **Job:** `release` runs on `ubuntu-latest` in the `archlinux:latest` container and installs `rust`, `rustup`, `appimagetool` (from the AppImage project's continuous release), `podman` (not needed), `python` and `github-cli`.
  - **Steps:** run `scripts/release.sh "${GITHUB_REF_NAME#v}"`, then `gh release create "$GITHUB_REF_NAME" dist/crumbs-* dist/LICENSE dist/SHA256SUMS --notes-file dist/RELEASE_NOTES.md`.
  - Upload `dist/aur/` as a workflow artifact, so the user can push it to the AUR.

- [ ] **Step 1: Write the failing tests** in `scripts/tests/release.sh`:
  - `release_refuses_dirty_tree`;
  - `release_refuses_version_mismatch`: `release.sh 9.9.9` exits 2 with `version 9.9.9 does not match Cargo.toml (0.1.0)`;
  - `release_builds_everything_with_sums`: `release.sh 0.1.0 --allow-dirty`, then `cd dist && sha256sum -c SHA256SUMS` passes. All files are present, and `dist/aur/*/PKGBUILD` contain no `SKIP`;
  - `release_workflow_is_valid_yaml`: `python3 -c "import yaml…"`, or a minimal parse if PyYAML is missing. The workflow references `scripts/release.sh` and `gh release create`.
- [ ] **Step 2: Run** `scripts/test-packaging.sh release`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `scripts/test-packaging.sh` for every packaging test. Expected: all PASS.
- [ ] **Step 5: Commit** `release: release script and tag workflow`.

### Task 7: Docs

**Files:**
- Modify:
  - `README.md`. Add an "Install" section covering:
    - the AppImage (download, `chmod +x`, run in a terminal; `--appimage-extract-and-run` without FUSE; the AppImage location is not an app crumbs manages);
    - `.deb`/`.rpm`;
    - `yay -S crumbs` / `crumbs-bin` once published;
    - the static binary;
    - building from source;
    - `cargo build --no-default-features` without MCP.

    Add a "Releasing" section covering `scripts/release.sh`, `scripts/test-packaging.sh`, the tag push, and publishing `dist/aur/` to the AUR. Add the licence line.
  - Spec §13: 6 done.
- Create: `docs/superpowers/notes/2026-11-09-m6-review-followups.md`, covering signing, aarch64, AUR publishing, and anything the ledger parks.

- [ ] **Step 1: Write the docs.**
- [ ] **Step 2: Check them.** Every command and file name in them must exist, checked with `grep` against `scripts/` and `packaging/`.
- [ ] **Step 3: Commit** `docs: install and release`.
