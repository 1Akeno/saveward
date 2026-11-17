use crate::backup::{self, Version};
use crate::cloud::{AppCreds, Cloud, Provider};
use crate::config::{Settings, app_dir};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

type Res<T> = Result<T, String>;

pub enum Up<'a> {
    Progress(&'a str, u64, u64),
    Done(&'a str),
    Failed(&'a str, &'a str),
}

#[derive(Serialize, Deserialize, Clone, PartialEq)]
pub struct Item {
    pub game: String,
    pub file: String,
}

#[derive(Serialize, Clone)]
pub struct CloudVersion {
    pub name: String,
    pub created: String,
    pub pc: String,
    pub size: u64,
    pub local: bool,
}

#[derive(Default)]
pub struct Sync {
    queue: Mutex<Vec<Item>>,
    wake: (Mutex<bool>, Condvar),
    client: Mutex<Option<Arc<Cloud>>>,
    pub last_error: Mutex<Option<String>>,
    pub last_sync: Mutex<Option<String>>,
    pub busy: Mutex<bool>,
    pub newer: Mutex<HashMap<String, CloudVersion>>,
    notified: Mutex<HashSet<String>>,
}

fn queue_path() -> std::path::PathBuf {
    app_dir().join("cloud_queue.json")
}

pub fn creds(settings: &Settings, p: Provider) -> AppCreds {
    match p {
        Provider::Google => {
            let (id, secret) = settings.cloud_apps.google();
            AppCreds { id, secret }
        }
        Provider::OneDrive => AppCreds {
            id: settings.cloud_apps.onedrive(),
            secret: String::new(),
        },
        Provider::Dropbox => AppCreds {
            id: settings.cloud_apps.dropbox(),
            secret: String::new(),
        },
    }
}

impl Sync {
    pub fn new() -> Self {
        let s = Self::default();
        if let Some(q) = fs::read_to_string(queue_path())
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
        {
            *s.queue.lock().unwrap() = q;
        }
        s
    }

    fn save_queue(&self) {
        if let Ok(j) = serde_json::to_string(&*self.queue.lock().unwrap()) {
            let _ = fs::write(queue_path(), j);
        }
    }

    pub fn pending(&self) -> usize {
        self.queue.lock().unwrap().len()
    }

    pub fn enqueue(&self, game: &str, file: &str) {
        let item = Item {
            game: game.into(),
            file: file.into(),
        };
        {
            let mut q = self.queue.lock().unwrap();
            if !q.contains(&item) {
                q.push(item);
            }
        }
        self.save_queue();
        self.poke();
    }

    pub fn clear_queue(&self) {
        self.queue.lock().unwrap().clear();
        self.save_queue();
    }

    pub fn poke(&self) {
        *self.wake.0.lock().unwrap() = true;
        self.wake.1.notify_one();
    }

    pub fn wait(&self, timeout: Duration) {
        let mut woken = self.wake.0.lock().unwrap();
        if !*woken {
            woken = self.wake.1.wait_timeout(woken, timeout).unwrap().0;
        }
        *woken = false;
    }

    pub fn reset_client(&self) {
        *self.client.lock().unwrap() = None;
        self.newer.lock().unwrap().clear();
        self.notified.lock().unwrap().clear();
        *self.last_error.lock().unwrap() = None;
    }

    pub fn client(&self, settings: &Settings) -> Res<Arc<Cloud>> {
        let p = Provider::parse(settings.cloud.as_deref().ok_or("No cloud connected.")?)?;
        let mut c = self.client.lock().unwrap();
        if let Some(c) = c.as_ref().filter(|c| c.provider == p) {
            return Ok(c.clone());
        }
        let cloud = Arc::new(Cloud::load(p, creds(settings, p))?);
        *c = Some(cloud.clone());
        Ok(cloud)
    }

    pub fn upload_queued(&self, settings: &Settings, on: &dyn Fn(Up)) -> Res<usize> {
        let items = self.queue.lock().unwrap().clone();
        if items.is_empty() {
            return Ok(0);
        }
        let cloud = self.client(settings)?;
        let mut done = 0;
        for item in items {
            let versions = backup::list(settings, &item.game);
            let path = backup::game_dir(settings, &item.game).join(&item.file);
            if let (Some(v), Ok(data)) = (
                versions.iter().find(|v| v.file == item.file),
                fs::read(&path),
            ) {
                let folder = backup::safe_name(&item.game);
                let game = item.game.as_str();
                on(Up::Progress(game, 0, data.len() as u64));
                if let Err(e) = cloud.upload(&folder, &backup::remote_name(v), &data, &|d, t| {
                    on(Up::Progress(game, d, t))
                }) {
                    on(Up::Failed(game, &e));
                    return Err(e);
                }
                backup::mark_uploaded(settings, &item.game, &item.file)?;
                let _ = prune_remote(&cloud, settings, &item.game);
                on(Up::Done(game));
                done += 1;
            }
            self.queue.lock().unwrap().retain(|q| q != &item);
            self.save_queue();
        }
        Ok(done)
    }

    pub fn check_newer(&self, settings: &Settings, games: &[String]) -> Res<Vec<String>> {
        let cloud = self.client(settings)?;
        let folders: HashSet<String> = cloud.list_games()?.into_iter().collect();
        let mut found = HashMap::new();
        for game in games {
            let folder = backup::safe_name(game);
            if !folders.contains(&folder) {
                continue;
            }
            let local = backup::list(settings, game);
            let list = cloud_versions(&cloud, settings, game, &local)?;
            let newest_local = local.iter().filter_map(|v| parse_time(&v.created)).max();
            if let Some(top) = list.first().filter(|c| !c.local) {
                let t = parse_time(&top.created);
                if newest_local.is_none()
                    || t.zip(newest_local)
                        .is_some_and(|(r, l)| r > l + chrono::Duration::seconds(60))
                {
                    found.insert(game.clone(), top.clone());
                }
            }
        }
        let mut notified = self.notified.lock().unwrap();
        let fresh: Vec<String> = found
            .keys()
            .filter(|g| notified.insert((*g).clone()))
            .cloned()
            .collect();
        *self.newer.lock().unwrap() = found;
        Ok(fresh)
    }
}

fn parse_time(s: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    chrono::DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|d| d.with_timezone(&chrono::Utc))
}

pub fn cloud_versions(
    cloud: &Cloud,
    _settings: &Settings,
    game: &str,
    local: &[Version],
) -> Res<Vec<CloudVersion>> {
    let here: HashSet<String> = local.iter().map(backup::remote_name).collect();
    let mut out: Vec<CloudVersion> = cloud
        .list(&backup::safe_name(game))?
        .into_iter()
        .filter_map(|f| {
            let (t, pc) = backup::parse_remote_name(&f.name)?;
            Some(CloudVersion {
                local: here.contains(&f.name),
                created: t.to_rfc3339(),
                pc,
                size: f.size,
                name: f.name,
            })
        })
        .collect();
    out.sort_by(|a, b| b.name.cmp(&a.name));
    Ok(out)
}

fn prune_remote(cloud: &Cloud, settings: &Settings, game: &str) -> Res<()> {
    let keep_n = settings.versions_to_keep.max(1);
    let pinned: HashSet<String> = backup::list(settings, game)
        .iter()
        .filter(|v| v.pinned)
        .map(backup::remote_name)
        .collect();
    let folder = backup::safe_name(game);
    let mut files: Vec<_> = cloud
        .list(&folder)?
        .into_iter()
        .filter(|f| backup::parse_remote_name(&f.name).is_some())
        .collect();
    files.sort_by(|a, b| b.name.cmp(&a.name));
    for f in files
        .iter()
        .skip(keep_n)
        .filter(|f| !pinned.contains(&f.name))
    {
        cloud.delete(&folder, f)?;
    }
    Ok(())
}

pub fn fetch(cloud: &Cloud, settings: &Settings, game: &str, name: &str) -> Res<String> {
    let local = backup::list(settings, game);
    if let Some(v) = local.iter().find(|v| backup::remote_name(v) == name) {
        return Ok(v.file.clone());
    }
    let folder = backup::safe_name(game);
    let f = cloud
        .list(&folder)?
        .into_iter()
        .find(|f| f.name == name)
        .ok_or("That backup is no longer in the cloud.")?;
    let data = cloud.download(&folder, &f)?;
    Ok(backup::import(settings, game, name, &data)?.file)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_names_roundtrip() {
        let v = Version {
            file: "2026-11-17_19-30-48.zip".into(),
            created: "2026-11-17T19:30:48+02:00".into(),
            pc: "DESKTOP 1".into(),
            size: 0,
            files: 0,
            fingerprint: String::new(),
            pinned: false,
            note: String::new(),
            uploaded: false,
        };
        let name = backup::remote_name(&v);
        assert_eq!(name, "20261117T173048Z__DESKTOP-1.zip");
        let (t, pc) = backup::parse_remote_name(&name).unwrap();
        assert_eq!(t.to_rfc3339(), "2026-11-17T17:30:48+00:00");
        assert_eq!(pc, "DESKTOP-1");
        let imported = Version {
            file: name.clone(),
            ..v
        };
        assert_eq!(backup::remote_name(&imported), name);
    }
}
