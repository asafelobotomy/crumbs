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

## rpm/

- `qa`: `rpm -q --qf '@@\t%{NAME}\t%{EPOCHNUM}:%{VERSION}-%{RELEASE}.%{ARCH}\t%{SIZE}\n[%{FILENAMES}\n]' coreutils shadow-utils gpg-pubkey` in
  `registry.fedoraproject.org/fedora:latest` (Fedora 44), trimmed to 6 files per package.
  The `firefox` block is hand-written in the same format. `gpg-pubkey` is a key, not a
  package (arch `(none)`), and is skipped.

## snap/

- `list`: hand-written in the column layout of `snap list --unicode=never`
  (`Name Version Rev Tracking Publisher Notes`; verified publishers end in `**`).
  snapd needs systemd, so it does not run in the test containers.
