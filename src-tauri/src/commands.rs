use crate::discord::{is_discord_running, load_games, SpoofMethod};
use crate::history::{clear_history as clear_history_fs, load_history, record_played_batch};
use crate::security::MAX_HISTORY;
use crate::spoofer::run_spoofer;
use crate::state::{AppState, SpooferConfig, SpooferStatus, SpooferStatusEvent};
use serde::Serialize;
use std::sync::atomic::Ordering;
use tauri::{AppHandle, Emitter, State};
#[derive(Serialize)]
pub struct FetchGamesResponse {
    pub total: usize,
    pub process_count: usize,
    pub rpc_count: usize,
    pub history_process_count: usize,
    pub history_rpc_count: usize,
    pub from_cache: bool,
}

#[derive(Serialize)]
pub struct SpooferStateResponse {
    pub status: SpooferStatus,
    pub total_count: usize,
    pub process_count: usize,
    pub rpc_count: usize,
    pub history_count: usize,
    pub history_process_count: usize,
    pub history_rpc_count: usize,
}

#[tauri::command]
pub async fn fetch_games(
    force: Option<bool>,
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<FetchGamesResponse, String> {
    // Serialize refreshes; never replace the database during a live session.
    let _control = state.current_task.lock().await;
    if matches!(
        *state.status.read().await,
        SpooferStatus::Running | SpooferStatus::Paused
    ) {
        return Err("Остановите сессию перед обновлением базы".into());
    }
    let (games, from_cache) = load_games(&app, force.unwrap_or(false)).await?;
    let history = load_history()?;
    let process_count = games
        .iter()
        .filter(|g| g.method == SpoofMethod::Process)
        .count();
    let hp = games
        .iter()
        .filter(|g| g.method == SpoofMethod::Process && history.contains(&g.id))
        .count();
    let hr = games
        .iter()
        .filter(|g| g.method == SpoofMethod::Rpc && history.contains(&g.id))
        .count();
    let result = FetchGamesResponse {
        total: games.len(),
        process_count,
        rpc_count: games.len() - process_count,
        history_process_count: hp,
        history_rpc_count: hr,
        from_cache,
    };
    *state.games_db.write().await = games;
    Ok(result)
}
#[tauri::command]
pub async fn start_spoofer(
    config: SpooferConfig,
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<(), String> {
    config.validate()?;
    let mut task = state.current_task.lock().await;
    if task.as_ref().is_some_and(|t| !t.is_finished()) {
        return Err("Сессия уже запущена".into());
    }
    let games = state.games_db.read().await.clone();
    if games.is_empty() {
        return Err("Сначала загрузите базу игр".into());
    }
    if !is_discord_running() {
        return Err("Сначала запустите Discord Desktop".into());
    }
    state.cancel_flag.store(false, Ordering::SeqCst);
    state.pause_flag.store(false, Ordering::SeqCst);
    *state.status.write().await = SpooferStatus::Running;
    let _ = app.emit(
        "spoofer_status",
        SpooferStatusEvent {
            status: SpooferStatus::Running,
            message: None,
        },
    );
    *task = Some(tokio::spawn(run_spoofer(app, games, config)));
    Ok(())
}
#[tauri::command]
pub async fn pause_spoofer(state: State<'_, AppState>, app: AppHandle) -> Result<(), String> {
    let _g = state.current_task.lock().await;
    let mut s = state.status.write().await;
    if *s != SpooferStatus::Running {
        return Err("Нет активной сессии".into());
    }
    state.pause_flag.store(true, Ordering::SeqCst);
    *s = SpooferStatus::Paused;
    let _ = app.emit(
        "spoofer_status",
        SpooferStatusEvent {
            status: *s,
            message: None,
        },
    );
    Ok(())
}
#[tauri::command]
pub async fn resume_spoofer(state: State<'_, AppState>, app: AppHandle) -> Result<(), String> {
    let _g = state.current_task.lock().await;
    let mut s = state.status.write().await;
    if *s != SpooferStatus::Paused {
        return Err("Сессия не на паузе".into());
    }
    state.pause_flag.store(false, Ordering::SeqCst);
    *s = SpooferStatus::Running;
    let _ = app.emit(
        "spoofer_status",
        SpooferStatusEvent {
            status: *s,
            message: None,
        },
    );
    Ok(())
}
pub async fn shutdown(state: &AppState) -> Result<(), String> {
    let mut task = state.current_task.lock().await;
    state.cancel_flag.store(true, Ordering::SeqCst);
    state.pause_flag.store(false, Ordering::SeqCst);
    if let Some(t) = task.take() {
        t.await
            .map_err(|e| format!("Ошибка завершения сессии: {e}"))?;
    }
    *state.status.write().await = SpooferStatus::Idle;
    Ok(())
}
#[tauri::command]
pub async fn stop_spoofer(state: State<'_, AppState>, app: AppHandle) -> Result<(), String> {
    shutdown(&state).await?;
    let _ = app.emit(
        "spoofer_status",
        SpooferStatusEvent {
            status: SpooferStatus::Idle,
            message: None,
        },
    );
    Ok(())
}
#[tauri::command]
pub fn check_discord_status() -> bool {
    is_discord_running()
}
#[tauri::command]
pub async fn get_spoofer_state(state: State<'_, AppState>) -> Result<SpooferStateResponse, String> {
    let status = *state.status.read().await;
    let games = state.games_db.read().await;
    let h = load_history()?;
    let process_count = games
        .iter()
        .filter(|g| g.method == SpoofMethod::Process)
        .count();
    Ok(SpooferStateResponse {
        status,
        total_count: games.len(),
        process_count,
        rpc_count: games.len() - process_count,
        history_count: h.len(),
        history_process_count: games
            .iter()
            .filter(|g| g.method == SpoofMethod::Process && h.contains(&g.id))
            .count(),
        history_rpc_count: games
            .iter()
            .filter(|g| g.method == SpoofMethod::Rpc && h.contains(&g.id))
            .count(),
    })
}
#[tauri::command]
pub async fn get_games_list(
    state: State<'_, AppState>,
) -> Result<Vec<crate::discord::SpoofableGame>, String> {
    Ok(state.games_db.read().await.clone())
}
#[tauri::command]
pub fn set_language(language: String) {
    crate::i18n::set_language(&language);
}
#[tauri::command]
pub async fn clear_history(state: State<'_, AppState>) -> Result<(), String> {
    let _g = state.current_task.lock().await;
    if matches!(
        *state.status.read().await,
        SpooferStatus::Running | SpooferStatus::Paused
    ) {
        return Err("Остановите сессию перед изменением истории".into());
    }
    clear_history_fs()
}
#[tauri::command]
pub fn export_history() -> Result<String, String> {
    let mut ids: Vec<_> = load_history()?.into_iter().collect();
    ids.sort();
    serde_json::to_string_pretty(&ids).map_err(|e| e.to_string())
}
#[tauri::command]
pub async fn import_history(
    json_data: String,
    state: State<'_, AppState>,
) -> Result<usize, String> {
    let _g = state.current_task.lock().await;
    if matches!(
        *state.status.read().await,
        SpooferStatus::Running | SpooferStatus::Paused
    ) {
        return Err("Остановите сессию перед изменением истории".into());
    }
    if json_data.len() > 4 * 1024 * 1024 {
        return Err("Файл истории превышает 4 МБ".into());
    }
    let ids: Vec<String> = serde_json::from_str(&json_data).map_err(|e| e.to_string())?;
    if ids.len() > MAX_HISTORY {
        return Err("Слишком много записей".into());
    }
    record_played_batch(&ids)
}
