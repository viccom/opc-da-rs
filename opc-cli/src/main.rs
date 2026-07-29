#![forbid(unsafe_code)]
//! # opc-cli
//!
//! Interactive TUI for browsing, reading, and writing OPC DA tags on Windows.
//!
//! ## Overview
//!
//! This binary crate implements the user-facing CLI and interactive TUI
//! (Terminal User Interface) for the workspace. It initializes the OPC DA
//! client, manages the terminal lifecycle using `ratatui` and `crossterm`,
//! and runs the primary input-event and render loops.

mod app;
mod ui;

use crate::app::{App, CurrentScreen};
use anyhow::Result;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use opc_da_client::{ComConnector, OpcDaClient};
use ratatui::{Terminal, backend::CrosstermBackend};
use std::{io, sync::Arc, time::Duration};
use tracing_subscriber::{EnvFilter, fmt, prelude::*};

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize logging
    let file_appender = tracing_appender_localtime::rolling::daily("logs", "opc-cli.log");
    let (non_blocking, _guard) = tracing_appender_localtime::non_blocking(file_appender);

    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("debug"));

    tracing_subscriber::registry()
        .with(
            fmt::layer()
                .with_writer(non_blocking)
                .with_ansi(false)
                .with_filter(filter),
        )
        .init();

    tracing::info!("Starting OPC CLI");

    // COM initialization is handled transparently by the OpcDaClient worker thread.

    // Create OPC client BEFORE entering TUI mode so init errors are visible
    let opc_wrapper = Arc::new(OpcDaClient::new(ComConnector::default())?);

    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Create app and run it
    let mut app = App::new(opc_wrapper);
    let res = run_app(&mut terminal, &mut app);

    // Restore terminal
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    if let Err(err) = res {
        tracing::error!(error = ?err, "Application error");
    }

    Ok(())
}

fn run_app<B: ratatui::backend::Backend>(
    terminal: &mut Terminal<B>,
    app: &mut App,
) -> io::Result<()> {
    // Clear any leftover events (like the Enter key used to start the app)
    while event::poll(Duration::from_millis(0))? {
        let _ = event::read()?;
    }

    loop {
        app.poll_fetch_result();
        app.poll_browse_result();
        app.poll_read_result();
        app.poll_write_result();
        app.maybe_auto_refresh();

        terminal.draw(|f| ui::render(f, app))?;

        if event::poll(Duration::from_millis(100))?
            && let Event::Key(key) = event::read()?
        {
            handle_key_event(app, key);
        }

        if app.current_screen == CurrentScreen::Exiting {
            return Ok(());
        }
    }
}

fn handle_key_event(app: &mut App, key: event::KeyEvent) {
    if key.kind != event::KeyEventKind::Press {
        return;
    }

    match app.current_screen {
        CurrentScreen::Home => match key.code {
            KeyCode::Enter => {
                app.start_fetch_servers();
            }
            KeyCode::Char(c) => {
                app.host_input.push(c);
            }
            KeyCode::Backspace => {
                app.host_input.pop();
            }
            KeyCode::Esc => {
                app.current_screen = CurrentScreen::Exiting;
            }
            _ => {}
        },
        CurrentScreen::ServerList => match key.code {
            KeyCode::Esc => app.go_back(),
            KeyCode::PageDown => app.page_down(),
            KeyCode::PageUp => app.page_up(),
            KeyCode::Down => app.select_next(),
            KeyCode::Up => app.select_prev(),
            KeyCode::Enter => {
                app.start_browse_tags();
            }
            KeyCode::Char('q' | 'Q') => {
                app.current_screen = CurrentScreen::Exiting;
            }
            _ => {}
        },
        CurrentScreen::TagList => {
            if app.search_mode {
                match key.code {
                    KeyCode::Esc => app.exit_search_mode(),
                    KeyCode::Backspace => app.search_backspace(),
                    KeyCode::Tab => app.next_search_match(),
                    KeyCode::BackTab => app.prev_search_match(),
                    KeyCode::Char(' ') => app.toggle_tag_selection(),
                    KeyCode::Enter => {
                        app.exit_search_mode();
                        app.start_read_values();
                    }
                    KeyCode::Char(c) => app.update_search_query(c),
                    _ => {}
                }
            } else {
                match key.code {
                    KeyCode::Esc => app.go_back(),
                    KeyCode::PageDown => app.page_down(),
                    KeyCode::PageUp => app.page_up(),
                    KeyCode::Down => app.select_next(),
                    KeyCode::Up => app.select_prev(),
                    KeyCode::Char(' ') => app.toggle_tag_selection(),
                    KeyCode::Char('s' | 'S') => app.enter_search_mode(),
                    KeyCode::Enter => app.start_read_values(),
                    KeyCode::Char('q' | 'Q') => {
                        app.current_screen = CurrentScreen::Exiting;
                    }
                    _ => {}
                }
            }
        }
        CurrentScreen::TagValues => match key.code {
            KeyCode::Esc => app.go_back(),
            KeyCode::PageDown => app.page_down(),
            KeyCode::PageUp => app.page_up(),
            KeyCode::Down => app.select_next(),
            KeyCode::Up => app.select_prev(),
            KeyCode::Char('w' | 'W') => app.enter_write_mode(),
            KeyCode::Char('q' | 'Q') => {
                app.current_screen = CurrentScreen::Exiting;
            }
            _ => {}
        },
        CurrentScreen::WriteInput => match key.code {
            KeyCode::Enter => app.start_write_value(),
            KeyCode::Esc => app.go_back(),
            KeyCode::Char(c) => app.write_value_input.push(c),
            KeyCode::Backspace => {
                app.write_value_input.pop();
            }
            _ => {}
        },
        CurrentScreen::Loading => {
            if key.code == KeyCode::Esc {
                app.go_back();
            }
        }
        CurrentScreen::Exiting => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
    use opc_da_client::MockOpcProvider;

    #[test]
    fn test_handle_key_event_press_release() {
        let mock = MockOpcProvider::new();
        let mut app = App::new(Arc::new(mock));

        // 1. Simulate Press 'a'
        let press_a = KeyEvent {
            code: KeyCode::Char('a'),
            modifiers: KeyModifiers::empty(),
            kind: KeyEventKind::Press,
            state: KeyEventState::empty(),
        };
        handle_key_event(&mut app, press_a);
        assert_eq!(app.host_input, "localhosta");

        // 2. Simulate Release 'b' (should be ignored)
        let release_b = KeyEvent {
            code: KeyCode::Char('b'),
            modifiers: KeyModifiers::empty(),
            kind: KeyEventKind::Release,
            state: KeyEventState::empty(),
        };
        handle_key_event(&mut app, release_b);
        assert_eq!(app.host_input, "localhosta"); // Still 'a', 'b' ignored
    }

    #[test]
    fn test_quit_logic_on_all_screens() {
        let mock = MockOpcProvider::new();
        let mut app = App::new(Arc::new(mock));

        let quit_q = KeyEvent {
            code: KeyCode::Char('q'),
            modifiers: KeyModifiers::empty(),
            kind: KeyEventKind::Press,
            state: KeyEventState::empty(),
        };

        let esc = KeyEvent {
            code: KeyCode::Esc,
            modifiers: KeyModifiers::empty(),
            kind: KeyEventKind::Press,
            state: KeyEventState::empty(),
        };

        // 1. Home Screen: Esc quits, 'q' does NOT quit (it's input)
        app.current_screen = CurrentScreen::Home;
        handle_key_event(&mut app, quit_q);
        assert_eq!(app.current_screen, CurrentScreen::Home);
        assert!(app.host_input.ends_with('q'));

        handle_key_event(&mut app, esc);
        assert_eq!(app.current_screen, CurrentScreen::Exiting);

        // 2. Server List: 'q' quits
        app.current_screen = CurrentScreen::ServerList;
        handle_key_event(&mut app, quit_q);
        assert_eq!(app.current_screen, CurrentScreen::Exiting);

        // 3. Tag List: 'q' quits
        app.current_screen = CurrentScreen::TagList;
        handle_key_event(&mut app, quit_q);
        assert_eq!(app.current_screen, CurrentScreen::Exiting);
    }
}
