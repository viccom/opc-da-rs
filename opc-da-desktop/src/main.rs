//! `opc-da-desktop` binary entry point.
//!
//! Initializes logging, constructs the Tauri builder, and registers all
//! `#[tauri::command]` IPC handlers. On non-Windows the binary is a stub
//! that prints a friendly message — the COM stack cannot run off Windows.

#![cfg_attr(not(windows), allow(unused_imports))]

#[cfg(windows)]
fn main() {
    opc_da_desktop::app::run();
}

#[cfg(not(windows))]
fn main() {
    eprintln!(
        "opc-da-desktop requires Windows to run (uses COM/DCOM).\n\
         On this platform, only `cargo check --workspace` succeeds."
    );
    std::process::exit(2);
}
