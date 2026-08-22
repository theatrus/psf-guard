/// Debug logging utilities for PSF Guard
use std::sync::atomic::{AtomicBool, Ordering};

/// Global debug flag - can be set by CLI or environment
static DEBUG_ENABLED: AtomicBool = AtomicBool::new(false);

/// Initialize debug mode
pub fn init_debug(verbose: bool) {
    DEBUG_ENABLED.store(verbose, Ordering::Relaxed);
    // The detectors moved to seiza-stars with their own gate of the same
    // shape; one verbose flag turns on both, as it always did.
    seiza_stars::debug::init_debug(verbose);
}

/// Check if debug mode is enabled
pub fn is_debug_enabled() -> bool {
    DEBUG_ENABLED.load(Ordering::Relaxed)
}

/// Debug print macro - only prints if debug is enabled
#[macro_export]
macro_rules! debug_print {
    ($($arg:tt)*) => {
        if $crate::debug::is_debug_enabled() {
            eprintln!("DEBUG: {}", format!($($arg)*));
        }
    }
}

/// Debug print for MTF stretching details
#[macro_export]
macro_rules! debug_mtf {
    ($($arg:tt)*) => {
        if $crate::debug::is_debug_enabled() {
            eprintln!("MTF: {}", format!($($arg)*));
        }
    }
}

/// Debug print for star detection pipeline
#[macro_export]
macro_rules! debug_detection {
    ($($arg:tt)*) => {
        if $crate::debug::is_debug_enabled() {
            eprintln!("DETECT: {}", format!($($arg)*));
        }
    }
}

/// Debug print for blob analysis
#[macro_export]
macro_rules! debug_blob {
    ($($arg:tt)*) => {
        if $crate::debug::is_debug_enabled() {
            eprintln!("BLOB: {}", format!($($arg)*));
        }
    }
}

/// Always print important information (not affected by debug flag)
#[macro_export]
macro_rules! info_print {
    ($($arg:tt)*) => {
        println!("{}", format!($($arg)*));
    }
}
