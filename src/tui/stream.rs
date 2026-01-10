use std::collections::HashSet;

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    text::{Line, Span},
    widgets::Widget,
};

use super::styles::*;
use crate::types::{EnabledFilter, StreamItem, StreamItemType};

/// Maximum number of items to keep in the stream
pub const MAX_STREAM_ITEMS: usize = 1000;

/// Maximum lines to display per stream item
pub const MAX_LINES_PER_ITEM: usize = 50;

/// Stream view widget for displaying items
pub struct StreamView {
    items: Vec<StreamItem>,
    seen_tool_ids: HashSet<String>, // dedupe tool input/output by tool_id
    scroll_offset: usize,
    auto_scroll: bool,
    width: u16,
    height: u16,

    // Type filters
    show_thinking: bool,
    show_tool_input: bool,
    show_tool_output: bool,
    show_text: bool,

    // Session/Agent filter
    enabled_filters: Vec<EnabledFilter>,

    // Cached rendered lines
    rendered_lines: Vec<Line<'static>>,
    needs_rerender: bool,
}

impl StreamView {
    pub fn new() -> Self {
        Self {
            items: Vec::new(),
            seen_tool_ids: HashSet::new(),
            scroll_offset: 0,
            auto_scroll: true,
            width: 80,
            height: 20,
            show_thinking: true,
            show_tool_input: true,
            show_tool_output: true,
            show_text: false,
            enabled_filters: Vec::new(),
            rendered_lines: Vec::new(),
            needs_rerender: true,
        }
    }

    /// Set dimensions
    pub fn set_size(&mut self, width: u16, height: u16) {
        if self.width != width || self.height != height {
            self.width = width;
            self.height = height;
            self.needs_rerender = true;
        }
    }

    /// Add an item to the stream
    pub fn add_item(&mut self, item: StreamItem) {
        // Deduplicate tool input/output by tool_id
        if let Some(ref tool_id) = item.tool_id {
            if !tool_id.is_empty() {
                if self.seen_tool_ids.contains(tool_id) {
                    return; // Skip duplicate
                }
                self.seen_tool_ids.insert(tool_id.clone());
            }
        }

        self.items.push(item);

        // Keep last MAX_STREAM_ITEMS
        if self.items.len() > MAX_STREAM_ITEMS {
            self.items.drain(0..self.items.len() - MAX_STREAM_ITEMS);
        }

        self.needs_rerender = true;
    }

    /// Set enabled session/agent filters
    pub fn set_enabled_filters(&mut self, filters: Vec<EnabledFilter>) {
        self.enabled_filters = filters;
        self.needs_rerender = true;
    }

    /// Toggle thinking visibility
    pub fn toggle_thinking(&mut self) {
        self.show_thinking = !self.show_thinking;
        self.needs_rerender = true;
    }

    /// Toggle tool input visibility
    pub fn toggle_tool_input(&mut self) {
        self.show_tool_input = !self.show_tool_input;
        self.needs_rerender = true;
    }

    /// Toggle tool output visibility
    pub fn toggle_tool_output(&mut self) {
        self.show_tool_output = !self.show_tool_output;
        self.needs_rerender = true;
    }

    /// Toggle auto-scroll
    pub fn toggle_auto_scroll(&mut self) {
        self.auto_scroll = !self.auto_scroll;
    }

    /// Scroll up
    pub fn scroll_up(&mut self, lines: usize) {
        self.auto_scroll = false;
        self.scroll_offset = self.scroll_offset.saturating_sub(lines);
    }

    /// Scroll down
    pub fn scroll_down(&mut self, lines: usize) {
        self.ensure_rendered();
        let max_offset = self.rendered_lines.len().saturating_sub(self.height as usize);
        self.scroll_offset = (self.scroll_offset + lines).min(max_offset);
    }

    /// Scroll to top
    pub fn scroll_to_top(&mut self) {
        self.auto_scroll = false;
        self.scroll_offset = 0;
    }

    /// Scroll to bottom and enable auto-scroll
    pub fn scroll_to_bottom(&mut self) {
        self.ensure_rendered();
        let max_offset = self.rendered_lines.len().saturating_sub(self.height as usize);
        self.scroll_offset = max_offset;
        self.auto_scroll = true;
    }

    // Getters for filter states
    pub fn is_thinking_enabled(&self) -> bool {
        self.show_thinking
    }

    pub fn is_tool_input_enabled(&self) -> bool {
        self.show_tool_input
    }

    pub fn is_tool_output_enabled(&self) -> bool {
        self.show_tool_output
    }

    pub fn is_auto_scroll_enabled(&self) -> bool {
        self.auto_scroll
    }

    /// Check if an item passes the filters
    fn is_item_enabled(&self, item: &StreamItem) -> bool {
        self.enabled_filters.iter().any(|f| {
            f.session_id == item.session_id && f.agent_id == item.agent_id
        })
    }

    /// Ensure lines are rendered
    fn ensure_rendered(&mut self) {
        if !self.needs_rerender {
            return;
        }

        self.rendered_lines.clear();
        let content_width = self.width.saturating_sub(4) as usize;

        // Collect items to render to avoid borrow conflicts
        let items_to_render: Vec<_> = self
            .items
            .iter()
            .filter(|item| {
                // Check session/agent filter
                if !self.is_item_enabled(item) {
                    return false;
                }

                // Check type filter
                match item.item_type {
                    StreamItemType::Thinking => self.show_thinking,
                    StreamItemType::ToolInput => self.show_tool_input,
                    StreamItemType::ToolOutput => self.show_tool_output,
                    StreamItemType::Text => self.show_text,
                }
            })
            .cloned()
            .collect();

        for item in items_to_render {
            self.render_item(&item, content_width);
        }

        self.needs_rerender = false;

        // Auto-scroll to bottom if enabled
        if self.auto_scroll {
            let max_offset = self.rendered_lines.len().saturating_sub(self.height as usize);
            self.scroll_offset = max_offset;
        }
    }

    fn render_item(&mut self, item: &StreamItem, width: usize) {
        // Agent name styling
        let agent_style = if item.agent_id.is_empty() {
            main_agent_style()
        } else {
            sub_agent_style()
        };

        // Header line with agent name and type
        let (icon, type_name, header_style) = match item.item_type {
            StreamItemType::Thinking => (
                THINKING_ICON,
                " Thinking".to_string(),
                thinking_header_style(),
            ),
            StreamItemType::ToolInput => {
                let tool_name = item.tool_name.as_deref().unwrap_or("Tool");
                (
                    TOOL_INPUT_ICON,
                    format!(" {}", tool_name),
                    tool_input_header_style(),
                )
            }
            StreamItemType::ToolOutput => (
                TOOL_OUTPUT_ICON,
                " Output".to_string(),
                tool_output_header_style(),
            ),
            StreamItemType::Text => (TEXT_ICON, " Response".to_string(), text_header_style()),
        };

        let header_line = Line::from(vec![
            Span::styled(item.agent_name.clone(), agent_style),
            Span::styled(" » ", separator_style()),
            Span::styled(format!("{}{}", icon, type_name), header_style),
        ]);
        self.rendered_lines.push(header_line);

        // Content lines
        let content_style = match item.item_type {
            StreamItemType::Thinking => thinking_content_style(),
            StreamItemType::ToolInput => tool_input_content_style(),
            StreamItemType::ToolOutput => tool_output_content_style(),
            StreamItemType::Text => text_header_style(),
        };

        let truncated = self.truncate_content(&item.content, width);
        for line in truncated.lines() {
            self.rendered_lines
                .push(Line::from(Span::styled(line.to_string(), content_style)));
        }

        // Separator (no extra blank line - matches Go version)
        let sep_width = width.min(60);
        let separator = "─".repeat(sep_width);
        self.rendered_lines
            .push(Line::from(Span::styled(separator, separator_style())));
    }

    fn truncate_content(&self, content: &str, width: usize) -> String {
        let lines: Vec<&str> = content.lines().collect();

        // Truncate number of lines
        let (lines, truncated_count) = if lines.len() > MAX_LINES_PER_ITEM {
            (&lines[..MAX_LINES_PER_ITEM], lines.len() - MAX_LINES_PER_ITEM)
        } else {
            (&lines[..], 0)
        };

        // Word wrap each line
        let mut wrapped: Vec<String> = Vec::new();
        for line in lines {
            if line.len() > width && width > 0 {
                let mut remaining = *line;
                while remaining.len() > width {
                    wrapped.push(remaining[..width].to_string());
                    remaining = &remaining[width..];
                }
                if !remaining.is_empty() {
                    wrapped.push(remaining.to_string());
                }
            } else {
                wrapped.push(line.to_string());
            }
        }

        let mut result = wrapped.join("\n");
        if truncated_count > 0 {
            result.push_str(&format!("\n... ({} more lines)", truncated_count));
        }

        result
    }

    /// Get visible lines for rendering
    pub fn get_visible_lines(&mut self) -> Vec<Line<'static>> {
        self.ensure_rendered();

        let start = self.scroll_offset;
        let end = (start + self.height as usize).min(self.rendered_lines.len());

        if start >= self.rendered_lines.len() {
            return Vec::new();
        }

        self.rendered_lines[start..end].to_vec()
    }
}

impl Default for StreamView {
    fn default() -> Self {
        Self::new()
    }
}

impl Widget for &mut StreamView {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let lines = self.get_visible_lines();

        for (i, line) in lines.iter().enumerate() {
            if i >= area.height as usize {
                break;
            }

            let y = area.y + i as u16;
            let mut x = area.x;

            for span in line.spans.iter() {
                // Calculate character width (accounting for Unicode)
                let char_count: usize = span.content.chars().count();
                let width = char_count as u16;

                if x + width > area.x + area.width {
                    break;
                }
                buf.set_span(x, y, span, width);
                x += width;
            }
        }
    }
}
