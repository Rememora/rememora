mod commands;

use commands::AppState;

/// Entry point invoked from `main.rs`.
///
/// Opens the encrypted Rememora SQLite DB using the CLI's keychain-derived
/// key (never prompts) and hands it to Tauri via managed state. Any failure
/// at open time is captured in `AppState` so the UI can render a targeted
/// error message — the window still launches so users are not left staring
/// at a dead dock icon.
pub fn run() {
    let state = AppState::initialise();

    tauri::Builder::default()
        .manage(state)
        .invoke_handler(tauri::generate_handler![
            commands::get_db_status,
            commands::list_contexts,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
