use anyhow::Result;

pub fn run() -> Result<()> {
    println!("Rememora Desktop (Tauri v2)");
    println!();
    println!("The desktop app is a separate Tauri project in the `desktop/` directory.");
    println!();
    println!("To launch in development mode:");
    println!();
    println!("  cd desktop");
    println!("  pnpm install");
    println!("  cargo tauri dev");
    println!();
    println!("To build a distributable binary:");
    println!();
    println!("  cd desktop");
    println!("  cargo tauri build");
    println!();
    println!("Prerequisites:");
    println!("  - Node.js and pnpm (for the frontend)");
    println!("  - cargo-tauri CLI: cargo install tauri-cli --version '^2'");
    Ok(())
}
