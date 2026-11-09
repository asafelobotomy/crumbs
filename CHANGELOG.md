# Changelog

## 0.2.0

The first public release.

- **Apps:** lists apps from pacman, dpkg, rpm, Flatpak, Snap, AppImages and unmanaged installs, with their sizes and leftovers (`crumbs apps list`, `crumbs apps show`).
- **Uninstall:** removes an app with its leftovers, after a preview, through the system package manager. Root steps run as one batch with a single prompt (run0, pkexec or sudo).
- **Clean:** finds and removes caches, logs, trash, temp files and system bloat such as package caches and orphan packages (`crumbs scan`, `crumbs clean`, `crumbs orphans`).
- **Safety:** every deletion is checked against safety scopes, moved to the trash where possible, recorded in the history and can be restored (`crumbs restore`).
- **MCP:** `crumbs mcp` serves the read and preview tools over stdio; `apply` asks you to confirm each plan.
- **Distros:** Arch and Manjaro, Debian and Ubuntu, and Fedora.
- **Packages:** an AppImage that needs no libfuse2, `.deb` and `.rpm` packages, the AUR packages `crum-bs` and `crum-bs-bin`, and a static x86_64 binary.
