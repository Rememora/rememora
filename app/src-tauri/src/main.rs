// Prevents an additional console window on Windows in release. The app is
// macOS-only in v0, but this line is standard boilerplate and harmless.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    rememora_app_lib::run();
}
