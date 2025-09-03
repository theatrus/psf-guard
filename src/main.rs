#[cfg(feature = "tauri")]
use std::env;

#[cfg(feature = "tauri")]
fn main() {
    // Check if any arguments are passed (excluding the binary name)
    let args: Vec<String> = env::args().collect();

    if args.len() > 1 {
        // Arguments provided - run CLI mode
        if let Err(e) = psf_guard::cli_main::main() {
            eprintln!("Error: {}", e);
            std::process::exit(1);
        }
    } else {
        // No arguments - run GUI mode
        psf_guard::tauri_main::main();
    }
}

#[cfg(not(feature = "tauri"))]
fn main() -> anyhow::Result<()> {
    // Always run CLI mode when tauri feature is not enabled
    psf_guard::cli_main::main()
}
