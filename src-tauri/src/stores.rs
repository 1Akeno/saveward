use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct Installed {
    pub store: &'static str,
    pub title: String,
    pub store_id: Option<u64>,
    pub install_dir: PathBuf,
    pub root: PathBuf,
}

pub fn detect_all() -> Vec<Installed> {
    let mut out = Vec::new();
    out.extend(steam());
    out.extend(epic());
    out.extend(gog());
    out
}

#[cfg(windows)]
fn reg_string(hive: winreg::HKEY, key: &str, value: &str) -> Option<String> {
    winreg::RegKey::predef(hive)
        .open_subkey(key)
        .ok()?
        .get_value::<String, _>(value)
        .ok()
}

pub fn steam_path() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        use winreg::enums::*;
        if let Some(p) = reg_string(HKEY_CURRENT_USER, r"Software\Valve\Steam", "SteamPath") {
            return Some(PathBuf::from(p));
        }
        if let Some(p) = reg_string(
            HKEY_LOCAL_MACHINE,
            r"SOFTWARE\WOW6432Node\Valve\Steam",
            "InstallPath",
        ) {
            return Some(PathBuf::from(p));
        }
    }
    let default = PathBuf::from(r"C:\Program Files (x86)\Steam");
    default.exists().then_some(default)
}

fn vdf_values<'a>(text: &'a str, key: &str) -> Vec<String> {
    let needle = format!("\"{key}\"");
    text.lines()
        .filter_map(|line| {
            let line = line.trim();
            let rest = line.strip_prefix(&needle)?.trim();
            let v = rest.strip_prefix('"')?.strip_suffix('"')?;
            Some(v.replace("\\\\", "\\"))
        })
        .collect()
}

fn steam() -> Vec<Installed> {
    let Some(steam) = steam_path() else {
        return vec![];
    };
    let mut libraries = vec![steam.clone()];
    if let Ok(text) = fs::read_to_string(steam.join("steamapps/libraryfolders.vdf")) {
        for p in vdf_values(&text, "path") {
            let p = PathBuf::from(p);
            if !libraries.iter().any(|l| same_path(l, &p)) {
                libraries.push(p);
            }
        }
    }

    let mut out = Vec::new();
    for lib in libraries {
        let Ok(entries) = fs::read_dir(lib.join("steamapps")) else {
            continue;
        };
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if !(name.starts_with("appmanifest_") && name.ends_with(".acf")) {
                continue;
            }
            let Ok(text) = fs::read_to_string(e.path()) else {
                continue;
            };
            let id = vdf_values(&text, "appid")
                .first()
                .and_then(|s| s.parse().ok());
            let title = vdf_values(&text, "name").into_iter().next();
            let dir = vdf_values(&text, "installdir").into_iter().next();
            if let (Some(title), Some(dir)) = (title, dir) {
                let install_dir = lib.join("steamapps/common").join(dir);
                if install_dir.exists() {
                    out.push(Installed {
                        store: "steam",
                        title,
                        store_id: id,
                        install_dir,
                        root: lib.clone(),
                    });
                }
            }
        }
    }
    out
}

fn epic() -> Vec<Installed> {
    let dir = PathBuf::from(r"C:\ProgramData\Epic\EpicGamesLauncher\Data\Manifests");
    let Ok(entries) = fs::read_dir(dir) else {
        return vec![];
    };
    entries
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "item"))
        .filter_map(|e| {
            let v: serde_json::Value =
                serde_json::from_str(&fs::read_to_string(e.path()).ok()?).ok()?;
            let install_dir = PathBuf::from(v["InstallLocation"].as_str()?);
            if !install_dir.exists() {
                return None;
            }
            Some(Installed {
                store: "epic",
                title: v["DisplayName"].as_str()?.to_string(),
                store_id: None,
                root: install_dir.parent()?.to_path_buf(),
                install_dir,
            })
        })
        .collect()
}

fn gog() -> Vec<Installed> {
    #[cfg(windows)]
    {
        use winreg::enums::*;
        let Ok(games) = winreg::RegKey::predef(HKEY_LOCAL_MACHINE)
            .open_subkey(r"SOFTWARE\WOW6432Node\GOG.com\Games")
        else {
            return vec![];
        };
        games
            .enum_keys()
            .flatten()
            .filter_map(|k| {
                let key = games.open_subkey(&k).ok()?;
                let path: String = key.get_value("path").ok()?;
                let title: String = key.get_value("gameName").ok()?;
                let install_dir = PathBuf::from(path);
                if !install_dir.exists() {
                    return None;
                }
                Some(Installed {
                    store: "gog",
                    title,
                    store_id: k.parse().ok(),
                    root: install_dir.parent()?.to_path_buf(),
                    install_dir,
                })
            })
            .collect()
    }
    #[cfg(not(windows))]
    vec![]
}

pub fn same_path(a: &Path, b: &Path) -> bool {
    norm(a) == norm(b)
}

pub fn norm(p: &Path) -> String {
    p.to_string_lossy()
        .replace('/', "\\")
        .trim_end_matches('\\')
        .to_lowercase()
}
