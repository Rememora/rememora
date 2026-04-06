// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;

use commands::DbState;
use std::sync::Mutex;

fn main() {
    let db_path = rememora::db::default_db_path();

    let conn = rememora::db::open(&db_path).expect("Failed to open rememora database");

    tauri::Builder::default()
        .manage(DbState(Mutex::new(conn)))
        .invoke_handler(tauri::generate_handler![
            commands::get_projects,
            commands::get_memories,
            commands::get_memory_detail,
            commands::search_memories,
            commands::get_dashboard_stats,
            commands::get_sessions,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
