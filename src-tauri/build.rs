fn main() {
    println!("cargo:rerun-if-changed=cloud-keys.json");
    if let Ok(text) = std::fs::read_to_string("cloud-keys.json") {
        let keys = [
            ("google_id", "SAVEWARD_GOOGLE_ID"),
            ("google_secret", "SAVEWARD_GOOGLE_SECRET"),
            ("onedrive_id", "SAVEWARD_ONEDRIVE_ID"),
            ("dropbox_key", "SAVEWARD_DROPBOX_KEY"),
        ];
        for (json_key, env) in keys {
            if std::env::var(env).is_ok() {
                continue;
            }
            if let Some(v) = json_value(&text, json_key) {
                println!("cargo:rustc-env={env}={v}");
            }
        }
    }
    tauri_build::build()
}

fn json_value(text: &str, key: &str) -> Option<String> {
    let start = text.find(&format!("\"{key}\""))?;
    let rest = &text[start + key.len() + 2..];
    let rest = &rest[rest.find(':')? + 1..];
    let rest = &rest[rest.find('"')? + 1..];
    let v = &rest[..rest.find('"')?];
    (!v.is_empty()).then(|| v.to_string())
}
