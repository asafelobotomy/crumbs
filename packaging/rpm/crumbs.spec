# Wraps the prebuilt static binary; built by scripts/build-packages.sh with
#   rpmbuild -bb --define "crumbs_version <v>" --define "crumbs_src <dir>"
# where <dir> holds crumbs, README.md and LICENSE.
%global debug_package %{nil}
%global __os_install_post %{nil}
# /usr/share/{doc,licenses}/crumbs, as on Fedora, whatever the build host.
%global _docdir_fmt %{NAME}

Name:           crumbs
Version:        %{crumbs_version}
Release:        1
Summary:        Find and remove app leftovers, caches and system bloat
License:        GPL-3.0-or-later
URL:            https://github.com/asafelobotomy/crumbs
ExclusiveArch:  x86_64
# The binary is static and the package runs no scriptlets.
AutoReqProv:    no

%description
crumBS lists the apps installed from your package manager, Flatpak, Snap and
AppImages, and finds what each one leaves behind: configuration, caches, data
and system files. It removes an app together with its leftovers after showing
you exactly what will go, and cleans caches, logs, trash and orphan packages.

%prep
cp %{crumbs_src}/README.md %{crumbs_src}/LICENSE .

%install
install -Dm755 %{crumbs_src}/crumbs %{buildroot}%{_bindir}/crumbs

%files
%{_bindir}/crumbs
%doc README.md
%license LICENSE
