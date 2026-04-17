use chrono::{DateTime, Utc};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;

/// How many chars of agent ID to show in display name
pub const AGENT_ID_DISPLAY_LENGTH: usize = 7;

/// How many chars of tool ID to show
pub const TOOL_ID_DISPLAY_LENGTH: usize = 12;

/// Type of content in a stream item
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamItemType {
    Thinking,
    ToolInput,
    ToolOutput,
    Text,
    /// Turn boundary + duration from system.turn_duration JSONL lines.
    TurnMarker,
    /// Session label update from agent-name / custom-title JSONL lines.
    SessionTitle,
}

/// A single item in the output stream
#[derive(Debug, Clone)]
pub struct StreamItem {
    pub item_type: StreamItemType,
    pub session_id: String,
    pub agent_id: String,
    pub agent_name: String,
    pub timestamp: DateTime<Utc>,
    pub content: String,
    pub tool_name: Option<String>,
    pub tool_id: Option<String>,
    pub duration_ms: Option<i64>,
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    /// usage.cache_creation_input_tokens from assistant messages.
    pub cache_creation_tokens: Option<i64>,
    /// usage.cache_read_input_tokens from assistant messages.
    pub cache_read_tokens: Option<i64>,
}

/// A background task launched by an agent
#[derive(Debug, Clone)]
pub struct BackgroundTask {
    pub tool_id: String,
    pub parent_agent_id: String,
    pub tool_name: String,
    pub output_path: PathBuf,
    pub is_complete: bool,
}

/// A Claude Code session with its files
#[derive(Debug)]
pub struct Session {
    pub id: String,
    pub project_path: String,
    pub main_file: PathBuf,
    pub subagents: Arc<RwLock<HashMap<String, PathBuf>>>,
    pub subagent_types: Arc<RwLock<HashMap<String, String>>>,
    pub background_tasks: Arc<RwLock<HashMap<String, BackgroundTask>>>,
}

impl Session {
    pub fn new(id: String, project_path: String, main_file: PathBuf) -> Self {
        Self {
            id,
            project_path,
            main_file,
            subagents: Arc::new(RwLock::new(HashMap::new())),
            subagent_types: Arc::new(RwLock::new(HashMap::new())),
            background_tasks: Arc::new(RwLock::new(HashMap::new())),
        }
    }
}

/// Basic info about a session for listing
#[derive(Debug, Clone)]
pub struct SessionInfo {
    pub id: String,
    pub path: PathBuf,
    pub project_path: String,
    pub modified: DateTime<Utc>,
    pub is_active: bool,
}

/// Filter for enabled session/agent combinations
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnabledFilter {
    pub session_id: String,
    pub agent_id: String,
}

/// Message signaling a new agent was discovered
#[derive(Debug, Clone)]
pub struct NewAgentMsg {
    pub session_id: String,
    pub agent_id: String,
    pub agent_type: String,
}

/// Message signaling a new session was discovered
#[derive(Debug, Clone)]
pub struct NewSessionMsg {
    pub session_id: String,
    pub project_path: String,
}

/// Message signaling a new background task was discovered
#[derive(Debug, Clone)]
pub struct NewBackgroundTaskMsg {
    pub session_id: String,
    pub parent_agent_id: String,
    pub tool_id: String,
    pub tool_name: String,
    pub output_path: PathBuf,
    pub is_complete: bool,
}

/// Activity info for a session or agent
#[derive(Debug, Clone)]
pub struct ActivityInfo {
    pub session_id: String,
    pub agent_id: String,
    pub is_active: bool,
    /// Last file modification time — drives auto-collapse policy.
    pub last_modified: DateTime<Utc>,
}
