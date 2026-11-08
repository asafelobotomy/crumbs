# crumBS

crumBS is a Linux command-line tool that lists the apps installed on your system across pacman, Flatpak, AppImage and unmanaged `.desktop` sources, so you can see what you have and where it came from. It is pre-alpha software.

**Status:** M1: read-only app listing. Nothing is deleted or modified.

## Build

```sh
cargo build --release
./target/release/crumbs --help
```

## Commands

- `crumbs apps list [--source <source>] [--all] [--sort name|size] [--json]` lists installed apps.
- `crumbs apps show <app> [--json]` shows details for one app.
- `crumbs doctor [--json]` reports which sources are available and any problems.

Exit codes: 0 ok, 1 error, 2 usage error or ambiguous app, 3 partial failure.

See [docs/superpowers/specs/2026-11-08-crumbs-design.md](docs/superpowers/specs/2026-11-08-crumbs-design.md) for the full design.
