use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::Value;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::types::{StreamItem, StreamItemType, AGENT_ID_DISPLAY_LENGTH};

/// When `true`, `parse_line` emits a `StreamItemType::Debug` for every line
/// (or attachment subtype) the parser would otherwise drop. Set once at
/// startup from a CLI flag; `Relaxed` is fine because we never read it
/// concurrently with a transition.
pub static DEBUG_ALL: AtomicBool = AtomicBool::new(false);

/// Cap on the raw-line preview attached to a debug stream item.
const DEBUG_PREVIEW_LEN: usize = 240;

/// Build "Main" or "Agent-<id>" (truncated) from an agent ID.
fn agent_display_name(agent_id: &str) -> String {
    if agent_id.is_empty() {
        "Main".to_string()
    } else {
        format!(
            "Agent-{}",
            &agent_id[..agent_id.len().min(AGENT_ID_DISPLAY_LENGTH)]
        )
    }
}

/// Raw message from JSONL file
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawMessage {
    #[serde(rename = "type")]
    msg_type: String,
    #[serde(default)]
    subtype: String,
    #[serde(default)]
    agent_id: String,
    #[serde(default)]
    session_id: String,
    #[serde(default)]
    timestamp: String,
    #[serde(default)]
    duration_ms: i64,
    #[serde(default)]
    message: Value,
    #[serde(default)]
    tool_use_result: Value,
    /// type="agent-name" lines: Claude's auto-generated session title.
    #[serde(default, rename = "agentName")]
    agent_title: String,
    /// type="custom-title" lines: user-set session title.
    #[serde(default, rename = "customTitle")]
    custom_title: String,
    /// system.compact_boundary metadata.
    #[serde(default)]
    compact_metadata: Option<CompactMetadata>,
    /// type="attachment" payload.
    #[serde(default)]
    attachment: Option<Attachment>,
    /// type="pr-link" fields.
    #[serde(default, rename = "prNumber")]
    pr_number: i64,
    #[serde(default, rename = "prUrl")]
    pr_url: String,
    #[serde(default, rename = "prRepository")]
    pr_repository: String,
}

/// Metadata on a system.compact_boundary line.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CompactMetadata {
    #[serde(default)]
    trigger: String,
    #[serde(default)]
    pre_tokens: i64,
}

/// Payload on a type="attachment" line. Subtype-dependent fields share one
/// struct to avoid per-subtype unmarshalling. `content` is intentionally
/// omitted because subtypes disagree on its shape (string for hook_success,
/// array for task_reminder); use `stdout` for hooks.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Attachment {
    #[serde(rename = "type")]
    attachment_type: String,
    #[serde(default)]
    hook_name: String,
    #[serde(default)]
    stdout: String,
    #[serde(default)]
    duration_ms: i64,
    #[serde(default)]
    files: Vec<DiagnosticFile>,
}

/// One file's worth of LSP diagnostics.
#[derive(Debug, Deserialize)]
struct DiagnosticFile {
    #[serde(default)]
    uri: String,
    #[serde(default)]
    diagnostics: Vec<Diagnostic>,
}

/// A single LSP finding.
#[derive(Debug, Deserialize)]
struct Diagnostic {
    #[serde(default)]
    message: String,
    #[serde(default)]
    severity: String,
    #[serde(default)]
    source: String,
}

/// Assistant message content
#[derive(Debug, Deserialize)]
struct AssistantMessage {
    #[serde(default)]
    content: Vec<ContentBlock>,
}

/// Content block in assistant message
#[derive(Debug, Deserialize)]
struct ContentBlock {
    #[serde(rename = "type")]
    block_type: String,
    #[serde(default)]
    text: String,
    #[serde(default)]
    thinking: String,
    #[serde(default)]
    id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    input: Value,
}

/// Tool input structure for common tools
#[derive(Debug, Deserialize, serde::Serialize, Default)]
struct ToolInput {
    #[serde(default)]
    command: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    pattern: String,
    #[serde(default)]
    path: String,
    #[serde(default)]
    file_path: String,
    #[serde(default)]
    content: String,
    #[serde(default)]
    prompt: String,
    #[serde(default)]
    query: String,
    #[serde(default)]
    skill: String,
    #[serde(default)]
    args: String,
    #[serde(default)]
    reason: String,
    #[serde(default, rename = "delaySeconds")]
    delay_seconds: i64,
    #[serde(default)]
    subject: String,
    #[serde(default, rename = "taskId")]
    task_id_camel: String,
    #[serde(default)]
    task_id: String,
    #[serde(default)]
    cron: String,
}

/// Shorten an MCP tool name. `mcp__plugin_context7_context7__query-docs`
/// becomes `mcp:query-docs` — users know which servers they configured, so
/// the method name is the only informative part in a narrow tree pane.
pub fn pretty_tool_name(name: &str) -> String {
    if !name.starts_with("mcp__") {
        return name.to_string();
    }
    match name.rfind("__") {
        Some(idx) if idx > 3 && idx + 2 < name.len() => format!("mcp:{}", &name[idx + 2..]),
        _ => name.to_string(),
    }
}

/// Parse a single JSONL line and return stream items
pub fn parse_line(line: &str) -> Result<Vec<StreamItem>> {
    let line = line.trim();
    if line.is_empty() {
        return Ok(vec![]);
    }

    // Try to parse as JSON first to check the type
    let value: Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(_) => return Ok(vec![]), // Skip malformed lines
    };

    // Get message type
    let msg_type = match value.get("type").and_then(|v| v.as_str()) {
        Some(t) => t,
        None => return Ok(vec![]), // Skip lines without type
    };

    let debug_all = DEBUG_ALL.load(Ordering::Relaxed);

    // Types we have a parser for. Anything else is dropped (or surfaced as
    // a Debug item when DEBUG_ALL is on).
    let handled = matches!(
        msg_type,
        "assistant" | "user" | "system" | "agent-name" | "custom-title" | "attachment" | "pr-link"
    );
    if !handled && !debug_all {
        return Ok(vec![]);
    }

    // Parse the full message
    let raw: RawMessage = match serde_json::from_value(value) {
        Ok(r) => r,
        Err(_) => return Ok(vec![]), // Skip if parsing fails
    };

    let timestamp = DateTime::parse_from_rfc3339(&raw.timestamp)
        .map(|dt| dt.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now());

    let items = match raw.msg_type.as_str() {
        "assistant" => parse_assistant_message(&raw, timestamp),
        "user" => parse_user_message(&raw, timestamp),
        "system" => {
            let items = parse_system_message(&raw, timestamp);
            if debug_all && items.is_empty() {
                vec![debug_item(&raw, line, timestamp)]
            } else {
                items
            }
        }
        "agent-name" => parse_session_title(&raw, timestamp, &raw.agent_title),
        "custom-title" => parse_session_title(&raw, timestamp, &raw.custom_title),
        "attachment" => {
            let items = parse_attachment(&raw, timestamp);
            if debug_all && items.is_empty() {
                vec![debug_item(&raw, line, timestamp)]
            } else {
                items
            }
        }
        "pr-link" => parse_pr_link(&raw, timestamp),
        _ => {
            if debug_all {
                vec![debug_item(&raw, line, timestamp)]
            } else {
                vec![]
            }
        }
    };

    Ok(items)
}

/// Build a Debug stream item describing a line that the parser would
/// otherwise drop. Label is `<type>`, `system:<subtype>`, or
/// `attachment.<subtype>`. Content is a truncated raw-JSON preview.
fn debug_item(raw: &RawMessage, line: &str, timestamp: DateTime<Utc>) -> StreamItem {
    let label = if raw.msg_type == "system" && !raw.subtype.is_empty() {
        format!("system:{}", raw.subtype)
    } else if raw.msg_type == "attachment" {
        match &raw.attachment {
            Some(a) if !a.attachment_type.is_empty() => {
                format!("attachment.{}", a.attachment_type)
            }
            _ => raw.msg_type.clone(),
        }
    } else {
        raw.msg_type.clone()
    };

    let preview = if line.len() > DEBUG_PREVIEW_LEN {
        let mut p = line[..DEBUG_PREVIEW_LEN].to_string();
        p.push('…');
        p
    } else {
        line.to_string()
    };

    StreamItem {
        item_type: StreamItemType::Debug,
        session_id: raw.session_id.clone(),
        agent_id: raw.agent_id.clone(),
        agent_name: agent_display_name(&raw.agent_id),
        timestamp,
        content: preview,
        tool_name: Some(label),
        tool_id: None,
        duration_ms: None,
        input_tokens: None,
        output_tokens: None,
        cache_creation_tokens: None,
        cache_read_tokens: None,
        model: None,
    }
}

/// Dispatch on attachment.type. Surfaces hook_success and diagnostics; every
/// other subtype is intentionally dropped (DEBUG_ALL surfaces the rest).
fn parse_attachment(raw: &RawMessage, timestamp: DateTime<Utc>) -> Vec<StreamItem> {
    let att = match &raw.attachment {
        Some(a) => a,
        None => return vec![],
    };
    let agent_name = agent_display_name(&raw.agent_id);

    match att.attachment_type.as_str() {
        "hook_success" => vec![StreamItem {
            item_type: StreamItemType::HookOutput,
            session_id: raw.session_id.clone(),
            agent_id: raw.agent_id.clone(),
            agent_name,
            timestamp,
            content: att.stdout.clone(),
            tool_name: Some(att.hook_name.clone()),
            tool_id: None,
            duration_ms: Some(att.duration_ms),
            input_tokens: None,
            output_tokens: None,
            cache_creation_tokens: None,
            cache_read_tokens: None,
            model: None,
        }],
        "diagnostics" => diagnostics_items(raw, timestamp, &agent_name, att),
        _ => vec![],
    }
}

/// One stream item per file with at least one diagnostic.
fn diagnostics_items(
    raw: &RawMessage,
    timestamp: DateTime<Utc>,
    agent_name: &str,
    att: &Attachment,
) -> Vec<StreamItem> {
    att.files
        .iter()
        .filter(|f| !f.diagnostics.is_empty())
        .map(|f| StreamItem {
            item_type: StreamItemType::Diagnostics,
            session_id: raw.session_id.clone(),
            agent_id: raw.agent_id.clone(),
            agent_name: agent_name.to_string(),
            timestamp,
            content: diagnostics_body(&f.diagnostics),
            tool_name: Some(diagnostics_header(f)),
            tool_id: None,
            duration_ms: None,
            input_tokens: None,
            output_tokens: None,
            cache_creation_tokens: None,
            cache_read_tokens: None,
            model: None,
        })
        .collect()
}

/// "<basename> (2 errors, 5 hints)"
fn diagnostics_header(f: &DiagnosticFile) -> String {
    use std::collections::HashMap;
    let mut counts: HashMap<String, usize> = HashMap::new();
    for d in &f.diagnostics {
        *counts.entry(d.severity.to_lowercase()).or_insert(0) += 1;
    }
    let mut parts = Vec::new();
    for sev in ["error", "warning", "info", "hint"] {
        if let Some(&n) = counts.get(sev) {
            if n > 0 {
                let label = if n == 1 {
                    sev.to_string()
                } else {
                    format!("{}s", sev)
                };
                parts.push(format!("{} {}", n, label));
            }
        }
    }
    let name = f.uri.rsplit('/').next().unwrap_or(&f.uri);
    if parts.is_empty() {
        name.to_string()
    } else {
        format!("{} ({})", name, parts.join(", "))
    }
}

/// Each finding rendered "[severity] message (source)".
fn diagnostics_body(ds: &[Diagnostic]) -> String {
    ds.iter()
        .map(|d| {
            let sev = if d.severity.is_empty() {
                "?"
            } else {
                &d.severity
            };
            if d.source.is_empty() {
                format!("[{}] {}", sev, d.message)
            } else {
                format!("[{}] {} ({})", sev, d.message, d.source)
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// PR-link marker: "PR #N repo → url"
fn parse_pr_link(raw: &RawMessage, timestamp: DateTime<Utc>) -> Vec<StreamItem> {
    if raw.pr_number == 0 && raw.pr_url.is_empty() {
        return vec![];
    }
    let content = if !raw.pr_repository.is_empty() && !raw.pr_url.is_empty() {
        format!(
            "PR #{} {} → {}",
            raw.pr_number, raw.pr_repository, raw.pr_url
        )
    } else if !raw.pr_url.is_empty() {
        format!("PR #{} → {}", raw.pr_number, raw.pr_url)
    } else {
        format!("PR #{}", raw.pr_number)
    };
    vec![StreamItem {
        item_type: StreamItemType::PRLink,
        session_id: raw.session_id.clone(),
        agent_id: String::new(),
        agent_name: String::new(),
        timestamp,
        content,
        tool_name: None,
        tool_id: None,
        duration_ms: None,
        input_tokens: None,
        output_tokens: None,
        cache_creation_tokens: None,
        cache_read_tokens: None,
        model: None,
    }]
}

/// Handle system-type JSONL lines. Surfaces:
///   - subtype=turn_duration → TurnMarker (turn ended + duration)
///   - subtype=compact_boundary → CompactMarker (auto/manual compaction with
///     pre-tokens count)
///
/// Other subtypes are intentionally dropped.
fn parse_system_message(raw: &RawMessage, timestamp: DateTime<Utc>) -> Vec<StreamItem> {
    let agent_name = agent_display_name(&raw.agent_id);
    match raw.subtype.as_str() {
        "turn_duration" => vec![StreamItem {
            item_type: StreamItemType::TurnMarker,
            session_id: raw.session_id.clone(),
            agent_id: raw.agent_id.clone(),
            agent_name,
            timestamp,
            content: String::new(),
            tool_name: None,
            tool_id: None,
            duration_ms: Some(raw.duration_ms),
            input_tokens: None,
            output_tokens: None,
            cache_creation_tokens: None,
            cache_read_tokens: None,
            model: None,
        }],
        "compact_boundary" => vec![StreamItem {
            item_type: StreamItemType::CompactMarker,
            session_id: raw.session_id.clone(),
            agent_id: raw.agent_id.clone(),
            agent_name,
            timestamp,
            content: format_compact_summary(raw.compact_metadata.as_ref()),
            tool_name: None,
            tool_id: None,
            duration_ms: None,
            input_tokens: None,
            output_tokens: None,
            cache_creation_tokens: None,
            cache_read_tokens: None,
            model: None,
        }],
        _ => vec![],
    }
}

/// "auto, 179k pre-tokens" — empty when no metadata.
fn format_compact_summary(m: Option<&CompactMetadata>) -> String {
    let m = match m {
        Some(m) => m,
        None => return String::new(),
    };
    let mut parts = Vec::new();
    if !m.trigger.is_empty() {
        parts.push(m.trigger.clone());
    }
    if m.pre_tokens > 0 {
        parts.push(format!("{} pre-tokens", format_token_count(m.pre_tokens)));
    }
    parts.join(", ")
}

/// Render a token count as 1.2k / 179k / 2.3M.
fn format_token_count(n: i64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{}k", n / 1_000)
    } else {
        n.to_string()
    }
}

/// Resolve the *max context window* (in tokens) for a model id from
/// `message.model`. This is the model's hard ceiling — NOT the user's
/// `autoCompactWindow` setting. Update this table as new models ship;
/// unknown models fall back to 200k (the conservative current floor).
pub fn context_window_for(model: &str) -> i64 {
    if model.starts_with("claude-opus-4-7") || model.starts_with("claude-sonnet-4-6") {
        return 1_000_000;
    }
    if model.starts_with("claude-haiku-4-5") {
        return 200_000;
    }
    200_000
}

/// Emit a TypeSessionTitle item carrying a human-readable label for the
/// session. Both `agent-name` (Claude-generated) and `custom-title`
/// (user-set) feed through this.
fn parse_session_title(raw: &RawMessage, timestamp: DateTime<Utc>, title: &str) -> Vec<StreamItem> {
    if title.is_empty() {
        return vec![];
    }
    vec![StreamItem {
        item_type: StreamItemType::SessionTitle,
        session_id: raw.session_id.clone(),
        agent_id: String::new(),
        agent_name: String::new(),
        timestamp,
        content: title.to_string(),
        tool_name: None,
        tool_id: None,
        duration_ms: None,
        input_tokens: None,
        output_tokens: None,
        cache_creation_tokens: None,
        cache_read_tokens: None,
        model: None,
    }]
}

fn parse_assistant_message(raw: &RawMessage, timestamp: DateTime<Utc>) -> Vec<StreamItem> {
    let msg: AssistantMessage = match serde_json::from_value(raw.message.clone()) {
        Ok(m) => m,
        Err(_) => return vec![],
    };

    // Extract token usage from message.usage (including cache tokens — with
    // prompt caching default, cache_creation + cache_read are often the
    // majority of real usage).
    let usage = raw.message.get("usage");
    let input_tokens = usage
        .and_then(|u| u.get("input_tokens"))
        .and_then(|v| v.as_i64());
    let output_tokens = usage
        .and_then(|u| u.get("output_tokens"))
        .and_then(|v| v.as_i64());
    let cache_creation_tokens = usage
        .and_then(|u| u.get("cache_creation_input_tokens"))
        .and_then(|v| v.as_i64());
    let cache_read_tokens = usage
        .and_then(|u| u.get("cache_read_input_tokens"))
        .and_then(|v| v.as_i64());

    // message.model — used to look up the per-agent max context window.
    // Filter out "<synthetic>" (Claude Code's placeholder for synthesized
    // assistant turns); those carry no real model identity.
    let model = raw
        .message
        .get("model")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty() && *s != "<synthetic>")
        .map(|s| s.to_string());

    let agent_name = agent_display_name(&raw.agent_id);

    let mut items = Vec::new();
    let mut first = true;

    for block in msg.content {
        // Only attach token counts and model to the first item — context
        // size is a per-message snapshot, not per content-block.
        let (itok, otok, ctc, crc, mdl) = if first {
            first = false;
            (
                input_tokens,
                output_tokens,
                cache_creation_tokens,
                cache_read_tokens,
                model.clone(),
            )
        } else {
            (None, None, None, None, None)
        };

        match block.block_type.as_str() {
            "thinking" if !block.thinking.is_empty() => {
                items.push(StreamItem {
                    item_type: StreamItemType::Thinking,
                    session_id: raw.session_id.clone(),
                    agent_id: raw.agent_id.clone(),
                    agent_name: agent_name.clone(),
                    timestamp,
                    content: block.thinking,
                    tool_name: None,
                    tool_id: None,
                    duration_ms: None,
                    input_tokens: itok,
                    output_tokens: otok,
                    cache_creation_tokens: ctc,
                    cache_read_tokens: crc,
                    model: mdl,
                });
            }
            "text" if !block.text.is_empty() => {
                items.push(StreamItem {
                    item_type: StreamItemType::Text,
                    session_id: raw.session_id.clone(),
                    agent_id: raw.agent_id.clone(),
                    agent_name: agent_name.clone(),
                    timestamp,
                    content: block.text,
                    tool_name: None,
                    tool_id: None,
                    duration_ms: None,
                    input_tokens: itok,
                    output_tokens: otok,
                    cache_creation_tokens: ctc,
                    cache_read_tokens: crc,
                    model: mdl,
                });
            }
            "tool_use" => {
                let content = format_tool_input(&block.name, &block.input);
                items.push(StreamItem {
                    item_type: StreamItemType::ToolInput,
                    session_id: raw.session_id.clone(),
                    agent_id: raw.agent_id.clone(),
                    agent_name: agent_name.clone(),
                    timestamp,
                    content,
                    tool_name: Some(pretty_tool_name(&block.name)),
                    tool_id: Some(block.id),
                    duration_ms: None,
                    input_tokens: itok,
                    output_tokens: otok,
                    cache_creation_tokens: ctc,
                    cache_read_tokens: crc,
                    model: mdl,
                });
            }
            _ => {}
        }
    }

    items
}

fn parse_user_message(raw: &RawMessage, timestamp: DateTime<Utc>) -> Vec<StreamItem> {
    let agent_name = agent_display_name(&raw.agent_id);

    // Extract duration from toolUseResult.durationMs
    let duration_ms = raw
        .tool_use_result
        .get("durationMs")
        .and_then(|v| v.as_i64());

    let mut items = Vec::new();

    // Get the message content - can be string or array
    let message = &raw.message;
    let content = message.get("content");

    match content {
        // If content is an array, look for tool_result items
        Some(Value::Array(arr)) => {
            for item in arr {
                // Check if this is a tool_result
                if item.get("type").and_then(|v| v.as_str()) == Some("tool_result") {
                    let tool_use_id = item
                        .get("tool_use_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();

                    // Content can be a string or an array of objects
                    let result_content = extract_tool_result_content(item.get("content"));

                    if !result_content.is_empty() {
                        items.push(StreamItem {
                            item_type: StreamItemType::ToolOutput,
                            session_id: raw.session_id.clone(),
                            agent_id: raw.agent_id.clone(),
                            agent_name: agent_name.clone(),
                            timestamp,
                            content: result_content,
                            tool_name: None,
                            tool_id: Some(tool_use_id),
                            duration_ms,
                            input_tokens: None,
                            output_tokens: None,
                            cache_creation_tokens: None,
                            cache_read_tokens: None,
                            model: None,
                        });
                    }
                }
            }
        }
        // If content is a string, it's a user prompt - we don't show these
        Some(Value::String(_)) => {}
        _ => {}
    }

    items
}

/// Extract content from tool_result which can be string or array of text objects
fn extract_tool_result_content(content: Option<&Value>) -> String {
    match content {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(arr)) => {
            let mut texts = Vec::new();
            for item in arr {
                if let Some(text) = item.get("text").and_then(|v| v.as_str()) {
                    texts.push(text.to_string());
                }
            }
            texts.join("\n")
        }
        _ => String::new(),
    }
}

fn format_tool_input(tool_name: &str, input: &Value) -> String {
    let input: ToolInput = serde_json::from_value(input.clone()).unwrap_or_default();

    match tool_name {
        "Bash" => {
            if !input.description.is_empty() {
                format!("{}\n  # {}", input.command, input.description)
            } else {
                input.command
            }
        }
        "Read" => input.file_path,
        "Write" => {
            format!("{} ({} bytes)", input.file_path, input.content.len())
        }
        "Edit" => input.file_path,
        "Glob" => {
            if !input.path.is_empty() {
                format!("{} in {}", input.pattern, input.path)
            } else {
                input.pattern
            }
        }
        "Grep" => {
            if !input.path.is_empty() {
                format!("/{0}/ in {1}", input.pattern, input.path)
            } else {
                format!("/{}/", input.pattern)
            }
        }
        "WebFetch" => input.prompt,
        "WebSearch" => input.query,
        // "Task" is the legacy name; "Agent" is current (Claude Code 2.x).
        "Task" | "Agent" => {
            if !input.description.is_empty() {
                input.description
            } else {
                input.prompt
            }
        }
        "Skill" => {
            if !input.args.is_empty() {
                format!("{} — {}", input.skill, input.args)
            } else {
                input.skill
            }
        }
        "ToolSearch" => input.query,
        "ScheduleWakeup" => {
            if !input.reason.is_empty() {
                input.reason
            } else if input.delay_seconds > 0 {
                format!("delay {}s", input.delay_seconds)
            } else {
                String::new()
            }
        }
        "TaskCreate" => input.subject,
        "TaskUpdate" => {
            if !input.task_id_camel.is_empty() {
                format!("task {}", input.task_id_camel)
            } else {
                String::new()
            }
        }
        "TaskStop" => input.task_id,
        "EnterPlanMode" => "(enter plan mode)".to_string(),
        "ExitPlanMode" => "(exit plan mode)".to_string(),
        "CronCreate" => {
            if !input.cron.is_empty() && !input.prompt.is_empty() {
                format!("{}: {}", input.cron, input.prompt)
            } else {
                String::new()
            }
        }
        _ => {
            // For unknown tools, try to show something useful
            if !input.description.is_empty() {
                input.description
            } else if !input.prompt.is_empty() {
                input.prompt
            } else {
                // Return empty string for truly unknown tools
                String::new()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_empty_line() {
        let result = parse_line("").unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn test_parse_whitespace_line() {
        let result = parse_line("   \n\t  ").unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn test_parse_unknown_type() {
        let _g = debug_all_lock();
        DEBUG_ALL.store(false, Ordering::Relaxed);
        let line = r#"{"type":"file-history-snapshot","sessionId":"abc","timestamp":"2025-01-01T00:00:00Z"}"#;
        let result = parse_line(line).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn test_parse_assistant_thinking() {
        let line = r#"{"type":"assistant","sessionId":"sess-123","agentId":"","timestamp":"2025-01-01T00:00:00Z","message":{"role":"assistant","content":[{"type":"thinking","thinking":"Let me think about this..."}]}}"#;
        let result = parse_line(line).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].item_type, StreamItemType::Thinking);
        assert_eq!(result[0].content, "Let me think about this...");
        assert_eq!(result[0].agent_name, "Main");
    }

    #[test]
    fn test_parse_assistant_tool_use() {
        let line = r#"{"type":"assistant","sessionId":"sess-123","agentId":"abc1234","timestamp":"2025-01-01T00:00:00Z","message":{"role":"assistant","content":[{"type":"tool_use","id":"tool-1","name":"Read","input":{"file_path":"/tmp/test.txt"}}]}}"#;
        let result = parse_line(line).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].item_type, StreamItemType::ToolInput);
        assert_eq!(result[0].content, "/tmp/test.txt");
        assert_eq!(result[0].tool_name, Some("Read".to_string()));
    }

    #[test]
    fn test_parse_tool_result_string() {
        let line = r#"{"type":"user","sessionId":"sess-123","agentId":"","timestamp":"2025-01-01T00:00:00Z","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"tool-1","content":"File contents here"}]}}"#;
        let result = parse_line(line).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].item_type, StreamItemType::ToolOutput);
        assert_eq!(result[0].content, "File contents here");
    }

    #[test]
    fn test_parse_tool_result_array() {
        let line = r#"{"type":"user","sessionId":"sess-123","agentId":"","timestamp":"2025-01-01T00:00:00Z","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"tool-1","content":[{"type":"text","text":"First line"},{"type":"text","text":"Second line"}]}]}}"#;
        let result = parse_line(line).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].item_type, StreamItemType::ToolOutput);
        assert_eq!(result[0].content, "First line\nSecond line");
    }

    #[test]
    fn test_parse_user_prompt_ignored() {
        let line = r#"{"type":"user","sessionId":"sess-123","agentId":"","timestamp":"2025-01-01T00:00:00Z","message":{"role":"user","content":"Hello, help me with something"}}"#;
        let result = parse_line(line).unwrap();
        assert!(result.is_empty()); // User prompts are not shown
    }

    #[test]
    fn test_parse_invalid_json() {
        // Invalid JSON should be silently skipped
        let result = parse_line("not json at all").unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn test_parse_truncated_json() {
        // Truncated JSON (e.g. from oversized image) should be skipped
        let line = r#"{"type":"user","sessionId":"sess-123","timestamp":"2025-01-01T00:00:00Z","message":{"role":"user","content":[{"type":"image","source":{"type":"base64","data":"JVBER"#;
        let result = parse_line(line).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn test_parse_user_message_with_image() {
        // Image blocks should be silently skipped
        let line = r#"{"type":"user","sessionId":"sess-123","agentId":"","timestamp":"2025-01-01T00:00:00Z","message":{"role":"user","content":[{"type":"image","source":{"type":"base64","media_type":"image/png","data":"iVBORw0KGgo"}}]}}"#;
        let result = parse_line(line).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn test_parse_user_message_with_image_and_tool_result() {
        // Image + tool_result: only tool_result should be returned
        let line = r#"{"type":"user","sessionId":"sess-123","agentId":"","timestamp":"2025-01-01T00:00:00Z","message":{"role":"user","content":[{"type":"image","source":{"type":"base64","data":"iVBOR"}},{"type":"tool_result","tool_use_id":"toolu_img1","content":"tool output here"}]}}"#;
        let result = parse_line(line).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].content, "tool output here");
    }

    #[test]
    fn test_pretty_tool_name() {
        assert_eq!(pretty_tool_name("Bash"), "Bash");
        assert_eq!(pretty_tool_name("Skill"), "Skill");
        assert_eq!(
            pretty_tool_name("mcp__plugin_context7_context7__query-docs"),
            "mcp:query-docs"
        );
        assert_eq!(pretty_tool_name("mcp__context7__resolve"), "mcp:resolve");
        // No trailing method — pass through unchanged.
        assert_eq!(pretty_tool_name("mcp__weird"), "mcp__weird");
    }

    #[test]
    fn test_format_tool_input_new_tools() {
        fn fmt(name: &str, input: &str) -> String {
            let v: Value = serde_json::from_str(input).unwrap();
            format_tool_input(name, &v)
        }
        // Agent (renamed from Task) shows description when available
        assert_eq!(
            fmt(
                "Agent",
                r#"{"description":"audit deps","prompt":"check all"}"#
            ),
            "audit deps"
        );
        // Task still works (legacy alias)
        assert_eq!(
            fmt("Task", r#"{"description":"legacy task"}"#),
            "legacy task"
        );
        // Skill renders as "<skill> — <args>"
        assert_eq!(
            fmt("Skill", r#"{"skill":"beads:create","args":"--title x"}"#),
            "beads:create — --title x"
        );
        assert_eq!(fmt("Skill", r#"{"skill":"beads:list"}"#), "beads:list");
        assert_eq!(
            fmt("ToolSearch", r#"{"query":"select:Read","max_results":1}"#),
            "select:Read"
        );
        assert_eq!(
            fmt(
                "ScheduleWakeup",
                r#"{"delaySeconds":90,"reason":"watching build"}"#
            ),
            "watching build"
        );
        assert_eq!(fmt("ScheduleWakeup", r#"{"delaySeconds":90}"#), "delay 90s");
        assert_eq!(
            fmt(
                "TaskCreate",
                r#"{"subject":"write docs","activeForm":"writing"}"#
            ),
            "write docs"
        );
        assert_eq!(
            fmt("TaskUpdate", r#"{"taskId":"42","status":"in_progress"}"#),
            "task 42"
        );
        assert_eq!(fmt("TaskStop", r#"{"task_id":"abc123"}"#), "abc123");
        assert_eq!(fmt("EnterPlanMode", r#"{}"#), "(enter plan mode)");
        assert_eq!(fmt("ExitPlanMode", r#"{}"#), "(exit plan mode)");
        assert_eq!(
            fmt(
                "CronCreate",
                r#"{"cron":"*/5 * * * *","prompt":"ping","recurring":true}"#
            ),
            "*/5 * * * *: ping"
        );
    }

    #[test]
    fn test_parse_assistant_model() {
        // message.model should land on the first item only, with synthetic
        // placeholders filtered out.
        let line = r#"{"type":"assistant","sessionId":"s","agentId":"","timestamp":"2025-01-01T00:00:00Z","message":{"role":"assistant","model":"claude-opus-4-7","content":[{"type":"thinking","thinking":"hmm"},{"type":"text","text":"hi"}],"usage":{"input_tokens":10}}}"#;
        let items = parse_line(line).unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].model.as_deref(), Some("claude-opus-4-7"));
        assert_eq!(items[1].model, None, "model only on first item");

        let synthetic = r#"{"type":"assistant","sessionId":"s","agentId":"","timestamp":"2025-01-01T00:00:00Z","message":{"role":"assistant","model":"<synthetic>","content":[{"type":"text","text":"hi"}]}}"#;
        let items = parse_line(synthetic).unwrap();
        assert_eq!(items[0].model, None, "<synthetic> filtered out");
    }

    #[test]
    fn test_context_window_for() {
        assert_eq!(context_window_for("claude-opus-4-7"), 1_000_000);
        assert_eq!(context_window_for("claude-opus-4-7-20260101"), 1_000_000);
        assert_eq!(context_window_for("claude-sonnet-4-6"), 1_000_000);
        assert_eq!(context_window_for("claude-haiku-4-5"), 200_000);
        assert_eq!(context_window_for("claude-haiku-4-5-20251001"), 200_000);
        // Unknown models fall back to the conservative 200k floor.
        assert_eq!(context_window_for("claude-mystery-9-9"), 200_000);
        assert_eq!(context_window_for(""), 200_000);
    }

    #[test]
    fn test_parse_cache_tokens() {
        // cache_creation_input_tokens + cache_read_input_tokens should be
        // captured alongside input/output_tokens.
        let line = r#"{"type":"assistant","sessionId":"s","agentId":"","timestamp":"2025-01-01T00:00:00Z","message":{"role":"assistant","content":[{"type":"text","text":"hi"}],"usage":{"input_tokens":10,"output_tokens":5,"cache_creation_input_tokens":35656,"cache_read_input_tokens":1234}}}"#;
        let items = parse_line(line).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].input_tokens, Some(10));
        assert_eq!(items[0].output_tokens, Some(5));
        assert_eq!(items[0].cache_creation_tokens, Some(35656));
        assert_eq!(items[0].cache_read_tokens, Some(1234));
    }

    #[test]
    fn test_parse_turn_duration() {
        let line = r#"{"type":"system","subtype":"turn_duration","sessionId":"s","timestamp":"2025-01-01T00:00:00Z","durationMs":41751,"messageCount":42}"#;
        let items = parse_line(line).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].item_type, StreamItemType::TurnMarker);
        assert_eq!(items[0].duration_ms, Some(41751));
    }

    #[test]
    fn test_parse_system_unknown_subtype_dropped() {
        let _g = debug_all_lock();
        DEBUG_ALL.store(false, Ordering::Relaxed);
        // Non-turn_duration system messages are silently dropped.
        let line = r#"{"type":"system","subtype":"something_else","sessionId":"s","timestamp":"2025-01-01T00:00:00Z"}"#;
        assert!(parse_line(line).unwrap().is_empty());
    }

    #[test]
    fn test_parse_agent_name_title() {
        let line =
            r#"{"type":"agent-name","agentName":"auto-collapse-feature","sessionId":"sess-1"}"#;
        let items = parse_line(line).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].item_type, StreamItemType::SessionTitle);
        assert_eq!(items[0].content, "auto-collapse-feature");
        assert_eq!(items[0].session_id, "sess-1");
    }

    #[test]
    fn test_parse_custom_title() {
        let line = r#"{"type":"custom-title","customTitle":"my-label","sessionId":"sess-2"}"#;
        let items = parse_line(line).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].content, "my-label");
    }

    #[test]
    fn test_parse_mcp_tool_name_prettified() {
        let line = r#"{"type":"assistant","sessionId":"s","agentId":"","timestamp":"2025-01-01T00:00:00Z","message":{"role":"assistant","content":[{"type":"tool_use","id":"t1","name":"mcp__plugin_context7_context7__query-docs","input":{"library":"react"}}]}}"#;
        let items = parse_line(line).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].tool_name.as_deref(), Some("mcp:query-docs"));
    }

    #[test]
    fn test_parse_compact_boundary() {
        let line = r#"{"type":"system","subtype":"compact_boundary","sessionId":"abc","timestamp":"2025-01-01T12:00:00Z","compactMetadata":{"trigger":"auto","preTokens":179698}}"#;
        let items = parse_line(line).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].item_type, StreamItemType::CompactMarker);
        assert_eq!(items[0].content, "auto, 179k pre-tokens");
        assert_eq!(items[0].session_id, "abc");
    }

    #[test]
    fn test_parse_compact_boundary_no_metadata() {
        let line = r#"{"type":"system","subtype":"compact_boundary","sessionId":"abc","timestamp":"2025-01-01T12:00:00Z"}"#;
        let items = parse_line(line).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].item_type, StreamItemType::CompactMarker);
        assert_eq!(items[0].content, "");
    }

    #[test]
    fn test_parse_hook_success() {
        let line = r#"{"type":"attachment","sessionId":"abc","timestamp":"2025-01-01T12:00:00Z","attachment":{"type":"hook_success","hookName":"SessionStart:startup","hookEvent":"SessionStart","stdout":"hello\nworld","exitCode":0,"durationMs":116,"command":"bd prime"}}"#;
        let items = parse_line(line).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].item_type, StreamItemType::HookOutput);
        assert_eq!(items[0].tool_name.as_deref(), Some("SessionStart:startup"));
        assert_eq!(items[0].duration_ms, Some(116));
        assert_eq!(items[0].content, "hello\nworld");
    }

    #[test]
    fn test_parse_attachment_unknown_subtype_dropped() {
        let _g = debug_all_lock();
        DEBUG_ALL.store(false, Ordering::Relaxed);
        let line = r#"{"type":"attachment","sessionId":"abc","timestamp":"2025-01-01T12:00:00Z","attachment":{"type":"task_reminder","content":[],"itemCount":0}}"#;
        let items = parse_line(line).unwrap();
        assert!(items.is_empty());
    }

    #[test]
    fn test_parse_diagnostics() {
        let line = r#"{"type":"attachment","sessionId":"abc","timestamp":"2025-01-01T12:00:00Z","attachment":{"type":"diagnostics","files":[{"uri":"/path/to/foo.go","diagnostics":[{"message":"unused parameter","severity":"Info","source":"unusedparams"},{"message":"loop can be modernized","severity":"Hint","source":"rangeint"}]}]}}"#;
        let items = parse_line(line).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].item_type, StreamItemType::Diagnostics);
        assert_eq!(
            items[0].tool_name.as_deref(),
            Some("foo.go (1 info, 1 hint)")
        );
        assert!(items[0]
            .content
            .contains("[Info] unused parameter (unusedparams)"));
    }

    #[test]
    fn test_parse_diagnostics_empty_files_skipped() {
        let line = r#"{"type":"attachment","sessionId":"abc","timestamp":"2025-01-01T12:00:00Z","attachment":{"type":"diagnostics","files":[{"uri":"/x.go","diagnostics":[]}]}}"#;
        let items = parse_line(line).unwrap();
        assert!(items.is_empty());
    }

    #[test]
    fn test_parse_pr_link() {
        let line = r#"{"type":"pr-link","sessionId":"abc","prNumber":13,"prUrl":"https://github.com/phiat/claude-esp/pull/13","prRepository":"phiat/claude-esp","timestamp":"2025-01-01T12:00:00Z"}"#;
        let items = parse_line(line).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].item_type, StreamItemType::PRLink);
        assert_eq!(
            items[0].content,
            "PR #13 phiat/claude-esp → https://github.com/phiat/claude-esp/pull/13"
        );
    }

    /// DEBUG_ALL is process-global; serialize the toggle so tests don't race.
    /// Cargo runs tests in parallel by default — using a mutex here keeps
    /// tests that flip the flag from corrupting each other.
    fn debug_all_lock() -> std::sync::MutexGuard<'static, ()> {
        use std::sync::{Mutex, OnceLock};
        static M: OnceLock<Mutex<()>> = OnceLock::new();
        M.get_or_init(|| Mutex::new(())).lock().unwrap()
    }

    #[test]
    fn test_parse_debug_all() {
        let _g = debug_all_lock();
        DEBUG_ALL.store(true, Ordering::Relaxed);

        let cases = [
            (
                r#"{"type":"file-history-snapshot","sessionId":"s","timestamp":"2025-01-01T12:00:00Z"}"#,
                "file-history-snapshot",
            ),
            (
                r#"{"type":"system","subtype":"foo","sessionId":"s","timestamp":"2025-01-01T12:00:00Z"}"#,
                "system:foo",
            ),
            (
                r#"{"type":"attachment","sessionId":"s","timestamp":"2025-01-01T12:00:00Z","attachment":{"type":"task_reminder","content":[],"itemCount":0}}"#,
                "attachment.task_reminder",
            ),
        ];
        for (line, want_label) in cases {
            let items = parse_line(line).unwrap();
            assert_eq!(items.len(), 1, "{}", line);
            assert_eq!(items[0].item_type, StreamItemType::Debug);
            assert_eq!(items[0].tool_name.as_deref(), Some(want_label));
        }

        DEBUG_ALL.store(false, Ordering::Relaxed);
    }

    #[test]
    fn test_parse_debug_all_skips_handled_lines() {
        let _g = debug_all_lock();
        DEBUG_ALL.store(true, Ordering::Relaxed);

        // pr-link is handled → exactly one PRLink item, NOT a Debug entry.
        let line = r#"{"type":"pr-link","sessionId":"s","prNumber":1,"prUrl":"http://x","prRepository":"a/b","timestamp":"2025-01-01T12:00:00Z"}"#;
        let items = parse_line(line).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].item_type, StreamItemType::PRLink);

        DEBUG_ALL.store(false, Ordering::Relaxed);
    }
}
