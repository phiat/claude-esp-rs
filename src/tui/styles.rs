use ratatui::style::{Color, Modifier, Style};

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

pub const SESSION_ACTIVE_ICON: &str = "📁";
pub const SESSION_INACTIVE_ICON: &str = "📂";
pub const MAIN_ACTIVE_ICON: &str = "💬";
pub const MAIN_INACTIVE_ICON: &str = "💤";
pub const AGENT_ACTIVE_ICON: &str = "🤖";
pub const AGENT_INACTIVE_ICON: &str = "💤";
pub const TASK_COMPLETE_ICON: &str = "✓";
pub const TASK_RUNNING_ICON: &str = "⏳";

pub const CHECKBOX_CHECKED: &str = "☑";
pub const CHECKBOX_UNCHECKED: &str = "☐";

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

pub fn tree_checked_style() -> Style {
    Style::default().fg(SECONDARY)
}

pub fn tree_unchecked_style() -> Style {
    Style::default().fg(MUTED)
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

pub fn border_style() -> Style {
    Style::default().fg(MUTED)
}

pub fn focused_border_style() -> Style {
    Style::default().fg(PRIMARY)
}

/// Truncate a string to max length, adding "..." if truncated
pub fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else if max <= 3 {
        s[..max].to_string()
    } else {
        format!("{}...", &s[..max - 3])
    }
}
