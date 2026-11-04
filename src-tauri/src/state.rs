use crate::discord::SpoofableGame;
use serde::{Deserialize, Serialize};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SpooferStatus {
    Idle,
    Running,
    Paused,
    Finished,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SpoofMode {
    Hybrid,
    #[default]
    ProcessOnly,
    RpcOnly,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActiveGameInfo {
    pub id: String,
    pub name: String,
    pub executable: Option<String>,
    pub pid: Option<u32>,
    pub icon_hash: Option<String>,
    pub method: String, // "process" | "rpc"
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpooferTickEvent {
    pub current_index: usize,
    pub total_games: usize,
    pub active_batch: Vec<ActiveGameInfo>,
    pub elapsed_secs: u64,
    pub estimated_remaining_secs: u64,
    pub games_per_minute: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpooferLogEvent {
    pub timestamp: String,
    pub level: String, // "info" | "success" | "warn" | "error"
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpooferStatusEvent {
    pub status: SpooferStatus,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FetchProgressEvent {
    pub total_fetched: usize,
    pub process_count: usize,
    pub rpc_count: usize,
    #[serde(default)]
    pub history_process_count: usize,
    #[serde(default)]
    pub history_rpc_count: usize,
    pub is_loading: bool,
}

fn default_true() -> bool {
    true
}

fn default_cooldown_batches() -> usize {
    25
}

fn default_cooldown_seconds() -> u64 {
    30
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpooferConfig {
    pub limit: Option<usize>,
    pub batch_size: usize,
    pub hold_duration_secs: u64,
    pub start_offset: Option<usize>,
    #[serde(default)]
    pub skip_previously_spoofed: bool,
    #[serde(default)]
    pub spoof_mode: SpoofMode,
    #[serde(default = "default_true")]
    pub cooldown_enabled: bool,
    #[serde(default = "default_cooldown_batches")]
    pub cooldown_every_n_batches: usize,
    #[serde(default = "default_cooldown_seconds")]
    pub cooldown_seconds: u64,
    #[serde(default)]
    pub jitter_enabled: bool,
    /// Farm mode: when non-empty, exactly these games run as one fixed batch.
    #[serde(default)]
    pub farm_game_ids: Vec<String>,
    /// Farm mode time limit; `None` keeps the games running until stopped.
    #[serde(default)]
    pub farm_duration_secs: Option<u64>,
    #[serde(default)]
    pub beta_enabled: bool,
}

pub struct AppState {
    pub games_db: Arc<RwLock<Vec<SpoofableGame>>>,
    pub status: Arc<RwLock<SpooferStatus>>,
    pub cancel_flag: Arc<AtomicBool>,
    pub pause_flag: Arc<AtomicBool>,
    pub current_task: Arc<Mutex<Option<tokio::task::JoinHandle<()>>>>,
}

impl AppState {
    pub fn new() -> Self {
        Self {
            games_db: Arc::new(RwLock::new(Vec::new())),
            status: Arc::new(RwLock::new(SpooferStatus::Idle)),
            cancel_flag: Arc::new(AtomicBool::new(false)),
            pause_flag: Arc::new(AtomicBool::new(false)),
            current_task: Arc::new(Mutex::new(None)),
        }
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}
impl SpooferConfig {
    pub fn validate(&self) -> Result<(), String> {
        if !(1..=4).contains(&self.batch_size) {
            return Err("Выберите от 1 до 4 процессов".into());
        }
        if !(15..=3600).contains(&self.hold_duration_secs) {
            return Err("Удержание: от 15 до 3600 секунд".into());
        }
        if self.limit.is_some_and(|n| n == 0 || n > 100_000)
            || self.start_offset.is_some_and(|n| n > 100_000)
        {
            return Err("Некорректный диапазон игр".into());
        }
        if !(1..=100).contains(&self.cooldown_every_n_batches)
            || !(1..=600).contains(&self.cooldown_seconds)
        {
            return Err("Некорректные параметры паузы".into());
        }
        if self.farm_game_ids.len() > 4
            || self
                .farm_game_ids
                .iter()
                .any(|id| !crate::security::valid_id(id))
        {
            return Err("Выберите до 4 игр".into());
        }
        if self
            .farm_duration_secs
            .is_some_and(|n| !(15..=28800).contains(&n))
        {
            return Err("Длительность: от 15 секунд до 8 часов".into());
        }
        if self.spoof_mode != SpoofMode::ProcessOnly && !self.beta_enabled {
            return Err("RPC требует явного включения экспериментального режима".into());
        }
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn config() -> SpooferConfig {
        serde_json::from_value(serde_json::json!({"limit":null,"batch_size":1,"hold_duration_secs":60,"start_offset":0})).unwrap()
    }
    #[test]
    fn defaults_are_native() {
        let c = config();
        assert_eq!(c.spoof_mode, SpoofMode::ProcessOnly);
        assert!(!c.beta_enabled);
        assert!(c.validate().is_ok());
    }
    #[test]
    fn validates_backend_limits() {
        let mut c = config();
        c.batch_size = 0;
        assert!(c.validate().is_err());
        c.batch_size = 5;
        assert!(c.validate().is_err());
        c.batch_size = 1;
        c.cooldown_every_n_batches = 0;
        assert!(c.validate().is_err());
        c.cooldown_every_n_batches = 25;
        c.hold_duration_secs = u64::MAX;
        assert!(c.validate().is_err());
    }
    #[test]
    fn rpc_requires_opt_in() {
        let mut c = config();
        c.spoof_mode = SpoofMode::RpcOnly;
        assert!(c.validate().is_err());
        c.beta_enabled = true;
        assert!(c.validate().is_ok());
    }
    #[test]
    fn duration_is_bounded() {
        let mut c = config();
        c.farm_duration_secs = Some(28801);
        assert!(c.validate().is_err());
        c.farm_duration_secs = Some(0);
        assert!(c.validate().is_err());
    }
}
