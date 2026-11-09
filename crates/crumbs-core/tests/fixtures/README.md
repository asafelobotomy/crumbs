# Test fixtures

Captured from the podman images named below (offline, `--network=none`), then trimmed.

## dpkg/

- `status`: the `coreutils` and `libacl1` stanzas from `docker.io/library/debian:trixie`:
  `awk '/^Package: (coreutils|libacl1)$/{p=1} p{print} /^$/{p=0}' /var/lib/dpkg/status`.
  `firefox-esr` (a GUI package) and `oldpkg` (in `deinstall ok config-files`, the `rc` state)
  are hand-written in the same format.
- `info/coreutils.list` (first 12 lines) and `info/libacl1:amd64.list` are real; the
  multiarch file name is how dpkg names `Multi-Arch: same` packages.
  `info/firefox-esr.list` and `info/oldpkg.list` are hand-written.
