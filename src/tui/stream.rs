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

/// Collapse whitespace into single spaces and truncate to `max` chars with
/// trailing ellipsis. Used by single-line markers (cache_miss, session_event)
/// where the source content may contain newlines or be much longer than the
/// marker line should be.
fn one_line_snippet(s: &str, max: usize) -> String {
    let flat: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    let count = flat.chars().count();
    if count <= max {
        return flat;
    }
    let mut out: String = flat.chars().take(max).collect();
    out.push('…');
    out
}

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
            show_text: true,
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
        // Deduplicate by (tool_id, item_type) so tool input and output
        // with the same tool_id are both kept
        if let Some(ref tool_id) = item.tool_id {
            if !tool_id.is_empty() {
                let dedup_key = format!("{}:{:?}", tool_id, item.item_type);
                if self.seen_tool_ids.contains(&dedup_key) {
                    return; // Skip duplicate
                }
                self.seen_tool_ids.insert(dedup_key);
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

    /// Toggle text visibility
    pub fn toggle_text(&mut self) {
        self.show_text = !self.show_text;
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
        let max_offset = self
            .rendered_lines
            .len()
            .saturating_sub(self.height as usize);
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
        let max_offset = self
            .rendered_lines
            .len()
            .saturating_sub(self.height as usize);
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

    pub fn is_text_enabled(&self) -> bool {
        self.show_text
    }

    pub fn is_auto_scroll_enabled(&self) -> bool {
        self.auto_scroll
    }

    /// Check if an item passes the filters
    fn is_item_enabled(&self, item: &StreamItem) -> bool {
        self.enabled_filters
            .iter()
            .any(|f| f.session_id == item.session_id && f.agent_id == item.agent_id)
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

                // Check type filter. Turn/compact/PR markers, hook output,
                // diagnostics, and debug entries are always shown — they have
                // no dedicated filter key. Session-title items are pure
                // metadata and never render in the stream.
                match item.item_type {
                    StreamItemType::Thinking => self.show_thinking,
                    StreamItemType::ToolInput => self.show_tool_input,
                    StreamItemType::ToolOutput => self.show_tool_output,
                    StreamItemType::Text => self.show_text,
                    StreamItemType::TurnMarker
                    | StreamItemType::CompactMarker
                    | StreamItemType::PRLink
                    | StreamItemType::HookOutput
                    | StreamItemType::Diagnostics
                    | StreamItemType::Debug
                    | StreamItemType::CacheMiss
                    | StreamItemType::SessionEvent => true,
                    StreamItemType::SessionTitle => false,
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
            let max_offset = self
                .rendered_lines
                .len()
                .saturating_sub(self.height as usize);
            self.scroll_offset = max_offset;
        }
    }

    fn render_item(&mut self, item: &StreamItem, width: usize) {
        // Single-line dividers — render and return without agent header /
        // trailing separator.
        match item.item_type {
            StreamItemType::TurnMarker => {
                let dur = match item.duration_ms {
                    Some(ms) if ms >= 1000 => format!("{:.1}s", ms as f64 / 1000.0),
                    Some(ms) if ms > 0 => format!("{}ms", ms),
                    _ => "?".to_string(),
                };
                self.rendered_lines.push(Line::from(Span::styled(
                    format!("── turn ended {} ──", dur),
                    muted_style(),
                )));
                return;
            }
            StreamItemType::CompactMarker => {
                let text = if item.content.is_empty() {
                    "── compacted ──".to_string()
                } else {
                    format!("── compacted ({}) ──", item.content)
                };
                self.rendered_lines
                    .push(Line::from(Span::styled(text, muted_style())));
                return;
            }
            StreamItemType::PRLink => {
                self.rendered_lines.push(Line::from(Span::styled(
                    format!("── {} ──", item.content),
                    muted_style(),
                )));
                return;
            }
            StreamItemType::CacheMiss => {
                let text = if item.content.is_empty() {
                    "── cache miss ──".to_string()
                } else {
                    format!("── cache miss: {} ──", item.content)
                };
                self.rendered_lines
                    .push(Line::from(Span::styled(text, muted_style())));
                return;
            }
            StreamItemType::SessionEvent => {
                let label = item
                    .tool_name
                    .as_deref()
                    .filter(|s| !s.is_empty())
                    .unwrap_or("event");
                let text = if item.content.is_empty() {
                    format!("── {} ──", label)
                } else {
                    format!("── {}: {} ──", label, one_line_snippet(&item.content, 80))
                };
                self.rendered_lines
                    .push(Line::from(Span::styled(text, muted_style())));
                return;
            }
            StreamItemType::SessionTitle => return,
            _ => {}
        }

        // Agent name styling
        let agent_style = if item.agent_id.is_empty() {
            main_agent_style()
        } else {
            sub_agent_style()
        };

        // Header line with agent name and type
        let (icon, type_name, header_style) = match item.item_type {
            // Unreachable: dividers returned early above.
            StreamItemType::TurnMarker
            | StreamItemType::CompactMarker
            | StreamItemType::PRLink
            | StreamItemType::CacheMiss
            | StreamItemType::SessionEvent
            | StreamItemType::SessionTitle => return,
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
            StreamItemType::ToolOutput => {
                let duration_str = match item.duration_ms {
                    Some(ms) if ms >= 1000 => format!(" ({:.1}s)", ms as f64 / 1000.0),
                    Some(ms) if ms > 0 => format!(" ({}ms)", ms),
                    _ => String::new(),
                };
                // Look up the tool name from the matching ToolInput
                let tool_name = item.tool_id.as_ref().and_then(|tid| {
                    self.items.iter().find_map(|i| {
                        if i.item_type == StreamItemType::ToolInput
                            && i.tool_id.as_ref() == Some(tid)
                        {
                            i.tool_name.clone()
                        } else {
                            None
                        }
                    })
                });
                let label = match tool_name {
                    Some(name) => format!(" {} result{}", name, duration_str),
                    None => format!(" Output{}", duration_str),
                };
                (TOOL_OUTPUT_ICON, label, tool_output_header_style())
            }
            StreamItemType::Text => (TEXT_ICON, " Response".to_string(), text_header_style()),
            StreamItemType::HookOutput => {
                let mut label = " Hook".to_string();
                if let Some(name) = &item.tool_name {
                    if !name.is_empty() {
                        label.push(' ');
                        label.push_str(name);
                    }
                }
                if let Some(ms) = item.duration_ms {
                    if ms >= 1000 {
                        label.push_str(&format!(" ({:.1}s)", ms as f64 / 1000.0));
                    } else if ms > 0 {
                        label.push_str(&format!(" ({}ms)", ms));
                    }
                }
                (HOOK_ICON, label, hook_header_style())
            }
            StreamItemType::Diagnostics => {
                let mut label = " Diagnostics".to_string();
                if let Some(name) = &item.tool_name {
                    if !name.is_empty() {
                        label.push(' ');
                        label.push_str(name);
                    }
                }
                (DIAGNOSTICS_ICON, label, diagnostics_header_style())
            }
            StreamItemType::Debug => {
                let mut label = " Debug".to_string();
                if let Some(name) = &item.tool_name {
                    if !name.is_empty() {
                        label.push(' ');
                        label.push_str(name);
                    }
                }
                (DEBUG_ICON, label, debug_header_style())
            }
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
            StreamItemType::HookOutput => hook_content_style(),
            StreamItemType::Diagnostics => diagnostics_content_style(),
            StreamItemType::Debug => debug_content_style(),
            // Unreachable: dividers returned early above.
            StreamItemType::TurnMarker
            | StreamItemType::CompactMarker
            | StreamItemType::PRLink
            | StreamItemType::CacheMiss
            | StreamItemType::SessionEvent
            | StreamItemType::SessionTitle => return,
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
            (
                &lines[..MAX_LINES_PER_ITEM],
                lines.len() - MAX_LINES_PER_ITEM,
            )
        } else {
            (&lines[..], 0)
        };

        // Word wrap each line using display width (handles CJK/emoji correctly)
        use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};
        let mut wrapped: Vec<String> = Vec::new();
        for line in lines {
            let line_width = UnicodeWidthStr::width(*line);
            if line_width > width && width > 0 {
                let mut remaining = *line;
                while UnicodeWidthStr::width(remaining) > width {
                    let mut col = 0;
                    let mut split_at = 0;
                    for (idx, ch) in remaining.char_indices() {
                        let cw = UnicodeWidthChar::width(ch).unwrap_or(0);
                        if col + cw > width {
                            break;
                        }
                        col += cw;
                        split_at = idx + ch.len_utf8();
                    }
                    if split_at == 0 {
                        // Single char wider than width — force advance
                        let ch = remaining.chars().next().unwrap();
                        split_at = ch.len_utf8();
                    }
                    wrapped.push(remaining[..split_at].to_string());
                    remaining = &remaining[split_at..];
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
                // Calculate display width (accounts for wide chars like emoji)
                let width = unicode_width::UnicodeWidthStr::width(span.content.as_ref()) as u16;

                if x + width > area.x + area.width {
                    break;
                }
                buf.set_span(x, y, span, width);
                x += width;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_truncate_content_cjk() {
        use unicode_width::UnicodeWidthStr;
        let mut s = StreamView::new();
        s.width = 80;

        // CJK characters: 3 bytes UTF-8, 2 display columns each
        let content = "# Step 5: 測試 focus-pane 回到原 pane";
        let result = s.truncate_content(content, 20);

        for line in result.lines() {
            let w = UnicodeWidthStr::width(line);
            assert!(w <= 20, "line exceeds width 20: {:?} (width {})", line, w);
        }
    }

    #[test]
    fn test_truncate_content_emoji() {
        use unicode_width::UnicodeWidthStr;
        let mut s = StreamView::new();
        s.width = 80;

        let content = "Hello 🔧🔧🔧🔧🔧🔧🔧🔧🔧🔧 world";
        let result = s.truncate_content(content, 15);

        for line in result.lines() {
            let w = UnicodeWidthStr::width(line);
            assert!(w <= 15, "line exceeds width 15: {:?} (width {})", line, w);
        }
    }
}
