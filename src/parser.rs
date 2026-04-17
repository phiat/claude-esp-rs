use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::Value;

use crate::types::{StreamItem, StreamItemType, AGENT_ID_DISPLAY_LENGTH};

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

    // We care about assistant/user (main content), system (turn-duration
    // markers), and agent-name/custom-title (session-title updates). Other
    // metadata types are silently dropped.
    let handled = matches!(
        msg_type,
        "assistant" | "user" | "system" | "agent-name" | "custom-title"
    );
    if !handled {
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
        "system" => parse_system_message(&raw, timestamp),
        "agent-name" => parse_session_title(&raw, timestamp, &raw.agent_title),
        "custom-title" => parse_session_title(&raw, timestamp, &raw.custom_title),
        _ => vec![],
    };

    Ok(items)
}

/// Handle system-type JSONL lines. Only surfaces subtype=turn_duration as
/// a subtle turn-boundary marker; other subtypes are dropped.
fn parse_system_message(raw: &RawMessage, timestamp: DateTime<Utc>) -> Vec<StreamItem> {
    if raw.subtype != "turn_duration" {
        return vec![];
    }
    let agent_name = if raw.agent_id.is_empty() {
        "Main".to_string()
    } else {
        format!(
            "Agent-{}",
            &raw.agent_id[..raw.agent_id.len().min(AGENT_ID_DISPLAY_LENGTH)]
        )
    };
    vec![StreamItem {
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
    }]
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

    let agent_name = if raw.agent_id.is_empty() {
        "Main".to_string()
    } else {
        format!(
            "Agent-{}",
            &raw.agent_id[..raw.agent_id.len().min(AGENT_ID_DISPLAY_LENGTH)]
        )
    };

    let mut items = Vec::new();
    let mut first = true;

    for block in msg.content {
        // Only attach token counts (including cache) to the first item
        let (itok, otok, ctc, crc) = if first {
            first = false;
            (
                input_tokens,
                output_tokens,
                cache_creation_tokens,
                cache_read_tokens,
            )
        } else {
            (None, None, None, None)
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
                });
            }
            _ => {}
        }
    }

    items
}

fn parse_user_message(raw: &RawMessage, timestamp: DateTime<Utc>) -> Vec<StreamItem> {
    let agent_name = if raw.agent_id.is_empty() {
        "Main".to_string()
    } else {
        format!(
            "Agent-{}",
            &raw.agent_id[..raw.agent_id.len().min(AGENT_ID_DISPLAY_LENGTH)]
        )
    };

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
}
