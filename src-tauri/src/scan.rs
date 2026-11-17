use crate::config::{Settings, app_dir};
use crate::manifest::{self, Entry};
use crate::stores::{self, Installed};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Serialize, Deserialize, Clone)]
pub struct SavePath {
    pub path: String,
    pub detected: bool,
    pub enabled: bool,
    pub exists: bool,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct Game {
    pub name: String,
    pub source: String,
    pub install_dir: Option<String>,
    pub exe: Option<String>,
    pub paths: Vec<SavePath>,
    pub steam_id: Option<u64>,
    pub cover: Option<String>,
    pub match_dirs: Vec<String>,
    pub match_exes: Vec<String>,
}

const CACHE_VERSION: u32 = 3;

const GENERIC_EXES: &[&str] = &[
    "game.exe",
    "launcher.exe",
    "start.exe",
    "play.exe",
    "run.exe",
    "setup.exe",
    "main.exe",
    "app.exe",
    "client.exe",
    "java.exe",
    "javaw.exe",
    "python.exe",
    "pythonw.exe",
    "dosbox.exe",
    "scummvm.exe",
    "nw.exe",
    "electron.exe",
    "unitycrashhandler64.exe",
    "unitycrashhandler32.exe",
    "crashreporter.exe",
    "uninstall.exe",
    "unins000.exe",
    "win64.exe",
    "shipping.exe",
    "bin.exe",
];
const GENERIC_DIRS: &[&str] = &[
    "bin",
    "game",
    "games",
    "steam",
    "common",
    "steamapps",
    "content",
    "binaries",
    "win64",
    "win32",
    "program files",
    "program files (x86)",
    "epic games",
    "gog games",
    "xboxgames",
    "data",
    "app",
    "apps",
];

const EXTRA_EXES: &[(&str, &str)] = &[
    ("Fortnite", "fortniteclient-win64-shipping.exe"),
    ("Valorant", "valorant-win64-shipping.exe"),
    ("Roblox", "robloxplayerbeta.exe"),
    ("Microsoft Solitaire Collection", "solitaire.exe"),
];

fn hints(entry: &Entry) -> (Vec<String>, Vec<String>) {
    let dirs = entry
        .install_dir
        .keys()
        .map(|d| d.to_lowercase())
        .filter(|d| d.len() >= 4 && !GENERIC_DIRS.contains(&d.as_str()))
        .collect();
    let mut exes: Vec<String> = entry
        .launch
        .iter()
        .filter(|(_, rules)| rules.is_empty() || rules.iter().any(|r| r.applies_to_windows()))
        .filter_map(|(path, _)| path.rsplit(['/', '\\']).next().map(str::to_lowercase))
        .filter(|e| {
            e.ends_with(".exe")
                && e.len() > 7
                && !GENERIC_EXES.contains(&e.as_str())
                && !e.contains('*')
        })
        .filter(|e| {
            ![
                "anticheat",
                "launcher",
                "crash",
                "redist",
                "setup",
                "unins",
                "vc_",
            ]
            .iter()
            .any(|w| e.contains(w))
        })
        .collect();
    exes.sort();
    exes.dedup();
    (dirs, exes)
}

#[derive(Serialize, Deserialize, Clone, Default)]
pub struct Detected {
    #[serde(default)]
    pub version: u32,
    pub games: BTreeMap<String, DetectedGame>,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct DetectedGame {
    pub source: String,
    pub install_dir: Option<String>,
    pub paths: Vec<String>,
    #[serde(default)]
    pub steam_id: Option<u64>,
    #[serde(default)]
    pub match_dirs: Vec<String>,
    #[serde(default)]
    pub match_exes: Vec<String>,
}

fn cache_path() -> PathBuf {
    app_dir().join("scan.json")
}

pub fn load_cached() -> Detected {
    fs::read_to_string(cache_path())
        .ok()
        .and_then(|s| serde_json::from_str::<Detected>(&s).ok())
        .filter(|d| d.version == CACHE_VERSION)
        .unwrap_or_default()
}

pub fn detect(refresh_manifest: bool) -> Result<Detected, String> {
    let manifest = manifest::load(refresh_manifest)?;
    let installed = stores::detect_all();
    let vars = Vars::new();

    let mut by_steam = HashMap::new();
    let mut by_gog = HashMap::new();
    let mut by_dir = HashMap::new();
    let mut by_title = HashMap::new();
    for (name, e) in &manifest {
        if let Some(id) = e.steam.as_ref().and_then(|s| s.id) {
            by_steam.insert(id, name.as_str());
        }
        if let Some(id) = e.gog.as_ref().and_then(|s| s.id) {
            by_gog.insert(id, name.as_str());
        }
        for d in e.install_dir.keys() {
            by_dir.insert(d.to_lowercase(), name.as_str());
        }
        by_title.insert(name.to_lowercase(), name.as_str());
    }

    let mut installs: HashMap<&str, &Installed> = HashMap::new();
    let mut unmatched = Vec::new();
    const NOT_GAMES: [u64; 3] = [228980, 1070560, 1391110];
    for inst in &installed {
        if inst.store == "steam" && inst.store_id.is_some_and(|id| NOT_GAMES.contains(&id)) {
            continue;
        }
        let folder = inst
            .install_dir
            .file_name()
            .map(|f| f.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        let hit = match inst.store {
            "steam" => inst.store_id.and_then(|id| by_steam.get(&id)),
            "gog" => inst.store_id.and_then(|id| by_gog.get(&id)),
            _ => None,
        }
        .or_else(|| by_dir.get(&folder))
        .or_else(|| by_title.get(&inst.title.to_lowercase()));
        match hit {
            Some(name) => {
                installs.insert(name, inst);
            }
            None => unmatched.push(inst),
        }
    }

    let mut out = Detected::default();
    for (name, entry) in &manifest {
        let inst = installs.get(name.as_str()).copied();
        let paths = resolve_entry(entry, inst, &vars);
        if paths.is_empty() && inst.is_none() {
            continue;
        }
        let (match_dirs, mut match_exes) = if inst.is_none() {
            hints(entry)
        } else {
            (vec![], vec![])
        };
        if inst.is_none() {
            match_exes.extend(
                EXTRA_EXES
                    .iter()
                    .filter(|(g, _)| g == name)
                    .map(|(_, e)| e.to_string()),
            );
        }
        out.games.insert(
            name.clone(),
            DetectedGame {
                source: inst.map_or("found", |i| i.store).to_string(),
                install_dir: inst.map(|i| i.install_dir.to_string_lossy().into_owned()),
                paths,
                steam_id: entry.steam.as_ref().and_then(|s| s.id),
                match_dirs,
                match_exes,
            },
        );
    }
    for inst in unmatched {
        out.games.entry(inst.title.clone()).or_insert(DetectedGame {
            source: inst.store.to_string(),
            install_dir: Some(inst.install_dir.to_string_lossy().into_owned()),
            paths: vec![],
            steam_id: if inst.store == "steam" {
                inst.store_id
            } else {
                None
            },
            match_dirs: vec![],
            match_exes: vec![],
        });
    }

    out.version = CACHE_VERSION;
    if let Ok(json) = serde_json::to_string(&out) {
        let _ = fs::write(cache_path(), json);
    }
    Ok(out)
}

pub fn merge(detected: &Detected, settings: &Settings) -> Vec<Game> {
    let mut games: Vec<Game> = Vec::new();
    for (name, d) in &detected.games {
        let ov = settings.games.get(name);
        if ov.is_some_and(|o| o.ignored) {
            continue;
        }
        let mut paths: Vec<SavePath> = d
            .paths
            .iter()
            .map(|p| SavePath {
                path: p.clone(),
                detected: true,
                enabled: !ov.is_some_and(|o| o.excluded_paths.contains(p)),
                exists: true,
            })
            .collect();
        if let Some(o) = ov {
            add_extra(&mut paths, &o.extra_paths);
        }
        games.push(Game {
            name: name.clone(),
            source: d.source.clone(),
            install_dir: d.install_dir.clone(),
            exe: ov.and_then(|o| o.exe.clone()),
            paths,
            steam_id: d.steam_id,
            cover: ov.and_then(|o| o.cover.clone()),
            match_dirs: d.match_dirs.clone(),
            match_exes: d.match_exes.clone(),
        });
    }
    for (name, o) in &settings.games {
        if o.custom && !o.ignored && !detected.games.contains_key(name) {
            let mut paths = vec![];
            add_extra(&mut paths, &o.extra_paths);
            games.push(Game {
                name: name.clone(),
                source: "custom".into(),
                install_dir: o
                    .exe
                    .as_ref()
                    .and_then(|e| Path::new(e).parent())
                    .map(|p| p.to_string_lossy().into_owned()),
                exe: o.exe.clone(),
                paths,
                steam_id: None,
                cover: o.cover.clone(),
                match_dirs: vec![],
                match_exes: vec![],
            });
        }
    }
    games.sort_by_key(|g| g.name.to_lowercase());
    games
}

fn add_extra(paths: &mut Vec<SavePath>, extra: &[String]) {
    for p in extra {
        if !paths.iter().any(|x| &x.path == p) {
            paths.push(SavePath {
                path: p.clone(),
                detected: false,
                enabled: true,
                exists: Path::new(p).exists(),
            });
        }
    }
}

pub fn concrete_roots(game: &Game) -> Vec<PathBuf> {
    let mut out: Vec<String> = Vec::new();
    for sp in game.paths.iter().filter(|p| p.enabled) {
        let hits = if sp.detected && has_wildcard(&sp.path) {
            glob_existing(&sp.path)
        } else {
            let p = PathBuf::from(&sp.path);
            if p.exists() { vec![p] } else { vec![] }
        };
        for h in hits {
            let s = h.to_string_lossy().replace('/', "\\");
            if !out.iter().any(|o| o.eq_ignore_ascii_case(&s)) {
                out.push(s);
            }
        }
    }
    drop_nested(out).into_iter().map(PathBuf::from).collect()
}

pub struct Vars {
    map: Vec<(&'static str, String)>,
    steam_root: Option<PathBuf>,
}

impl Vars {
    pub fn new() -> Self {
        let s = |p: Option<PathBuf>| {
            p.map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_default()
        };
        let env = |k: &str| std::env::var(k).unwrap_or_default();
        let home = dirs::home_dir();
        Self {
            map: vec![
                (
                    "<winLocalAppDataLow>",
                    s(home.as_ref().map(|h| h.join("AppData/LocalLow"))),
                ),
                ("<winLocalAppData>", s(dirs::data_local_dir())),
                ("<winAppData>", s(dirs::data_dir())),
                ("<winDocuments>", s(dirs::document_dir())),
                ("<winPublic>", env("PUBLIC")),
                ("<winProgramData>", env("PROGRAMDATA")),
                ("<winDir>", env("WINDIR")),
                ("<osUserName>", env("USERNAME")),
                ("<home>", s(home)),
            ],
            steam_root: stores::steam_path(),
        }
    }
}

fn resolve_entry(entry: &Entry, inst: Option<&Installed>, vars: &Vars) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    for (raw, rule) in &entry.files {
        if !rule.applies_to_windows() || raw.contains("<xdg") {
            continue;
        }
        let Some(pattern) = expand(raw, inst, vars) else {
            continue;
        };
        let pattern = pattern.replace('/', "\\");
        if !glob_existing(&pattern).is_empty()
            && !found.iter().any(|f| f.eq_ignore_ascii_case(&pattern))
        {
            found.push(pattern);
        }
    }
    drop_nested(found)
}

pub fn drop_nested(paths: Vec<String>) -> Vec<String> {
    let lower: Vec<String> = paths.iter().map(|p| p.to_lowercase()).collect();
    paths
        .iter()
        .enumerate()
        .filter(|(i, _)| {
            !lower.iter().enumerate().any(|(j, o)| {
                *i != j && !o.contains(['*', '?', '[']) && lower[*i].starts_with(&format!("{o}\\"))
            })
        })
        .map(|(_, p)| p.clone())
        .collect()
}

pub fn has_wildcard(p: &str) -> bool {
    p.contains(['*', '?', '['])
}

fn expand(raw: &str, inst: Option<&Installed>, vars: &Vars) -> Option<String> {
    let esc = |s: &str| glob::Pattern::escape(s);
    let mut out = raw.to_string();
    if out.contains("<base>") || out.contains("<game>") || out.contains("<root>") {
        let inst = inst?;
        let game = inst.install_dir.file_name()?.to_string_lossy().into_owned();
        let root = if inst.store == "steam" && out.contains("<root>/userdata") {
            vars.steam_root.clone()?
        } else {
            inst.root.clone()
        };
        out = out
            .replace("<base>", &esc(&inst.install_dir.to_string_lossy()))
            .replace("<root>", &esc(&root.to_string_lossy()))
            .replace("<game>", &esc(&game));
    }
    out = out.replace("<storeUserId>", "*");
    for (k, v) in &vars.map {
        if out.contains(k) {
            if v.is_empty() {
                return None;
            }
            out = out.replace(k, &esc(v));
        }
    }
    (!out.contains('<')).then_some(out)
}

pub fn glob_existing(pattern: &str) -> Vec<PathBuf> {
    let has_wild = pattern.contains(['*', '?', '[']);
    if !has_wild {
        let p = PathBuf::from(pattern);
        return if p.exists() { vec![p] } else { vec![] };
    }
    let prefix: String = pattern
        .split(['/', '\\'])
        .take_while(|c| !c.contains(['*', '?', '[']))
        .collect::<Vec<_>>()
        .join("\\");
    if !Path::new(&prefix).exists() {
        return vec![];
    }
    let opts = glob::MatchOptions {
        case_sensitive: false,
        require_literal_separator: true,
        require_literal_leading_dot: false,
    };
    glob::glob_with(pattern, opts)
        .map(|it| it.flatten().take(500).collect())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    #[test]
    #[ignore]
    fn real_scan() {
        let t = std::time::Instant::now();
        let d = super::detect(false).unwrap();
        println!("scan took {:?}, {} games", t.elapsed(), d.games.len());
        for (name, g) in d.games.iter().take(400) {
            println!("[{}] {} -> {:?}", g.source, name, g.paths);
        }
    }
}
