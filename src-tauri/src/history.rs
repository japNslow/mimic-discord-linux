use crate::security::{atomic_write, data_dir, read_json, valid_id, MAX_HISTORY};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::sync::Mutex;
static HISTORY_LOCK: Mutex<()> = Mutex::new(());
#[derive(Serialize, Deserialize, Default)]
struct History {
    played_ids: HashSet<String>,
}
fn load() -> Result<HashSet<String>, String> {
    let path = data_dir()?.join("history.json");
    if !path.try_exists().map_err(|e| e.to_string())? {
        return Ok(HashSet::new());
    }
    let h: History =
        serde_json::from_str(&read_json(&path, 4 * 1024 * 1024)?).map_err(|e| e.to_string())?;
    if h.played_ids.len() > MAX_HISTORY || h.played_ids.iter().any(|id| !valid_id(id)) {
        return Err("Invalid history".into());
    }
    Ok(h.played_ids)
}
pub fn load_history() -> Result<HashSet<String>, String> {
    let _g = HISTORY_LOCK.lock().map_err(|e| e.to_string())?;
    load()
}
pub fn record_played_batch(ids: &[String]) -> Result<usize, String> {
    if ids.len() > MAX_HISTORY || ids.iter().any(|id| !valid_id(id)) {
        return Err("Invalid history IDs".into());
    }
    let _g = HISTORY_LOCK.lock().map_err(|e| e.to_string())?;
    let mut all = load()?;
    let before = all.len();
    all.extend(ids.iter().cloned());
    if all.len() > MAX_HISTORY {
        return Err("History limit exceeded".into());
    }
    let added = all.len() - before;
    let bytes = serde_json::to_vec(&History { played_ids: all }).map_err(|e| e.to_string())?;
    atomic_write(&data_dir()?.join("history.json"), &bytes)?;
    Ok(added)
}
pub fn clear_history() -> Result<(), String> {
    let _g = HISTORY_LOCK.lock().map_err(|e| e.to_string())?;
    atomic_write(&data_dir()?.join("history.json"), br#"{"played_ids":[]}"#)
}
