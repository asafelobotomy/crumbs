//! Per-app identifiers and name scoring.

use crate::model::{App, SourceKind};
use crate::sources::appimage::{normalize_name, split_appimage_name};

use super::Confidence;
use super::candidates::NameKey;

/// Identifiers that are too generic to say anything about an app.
const STOPLIST: [&str; 26] = [
    "app",
    "apps",
    "bin",
    "lib",
    "lib64",
    "share",
    "data",
    "cache",
    "config",
    "local",
    "icons",
    "applications",
    "autostart",
    "systemd",
    "fonts",
    "themes",
    "java",
    "python",
    "node",
    "npm",
    "git",
    "go",
    "user",
    "default",
    "common",
    "settings",
];

const MIN_LEN: usize = 3;
const MIN_LOW_LEN: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IdentKind {
    Name,
    DesktopId,
    Rdns,
    RdnsSegment,
    Exec,
    WmClass,
    DisplayName,
}

impl IdentKind {
    fn label(self) -> &'static str {
        match self {
            IdentKind::Name => "app name",
            IdentKind::DesktopId => "desktop ID",
            IdentKind::Rdns => "app ID",
            IdentKind::RdnsSegment => "last part of the app ID",
            IdentKind::Exec => "program name",
            IdentKind::WmClass => "window class",
            IdentKind::DisplayName => "display name",
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct Ident {
    pub(crate) raw: String,
    pub(crate) norm: String,
    pub(crate) kind: IdentKind,
}

/// An app's usable identifiers, after the stoplist and minimum length.
#[derive(Debug, Clone, Default)]
pub(crate) struct Idents {
    pub(crate) strong: Vec<Ident>,
    pub(crate) weak: Vec<Ident>,
}

fn usable(raw: &str, norm: &str) -> bool {
    let lower = raw.to_lowercase();
    raw.chars().count() >= MIN_LEN
        && norm.chars().count() >= MIN_LEN
        && !STOPLIST.contains(&lower.as_str())
        && !STOPLIST.contains(&norm)
}

fn push(list: &mut Vec<Ident>, raw: &str, kind: IdentKind) {
    let norm = normalize_name(raw);
    if usable(raw, &norm) && !list.iter().any(|i| i.raw == raw && i.kind == kind) {
        list.push(Ident {
            raw: raw.to_string(),
            norm,
            kind,
        });
    }
}

impl Idents {
    pub(crate) fn of(app: &App) -> Idents {
        let mut strong = Vec::new();
        let name = if app.source == SourceKind::AppImage {
            split_appimage_name(&app.name).0
        } else {
            app.name.clone()
        };
        push(&mut strong, &name, IdentKind::Name);
        for d in &app.desktop_ids {
            push(&mut strong, desktop_stem(d), IdentKind::DesktopId);
        }
        if let Some(rdns) = app.rdns_id.as_deref() {
            push(&mut strong, rdns, IdentKind::Rdns);
            if let Some((_, last)) = rdns.rsplit_once('.')
                && last.chars().count() >= MIN_LOW_LEN
            {
                push(&mut strong, last, IdentKind::RdnsSegment);
            }
        }
        let mut weak = Vec::new();
        for e in &app.exec_names {
            push(&mut weak, e, IdentKind::Exec);
        }
        for w in &app.wm_classes {
            push(&mut weak, w, IdentKind::WmClass);
        }
        push(&mut weak, &app.display_name, IdentKind::DisplayName);
        Idents { strong, weak }
    }

    pub(crate) fn has_exec(&self, basename: &str) -> bool {
        self.weak
            .iter()
            .any(|i| i.kind == IdentKind::Exec && i.raw == basename)
    }

    /// Desktop-ID stems and full rDNS IDs, which icons are named after.
    pub(crate) fn icon_names(&self) -> impl Iterator<Item = &str> {
        self.strong
            .iter()
            .filter(|i| matches!(i.kind, IdentKind::DesktopId | IdentKind::Rdns))
            .map(|i| i.raw.as_str())
    }
}

pub(crate) fn desktop_stem(id: &str) -> &str {
    id.strip_suffix(".desktop").unwrap_or(id)
}

/// Drops one final `-`/`_`/`.` segment that starts with a digit
/// (`Foo-2` → `Foo`, `gtk-3.0` → `gtk-3`), never leaving an empty name.
pub(crate) fn strip_version(name: &str) -> &str {
    match name.rfind(['-', '_', '.']) {
        Some(i) if i > 0 && name[i + 1..].starts_with(|c: char| c.is_ascii_digit()) => &name[..i],
        _ => name,
    }
}

/// Scores a directory or file name (dot already stripped) against an app.
pub(crate) fn score_name(key: &NameKey, ids: &Idents) -> Option<(Confidence, String)> {
    let name = key.raw.as_str();
    if let Some(i) = ids.strong.iter().find(|i| i.raw == name) {
        return Some((
            Confidence::High,
            format!("named after the {}", i.kind.label()),
        ));
    }
    if let Some(i) = ids.weak.iter().find(|i| i.raw == name) {
        return Some((
            Confidence::Medium,
            format!("named after the {} '{}'", i.kind.label(), i.raw),
        ));
    }
    let (norm, norm_unversioned) = (&key.norm, &key.unversioned);
    if let Some(i) = ids
        .strong
        .iter()
        .chain(&ids.weak)
        .find(|i| i.norm == *norm || i.norm == *norm_unversioned)
    {
        return Some((
            Confidence::Medium,
            format!("name resembles the {} '{}'", i.kind.label(), i.raw),
        ));
    }
    if let Some(i) = ids
        .strong
        .iter()
        .find(|i| i.norm.chars().count() >= MIN_LOW_LEN && norm.contains(&i.norm))
    {
        return Some((
            Confidence::Low,
            format!("name contains the {} '{}'", i.kind.label(), i.raw),
        ));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_version_table() {
        for (input, want) in [
            ("Foo-2", "Foo"),
            ("gimp-2.10", "gimp-2"),
            ("gtk-3.0", "gtk-3"),
            ("foo_1.4.2", "foo_1.4"),
            ("foo-bar", "foo-bar"),
            ("foo", "foo"),
            ("-2", "-2"),
            ("python3", "python3"),
        ] {
            assert_eq!(strip_version(input), want, "{input}");
        }
    }

    #[test]
    fn stoplist_and_short_ids_dropped() {
        let mut app = App::new(SourceKind::Pacman, "go");
        app.exec_names = vec!["Java".into(), "ab".into(), "vim".into()];
        app.display_name = "G O".into();
        let ids = Idents::of(&app);
        assert!(ids.strong.is_empty(), "{:?}", ids.strong);
        let weak: Vec<&str> = ids.weak.iter().map(|i| i.raw.as_str()).collect();
        assert_eq!(weak, ["vim"]);
    }

    #[test]
    fn rdns_last_segment_needs_four_chars() {
        let mut app = App::new(SourceKind::Flatpak, "org.example.Bar");
        app.rdns_id = Some("org.example.Bar".into());
        let raws: Vec<String> = Idents::of(&app).strong.into_iter().map(|i| i.raw).collect();
        assert!(!raws.contains(&"Bar".to_string()));
        app.rdns_id = Some("org.example.Barn".into());
        let raws: Vec<String> = Idents::of(&app).strong.into_iter().map(|i| i.raw).collect();
        assert!(raws.contains(&"Barn".to_string()));
    }
}
