#[cfg(not(target_os = "linux"))]
compile_error!("Mimic Linux supports Linux only");
pub mod commands;
pub mod discord;
pub mod history;
pub mod i18n;
pub mod rpc;
pub mod security;
pub mod spoofer;
pub mod state;
use state::AppState;
use tauri::Manager;
pub fn run() {
    if unsafe { libc::geteuid() } == 0 {
        eprintln!("Mimic Linux: запуск от root запрещён.");
        std::process::exit(1);
    }
    let dir = security::data_dir().unwrap_or_else(|e| {
        eprintln!("Mimic Linux: {e}");
        std::process::exit(1)
    });
    let _instance = security::InstanceLock::acquire(&dir).unwrap_or_else(|e| {
        eprintln!("Mimic Linux: {e}");
        std::process::exit(1)
    });
    tauri::Builder::default()
        .manage(AppState::new())
        .invoke_handler(tauri::generate_handler![
            commands::fetch_games,
            commands::get_games_list,
            commands::start_spoofer,
            commands::pause_spoofer,
            commands::resume_spoofer,
            commands::stop_spoofer,
            commands::check_discord_status,
            commands::get_spoofer_state,
            commands::clear_history,
            commands::export_history,
            commands::import_history,
            commands::set_language
        ])
        .build(tauri::generate_context!())
        .expect("Cannot build Mimic Linux")
        .run(|app, event| {
            if matches!(event, tauri::RunEvent::Exit) {
                let _ =
                    tauri::async_runtime::block_on(commands::shutdown(&app.state::<AppState>()));
            }
        });
}
