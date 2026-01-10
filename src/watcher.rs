use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, RwLock};
use tokio::time::interval;
use tokio_util::sync::CancellationToken;

use crate::parser;
use crate::types::{
    ActivityInfo, BackgroundTask, NewAgentMsg, NewBackgroundTaskMsg, NewSessionMsg, Session,
    SessionInfo, StreamItem, AGENT_ID_DISPLAY_LENGTH,
};

/// How often to check for new content (ms)
pub const POLL_INTERVAL_MS: u64 = 500;

/// How recent a session must be to be considered active
pub const ACTIVE_WINDOW_SECS: u64 = 5 * 60;

/// Total line count above which we auto-skip history
pub const AUTO_SKIP_LINE_THRESHOLD: usize = 100;

/// How many recent lines to keep when skipping
pub const KEEP_RECENT_LINES: usize = 10;

/// How often to clean up stale file position entries (secs)
pub const CLEANUP_INTERVAL_SECS: u64 = 5 * 60;

/// Buffer size for reading files
pub const FILE_READ_BUFFER_SIZE: usize = 32 * 1024;

/// Max buffer size for line scanner
pub const SCANNER_MAX_BUFFER_SIZE: usize = 1024 * 1024;

/// How recent a session must be to show as "active" in listings
pub const RECENT_ACTIVITY_THRESHOLD_SECS: u64 = 2 * 60;

/// Channel buffer size for items
pub const ITEM_CHANNEL_BUFFER: usize = 100;

/// Channel buffer size for events
pub const EVENT_CHANNEL_BUFFER: usize = 10;

/// Get the Claude projects directory
fn get_claude_projects_dir() -> Result<PathBuf> {
    let home = dirs::home_dir().context("Failed to get home directory")?;
    Ok(home.join(".claude").join("projects"))
}

/// Check if a path is a main session file (not a subagent, not a directory)
fn is_main_session_file(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }
    let path_str = path.to_string_lossy();
    if !path_str.ends_with(".jsonl") {
        return false;
    }
    if path_str.contains("/subagents/") {
        return false;
    }
    if let Some(name) = path.file_name() {
        if name.to_string_lossy().starts_with("agent-") {
            return false;
        }
    }
    true
}

/// Channels for watcher output
pub struct WatcherChannels {
    pub items: mpsc::Receiver<StreamItem>,
    pub new_agent: mpsc::Receiver<NewAgentMsg>,
    pub new_session: mpsc::Receiver<NewSessionMsg>,
    pub new_background_task: mpsc::Receiver<NewBackgroundTaskMsg>,
    pub errors: mpsc::Receiver<anyhow::Error>,
}

/// File watcher for Claude session files
pub struct Watcher {
    claude_dir: PathBuf,
    sessions: Arc<RwLock<HashMap<String, Arc<Session>>>>,
    file_positions: Arc<RwLock<HashMap<PathBuf, u64>>>,
    item_tx: mpsc::Sender<StreamItem>,
    new_agent_tx: mpsc::Sender<NewAgentMsg>,
    new_session_tx: mpsc::Sender<NewSessionMsg>,
    new_background_task_tx: mpsc::Sender<NewBackgroundTaskMsg>,
    error_tx: mpsc::Sender<anyhow::Error>,
    cancel_token: CancellationToken,
    watch_active: Arc<AtomicBool>,
    skip_history: Arc<AtomicBool>,
    active_window: Duration,
}

impl Watcher {
    /// Create a new watcher, optionally for a specific session
    pub async fn new(session_id: Option<&str>) -> Result<(Self, WatcherChannels)> {
        let claude_dir = get_claude_projects_dir()?;

        let (item_tx, item_rx) = mpsc::channel(ITEM_CHANNEL_BUFFER);
        let (new_agent_tx, new_agent_rx) = mpsc::channel(EVENT_CHANNEL_BUFFER);
        let (new_session_tx, new_session_rx) = mpsc::channel(EVENT_CHANNEL_BUFFER);
        let (new_background_task_tx, new_background_task_rx) = mpsc::channel(EVENT_CHANNEL_BUFFER);
        let (error_tx, error_rx) = mpsc::channel(EVENT_CHANNEL_BUFFER);

        let watch_active = Arc::new(AtomicBool::new(session_id.is_none()));
        let skip_history = Arc::new(AtomicBool::new(false));

        let watcher = Self {
            claude_dir,
            sessions: Arc::new(RwLock::new(HashMap::new())),
            file_positions: Arc::new(RwLock::new(HashMap::new())),
            item_tx,
            new_agent_tx,
            new_session_tx,
            new_background_task_tx,
            error_tx,
            cancel_token: CancellationToken::new(),
            watch_active,
            skip_history,
            active_window: Duration::from_secs(ACTIVE_WINDOW_SECS),
        };

        // Initialize sessions
        if let Some(sid) = session_id {
            let session = watcher.find_session(sid)?;
            let mut sessions = watcher.sessions.write().await;
            sessions.insert(session.id.clone(), Arc::new(session));
        } else {
            watcher.discover_active_sessions().await?;
        }

        let channels = WatcherChannels {
            items: item_rx,
            new_agent: new_agent_rx,
            new_session: new_session_rx,
            new_background_task: new_background_task_rx,
            errors: error_rx,
        };

        Ok((watcher, channels))
    }

    /// Get a copy of all watched sessions
    pub async fn get_sessions(&self) -> HashMap<String, Arc<Session>> {
        self.sessions.read().await.clone()
    }

    /// Get sessions synchronously (for initialization)
    pub fn get_sessions_sync(&self) -> HashMap<String, Arc<Session>> {
        self.sessions.blocking_read().clone()
    }

    /// Set whether to skip history on startup
    pub fn set_skip_history(&self, skip: bool) {
        self.skip_history.store(skip, Ordering::SeqCst);
    }

    /// Remove a session from being watched
    pub async fn remove_session(&self, session_id: &str) {
        self.sessions.write().await.remove(session_id);
    }

    /// Toggle auto-discovery of new sessions
    pub fn toggle_auto_discovery(&self) {
        let current = self.watch_active.load(Ordering::SeqCst);
        self.watch_active.store(!current, Ordering::SeqCst);
    }

    /// Check if auto-discovery is enabled
    pub fn is_auto_discovery_enabled(&self) -> bool {
        self.watch_active.load(Ordering::SeqCst)
    }

    /// Get activity info for all watched sessions and agents
    pub async fn get_activity_info(&self, active_within: Duration) -> Vec<ActivityInfo> {
        let mut info = Vec::new();
        let now = std::time::SystemTime::now();
        let sessions = self.sessions.read().await;

        for session in sessions.values() {
            // Check main file
            if let Ok(metadata) = fs::metadata(&session.main_file) {
                if let Ok(modified) = metadata.modified() {
                    let is_active = now
                        .duration_since(modified)
                        .map(|d| d < active_within)
                        .unwrap_or(false);
                    info.push(ActivityInfo {
                        session_id: session.id.clone(),
                        agent_id: String::new(),
                        is_active,
                    });
                }
            }

            // Check subagent files
            let subagents = session.subagents.read().await;
            for (agent_id, path) in subagents.iter() {
                if let Ok(metadata) = fs::metadata(path) {
                    if let Ok(modified) = metadata.modified() {
                        let is_active = now
                            .duration_since(modified)
                            .map(|d| d < active_within)
                            .unwrap_or(false);
                        info.push(ActivityInfo {
                            session_id: session.id.clone(),
                            agent_id: agent_id.clone(),
                            is_active,
                        });
                    }
                }
            }
        }

        info
    }

    /// Start the watcher loop
    pub fn start(self: Arc<Self>) {
        let watcher = self.clone();
        tokio::spawn(async move {
            watcher.watch_loop().await;
        });
    }

    /// Stop the watcher
    pub fn stop(&self) {
        self.cancel_token.cancel();
    }

    /// Find a specific session by ID
    fn find_session(&self, session_id: &str) -> Result<Session> {
        let mut jsonl_files = Vec::new();

        Self::walk_directory(&self.claude_dir, &mut |path| {
            if is_main_session_file(path) {
                jsonl_files.push(path.to_path_buf());
            }
        })?;

        if jsonl_files.is_empty() {
            anyhow::bail!("No session files found in {:?}", self.claude_dir);
        }

        // Sort by modification time (most recent first)
        jsonl_files.sort_by(|a, b| {
            let time_a = fs::metadata(a).and_then(|m| m.modified()).ok();
            let time_b = fs::metadata(b).and_then(|m| m.modified()).ok();
            time_b.cmp(&time_a)
        });

        // Find specific session
        let main_file = jsonl_files
            .iter()
            .find(|f| f.to_string_lossy().contains(session_id))
            .ok_or_else(|| anyhow::anyhow!("Session {} not found", session_id))?;

        self.build_session(main_file)
    }

    /// Build a Session from a main file path
    fn build_session(&self, main_file: &Path) -> Result<Session> {
        let basename = main_file
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("");
        let id = basename.trim_end_matches(".jsonl").to_string();

        // Extract project path from parent directory name
        let project_dir = main_file
            .parent()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            .unwrap_or("");
        let project_path = project_dir.replace('-', "/").trim_start_matches('/').to_string();

        // Find subagent files first (before creating Session)
        let mut subagents_map = HashMap::new();
        let subagent_dir = main_file
            .parent()
            .map(|p| p.join(&id).join("subagents"))
            .unwrap_or_default();

        if let Ok(entries) = fs::read_dir(&subagent_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                    if name.ends_with(".jsonl") {
                        let agent_id = name
                            .trim_start_matches("agent-")
                            .trim_end_matches(".jsonl")
                            .to_string();
                        subagents_map.insert(agent_id, path);
                    }
                }
            }
        }

        Ok(Session {
            id,
            project_path,
            main_file: main_file.to_path_buf(),
            subagents: Arc::new(RwLock::new(subagents_map)),
            background_tasks: Arc::new(RwLock::new(HashMap::new())),
        })
    }

    /// Discover active sessions
    async fn discover_active_sessions(&self) -> Result<()> {
        let now = std::time::SystemTime::now();
        let active_window = self.active_window;
        let claude_dir = self.claude_dir.clone();

        // Collect sessions to add
        let mut sessions_to_add = Vec::new();

        Self::walk_directory(&claude_dir, &mut |path| {
            if !is_main_session_file(path) {
                return;
            }

            // Check if recently modified
            if let Ok(metadata) = fs::metadata(path) {
                if let Ok(modified) = metadata.modified() {
                    if let Ok(duration) = now.duration_since(modified) {
                        if duration > active_window {
                            return;
                        }
                    }
                }
            }

            if let Ok(session) = self.build_session(path) {
                sessions_to_add.push(session);
            }
        })?;

        // Add sessions to the map
        let mut sessions = self.sessions.write().await;
        for session in sessions_to_add {
            sessions.insert(session.id.clone(), Arc::new(session));
        }

        Ok(())
    }

    /// Walk a directory tree
    fn walk_directory<F>(dir: &Path, callback: &mut F) -> Result<()>
    where
        F: FnMut(&Path),
    {
        if !dir.exists() {
            return Ok(());
        }

        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();

            if path.is_dir() {
                Self::walk_directory(&path, callback)?;
            } else {
                callback(&path);
            }
        }

        Ok(())
    }

    /// Main watch loop
    async fn watch_loop(&self) {
        let mut poll_interval = interval(Duration::from_millis(POLL_INTERVAL_MS));
        let mut cleanup_interval = interval(Duration::from_secs(CLEANUP_INTERVAL_SECS));

        // Initialize reading
        self.initialize_session_reading().await;

        loop {
            tokio::select! {
                _ = self.cancel_token.cancelled() => {
                    break;
                }
                _ = poll_interval.tick() => {
                    self.handle_poll_tick().await;
                }
                _ = cleanup_interval.tick() => {
                    self.cleanup_file_positions().await;
                }
            }
        }
    }

    /// Initialize reading from session files
    async fn initialize_session_reading(&self) {
        let sessions: Vec<Arc<Session>> = self.sessions.read().await.values().cloned().collect();
        let should_skip = self.skip_history.load(Ordering::SeqCst);

        let total_lines = if !should_skip {
            self.count_total_lines(&sessions).await
        } else {
            AUTO_SKIP_LINE_THRESHOLD + 1 // Force skip
        };

        let skip = should_skip || total_lines > AUTO_SKIP_LINE_THRESHOLD;

        for session in sessions {
            if skip {
                self.skip_to_end_of_files(&session).await;
            } else {
                self.read_session_files(&session).await;
            }
        }
    }

    /// Handle a single poll tick
    async fn handle_poll_tick(&self) {
        if self.watch_active.load(Ordering::SeqCst) {
            self.check_for_new_sessions().await;
        }

        let sessions: Vec<Arc<Session>> = self.sessions.read().await.values().cloned().collect();

        for session in sessions {
            self.check_for_new_subagents(&session).await;
            self.check_for_background_tasks(&session).await;
            self.read_session_files(&session).await;
        }
    }

    /// Check for new sessions
    async fn check_for_new_sessions(&self) {
        let now = std::time::SystemTime::now();
        let claude_dir = self.claude_dir.clone();

        let mut new_sessions = Vec::new();

        Self::walk_directory(&claude_dir, &mut |path| {
            if !is_main_session_file(path) {
                return;
            }

            // Check if recently modified
            if let Ok(metadata) = fs::metadata(path) {
                if let Ok(modified) = metadata.modified() {
                    if let Ok(duration) = now.duration_since(modified) {
                        if duration > self.active_window {
                            return;
                        }
                    }
                }
            }

            let basename = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            let id = basename.trim_end_matches(".jsonl").to_string();

            new_sessions.push((id, path.to_path_buf()));
        })
        .ok();

        for (id, path) in new_sessions {
            let exists = self.sessions.read().await.contains_key(&id);
            if !exists {
                if let Ok(session) = self.build_session(&path) {
                    let project_path = session.project_path.clone();
                    self.sessions
                        .write()
                        .await
                        .insert(id.clone(), Arc::new(session));

                    let _ = self
                        .new_session_tx
                        .try_send(NewSessionMsg {
                            session_id: id,
                            project_path,
                        });
                }
            }
        }
    }

    /// Check for new subagents in a session
    async fn check_for_new_subagents(&self, session: &Session) {
        let subagent_dir = session
            .main_file
            .parent()
            .map(|p| p.join(&session.id).join("subagents"))
            .unwrap_or_default();

        let entries = match fs::read_dir(&subagent_dir) {
            Ok(e) => e,
            Err(_) => return,
        };

        for entry in entries.flatten() {
            let path = entry.path();
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                if name.ends_with(".jsonl") {
                    let agent_id = name
                        .trim_start_matches("agent-")
                        .trim_end_matches(".jsonl")
                        .to_string();

                    let exists = session.subagents.read().await.contains_key(&agent_id);
                    if !exists {
                        session
                            .subagents
                            .write()
                            .await
                            .insert(agent_id.clone(), path);

                        let _ = self.new_agent_tx.try_send(NewAgentMsg {
                            session_id: session.id.clone(),
                            agent_id,
                        });
                    }
                }
            }
        }
    }

    /// Check for background tasks in tool-results directory
    async fn check_for_background_tasks(&self, session: &Session) {
        let tool_results_dir = session
            .main_file
            .parent()
            .map(|p| p.join(&session.id).join("tool-results"))
            .unwrap_or_default();

        let entries = match fs::read_dir(&tool_results_dir) {
            Ok(e) => e,
            Err(_) => return,
        };

        for entry in entries.flatten() {
            let path = entry.path();
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                if !name.ends_with(".txt") {
                    continue;
                }

                let tool_id = name.trim_end_matches(".txt").to_string();

                let exists = session
                    .background_tasks
                    .read()
                    .await
                    .contains_key(&tool_id);
                if exists {
                    continue;
                }

                let (parent_agent_id, tool_name) =
                    self.find_background_task_parent(session, &tool_id).await;
                let is_complete = self
                    .is_background_task_complete(session, &tool_id)
                    .await;

                let task = BackgroundTask {
                    tool_id: tool_id.clone(),
                    parent_agent_id: parent_agent_id.clone(),
                    tool_name: tool_name.clone(),
                    output_path: path.clone(),
                    is_complete,
                };

                session
                    .background_tasks
                    .write()
                    .await
                    .insert(tool_id.clone(), task);

                let _ = self.new_background_task_tx.try_send(NewBackgroundTaskMsg {
                    session_id: session.id.clone(),
                    parent_agent_id,
                    tool_id,
                    tool_name,
                    output_path: path,
                    is_complete,
                });
            }
        }
    }

    /// Find which agent spawned a background task
    async fn find_background_task_parent(
        &self,
        session: &Session,
        tool_id: &str,
    ) -> (String, String) {
        // Search main file first
        if let Some(name) = Self::find_tool_in_file(&session.main_file, tool_id) {
            return (String::new(), name);
        }

        // Search subagent files
        let subagents = session.subagents.read().await;
        for (agent_id, path) in subagents.iter() {
            if let Some(name) = Self::find_tool_in_file(path, tool_id) {
                return (agent_id.clone(), name);
            }
        }

        (String::new(), "Background Task".to_string())
    }

    /// Search a JSONL file for a tool_use with the given ID
    fn find_tool_in_file(path: &Path, tool_id: &str) -> Option<String> {
        let file = File::open(path).ok()?;
        let reader = BufReader::new(file);

        for line in reader.lines().map_while(Result::ok) {
            if !line.contains(tool_id) {
                continue;
            }

            if let Some(name) = Self::extract_tool_name_from_line(&line, tool_id) {
                return Some(name);
            }
        }

        None
    }

    /// Extract tool name from a JSONL line containing the tool ID
    fn extract_tool_name_from_line(line: &str, _tool_id: &str) -> Option<String> {
        if !line.contains("\"type\":\"tool_use\"") && !line.contains("\"type\": \"tool_use\"") {
            return None;
        }

        // Look for name field
        for pattern in &["\"name\":\"", "\"name\": \""] {
            if let Some(idx) = line.find(pattern) {
                let start = idx + pattern.len();
                if let Some(end) = line[start..].find('"') {
                    let name = &line[start..start + end];
                    return Some(Self::format_tool_name(name, line));
                }
            }
        }

        None
    }

    /// Format tool name with context
    fn format_tool_name(tool_name: &str, line: &str) -> String {
        match tool_name {
            "Bash" => {
                if let Some(cmd) = Self::extract_field(line, "command") {
                    let cmd = if cmd.len() > 30 {
                        format!("{}...", &cmd[..30])
                    } else {
                        cmd
                    };
                    return format!("Bash: {}", cmd);
                }
            }
            "Task" => {
                if let Some(desc) = Self::extract_field(line, "description") {
                    let desc = if desc.len() > 30 {
                        format!("{}...", &desc[..30])
                    } else {
                        desc
                    };
                    return format!("Task: {}", desc);
                }
            }
            _ => {}
        }

        tool_name.to_string()
    }

    /// Extract a JSON field value
    fn extract_field(line: &str, field: &str) -> Option<String> {
        for pattern in &[
            format!("\"{}\":\"", field),
            format!("\"{}\": \"", field),
        ] {
            if let Some(idx) = line.find(pattern.as_str()) {
                let start = idx + pattern.len();
                let mut end = start;
                while end < line.len() {
                    let c = line.chars().nth(end)?;
                    if c == '"' && (end == start || line.chars().nth(end - 1) != Some('\\')) {
                        break;
                    }
                    end += 1;
                }
                if end > start {
                    return Some(line[start..end].to_string());
                }
            }
        }
        None
    }

    /// Check if a background task has completed
    async fn is_background_task_complete(&self, session: &Session, tool_id: &str) -> bool {
        // Check main file
        if Self::file_contains_tool_result(&session.main_file, tool_id) {
            return true;
        }

        // Check subagent files
        let subagents = session.subagents.read().await;
        for path in subagents.values() {
            if Self::file_contains_tool_result(path, tool_id) {
                return true;
            }
        }

        false
    }

    /// Check if a file contains a tool_result for the given tool ID
    fn file_contains_tool_result(path: &Path, tool_id: &str) -> bool {
        let file = match File::open(path) {
            Ok(f) => f,
            Err(_) => return false,
        };
        let reader = BufReader::new(file);

        for line in reader.lines().map_while(Result::ok) {
            if line.contains(tool_id) && line.contains("\"tool_result\"") {
                return true;
            }
        }

        false
    }

    /// Count total lines across all session files
    async fn count_total_lines(&self, sessions: &[Arc<Session>]) -> usize {
        let mut total = 0;

        for session in sessions {
            total += Self::count_file_lines(&session.main_file);

            let subagents = session.subagents.read().await;
            for path in subagents.values() {
                total += Self::count_file_lines(path);
            }
        }

        total
    }

    /// Count lines in a file
    fn count_file_lines(path: &Path) -> usize {
        let file = match File::open(path) {
            Ok(f) => f,
            Err(_) => return 0,
        };

        let mut count = 0;
        let mut reader = BufReader::with_capacity(FILE_READ_BUFFER_SIZE, file);
        let mut buf = [0u8; FILE_READ_BUFFER_SIZE];

        loop {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    count += buf[..n].iter().filter(|&&b| b == b'\n').count();
                }
                Err(_) => break,
            }
        }

        count
    }

    /// Skip to end of files, keeping last N lines
    async fn skip_to_end_of_files(&self, session: &Session) {
        let main_pos = Self::find_position_for_last_n_lines(&session.main_file, KEEP_RECENT_LINES);

        let subagents = session.subagents.read().await;
        let subagent_positions: Vec<_> = subagents
            .values()
            .map(|path| {
                (
                    path.clone(),
                    Self::find_position_for_last_n_lines(path, KEEP_RECENT_LINES),
                )
            })
            .collect();
        drop(subagents);

        let mut positions = self.file_positions.write().await;
        positions.insert(session.main_file.clone(), main_pos);
        for (path, pos) in subagent_positions {
            positions.insert(path, pos);
        }
    }

    /// Find byte offset to start reading last N lines
    fn find_position_for_last_n_lines(path: &Path, n: usize) -> u64 {
        let file = match File::open(path) {
            Ok(f) => f,
            Err(_) => return 0,
        };

        let mut reader = BufReader::with_capacity(FILE_READ_BUFFER_SIZE, file);
        let mut newline_positions = Vec::new();
        let mut pos: u64 = 0;
        let mut buf = [0u8; FILE_READ_BUFFER_SIZE];

        loop {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(bytes_read) => {
                    for (i, &byte) in buf[..bytes_read].iter().enumerate() {
                        if byte == b'\n' {
                            newline_positions.push(pos + i as u64 + 1);
                        }
                    }
                    pos += bytes_read as u64;
                }
                Err(_) => break,
            }
        }

        if newline_positions.len() <= n {
            return 0;
        }

        newline_positions[newline_positions.len() - n]
    }

    /// Read session files from last known position
    async fn read_session_files(&self, session: &Session) {
        // Read main file
        self.read_file(&session.main_file, &session.id, "").await;

        // Read subagent files
        let subagents: Vec<_> = session
            .subagents
            .read()
            .await
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();

        for (agent_id, path) in subagents {
            self.read_file(&path, &session.id, &agent_id).await;
        }
    }

    /// Read new content from a file
    async fn read_file(&self, path: &Path, session_id: &str, agent_id: &str) {
        let mut file = match File::open(path) {
            Ok(f) => f,
            Err(_) => return,
        };

        // Get last position
        let pos = self
            .file_positions
            .read()
            .await
            .get(path)
            .copied()
            .unwrap_or(0);

        if file.seek(SeekFrom::Start(pos)).is_err() {
            return;
        }

        let reader = BufReader::with_capacity(FILE_READ_BUFFER_SIZE, &file);

        for line in reader.lines() {
            let line = match line {
                Ok(l) => l,
                Err(e) => {
                    let _ = self.error_tx.try_send(e.into());
                    continue;
                }
            };

            let items = match parser::parse_line(&line) {
                Ok(items) => items,
                Err(e) => {
                    let _ = self.error_tx.try_send(e);
                    continue;
                }
            };

            for mut item in items {
                item.session_id = session_id.to_string();

                if !agent_id.is_empty() && item.agent_id.is_empty() {
                    item.agent_id = agent_id.to_string();
                    item.agent_name = format!(
                        "Agent-{}",
                        &agent_id[..agent_id.len().min(AGENT_ID_DISPLAY_LENGTH)]
                    );
                }

                if self.item_tx.send(item).await.is_err() {
                    return;
                }
            }
        }

        // Update position
        if let Ok(new_pos) = file.stream_position() {
            self.file_positions
                .write()
                .await
                .insert(path.to_path_buf(), new_pos);
        }
    }

    /// Clean up stale file position entries
    async fn cleanup_file_positions(&self) {
        let mut positions = self.file_positions.write().await;
        positions.retain(|path, _| path.exists());
    }
}

/// List recent sessions
pub fn list_sessions(limit: usize) -> Result<Vec<SessionInfo>> {
    list_sessions_filtered(limit, Duration::ZERO)
}

/// List active sessions (modified within duration)
pub fn list_active_sessions(within: Duration) -> Result<Vec<SessionInfo>> {
    list_sessions_filtered(0, within)
}

fn list_sessions_filtered(limit: usize, active_within: Duration) -> Result<Vec<SessionInfo>> {
    let claude_dir = get_claude_projects_dir()?;
    let now = std::time::SystemTime::now();
    let mut sessions = Vec::new();

    Watcher::walk_directory(&claude_dir, &mut |path| {
        if !is_main_session_file(path) {
            return;
        }

        let metadata = match fs::metadata(path) {
            Ok(m) => m,
            Err(_) => return,
        };

        let modified = match metadata.modified() {
            Ok(m) => m,
            Err(_) => return,
        };

        // Filter by active time if specified
        if !active_within.is_zero() {
            if let Ok(duration) = now.duration_since(modified) {
                if duration > active_within {
                    return;
                }
            }
        }

        let basename = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        let project_dir = path
            .parent()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            .unwrap_or("");
        let project_path = project_dir.replace('-', "/").trim_start_matches('/').to_string();

        let is_active = now
            .duration_since(modified)
            .map(|d| d.as_secs() < RECENT_ACTIVITY_THRESHOLD_SECS)
            .unwrap_or(false);

        let modified_utc = DateTime::<Utc>::from(modified);

        sessions.push(SessionInfo {
            id: basename.trim_end_matches(".jsonl").to_string(),
            path: path.to_path_buf(),
            project_path,
            modified: modified_utc,
            is_active,
        });
    })?;

    // Sort by modification time (most recent first)
    sessions.sort_by(|a, b| b.modified.cmp(&a.modified));

    if limit > 0 && sessions.len() > limit {
        sessions.truncate(limit);
    }

    Ok(sessions)
}
