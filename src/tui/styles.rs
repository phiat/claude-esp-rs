use ratatui::style::{Color, Modifier, Style};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

// Colors matching Go version
pub const PRIMARY: Color = Color::Rgb(124, 58, 237); // #7C3AED Purple
pub const SECONDARY: Color = Color::Rgb(16, 185, 129); // #10B981 Green
pub const WARNING: Color = Color::Rgb(245, 158, 11); // #F59E0B Yellow/Orange
pub const ERROR: Color = Color::Rgb(239, 68, 68); // #EF4444 Red
pub const MUTED: Color = Color::Rgb(107, 114, 128); // #6B7280 Gray
pub const BG: Color = Color::Rgb(31, 41, 55); // #1F2937 Dark gray

// Content colors
pub const THINKING_CONTENT: Color = Color::Rgb(167, 139, 250); // #A78BFA
pub const TOOL_INPUT_CONTENT: Color = Color::Rgb(252, 211, 77); // #FCD34D
pub const TOOL_OUTPUT_CONTENT: Color = Color::Rgb(110, 231, 183); // #6EE7B7
pub const TEXT_CONTENT: Color = Color::Rgb(249, 250, 251); // #F9FAFB

// Hook (cyan) — system-injected output, distinct from tool calls.
pub const HOOK_HEADER: Color = Color::Rgb(6, 182, 212); // #06B6D4
pub const HOOK_CONTENT: Color = Color::Rgb(103, 232, 249); // #67E8F9

// Diagnostics (red-ish) — LSP findings after edits.
pub const DIAGNOSTICS_HEADER: Color = Color::Rgb(248, 113, 113); // #F87171
pub const DIAGNOSTICS_CONTENT: Color = Color::Rgb(252, 165, 165); // #FCA5A5

// Debug (dim grey) — used by the -D flag.
pub const DEBUG_HEADER: Color = Color::Rgb(156, 163, 175); // #9CA3AF
pub const DEBUG_CONTENT: Color = Color::Rgb(156, 163, 175); // #9CA3AF

// Agent colors
pub const MAIN_AGENT: Color = Color::Rgb(96, 165, 250); // #60A5FA
pub const SUB_AGENT: Color = Color::Rgb(244, 114, 182); // #F472B6

// Tree colors
pub const TREE_SELECTED_BG: Color = Color::Rgb(55, 65, 81); // #374151
pub const TREE_SELECTED_FG: Color = Color::Rgb(249, 250, 251); // #F9FAFB
pub const TREE_NORMAL: Color = Color::Rgb(209, 213, 219); // #D1D5DB

// Icons
pub const THINKING_ICON: &str = "🧠";
pub const TOOL_INPUT_ICON: &str = "🔧";
pub const TOOL_OUTPUT_ICON: &str = "📤";
pub const TEXT_ICON: &str = "💬";
pub const HOOK_ICON: &str = "🪝";
pub const DIAGNOSTICS_ICON: &str = "⚠";
pub const DEBUG_ICON: &str = "🔍";

pub const SESSION_ACTIVE_ICON: &str = "📁";
pub const SESSION_INACTIVE_ICON: &str = "📂";
pub const MAIN_ACTIVE_ICON: &str = "💬";
pub const MAIN_INACTIVE_ICON: &str = "💤";
pub const AGENT_ACTIVE_ICON: &str = "🤖";
pub const AGENT_INACTIVE_ICON: &str = "💤";
pub const TASK_COMPLETE_ICON: &str = "✓";
pub const TASK_RUNNING_ICON: &str = "⏳";

// Styles
pub fn thinking_header_style() -> Style {
    Style::default().fg(PRIMARY).add_modifier(Modifier::BOLD)
}

pub fn thinking_content_style() -> Style {
    Style::default().fg(THINKING_CONTENT)
}

pub fn tool_input_header_style() -> Style {
    Style::default().fg(WARNING).add_modifier(Modifier::BOLD)
}

pub fn tool_input_content_style() -> Style {
    Style::default().fg(TOOL_INPUT_CONTENT)
}

pub fn tool_output_header_style() -> Style {
    Style::default().fg(SECONDARY).add_modifier(Modifier::BOLD)
}

pub fn tool_output_content_style() -> Style {
    Style::default().fg(TOOL_OUTPUT_CONTENT)
}

pub fn text_header_style() -> Style {
    Style::default().fg(TEXT_CONTENT)
}

pub fn hook_header_style() -> Style {
    Style::default()
        .fg(HOOK_HEADER)
        .add_modifier(Modifier::BOLD)
}

pub fn hook_content_style() -> Style {
    Style::default().fg(HOOK_CONTENT)
}

pub fn diagnostics_header_style() -> Style {
    Style::default()
        .fg(DIAGNOSTICS_HEADER)
        .add_modifier(Modifier::BOLD)
}

pub fn diagnostics_content_style() -> Style {
    Style::default().fg(DIAGNOSTICS_CONTENT)
}

pub fn debug_header_style() -> Style {
    Style::default()
        .fg(DEBUG_HEADER)
        .add_modifier(Modifier::BOLD)
}

pub fn debug_content_style() -> Style {
    Style::default().fg(DEBUG_CONTENT)
}

pub fn main_agent_style() -> Style {
    Style::default().fg(MAIN_AGENT).add_modifier(Modifier::BOLD)
}

pub fn sub_agent_style() -> Style {
    Style::default().fg(SUB_AGENT).add_modifier(Modifier::BOLD)
}

pub fn tree_selected_style() -> Style {
    Style::default()
        .bg(TREE_SELECTED_BG)
        .fg(TREE_SELECTED_FG)
        .add_modifier(Modifier::BOLD)
}

pub fn tree_normal_style() -> Style {
    Style::default().fg(TREE_NORMAL)
}

pub fn header_style() -> Style {
    Style::default().bg(TREE_SELECTED_BG).fg(TREE_SELECTED_FG)
}

pub fn help_style() -> Style {
    Style::default().fg(MUTED)
}

pub fn separator_style() -> Style {
    Style::default().fg(MUTED)
}

pub fn muted_style() -> Style {
    Style::default().fg(MUTED)
}

/// API error marker - red, so failed/retrying requests stand out.
pub fn api_error_style() -> Style {
    Style::default().fg(ERROR)
}

pub fn border_style() -> Style {
    Style::default().fg(MUTED)
}

pub fn focused_border_style() -> Style {
    Style::default().fg(PRIMARY)
}

/// Truncate a string to `max` terminal columns, adding "..." if truncated.
///
/// Width is measured with unicode-width (same basis as the stream pane's
/// wrapping) and cuts always land on a char boundary, so multi-byte text
/// such as CJK session titles never splits mid-character.
pub fn truncate(s: &str, max: usize) -> String {
    if UnicodeWidthStr::width(s) <= max {
        return s.to_string();
    }
    let ellipsis = max > 3;
    let budget = if ellipsis { max - 3 } else { max };
    let mut out = String::new();
    let mut used = 0;
    for ch in s.chars() {
        let w = UnicodeWidthChar::width(ch).unwrap_or(0);
        if used + w > budget {
            break;
        }
        out.push(ch);
        used += w;
    }
    if ellipsis {
        out.push_str("...");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::truncate;
    use unicode_width::UnicodeWidthStr;

    #[test]
    fn test_truncate_ascii() {
        assert_eq!(truncate("short", 25), "short");
        assert_eq!(truncate("0123456789abcdef", 10), "0123456...");
        assert_eq!(truncate("0123456789", 3), "012");
    }

    #[test]
    fn test_truncate_cjk_session_title_fits() {
        // 26 bytes but only 22 display columns (14 ASCII + 4 CJK x 2). Byte
        // slicing both mis-measured this as over-long and panicked cutting it.
        let title = "Claude-esp-rs 安裝確認";
        assert_eq!(truncate(title, 25), title);
    }

    #[test]
    fn test_truncate_cjk_session_title_overflows() {
        let title = "Claude-esp-rs 安裝確認與完整操作方式";
        let result = truncate(title, 25);

        assert!(
            UnicodeWidthStr::width(result.as_str()) <= 25,
            "exceeds width 25: {:?}",
            result
        );
        assert!(result.ends_with("..."), "got: {:?}", result);
    }

    #[test]
    fn test_truncate_cjk_never_splits_char() {
        let title = "測試中文標題截斷行為不要壞掉";
        for max in 0..=40 {
            let result = truncate(title, max);
            assert!(
                UnicodeWidthStr::width(result.as_str()) <= max.max(3),
                "max {}: {:?}",
                max,
                result
            );
        }
    }

    #[test]
    fn test_truncate_emoji() {
        let result = truncate("Hello 🔧🔧🔧🔧🔧🔧 world", 15);
        assert!(
            UnicodeWidthStr::width(result.as_str()) <= 15,
            "exceeds width 15: {:?}",
            result
        );
    }
}
