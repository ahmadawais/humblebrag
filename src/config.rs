//! Optional config: named profiles that group npm users/packages and GitHub
//! accounts, so `hb <profile>` totals all of them.
//!
//! Looked up at $HUMBLEBRAG_CONFIG, else $XDG_CONFIG_HOME/humblebrag/config.json,
//! else ~/.config/humblebrag/config.json.
//!
//! {
//!   "profiles": {
//!     "ahmadawais": {
//!       "npm": ["~ahmadawais", "command-code"],
//!       "github": ["ahmadawais", "CommandCodeAI"],
//!       "contributions": true,
//!       "social": { "x": "MrAhmadAwais", "youtube": "@AhmadAwais", "quora": 1200 }
//!     }
//!   }
//! }
//!
//! `hb onboard` writes it. Records (contributions, daily history) live next to
//! the config file unless "record_dir" says otherwise. The default path can be
//! a symlink to a file kept elsewhere, such as a dotfiles repo.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::PathBuf;

#[derive(Deserialize, Serialize, Default)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    pub profiles: BTreeMap<String, Profile>,
    /// Where records go. Default: the config file's folder.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub record_dir: Option<String>,
}

#[derive(Deserialize, Serialize, Clone, Default)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    /// npm users (`~name`) and packages.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub npm: Vec<String>,
    /// GitHub users, orgs, or `owner/repo`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub github: Vec<String>,
    /// Also list merged PRs to repos outside `github`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub contributions: bool,
    /// Site -> handle/id (fetched) or number (kept by hand). See social.rs.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub social: BTreeMap<String, Value>,
}

fn home() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).unwrap_or_default())
}

/// `~/x` for paths under home, for display.
pub fn tilde(p: &std::path::Path) -> String {
    match p.strip_prefix(home()) {
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => p.display().to_string(),
    }
}

pub fn expand(p: &str) -> PathBuf {
    match p.strip_prefix("~/") {
        Some(rest) => home().join(rest),
        None => PathBuf::from(p),
    }
}

pub fn path() -> PathBuf {
    if let Some(p) = std::env::var_os("HUMBLEBRAG_CONFIG") {
        return expand(&p.to_string_lossy());
    }
    let base = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|| home().join(".config"));
    base.join("humblebrag").join("config.json")
}

/// The config, or the default when there is no file. A broken file is an error.
pub fn load() -> Config {
    let p = path();
    let Ok(text) = std::fs::read_to_string(&p) else {
        return Config::default();
    };
    serde_json::from_str(&text).unwrap_or_else(|e| {
        eprintln!("humblebrag: can't read {}: {e}", p.display());
        std::process::exit(2)
    })
}

impl Config {
    pub fn profile(&self, name: &str) -> Option<(String, Profile)> {
        self.profiles.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(k, v)| (k.clone(), v.clone()))
    }

    pub fn record_dir(&self) -> PathBuf {
        if let Some(d) = &self.record_dir {
            return expand(d);
        }
        // Next to the config file, following a symlink to its real home.
        if let Ok(real) = std::fs::canonicalize(path()) {
            if let Some(dir) = real.parent() {
                return dir.to_path_buf();
            }
        }
        let base = std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).unwrap_or_else(|| home().join(".local/share"));
        base.join("humblebrag")
    }
}
