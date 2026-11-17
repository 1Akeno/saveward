use base64::Engine;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub const PORT: u16 = 47863;
type Res<T> = Result<T, String>;

pub static CANCEL: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Provider {
    Google,
    OneDrive,
    Dropbox,
}

impl Provider {
    pub fn parse(s: &str) -> Res<Self> {
        match s {
            "google" => Ok(Self::Google),
            "onedrive" => Ok(Self::OneDrive),
            "dropbox" => Ok(Self::Dropbox),
            _ => Err(format!("Unknown cloud: {s}")),
        }
    }
    pub fn id(self) -> &'static str {
        match self {
            Self::Google => "google",
            Self::OneDrive => "onedrive",
            Self::Dropbox => "dropbox",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Google => "Google Drive",
            Self::OneDrive => "OneDrive",
            Self::Dropbox => "Dropbox",
        }
    }
    fn redirect(self) -> String {
        let host = if self == Self::OneDrive {
            "localhost"
        } else {
            "127.0.0.1"
        };
        format!("http://{host}:{PORT}/callback")
    }
    fn token_url(self) -> &'static str {
        match self {
            Self::Google => "https://oauth2.googleapis.com/token",
            Self::OneDrive => "https://login.microsoftonline.com/common/oauth2/v2.0/token",
            Self::Dropbox => "https://api.dropboxapi.com/oauth2/token",
        }
    }
}

#[derive(Clone)]
pub struct AppCreds {
    pub id: String,
    pub secret: String,
}

fn keyring(p: Provider) -> Res<keyring::Entry> {
    keyring::Entry::new("Saveward", p.id()).map_err(|e| e.to_string())
}

fn saved_token(p: Provider) -> Option<String> {
    if let Ok(t) = keyring(p).and_then(|e| e.get_password().map_err(|e| e.to_string())) {
        return Some(t);
    }
    let old = keyring::Entry::new("SaveVault", p.id()).ok()?;
    let t = old.get_password().ok()?;
    if keyring(p)
        .and_then(|e| e.set_password(&t).map_err(|e| e.to_string()))
        .is_ok()
    {
        let _ = old.delete_credential();
    }
    Some(t)
}

pub fn forget(p: Provider) {
    if let Ok(e) = keyring(p) {
        let _ = e.delete_credential();
    }
}

fn http_err(e: ureq::Error) -> String {
    match e {
        ureq::Error::Status(code, resp) => {
            let body = resp.into_string().unwrap_or_default();
            let short: String = body.chars().take(300).collect();
            format!("server said {code}: {short}")
        }
        ureq::Error::Transport(t) => format!("network problem: {t}"),
    }
}

fn status(e: &ureq::Error) -> Option<u16> {
    match e {
        ureq::Error::Status(code, _) => Some(*code),
        _ => None,
    }
}

fn read_json(resp: ureq::Response) -> Res<Value> {
    let s = resp.into_string().map_err(|e| e.to_string())?;
    serde_json::from_str(&s).map_err(|e| e.to_string())
}

fn read_bytes(resp: ureq::Response) -> Res<Vec<u8>> {
    let mut buf = Vec::new();
    resp.into_reader()
        .take(4 * 1024 * 1024 * 1024)
        .read_to_end(&mut buf)
        .map_err(|e| e.to_string())?;
    Ok(buf)
}

fn rand_b64(n: usize) -> String {
    let mut b = vec![0u8; n];
    getrandom::getrandom(&mut b).expect("random bytes");
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b)
}

pub fn pct(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn ascii_json(v: &Value) -> String {
    v.to_string()
        .chars()
        .map(|c| {
            if c.is_ascii() {
                c.to_string()
            } else {
                c.encode_utf16(&mut [0; 2])
                    .iter()
                    .map(|u| format!("\\u{u:04x}"))
                    .collect()
            }
        })
        .collect()
}

struct Counting<'a> {
    data: &'a [u8],
    pos: usize,
    base: u64,
    total: u64,
    on: &'a dyn Fn(u64, u64),
}

impl Read for Counting<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = buf.len().min(64 * 1024).min(self.data.len() - self.pos);
        buf[..n].copy_from_slice(&self.data[self.pos..self.pos + n]);
        self.pos += n;
        (self.on)(self.base + self.pos as u64, self.total);
        Ok(n)
    }
}

fn counting<'a>(data: &'a [u8], base: u64, total: u64, on: &'a dyn Fn(u64, u64)) -> Counting<'a> {
    Counting {
        data,
        pos: 0,
        base,
        total,
        on,
    }
}

pub fn sign_in(p: Provider, creds: &AppCreds, open_url: impl Fn(&str)) -> Res<String> {
    CANCEL.store(false, std::sync::atomic::Ordering::Relaxed);
    let listener = TcpListener::bind(("127.0.0.1", PORT))
        .map_err(|e| format!("Couldn't listen on port {PORT} for the sign-in reply: {e}"))?;
    let verifier = rand_b64(48);
    let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(Sha256::digest(verifier.as_bytes()));
    let state = rand_b64(16);
    let redirect = p.redirect();

    let url = match p {
        Provider::Google => format!(
            "https://accounts.google.com/o/oauth2/v2/auth?client_id={}&redirect_uri={}&response_type=code&scope={}&code_challenge={challenge}&code_challenge_method=S256&state={state}&access_type=offline&prompt=consent",
            pct(&creds.id),
            pct(&redirect),
            pct("https://www.googleapis.com/auth/drive.file")
        ),
        Provider::OneDrive => format!(
            "https://login.microsoftonline.com/common/oauth2/v2.0/authorize?client_id={}&redirect_uri={}&response_type=code&scope={}&code_challenge={challenge}&code_challenge_method=S256&state={state}",
            pct(&creds.id),
            pct(&redirect),
            pct("Files.ReadWrite.AppFolder offline_access")
        ),
        Provider::Dropbox => format!(
            "https://www.dropbox.com/oauth2/authorize?client_id={}&redirect_uri={}&response_type=code&code_challenge={challenge}&code_challenge_method=S256&state={state}&token_access_type=offline",
            pct(&creds.id),
            pct(&redirect)
        ),
    };
    open_url(&url);

    let code = wait_for_code(&listener, &state)?;
    let mut form: Vec<(&str, &str)> = vec![
        ("grant_type", "authorization_code"),
        ("code", &code),
        ("redirect_uri", &redirect),
        ("client_id", &creds.id),
        ("code_verifier", &verifier),
    ];
    if p == Provider::Google {
        form.push(("client_secret", &creds.secret));
    }
    let v = read_json(
        ureq::post(p.token_url())
            .send_form(&form)
            .map_err(http_err)?,
    )?;
    let refresh = v["refresh_token"]
        .as_str()
        .ok_or("The provider didn't return a refresh token.")?;
    keyring(p)?
        .set_password(refresh)
        .map_err(|e| e.to_string())?;
    Ok(refresh.to_string())
}

fn wait_for_code(listener: &TcpListener, state: &str) -> Res<String> {
    listener.set_nonblocking(true).map_err(|e| e.to_string())?;
    let deadline = Instant::now() + Duration::from_secs(300);
    loop {
        match listener.accept() {
            Ok((mut stream, _)) => {
                let _ = stream.set_nonblocking(false);
                let mut line = String::new();
                BufReader::new(&stream)
                    .read_line(&mut line)
                    .map_err(|e| e.to_string())?;
                let query = line
                    .split_whitespace()
                    .nth(1)
                    .and_then(|p| p.split_once('?'))
                    .map(|(_, q)| q)
                    .unwrap_or("");
                let params: HashMap<String, String> = query
                    .split('&')
                    .filter_map(|kv| kv.split_once('='))
                    .map(|(k, v)| (k.to_string(), unpct(v)))
                    .collect();
                if !line.contains("/callback") {
                    let _ =
                        stream.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n");
                    continue;
                }
                let ok = params.get("state").map(String::as_str) == Some(state)
                    && params.contains_key("code");
                let msg = if ok {
                    "Signed in. You can close this tab and go back to Saveward."
                } else {
                    "Sign-in didn't complete. Go back to Saveward and try again."
                };
                let body = format!(
                    "<!doctype html><meta charset=utf-8><title>Saveward</title><body style=\"font:16px system-ui;padding:48px\"><p>{msg}</p>"
                );
                let _ = write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                if ok {
                    return Ok(params["code"].clone());
                }
                let why = params
                    .get("error_description")
                    .or(params.get("error"))
                    .cloned()
                    .unwrap_or_else(|| "cancelled".into());
                return Err(format!("Sign-in failed: {why}"));
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                if CANCEL.load(std::sync::atomic::Ordering::Relaxed) {
                    return Err("Sign-in cancelled.".into());
                }
                if Instant::now() > deadline {
                    return Err("Sign-in timed out after 5 minutes.".into());
                }
                std::thread::sleep(Duration::from_millis(150));
            }
            Err(e) => return Err(e.to_string()),
        }
    }
}

fn unpct(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'%' if i + 2 < b.len() => {
                if let Ok(v) =
                    u8::from_str_radix(std::str::from_utf8(&b[i + 1..i + 3]).unwrap_or("zz"), 16)
                {
                    out.push(v);
                    i += 3;
                    continue;
                }
                out.push(b'%');
            }
            b'+' => out.push(b' '),
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub struct RemoteFile {
    pub name: String,
    pub id: String,
    pub size: u64,
}

pub struct Cloud {
    pub provider: Provider,
    creds: AppCreds,
    refresh: Mutex<String>,
    access: Mutex<Option<(String, Instant)>>,
    folders: Mutex<HashMap<String, String>>,
}

impl Cloud {
    pub fn load(p: Provider, creds: AppCreds) -> Res<Self> {
        let refresh = saved_token(p).ok_or_else(|| format!("Not signed in to {}.", p.label()))?;
        Ok(Self::new(p, creds, refresh))
    }

    pub fn new(p: Provider, creds: AppCreds, refresh: String) -> Self {
        Self {
            provider: p,
            creds,
            refresh: Mutex::new(refresh),
            access: Mutex::new(None),
            folders: Mutex::new(HashMap::new()),
        }
    }

    fn token(&self) -> Res<String> {
        if let Some((t, until)) = self.access.lock().unwrap().as_ref() {
            if Instant::now() < *until {
                return Ok(t.clone());
            }
        }
        let refresh = self.refresh.lock().unwrap().clone();
        let mut form: Vec<(&str, &str)> = vec![
            ("grant_type", "refresh_token"),
            ("refresh_token", &refresh),
            ("client_id", &self.creds.id),
        ];
        if self.provider == Provider::Google {
            form.push(("client_secret", &self.creds.secret));
        }
        if self.provider == Provider::OneDrive {
            form.push(("scope", "Files.ReadWrite.AppFolder offline_access"));
        }
        let v = match ureq::post(self.provider.token_url()).send_form(&form) {
            Ok(r) => read_json(r)?,
            Err(e) if matches!(status(&e), Some(400 | 401)) => {
                return Err(format!(
                    "{} sign-in expired. Connect again in Cloud settings.",
                    self.provider.label()
                ));
            }
            Err(e) => return Err(http_err(e)),
        };
        let access = v["access_token"]
            .as_str()
            .ok_or("No access token in reply.")?
            .to_string();
        let secs = v["expires_in"].as_u64().unwrap_or(3000).saturating_sub(120);
        if let Some(new) = v["refresh_token"].as_str() {
            if new != refresh {
                *self.refresh.lock().unwrap() = new.to_string();
                if let Ok(k) = keyring(self.provider) {
                    let _ = k.set_password(new);
                }
            }
        }
        *self.access.lock().unwrap() =
            Some((access.clone(), Instant::now() + Duration::from_secs(secs)));
        Ok(access)
    }

    fn auth(&self) -> Res<String> {
        Ok(format!("Bearer {}", self.token()?))
    }

    pub fn list_games(&self) -> Res<Vec<String>> {
        match self.provider {
            Provider::Google => {
                let Some(root) = self.gd_folder("", false)? else {
                    return Ok(vec![]);
                };
                Ok(self
                    .gd_children(&root, true)?
                    .into_iter()
                    .map(|f| f.name)
                    .collect())
            }
            Provider::OneDrive => Ok(self
                .od_children("")?
                .into_iter()
                .filter(|(_, folder)| *folder)
                .map(|(f, _)| f.name)
                .collect()),
            Provider::Dropbox => Ok(self
                .db_list("")?
                .into_iter()
                .filter(|(_, folder)| *folder)
                .map(|(f, _)| f.name)
                .collect()),
        }
    }

    pub fn list(&self, game: &str) -> Res<Vec<RemoteFile>> {
        match self.provider {
            Provider::Google => match self.gd_folder(game, false)? {
                Some(id) => self.gd_children(&id, false),
                None => Ok(vec![]),
            },
            Provider::OneDrive => Ok(self
                .od_children(game)?
                .into_iter()
                .filter(|(_, d)| !d)
                .map(|(f, _)| f)
                .collect()),
            Provider::Dropbox => Ok(self
                .db_list(game)?
                .into_iter()
                .filter(|(_, d)| !d)
                .map(|(f, _)| f)
                .collect()),
        }
    }

    pub fn upload(&self, game: &str, name: &str, data: &[u8], on: &dyn Fn(u64, u64)) -> Res<()> {
        match self.provider {
            Provider::Google => {
                let parent = self
                    .gd_folder(game, true)?
                    .ok_or("Couldn't create the cloud folder.")?;
                let boundary = format!("sv{}", rand_b64(12));
                let meta = json!({ "name": name, "parents": [parent] }).to_string();
                let mut body = format!(
                    "--{boundary}\r\nContent-Type: application/json; charset=UTF-8\r\n\r\n{meta}\r\n--{boundary}\r\nContent-Type: application/zip\r\n\r\n"
                )
                .into_bytes();
                body.extend_from_slice(data);
                body.extend_from_slice(format!("\r\n--{boundary}--").as_bytes());
                let total = body.len() as u64;
                ureq::post("https://www.googleapis.com/upload/drive/v3/files?uploadType=multipart&fields=id")
                    .set("Authorization", &self.auth()?)
                    .set("Content-Type", &format!("multipart/related; boundary={boundary}"))
                    .set("Content-Length", &total.to_string())
                    .send(counting(&body, 0, total, on))
                    .map_err(http_err)?;
                Ok(())
            }
            Provider::OneDrive => self.od_upload(game, name, data, on),
            Provider::Dropbox => {
                let arg =
                    json!({ "path": format!("/{game}/{name}"), "mode": "overwrite", "mute": true });
                let total = data.len() as u64;
                ureq::post("https://content.dropboxapi.com/2/files/upload")
                    .set("Authorization", &self.auth()?)
                    .set("Dropbox-API-Arg", &ascii_json(&arg))
                    .set("Content-Type", "application/octet-stream")
                    .set("Content-Length", &total.to_string())
                    .send(counting(data, 0, total, on))
                    .map_err(http_err)?;
                Ok(())
            }
        }
    }

    pub fn download(&self, game: &str, f: &RemoteFile) -> Res<Vec<u8>> {
        let resp = match self.provider {
            Provider::Google => ureq::get(&format!(
                "https://www.googleapis.com/drive/v3/files/{}?alt=media",
                f.id
            ))
            .set("Authorization", &self.auth()?)
            .call(),
            Provider::OneDrive => ureq::get(&format!(
                "{}:/content",
                self.od_path(&format!("{game}/{}", f.name))
            ))
            .set("Authorization", &self.auth()?)
            .call(),
            Provider::Dropbox => ureq::post("https://content.dropboxapi.com/2/files/download")
                .set("Authorization", &self.auth()?)
                .set(
                    "Dropbox-API-Arg",
                    &ascii_json(&json!({ "path": format!("/{game}/{}", f.name) })),
                )
                .call(),
        };
        read_bytes(resp.map_err(http_err)?)
    }

    pub fn delete(&self, game: &str, f: &RemoteFile) -> Res<()> {
        let r = match self.provider {
            Provider::Google => ureq::delete(&format!(
                "https://www.googleapis.com/drive/v3/files/{}",
                f.id
            ))
            .set("Authorization", &self.auth()?)
            .call(),
            Provider::OneDrive => ureq::delete(&self.od_path(&format!("{game}/{}", f.name)))
                .set("Authorization", &self.auth()?)
                .call(),
            Provider::Dropbox => ureq::post("https://api.dropboxapi.com/2/files/delete_v2")
                .set("Authorization", &self.auth()?)
                .set("Content-Type", "application/json")
                .send_string(&json!({ "path": format!("/{game}/{}", f.name) }).to_string()),
        };
        match r {
            Ok(_) => Ok(()),
            Err(e) if status(&e) == Some(404) => Ok(()),
            Err(e) => Err(http_err(e)),
        }
    }

    fn gd_query(&self, q: &str) -> Res<Vec<Value>> {
        let mut out = Vec::new();
        let mut page = String::new();
        loop {
            let mut url = format!(
                "https://www.googleapis.com/drive/v3/files?q={}&fields={}&pageSize=1000&spaces=drive",
                pct(q),
                pct("nextPageToken,files(id,name,size,mimeType)")
            );
            if !page.is_empty() {
                url.push_str(&format!("&pageToken={}", pct(&page)));
            }
            let v = read_json(
                ureq::get(&url)
                    .set("Authorization", &self.auth()?)
                    .call()
                    .map_err(http_err)?,
            )?;
            out.extend(v["files"].as_array().cloned().unwrap_or_default());
            match v["nextPageToken"].as_str() {
                Some(t) => page = t.to_string(),
                None => return Ok(out),
            }
        }
    }

    fn gd_children(&self, parent: &str, folders: bool) -> Res<Vec<RemoteFile>> {
        let kind = if folders { "=" } else { "!=" };
        let q = format!(
            "'{parent}' in parents and trashed=false and mimeType{kind}'application/vnd.google-apps.folder'"
        );
        Ok(self
            .gd_query(&q)?
            .into_iter()
            .map(|f| RemoteFile {
                name: f["name"].as_str().unwrap_or_default().to_string(),
                id: f["id"].as_str().unwrap_or_default().to_string(),
                size: f["size"].as_str().and_then(|s| s.parse().ok()).unwrap_or(0),
            })
            .collect())
    }

    fn gd_folder(&self, game: &str, create: bool) -> Res<Option<String>> {
        if let Some(id) = self.folders.lock().unwrap().get(game) {
            return Ok(Some(id.clone()));
        }
        let parent = if game.is_empty() {
            None
        } else {
            match self.gd_folder("", create)? {
                Some(p) => Some(p),
                None => return Ok(None),
            }
        };
        let name = if game.is_empty() { "Saveward" } else { game };
        let esc = name.replace('\\', "\\\\").replace('\'', "\\'");
        let mut q = format!(
            "name='{esc}' and mimeType='application/vnd.google-apps.folder' and trashed=false"
        );
        if let Some(p) = &parent {
            q.push_str(&format!(" and '{p}' in parents"));
        }
        let mut found = self
            .gd_query(&q)?
            .first()
            .and_then(|f| f["id"].as_str())
            .map(String::from);
        if found.is_none() && game.is_empty() {
            found = self.gd_rename_old_root()?;
        }
        let id = match found {
            Some(id) => id,
            None if create => {
                let mut meta =
                    json!({ "name": name, "mimeType": "application/vnd.google-apps.folder" });
                if let Some(p) = parent {
                    meta["parents"] = json!([p]);
                }
                let v = read_json(
                    ureq::post("https://www.googleapis.com/drive/v3/files?fields=id")
                        .set("Authorization", &self.auth()?)
                        .set("Content-Type", "application/json")
                        .send_string(&meta.to_string())
                        .map_err(http_err)?,
                )?;
                v["id"]
                    .as_str()
                    .ok_or("Folder creation failed.")?
                    .to_string()
            }
            None => return Ok(None),
        };
        self.folders
            .lock()
            .unwrap()
            .insert(game.to_string(), id.clone());
        Ok(Some(id))
    }

    fn gd_rename_old_root(&self) -> Res<Option<String>> {
        let q =
            "name='SaveVault' and mimeType='application/vnd.google-apps.folder' and trashed=false";
        let Some(id) = self
            .gd_query(q)?
            .first()
            .and_then(|f| f["id"].as_str())
            .map(String::from)
        else {
            return Ok(None);
        };
        ureq::request(
            "PATCH",
            &format!("https://www.googleapis.com/drive/v3/files/{id}"),
        )
        .set("Authorization", &self.auth()?)
        .set("Content-Type", "application/json")
        .send_string(&json!({ "name": "Saveward" }).to_string())
        .map_err(http_err)?;
        Ok(Some(id))
    }

    fn od_path(&self, rel: &str) -> String {
        let enc: Vec<String> = rel.split('/').map(pct).collect();
        format!(
            "https://graph.microsoft.com/v1.0/me/drive/special/approot:/{}",
            enc.join("/")
        )
    }

    fn od_children(&self, game: &str) -> Res<Vec<(RemoteFile, bool)>> {
        let mut url = if game.is_empty() {
            "https://graph.microsoft.com/v1.0/me/drive/special/approot/children?$top=999"
                .to_string()
        } else {
            format!("{}:/children?$top=999", self.od_path(game))
        };
        let mut out = Vec::new();
        loop {
            let v = match ureq::get(&url).set("Authorization", &self.auth()?).call() {
                Ok(r) => read_json(r)?,
                Err(e) if status(&e) == Some(404) => return Ok(out),
                Err(e) => return Err(http_err(e)),
            };
            for f in v["value"].as_array().cloned().unwrap_or_default() {
                out.push((
                    RemoteFile {
                        name: f["name"].as_str().unwrap_or_default().to_string(),
                        id: f["id"].as_str().unwrap_or_default().to_string(),
                        size: f["size"].as_u64().unwrap_or(0),
                    },
                    f.get("folder").is_some(),
                ));
            }
            match v["@odata.nextLink"].as_str() {
                Some(next) => url = next.to_string(),
                None => return Ok(out),
            }
        }
    }

    fn od_upload(&self, game: &str, name: &str, data: &[u8], on: &dyn Fn(u64, u64)) -> Res<()> {
        let path = self.od_path(&format!("{game}/{name}"));
        let total_len = data.len() as u64;
        if data.len() < 4 * 1024 * 1024 {
            ureq::put(&format!("{path}:/content"))
                .set("Authorization", &self.auth()?)
                .set("Content-Type", "application/zip")
                .set("Content-Length", &total_len.to_string())
                .send(counting(data, 0, total_len, on))
                .map_err(http_err)?;
            return Ok(());
        }
        let v = read_json(
            ureq::post(&format!("{path}:/createUploadSession"))
                .set("Authorization", &self.auth()?)
                .set("Content-Type", "application/json")
                .send_string(
                    &json!({ "item": { "@microsoft.graph.conflictBehavior": "replace" } })
                        .to_string(),
                )
                .map_err(http_err)?,
        )?;
        let upload_url = v["uploadUrl"].as_str().ok_or("No upload URL.")?.to_string();
        const CHUNK: usize = 327_680 * 30;
        let total = data.len();
        let mut start = 0;
        while start < total {
            let end = (start + CHUNK).min(total);
            ureq::put(&upload_url)
                .set(
                    "Content-Range",
                    &format!("bytes {start}-{}/{total}", end - 1),
                )
                .set("Content-Length", &(end - start).to_string())
                .send(counting(&data[start..end], start as u64, total as u64, on))
                .map_err(http_err)?;
            start = end;
        }
        Ok(())
    }

    fn db_list(&self, game: &str) -> Res<Vec<(RemoteFile, bool)>> {
        let path = if game.is_empty() {
            String::new()
        } else {
            format!("/{game}")
        };
        let mut out = Vec::new();
        let mut req = (
            "https://api.dropboxapi.com/2/files/list_folder",
            json!({ "path": path, "limit": 2000 }),
        );
        loop {
            let v = match ureq::post(req.0)
                .set("Authorization", &self.auth()?)
                .set("Content-Type", "application/json")
                .send_string(&req.1.to_string())
            {
                Ok(r) => read_json(r)?,
                Err(e) if status(&e) == Some(409) => return Ok(out),
                Err(e) => return Err(http_err(e)),
            };
            for f in v["entries"].as_array().cloned().unwrap_or_default() {
                out.push((
                    RemoteFile {
                        name: f["name"].as_str().unwrap_or_default().to_string(),
                        id: f["id"].as_str().unwrap_or_default().to_string(),
                        size: f["size"].as_u64().unwrap_or(0),
                    },
                    f[".tag"] == "folder",
                ));
            }
            if v["has_more"].as_bool() != Some(true) {
                return Ok(out);
            }
            req = (
                "https://api.dropboxapi.com/2/files/list_folder/continue",
                json!({ "cursor": v["cursor"] }),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encoding_helpers() {
        assert_eq!(
            pct("Elden Ring: Nightreign"),
            "Elden%20Ring%3A%20Nightreign"
        );
        assert_eq!(unpct("a%20b+c%2F"), "a b c/");
        assert_eq!(
            ascii_json(&json!({"p": "/Pokémon"})),
            "{\"p\":\"/Pok\\u00e9mon\"}"
        );
    }

    #[test]
    fn loopback_callback() {
        let l = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = l.local_addr().unwrap().port();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(200));
            let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
            s.write_all(b"GET /callback?code=abc%2F123&state=xyz HTTP/1.1\r\nHost: x\r\n\r\n")
                .unwrap();
            let mut r = String::new();
            let _ = s.read_to_string(&mut r);
        });
        assert_eq!(wait_for_code(&l, "xyz").unwrap(), "abc/123");
    }
}
