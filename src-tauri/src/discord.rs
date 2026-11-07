use crate::security::{atomic_write, data_dir, read_json, safe_relative, valid_id, MAX_JSON};
use serde::{Deserialize, Serialize};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tauri::AppHandle;
const URL: &str = "https://cdn.discordapp.com/detectables/games-v1.json";
// Version 1 incorrectly classified every Windows-only rule as RPC on Linux.
const CACHE_VERSION: u32 = 2;
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpoofMethod {
    Process,
    Rpc,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpoofableGame {
    pub id: String,
    pub name: String,
    pub rel_path: Option<String>,
    pub arguments: Option<String>,
    pub icon_hash: Option<String>,
    pub method: SpoofMethod,
}
impl SpoofableGame {
    pub fn validate(&self) -> bool {
        valid_id(&self.id)
            && !self.name.is_empty()
            && self.name.len() <= 512
            && !self.name.chars().any(char::is_control)
            && match self.method {
                SpoofMethod::Process => self
                    .rel_path
                    .as_ref()
                    .is_some_and(|p| safe_relative(p).is_some()),
                SpoofMethod::Rpc => self.rel_path.is_none(),
            }
    }
}
#[derive(Deserialize)]
struct RawExe {
    name: String,
    os: String,
    #[serde(default)]
    is_launcher: bool,
    #[serde(default)]
    arguments: Option<String>,
}
#[derive(Deserialize)]
struct RawApp {
    id: String,
    name: String,
    #[serde(default)]
    executables: Vec<RawExe>,
}
#[derive(Serialize, Deserialize)]
struct Cache {
    #[serde(default)]
    version: u32,
    fetched_at: u64,
    games: Vec<SpoofableGame>,
}
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
fn cached() -> Option<Cache> {
    let p = data_dir().ok()?.join("games.json");
    let cache: Cache = serde_json::from_str(&read_json(&p, MAX_JSON).ok()?).ok()?;
    if cache.version != CACHE_VERSION
        || cache.games.is_empty()
        || cache.games.len() > 100_000
        || cache.games.iter().any(|g| !g.validate())
    {
        return None;
    }
    Some(cache)
}
fn parse_games(bytes: &[u8]) -> Result<Vec<SpoofableGame>, String> {
    let apps: Vec<RawApp> =
        serde_json::from_slice(bytes).map_err(|e| format!("Invalid database: {e}"))?;
    if apps.len() > 100_000 {
        return Err("Database exceeds application limit".into());
    }
    let mut ids = std::collections::HashSet::new();
    let games: Vec<_> = apps
        .into_iter()
        .filter_map(|app| {
            let rel = app
                .executables
                .iter()
                // Discord's current Linux observer receives executable rules
                // from every OS. A Linux ELF named overwatch.exe is detectable
                // by that observer; no Wine, RPC or client modification is used.
                // Prefer a Linux name, otherwise a Windows name. Skip launchers
                // and argument-specific rules we cannot faithfully reproduce.
                .filter(|e| {
                    matches!(e.os.as_str(), "linux" | "win32")
                        && !e.is_launcher
                        && e.arguments.as_deref().is_none_or(str::is_empty)
                        && safe_relative(&e.name).is_some()
                })
                .min_by_key(|e| e.os != "linux")
                .map(|e| e.name.clone());
            let game = SpoofableGame {
                id: app.id,
                name: app.name,
                method: if rel.is_some() {
                    SpoofMethod::Process
                } else {
                    SpoofMethod::Rpc
                },
                rel_path: rel,
                arguments: None,
                icon_hash: None,
            };
            if game.validate() && ids.insert(game.id.clone()) {
                Some(game)
            } else {
                None
            }
        })
        .collect();
    if games.is_empty() {
        return Err("Database contains no valid games".into());
    }
    Ok(games)
}
async fn fetch() -> Result<Vec<SpoofableGame>, String> {
    let client = reqwest::Client::builder()
        .https_only(true)
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(8))
        .timeout(Duration::from_secs(30))
        .user_agent("Mimic-Linux/0.1.0")
        .build()
        .map_err(|e| e.to_string())?;
    // No credentials, account tokens, retries or hidden refreshes.
    let mut response = client
        .get(URL)
        .send()
        .await
        .map_err(|e| format!("Discord network error: {e}"))?;
    if response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
        return Err("Discord: 429. Wait before refreshing the database.".into());
    }
    if !response.status().is_success() {
        return Err(format!("Discord HTTP {}", response.status()));
    }
    if response.content_length().is_some_and(|n| n > MAX_JSON) {
        return Err("Database too large".into());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|e| e.to_string())? {
        if bytes.len() + chunk.len() > MAX_JSON as usize {
            return Err("Database too large".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    parse_games(&bytes)
}
pub async fn load_games(
    app: &AppHandle,
    force: bool,
) -> Result<(Vec<SpoofableGame>, bool), String> {
    let cache = cached();
    if !force {
        if let Some(c) = &cache {
            if now().saturating_sub(c.fetched_at) < 12 * 3600 {
                return Ok((c.games.clone(), true));
            }
        }
    }
    match fetch().await {
        Ok(games) => {
            let bytes = serde_json::to_vec(&Cache {
                version: CACHE_VERSION,
                fetched_at: now(),
                games: games.clone(),
            })
            .map_err(|e| e.to_string())?;
            atomic_write(&data_dir()?.join("games.json"), &bytes)?;
            Ok((games, false))
        }
        Err(e) => {
            if let Some(c) = cache {
                crate::spoofer::log_event(
                    app,
                    "warn",
                    &format!("{e} Используется ранее сохранённая база."),
                );
                Ok((c.games, true))
            } else {
                Err(e)
            }
        }
    }
}
pub fn is_discord_running() -> bool {
    // Read only comm for processes owned by this user; never launch pgrep or a shell.
    use std::os::unix::fs::MetadataExt;
    std::fs::read_dir("/proc")
        .into_iter()
        .flatten()
        .flatten()
        .any(|e| {
            e.file_name()
                .to_str()
                .is_some_and(|s| s.bytes().all(|b| b.is_ascii_digit()))
                && e.metadata()
                    .is_ok_and(|m| m.uid() == unsafe { libc::geteuid() })
                && std::fs::read_to_string(e.path().join("comm")).is_ok_and(|s| {
                    matches!(
                        s.trim().to_ascii_lowercase().as_str(),
                        "discord" | "discordcanary" | "discordptb" | "discorddevelopm"
                    )
                })
        })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn windows_names_are_linux_process_candidates() {
        let games=parse_games(br#"[{"id":"1","name":"Linux game","executables":[{"name":"bin/game","os":"linux"}]},{"id":"2","name":"Windows","executables":[{"name":"game/game.exe","os":"win32"}]}]"#).unwrap();
        assert_eq!(games[0].method, SpoofMethod::Process);
        assert!(games[0].arguments.is_none());
        assert_eq!(games[1].method, SpoofMethod::Process);
        assert_eq!(games[1].rel_path.as_deref(), Some("game/game.exe"));
        assert!(games[1].arguments.is_none());
    }
    #[test]
    fn prefers_linux_name_and_skips_launchers_or_required_arguments() {
        let games=parse_games(br#"[
          {"id":"1","name":"Native first","executables":[{"name":"a.exe","os":"win32"},{"name":"native","os":"linux"}]},
          {"id":"2","name":"Launcher","executables":[{"name":"launcher.exe","os":"win32","is_launcher":true},{"name":"real.exe","os":"win32"}]},
          {"id":"3","name":"Needs arguments","executables":[{"name":"runtime.exe","os":"win32","arguments":"--game test"}]},
          {"id":"4","name":"Mac only","executables":[{"name":"game.app","os":"darwin"}]}
        ]"#).unwrap();
        assert_eq!(games[0].rel_path.as_deref(), Some("native"));
        assert_eq!(games[1].rel_path.as_deref(), Some("real.exe"));
        assert_eq!(games[2].method, SpoofMethod::Rpc);
        assert_eq!(games[3].method, SpoofMethod::Rpc);
    }
    #[test]
    fn rejects_unsafe_windows_names() {
        let games = parse_games(
            br#"[
          {"id":"1","name":"Traversal","executables":[{"name":"../game.exe","os":"win32"}]},
          {"id":"2","name":"Absolute","executables":[{"name":"/tmp/game.exe","os":"win32"}]}
        ]"#,
        )
        .unwrap();
        assert!(games
            .iter()
            .all(|g| g.method == SpoofMethod::Rpc && g.rel_path.is_none()));
    }
    #[test]
    fn legacy_cache_cannot_hide_new_process_candidates() {
        let old: Cache = serde_json::from_str(r#"{"fetched_at":1,"games":[]}"#).unwrap();
        assert_ne!(old.version, CACHE_VERSION);
    }
    #[test]
    fn rejects_poisoned_cache_path() {
        let g = SpoofableGame {
            id: "1".into(),
            name: "test".into(),
            rel_path: Some("/tmp/pwn".into()),
            arguments: None,
            icon_hash: None,
            method: SpoofMethod::Process,
        };
        assert!(!g.validate());
    }
    #[test]
    fn filters_duplicates_and_bad_ids() {
        let g = parse_games(
            br#"[{"id":"1","name":"A"},{"id":"1","name":"B"},{"id":"../x","name":"C"}]"#,
        )
        .unwrap();
        assert_eq!(g.len(), 1);
    }
    #[test]
    #[ignore = "requires MIMIC_CATALOG_FIXTURE pointing to a downloaded public catalog"]
    fn downloaded_public_catalog() {
        let path = std::env::var("MIMIC_CATALOG_FIXTURE").expect("Set MIMIC_CATALOG_FIXTURE");
        let bytes = std::fs::read(path).unwrap();
        assert!(bytes.len() <= MAX_JSON as usize);
        let games = parse_games(&bytes).unwrap();
        let processes = games
            .iter()
            .filter(|g| g.method == SpoofMethod::Process)
            .count();
        assert!(
            processes > 1_000,
            "Expected the expanded catalog, not only the 8 Linux-labelled entries"
        );
        assert!(games.iter().all(SpoofableGame::validate));
        println!(
            "Public catalog parsed: {} valid applications, {} process candidates",
            games.len(),
            processes
        );
    }
}
