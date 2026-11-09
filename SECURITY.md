# Security policy

crumbs runs as your user. Anything that needs administrator rights (removing packages, system leftovers in `/etc` and `/opt`) runs in one batch behind one polkit prompt, and items are moved to the trash rather than deleted.

## Reporting a vulnerability

Please use GitHub's private vulnerability reporting (Security tab, "Report a vulnerability") rather than a public issue.
Include the version (`crumbs --version`), your distro, what you did and what you expected.

## Supported versions

crumbs is pre-alpha: only the latest release receives fixes.

## Scope notes

- `crumbs mcp` asks for your confirmation before every change an agent requests.
- Every release ships a `SHA256SUMS` file for its binaries.
- Dependency advisories are checked in CI with `cargo deny`.
