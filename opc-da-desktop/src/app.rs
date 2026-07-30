//! Tauri `Builder` setup. Only compiled on Windows (COM/DCOM runtime).

use tauri::Manager;

use crate::state::AppState;

/// Initialize logging, build the Tauri app, register IPC handlers, run.
///
/// # Panics
///
/// Panics if the OPC COM worker fails to start (`AppState::new`), or if
/// `tauri::generate_context!` cannot run the app (e.g. missing frontend dist).
pub fn run() {
    init_tracing();

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState::new().expect("failed to start OPC COM worker"))
        .setup(|app| {
            // Keep a handle to the main window for future direct access
            // (e.g. attaching menu items / accelerators).
            let _main = app.get_webview_window("main");
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            crate::commands::servers::list_servers,
            crate::commands::servers::connect,
            crate::commands::servers::disconnect,
            crate::commands::browse::browse_tags,
            crate::commands::browse::browse_children,
            crate::commands::read::read_tag_values,
            crate::commands::write::write_tag_value,
            crate::commands::subscription::subscribe_tags,
            crate::commands::subscription::unsubscribe_tags,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

/// Initialize tracing with a daily-rolling file appender + env filter.
fn init_tracing() {
    use tracing_subscriber::{EnvFilter, fmt, prelude::*};

    let file_appender = tracing_appender_localtime::rolling::daily(
        std::path::Path::new("logs"),
        "opc-da-desktop.log",
    );
    let (file_writer, guard) = tracing_appender_localtime::non_blocking(file_appender);

    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,opc_da_client=info,opc_da_desktop=info"));

    tracing_subscriber::registry()
        .with(filter)
        .with(fmt::layer().with_writer(file_writer).with_ansi(false))
        .init();

    // Keep the guard alive for the lifetime of the process.
    Box::leak(Box::new(guard));
}
