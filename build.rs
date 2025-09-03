use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    // Register our custom cfg flag with cargo
    println!("cargo::rustc-check-cfg=cfg(opencv_has_algorithm_hint)");

    // Only run OpenCV version detection if the opencv feature is enabled
    if env::var("CARGO_FEATURE_OPENCV").is_ok() {
        // Check for macOS libclang issues
        check_macos_libclang();
        detect_opencv_version();
    }

    // Run tauri-build when building with Tauri feature enabled
    if env::var("CARGO_FEATURE_TAURI").is_ok() {
        tauri_build::build()
    }

    // Build and embed the React app
    build_react_app();
}

#[cfg(target_os = "macos")]
fn check_macos_libclang() {
    // Check if DYLD_FALLBACK_LIBRARY_PATH is set
    if env::var("DYLD_FALLBACK_LIBRARY_PATH").is_err() {
        eprintln!("cargo:warning=================================================================");
        eprintln!("cargo:warning=macOS libclang setup required for OpenCV!");
        eprintln!("cargo:warning=");
        eprintln!("cargo:warning=Please run one of these commands before building:");
        eprintln!("cargo:warning=");
        eprintln!("cargo:warning=For Xcode:");
        eprintln!("cargo:warning=  export DYLD_FALLBACK_LIBRARY_PATH=\"$(xcode-select --print-path)/Toolchains/XcodeDefault.xctoolchain/usr/lib/\"");
        eprintln!("cargo:warning=");
        eprintln!("cargo:warning=For Command Line Tools only:");
        eprintln!("cargo:warning=  export DYLD_FALLBACK_LIBRARY_PATH=\"$(xcode-select --print-path)/usr/lib/\"");
        eprintln!("cargo:warning=");
        eprintln!("cargo:warning=Or add to your shell profile (~/.zshrc or ~/.bash_profile)");
        eprintln!("cargo:warning=================================================================");
    }
}

#[cfg(not(target_os = "macos"))]
fn check_macos_libclang() {
    // No-op on non-macOS platforms
}

fn detect_opencv_version() {
    // Try to get OpenCV version using pkg-config
    let output = Command::new("pkg-config")
        .args(["--modversion", "opencv4"])
        .output();

    // If opencv4 isn't found, try opencv
    let version = match output {
        Ok(output) if output.status.success() => {
            String::from_utf8_lossy(&output.stdout).trim().to_string()
        }
        _ => {
            // Try plain opencv
            let output = Command::new("pkg-config")
                .args(["--modversion", "opencv"])
                .output();

            match output {
                Ok(output) if output.status.success() => {
                    String::from_utf8_lossy(&output.stdout).trim().to_string()
                }
                _ => {
                    // If we can't detect version, assume newer version
                    println!("cargo:rustc-cfg=opencv_has_algorithm_hint");
                    eprintln!("cargo:warning=Could not detect OpenCV version, assuming newer");
                    return;
                }
            }
        }
    };

    eprintln!("cargo:warning=Detected OpenCV version: {}", version);

    // Parse the version
    let parts: Vec<&str> = version.split('.').collect();
    if parts.len() >= 2 {
        let major = parts[0].parse::<u32>().unwrap_or(0);
        let minor = parts[1].parse::<u32>().unwrap_or(0);

        // AlgorithmHint was added in OpenCV 4.11+ (not present in 4.10)
        // We're being conservative and only enabling for 4.12+ where we know it exists
        if major > 4 || (major == 4 && minor >= 12) {
            println!("cargo:rustc-cfg=opencv_has_algorithm_hint");
            eprintln!(
                "cargo:warning=OpenCV {} detected, enabling algorithm_hint feature",
                version
            );
        } else {
            eprintln!(
                "cargo:warning=OpenCV {} detected, algorithm_hint not available",
                version
            );
        }
    }
}

fn build_react_app() {
    let cargo_manifest_dir = env::var("CARGO_MANIFEST_DIR").unwrap();
    let static_dir = PathBuf::from(&cargo_manifest_dir).join("static");
    let dist_dir = static_dir.join("dist");

    // Tell cargo to rerun if any file in static/src changes
    println!("cargo:rerun-if-changed=static/src");
    println!("cargo:rerun-if-changed=static/package.json");
    println!("cargo:rerun-if-changed=static/package-lock.json");
    println!("cargo:rerun-if-changed=static/vite.config.ts");
    println!("cargo:rerun-if-changed=static/tsconfig.json");
    println!("cargo:rerun-if-changed=static/index.html");

    // Check if we're in a development environment or CI
    let is_dev = env::var("PROFILE").unwrap_or_default() == "debug";
    let skip_build = env::var("PSF_GUARD_SKIP_FRONTEND_BUILD").is_ok();

    if skip_build {
        eprintln!(
            "cargo:warning=Skipping frontend build due to PSF_GUARD_SKIP_FRONTEND_BUILD env var"
        );
        return;
    }

    // Check if static directory exists
    if !static_dir.exists() {
        eprintln!(
            "cargo:warning=Static directory not found at {:?}, skipping frontend build",
            static_dir
        );
        return;
    }

    // Check if dist directory already exists and is newer than source files in development
    if is_dev && dist_dir.exists() && is_dist_newer_than_sources(&static_dir, &dist_dir) {
        eprintln!("cargo:warning=Frontend dist is up to date, skipping build");
        return;
    }

    eprintln!("cargo:warning=Building React frontend...");

    // Use npm.cmd on Windows, npm on other platforms
    let npm_cmd = if cfg!(target_os = "windows") {
        "npm.cmd"
    } else {
        "npm"
    };

    // Check if node_modules exists, if not run npm install
    let node_modules = static_dir.join("node_modules");
    if !node_modules.exists() {
        eprintln!("cargo:warning=node_modules not found, running npm install...");
        let install_output = Command::new(npm_cmd)
            .args(["install"])
            .current_dir(&static_dir)
            .output();

        match install_output {
            Ok(output) => {
                if !output.status.success() {
                    eprintln!("cargo:warning=npm install failed:");
                    eprintln!(
                        "cargo:warning=stdout: {}",
                        String::from_utf8_lossy(&output.stdout)
                    );
                    eprintln!(
                        "cargo:warning=stderr: {}",
                        String::from_utf8_lossy(&output.stderr)
                    );
                    panic!("npm install failed");
                }
                eprintln!("cargo:warning=npm install completed successfully");
            }
            Err(e) => {
                eprintln!("cargo:warning=Failed to run npm install: {}", e);
                eprintln!("cargo:warning=Make sure Node.js and npm are installed and in PATH");
                if cfg!(target_os = "windows") {
                    eprintln!("cargo:warning=On Windows, npm might need to be in your PATH.");
                    eprintln!(
                        "cargo:warning=Try running 'where npm' in a command prompt to check."
                    );
                }
                panic!("Could not execute npm install");
            }
        }
    }

    // Run npm build
    let output = Command::new(npm_cmd)
        .args(["run", "build"])
        .current_dir(&static_dir)
        .output();

    match output {
        Ok(output) => {
            if output.status.success() {
                eprintln!("cargo:warning=Frontend build completed successfully");
            } else {
                eprintln!("cargo:warning=Frontend build failed:");
                eprintln!(
                    "cargo:warning=stdout: {}",
                    String::from_utf8_lossy(&output.stdout)
                );
                eprintln!(
                    "cargo:warning=stderr: {}",
                    String::from_utf8_lossy(&output.stderr)
                );
                panic!("Frontend build failed");
            }
        }
        Err(e) => {
            eprintln!("cargo:warning=Failed to run npm build: {}", e);
            eprintln!("cargo:warning=Make sure Node.js and npm are installed and in PATH");
            if cfg!(target_os = "windows") {
                eprintln!("cargo:warning=On Windows, npm might need to be in your PATH.");
                eprintln!("cargo:warning=Try running 'where npm' in a command prompt to check.");
            }
            panic!("Could not execute npm build");
        }
    }
}

fn is_dist_newer_than_sources(static_dir: &Path, dist_dir: &Path) -> bool {
    use std::fs;

    // Get the modification time of the dist directory
    let dist_time = match fs::metadata(dist_dir).and_then(|m| m.modified()) {
        Ok(time) => time,
        Err(_) => return false, // If we can't get dist time, rebuild
    };

    // Check if any source file is newer than dist
    let src_dir = static_dir.join("src");
    if let Ok(entries) = fs::read_dir(&src_dir) {
        for entry in entries.flatten() {
            if let Ok(metadata) = entry.metadata() {
                if let Ok(modified) = metadata.modified() {
                    if modified > dist_time {
                        return false; // Source is newer, need to rebuild
                    }
                }
            }
        }
    }

    // Also check package.json and other config files
    let config_files = [
        "package.json",
        "package-lock.json",
        "vite.config.ts",
        "tsconfig.json",
    ];
    for file in &config_files {
        let file_path = static_dir.join(file);
        if let Ok(metadata) = fs::metadata(&file_path) {
            if let Ok(modified) = metadata.modified() {
                if modified > dist_time {
                    return false; // Config file is newer, need to rebuild
                }
            }
        }
    }

    true // All source files are older than dist
}
