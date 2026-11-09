use std::path::PathBuf;
use std::str::FromStr;

use serde::Serialize;

#[derive(Serialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum SourceKind {
    Pacman,
    Dpkg,
    Flatpak,
    #[serde(rename = "appimage")]
    AppImage,
    Unmanaged,
}

impl SourceKind {
    pub fn as_str(self) -> &'static str {
        match self {
            SourceKind::Pacman => "pacman",
            SourceKind::Dpkg => "dpkg",
            SourceKind::Flatpak => "flatpak",
            SourceKind::AppImage => "appimage",
            SourceKind::Unmanaged => "unmanaged",
        }
    }
}

impl FromStr for SourceKind {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "pacman" => Ok(SourceKind::Pacman),
            "dpkg" => Ok(SourceKind::Dpkg),
            "flatpak" => Ok(SourceKind::Flatpak),
            "appimage" => Ok(SourceKind::AppImage),
            "unmanaged" => Ok(SourceKind::Unmanaged),
            other => Err(format!(
                "unknown source '{other}' (expected pacman, dpkg, flatpak, appimage or unmanaged)"
            )),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Installation {
    User,
    System,
    /// A named Flatpak installation other than the user and system ones.
    Custom(String),
}

impl Installation {
    /// `user`, `system` or `custom:<name>`; also the JSON form.
    pub fn label(&self) -> String {
        match self {
            Installation::User => "user".into(),
            Installation::System => "system".into(),
            Installation::Custom(name) => format!("custom:{name}"),
        }
    }
}

impl Serialize for Installation {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.label())
    }
}

/// An installed application. Paths are in-system (`env.unsys` applied).
#[derive(Serialize, Debug, Clone, PartialEq)]
pub struct App {
    pub id: String,
    pub name: String,
    pub display_name: String,
    pub version: Option<String>,
    pub description: Option<String>,
    pub source: SourceKind,
    pub installation: Installation,
    pub install_size: Option<u64>,
    pub location: Option<PathBuf>,
    pub desktop_ids: Vec<String>,
    pub rdns_id: Option<String>,
    pub exec_names: Vec<String>,
    pub wm_classes: Vec<String>,
    pub has_gui: bool,
    /// An unmanaged app's canonical program (symlinks followed), in-system.
    #[serde(skip)]
    pub program: Option<PathBuf>,
    /// The path the program was reached through, when it differs from
    /// `program` (a symlink such as `/usr/bin/foo`).
    #[serde(skip)]
    pub program_link: Option<PathBuf>,
}

impl App {
    /// Builds an app whose `id` is `<source>:<name>`; sources fill in the rest.
    pub fn new(source: SourceKind, name: &str) -> App {
        App {
            id: format!("{}:{name}", source.as_str()),
            name: name.to_string(),
            display_name: name.to_string(),
            version: None,
            description: None,
            source,
            installation: Installation::System,
            install_size: None,
            location: None,
            desktop_ids: Vec::new(),
            rdns_id: None,
            exec_names: Vec::new(),
            wm_classes: Vec::new(),
            has_gui: false,
            program: None,
            program_link: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_id_is_source_qualified() {
        assert_eq!(App::new(SourceKind::Pacman, "firefox").id, "pacman:firefox");
        assert_eq!(
            App::new(SourceKind::Flatpak, "org.mozilla.firefox").id,
            "flatpak:org.mozilla.firefox"
        );
        let app = App::new(SourceKind::AppImage, "GutCheck-0.19.0-x86_64.AppImage");
        assert_eq!(app.id, "appimage:GutCheck-0.19.0-x86_64.AppImage");
        assert_eq!(app.display_name, "GutCheck-0.19.0-x86_64.AppImage");
        assert_eq!(
            App::new(SourceKind::Unmanaged, "foo.desktop").id,
            "unmanaged:foo.desktop"
        );
    }

    #[test]
    fn source_kind_round_trips_from_str() {
        for kind in [
            SourceKind::Pacman,
            SourceKind::Flatpak,
            SourceKind::AppImage,
            SourceKind::Unmanaged,
        ] {
            assert_eq!(kind.as_str().parse::<SourceKind>(), Ok(kind));
        }
        assert!("rpm".parse::<SourceKind>().is_err());
    }

    #[test]
    fn app_serializes_source_names() {
        let json = |k| serde_json::to_value(App::new(k, "x")).unwrap()["source"].clone();
        assert_eq!(json(SourceKind::Pacman), "pacman");
        assert_eq!(json(SourceKind::Flatpak), "flatpak");
        assert_eq!(json(SourceKind::AppImage), "appimage");
        assert_eq!(json(SourceKind::Unmanaged), "unmanaged");
    }
}
