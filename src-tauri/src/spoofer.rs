use crate::discord::{SpoofMethod, SpoofableGame};
use crate::history::{load_history, record_played_batch};
use crate::rpc::DiscordRpcSession;
use crate::security::{data_dir, private_tempdir, safe_relative};
use crate::state::{
    ActiveGameInfo, AppState, SpoofMode, SpooferConfig, SpooferLogEvent, SpooferStatus,
    SpooferStatusEvent, SpooferTickEvent,
};
use std::collections::HashSet;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::{fs::OpenOptionsExt, process::CommandExt};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter, Manager};
static DUMMY: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/dummy"));
const MAX_SESSION: Duration = Duration::from_secs(8 * 3600);

pub(crate) fn log_event(app: &AppHandle, level: &str, message: &str) {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        % 86400;
    let _ = app.emit(
        "spoofer_log",
        SpooferLogEvent {
            timestamp: format!(
                "{:02}:{:02}:{:02} UTC",
                seconds / 3600,
                seconds % 3600 / 60,
                seconds % 60
            ),
            level: level.into(),
            message: message.into(),
        },
    );
}
async fn status(app: &AppHandle, value: SpooferStatus, message: Option<String>) {
    *app.state::<AppState>().status.write().await = value;
    let _ = app.emit(
        "spoofer_status",
        SpooferStatusEvent {
            status: value,
            message,
        },
    );
}
fn cancelled(app: &AppHandle) -> bool {
    app.state::<AppState>().cancel_flag.load(Ordering::SeqCst)
}
fn paused(app: &AppHandle) -> bool {
    app.state::<AppState>().pause_flag.load(Ordering::SeqCst)
}
fn expired(start: Instant) -> bool {
    start.elapsed() >= MAX_SESSION
}
// Owning Child handles prevents killing an unrelated process after PID reuse.
struct OwnedChild(Child);
impl Drop for OwnedChild {
    fn drop(&mut self) {
        if !matches!(self.0.try_wait(), Ok(Some(_))) {
            let _ = self.0.kill();
        }
        let _ = self.0.wait();
    }
}
fn spawn_helper(root: &Path, game: &SpoofableGame) -> Result<OwnedChild, String> {
    if !game.validate() {
        return Err("Invalid game data".into());
    }
    let rel = safe_relative(game.rel_path.as_deref().ok_or("Missing executable")?)
        .ok_or("Unsafe executable path")?;
    let path = root.join(rel);
    fs::create_dir_all(path.parent().ok_or("No parent directory")?).map_err(|e| e.to_string())?;
    // Only write our embedded helper, never execute anything from the database.
    let mut f = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o700)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&path)
        .map_err(|e| e.to_string())?;
    f.write_all(DUMMY).map_err(|e| e.to_string())?;
    drop(f);
    let parent = std::process::id() as i32;
    let mut c = Command::new(path);
    c.arg(parent.to_string())
        .current_dir(root)
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    unsafe {
        c.pre_exec(move || {
            if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL, 0, 0, 0) != 0
                || libc::getppid() != parent
                || libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) != 0
            {
                return Err(std::io::Error::other("Cannot secure helper process"));
            }
            Ok(())
        });
    }
    c.spawn().map(OwnedChild).map_err(|e| e.to_string())
}
fn emit_tick(
    app: &AppHandle,
    active: Vec<ActiveGameInfo>,
    done: usize,
    total: usize,
    start: Instant,
    remaining: u64,
) {
    let _ = app.emit(
        "spoofer_tick",
        SpooferTickEvent {
            current_index: done,
            total_games: total,
            active_batch: active,
            elapsed_secs: start.elapsed().as_secs(),
            estimated_remaining_secs: remaining,
            games_per_minute: 0.0,
        },
    );
}
async fn wait(app: &AppHandle, duration: Duration, start: Instant) {
    let end = Instant::now() + duration;
    while Instant::now() < end && !cancelled(app) && !expired(start) {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}
pub async fn run_spoofer(app: AppHandle, games: Vec<SpoofableGame>, config: SpooferConfig) {
    let result = run(&app, games, config).await;
    match result {
        Ok(()) => {
            status(
                &app,
                if cancelled(&app) {
                    SpooferStatus::Idle
                } else {
                    SpooferStatus::Finished
                },
                None,
            )
            .await
        }
        Err(e) => {
            log_event(&app, "error", &e);
            status(&app, SpooferStatus::Error, Some(e)).await;
        }
    }
}
async fn run(
    app: &AppHandle,
    games: Vec<SpoofableGame>,
    config: SpooferConfig,
) -> Result<(), String> {
    config.validate()?;
    let start = Instant::now();
    let farm = !config.farm_game_ids.is_empty();
    let history = load_history()?;
    let mut pool: Vec<_> = games
        .into_iter()
        .filter(|g| g.validate())
        .filter(|g| match config.spoof_mode {
            SpoofMode::ProcessOnly => g.method == SpoofMethod::Process,
            SpoofMode::RpcOnly => g.method == SpoofMethod::Rpc,
            SpoofMode::Hybrid => true,
        })
        .filter(|g| {
            if farm {
                config.farm_game_ids.contains(&g.id)
            } else {
                !config.skip_previously_spoofed || !history.contains(&g.id)
            }
        })
        .skip(if farm {
            0
        } else {
            config.start_offset.unwrap_or(0)
        })
        .take(if farm {
            4
        } else {
            config.limit.unwrap_or(100_000)
        })
        .collect();
    let total = pool.len();
    if total == 0 {
        return Err("Нет подходящих игр. Обновите базу, проверьте фильтр и историю.".into());
    }
    let dir = data_dir()?;
    // Private random directories; RAII also cleans up on normal error unwinding.
    let session = private_tempdir(&dir, "session-")?;
    let mut done = 0;
    let mut completed = 0;
    let mut batches = 0;
    log_event(
        app,
        "info",
        "Сессия запущена. Автоматическая остановка через 8 часов. Зачёт Discord не проверяется.",
    );
    while !pool.is_empty() && !cancelled(app) && !expired(start) {
        while paused(app) && !cancelled(app) && !expired(start) {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        if cancelled(app) || expired(start) {
            break;
        }
        if config.cooldown_enabled && batches > 0 && batches % config.cooldown_every_n_batches == 0
        {
            wait(app, Duration::from_secs(config.cooldown_seconds), start).await;
            if cancelled(app) || expired(start) {
                break;
            }
        }
        let size = if farm {
            pool.len().min(4)
        } else {
            config.batch_size
        };
        let mut names = HashSet::new();
        let mut batch = Vec::new();
        let mut i = 0;
        while i < pool.len() && batch.len() < size {
            let key = pool[i]
                .rel_path
                .as_deref()
                .and_then(|p| Path::new(p).file_name())
                .map(|s| s.to_string_lossy().to_string());
            if key.is_none_or(|k| names.insert(k)) {
                batch.push(pool.remove(i));
            } else {
                i += 1;
            }
        }
        if batch.is_empty() {
            break;
        }
        let batch_dir = private_tempdir(session.path(), "batch-")?;
        let mut children = Vec::new();
        let mut rpc = Vec::new();
        let mut active = Vec::new();
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        for game in &batch {
            if cancelled(app) || expired(start) {
                break;
            }
            let info = ActiveGameInfo {
                id: game.id.clone(),
                name: game.name.clone(),
                executable: game.rel_path.clone(),
                pid: None,
                icon_hash: None,
                method: if game.method == SpoofMethod::Process {
                    "process"
                } else {
                    "rpc"
                }
                .into(),
            };
            if game.method == SpoofMethod::Process {
                match spawn_helper(batch_dir.path(), game) {
                    Ok(child) => {
                        let mut info = info;
                        info.pid = Some(child.0.id());
                        children.push((child, game.id.clone()));
                        active.push(info);
                    }
                    Err(e) => log_event(app, "warn", &format!("{}: {e}", game.name)),
                }
            } else {
                match DiscordRpcSession::connect(&game.id).await {
                    Ok(mut s) => {
                        match s.set_activity(&game.name, stamp, std::process::id()).await {
                            Ok(()) => {
                                rpc.push((s, game.id.clone()));
                                active.push(info);
                            }
                            Err(e) => log_event(app, "warn", &format!("{}: {e}", game.name)),
                        }
                    }
                    Err(e) => log_event(app, "warn", &format!("{}: {e}", game.name)),
                }
            }
        }
        if active.is_empty() {
            return Err("Ни одну активность запустить не удалось. Подробности в журнале.".into());
        }
        let hold = Duration::from_secs(if farm {
            config.farm_duration_secs.unwrap_or(3600)
        } else {
            config.hold_duration_secs
                + if config.jitter_enabled {
                    1 + batches as u64 % 3
                } else {
                    0
                }
        });
        log_event(
            app,
            "info",
            &format!(
                "Активны: {}",
                active
                    .iter()
                    .map(|g| g.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        );
        let mut held = Duration::ZERO;
        let mut tick = Instant::now();
        while held < hold && !cancelled(app) && !expired(start) {
            let step = Instant::now();
            for (child, id) in &mut children {
                if child.0.try_wait().map_err(|e| e.to_string())?.is_some() {
                    active.retain(|a| &a.id != id);
                }
            }
            for (session, id) in &mut rpc {
                if active.iter().any(|g| &g.id == id) {
                    if let Err(e) = session.poll_connection().await {
                        active.retain(|g| &g.id != id);
                        log_event(app, "warn", &format!("RPC {id}: {e}"));
                    }
                }
            }
            if active.is_empty() {
                break;
            }
            if tick.elapsed() >= Duration::from_millis(250) {
                emit_tick(
                    app,
                    active.clone(),
                    done,
                    total,
                    start,
                    hold.saturating_sub(held).as_secs(),
                );
                tick = Instant::now();
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
            // Pause holds scheduling; the current activity remains visible, as stated in UI.
            if !paused(app) {
                held += step.elapsed();
            }
        }
        let success = held >= hold && !cancelled(app) && !expired(start);
        let ids: Vec<_> = active.iter().map(|g| g.id.clone()).collect();
        drop(children); // kill + reap before removing executables
        for (mut s, _) in rpc {
            let _ = s.clear_activity(std::process::id()).await;
        }
        drop(batch_dir);
        if success && !farm {
            record_played_batch(&ids)?;
        }
        if success {
            completed += ids.len();
        }
        done += batch.len();
        batches += 1;
        emit_tick(app, Vec::new(), done, total, start, 0);
        if farm {
            break;
        }
        wait(app, Duration::from_secs(1), start).await;
    }
    emit_tick(app, Vec::new(), done, total, start, 0);
    log_event(app,"info",&format!("Сессия завершена. Полностью выдержали интервал: {completed}. Это локальная история, а не подтверждение значков Discord."));
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    fn game() -> SpoofableGame {
        SpoofableGame {
            id: "1".into(),
            name: "Test".into(),
            rel_path: Some("bin/test-game".into()),
            arguments: None,
            icon_hash: None,
            method: SpoofMethod::Process,
        }
    }
    #[test]
    fn helper_uses_basename_and_dies_on_drop() {
        let d = tempfile::tempdir().unwrap();
        let c = spawn_helper(d.path(), &game()).unwrap();
        let pid = c.0.id();
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(
            fs::read_to_string(format!("/proc/{pid}/comm"))
                .unwrap()
                .trim(),
            "test-game"
        );
        assert!(fs::read_to_string(format!("/proc/{pid}/status"))
            .unwrap()
            .contains("NoNewPrivs:\t1"));
        // Discord must be able to inspect exe, not just comm.
        assert!(fs::read_link(format!("/proc/{pid}/exe")).is_ok());
        drop(c);
        assert!(!Path::new(&format!("/proc/{pid}")).exists());
    }
    #[test]
    fn rejects_cache_traversal_at_spawn() {
        let d = tempfile::tempdir().unwrap();
        let mut g = game();
        g.rel_path = Some("../escape".into());
        assert!(spawn_helper(d.path(), &g).is_err());
    }
    #[test]
    fn windows_name_runs_our_linux_helper() {
        let d = tempfile::tempdir().unwrap();
        let mut g = game();
        g.rel_path = Some("game/eldenring.exe".into());
        let c = spawn_helper(d.path(), &g).unwrap();
        let pid = c.0.id();
        std::thread::sleep(Duration::from_millis(100));
        let path = fs::read_link(format!("/proc/{pid}/exe")).unwrap();
        assert_eq!(path, d.path().join("game/eldenring.exe"));
        assert_eq!(&fs::read(&path).unwrap()[..4], b"\x7fELF");
        assert_eq!(
            fs::read_to_string(format!("/proc/{pid}/comm"))
                .unwrap()
                .trim(),
            "eldenring.exe"
        );
        drop(c);
        assert!(!Path::new(&format!("/proc/{pid}")).exists());
    }
}
