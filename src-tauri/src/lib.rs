mod backup;
mod cloud;
mod config;
mod manifest;
mod popup;
mod scan;
mod stores;
mod sync;
mod watcher;

use config::{GameOverride, Settings};
use scan::{Detected, Game};
use serde::Serialize;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, RunEvent, State, Wry};
use tauri_plugin_notification::NotificationExt;

struct AppState {
    settings: Mutex<Settings>,
    detected: Mutex<Detected>,
    dirty: AtomicBool,
    backup_lock: Mutex<()>,
    running: Mutex<Vec<String>>,
    auto_item: Mutex<Option<CheckMenuItem<Wry>>>,
    sync: sync::Sync,
    check_cloud: AtomicBool,
    popup: popup::Popup,
    batch_running: AtomicBool,
}

#[derive(Serialize)]
struct GameRow {
    #[serde(flatten)]
    game: Game,
    last_backup: Option<String>,
    versions: usize,
}

type Res<T> = Result<T, String>;

fn all_games(state: &AppState) -> Vec<Game> {
    let settings = state.settings.lock().unwrap();
    let detected = state.detected.lock().unwrap();
    scan::merge(&detected, &settings)
}

fn find_game(state: &AppState, name: &str) -> Res<Game> {
    all_games(state)
        .into_iter()
        .find(|g| g.name == name)
        .ok_or_else(|| format!("Unknown game: {name}"))
}

fn row(settings: &Settings, game: Game) -> GameRow {
    let versions = backup::list(settings, &game.name);
    GameRow {
        last_backup: versions.first().map(|v| v.created.clone()),
        versions: versions.len(),
        game,
    }
}

fn rows(state: &AppState) -> Vec<GameRow> {
    let games = all_games(state);
    let settings = state.settings.lock().unwrap();
    games.into_iter().map(|g| row(&settings, g)).collect()
}

fn do_backup(app: &AppHandle, game: &Game, note: &str) -> Res<backup::BackupResult> {
    do_backup_in(app, game, note, None)
}

fn do_backup_in(
    app: &AppHandle,
    game: &Game,
    note: &str,
    batch: Option<(usize, usize)>,
) -> Res<backup::BackupResult> {
    let state = app.state::<AppState>();
    let _guard = state.backup_lock.lock().unwrap();
    let settings = state.settings.lock().unwrap().clone();
    let popup_on = settings.show_popup;
    let cloud_label = settings
        .cloud
        .as_deref()
        .and_then(|p| cloud::Provider::parse(p).ok())
        .map(|p| p.label().to_string());
    let base = popup::Progress {
        game: game.name.clone(),
        steam_id: game.steam_id,
        cover: game.cover.clone(),
        cloud: cloud_label.clone(),
        batch_index: batch.map_or(0, |b| b.0),
        batch_total: batch.map_or(0, |b| b.1),
        ..Default::default()
    };
    if popup_on {
        state
            .popup
            .remember_art(&game.name, game.steam_id, game.cover.clone());
    }
    let show = |p: popup::Progress| {
        if popup_on {
            state.popup.send(app, p);
        }
    };

    let result = backup::backup_progress(&settings, game, note, &mut |done, total| {
        show(popup::Progress {
            stage: "backup".into(),
            done,
            total,
            ..base.clone()
        });
    });
    if let Ok(r) = &result {
        if let (Some(v), Some(_)) = (&r.version, &settings.cloud) {
            state.sync.enqueue(&game.name, &v.file);
        }
    }
    if batch.is_none() {
        match &result {
            Ok(r) if r.created => show(popup::Progress {
                stage: "saved".into(),
                upload_next: settings.cloud.is_some(),
                ..base.clone()
            }),
            Ok(_) => show(popup::Progress {
                stage: "same".into(),
                ..base.clone()
            }),
            Err(e) => show(popup::Progress {
                stage: "failed".into(),
                message: e.clone(),
                ..base.clone()
            }),
        }
    }
    result
}

fn upload_progress(app: &AppHandle, up: sync::Up) {
    let state = app.state::<AppState>();
    let (popup_on, cloud_label) = {
        let s = state.settings.lock().unwrap();
        (
            s.show_popup,
            s.cloud
                .as_deref()
                .and_then(|p| cloud::Provider::parse(p).ok())
                .map(|p| p.label().to_string()),
        )
    };
    if !popup_on {
        return;
    }
    let p = match up {
        sync::Up::Progress(game, done, total) => popup::Progress {
            game: game.into(),
            stage: "upload".into(),
            done,
            total,
            cloud: cloud_label,
            ..Default::default()
        },
        sync::Up::Done(game) => popup::Progress {
            game: game.into(),
            stage: "done".into(),
            cloud: cloud_label,
            ..Default::default()
        },
        sync::Up::Failed(game, e) => popup::Progress {
            game: game.into(),
            stage: "upload-failed".into(),
            message: e.into(),
            cloud: cloud_label,
            ..Default::default()
        },
    };
    state.popup.send(app, p);
}

#[tauri::command]
fn get_games(state: State<AppState>) -> Vec<GameRow> {
    rows(&state)
}

#[tauri::command]
fn get_game(state: State<AppState>, name: String) -> Option<GameRow> {
    let game = find_game(&state, &name).ok()?;
    Some(row(&state.settings.lock().unwrap(), game))
}

#[tauri::command]
fn get_running(state: State<AppState>) -> Vec<String> {
    state.running.lock().unwrap().clone()
}

#[tauri::command]
async fn scan_games(state: State<'_, AppState>, refresh_db: bool) -> Res<Vec<GameRow>> {
    let detected = tauri::async_runtime::spawn_blocking(move || scan::detect(refresh_db))
        .await
        .map_err(|e| e.to_string())??;
    *state.detected.lock().unwrap() = detected;
    state.dirty.store(true, Ordering::Relaxed);
    Ok(rows(&state))
}

fn edit_override(state: &AppState, name: &str, f: impl FnOnce(&mut GameOverride)) -> Res<()> {
    let mut settings = state.settings.lock().unwrap();
    f(settings.games.entry(name.to_string()).or_default());
    state.dirty.store(true, Ordering::Relaxed);
    settings.save()
}

#[tauri::command]
fn add_custom_game(
    state: State<AppState>,
    name: String,
    paths: Vec<String>,
    exe: Option<String>,
) -> Res<()> {
    let name = name.trim().to_string();
    if name.is_empty() {
        return Err("Name is required.".into());
    }
    edit_override(&state, &name, |o| {
        o.custom = true;
        o.ignored = false;
        o.extra_paths = paths;
        o.exe = exe.filter(|e| !e.is_empty());
    })
}

#[tauri::command]
fn remove_game(state: State<AppState>, name: String) -> Res<()> {
    let mut settings = state.settings.lock().unwrap();
    let is_custom = settings.games.get(&name).is_some_and(|o| o.custom)
        && !state.detected.lock().unwrap().games.contains_key(&name);
    if is_custom {
        if let Some(c) = settings.games.remove(&name).and_then(|o| o.cover) {
            remove_cover_file(&c);
        }
    } else {
        settings.games.entry(name).or_default().ignored = true;
    }
    state.dirty.store(true, Ordering::Relaxed);
    settings.save()
}

#[tauri::command]
fn add_path(state: State<AppState>, name: String, path: String) -> Res<()> {
    edit_override(&state, &name, |o| {
        o.excluded_paths.retain(|p| p != &path);
        if !o.extra_paths.contains(&path) {
            o.extra_paths.push(path);
        }
    })
}

#[tauri::command]
fn toggle_path(state: State<AppState>, name: String, path: String, enabled: bool) -> Res<()> {
    edit_override(&state, &name, |o| {
        if o.extra_paths.contains(&path) {
            o.extra_paths.retain(|p| p != &path);
        } else if enabled {
            o.excluded_paths.retain(|p| p != &path);
        } else if !o.excluded_paths.contains(&path) {
            o.excluded_paths.push(path);
        }
    })
}

#[tauri::command]
fn set_exe(state: State<AppState>, name: String, exe: Option<String>) -> Res<()> {
    edit_override(&state, &name, |o| o.exe = exe.filter(|e| !e.is_empty()))
}

fn remove_cover_file(path: &str) {
    let p = std::path::Path::new(path);
    if p.starts_with(config::covers_dir()) {
        let _ = std::fs::remove_file(p);
    }
}

#[tauri::command]
fn set_cover(state: State<AppState>, name: String, image: Option<String>) -> Res<()> {
    let new_path = match image {
        Some(src) => {
            let src = std::path::Path::new(&src);
            let ext = src
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("png")
                .to_lowercase();
            if !["png", "jpg", "jpeg", "webp", "gif", "bmp", "avif"].contains(&ext.as_str()) {
                return Err("Pick a PNG, JPG, WEBP, GIF or BMP image.".into());
            }
            let file = format!(
                "{}-{}.{ext}",
                backup::safe_name(&name),
                chrono::Local::now().timestamp_millis()
            );
            let dest = config::covers_dir().join(file);
            std::fs::copy(src, &dest).map_err(|e| format!("Couldn't copy the image: {e}"))?;
            Some(dest.to_string_lossy().into_owned())
        }
        None => None,
    };
    let mut old = None;
    edit_override(&state, &name, |o| {
        old = std::mem::replace(&mut o.cover, new_path)
    })?;
    if let Some(old) = old {
        remove_cover_file(&old);
    }
    Ok(())
}

#[tauri::command]
async fn backup_game(app: AppHandle, name: String) -> Res<backup::BackupResult> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let game = find_game(&state, &name)?;
        do_backup(&app, &game, "Manual")
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
fn list_backups(state: State<AppState>, name: String) -> Vec<backup::Version> {
    backup::list(&state.settings.lock().unwrap(), &name)
}

#[tauri::command]
async fn restore_backup(app: AppHandle, name: String, file: String) -> Res<usize> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        if state.running.lock().unwrap().contains(&name) {
            return Err(format!(
                "{name} is running. Quit the game first, then restore."
            ));
        }
        let game = find_game(&state, &name)?;
        let _guard = state.backup_lock.lock().unwrap();
        let settings = state.settings.lock().unwrap().clone();
        backup::restore(&settings, &game, &file)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
fn pin_backup(state: State<AppState>, name: String, file: String, pinned: bool) -> Res<()> {
    backup::set_pinned(&state.settings.lock().unwrap(), &name, &file, pinned)
}

#[tauri::command]
fn delete_backup(state: State<AppState>, name: String, file: String) -> Res<()> {
    backup::delete(&state.settings.lock().unwrap(), &name, &file)
}

#[tauri::command]
fn get_settings(state: State<AppState>) -> Settings {
    state.settings.lock().unwrap().clone()
}

#[tauri::command]
fn update_settings(
    state: State<AppState>,
    backup_dir: String,
    versions_to_keep: usize,
    auto_backup: bool,
    show_covers: bool,
    start_with_windows: bool,
    show_popup: bool,
) -> Res<()> {
    let mut s = state.settings.lock().unwrap();
    if s.start_with_windows != start_with_windows {
        config::set_autostart(start_with_windows)?;
    }
    s.backup_dir = backup_dir;
    s.versions_to_keep = versions_to_keep.clamp(1, 1000);
    s.auto_backup = auto_backup;
    s.show_covers = show_covers;
    s.start_with_windows = start_with_windows;
    s.show_popup = show_popup;
    if let Some(item) = state.auto_item.lock().unwrap().as_ref() {
        let _ = item.set_checked(auto_backup);
    }
    state.dirty.store(true, Ordering::Relaxed);
    s.save()
}

#[derive(Serialize)]
struct CloudStatus {
    provider: Option<String>,
    label: Option<String>,
    pending: usize,
    busy: bool,
    last_error: Option<String>,
    last_sync: Option<String>,
    newer: std::collections::HashMap<String, sync::CloudVersion>,
    ready: std::collections::HashMap<String, bool>,
}

#[tauri::command]
fn cloud_status(state: State<AppState>) -> CloudStatus {
    let s = state.settings.lock().unwrap();
    let ready = [
        (
            "google",
            !s.cloud_apps.google().0.is_empty() && !s.cloud_apps.google().1.is_empty(),
        ),
        ("onedrive", !s.cloud_apps.onedrive().is_empty()),
        ("dropbox", !s.cloud_apps.dropbox().is_empty()),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v))
    .collect();
    CloudStatus {
        provider: s.cloud.clone(),
        label: s
            .cloud
            .as_deref()
            .and_then(|p| cloud::Provider::parse(p).ok())
            .map(|p| p.label().to_string()),
        pending: state.sync.pending(),
        busy: *state.sync.busy.lock().unwrap(),
        last_error: state.sync.last_error.lock().unwrap().clone(),
        last_sync: state.sync.last_sync.lock().unwrap().clone(),
        newer: state.sync.newer.lock().unwrap().clone(),
        ready,
    }
}

#[tauri::command]
async fn cloud_connect(
    app: AppHandle,
    provider: String,
    app_id: String,
    app_secret: String,
) -> Res<()> {
    let p = cloud::Provider::parse(&provider)?;
    let creds = {
        let state = app.state::<AppState>();
        let mut s = state.settings.lock().unwrap();
        if !app_id.trim().is_empty() {
            match p {
                cloud::Provider::Google => {
                    s.cloud_apps.google_id = app_id.trim().into();
                    s.cloud_apps.google_secret = app_secret.trim().into();
                }
                cloud::Provider::OneDrive => s.cloud_apps.onedrive_id = app_id.trim().into(),
                cloud::Provider::Dropbox => s.cloud_apps.dropbox_key = app_id.trim().into(),
            }
            s.save()?;
        }
        sync::creds(&s, p)
    };
    if creds.id.is_empty() || (p == cloud::Provider::Google && creds.secret.is_empty()) {
        return Err(format!(
            "{} needs an app registration first (see the steps in this window).",
            p.label()
        ));
    }
    let handle = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        use tauri_plugin_opener::OpenerExt;
        cloud::sign_in(p, &creds, |url| {
            let _ = handle.opener().open_url(url, None::<&str>);
        })
    })
    .await
    .map_err(|e| e.to_string())??;

    let state = app.state::<AppState>();
    let settings = {
        let mut s = state.settings.lock().unwrap();
        if let Some(old) = s
            .cloud
            .as_deref()
            .and_then(|o| cloud::Provider::parse(o).ok())
            .filter(|o| *o != p)
        {
            cloud::forget(old);
        }
        s.cloud = Some(p.id().into());
        s.save()?;
        s.clone()
    };
    state.sync.reset_client();
    state.sync.clear_queue();
    for g in all_games(&state) {
        if let Some(v) = backup::list(&settings, &g.name)
            .first()
            .filter(|v| !v.uploaded)
        {
            state.sync.enqueue(&g.name, &v.file);
        }
    }
    state.check_cloud.store(true, Ordering::Relaxed);
    state.sync.poke();
    Ok(())
}

#[tauri::command]
fn cloud_disconnect(state: State<AppState>) -> Res<()> {
    let mut s = state.settings.lock().unwrap();
    if let Some(p) = s.cloud.take().and_then(|p| cloud::Provider::parse(&p).ok()) {
        cloud::forget(p);
    }
    state.sync.reset_client();
    state.sync.clear_queue();
    s.save()
}

#[tauri::command]
fn popup_state(state: State<AppState>) -> Option<popup::Progress> {
    state.popup.latest()
}

#[tauri::command]
fn open_main(app: AppHandle) {
    show_window(&app);
}

#[tauri::command]
fn cloud_cancel_sign_in() {
    cloud::CANCEL.store(true, Ordering::Relaxed);
}

#[tauri::command]
fn cloud_sync_now(state: State<AppState>) {
    state.check_cloud.store(true, Ordering::Relaxed);
    state.sync.poke();
}

#[tauri::command]
async fn cloud_list(app: AppHandle, name: String) -> Res<Vec<sync::CloudVersion>> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let settings = state.settings.lock().unwrap().clone();
        let cloud = state.sync.client(&settings)?;
        sync::cloud_versions(&cloud, &settings, &name, &backup::list(&settings, &name))
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn cloud_restore(app: AppHandle, name: String, remote: String) -> Res<usize> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        if state.running.lock().unwrap().contains(&name) {
            return Err(format!(
                "{name} is running. Quit the game first, then restore."
            ));
        }
        let game = find_game(&state, &name)?;
        let settings = state.settings.lock().unwrap().clone();
        let cloud = state.sync.client(&settings)?;
        let _guard = state.backup_lock.lock().unwrap();
        let file = sync::fetch(&cloud, &settings, &name, &remote)?;
        let n = backup::restore(&settings, &game, &file)?;
        state.sync.newer.lock().unwrap().remove(&name);
        let _ = app.emit("cloud", ());
        Ok(n)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
fn game_backup_dir(state: State<AppState>, name: String) -> String {
    let s = state.settings.lock().unwrap();
    let dir = backup::game_dir(&s, &name);
    let _ = std::fs::create_dir_all(&dir);
    dir.to_string_lossy().into_owned()
}

fn show_window(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
        return;
    }
    let Some(cfg) = app.config().app.windows.first().cloned() else {
        return;
    };
    if tauri::WebviewWindowBuilder::from_config(app, &cfg)
        .and_then(|b| b.build())
        .is_ok()
    {
        let handle = app.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_secs(3));
            if let Some(w) = handle.get_webview_window("main") {
                let _ = w.show();
            }
        });
    }
}

fn notify(app: &AppHandle, body: &str) {
    let _ = app
        .notification()
        .builder()
        .title("Saveward")
        .body(body)
        .show();
}

fn set_tooltip(app: &AppHandle, text: &str) {
    if let Some(tray) = app.tray_by_id("main") {
        let _ = tray.set_tooltip(Some(text));
    }
}

fn backup_all(app: AppHandle) {
    let state = app.state::<AppState>();
    if state.batch_running.swap(true, Ordering::SeqCst) {
        return;
    }
    let _ = app.emit("batch", true);
    std::thread::spawn(move || {
        let state = app.state::<AppState>();
        let games: Vec<Game> = all_games(&state)
            .into_iter()
            .filter(|g| g.paths.iter().any(|p| p.enabled && p.exists))
            .collect();
        let total = games.len();
        let (mut made, mut same, mut failed) = (0, 0, 0);
        for (i, g) in games.iter().enumerate() {
            match do_backup_in(&app, g, "Back up all", Some((i + 1, total))) {
                Ok(r) if r.created => made += 1,
                Ok(_) => same += 1,
                Err(_) => failed += 1,
            }
            let _ = app.emit("backup-done", &g.name);
        }
        let mut msg = match made {
            0 => "Nothing new to back up.".to_string(),
            1 => "Backed up 1 game.".to_string(),
            n => format!("Backed up {n} games."),
        };
        if same > 0 {
            msg.push_str(&format!(" {same} hadn't changed."));
        }
        if failed > 0 {
            msg.push_str(&format!(" {failed} couldn't be backed up."));
        }
        let (popup_on, cloud_label) = {
            let s = state.settings.lock().unwrap();
            (
                s.show_popup,
                s.cloud
                    .as_deref()
                    .and_then(|p| cloud::Provider::parse(p).ok())
                    .map(|p| p.label().to_string()),
            )
        };
        if popup_on {
            state.popup.send(
                &app,
                popup::Progress {
                    game: "All games".into(),
                    stage: if failed > 0 {
                        "batch-failed".into()
                    } else {
                        "batch".into()
                    },
                    message: msg.clone(),
                    upload_next: made > 0 && cloud_label.is_some(),
                    cloud: cloud_label,
                    ..Default::default()
                },
            );
        } else {
            notify(&app, &msg);
        }
        state.batch_running.store(false, Ordering::SeqCst);
        state.sync.poke();
        let _ = app.emit("batch", false);
        let _ = app.emit("batch-summary", &msg);
    });
}

#[tauri::command]
fn backup_all_games(app: AppHandle) -> Res<()> {
    if app.state::<AppState>().batch_running.load(Ordering::SeqCst) {
        return Err("Already backing up all games.".into());
    }
    backup_all(app);
    Ok(())
}

#[tauri::command]
fn batch_running(state: State<AppState>) -> bool {
    state.batch_running.load(Ordering::SeqCst)
}

fn build_tray(app: &AppHandle) -> tauri::Result<()> {
    let auto_on = app.state::<AppState>().settings.lock().unwrap().auto_backup;
    let open = MenuItem::with_id(app, "open", "Open Saveward", true, None::<&str>)?;
    let all = MenuItem::with_id(
        app,
        "backup_all",
        "Back up all games now",
        true,
        None::<&str>,
    )?;
    let auto = CheckMenuItem::with_id(
        app,
        "auto",
        "Back up when a game closes",
        true,
        auto_on,
        None::<&str>,
    )?;
    let quit = MenuItem::with_id(app, "quit", "Quit Saveward", true, None::<&str>)?;
    let sep1 = PredefinedMenuItem::separator(app)?;
    let sep2 = PredefinedMenuItem::separator(app)?;
    let menu = Menu::with_items(app, &[&open, &all, &sep1, &auto, &sep2, &quit])?;
    *app.state::<AppState>().auto_item.lock().unwrap() = Some(auto.clone());

    TrayIconBuilder::with_id("main")
        .icon(app.default_window_icon().cloned().expect("app icon"))
        .tooltip("Saveward")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(move |app, e| match e.id.as_ref() {
            "open" => show_window(app),
            "backup_all" => backup_all(app.clone()),
            "auto" => {
                let state = app.state::<AppState>();
                let mut s = state.settings.lock().unwrap();
                s.auto_backup = !s.auto_backup;
                let _ = auto.set_checked(s.auto_backup);
                let _ = s.save();
            }
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, e| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = e
            {
                show_window(tray.app_handle());
            }
        })
        .build(app)?;
    Ok(())
}

fn start_watcher(app: AppHandle) {
    std::thread::spawn(move || {
        let mut w = watcher::Watcher::default();
        let mut targets = Vec::new();
        let mut ticks = 0u32;
        loop {
            std::thread::sleep(Duration::from_secs(3));
            let state = app.state::<AppState>();
            if !state.settings.lock().unwrap().auto_backup {
                continue;
            }
            if state.dirty.swap(false, Ordering::Relaxed) || ticks % 20 == 0 {
                targets = watcher::targets(&all_games(&state));
            }
            ticks = ticks.wrapping_add(1);
            if targets.is_empty() {
                continue;
            }

            let procs = w.processes();
            let (started, due) = w.step(&targets, &procs, Instant::now());
            if !started.is_empty() || !due.is_empty() {
                let mut running: Vec<String> = w.running().iter().cloned().collect();
                running.sort();
                *state.running.lock().unwrap() = running.clone();
                let _ = app.emit("running", &running);
                set_tooltip(
                    &app,
                    &match running.first() {
                        Some(g) => format!("Saveward: watching {g}"),
                        None => "Saveward".into(),
                    },
                );
            }

            for name in due {
                let Ok(game) = find_game(&state, &name) else {
                    continue;
                };
                match do_backup(&app, &game, "Auto: game closed") {
                    Ok(r) if r.created => {
                        notify(&app, &format!("{name} backed up."));
                        set_tooltip(
                            &app,
                            &format!(
                                "Saveward: {name} backed up at {}",
                                chrono::Local::now().format("%H:%M")
                            ),
                        );
                    }
                    Ok(_) => {}
                    Err(e) => notify(&app, &format!("Couldn't back up {name}: {e}")),
                }
                let _ = app.emit("backup-done", &name);
            }
        }
    });
}

fn start_sync(app: AppHandle) {
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(15));
        let mut last_check: Option<Instant> = None;
        loop {
            let state = app.state::<AppState>();
            let settings = state.settings.lock().unwrap().clone();
            if settings.cloud.is_some() && !state.batch_running.load(Ordering::SeqCst) {
                *state.sync.busy.lock().unwrap() = true;
                let _ = app.emit("cloud", ());
                let mut result = state
                    .sync
                    .upload_queued(&settings, &|up| upload_progress(&app, up))
                    .map(|_| ());

                let due = last_check.is_none_or(|t| t.elapsed() > Duration::from_secs(30 * 60));
                if result.is_ok() && (due || state.check_cloud.swap(false, Ordering::Relaxed)) {
                    let names: Vec<String> =
                        all_games(&state).into_iter().map(|g| g.name).collect();
                    match state.sync.check_newer(&settings, &names) {
                        Ok(fresh) => {
                            last_check = Some(Instant::now());
                            for g in fresh {
                                let from = state
                                    .sync
                                    .newer
                                    .lock()
                                    .unwrap()
                                    .get(&g)
                                    .map(|v| v.pc.clone())
                                    .unwrap_or_default();
                                notify(
                                    &app,
                                    &format!(
                                        "A newer save of {g} from {from} is in the cloud. Open Saveward to restore it."
                                    ),
                                );
                            }
                        }
                        Err(e) => result = Err(e),
                    }
                }
                *state.sync.busy.lock().unwrap() = false;
                match result {
                    Ok(()) => {
                        *state.sync.last_error.lock().unwrap() = None;
                        *state.sync.last_sync.lock().unwrap() =
                            Some(chrono::Local::now().to_rfc3339());
                    }
                    Err(e) => *state.sync.last_error.lock().unwrap() = Some(e),
                }
                let _ = app.emit("cloud", ());
            }
            state.sync.wait(Duration::from_secs(120));
        }
    });
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let background = std::env::args().any(|a| a == "--background");

    let app = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            show_window(app)
        }))
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .manage(AppState {
            settings: Mutex::new(Settings::load()),
            detected: Mutex::new(scan::load_cached()),
            dirty: AtomicBool::new(true),
            backup_lock: Mutex::new(()),
            running: Mutex::new(Vec::new()),
            auto_item: Mutex::new(None),
            sync: sync::Sync::new(),
            check_cloud: AtomicBool::new(true),
            popup: popup::Popup::default(),
            batch_running: AtomicBool::new(false),
        })
        .setup(move |app| {
            if app
                .state::<AppState>()
                .settings
                .lock()
                .unwrap()
                .start_with_windows
            {
                let _ = config::set_autostart(true);
            }
            build_tray(app.handle())?;
            start_watcher(app.handle().clone());
            start_sync(app.handle().clone());
            if !background {
                show_window(app.handle());
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_games,
            get_game,
            get_running,
            scan_games,
            add_custom_game,
            remove_game,
            add_path,
            toggle_path,
            set_exe,
            set_cover,
            backup_game,
            list_backups,
            restore_backup,
            pin_backup,
            delete_backup,
            get_settings,
            update_settings,
            game_backup_dir,
            cloud_status,
            cloud_connect,
            cloud_disconnect,
            cloud_cancel_sign_in,
            cloud_sync_now,
            popup_state,
            open_main,
            backup_all_games,
            batch_running,
            cloud_list,
            cloud_restore,
        ])
        .build(tauri::generate_context!())
        .expect("error while starting Saveward");

    app.run(|_app, event| {
        if let RunEvent::ExitRequested {
            api, code: None, ..
        } = event
        {
            api.prevent_exit();
        }
    });
}

#[cfg(test)]
mod real_checks {
    #[test]
    #[ignore]
    fn hint_coverage() {
        let d = crate::scan::detect(false).unwrap();
        let with_saves: Vec<_> = d
            .games
            .iter()
            .filter(|(_, g)| !g.paths.is_empty())
            .collect();
        let store = with_saves
            .iter()
            .filter(|(_, g)| g.install_dir.is_some())
            .count();
        let hinted: Vec<_> = with_saves
            .iter()
            .filter(|(_, g)| {
                g.install_dir.is_none() && (!g.match_dirs.is_empty() || !g.match_exes.is_empty())
            })
            .collect();
        let none: Vec<_> = with_saves
            .iter()
            .filter(|(_, g)| {
                g.install_dir.is_none() && g.match_dirs.is_empty() && g.match_exes.is_empty()
            })
            .map(|(n, _)| n.as_str())
            .collect();
        println!(
            "games with saves: {}, via store install: {}, via database hints: {}, still need 'Choose program': {}",
            with_saves.len(),
            store,
            hinted.len(),
            none.len()
        );
        for (n, g) in hinted.iter().take(6) {
            println!("  {n}: dirs {:?} exes {:?}", g.match_dirs, g.match_exes);
        }
        println!("  no hints: {:?}", none);
    }

    #[test]
    #[ignore]
    fn credential_manager() {
        let e = keyring::Entry::new("Saveward-selftest", "probe").unwrap();
        e.set_password("hello").unwrap();
        assert_eq!(e.get_password().unwrap(), "hello");
        e.delete_credential().unwrap();
        println!("credential manager: store/read/delete OK");
    }
}
