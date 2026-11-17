import { mockIPC, mockWindows } from "@tauri-apps/api/mocks";

const h = (n: number) => new Date(Date.now() - n * 3600_000).toISOString();
const p = (path: string, extra: Partial<{ detected: boolean; enabled: boolean; exists: boolean }> = {}) => ({
  path, detected: true, enabled: true, exists: true, ...extra,
});
const U = "C:\\Users\\Player\\";
const games = [
  { name: "Cyberpunk 2077", steam_id: 1091500, source: "steam", install_dir: "D:\\Steam\\steamapps\\common\\Cyberpunk 2077", exe: null, paths: [p(U + "Saved Games\\CD Projekt Red\\Cyberpunk 2077"), p(U + "AppData\\Local\\CD Projekt Red\\Cyberpunk 2077", { enabled: false })], last_backup: h(2), versions: 4 },
  { name: "Elden Ring", steam_id: 1245620, source: "steam", install_dir: "D:\\Steam\\steamapps\\common\\ELDEN RING", exe: null, paths: [p(U + "AppData\\Roaming\\EldenRing\\*"), p(U + "AppData\\Roaming\\EldenRing\\GraphicsConfig.xml")], last_backup: h(30), versions: 9 },
  { name: "Hollow Knight", steam_id: 367520, source: "found", install_dir: null, exe: null, paths: [p(U + "AppData\\LocalLow\\Team Cherry\\Hollow Knight\\user*.dat")], last_backup: null, versions: 0 },
  { name: "Lossless Scaling", steam_id: 993090, source: "steam", install_dir: "D:\\Steam\\steamapps\\common\\Lossless Scaling", exe: null, paths: [], last_backup: null, versions: 0 },
  { name: "Minecraft: Java Edition", source: "found", install_dir: null, exe: null, paths: [p(U + "AppData\\Roaming\\.minecraft\\saves"), p(U + "AppData\\Roaming\\.minecraft\\options.txt")], last_backup: h(0.2), versions: 10 },
  { name: "Project Zomboid", steam_id: 108600, source: "found", install_dir: null, exe: null, paths: [p(U + "Zomboid")], last_backup: h(200), versions: 2 },
  { name: "Raft", steam_id: 648800, source: "found", install_dir: null, exe: null, paths: [p(U + "AppData\\LocalLow\\Redbeet Interactive\\Raft\\User\\User_*\\World\\*\\*\\*.rgd")], last_backup: null, versions: 0 },
  { name: "Stardew Valley", steam_id: 413150, source: "steam", install_dir: "D:\\Steam\\steamapps\\common\\Stardew Valley", exe: null, paths: [p(U + "AppData\\Roaming\\StardewValley\\Saves")], last_backup: h(5), versions: 6 },
  { name: "Terraria", steam_id: 105600, source: "found", install_dir: null, exe: null, paths: [p(U + "Documents\\My Games\\Terraria")], last_backup: null, versions: 0 },
  { name: "Old Emulator Saves", source: "custom", install_dir: "E:\\Emu", exe: "E:\\Emu\\pcsx2.exe", paths: [p("E:\\Emu\\memcards", { detected: false }), p("E:\\Emu\\sstates", { detected: false, exists: false })], last_backup: h(70), versions: 1 },
];
const versions = [
  { file: "a.zip", created: h(2), pc: "DESKTOP-GAMING", size: 48_200_000, files: 214, pinned: false, note: "Auto: on exit" },
  { file: "b.zip", created: h(26), pc: "DESKTOP-GAMING", size: 47_900_000, files: 212, pinned: true, note: "Auto: before restore" },
  { file: "c.zip", created: h(50), pc: "LAPTOP", size: 45_100_000, files: 205, pinned: false, note: "Manual" },
  { file: "d.zip", created: h(170), pc: "DESKTOP-GAMING", size: 39_000_000, files: 190, pinned: false, note: "Manual" },
];

for (const g of games as any[]) {
  g.match_dirs ??= g.source === "found" ? [g.name.toLowerCase()] : [];
  g.match_exes ??= [];
  g.cover ??= null;
}
for (const v of versions as any[]) v.uploaded = v.file !== "d.zip";

const connected = !location.search.includes("cloud=off");
const cloudStatus = {
  provider: connected ? "google" : null,
  label: connected ? "Google Drive" : null,
  pending: 0,
  busy: false,
  last_error: null,
  last_sync: h(0.05),
  newer: connected ? { "Elden Ring": { name: "x__LAPTOP.zip", created: h(1), pc: "LAPTOP", size: 51_000_000, local: false } } : {},
  ready: { google: true, onedrive: false, dropbox: false },
};

export function installMock() {
  mockWindows("main");
  mockIPC((cmd, args: any) => {
    switch (cmd) {
      case "cloud_status":
        return cloudStatus;
      case "batch_running":
        return false;
      case "cloud_list":
        return args.name === "Elden Ring"
          ? [
              { name: "x__LAPTOP.zip", created: h(1), pc: "LAPTOP", size: 51_000_000, local: false },
              { name: "y__LAPTOP.zip", created: h(80), pc: "LAPTOP", size: 50_100_000, local: false },
            ]
          : [];
      case "get_games":
      case "scan_games":
        return games;
      case "get_game":
        return games.find((g) => g.name === args.name) ?? null;
      case "list_backups":
        return games.find((g) => g.name === args.name)?.versions ? versions.slice(0, Math.min(4, games.find((g) => g.name === args.name)!.versions)) : [];
      case "get_running":
        return ["Elden Ring"];
      case "get_settings":
        return { backup_dir: U + "AppData\\Roaming\\Saveward\\backups", versions_to_keep: 10, auto_backup: true, show_covers: true, start_with_windows: false };
      default:
        return null;
    }
  });
}
