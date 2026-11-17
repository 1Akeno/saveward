import "@fontsource-variable/archivo/wdth.css";
import "./styles.css";
import { icon } from "./icons";
import { initCloud, newerBanner, cloudOnly, forget as forgetCloud, cloud, refresh as refreshCloud } from "./cloud";
import { invoke, convertFileSrc } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { open, ask } from "@tauri-apps/plugin-dialog";
import { revealItemInDir } from "@tauri-apps/plugin-opener";

type SavePath = { path: string; detected: boolean; enabled: boolean; exists: boolean };
type Game = {
  name: string;
  source: string;
  install_dir: string | null;
  exe: string | null;
  paths: SavePath[];
  steam_id: number | null;
  cover: string | null;
  match_dirs: string[];
  match_exes: string[];
  last_backup: string | null;
  versions: number;
};
type Settings = {
  backup_dir: string;
  versions_to_keep: number;
  auto_backup: boolean;
  show_covers: boolean;
  start_with_windows: boolean;
  show_popup: boolean;
};
type Version = {
  file: string;
  created: string;
  pc: string;
  size: number;
  files: number;
  pinned: boolean;
  note: string;
  uploaded: boolean;
};

const $ = <T extends HTMLElement = HTMLElement>(s: string) => document.querySelector(s) as T;
const listEl = $("#list");
const detailEl = $("#detail");
const statusEl = $("#status");

let games: Game[] = [];
let shown: Game[] = [];
let selected: string | null = null;
let filter = "saves";
let query = "";
const history = new Map<string, Version[]>();
let covers = true;
let running = new Set<string>();

const art = (id: number, kind: "library_600x900" | "library_hero") =>
  `https://cdn.akamai.steamstatic.com/steam/apps/${id}/${kind}.jpg`;

function coverSrc(g: Game) {
  if (g.cover) return convertFileSrc(g.cover);
  return g.steam_id ? art(g.steam_id, "library_600x900") : null;
}

const IMAGES = [{ name: "Images", extensions: ["png", "jpg", "jpeg", "webp", "gif", "bmp", "avif"] }];

document.addEventListener("error", (e) => {
  const t = e.target as HTMLElement;
  if (!(t instanceof HTMLImageElement) || t.dataset.art === undefined) return;
  const box = t.parentElement;
  if (box?.classList.contains("hero")) {
    box.nextElementSibling?.classList.remove("art");
    box.remove();
  } else if (box?.classList.contains("cover")) {
    box.classList.add("blank");
    t.outerHTML = `${icon("image")}<span>Add cover</span>`;
  } else {
    t.remove();
  }
}, true);
document.addEventListener("load", (e) => {
  const t = e.target as HTMLElement;
  if (t instanceof HTMLImageElement && t.dataset.art !== undefined) t.classList.add("in");
}, true);

const STORE: Record<string, string> = { steam: "Steam", epic: "Epic Games", gog: "GOG", custom: "Added by you" };

const esc = (s: string) =>
  s.replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]!);

let statusTimer = 0;
function say(msg: string, err = false) {
  statusEl.textContent = msg;
  statusEl.classList.toggle("err", err);
  clearTimeout(statusTimer);
  if (!err) statusTimer = window.setTimeout(() => (statusEl.textContent = ""), 6000);
}

async function call<T>(cmd: string, args?: Record<string, unknown>): Promise<T | undefined> {
  try {
    return await invoke<T>(cmd, args);
  } catch (e) {
    say(String(e), true);
    return undefined;
  }
}

function ago(iso: string) {
  const s = (Date.now() - new Date(iso).getTime()) / 1000;
  if (s < 60) return "just now";
  if (s < 3600) return `${Math.floor(s / 60)} min ago`;
  if (s < 86400) return `${Math.floor(s / 3600)} h ago`;
  const d = Math.floor(s / 86400);
  return d === 1 ? "yesterday" : `${d} days ago`;
}

function stamp(iso: string) {
  const d = new Date(iso);
  const time = d.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
  const today = new Date();
  const y = new Date(today);
  y.setDate(today.getDate() - 1);
  if (d.toDateString() === today.toDateString()) return `Today, ${time}`;
  if (d.toDateString() === y.toDateString()) return `Yesterday, ${time}`;
  const opts: Intl.DateTimeFormatOptions = { day: "numeric", month: "short" };
  if (d.getFullYear() !== today.getFullYear()) opts.year = "numeric";
  return `${d.toLocaleDateString([], opts)}, ${time}`;
}

function size(b: number) {
  if (b < 1024) return `${b} B`;
  if (b < 1048576) return `${Math.round(b / 1024)} KB`;
  return `${(b / 1048576).toFixed(1)} MB`;
}

function matches(g: Game, f: string) {
  if (f === "saves") return g.paths.length > 0;
  if (f === "backed") return g.versions > 0;
  return true;
}

function renderList() {
  shown = games.filter((g) => matches(g, filter) && (!query || g.name.toLowerCase().includes(query)));
  document.querySelectorAll<HTMLElement>("#tabs button").forEach((b) => {
    b.querySelector("span")!.textContent = String(games.filter((g) => matches(g, b.dataset.filter!)).length);
  });
  listEl.innerHTML = shown.length
    ? shown.map(rowHtml).join("")
    : `<div class="nothing">${query ? `Nothing matches “${esc(query)}”.` : "No games here yet. Rescan, or add one yourself."}</div>`;
}

function rowHtml(g: Game) {
  const cls = ["g", g.name === selected ? "sel" : "", g.paths.length ? "" : "nosave"].join(" ");
  const t = running.has(g.name)
    ? `<span class="t live">Playing</span>`
    : g.last_backup
      ? `<span class="t">${ago(g.last_backup)}</span>`
      : "";
  const src = covers ? coverSrc(g) : null;
  const cv = covers
    ? `<span class="cv">${src ? `<img data-art src="${src}" loading="lazy" decoding="async" alt="">` : ""}</span>`
    : "";
  return `<div class="${cls}" data-name="${esc(g.name)}">${cv}<span class="n">${esc(g.name)}</span>${t}</div>`;
}

function updateRow(g: Game) {
  const el = listEl.querySelector<HTMLElement>(`[data-name="${CSS.escape(g.name)}"]`);
  if (el) el.outerHTML = rowHtml(g);
}

function select(name: string | null, scroll = false) {
  selected = name;
  listEl.querySelector(".g.sel")?.classList.remove("sel");
  if (name) {
    const el = listEl.querySelector<HTMLElement>(`[data-name="${CSS.escape(name)}"]`);
    el?.classList.add("sel");
    if (scroll) el?.scrollIntoView({ block: "nearest" });
  }
  renderDetail();
  if (name) loadHistory(name);
}

listEl.addEventListener("mousedown", (e) => {
  const row = (e.target as HTMLElement).closest<HTMLElement>(".g");
  if (row) select(row.dataset.name!);
});

async function loadHistory(name: string) {
  const v = await call<Version[]>("list_backups", { name });
  if (!v) return;
  const before = JSON.stringify(history.get(name));
  history.set(name, v);
  if (selected === name && before !== JSON.stringify(v)) renderDetail();
}

function renderDetail() {
  const g = games.find((x) => x.name === selected);
  if (!g) {
    detailEl.innerHTML = `<div class="empty">${icon("gamepad", "big")}<p>Pick a game on the left to see its saves and backups.</p></div>`;
    return;
  }
  const versions = history.get(g.name) ?? [];
  const usable = g.paths.some((p) => p.enabled && p.exists);
  const where = STORE[g.source] ?? "Found by its save folder";

  const state = running.has(g.name)
    ? `<div class="state live">Playing now. It will be backed up when you quit.</div>`
    : g.last_backup
    ? `<div class="state ok">Backed up ${ago(g.last_backup)}</div>`
    : g.paths.length
      ? `<div class="state never">Not backed up yet</div>`
      : `<div class="state">No save folder known</div>`;

  const locs = g.paths
    .map(
      (p) => `<div class="loc ${p.enabled ? "" : "off"} ${p.exists ? "" : "gone"}" data-path="${esc(p.path)}">
        ${p.detected ? `<input type="checkbox" data-act="toggle" ${p.enabled ? "checked" : ""} title="Include in backups">` : ""}
        <code>${esc(p.path)}</code>
        ${p.exists ? "" : `<span class="by miss">not found</span>`}
        ${p.detected ? "" : `<span class="by">yours</span>`}
        ${p.path.includes("*") || !p.exists ? "" : `<button class="ib" data-act="reveal" title="Open in Explorer">${icon("folder")}</button>`}
        ${p.detected ? "" : `<button class="ib warn" data-act="del-path" title="Remove this location">${icon("close")}</button>`}
      </div>`,
    )
    .join("");

  const hints = [
    ...g.match_exes.map((e) => `<code>${esc(e)}</code>`),
    ...g.match_dirs.map((d) => `a folder called <code>${esc(d)}</code>`),
  ];
  const runner = g.exe
    ? `When you quit <code>${esc(g.exe)}</code>`
    : g.install_dir
      ? `When you quit anything running from <code>${esc(g.install_dir)}</code>`
      : hints.length
        ? `When you quit the game. Saveward recognises it by ${hints.slice(0, 3).join(" or ")}. If it misses, choose the program yourself.`
        : null;

  const hist = versions
    .map(
      (v) => `<li data-file="${esc(v.file)}" class="${v.pinned ? "pinned" : ""}">
        <div class="when"><b>${stamp(v.created)}</b>${v.pinned ? `<span class="pin">Kept</span>` : ""}${v.uploaded && cloud?.provider ? `<span class="up" title="Also in ${cloud.label}">${icon("cloudCheck")}</span>` : ""}
          <span class="meta">${v.files} files, ${size(v.size)}, on ${esc(v.pc)}${v.note && v.note !== "Manual" ? `. ${esc(v.note.replace(/^Auto: b/, "B"))}` : ""}</span></div>
        <div class="acts">
          <button class="btn" data-act="restore">${icon("undo")}Restore</button>
          <button class="ib ${v.pinned ? "on" : ""}" data-act="pin" title="${v.pinned ? "Stop keeping (may be cleaned up later)" : "Keep forever"}">${icon(v.pinned ? "bookmarked" : "bookmark")}</button>
          <button class="ib warn" data-act="del-backup" title="Delete backup">${icon("trash")}</button>
        </div>
      </li>`,
    )
    .join("");

  const hero = covers && g.steam_id;
  const src = coverSrc(g);
  const coverBox = covers
    ? `<button class="cover ${src ? "" : "blank"}" data-act="set-cover" title="Choose a cover image">
        ${src ? `<img data-art src="${src}" alt="">` : `${icon("image")}<span>Add cover</span>`}
        <span class="change">Change cover</span>
      </button>`
    : "";
  detailEl.innerHTML = `
    ${hero ? `<div class="hero"><img data-art src="${art(g.steam_id!, "library_hero")}" alt=""></div>` : ""}
    <div class="head ${hero ? "art" : ""} ${covers ? "with-cover" : ""}">
      ${coverBox}
      <div>
        <h1>${esc(g.name)}</h1>
        <div class="sub">${where}</div>
        ${state}
      </div>
      <button class="primary" data-act="backup" ${usable ? "" : "disabled"} title="Ctrl+B">${icon("save")}Back up now</button>
    </div>
    ${newerBanner(g.name)}

    <h2>Save locations</h2>
    <p>${g.paths.length ? "Everything here is copied into each backup. Untick a location to leave it out." : "Saveward doesn't know where this game saves. Add the folder and it will be backed up from now on."}</p>
    ${locs}
    <div class="more">
      <button class="btn" data-act="add-path">${icon("folderPlus")}Add folder</button>
      <button class="btn" data-act="add-file">${icon("file")}Add file</button>
    </div>

    <h2>When to back up automatically</h2>
    <p>${runner ?? "Choose the game's program so Saveward knows when you quit it."}</p>
    <div class="more">
      <button class="btn" data-act="set-exe">${g.exe ? "Change program" : "Choose program"}</button>
      ${g.exe ? `<button class="link" data-act="clear-exe">Use install folder instead</button>` : ""}
    </div>

    <h2>History</h2>
    ${versions.length ? `<p>Newest first. Restoring keeps a copy of your current save, so nothing is lost.</p><ol class="hist">${hist}</ol>` : `<p>Backups you make will show up here.</p>`}
    <div id="cloud-only"></div>

    <div class="more" style="margin-top:36px">
      <button class="link" data-act="open-backups">Open backup folder</button>
      ${g.cover ? `<button class="link" data-act="clear-cover">Remove my cover image</button>` : ""}
      <button class="link warn" data-act="remove">${g.source === "custom" ? "Remove this game" : "Hide this game"}</button>
    </div>`;
  fillCloudOnly(g.name);
}

async function fillCloudOnly(name: string, force = false) {
  const items = await cloudOnly(name, force);
  const box = detailEl.querySelector("#cloud-only");
  if (!box || selected !== name || !items?.length) return;
  box.innerHTML = `
    <h2>Only in ${esc(cloud!.label!)}</h2>
    <p>Made on your other PCs. Restoring downloads it first.</p>
    <ol class="hist cloud">${items
      .map(
        (v) => `<li>
          <div class="when"><b>${stamp(v.created)}</b><span class="meta">${size(v.size)}, on ${esc(v.pc)}</span></div>
          <div class="acts"><button class="btn" data-act="cloud-restore" data-remote="${esc(v.name)}">${icon("cloudDown")}Restore</button></div>
        </li>`,
      )
      .join("")}</ol>`;
}

async function refreshGame(name: string) {
  const row = await call<Game | null>("get_game", { name });
  const i = games.findIndex((g) => g.name === name);
  if (row && i >= 0) games[i] = row;
  else if (row) games.push(row);
  else if (i >= 0) games.splice(i, 1);
  if (row) updateRow(row);
  else renderList();
  await loadHistory(name);
  renderDetail();
}

async function backupSelected() {
  if (!selected) return;
  const name = selected;
  say(`Backing up ${name}…`);
  const r = await call<{ created: boolean }>("backup_game", { name, force: false });
  if (r) say(r.created ? `Backed up ${name}` : `${name} hasn't changed since the last backup`);
  await refreshGame(name);
}

detailEl.addEventListener("click", async (e) => {
  const t = (e.target as HTMLElement).closest<HTMLElement>("[data-act]");
  const act = t?.dataset.act;
  const g = games.find((x) => x.name === selected);
  if (!t || !act || !g) return;
  const name = g.name;
  const path = t.closest<HTMLElement>("[data-path]")?.dataset.path;
  const file = t.closest<HTMLElement>("[data-file]")?.dataset.file;

  switch (act) {
    case "backup":
      return backupSelected();
    case "open-backups":
      return revealItemInDir((await call<string>("game_backup_dir", { name }))!);
    case "reveal":
      return path && revealItemInDir(path);
    case "toggle":
      await call("toggle_path", { name, path, enabled: (t as HTMLInputElement).checked });
      break;
    case "del-path":
      await call("toggle_path", { name, path, enabled: false });
      break;
    case "add-path":
    case "add-file": {
      const picked = await open({ directory: act === "add-path" });
      if (!picked) return;
      await call("add_path", { name, path: picked });
      say("Location added");
      break;
    }
    case "set-exe": {
      const picked = await open({ filters: [{ name: "Programs", extensions: ["exe"] }] });
      if (!picked) return;
      await call("set_exe", { name, exe: picked });
      break;
    }
    case "clear-exe":
      await call("set_exe", { name, exe: null });
      break;
    case "set-cover": {
      const picked = await open({ filters: IMAGES });
      if (!picked) return;
      if ((await call("set_cover", { name, image: picked })) === null) say("Cover updated");
      break;
    }
    case "cloud-restore": {
      const ok = await ask(
        `Restore this backup of ${name} from ${cloud?.label}?\n\nClose the game first. Your current save is backed up and kept before anything is replaced.`,
        { title: "Restore from the cloud", kind: "warning", okLabel: "Download and restore" },
      );
      if (!ok) return;
      say(`Downloading and restoring ${name}…`);
      const n = await call<number>("cloud_restore", { name, remote: t.dataset.remote });
      if (n !== undefined) say(`Restored ${n} files for ${name} from ${cloud?.label}`);
      forgetCloud(name);
      await refreshCloud();
      break;
    }
    case "clear-cover":
      await call("set_cover", { name, image: null });
      say("Cover removed");
      break;
    case "remove": {
      const verb = g.source === "custom" ? "Remove" : "Hide";
      if (!(await ask(`${verb} ${name}? Its backups stay on disk.`, { title: verb, kind: "warning" }))) return;
      await call("remove_game", { name });
      games = games.filter((x) => x.name !== name);
      renderList();
      select(null);
      say(`${name} hidden`);
      return;
    }
    case "restore": {
      const ok = await ask(
        `Restore this backup of ${name}?\n\nClose the game first. Your current save is backed up and kept before anything is replaced.`,
        { title: "Restore backup", kind: "warning", okLabel: "Restore" },
      );
      if (!ok) return;
      say(`Restoring ${name}…`);
      const n = await call<number>("restore_backup", { name, file });
      if (n !== undefined) say(`Restored ${n} files for ${name}`);
      break;
    }
    case "pin":
      await call("pin_backup", { name, file, pinned: !t.closest("li")!.classList.contains("pinned") });
      say(t.closest("li")!.classList.contains("pinned") ? "Backup can be cleaned up again" : "Backup will be kept forever");
      break;
    case "del-backup":
      if (!(await ask("Delete this backup? This can't be undone.", { title: "Delete backup", kind: "warning", okLabel: "Delete" })))
        return;
      await call("delete_backup", { name, file });
      say("Backup deleted");
      break;
  }
  await refreshGame(name);
});

const allBtn = $<HTMLButtonElement>("#btn-all");
function setBatch(running: boolean) {
  allBtn.disabled = running;
  allBtn.innerHTML = `${icon("save")}${running ? "Backing up…" : "Back up all"}`;
}
setBatch(false);
allBtn.onclick = async () => {
  setBatch(true);
  say("Backing up all games…");
  if ((await call("backup_all_games")) === undefined) setBatch(false);
};

$("#btn-add").insertAdjacentHTML("afterbegin", icon("plus"));
$("#btn-scan").insertAdjacentHTML("afterbegin", icon("reload"));
$("#btn-settings").insertAdjacentHTML("afterbegin", icon("sliders"));

async function scan() {
  const btn = $<HTMLButtonElement>("#btn-scan");
  btn.disabled = true;
  say(games.length ? "Looking for games…" : "First run: downloading the save database and looking for games…");
  const r = await call<Game[]>("scan_games", { refreshDb: false });
  btn.disabled = false;
  if (!r) return;
  games = r;
  renderList();
  renderDetail();
  say(`Found ${r.filter((g) => g.paths.length).length} games with saves`);
}

$("#btn-scan").onclick = scan;

const search = $<HTMLInputElement>("#search");
search.oninput = () => {
  query = search.value.trim().toLowerCase();
  renderList();
};

$("#tabs").addEventListener("click", (e) => {
  const b = (e.target as HTMLElement).closest<HTMLElement>("button");
  if (!b) return;
  document.querySelectorAll("#tabs button").forEach((x) => x.classList.toggle("on", x === b));
  filter = b.dataset.filter!;
  renderList();
});

document.addEventListener("keydown", (e) => {
  if (document.querySelector("dialog[open]")) return;
  if ((e.ctrlKey && e.key === "f") || (e.key === "/" && document.activeElement !== search)) {
    e.preventDefault();
    search.focus();
    search.select();
  } else if (e.ctrlKey && e.key === "b") {
    e.preventDefault();
    backupSelected();
  } else if (e.key === "ArrowDown" || e.key === "ArrowUp") {
    if (!shown.length) return;
    e.preventDefault();
    const i = shown.findIndex((g) => g.name === selected);
    const next = e.key === "ArrowDown" ? Math.min(i + 1, shown.length - 1) : Math.max(i - 1, 0);
    select(shown[next].name, true);
  } else if (e.key === "Escape" && document.activeElement === search) {
    search.value = "";
    query = "";
    renderList();
  }
});

document.querySelectorAll<HTMLElement>("[data-pick]").forEach((b) => {
  b.onclick = async () => {
    const field = b.dataset.pick!;
    const input = b.closest("form")!.querySelector<HTMLInputElement>(`[name="${field}"]`)!;
    const picked =
      field === "exe"
        ? await open({ filters: [{ name: "Programs", extensions: ["exe"] }] })
        : field === "cover"
          ? await open({ filters: IMAGES })
          : await open({ directory: true });
    if (picked) input.value = picked as string;
  };
});

const addDlg = $<HTMLDialogElement>("#dlg-add");
$("#btn-add").onclick = () => {
  addDlg.querySelector("form")!.reset();
  addDlg.showModal();
};
addDlg.onclose = async () => {
  if (addDlg.returnValue !== "ok") return;
  const f = new FormData(addDlg.querySelector("form")!);
  const name = String(f.get("name")).trim();
  const ok = await call("add_custom_game", { name, paths: [String(f.get("path"))], exe: String(f.get("exe")) || null });
  if (ok === undefined) return;
  const cover = String(f.get("cover") ?? "");
  if (cover) await call("set_cover", { name, image: cover });
  games = (await call<Game[]>("get_games")) ?? games;
  renderList();
  select(name, true);
  say(`${name} added`);
};

const setDlg = $<HTMLDialogElement>("#dlg-settings");
const field = (n: string) => setDlg.querySelector("form")!.elements.namedItem(n) as HTMLInputElement;
$("#btn-settings").onclick = async () => {
  const s = await call<Settings>("get_settings");
  if (!s) return;
  field("backup_dir").value = s.backup_dir;
  field("versions_to_keep").value = String(s.versions_to_keep);
  field("auto_backup").checked = s.auto_backup;
  field("show_covers").checked = s.show_covers;
  field("start_with_windows").checked = s.start_with_windows;
  field("show_popup").checked = s.show_popup;
  setDlg.showModal();
};
setDlg.onclose = async () => {
  if (setDlg.returnValue !== "ok") return;
  const ok = await call("update_settings", {
    backupDir: field("backup_dir").value,
    versionsToKeep: Number(field("versions_to_keep").value) || 10,
    autoBackup: field("auto_backup").checked,
    showCovers: field("show_covers").checked,
    startWithWindows: field("start_with_windows").checked,
    showPopup: field("show_popup").checked,
  });
  if (ok === undefined) return;
  covers = field("show_covers").checked;
  document.body.classList.toggle("covers", covers);
  say("Settings saved");
  history.clear();
  games = (await call<Game[]>("get_games")) ?? games;
  renderList();
  select(selected);
};

(async () => {
  if (import.meta.env.DEV && !("__TAURI_INTERNALS__" in window && (window as any).__TAURI_INTERNALS__.invoke)) {
    (await import("./dev-mock")).installMock();
  }
  covers = (await call<Settings>("get_settings"))?.show_covers ?? true;
  document.body.classList.toggle("covers", covers);
  games = (await call<Game[]>("get_games")) ?? [];
  running = new Set((await call<string[]>("get_running")) ?? []);
  renderList();

  setBatch((await call<boolean>("batch_running")) ?? false);
  listen<boolean>("batch", (e) => setBatch(e.payload));
  listen<string>("batch-summary", (e) => say(e.payload));

  listen<string>("backup-done", (e) => {
    if (games.some((g) => g.name === e.payload)) refreshGame(e.payload);
  });
  listen<string[]>("running", (e) => {
    const changed = new Set([...running, ...e.payload]);
    running = new Set(e.payload);
    for (const name of changed) {
      const g = games.find((x) => x.name === name);
      if (g) updateRow(g);
    }
    if (selected && changed.has(selected)) renderDetail();
  });

  let lastCloud = "";
  await initCloud({
    esc,
    ago,
    say,
    onChange: () => {
      const now = JSON.stringify([cloud?.provider, cloud?.newer, cloud?.pending]);
      if (now === lastCloud) return;
      lastCloud = now;
      if (selected) {
        loadHistory(selected);
        renderDetail();
      }
    },
  });

  const first = shown[0]?.name ?? null;
  select(first);
  await getCurrentWindow().show();
  if (!games.length) await scan();
})();
