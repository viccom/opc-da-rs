//! # app
//!
//! Main application state manager and event handler for the OPC DA client TUI.
//!
//! ## Overview
//!
//! This module defines the state structures ([`App`]) and screen state representations
//! ([`CurrentScreen`]) driving the TUI layout, handling user inputs, managing the list selection
//! states, and communicating asynchronously with the background OPC DA client provider.

use opc_da_client::{OpcError, OpcProvider, OpcValue, TagValue, WriteResult, friendly_com_hint};
use ratatui::widgets::{ListState, TableState}; // Added TableState
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use tokio::sync::oneshot;

/// Default timeout for OPC operations (server listing and tag browsing).
const OPC_TIMEOUT_SECS: u64 = 300;

/// Maximum tags to retrieve when browsing an OPC server namespace.
const MAX_BROWSE_TAGS: usize = 10000;

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum CurrentScreen {
    Home,
    Loading,
    ServerList,
    TagList,
    TagValues,
    WriteInput,
    Exiting,
}

/// Main application state for the OPC DA Client TUI.
///
/// Manages the current screen, loaded servers and tags, search state,
/// and terminal interaction through `ratatui`.
pub struct App {
    pub host_input: String,
    pub servers: Vec<String>,
    pub tags: Vec<String>,
    pub selected_index: Option<usize>,
    pub current_screen: CurrentScreen,
    pub opc_provider: Arc<dyn OpcProvider>,
    pub messages: Vec<String>,
    pub list_state: ListState,
    pub table_state: TableState, // New field
    pub browse_progress: Arc<AtomicUsize>,
    pub browse_result_rx: Option<oneshot::Receiver<Result<Vec<String>, OpcError>>>,
    pub fetch_result_rx: Option<oneshot::Receiver<Result<Vec<String>, OpcError>>>,
    pub selected_tags: Vec<bool>,
    pub tag_values: Vec<TagValue>,
    pub read_result_rx: Option<oneshot::Receiver<Result<Vec<TagValue>, OpcError>>>,
    /// Context for auto-refresh: server used for the last read.
    pub refresh_server: Option<String>,
    /// Context for auto-refresh: tag IDs from the last read.
    pub refresh_tag_ids: Vec<String>,
    /// Tracks when the last successful read completed.
    pub last_read_time: Option<std::time::Instant>,
    /// Whether the tag list is in search/filter mode.
    pub search_mode: bool,
    /// Current search query string.
    pub search_query: String,
    /// Indices into `self.tags` that match the current query.
    pub search_matches: Vec<usize>,
    /// Current position within `search_matches` (cycles).
    pub search_match_index: usize,

    /// The tag currently being edited for writing.
    pub write_tag_id: Option<String>,
    /// User-entered value string for writing.
    pub write_value_input: String,
    /// Receiver for background write result.
    pub write_result_rx: Option<oneshot::Receiver<Result<WriteResult, OpcError>>>,
    /// The server `ProgID` that was used for the current tag browse.
    pub browsed_server: Option<String>,
}

impl App {
    /// Create a new `App` instance with the given OPC provider.
    pub fn new(opc_provider: Arc<dyn OpcProvider>) -> Self {
        Self {
            host_input: "localhost".into(),
            servers: Vec::new(),
            tags: Vec::new(),
            selected_index: None,
            current_screen: CurrentScreen::Home,
            opc_provider,
            messages: Vec::new(),
            list_state: ListState::default(),
            table_state: TableState::default(), // Initialize
            browse_progress: Arc::new(AtomicUsize::new(0)),
            browse_result_rx: None,
            fetch_result_rx: None,
            selected_tags: Vec::new(),
            tag_values: Vec::new(),
            read_result_rx: None,
            refresh_server: None,
            refresh_tag_ids: Vec::new(),
            last_read_time: None,
            search_mode: false,
            search_query: String::new(),
            search_matches: Vec::new(),
            search_match_index: 0,

            write_tag_id: None,
            write_value_input: String::new(),
            write_result_rx: None,
            browsed_server: None,
        }
    }

    pub fn add_message(&mut self, message: String) {
        self.messages.push(message);
        if self.messages.len() > 10 {
            self.messages.remove(0);
        }
    }

    // Actions
    pub fn start_fetch_servers(&mut self) {
        let host = self.host_input.clone();
        self.current_screen = CurrentScreen::Loading;
        self.add_message(format!("Connecting to {host}..."));

        let provider = Arc::clone(&self.opc_provider);
        let (tx, rx) = oneshot::channel();

        tokio::spawn(async move {
            let result = tokio::time::timeout(
                std::time::Duration::from_secs(OPC_TIMEOUT_SECS),
                provider.list_servers(&host),
            )
            .await;

            let final_result = result.unwrap_or_else(|_| {
                tracing::error!("Server listing timed out ({OPC_TIMEOUT_SECS}s)");
                Err(OpcError::Internal(format!(
                    "Connection timed out ({OPC_TIMEOUT_SECS}s)"
                )))
            });

            let _ = tx.send(final_result);
        });

        self.fetch_result_rx = Some(rx);
    }

    pub fn poll_fetch_result(&mut self) {
        if let Some(rx) = &mut self.fetch_result_rx {
            match rx.try_recv() {
                Ok(Ok(servers)) => {
                    self.servers = servers;
                    self.current_screen = CurrentScreen::ServerList;
                    if self.servers.is_empty() {
                        self.selected_index = None;
                        self.list_state.select(None);
                    } else {
                        self.selected_index = Some(0);
                        self.list_state.select(Some(0));
                    }
                    self.add_message(format!(
                        "Found {} servers on {}",
                        self.servers.len(),
                        self.host_input
                    ));
                    self.fetch_result_rx = None;
                }
                Ok(Err(e)) => {
                    self.current_screen = CurrentScreen::Home;
                    tracing::error!(error = %e, "Failed to fetch servers");
                    self.add_message(format!("Error fetching servers: {e}"));
                    self.fetch_result_rx = None;
                }
                Err(oneshot::error::TryRecvError::Empty) => {
                    // Still running
                }
                Err(oneshot::error::TryRecvError::Closed) => {
                    self.current_screen = CurrentScreen::Home;
                    tracing::error!(
                        "Server listing background task terminated unexpectedly (sender dropped)"
                    );
                    self.add_message("Server listing task terminated unexpectedly".into());
                    self.fetch_result_rx = None;
                }
            }
        }
    }

    pub fn select_next(&mut self) {
        let count = match self.current_screen {
            CurrentScreen::ServerList => self.servers.len(),
            CurrentScreen::TagList => self.tags.len(),
            CurrentScreen::TagValues => self.tag_values.len(),
            _ => 0,
        };

        if count == 0 {
            return;
        }

        if let Some(idx) = self.selected_index {
            if idx < count - 1 {
                let new_idx = idx + 1;
                self.selected_index = Some(new_idx);
                self.list_state.select(Some(new_idx));
                if self.current_screen == CurrentScreen::TagValues {
                    self.table_state.select(Some(new_idx));
                }
            }
        } else {
            self.selected_index = Some(0);
            self.list_state.select(Some(0));
        }
    }

    pub fn select_prev(&mut self) {
        if let Some(idx) = self.selected_index
            && idx > 0
        {
            let new_idx = idx - 1;
            self.selected_index = Some(new_idx);
            self.list_state.select(Some(new_idx));
            if self.current_screen == CurrentScreen::TagValues {
                self.table_state.select(Some(new_idx));
            }
        }
    }

    /// Jump forward by PAGE_SIZE items (clamped to end of list).
    pub fn page_down(&mut self) {
        let count = match self.current_screen {
            CurrentScreen::ServerList => self.servers.len(),
            CurrentScreen::TagList => self.tags.len(),
            CurrentScreen::TagValues => self.tag_values.len(),
            _ => 0,
        };

        if count == 0 {
            return;
        }

        let page_size = 20;
        if let Some(idx) = self.selected_index {
            let new_idx = (idx + page_size).min(count - 1);
            self.selected_index = Some(new_idx);
            self.list_state.select(Some(new_idx));
            if self.current_screen == CurrentScreen::TagValues {
                self.table_state.select(Some(new_idx));
            }
        } else {
            self.selected_index = Some(0);
            self.list_state.select(Some(0));
            if self.current_screen == CurrentScreen::TagValues {
                self.table_state.select(Some(0));
            }
        }
    }

    /// Jump backward by PAGE_SIZE items (clamped to start of list).
    pub fn page_up(&mut self) {
        let page_size = 20;
        if let Some(idx) = self.selected_index {
            let new_idx = idx.saturating_sub(page_size);
            self.selected_index = Some(new_idx);
            self.list_state.select(Some(new_idx));
            if self.current_screen == CurrentScreen::TagValues {
                self.table_state.select(Some(new_idx));
            }
        } else {
            self.selected_index = Some(0);
            self.list_state.select(Some(0));
            if self.current_screen == CurrentScreen::TagValues {
                self.table_state.select(Some(0));
            }
        }
    }

    pub fn start_browse_tags(&mut self) {
        if self.current_screen != CurrentScreen::ServerList {
            return;
        }

        let Some(idx) = self.selected_index else {
            return;
        };

        let server = match self.servers.get(idx) {
            Some(s) => s.clone(),
            None => return,
        };

        self.browsed_server = Some(server.clone());

        self.current_screen = CurrentScreen::Loading;
        self.browse_progress = Arc::new(AtomicUsize::new(0));
        self.add_message(format!("Browsing tags on {server}..."));

        let provider = Arc::clone(&self.opc_provider);
        let progress = Arc::clone(&self.browse_progress);
        let tags_sink = Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink_for_task = Arc::clone(&tags_sink);

        let (tx, rx) = oneshot::channel();

        tokio::spawn(async move {
            let timeout_duration = std::time::Duration::from_secs(OPC_TIMEOUT_SECS);
            let result = tokio::time::timeout(
                timeout_duration,
                provider.browse_tags(&server, MAX_BROWSE_TAGS, progress, sink_for_task, 0, 0),
            )
            .await;

            let final_result = match result {
                Ok(inner) => inner,
                Err(_) => {
                    // Timeout occurred. Harvest partial results from sink.
                    let partial_tags = if let Ok(sink) = tags_sink.lock() {
                        sink.clone()
                    } else {
                        Vec::new()
                    };

                    if !partial_tags.is_empty() {
                        tracing::warn!(
                            server = %server,
                            count = partial_tags.len(),
                            timeout_secs = OPC_TIMEOUT_SECS,
                            "Browse tags timed out; returning partial results"
                        );
                        Ok(partial_tags)
                    } else {
                        tracing::error!(
                            server = %server,
                            timeout_secs = OPC_TIMEOUT_SECS,
                            "Browse tags timed out with zero tags found"
                        );
                        Err(OpcError::Internal(format!(
                            "Browse timed out ({OPC_TIMEOUT_SECS}s) for '{server}' with no tags found"
                        )))
                    }
                }
            };

            let _ = tx.send(final_result);
        });

        self.browse_result_rx = Some(rx);
    }

    pub fn poll_browse_result(&mut self) {
        if let Some(rx) = &mut self.browse_result_rx {
            match rx.try_recv() {
                Ok(Ok(tags)) => {
                    self.tags = tags;
                    self.selected_tags = vec![false; self.tags.len()];
                    self.current_screen = CurrentScreen::TagList;
                    if self.tags.is_empty() {
                        self.selected_index = None;
                        self.list_state.select(None);
                    } else {
                        self.selected_index = Some(0);
                        self.list_state.select(Some(0));
                    }
                    self.add_message(format!("Found {} tags", self.tags.len()));
                    self.browse_result_rx = None;
                }
                Ok(Err(e)) => {
                    self.current_screen = CurrentScreen::ServerList;
                    tracing::error!(error = %e, error_chain = ?e, "Browse tags failed");
                    let hint = friendly_com_hint(&e);
                    let msg = match hint {
                        Some(h) => format!("Error: {} ({})", h, e),
                        None => format!("Error: {:#}", e),
                    };
                    self.add_message(msg);
                    self.browse_result_rx = None;
                }
                Err(oneshot::error::TryRecvError::Empty) => {
                    // Still running
                }
                Err(oneshot::error::TryRecvError::Closed) => {
                    self.current_screen = CurrentScreen::ServerList;
                    tracing::error!(
                        "Browse background task terminated unexpectedly (sender dropped)"
                    );
                    self.add_message("Browse task terminated unexpectedly".into());
                    self.browse_result_rx = None;
                }
            }
        }
    }

    /// Toggle tag selection at the current selected index.
    pub fn toggle_tag_selection(&mut self) {
        if self.current_screen != CurrentScreen::TagList {
            return;
        }
        if let Some(idx) = self.selected_index
            && idx < self.selected_tags.len()
            && let Some(tag) = self.tags.get(idx)
        {
            self.selected_tags[idx] = !self.selected_tags[idx];
            tracing::debug!(
                tag = %tag,
                selected = self.selected_tags[idx],
                "toggle_tag_selection"
            );
        }
    }

    /// Start reading values for selected tags.
    pub fn start_read_values(&mut self) {
        if self.current_screen != CurrentScreen::TagList {
            return;
        }

        // Gather selected tag IDs
        let selected_tag_ids: Vec<String> = self
            .tags
            .iter()
            .enumerate()
            .filter_map(|(idx, tag_id)| {
                if self.selected_tags.get(idx).copied().unwrap_or(false) {
                    Some(tag_id.clone())
                } else {
                    None
                }
            })
            .collect();

        if selected_tag_ids.is_empty() {
            tracing::debug!("start_read_values: no tags selected");
            self.add_message("No tags selected. Press Space to select tags.".into());
            return;
        }

        let server = match &self.browsed_server {
            Some(s) => s.clone(),
            None => {
                self.add_message("No server context — please browse tags first".into());
                return;
            }
        };

        // Store context for auto-refresh
        self.refresh_server = Some(server.clone());
        self.refresh_tag_ids.clone_from(&selected_tag_ids);

        tracing::info!(
            server = %server,
            count = selected_tag_ids.len(),
            tags = ?selected_tag_ids,
            "start_read_values: sending tags to backend"
        );
        self.current_screen = CurrentScreen::Loading;
        self.add_message(format!("Reading {} tag values...", selected_tag_ids.len()));

        let provider = Arc::clone(&self.opc_provider);
        let (tx, rx) = oneshot::channel();

        tokio::spawn(async move {
            let result = tokio::time::timeout(
                std::time::Duration::from_secs(OPC_TIMEOUT_SECS),
                provider.read_tag_values(&server, selected_tag_ids),
            )
            .await;

            let final_result = match result {
                Ok(inner) => inner,
                Err(_) => {
                    tracing::error!("Read tag values timed out ({OPC_TIMEOUT_SECS}s)");
                    Err(OpcError::Internal(format!(
                        "Read timed out ({OPC_TIMEOUT_SECS}s)"
                    )))
                }
            };

            let _ = tx.send(final_result);
        });

        self.read_result_rx = Some(rx);
    }

    pub fn poll_read_result(&mut self) {
        if let Some(rx) = &mut self.read_result_rx {
            match rx.try_recv() {
                Ok(Ok(values)) => {
                    self.tag_values = values;
                    self.current_screen = CurrentScreen::TagValues;
                    if self.tag_values.is_empty() {
                        self.selected_index = None;
                        self.table_state.select(None);
                    } else if let Some(idx) = self.selected_index {
                        // Preserve cursor position, clamping to new list bounds
                        let clamped = idx.min(self.tag_values.len() - 1);
                        self.selected_index = Some(clamped);
                        self.table_state.select(Some(clamped));
                    } else {
                        self.selected_index = Some(0);
                        self.table_state.select(Some(0));
                    }

                    // Check for per-item errors and push single summary to status log
                    let error_count = self
                        .tag_values
                        .iter()
                        .filter(|tv| tv.value == "Error")
                        .count();

                    if error_count > 0 {
                        self.add_message(format!(
                            "Read {} tag values (⚠ {} errors)",
                            self.tag_values.len(),
                            error_count
                        ));
                    } else {
                        self.add_message(format!("Read {} tag values", self.tag_values.len()));
                    }

                    self.last_read_time = Some(std::time::Instant::now());
                    self.read_result_rx = None;
                }
                Ok(Err(e)) => {
                    self.current_screen = CurrentScreen::TagList;
                    tracing::error!(error = %e, error_chain = ?e, "Read tag values failed");
                    let hint = friendly_com_hint(&e);
                    let msg = match hint {
                        Some(h) => format!("Error reading values: {} ({})", h, e),
                        None => format!("Error reading values: {:#}", e),
                    };
                    self.add_message(msg);
                    self.read_result_rx = None;
                }
                Err(oneshot::error::TryRecvError::Empty) => {
                    // Still running
                }
                Err(oneshot::error::TryRecvError::Closed) => {
                    self.current_screen = CurrentScreen::TagList;
                    tracing::error!(
                        "Read values background task terminated unexpectedly (sender dropped)"
                    );
                    self.add_message("Read task terminated unexpectedly".into());
                    self.read_result_rx = None;
                }
            }
        }
    }

    /// Enter write mode for a tag.
    ///
    /// Triggered from TagValues. If only one tag is displayed, it is auto-selected.
    /// If multiple are displayed, the currently highlighted row is used.
    pub fn enter_write_mode(&mut self) {
        if self.current_screen != CurrentScreen::TagValues {
            return;
        }

        let tag_id = if self.tag_values.len() == 1 {
            // Auto-select the only tag
            Some(self.tag_values[0].tag_id.clone())
        } else if let Some(idx) = self.table_state.selected() {
            // Use the highlighted row
            self.tag_values.get(idx).map(|tv| tv.tag_id.clone())
        } else {
            None
        };

        if let Some(id) = tag_id {
            tracing::debug!(tag_id = %id, "enter_write_mode: entering write mode for tag");
            self.write_tag_id = Some(id);
            self.write_value_input.clear();
            self.current_screen = CurrentScreen::WriteInput;
        } else {
            tracing::debug!("enter_write_mode: no tag selected");
            self.add_message("No tag selected to write.".into());
        }
    }

    /// Start writing a value to the selected tag.
    pub fn start_write_value(&mut self) {
        let tag_id = match &self.write_tag_id {
            Some(t) => t.clone(),
            None => return,
        };
        let value_str = self.write_value_input.trim().to_string();
        if value_str.is_empty() {
            self.add_message("Value cannot be empty.".into());
            return;
        }

        // Parse the value string into OpcValue (try int -> float -> bool -> string)
        let opc_value = parse_opc_value(&value_str);

        tracing::info!(tag = %tag_id, value = %value_str, parsed_type = ?opc_value, "start_write_value: initiating write");

        let server = match &self.refresh_server {
            Some(s) => s.clone(),
            None => {
                self.add_message("No server context for write.".into());
                return;
            }
        };

        self.current_screen = CurrentScreen::Loading;
        self.add_message(format!("Writing '{value_str}' to {tag_id}..."));

        let provider = Arc::clone(&self.opc_provider);
        let (tx, rx) = oneshot::channel();

        // Use a consistent timeout
        const OPC_TIMEOUT_SECS_WRITE: u64 = 10;

        tokio::spawn(async move {
            let result = tokio::time::timeout(
                std::time::Duration::from_secs(OPC_TIMEOUT_SECS_WRITE),
                provider.write_tag_value(&server, &tag_id, opc_value),
            )
            .await;

            let final_result = match result {
                Ok(inner) => inner,
                Err(_) => {
                    tracing::error!("Write tag value timed out ({OPC_TIMEOUT_SECS_WRITE}s)");
                    Err(OpcError::Internal(format!(
                        "Write timed out ({OPC_TIMEOUT_SECS_WRITE}s)"
                    )))
                }
            };
            let _ = tx.send(final_result);
        });

        self.write_result_rx = Some(rx);
    }

    /// Poll for the result of the background write operation.
    pub fn poll_write_result(&mut self) {
        if let Some(rx) = &mut self.write_result_rx {
            match rx.try_recv() {
                Ok(Ok(result)) => {
                    if result.success {
                        tracing::info!(tag = %result.tag_id, "poll_write_result: write succeeded");
                        self.add_message(format!("✓ Write to '{}' succeeded", result.tag_id));
                    } else {
                        let err_msg = result.error.unwrap_or_default();
                        self.add_message(format!(
                            "✗ Write to '{}' failed: {}",
                            result.tag_id, err_msg
                        ));
                    }
                    self.current_screen = CurrentScreen::TagValues;
                    self.write_result_rx = None;
                    // Trigger a refresh to show the new value
                    self.start_read_values();
                }
                Ok(Err(e)) => {
                    tracing::error!(error = %e, "Write tag values failed");
                    self.add_message(format!("Write error: {e:#}"));
                    self.current_screen = CurrentScreen::TagValues;
                    self.write_result_rx = None;
                }
                Err(oneshot::error::TryRecvError::Empty) => {}
                Err(oneshot::error::TryRecvError::Closed) => {
                    self.current_screen = CurrentScreen::TagValues;
                    tracing::error!("Write background task terminated unexpectedly");
                    self.add_message("Write task terminated unexpectedly".into());
                    self.write_result_rx = None;
                }
            }
        }
    }

    pub fn maybe_auto_refresh(&mut self) {
        if self.current_screen != CurrentScreen::TagValues {
            return;
        }
        if self.read_result_rx.is_some() {
            return; // Read already in-flight
        }
        let elapsed = match self.last_read_time {
            Some(t) => t.elapsed(),
            None => return,
        };
        if elapsed < std::time::Duration::from_secs(1) {
            return;
        }

        let server_name = match &self.refresh_server {
            Some(s) => s.clone(),
            None => return,
        };
        let tag_ids = self.refresh_tag_ids.clone();
        if tag_ids.is_empty() {
            return;
        }

        tracing::debug!(tag_count = tag_ids.len(), "Auto-refreshing tag values");
        let provider = Arc::clone(&self.opc_provider);
        let (tx, rx) = oneshot::channel();

        tokio::spawn(async move {
            let result = tokio::time::timeout(
                std::time::Duration::from_secs(OPC_TIMEOUT_SECS),
                provider.read_tag_values(&server_name, tag_ids),
            )
            .await;

            let final_result = match result {
                Ok(inner) => inner,
                Err(_) => {
                    tracing::error!("Auto-refresh timed out ({OPC_TIMEOUT_SECS}s)");
                    Err(OpcError::Internal(format!(
                        "Auto-refresh timed out ({OPC_TIMEOUT_SECS}s)"
                    )))
                }
            };

            let _ = tx.send(final_result);
        });

        self.read_result_rx = Some(rx);
    }

    /// Enter search mode, clearing any previous query.
    pub fn enter_search_mode(&mut self) {
        if self.current_screen != CurrentScreen::TagList {
            return;
        }
        self.search_mode = true;
        self.search_query.clear();
        self.search_matches.clear();
        self.search_match_index = 0;
    }

    /// Exit search mode, keeping cursor position.
    pub fn exit_search_mode(&mut self) {
        self.search_mode = false;
        // Keep Query string so user sees what they searched for if they enter again?
        // Actually, the plan said "clear any previous query" on enter, so it's fine.
    }

    /// Update the search query and recompute matches.
    pub fn update_search_query(&mut self, c: char) {
        self.search_query.push(c);
        self.recompute_search_matches();
    }

    /// Delete last character from search query and recompute.
    pub fn search_backspace(&mut self) {
        self.search_query.pop();
        self.recompute_search_matches();
    }

    fn recompute_search_matches(&mut self) {
        let query = self.search_query.to_lowercase();
        self.search_matches = self
            .tags
            .iter()
            .enumerate()
            .filter_map(|(idx, tag)| {
                if tag.to_lowercase().contains(&query) {
                    Some(idx)
                } else {
                    None
                }
            })
            .collect();

        self.search_match_index = 0;
        if let Some(&first_match) = self.search_matches.first() {
            self.selected_index = Some(first_match);
            self.list_state.select(Some(first_match));
        }
    }

    /// Jump to the next search match.
    pub fn next_search_match(&mut self) {
        if self.search_matches.is_empty() {
            return;
        }
        self.search_match_index = (self.search_match_index + 1) % self.search_matches.len();
        if let Some(&next_idx) = self.search_matches.get(self.search_match_index) {
            self.selected_index = Some(next_idx);
            self.list_state.select(Some(next_idx));
        }
    }

    /// Jump to the previous search match.
    pub fn prev_search_match(&mut self) {
        if self.search_matches.is_empty() {
            return;
        }
        if self.search_match_index == 0 {
            self.search_match_index = self.search_matches.len() - 1;
        } else {
            self.search_match_index -= 1;
        }
        if let Some(&prev_idx) = self.search_matches.get(self.search_match_index) {
            self.selected_index = Some(prev_idx);
            self.list_state.select(Some(prev_idx));
        }
    }

    pub fn go_back(&mut self) {
        match self.current_screen {
            CurrentScreen::ServerList => {
                self.current_screen = CurrentScreen::Home;
                self.servers.clear();
                self.selected_index = None;
                self.list_state.select(None);
            }
            CurrentScreen::TagList => {
                self.current_screen = CurrentScreen::ServerList;
                self.tags.clear();
                // Restore selection to the previous server if possible
                if !self.servers.is_empty() {
                    self.selected_index = Some(0); // Simple fallback for now
                    self.list_state.select(Some(0));
                }
            }
            CurrentScreen::TagValues => {
                self.current_screen = CurrentScreen::TagList;
                self.tag_values.clear();
                self.refresh_server = None;
                self.refresh_tag_ids.clear();
                self.last_read_time = None;
                // Restore selection to tags list
                if !self.tags.is_empty() {
                    self.selected_index = Some(0);
                    self.list_state.select(Some(0));
                } else {
                    self.selected_index = None;
                    self.list_state.select(None);
                }
            }
            CurrentScreen::WriteInput => {
                self.current_screen = CurrentScreen::TagValues;
                self.write_tag_id = None;
                self.write_value_input.clear();
            }
            _ => {}
        }
    }
}

/// Helper to parse a user string into a typed [`OpcValue`].
fn parse_opc_value(s: &str) -> OpcValue {
    // Try integer first
    if let Ok(i) = s.parse::<i32>() {
        return OpcValue::Int(i);
    }
    // Then float
    if let Ok(f) = s.parse::<f64>() {
        return OpcValue::Float(f);
    }
    // Then boolean
    match s.to_lowercase().as_str() {
        "true" | "1" => return OpcValue::Bool(true),
        "false" | "0" => return OpcValue::Bool(false),
        _ => {}
    }
    // Default to string
    let result = OpcValue::String(s.to_string());
    tracing::debug!(input = %s, parsed = ?result, "parse_opc_value: detected type");
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use mockall::predicate::*;
    use opc_da_client::{MockOpcProvider, OpcResult};

    #[test]
    fn test_poll_fetch_result_success() {
        let (tx, rx) = oneshot::channel();
        let mock = MockOpcProvider::new();
        let mut app = App::new(Arc::new(mock));
        app.current_screen = CurrentScreen::Loading;
        app.fetch_result_rx = Some(rx);

        tx.send(Ok(vec!["Server1".into(), "Server2".into()]))
            .unwrap();
        app.poll_fetch_result();

        assert_eq!(app.current_screen, CurrentScreen::ServerList);
        assert_eq!(app.servers.len(), 2);
        assert_eq!(app.selected_index, Some(0));
        assert!(app.fetch_result_rx.is_none());
        assert!(app.messages.last().unwrap().contains("Found 2 servers"));
    }

    #[test]
    fn test_poll_fetch_result_error() {
        let (tx, rx) = oneshot::channel();
        let mock = MockOpcProvider::new();
        let mut app = App::new(Arc::new(mock));
        app.current_screen = CurrentScreen::Loading;
        app.fetch_result_rx = Some(rx);

        tx.send(Err(OpcError::Internal("Connection failed".to_string())))
            .unwrap();
        app.poll_fetch_result();

        assert_eq!(app.current_screen, CurrentScreen::Home);
        assert!(app.fetch_result_rx.is_none());
        assert!(app.messages.last().unwrap().contains("Error"));
    }

    #[test]
    fn test_poll_fetch_result_empty_servers() {
        let (tx, rx) = oneshot::channel();
        let mock = MockOpcProvider::new();
        let mut app = App::new(Arc::new(mock));
        app.current_screen = CurrentScreen::Loading;
        app.fetch_result_rx = Some(rx);

        tx.send(Ok(vec![])).unwrap();
        app.poll_fetch_result();

        assert_eq!(app.current_screen, CurrentScreen::ServerList);
        assert!(app.servers.is_empty());
        assert_eq!(app.selected_index, None);
        assert!(app.messages.last().unwrap().contains("Found 0 servers"));
    }

    #[test]
    fn test_poll_fetch_result_closed() {
        let (tx, rx) = oneshot::channel::<OpcResult<Vec<String>>>();
        let mock = MockOpcProvider::new();
        let mut app = App::new(Arc::new(mock));
        app.current_screen = CurrentScreen::Loading;
        app.fetch_result_rx = Some(rx);

        // Drop the sender
        drop(tx);
        app.poll_fetch_result();

        assert_eq!(app.current_screen, CurrentScreen::Home);
        assert!(
            app.messages
                .last()
                .unwrap()
                .contains("terminated unexpectedly")
        );
    }

    #[tokio::test]
    async fn test_start_fetch_servers_sets_loading() {
        let mut mock = MockOpcProvider::new();
        mock.expect_list_servers()
            .returning(|_| Ok(vec!["S1".into()]));

        let mut app = App::new(Arc::new(mock));
        app.start_fetch_servers();

        assert_eq!(app.current_screen, CurrentScreen::Loading);
        assert!(app.fetch_result_rx.is_some());
    }

    #[test]
    fn test_server_navigation() {
        let mock = MockOpcProvider::new();
        let mut app = App::new(Arc::new(mock));
        app.servers = vec!["S1".into(), "S2".into()];
        app.selected_index = Some(0);
        app.current_screen = CurrentScreen::ServerList;
        app.list_state.select(Some(0));

        app.select_next();
        assert_eq!(app.selected_index, Some(1));

        app.select_next(); // Should stay at 1
        assert_eq!(app.selected_index, Some(1));

        app.select_prev();
        assert_eq!(app.selected_index, Some(0));

        app.select_prev(); // Should stay at 0
        assert_eq!(app.selected_index, Some(0));
    }

    #[test]
    fn test_tag_navigation_logic() {
        let mock = MockOpcProvider::new();
        let mut app = App::new(Arc::new(mock));
        app.servers = vec!["S1".into()];
        app.selected_index = Some(0);
        app.tags = vec!["T1".into(), "T2".into()];
        app.current_screen = CurrentScreen::TagList;
        app.list_state.select(Some(0));

        // Test boundary check against tags (2), not servers (1)
        app.select_next();
        assert_eq!(app.selected_index, Some(1));
        assert_eq!(app.list_state.selected(), Some(1));

        app.select_next(); // Should stay at 1
        assert_eq!(app.selected_index, Some(1));
    }

    #[tokio::test]
    async fn test_enter_selected_server_navigation() {
        let mut mock = MockOpcProvider::new();
        mock.expect_browse_tags()
            .with(
                eq("S1"),
                eq(MAX_BROWSE_TAGS),
                always(),
                always(),
                always(),
                always(),
            )
            .returning(|_, _, _, _, _, _| Ok(vec!["T1".into()]));

        let mut app = App::new(Arc::new(mock));
        app.servers = vec!["S1".into()];
        app.selected_index = Some(0);
        app.current_screen = CurrentScreen::ServerList;
        app.list_state.select(Some(0));

        app.start_browse_tags();
        // Wait briefly for the spawned task
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        app.poll_browse_result();

        assert!(matches!(app.current_screen, CurrentScreen::TagList));
        assert_eq!(app.tags.len(), 1);
        assert_eq!(app.selected_index, Some(0));
    }

    #[test]
    fn test_go_back_navigation() {
        let mock = MockOpcProvider::new();
        let mut app = App::new(Arc::new(mock));
        app.servers = vec!["S1".into()];
        app.selected_index = Some(0);
        app.tags = vec!["T1".into()];
        app.current_screen = CurrentScreen::TagList;
        app.list_state.select(Some(0));

        // TagList -> ServerList
        app.go_back();
        assert!(matches!(app.current_screen, CurrentScreen::ServerList));
        assert!(app.tags.is_empty());
        assert_eq!(app.selected_index, Some(0));

        // ServerList -> Home
        app.go_back();
        assert!(matches!(app.current_screen, CurrentScreen::Home));
        assert!(app.servers.is_empty());
        assert_eq!(app.selected_index, None);
    }

    #[tokio::test]
    async fn test_loading_transition() {
        let mock = MockOpcProvider::new();
        let mut app = App::new(Arc::new(mock));
        app.start_fetch_servers();
        assert_eq!(app.current_screen, CurrentScreen::Loading);
        assert!(app.messages.iter().any(|m| m.contains("Connecting to")));
    }

    #[tokio::test]
    async fn test_tui_navigation_flow() {
        let (tx, rx) = oneshot::channel();
        let mock = MockOpcProvider::new();
        let mut app = App::new(Arc::new(mock));

        // 1. Initial State: Home
        assert!(matches!(app.current_screen, CurrentScreen::Home));
        assert_eq!(app.host_input, "localhost");

        // 2. Start fetch
        app.start_fetch_servers();
        assert_eq!(app.current_screen, CurrentScreen::Loading);
        app.fetch_result_rx = Some(rx);

        // 3. Complete fetch
        tx.send(Ok(vec!["Server1".into()])).unwrap();
        app.poll_fetch_result();

        assert!(matches!(app.current_screen, CurrentScreen::ServerList));
        assert_eq!(app.servers.len(), 1);
        assert_eq!(app.selected_index, Some(0));
        assert_eq!(app.list_state.selected(), Some(0));

        // 4. User goes back to Home
        app.go_back();
        assert!(matches!(app.current_screen, CurrentScreen::Home));
        assert!(app.servers.is_empty());
        assert_eq!(app.selected_index, None);
        assert_eq!(app.list_state.selected(), None);
    }

    #[tokio::test]
    async fn test_poll_browse_result_error_shows_message() {
        let (tx, rx) = oneshot::channel();
        let mock = MockOpcProvider::new();
        let mut app = App::new(Arc::new(mock));
        app.current_screen = CurrentScreen::Loading;
        app.browse_result_rx = Some(rx);

        // Simulate provider returning a descriptive error
        tx.send(Err(OpcError::Internal(
            "DCOM access denied on remote host".to_string(),
        )))
        .unwrap();

        app.poll_browse_result();

        assert_eq!(app.current_screen, CurrentScreen::ServerList);
        assert!(app.browse_result_rx.is_none());
        let last_msg = app.messages.last().unwrap();
        assert!(last_msg.contains("Error: "));
        assert!(last_msg.contains("DCOM access denied")); // Error context preserved
    }

    #[tokio::test]
    async fn test_poll_browse_result_closed_shows_message() {
        let (tx, rx) = oneshot::channel();
        let mock = MockOpcProvider::new();
        let mut app = App::new(Arc::new(mock));
        app.current_screen = CurrentScreen::Loading;
        app.browse_result_rx = Some(rx);

        // Drop sender without sending — simulates task panic
        drop(tx);

        app.poll_browse_result();

        assert_eq!(app.current_screen, CurrentScreen::ServerList);
        assert!(app.browse_result_rx.is_none());
        let last_msg = app.messages.last().unwrap();
        assert!(last_msg.contains("terminated unexpectedly"));
    }

    #[tokio::test]
    async fn test_poll_browse_result_empty_tags() {
        let (tx, rx) = oneshot::channel();
        let mock = MockOpcProvider::new();
        let mut app = App::new(Arc::new(mock));
        app.current_screen = CurrentScreen::Loading;
        app.browse_result_rx = Some(rx);

        tx.send(Ok(vec![])).unwrap();

        app.poll_browse_result();

        assert_eq!(app.current_screen, CurrentScreen::TagList);
        assert!(app.tags.is_empty());
        assert_eq!(app.selected_index, None);
        assert_eq!(app.list_state.selected(), None);
        assert!(app.messages.last().unwrap().contains("Found 0 tags"));
    }

    #[test]
    fn test_start_browse_no_selection() {
        let mock = MockOpcProvider::new();
        let mut app = App::new(Arc::new(mock));
        app.current_screen = CurrentScreen::ServerList;
        app.servers = vec!["S1".into()];
        app.selected_index = None; // No selection

        app.start_browse_tags();

        // Should remain on ServerList — no crash, no Loading transition
        assert_eq!(app.current_screen, CurrentScreen::ServerList);
        assert!(app.browse_result_rx.is_none());
    }

    #[test]
    fn test_start_browse_wrong_screen() {
        let mock = MockOpcProvider::new();
        let mut app = App::new(Arc::new(mock));
        app.current_screen = CurrentScreen::Home; // Wrong screen
        app.servers = vec!["S1".into()];
        app.selected_index = Some(0);

        app.start_browse_tags();

        assert_eq!(app.current_screen, CurrentScreen::Home); // Unchanged
        assert!(app.browse_result_rx.is_none());
    }

    #[test]
    fn test_poll_fetch_result_timeout() {
        let (tx, rx) = oneshot::channel();
        let mock = MockOpcProvider::new();
        let mut app = App::new(Arc::new(mock));
        app.current_screen = CurrentScreen::Loading;
        app.fetch_result_rx = Some(rx);

        tx.send(Err(OpcError::Internal(
            "Connection timed out (30s)".to_string(),
        )))
        .unwrap();
        app.poll_fetch_result();

        assert_eq!(app.current_screen, CurrentScreen::Home);
        assert!(app.messages.last().unwrap().contains("timed out"));
    }

    #[test]
    fn test_add_message_ring_buffer() {
        let mock = MockOpcProvider::new();
        let mut app = App::new(Arc::new(mock));

        for i in 0..15 {
            app.add_message(format!("msg-{}", i));
        }

        assert_eq!(app.messages.len(), 10); // Capped at 10
        assert_eq!(app.messages[0], "msg-5"); // Oldest surviving
        assert_eq!(app.messages[9], "msg-14"); // Latest
    }

    #[test]
    fn test_select_on_empty_list() {
        let mock = MockOpcProvider::new();
        let mut app = App::new(Arc::new(mock));
        app.current_screen = CurrentScreen::ServerList;
        app.servers = vec![]; // Empty

        app.select_next();
        assert_eq!(app.selected_index, None);

        app.select_prev();
        assert_eq!(app.selected_index, None);
    }

    #[test]
    fn test_poll_browse_result_no_task() {
        let mock = MockOpcProvider::new();
        let mut app = App::new(Arc::new(mock));
        app.current_screen = CurrentScreen::ServerList;

        // No browse_result_rx set — should not panic
        app.poll_browse_result();

        assert_eq!(app.current_screen, CurrentScreen::ServerList);
    }

    #[test]
    fn test_toggle_tag_selection() {
        let mock = MockOpcProvider::new();
        let mut app = App::new(Arc::new(mock));
        app.current_screen = CurrentScreen::TagList;
        app.tags = vec!["Tag1".into(), "Tag2".into()];
        app.selected_tags = vec![false, false];
        app.selected_index = Some(1);

        app.toggle_tag_selection();
        assert_eq!(app.selected_tags, vec![false, true]);

        app.toggle_tag_selection();
        assert_eq!(app.selected_tags, vec![false, false]);
    }

    #[test]
    fn test_start_read_values_no_selection() {
        let mock = MockOpcProvider::new();
        let mut app = App::new(Arc::new(mock));
        app.current_screen = CurrentScreen::TagList;
        app.tags = vec!["Tag1".into()];
        app.selected_tags = vec![false];

        app.start_read_values();

        assert_eq!(app.current_screen, CurrentScreen::TagList);
        assert!(app.messages.last().unwrap().contains("No tags selected"));
        assert!(app.read_result_rx.is_none());
    }

    #[test]
    fn test_start_read_values_wrong_screen() {
        let mock = MockOpcProvider::new();
        let mut app = App::new(Arc::new(mock));
        app.current_screen = CurrentScreen::ServerList;

        app.start_read_values();

        assert_eq!(app.current_screen, CurrentScreen::ServerList);
        assert!(app.read_result_rx.is_none());
    }

    #[tokio::test]
    async fn test_start_read_values_success() {
        use mockall::predicate::eq;
        let mut mock = MockOpcProvider::new();
        mock.expect_read_tag_values()
            .with(eq("TestServer"), eq(vec!["Tag1".to_string()]))
            .returning(|_, _| Ok(vec![]));

        let mut app = App::new(Arc::new(mock));
        app.current_screen = CurrentScreen::TagList;
        app.tags = vec!["Tag1".into()];
        app.selected_tags = vec![true];
        app.browsed_server = Some("TestServer".into());

        app.start_read_values();

        assert_eq!(app.current_screen, CurrentScreen::Loading);
        assert!(app.read_result_rx.is_some());
        assert_eq!(app.refresh_server, Some("TestServer".into()));
    }

    #[test]
    fn test_start_read_values_no_browsed_server() {
        let mock = MockOpcProvider::new();
        let mut app = App::new(Arc::new(mock));
        app.current_screen = CurrentScreen::TagList;
        app.tags = vec!["Tag1".into()];
        app.selected_tags = vec![true];
        app.browsed_server = None; // Simulate missing context

        app.start_read_values();

        assert_eq!(app.current_screen, CurrentScreen::TagList); // Should not transition
        assert!(app.read_result_rx.is_none());
        assert!(app.messages.last().unwrap().contains("No server context"));
    }

    #[test]
    fn test_poll_read_result_success() {
        let (tx, rx) = oneshot::channel();
        let mock = MockOpcProvider::new();
        let mut app = App::new(Arc::new(mock));
        app.current_screen = CurrentScreen::Loading;
        app.read_result_rx = Some(rx);

        let values = vec![TagValue {
            tag_id: "Tag1".into(),
            value: "123".into(),
            quality: "Good".into(),
            timestamp: "Today".into(),
        }];

        tx.send(Ok(values)).unwrap();
        app.poll_read_result();

        assert_eq!(app.current_screen, CurrentScreen::TagValues);
        assert_eq!(app.tag_values.len(), 1);
        assert_eq!(app.tag_values[0].value, "123");
        assert!(app.read_result_rx.is_none());
    }

    #[test]
    fn test_poll_read_result_error() {
        let (tx, rx) = oneshot::channel();
        let mock = MockOpcProvider::new();
        let mut app = App::new(Arc::new(mock));
        app.current_screen = CurrentScreen::Loading;
        app.read_result_rx = Some(rx);

        tx.send(Err(OpcError::Internal("Read failed".to_string())))
            .unwrap();
        app.poll_read_result();

        assert_eq!(app.current_screen, CurrentScreen::TagList);
        assert!(app.read_result_rx.is_none());
        assert!(
            app.messages
                .last()
                .unwrap()
                .contains("Error reading values")
        );
    }

    #[test]
    fn test_go_back_from_tag_values() {
        let mock = MockOpcProvider::new();
        let mut app = App::new(Arc::new(mock));
        app.current_screen = CurrentScreen::TagValues;
        app.tags = vec!["Tag1".into()];
        app.tag_values = vec![TagValue {
            tag_id: "Tag1".into(),
            value: "100".into(),
            quality: "Good".into(),
            timestamp: String::new(),
        }];

        app.go_back();

        assert_eq!(app.current_screen, CurrentScreen::TagList);
        assert!(app.tag_values.is_empty());
        assert_eq!(app.tags.len(), 1); // Tags preserved
    }

    #[test]
    fn test_select_next_on_tag_values() {
        let mock = MockOpcProvider::new();
        let mut app = App::new(Arc::new(mock));
        app.current_screen = CurrentScreen::TagValues;
        app.tag_values = vec![
            TagValue {
                tag_id: "T1".into(),
                value: "V1".into(),
                quality: "Q".into(),
                timestamp: "T".into(),
            },
            TagValue {
                tag_id: "T2".into(),
                value: "V2".into(),
                quality: "Q".into(),
                timestamp: "T".into(),
            },
        ];
        app.selected_index = Some(0);

        app.select_next();
        assert_eq!(app.selected_index, Some(1));

        app.select_next(); // Should stay at 1
        assert_eq!(app.selected_index, Some(1));
    }

    #[test]
    fn test_page_down_basic() {
        let mock = MockOpcProvider::new();
        let mut app = App::new(Arc::new(mock));
        app.current_screen = CurrentScreen::TagList;
        app.tags = (0..50).map(|i| format!("T{}", i)).collect();
        app.selected_index = Some(0);

        app.page_down();
        assert_eq!(app.selected_index, Some(20));

        app.page_down();
        assert_eq!(app.selected_index, Some(40));

        app.page_down(); // Should clamp to 49
        assert_eq!(app.selected_index, Some(49));
    }

    #[test]
    fn test_page_up_basic() {
        let mock = MockOpcProvider::new();
        let mut app = App::new(Arc::new(mock));
        app.current_screen = CurrentScreen::TagList;
        app.tags = (0..50).map(|i| format!("T{}", i)).collect();
        app.selected_index = Some(49);

        app.page_up();
        assert_eq!(app.selected_index, Some(29));

        app.page_up();
        assert_eq!(app.selected_index, Some(9));

        app.page_up(); // Should clamp to 0
        assert_eq!(app.selected_index, Some(0));
    }

    #[test]
    fn test_search_basic_matching() {
        let mock = MockOpcProvider::new();
        let mut app = App::new(Arc::new(mock));
        app.current_screen = CurrentScreen::TagList;
        app.tags = vec![
            "System.Cpu".into(),
            "System.Mem".into(),
            "User.Data".into(),
            "User.Settings".into(),
        ];
        app.selected_tags = vec![false; 4];

        app.enter_search_mode();
        assert!(app.search_mode);

        app.update_search_query('s');
        app.update_search_query('y');
        app.update_search_query('s'); // Query: "sys"

        assert_eq!(app.search_matches.len(), 2);
        assert_eq!(app.search_matches[0], 0); // System.Cpu
        assert_eq!(app.search_matches[1], 1); // System.Mem
        assert_eq!(app.selected_index, Some(0));

        app.next_search_match();
        assert_eq!(app.selected_index, Some(1));

        app.next_search_match(); // Should wrap
        assert_eq!(app.selected_index, Some(0));

        app.search_backspace(); // Query: "sy"
        assert_eq!(app.search_matches.len(), 2);

        app.exit_search_mode();
        assert!(!app.search_mode);
    }
}
