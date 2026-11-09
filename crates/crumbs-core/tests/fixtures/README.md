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

## apt/

Captured in `docker.io/library/debian:trixie` after `apt-get update` and
`apt-get install sl cowsay`, with `LC_ALL=C` (stdout and stderr together):

- `purge-sl.txt`: `apt-get -s purge sl` (exit 0).
- `purge-perl-base.txt`: `apt-get -s purge perl-base` (exit 0; removes an essential package).
- `purge-nosuch.txt`: `apt-get -s purge nosuchpkg` (exit 100, `E:` line).
- `autoremove.txt`: `apt-get -s autoremove --purge` after `apt-mark auto cowsay` (exit 0).

## dnf/

Captured in `registry.fedoraproject.org/fedora:latest` (dnf5 5.4.6) after
`dnf install sl cowsay`, with `LC_ALL=C`; `--assumeno` exits 1 after showing the table:

- `remove-sl.txt`: `dnf remove --assumeno sl`.
- `remove-perl-libs.txt`: `dnf remove --assumeno perl-libs`, first 40 lines (the table is cut
  before its summary).
- `remove-dnf5.txt`: `dnf remove --assumeno dnf5` (a protected package).
- `remove-nosuch.txt`: dnf5's text for an argument that is not installed.
- `dnf5-autoremove.txt`: hand-written in the captured dnf5 table format (the container had no
  unused dependencies to show).
- `dnf4-remove-sl.txt`: hand-written in dnf4's documented table format (no dnf4 image is used).
