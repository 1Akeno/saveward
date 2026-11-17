import "@fontsource-variable/archivo/wdth.css";
import "./popup.css";
import { invoke, convertFileSrc } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";

type Progress = {
  game: string;
  stage: "backup" | "saved" | "same" | "failed" | "upload" | "done" | "upload-failed" | "batch" | "batch-failed";
  batch_index: number;
  batch_total: number;
  done: number;
  total: number;
  message: string;
  steam_id: number | null;
  cover: string | null;
  upload_next: boolean;
  cloud: string | null;
};

const $ = (id: string) => document.getElementById(id)!;
const win = (window as any).__TAURI_INTERNALS__ ? getCurrentWindow() : ({ show() {}, close() {} } as any);
let closeTimer = 0;
let hovering = false;
let lastGame = "";
let shown = false;

function mb(b: number) {
  return b < 1048576 ? `${Math.max(1, Math.round(b / 1024))} KB` : `${(b / 1048576).toFixed(1)} MB`;
}

function render(p: Progress) {
  if (p.game !== lastGame) {
    lastGame = p.game;
    $("game").textContent = p.game;
    const src = p.cover ? convertFileSrc(p.cover) : p.steam_id ? `https://cdn.akamai.steamstatic.com/steam/apps/${p.steam_id}/library_600x900.jpg` : "";
    $("art").innerHTML = src ? `<img src="${src}" alt="" onerror="this.parentElement.innerHTML=''">` : "";
  }
  const cloud = p.cloud ?? "the cloud";
  const pct = p.total > 0 ? Math.min(100, Math.round((p.done / p.total) * 100)) : 0;
  let line = "";
  let sub = "";
  let bar = 0;
  let kind = "";
  let final = false;
  switch (p.stage) {
    case "backup":
      if (p.batch_total) {
        line = `Backing up all games: ${p.batch_index} of ${p.batch_total}`;
        sub = p.total ? `${mb(p.done)} of ${mb(p.total)}` : "";
        bar = Math.round(((p.batch_index - 1 + pct / 100) / p.batch_total) * 100);
      } else {
        line = `Backing up… ${pct}%`;
        sub = p.total ? `${mb(p.done)} of ${mb(p.total)}` : "";
        bar = pct;
      }
      break;
    case "batch":
    case "batch-failed":
      line = p.upload_next ? `${p.message} Uploading to ${cloud}…` : p.message;
      bar = p.upload_next ? 0 : 100;
      kind = p.upload_next ? "" : p.stage === "batch" ? "ok" : "bad";
      final = !p.upload_next;
      break;
    case "saved":
      line = p.upload_next ? `Backed up. Uploading to ${cloud}…` : "Backed up";
      bar = p.upload_next ? 0 : 100;
      kind = p.upload_next ? "" : "ok";
      final = !p.upload_next;
      break;
    case "upload":
      line = `Uploading to ${cloud}… ${pct}%`;
      sub = p.total ? `${mb(p.done)} of ${mb(p.total)}` : "";
      bar = pct;
      break;
    case "done":
      line = `Backed up and uploaded to ${cloud}`;
      bar = 100;
      kind = "ok";
      final = true;
      break;
    case "same":
      line = "No changes since the last backup";
      bar = 100;
      kind = "quiet";
      final = true;
      break;
    case "failed":
      line = "Couldn't back up";
      sub = p.message;
      bar = 100;
      kind = "bad";
      final = true;
      break;
    case "upload-failed":
      line = "Backed up on this PC. Upload will retry later.";
      sub = p.message;
      bar = 100;
      kind = "bad";
      final = true;
      break;
  }
  $("line").textContent = line;
  $("sub").textContent = sub;
  $("bar").style.width = `${bar}%`;
  $("pop").className = `pop ${kind}`;

  if (!shown) {
    shown = true;
    win.show();
  }
  clearTimeout(closeTimer);
  closeTimer = window.setTimeout(close, final ? (kind === "bad" ? 9000 : 3500) : 60000);
}

function close() {
  if (hovering) {
    closeTimer = window.setTimeout(close, 1500);
    return;
  }
  win.close();
}

$("pop").addEventListener("mouseenter", () => (hovering = true));
$("pop").addEventListener("mouseleave", () => (hovering = false));
$("pop").addEventListener("click", (e) => {
  if ((e.target as HTMLElement).id === "close") {
    win.close();
    return;
  }
  invoke("open_main");
  win.close();
});

(async () => {
  if (import.meta.env.DEV && !(window as any).__TAURI_INTERNALS__) {
    const q = new URLSearchParams(location.search);
    render({
      game: q.get("game") ?? "Elden Ring",
      stage: (q.get("stage") ?? "backup") as Progress["stage"],
      done: Number(q.get("done") ?? 28_000_000),
      total: Number(q.get("total") ?? 46_000_000),
      message: q.get("msg") ?? "",
      steam_id: q.get("nosteam") ? null : 1245620,
      cover: null,
      upload_next: q.get("next") === "1",
      cloud: "Google Drive",
      batch_index: Number(q.get("bi") ?? 0),
      batch_total: Number(q.get("bt") ?? 0),
    });
    return;
  }
  await listen<Progress>("progress", (e) => render(e.payload));
  const p = await invoke<Progress | null>("popup_state");
  if (p) render(p);
})();
