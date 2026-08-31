use anyhow::Result;

pub fn run() -> Result<()> {
    println!("Rememora Desktop (Tauri v2, macOS)");
    println!();
    println!("The desktop viewer is a separate Tauri project in the `app/` directory.");
    println!("It opens the encrypted DB read-only and never prompts for the key.");
    println!();
    println!("To launch in development mode:");
    println!();
    println!("  cd app");
    println!("  pnpm install");
    println!("  pnpm tauri dev");
    println!();
    println!("To build a distributable bundle (.app + .dmg, unsigned):");
    println!();
    println!("  cd app");
    println!("  pnpm tauri build");
    println!();
    println!("Prerequisites:");
    println!("  - Rust toolchain (stable)");
    println!("  - Node 22+ and pnpm 9+ (for the frontend)");
    println!("  - Xcode Command Line Tools (for linking on macOS)");
    println!();
    println!("If the app reports \"Encryption key not available\", run `rememora init`");
    println!("first so the key is resolvable from the environment or OS keychain.");
    Ok(())
}
