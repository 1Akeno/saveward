use crate::config::app_dir;
use serde::Deserialize;
use std::collections::HashMap;
use std::fs;
use std::io::Read;

const URL: &str =
    "https://raw.githubusercontent.com/mtkennerly/ludusavi-manifest/master/data/manifest.yaml";
const MAX_AGE_SECS: u64 = 7 * 24 * 3600;

#[derive(Deserialize, Default)]
#[serde(default)]
pub struct Entry {
    pub files: HashMap<String, FileRule>,
    #[serde(rename = "installDir")]
    pub install_dir: HashMap<String, serde_yaml::Value>,
    pub steam: Option<StoreId>,
    pub gog: Option<StoreId>,
    pub launch: HashMap<String, Vec<FileRule>>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
pub struct FileRule {
    pub when: Vec<Condition>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
pub struct Condition {
    pub os: Option<String>,
    pub store: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
pub struct StoreId {
    pub id: Option<u64>,
}

impl FileRule {
    pub fn applies_to_windows(&self) -> bool {
        self.when.is_empty()
            || self
                .when
                .iter()
                .any(|c| c.os.as_deref().is_none_or(|os| os == "windows"))
    }
}

pub fn load(force_refresh: bool) -> Result<HashMap<String, Entry>, String> {
    let path = app_dir().join("manifest.yaml");
    let stale = fs::metadata(&path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.elapsed().ok())
        .is_none_or(|age| age.as_secs() > MAX_AGE_SECS);

    if force_refresh || stale {
        match download() {
            Ok(text) => {
                let _ = fs::write(&path, &text);
            }
            Err(e) if !path.exists() => {
                return Err(format!("Could not download game database: {e}"));
            }
            Err(_) => {}
        }
    }

    let text = fs::read_to_string(&path).map_err(|e| e.to_string())?;
    serde_yaml::from_str(&text).map_err(|e| format!("Bad game database: {e}"))
}

fn download() -> Result<String, String> {
    let resp = ureq::get(URL).call().map_err(|e| e.to_string())?;
    let mut text = String::new();
    resp.into_reader()
        .take(200 * 1024 * 1024)
        .read_to_string(&mut text)
        .map_err(|e| e.to_string())?;
    Ok(text)
}
