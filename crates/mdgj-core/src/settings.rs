//! Per-device settings stored as JSON outside the repo.

use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::{atomic, Error, Result};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    pub vim: bool,
    pub mode_desktop: Mode,
    pub mode_mobile: Mode,
    /// Minutes between background fetches; 0 turns them off.
    pub fetch_interval_min: u32,
    pub image_max_edge: u32,
    pub image_quality: u8,
    pub theme: Theme,
    pub author_name: Option<String>,
    pub author_email: Option<String>,
    pub remote: Option<RemoteConfig>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            vim: false,
            mode_desktop: Mode::Source,
            mode_mobile: Mode::Styled,
            fetch_interval_min: 5,
            image_max_edge: 1024,
            image_quality: 60,
            theme: Theme::System,
            author_name: None,
            author_email: None,
            remote: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Source,
    Styled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    System,
    Light,
    Dark,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteConfig {
    /// `"owner/repo"`.
    pub full_name: String,
    /// `https://github.com/owner/repo.git`, without credentials.
    pub url: String,
    pub branch: String,
    pub auth: AuthKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthKind {
    GithubApp,
    Pat,
}

impl Settings {
    /// Reads settings from `path`. A missing or unparseable file gives the defaults; a field
    /// that is missing or has a bad value (say, one written by a newer version) gets its
    /// default without discarding the other fields. Unknown fields are ignored.
    pub fn load(path: &Path) -> Settings {
        let json = fs::read(path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok());
        let Some(Value::Object(mut fields)) = json else {
            return Settings::default();
        };
        fields.retain(|key, value| {
            let alone = Map::from_iter([(key.clone(), value.clone())]);
            serde_json::from_value::<Settings>(Value::Object(alone)).is_ok()
        });
        serde_json::from_value(Value::Object(fields)).unwrap_or_default()
    }

    /// Atomically writes the settings to `path` as JSON, creating its directory if needed.
    pub fn save(&self, path: &Path) -> Result<()> {
        let save_error = |err| {
            Error::io(
                format_args!("could not save settings to {}", path.display()),
                err,
            )
        };
        let mut json = serde_json::to_vec_pretty(self).map_err(|err| save_error(err.into()))?;
        json.push(b'\n');
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir).map_err(save_error)?;
        }
        atomic::write(path, &json).map_err(save_error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn remote() -> RemoteConfig {
        RemoteConfig {
            full_name: "dcxSt/journal".into(),
            url: "https://github.com/dcxSt/journal.git".into(),
            branch: "main".into(),
            auth: AuthKind::GithubApp,
        }
    }

    fn load_json(json: &str) -> Settings {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        fs::write(&path, json).unwrap();
        Settings::load(&path)
    }

    #[test]
    fn defaults() {
        let s = Settings::default();
        assert!(!s.vim);
        assert_eq!(
            (s.mode_desktop, s.mode_mobile),
            (Mode::Source, Mode::Styled)
        );
        assert_eq!(s.fetch_interval_min, 5);
        assert_eq!((s.image_max_edge, s.image_quality), (1024, 60));
        assert_eq!(s.theme, Theme::System);
        assert_eq!(
            (s.author_name, s.author_email, s.remote),
            (None, None, None)
        );
    }

    #[test]
    fn serializes_camel_case_with_contract_enum_names() {
        let s = Settings {
            author_name: Some("Stephen".into()),
            remote: Some(remote()),
            ..Settings::default()
        };
        assert_eq!(
            serde_json::to_value(&s).unwrap(),
            json!({
                "vim": false,
                "modeDesktop": "source",
                "modeMobile": "styled",
                "fetchIntervalMin": 5,
                "imageMaxEdge": 1024,
                "imageQuality": 60,
                "theme": "system",
                "authorName": "Stephen",
                "authorEmail": null,
                "remote": {
                    "fullName": "dcxSt/journal",
                    "url": "https://github.com/dcxSt/journal.git",
                    "branch": "main",
                    "auth": "github_app"
                }
            })
        );
        assert_eq!(serde_json::to_value(AuthKind::Pat).unwrap(), json!("pat"));
        assert_eq!(serde_json::to_value(Theme::Dark).unwrap(), json!("dark"));
    }

    #[test]
    fn missing_file_gives_defaults() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            Settings::load(&dir.path().join("nope.json")),
            Settings::default()
        );
    }

    #[test]
    fn corrupt_file_gives_defaults() {
        for json in ["", "{", "not json", "[]", "null", "42", "\"vim\""] {
            assert_eq!(load_json(json), Settings::default(), "{json:?}");
        }
    }

    #[test]
    fn missing_fields_get_defaults() {
        let s = load_json(r#"{ "vim": true, "theme": "dark" }"#);
        assert_eq!(
            s,
            Settings {
                vim: true,
                theme: Theme::Dark,
                ..Settings::default()
            }
        );
    }

    #[test]
    fn unknown_fields_are_ignored() {
        let s = load_json(r#"{ "vim": true, "fontSize": 20, "future": { "nested": [1] } }"#);
        assert!(s.vim);
    }

    #[test]
    fn a_bad_field_does_not_reset_the_others() {
        let s = load_json(
            r#"{ "vim": true, "theme": "sepia", "imageQuality": 900, "modeMobile": null,
                 "remote": { "fullName": "dcxSt/journal", "url": "https://github.com/dcxSt/journal.git",
                             "branch": "main", "auth": "github_app" } }"#,
        );
        assert_eq!(
            s,
            Settings {
                vim: true,
                remote: Some(remote()),
                ..Settings::default()
            }
        );
    }

    #[test]
    fn incomplete_remote_is_dropped() {
        let s = load_json(r#"{ "remote": { "fullName": "dcxSt/journal" } }"#);
        assert_eq!(s.remote, None);
    }

    #[test]
    fn save_then_load_roundtrips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config/nested/settings.json");
        let s = Settings {
            vim: true,
            mode_desktop: Mode::Styled,
            fetch_interval_min: 0,
            image_max_edge: 2048,
            image_quality: 80,
            theme: Theme::Light,
            author_name: Some("Stephen Fay".into()),
            author_email: Some("1+dcxSt@users.noreply.github.com".into()),
            remote: Some(RemoteConfig {
                auth: AuthKind::Pat,
                ..remote()
            }),
            ..Settings::default()
        };
        s.save(&path).unwrap();
        assert_eq!(Settings::load(&path), s);

        let saved = Settings { vim: false, ..s };
        saved.save(&path).unwrap();
        assert_eq!(Settings::load(&path), saved);
        let names: Vec<_> = fs::read_dir(path.parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, ["settings.json"], "temp files left behind");
        assert!(fs::read_to_string(&path).unwrap().ends_with("}\n"));
    }

    #[test]
    fn save_reports_unwritable_paths() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("file");
        fs::write(&file, "").unwrap();
        let err = Settings::default()
            .save(&file.join("settings.json"))
            .unwrap_err();
        assert!(matches!(err, Error::Io(_)));
        assert!(
            err.to_string().starts_with("could not save settings to "),
            "{err}"
        );
    }
}
