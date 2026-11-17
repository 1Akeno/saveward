use crate::scan::Game;
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

pub const SETTLE: Duration = Duration::from_secs(8);

pub struct Target {
    pub name: String,
    exe: Option<String>,
    dir: Option<String>,
    hint_dirs: Vec<String>,
    hint_exes: Vec<String>,
}

fn lower(p: &str) -> String {
    p.replace('/', "\\").to_lowercase()
}

pub fn targets(games: &[Game]) -> Vec<Target> {
    games
        .iter()
        .filter(|g| g.paths.iter().any(|p| p.enabled))
        .filter_map(|g| {
            let exe = g.exe.as_deref().map(lower);
            let dir = g.install_dir.as_deref().map(|d| {
                let d = lower(d);
                if d.ends_with('\\') {
                    d
                } else {
                    format!("{d}\\")
                }
            });
            let hint_dirs: Vec<String> = g
                .match_dirs
                .iter()
                .map(|d| format!("\\{}\\", d.to_lowercase()))
                .collect();
            let hint_exes: Vec<String> = g
                .match_exes
                .iter()
                .map(|e| format!("\\{}", e.to_lowercase()))
                .collect();
            (exe.is_some() || dir.is_some() || !hint_dirs.is_empty() || !hint_exes.is_empty()).then(
                || Target {
                    name: g.name.clone(),
                    exe,
                    dir,
                    hint_dirs,
                    hint_exes,
                },
            )
        })
        .collect()
}

impl Target {
    fn matches(&self, proc_path: &str) -> bool {
        if let Some(exe) = &self.exe {
            return proc_path == exe;
        }
        if let Some(d) = &self.dir {
            return proc_path.starts_with(d);
        }
        self.hint_dirs
            .iter()
            .any(|d| proc_path.contains(d.as_str()))
            || self
                .hint_exes
                .iter()
                .any(|e| proc_path.ends_with(e.as_str()))
    }
}

#[derive(Default)]
pub struct Watcher {
    running: HashSet<String>,
    pending: HashMap<String, Instant>,
    paths: HashMap<u32, String>,
}

impl Watcher {
    pub fn step(
        &mut self,
        targets: &[Target],
        procs: &[String],
        now: Instant,
    ) -> (Vec<String>, Vec<String>) {
        let mut running = HashSet::new();
        for t in targets {
            if procs.iter().any(|p| t.matches(p)) {
                running.insert(t.name.clone());
            }
        }
        let started: Vec<String> = running.difference(&self.running).cloned().collect();
        for name in self.running.difference(&running) {
            self.pending.insert(name.clone(), now + SETTLE);
        }
        for name in &running {
            self.pending.remove(name);
        }
        let due: Vec<String> = self
            .pending
            .iter()
            .filter(|(_, at)| **at <= now)
            .map(|(n, _)| n.clone())
            .collect();
        for n in &due {
            self.pending.remove(n);
        }
        self.running = running;
        (started, due)
    }

    pub fn running(&self) -> &HashSet<String> {
        &self.running
    }

    #[cfg(windows)]
    pub fn processes(&mut self) -> Vec<String> {
        use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
        use windows_sys::Win32::System::Diagnostics::ToolHelp::*;
        use windows_sys::Win32::System::Threading::*;

        let mut pids = Vec::with_capacity(400);
        unsafe {
            let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
            if snap == INVALID_HANDLE_VALUE {
                return vec![];
            }
            let mut e: PROCESSENTRY32W = std::mem::zeroed();
            e.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
            if Process32FirstW(snap, &mut e) != 0 {
                loop {
                    pids.push(e.th32ProcessID);
                    if Process32NextW(snap, &mut e) == 0 {
                        break;
                    }
                }
            }
            CloseHandle(snap);
        }

        let alive: HashSet<u32> = pids.iter().copied().collect();
        self.paths.retain(|pid, _| alive.contains(pid));
        for pid in pids {
            if pid == 0 || self.paths.contains_key(&pid) {
                continue;
            }
            let mut path = String::new();
            unsafe {
                let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
                if !h.is_null() {
                    let mut buf = [0u16; 1024];
                    let mut len = buf.len() as u32;
                    if QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, buf.as_mut_ptr(), &mut len)
                        != 0
                    {
                        path = String::from_utf16_lossy(&buf[..len as usize]).to_lowercase();
                    }
                    CloseHandle(h);
                }
            }
            self.paths.insert(pid, path);
        }
        self.paths
            .values()
            .filter(|p| !p.is_empty())
            .cloned()
            .collect()
    }

    #[cfg(not(windows))]
    pub fn processes(&mut self) -> Vec<String> {
        vec![]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scan::SavePath;

    fn game(name: &str, exe: Option<&str>, dir: Option<&str>) -> Game {
        Game {
            name: name.into(),
            source: "custom".into(),
            install_dir: dir.map(Into::into),
            exe: exe.map(Into::into),
            steam_id: None,
            cover: None,
            match_dirs: vec![],
            match_exes: vec![],
            paths: vec![SavePath {
                path: "x".into(),
                detected: false,
                enabled: true,
                exists: true,
            }],
        }
    }

    #[test]
    fn recognises_games_by_database_hints() {
        let mut hk = game("Hollow Knight", None, None);
        hk.match_dirs = vec!["Hollow Knight".into()];
        hk.match_exes = vec!["hollow_knight.exe".into()];
        let mut nothing = game("Mystery", None, None);
        nothing.match_dirs = vec![];
        let t = targets(&[hk, nothing]);
        assert_eq!(
            t.len(),
            1,
            "games with no way to recognise them aren't watched"
        );

        let mut w = Watcher::default();
        let now = Instant::now();
        let xbox = vec![r"c:\xboxgames\hollow knight\content\hollow_knight.exe".to_string()];
        assert_eq!(w.step(&t, &xbox, now).0, vec!["Hollow Knight"]);
        let mut w = Watcher::default();
        let elsewhere = vec![r"d:\games\hk-v1.5\hollow_knight.exe".to_string()];
        assert_eq!(w.step(&t, &elsewhere, now).0, vec!["Hollow Knight"]);
        let mut w = Watcher::default();
        let unrelated = vec![r"c:\games\hollow knightmare\other.exe".to_string()];
        assert!(w.step(&t, &unrelated, now).0.is_empty());
    }

    #[test]
    fn backs_up_after_exit_and_settle() {
        let t = targets(&[
            game(
                "Elden Ring",
                None,
                Some(r"D:\Steam\steamapps\common\ELDEN RING"),
            ),
            game("Emu", Some(r"E:\Emu\pcsx2.exe"), Some(r"E:\Emu")),
        ]);
        let mut w = Watcher::default();
        let t0 = Instant::now();
        let er = r"d:\steam\steamapps\common\elden ring\game\eldenring.exe".to_string();

        let (started, due) = w.step(&t, &[er.clone()], t0);
        assert_eq!(started, vec!["Elden Ring"]);
        assert!(due.is_empty());

        assert!(w.step(&t, &[], t0 + Duration::from_secs(3)).1.is_empty());
        assert_eq!(
            w.step(&t, &[], t0 + Duration::from_secs(12)).1,
            vec!["Elden Ring"]
        );
        assert!(w.step(&t, &[], t0 + Duration::from_secs(15)).1.is_empty());
    }

    #[test]
    fn restart_cancels_pending_backup() {
        let t = targets(&[game("Emu", Some(r"E:\Emu\pcsx2.exe"), Some(r"E:\Emu"))]);
        let mut w = Watcher::default();
        let t0 = Instant::now();
        let p = vec![r"e:\emu\pcsx2.exe".to_string()];
        w.step(&t, &p, t0);
        w.step(&t, &[], t0 + Duration::from_secs(3));
        w.step(&t, &p, t0 + Duration::from_secs(6));
        assert!(w.step(&t, &p, t0 + Duration::from_secs(20)).1.is_empty());
        let other = vec![r"e:\emu\tools\other.exe".to_string()];
        w.step(&t, &other, t0 + Duration::from_secs(21));
        assert_eq!(
            w.step(&t, &other, t0 + Duration::from_secs(40)).1,
            vec!["Emu"]
        );
    }

    #[test]
    fn sees_own_process() {
        let me = std::env::current_exe()
            .unwrap()
            .to_string_lossy()
            .to_lowercase();
        let procs = Watcher::default().processes();
        assert!(
            procs.contains(&me),
            "{} processes, own exe missing",
            procs.len()
        );
    }
}
