use crate::config::Settings;
use crate::scan::Game;
use serde::{Deserialize, Serialize};
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};
use walkdir::WalkDir;
use zip::write::SimpleFileOptions;

#[derive(Serialize, Deserialize, Clone)]
pub struct Version {
    pub file: String,
    pub created: String,
    pub pc: String,
    pub size: u64,
    pub files: usize,
    pub fingerprint: String,
    #[serde(default)]
    pub pinned: bool,
    #[serde(default)]
    pub note: String,
    #[serde(default)]
    pub uploaded: bool,
}

pub fn remote_name(v: &Version) -> String {
    if v.file.contains("__") {
        return v.file.clone();
    }
    let utc = chrono::DateTime::parse_from_rfc3339(&v.created)
        .map(|d| d.with_timezone(&chrono::Utc))
        .unwrap_or_else(|_| chrono::Utc::now());
    format!(
        "{}__{}.zip",
        utc.format("%Y%m%dT%H%M%SZ"),
        safe_name(&v.pc).replace(' ', "-")
    )
}

pub fn parse_remote_name(name: &str) -> Option<(chrono::DateTime<chrono::Utc>, String)> {
    let stem = name.strip_suffix(".zip")?;
    let (ts, pc) = stem.split_once("__")?;
    let t = chrono::NaiveDateTime::parse_from_str(ts, "%Y%m%dT%H%M%SZ")
        .ok()?
        .and_utc();
    Some((t, pc.to_string()))
}

pub fn mark_uploaded(settings: &Settings, game: &str, file: &str) -> Result<(), String> {
    let mut versions = list(settings, game);
    for v in versions.iter_mut().filter(|v| v.file == file) {
        v.uploaded = true;
    }
    save_index(settings, game, &versions)
}

const META: &str = "saveward.json";
const OLD_META: &str = "savevault.json";

fn read_meta<R: io::Read + io::Seek>(zip: &mut zip::ZipArchive<R>) -> Result<Meta, String> {
    let name = if zip.index_for_name(META).is_some() {
        META
    } else {
        OLD_META
    };
    let f = zip
        .by_name(name)
        .map_err(|_| "Backup is missing its info file.")?;
    serde_json::from_reader(f).map_err(|e| e.to_string())
}

pub fn import(settings: &Settings, game: &str, name: &str, data: &[u8]) -> Result<Version, String> {
    let mut zip = zip::ZipArchive::new(io::Cursor::new(data))
        .map_err(|e| format!("Downloaded backup is damaged: {e}"))?;
    let meta = read_meta(&mut zip)?;
    let dir = game_dir(settings, game);
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    fs::write(dir.join(name), data).map_err(|e| e.to_string())?;
    let version = Version {
        file: name.to_string(),
        created: meta.created,
        pc: meta.pc,
        size: data.len() as u64,
        files: zip.len().saturating_sub(1),
        fingerprint: String::new(),
        pinned: false,
        note: "From the cloud".into(),
        uploaded: true,
    };
    let mut versions = list(settings, game);
    versions.retain(|v| v.file != name);
    versions.push(version.clone());
    versions.sort_by(|a, b| b.created.cmp(&a.created));
    save_index(settings, game, &versions)?;
    Ok(version)
}

#[derive(Serialize, Deserialize)]
struct Meta {
    game: String,
    created: String,
    pc: String,
    roots: Vec<Root>,
}

#[derive(Serialize, Deserialize)]
struct Root {
    portable: String,
    is_file: bool,
}

#[derive(Serialize)]
pub struct BackupResult {
    pub created: bool,
    pub version: Option<Version>,
}

pub fn safe_name(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || " -_.()".contains(c) {
                c
            } else {
                '_'
            }
        })
        .collect();
    s.trim().trim_end_matches('.').to_string()
}

pub fn game_dir(settings: &Settings, game: &str) -> PathBuf {
    Path::new(&settings.backup_dir).join(safe_name(game))
}

pub fn list(settings: &Settings, game: &str) -> Vec<Version> {
    fs::read_to_string(game_dir(settings, game).join("index.json"))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_index(settings: &Settings, game: &str, versions: &[Version]) -> Result<(), String> {
    let dir = game_dir(settings, game);
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let json = serde_json::to_string_pretty(versions).map_err(|e| e.to_string())?;
    fs::write(dir.join("index.json"), json).map_err(|e| e.to_string())
}

fn pc_name() -> String {
    std::env::var("COMPUTERNAME").unwrap_or_else(|_| "PC".into())
}

fn home() -> String {
    dirs::home_dir()
        .map(|h| h.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn to_portable(path: &str, game: &Game) -> String {
    let lower = path.to_lowercase();
    if let Some(base) = &game.install_dir {
        if lower.starts_with(&base.to_lowercase()) {
            return format!("<base>{}", &path[base.len()..]);
        }
    }
    let h = home();
    if !h.is_empty() && lower.starts_with(&h.to_lowercase()) {
        return format!("<home>{}", &path[h.len()..]);
    }
    path.to_string()
}

fn from_portable(p: &str, game: &Game) -> Option<PathBuf> {
    if let Some(rest) = p.strip_prefix("<base>") {
        return Some(PathBuf::from(format!(
            "{}{}",
            game.install_dir.as_ref()?,
            rest
        )));
    }
    if let Some(rest) = p.strip_prefix("<home>") {
        return Some(PathBuf::from(format!("{}{}", home(), rest)));
    }
    Some(PathBuf::from(p))
}

type FileList = Vec<(usize, PathBuf, String)>;

fn collect(game: &Game) -> (Vec<PathBuf>, FileList) {
    let roots = crate::scan::concrete_roots(game);
    let mut out = Vec::new();
    for (i, root) in roots.iter().enumerate() {
        let root = root.as_path();
        if root.is_file() {
            let name = root.file_name().unwrap_or_default().to_string_lossy();
            out.push((i, root.to_path_buf(), format!("r{i}/{name}")));
        } else if root.is_dir() {
            for e in WalkDir::new(root)
                .into_iter()
                .flatten()
                .filter(|e| e.file_type().is_file())
            {
                let rel = e.path().strip_prefix(root).unwrap_or(e.path());
                let rel = rel.to_string_lossy().replace('\\', "/");
                out.push((i, e.path().to_path_buf(), format!("r{i}/{rel}")));
            }
        }
    }
    (roots, out)
}

fn fingerprint(files: &FileList) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    let mut eat = |bytes: &[u8]| {
        for b in bytes {
            h ^= *b as u64;
            h = h.wrapping_mul(0x100000001b3);
        }
    };
    for (_, path, name) in files {
        eat(name.as_bytes());
        if let Ok(m) = fs::metadata(path) {
            eat(&m.len().to_le_bytes());
            if let Ok(t) = m.modified().and_then(|t| {
                t.duration_since(std::time::UNIX_EPOCH)
                    .map_err(|e| io::Error::other(e))
            }) {
                eat(&t.as_nanos().to_le_bytes());
            }
        }
    }
    format!("{h:016x}")
}

#[cfg_attr(not(test), allow(dead_code))]
pub fn backup(
    settings: &Settings,
    game: &Game,
    note: &str,
    force: bool,
) -> Result<BackupResult, String> {
    backup_with(settings, game, note, force, true, &mut |_, _| {})
}

pub fn backup_progress(
    settings: &Settings,
    game: &Game,
    note: &str,
    progress: &mut dyn FnMut(u64, u64),
) -> Result<BackupResult, String> {
    backup_with(settings, game, note, false, true, progress)
}

fn backup_with(
    settings: &Settings,
    game: &Game,
    note: &str,
    force: bool,
    do_prune: bool,
    progress: &mut dyn FnMut(u64, u64),
) -> Result<BackupResult, String> {
    let (roots, files) = collect(game);
    if files.is_empty() {
        return Err("No save files found for this game.".into());
    }
    let fp = fingerprint(&files);
    let mut versions = list(settings, &game.name);
    if !force && versions.first().is_some_and(|v| v.fingerprint == fp) {
        return Ok(BackupResult {
            created: false,
            version: None,
        });
    }

    let now = chrono::Local::now();
    let file_name = format!("{}.zip", now.format("%Y-%m-%d_%H-%M-%S"));
    let dir = game_dir(settings, &game.name);
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let zip_path = dir.join(&file_name);
    let tmp_path = dir.join(format!("{file_name}.tmp"));

    let meta = Meta {
        game: game.name.clone(),
        created: now.to_rfc3339(),
        pc: pc_name(),
        roots: roots
            .iter()
            .map(|p| Root {
                portable: to_portable(&p.to_string_lossy(), game),
                is_file: p.is_file(),
            })
            .collect(),
    };

    let total: u64 = files
        .iter()
        .filter_map(|(_, p, _)| fs::metadata(p).ok())
        .map(|m| m.len())
        .sum();
    progress(0, total);
    let mut write = || -> Result<(), String> {
        let mut zip = zip::ZipWriter::new(File::create(&tmp_path).map_err(|e| e.to_string())?);
        let opts = SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated)
            .large_file(true);
        zip.start_file(META, opts).map_err(|e| e.to_string())?;
        zip.write_all(serde_json::to_string_pretty(&meta).unwrap().as_bytes())
            .map_err(|e| e.to_string())?;
        let mut done = 0u64;
        let mut buf = vec![0u8; 256 * 1024];
        for (_, path, name) in &files {
            let Ok(mut f) = File::open(path) else {
                continue;
            };
            zip.start_file(name.as_str(), opts)
                .map_err(|e| e.to_string())?;
            loop {
                let n = f.read(&mut buf).map_err(|e| e.to_string())?;
                if n == 0 {
                    break;
                }
                zip.write_all(&buf[..n]).map_err(|e| e.to_string())?;
                done += n as u64;
                progress(done, total);
            }
        }
        zip.finish().map_err(|e| e.to_string())?;
        Ok(())
    };
    if let Err(e) = write() {
        let _ = fs::remove_file(&tmp_path);
        return Err(e);
    }
    fs::rename(&tmp_path, &zip_path).map_err(|e| e.to_string())?;

    let version = Version {
        file: file_name,
        created: meta.created,
        pc: meta.pc,
        size: fs::metadata(&zip_path).map(|m| m.len()).unwrap_or(0),
        files: files.len(),
        fingerprint: fp,
        pinned: false,
        note: note.to_string(),
        uploaded: false,
    };
    versions.insert(0, version.clone());
    if do_prune {
        prune(settings, &game.name, &mut versions);
    }
    save_index(settings, &game.name, &versions)?;
    Ok(BackupResult {
        created: true,
        version: Some(version),
    })
}

fn prune(settings: &Settings, game: &str, versions: &mut Vec<Version>) {
    let keep = settings.versions_to_keep.max(1);
    let dir = game_dir(settings, game);
    let mut unpinned = 0;
    versions.retain(|v| {
        if v.pinned {
            return true;
        }
        unpinned += 1;
        if unpinned > keep {
            let _ = fs::remove_file(dir.join(&v.file));
            false
        } else {
            true
        }
    });
}

pub fn set_pinned(settings: &Settings, game: &str, file: &str, pinned: bool) -> Result<(), String> {
    let mut versions = list(settings, game);
    for v in versions.iter_mut().filter(|v| v.file == file) {
        v.pinned = pinned;
    }
    save_index(settings, game, &versions)
}

pub fn delete(settings: &Settings, game: &str, file: &str) -> Result<(), String> {
    let mut versions = list(settings, game);
    versions.retain(|v| v.file != file);
    let _ = fs::remove_file(game_dir(settings, game).join(file));
    save_index(settings, game, &versions)
}

pub fn restore(settings: &Settings, game: &Game, file: &str) -> Result<usize, String> {
    let zip_path = game_dir(settings, &game.name).join(file);
    let mut zip = zip::ZipArchive::new(File::open(&zip_path).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    let meta = read_meta(&mut zip)?;

    if !collect(game).1.is_empty() {
        let snap = backup_with(
            settings,
            game,
            "Auto: before restore",
            false,
            false,
            &mut |_, _| {},
        )?;
        if let Some(v) = snap.version {
            let _ = set_pinned(settings, &game.name, &v.file, true);
        }
    }

    let roots: Vec<Option<PathBuf>> = meta
        .roots
        .iter()
        .map(|r| from_portable(&r.portable, game))
        .collect();
    let mut restored = 0;
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i).map_err(|e| e.to_string())?;
        let name = entry.name().to_string();
        let Some((root_tag, rel)) = name.split_once('/') else {
            continue;
        };
        let Some(idx) = root_tag
            .strip_prefix('r')
            .and_then(|n| n.parse::<usize>().ok())
        else {
            continue;
        };
        let (Some(Some(root)), Some(info)) = (roots.get(idx), meta.roots.get(idx)) else {
            continue;
        };
        let rel_path = Path::new(rel);
        if rel_path
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
        {
            continue;
        }
        let target = if info.is_file {
            root.clone()
        } else {
            root.join(rel_path)
        };
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let mut out = File::create(&target).map_err(|e| format!("{}: {e}", target.display()))?;
        io::copy(&mut entry, &mut out).map_err(|e| e.to_string())?;
        restored += 1;
    }
    Ok(restored)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scan::SavePath;

    #[test]
    fn backup_restore_roundtrip() {
        let tmp = std::env::temp_dir().join(format!("saveward-test-{}", std::process::id()));
        let saves = tmp.join("saves");
        fs::create_dir_all(saves.join("slot1")).unwrap();
        fs::write(saves.join("slot1/data.sav"), "level 5").unwrap();
        fs::write(saves.join("settings.cfg"), "vol=3").unwrap();

        let mut settings = Settings::default();
        settings.backup_dir = tmp.join("backups").to_string_lossy().into_owned();
        settings.versions_to_keep = 2;
        let game = Game {
            name: "Test Game".into(),
            source: "custom".into(),
            install_dir: None,
            exe: None,
            steam_id: None,
            cover: None,
            match_dirs: vec![],
            match_exes: vec![],
            paths: vec![SavePath {
                path: saves.to_string_lossy().into_owned(),
                detected: false,
                enabled: true,
                exists: true,
            }],
        };

        let first = backup(&settings, &game, "t", false).unwrap();
        assert!(first.created);
        assert!(!backup(&settings, &game, "t", false).unwrap().created);

        fs::write(saves.join("slot1/data.sav"), "level 1 (corrupted)").unwrap();
        let n = restore(&settings, &game, &first.version.unwrap().file).unwrap();
        assert_eq!(n, 2);
        assert_eq!(
            fs::read_to_string(saves.join("slot1/data.sav")).unwrap(),
            "level 5"
        );

        let versions = list(&settings, &game.name);
        assert!(
            versions
                .iter()
                .any(|v| v.pinned && v.note.contains("before restore"))
        );
        fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn reads_backups_made_before_the_rename() {
        let mut buf = io::Cursor::new(Vec::new());
        {
            let mut zip = zip::ZipWriter::new(&mut buf);
            let opts = SimpleFileOptions::default();
            zip.start_file(OLD_META, opts).unwrap();
            zip.write_all(
                br#"{"game":"G","created":"2026-11-01T10:00:00+00:00","pc":"OLD-PC","roots":[]}"#,
            )
            .unwrap();
            zip.finish().unwrap();
        }
        let mut zip = zip::ZipArchive::new(io::Cursor::new(buf.into_inner())).unwrap();
        let meta = read_meta(&mut zip).unwrap();
        assert_eq!(meta.pc, "OLD-PC");
    }
}
