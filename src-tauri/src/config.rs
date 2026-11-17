use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

const OLD_NAME: &str = "SaveVault";

pub fn app_dir() -> PathBuf {
    static DIR: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    DIR.get_or_init(|| {
        let base = dirs::data_dir().unwrap_or_else(|| PathBuf::from("."));
        let dir = base.join("Saveward");
        let old = base.join(OLD_NAME);
        if !dir.exists() && old.is_dir() {
            let _ = fs::rename(&old, &dir);
        }
        let _ = fs::create_dir_all(&dir);
        dir
    })
    .clone()
}

fn old_app_dir() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(OLD_NAME)
}

fn moved(path: &str) -> Option<String> {
    let old = old_app_dir().to_string_lossy().into_owned();
    path.strip_prefix(&old)
        .map(|rest| format!("{}{rest}", app_dir().display()))
}

#[derive(Serialize, Deserialize, Clone, Default)]
#[serde(default)]
pub struct GameOverride {
    pub extra_paths: Vec<String>,
    pub excluded_paths: Vec<String>,
    pub custom: bool,
    pub exe: Option<String>,
    pub ignored: bool,
    pub cover: Option<String>,
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(default)]
pub struct Settings {
    pub backup_dir: String,
    pub versions_to_keep: usize,
    pub auto_backup: bool,
    pub show_covers: bool,
    pub start_with_windows: bool,
    pub show_popup: bool,
    pub cloud: Option<String>,
    pub cloud_apps: CloudApps,
    pub games: BTreeMap<String, GameOverride>,
}

#[derive(Serialize, Deserialize, Clone, Default)]
#[serde(default)]
pub struct CloudApps {
    pub google_id: String,
    pub google_secret: String,
    pub onedrive_id: String,
    pub dropbox_key: String,
}

impl CloudApps {
    fn pick(user: &str, built_in: Option<&'static str>) -> String {
        if !user.trim().is_empty() {
            user.trim().to_string()
        } else {
            built_in.unwrap_or_default().to_string()
        }
    }
    pub fn google(&self) -> (String, String) {
        (
            Self::pick(&self.google_id, option_env!("SAVEWARD_GOOGLE_ID")),
            Self::pick(&self.google_secret, option_env!("SAVEWARD_GOOGLE_SECRET")),
        )
    }
    pub fn onedrive(&self) -> String {
        Self::pick(&self.onedrive_id, option_env!("SAVEWARD_ONEDRIVE_ID"))
    }
    pub fn dropbox(&self) -> String {
        Self::pick(&self.dropbox_key, option_env!("SAVEWARD_DROPBOX_KEY"))
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            backup_dir: app_dir().join("backups").to_string_lossy().into_owned(),
            versions_to_keep: 10,
            auto_backup: true,
            show_covers: true,
            start_with_windows: false,
            show_popup: true,
            cloud: None,
            cloud_apps: CloudApps::default(),
            games: BTreeMap::new(),
        }
    }
}

impl Settings {
    fn path() -> PathBuf {
        app_dir().join("settings.json")
    }

    pub fn load() -> Self {
        let mut s: Self = fs::read_to_string(Self::path())
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        let mut changed = false;
        if let Some(p) = moved(&s.backup_dir) {
            s.backup_dir = p;
            changed = true;
        }
        for o in s.games.values_mut() {
            if let Some(p) = o.cover.as_deref().and_then(moved) {
                o.cover = Some(p);
                changed = true;
            }
        }
        if changed {
            let _ = s.save();
        }
        s
    }

    pub fn save(&self) -> Result<(), String> {
        let json = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        fs::write(Self::path(), json).map_err(|e| e.to_string())
    }
}

pub fn covers_dir() -> PathBuf {
    let dir = app_dir().join("covers");
    let _ = fs::create_dir_all(&dir);
    dir
}

#[cfg(windows)]
pub fn set_autostart(enable: bool) -> Result<(), String> {
    use winreg::enums::*;
    let run = winreg::RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey_with_flags(
            r"Software\Microsoft\Windows\CurrentVersion\Run",
            KEY_SET_VALUE,
        )
        .map_err(|e| e.to_string())?;
    let _ = run.delete_value(OLD_NAME);
    if enable {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        run.set_value("Saveward", &format!("\"{}\" --background", exe.display()))
            .map_err(|e| e.to_string())
    } else {
        match run.delete_value("Saveward") {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.to_string()),
            _ => Ok(()),
        }
    }
}

#[cfg(not(windows))]
pub fn set_autostart(_enable: bool) -> Result<(), String> {
    Ok(())
}
