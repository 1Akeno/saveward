use serde::Serialize;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindowBuilder};

pub const LABEL: &str = "popup";
const W: f64 = 360.0;
const H: f64 = 108.0;

#[derive(Serialize, Clone, Default)]
pub struct Progress {
    pub game: String,
    pub stage: String,
    pub done: u64,
    pub total: u64,
    pub message: String,
    pub steam_id: Option<u64>,
    pub cover: Option<String>,
    pub upload_next: bool,
    pub cloud: Option<String>,
    pub batch_index: usize,
    pub batch_total: usize,
}

#[derive(Default)]
pub struct Popup {
    last: Mutex<Option<Progress>>,
    last_emit: Mutex<Option<Instant>>,
    art: Mutex<std::collections::HashMap<String, (Option<u64>, Option<String>)>>,
}

impl Popup {
    pub fn latest(&self) -> Option<Progress> {
        self.last.lock().unwrap().clone()
    }

    pub fn remember_art(&self, game: &str, steam_id: Option<u64>, cover: Option<String>) {
        self.art
            .lock()
            .unwrap()
            .insert(game.to_string(), (steam_id, cover));
    }

    pub fn send(&self, app: &AppHandle, mut p: Progress) {
        let ticking =
            (p.stage == "backup" || p.stage == "upload") && p.done < p.total && p.done > 0;
        {
            let mut last = self.last_emit.lock().unwrap();
            if ticking && last.is_some_and(|t| t.elapsed() < Duration::from_millis(100)) {
                return;
            }
            *last = Some(Instant::now());
        }
        if p.steam_id.is_none() && p.cover.is_none() {
            if let Some((id, cover)) = self.art.lock().unwrap().get(&p.game).cloned() {
                p.steam_id = id;
                p.cover = cover;
            }
        }
        *self.last.lock().unwrap() = Some(p.clone());
        ensure_window(app);
        let _ = app.emit_to(LABEL, "progress", &p);
    }
}

fn ensure_window(app: &AppHandle) {
    if app.get_webview_window(LABEL).is_some() {
        return;
    }
    let mut b = WebviewWindowBuilder::new(app, LABEL, WebviewUrl::App("popup.html".into()))
        .title("Saveward")
        .inner_size(W, H)
        .resizable(false)
        .decorations(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .focused(false)
        .visible(false)
        .shadow(true);
    if let Ok(Some(m)) = app.primary_monitor() {
        let scale = m.scale_factor();
        let area = m.work_area();
        let right = (area.position.x as f64 + area.size.width as f64) / scale;
        let bottom = (area.position.y as f64 + area.size.height as f64) / scale;
        b = b.position(right - W - 16.0, bottom - H - 16.0);
    }
    let _ = b.build();
}
