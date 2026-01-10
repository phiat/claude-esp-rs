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
    agent_id: String,
    session_id: String,
    timestamp: String,
    #[serde(default)]
    message: Value,
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

    // Only process "assistant" and "user" message types
    if msg_type != "assistant" && msg_type != "user" {
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
        _ => vec![],
    };

    Ok(items)
}

fn parse_assistant_message(raw: &RawMessage, timestamp: DateTime<Utc>) -> Vec<StreamItem> {
    let msg: AssistantMessage = match serde_json::from_value(raw.message.clone()) {
        Ok(m) => m,
        Err(_) => return vec![],
    };

    let agent_name = if raw.agent_id.is_empty() {
        "Main".to_string()
    } else {
        format!(
            "Agent-{}",
            &raw.agent_id[..raw.agent_id.len().min(AGENT_ID_DISPLAY_LENGTH)]
        )
    };

    let mut items = Vec::new();

    for block in msg.content {
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
                    tool_name: Some(block.name),
                    tool_id: Some(block.id),
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
        "Task" => {
            if !input.prompt.is_empty() {
                // Truncate long Task prompts
                if input.prompt.len() > 100 {
                    format!("{}...", &input.prompt[..100])
                } else {
                    input.prompt
                }
            } else if !input.description.is_empty() {
                input.description
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
}
