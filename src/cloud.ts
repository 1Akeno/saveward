import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { openUrl } from "@tauri-apps/plugin-opener";
import { icon } from "./icons";

export type CloudVersion = { name: string; created: string; pc: string; size: number; local: boolean };
export type CloudStatus = {
  provider: string | null;
  label: string | null;
  pending: number;
  busy: boolean;
  last_error: string | null;
  last_sync: string | null;
  newer: Record<string, CloudVersion>;
  ready: Record<string, boolean>;
};

type Helpers = {
  esc: (s: string) => string;
  ago: (iso: string) => string;
  say: (msg: string, err?: boolean) => void;
  onChange: () => void;
};

const PROVIDERS = [
  { id: "google", label: "Google Drive" },
  { id: "onedrive", label: "OneDrive" },
  { id: "dropbox", label: "Dropbox" },
] as const;

const SETUP: Record<string, { steps: string[]; fields: { key: "app_id" | "app_secret"; label: string }[] }> = {
  google: {
    steps: [
      `Open the <a data-url="https://console.cloud.google.com/projectcreate">Google Cloud console</a> and create a project (any name).`,
      `Turn on the <a data-url="https://console.cloud.google.com/apis/library/drive.googleapis.com">Google Drive API</a> for it.`,
      `Set up the <a data-url="https://console.cloud.google.com/auth/overview">sign-in screen</a>: choose External, then under Audience click Publish app. Otherwise Google signs you out every 7 days.`,
      `Under <a data-url="https://console.cloud.google.com/auth/clients">Clients</a>, create a client of type Desktop app.`,
      `Paste its Client ID and Client secret below.`,
    ],
    fields: [
      { key: "app_id", label: "Client ID" },
      { key: "app_secret", label: "Client secret" },
    ],
  },
  onedrive: {
    steps: [
      `Open <a data-url="https://portal.azure.com/#view/Microsoft_AAD_RegisteredApps/ApplicationsListBlade">App registrations</a> and click New registration (any name).`,
      `Under Supported account types, choose the option that includes personal Microsoft accounts.`,
      `Under Redirect URI, pick Public client/native and enter <code>http://localhost</code>.`,
      `In API permissions, add Microsoft Graph, Delegated, <code>Files.ReadWrite.AppFolder</code>.`,
      `Paste the Application (client) ID below.`,
    ],
    fields: [{ key: "app_id", label: "Application (client) ID" }],
  },
  dropbox: {
    steps: [
      `Open the <a data-url="https://www.dropbox.com/developers/apps/create">Dropbox App Console</a>: choose Scoped access, then App folder, and give it a name.`,
      `On the Permissions tab, tick <code>files.content.write</code> and <code>files.content.read</code>, then Submit.`,
      `On the Settings tab, add the redirect URI <code>http://127.0.0.1:47863/callback</code>.`,
      `Paste the App key below.`,
    ],
    fields: [{ key: "app_id", label: "App key" }],
  },
};

export let cloud: CloudStatus | null = null;
let h: Helpers;
let pick = "google";
let connecting = false;
let showOthers = false;

const G_LOGO = `<svg class="glogo" viewBox="0 0 48 48" aria-hidden="true"><path fill="#EA4335" d="M24 9.5c3.54 0 6.71 1.22 9.21 3.6l6.85-6.85C35.9 2.38 30.47 0 24 0 14.62 0 6.51 5.38 2.56 13.22l7.98 6.19C12.43 13.72 17.74 9.5 24 9.5z"/><path fill="#4285F4" d="M46.98 24.55c0-1.57-.15-3.09-.38-4.55H24v9.02h12.94c-.58 2.96-2.26 5.48-4.78 7.18l7.73 6c4.51-4.18 7.09-10.36 7.09-17.65z"/><path fill="#FBBC05" d="M10.53 28.59c-.48-1.45-.76-2.99-.76-4.59s.27-3.14.76-4.59l-7.98-6.19C.92 16.46 0 20.12 0 24c0 3.88.92 7.54 2.56 10.78l7.97-6.19z"/><path fill="#34A853" d="M24 48c6.48 0 11.93-2.13 15.89-5.81l-7.73-6c-2.15 1.45-4.92 2.3-8.16 2.3-6.26 0-11.57-4.22-13.47-9.91l-7.98 6.19C6.51 42.62 14.62 48 24 48z"/></svg>`;
let dialogError = "";
const lists = new Map<string, { at: number; items: CloudVersion[] }>();

const dlg = () => document.querySelector<HTMLDialogElement>("#dlg-cloud")!;
const body = () => document.querySelector<HTMLElement>("#cloud-body")!;
const line = () => document.querySelector<HTMLButtonElement>("#cloud-line")!;

export async function initCloud(helpers: Helpers) {
  h = helpers;
  line().onclick = openDialog;
  line().title = "Cloud backup";
  body().addEventListener("click", onDialogClick);
  dlg().addEventListener("close", () => {
    if (connecting) invoke("cloud_cancel_sign_in");
  });
  listen("cloud", refresh);
  await refresh();
  setInterval(renderLine, 60_000);
}

export async function refresh() {
  try {
    cloud = await invoke<CloudStatus>("cloud_status");
  } catch {
    return;
  }
  renderLine();
  if (dlg().open && !connecting) renderDialog();
  h.onChange();
}

function renderLine() {
  const c = cloud;
  const el = line();
  el.classList.remove("bad", "off");
  if (!c?.provider) {
    el.classList.add("off");
    el.innerHTML = `${icon("cloud")}<span>Keep copies in the cloud</span>`;
  } else if (c.last_error) {
    el.classList.add("bad");
    el.innerHTML = `${icon("cloudWarn")}<span>Cloud sync stopped. Click to see why.</span>`;
  } else if (c.busy || c.pending) {
    el.innerHTML = `${icon("cloudUp")}<span>${c.pending ? `Uploading ${c.pending} backup${c.pending > 1 ? "s" : ""} to ${c.label}` : `Checking ${c.label}`}…</span>`;
  } else {
    el.innerHTML = `${icon("cloudCheck")}<span>Synced with ${c.label}${c.last_sync ? ` ${h.ago(c.last_sync)}` : ""}</span>`;
  }
}

function openDialog() {
  dialogError = "";
  showOthers = false;
  if (cloud?.provider) pick = cloud.provider;
  renderDialog();
  dlg().showModal();
}

function renderDialog() {
  const c = cloud;
  if (connecting) {
    const label = PROVIDERS.find((p) => p.id === pick)!.label;
    body().innerHTML = `
      <h3>Signing in to ${label}</h3>
      <p>Your browser opened the ${label} sign-in page. Finish there, and this window updates by itself.</p>
      <menu><button data-c="cancel">Cancel</button></menu>`;
    return;
  }
  if (c?.provider) {
    body().innerHTML = `
      <h3>Cloud backup</h3>
      <p>Connected to <b>${c.label}</b>. New backups upload by themselves, and backups made on your other PCs appear in each game's history.</p>
      <p class="muted">${c.last_sync ? `Last synced ${h.ago(c.last_sync)}.` : "Not synced yet."} ${c.pending ? `${c.pending} backup${c.pending > 1 ? "s" : ""} waiting to upload.` : "Everything is uploaded."}</p>
      ${c.last_error ? `<p class="err">${h.esc(c.last_error)}</p>` : ""}
      <menu>
        <button data-c="disconnect" class="left">Disconnect</button>
        <button data-c="close">Close</button>
        <button data-c="sync" class="primary">Sync now</button>
      </menu>`;
    return;
  }
  if (!showOthers && c?.ready.google) {
    body().innerHTML = `
      <h3>Back up to Google Drive</h3>
      <p>Keep copies of your saves in your Google Drive and restore them on any PC. Saveward can only open the folder it creates there, never your other files.</p>
      <button class="gsi" data-c="google">${G_LOGO}<span>Sign in with Google</span></button>
      ${dialogError ? `<p class="err">${h.esc(dialogError)}</p>` : ""}
      <menu>
        <button data-c="others" class="link left">Use OneDrive or Dropbox instead</button>
        <button data-c="close">Not now</button>
      </menu>`;
    return;
  }
  const setup = SETUP[pick];
  const ready = c?.ready[pick];
  const label = PROVIDERS.find((p) => p.id === pick)!.label;
  body().innerHTML = `
    <h3>Cloud backup</h3>
    <p>Pick where to keep copies of your backups. Saveward can only open its own folder there, never your other files.</p>
    <div class="providers">
      ${PROVIDERS.map(
        (p) => `<label class="${p.id === pick ? "on" : ""}"><input type="radio" name="prov" value="${p.id}" ${p.id === pick ? "checked" : ""}> ${p.label}</label>`,
      ).join("")}
    </div>
    ${
      ready
        ? `<p>${label} is ready to connect. Click Connect and sign in with your own account.</p>`
        : `<div class="setup">
            <p>First, a one-time setup. ${label} asks every app that uses it to register, and this gives you a key to paste here.</p>
            <ol>${setup.steps.map((s) => `<li>${s}</li>`).join("")}</ol>
            ${setup.fields.map((f) => `<label>${f.label}<input name="${f.key}" spellcheck="false" autocomplete="off"></label>`).join("")}
          </div>`
    }
    ${dialogError ? `<p class="err">${h.esc(dialogError)}</p>` : ""}
    <menu><button data-c="close">Cancel</button><button data-c="connect" class="primary">Connect to ${label}</button></menu>`;
  body().querySelectorAll<HTMLInputElement>("input[name=prov]").forEach((r) => {
    r.onchange = () => {
      pick = r.value;
      dialogError = "";
      renderDialog();
    };
  });
}

async function onDialogClick(e: Event) {
  const t = e.target as HTMLElement;
  const link = t.closest<HTMLElement>("[data-url]");
  if (link) {
    e.preventDefault();
    openUrl(link.dataset.url!);
    return;
  }
  const c = t.closest<HTMLElement>("[data-c]")?.dataset.c;
  if (!c) return;
  if (c === "close") dlg().close();
  if (c === "others") {
    showOthers = true;
    pick = "onedrive";
    renderDialog();
    return;
  }
  if (c === "google") pick = "google";
  if (c === "cancel") {
    await invoke("cloud_cancel_sign_in");
  }
  if (c === "sync") {
    await invoke("cloud_sync_now");
    h.say("Syncing…");
    dlg().close();
  }
  if (c === "disconnect") {
    await invoke("cloud_disconnect");
    lists.clear();
    h.say("Cloud disconnected. Your backups on this PC are untouched.");
    await refresh();
  }
  if (c === "connect" || c === "google") {
    const val = (k: string) => body().querySelector<HTMLInputElement>(`input[name=${k}]`)?.value ?? "";
    connecting = true;
    dialogError = "";
    renderDialog();
    try {
      await invoke("cloud_connect", { provider: pick, appId: val("app_id"), appSecret: val("app_secret") });
      connecting = false;
      h.say(`Connected to ${PROVIDERS.find((p) => p.id === pick)!.label}. Uploading your newest backups.`);
      await refresh();
    } catch (err) {
      connecting = false;
      dialogError = String(err);
      if (dlg().open) renderDialog();
    }
  }
}

export function newerBanner(game: string) {
  const v = cloud?.newer[game];
  if (!v) return "";
  return `<div class="banner">${icon("cloudDown")}
    <div><b>Newer save from ${h.esc(v.pc)}</b><br>Made ${h.ago(v.created)}, it's in ${cloud!.label}.</div>
    <button class="btn" data-act="cloud-restore" data-remote="${h.esc(v.name)}">Restore it</button>
  </div>`;
}

export async function cloudOnly(game: string, force = false): Promise<CloudVersion[] | null> {
  if (!cloud?.provider) return null;
  const hit = lists.get(game);
  if (hit && !force && Date.now() - hit.at < 120_000) return hit.items.filter((v) => !v.local);
  try {
    const items = await invoke<CloudVersion[]>("cloud_list", { name: game });
    lists.set(game, { at: Date.now(), items });
    return items.filter((v) => !v.local);
  } catch {
    return null;
  }
}

export function forget(game: string) {
  lists.delete(game);
}
