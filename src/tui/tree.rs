use ratatui::{
    buffer::Buffer,
    layout::Rect,
    text::{Line, Span},
    widgets::Widget,
};
use std::path::PathBuf;
use unicode_width::UnicodeWidthStr;

use super::styles::{self, *};
use crate::types::{EnabledFilter, AGENT_ID_DISPLAY_LENGTH};

/// Node type in the tree
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeType {
    Root,
    Session,
    Main,
    Agent,
    BackgroundTask,
}

/// A node in the session/agent tree
#[derive(Debug, Clone)]
pub struct TreeNode {
    pub node_type: NodeType,
    pub id: String,
    pub session_id: String,
    pub name: String,
    pub enabled: bool,
    pub is_active: bool,
    pub children: Vec<TreeNode>,
    // Background task specific
    pub parent_agent_id: String,
    pub output_path: Option<PathBuf>,
    pub is_complete: bool,
    /// Session-only: children hidden from tree navigation AND stream filter.
    pub collapsed: bool,
    /// Session-only: user manually expanded — suppress auto-collapse until wake.
    pub pinned: bool,
    /// Main/Agent only: latest snapshot of input + cache_creation + cache_read
    /// tokens from the most recent assistant message. 0 = not yet observed.
    pub context_tokens: i64,
    /// Main/Agent only: max context window (in tokens) for this node's model,
    /// resolved via parser::context_window_for. 0 = not yet observed.
    pub context_window: i64,
}

impl TreeNode {
    fn new(node_type: NodeType, id: String, name: String) -> Self {
        Self {
            node_type,
            id,
            session_id: String::new(),
            name,
            enabled: true,
            is_active: true,
            children: Vec::new(),
            parent_agent_id: String::new(),
            output_path: None,
            is_complete: false,
            collapsed: false,
            pinned: false,
            context_tokens: 0,
            context_window: 0,
        }
    }

    fn with_session_id(mut self, session_id: String) -> Self {
        self.session_id = session_id;
        self
    }
}

/// Reference to a node in the flattened tree with depth info
#[derive(Debug, Clone)]
struct FlatNode {
    depth: usize,
    is_last: bool,
    node_idx: Vec<usize>, // Path to node in tree
}

/// Tree view widget for sessions and agents
pub struct TreeView {
    root: TreeNode,
    flat_nodes: Vec<FlatNode>,
    cursor: usize,
    width: u16,
    height: u16,
}

impl TreeView {
    pub fn new() -> Self {
        let root = TreeNode::new(NodeType::Root, String::new(), "Sessions".to_string());
        let mut tree = Self {
            root,
            flat_nodes: Vec::new(),
            cursor: 0,
            width: 0,
            height: 0,
        };
        tree.rebuild_flat_list();
        tree
    }

    /// Add a session to the tree
    pub fn add_session(&mut self, session_id: &str, project_path: &str) {
        // Check if session already exists
        if self.root.children.iter().any(|c| c.id == session_id) {
            return;
        }

        // Create display name from project path
        let parts: Vec<&str> = project_path.split('/').collect();
        let display_name = if parts.len() > 2 {
            parts.last().unwrap_or(&project_path)
        } else {
            project_path
        };
        let display_name = styles::truncate(display_name, 15);

        let mut session = TreeNode::new(NodeType::Session, session_id.to_string(), display_name);
        session.is_active = true;

        // Add Main node under session
        let mut main = TreeNode::new(NodeType::Main, String::new(), "Main".to_string())
            .with_session_id(session_id.to_string());
        main.is_active = true;
        session.children.push(main);

        self.root.children.push(session);
        self.rebuild_flat_list();
    }

    /// Add an agent under a session
    pub fn add_agent(&mut self, session_id: &str, agent_id: &str, agent_type: &str) {
        let session = self
            .root
            .children
            .iter_mut()
            .find(|c| c.node_type == NodeType::Session && c.id == session_id);

        let session = match session {
            Some(s) => s,
            None => return,
        };

        // Check if agent already exists
        if session
            .children
            .iter()
            .any(|c| c.node_type == NodeType::Agent && c.id == agent_id)
        {
            return;
        }

        let display_name = if agent_type.is_empty() {
            let display_id = &agent_id[..agent_id.len().min(AGENT_ID_DISPLAY_LENGTH)];
            format!("Agent-{}", display_id)
        } else if let Some(pos) = agent_type.rfind(':') {
            agent_type[pos + 1..].to_string()
        } else {
            agent_type.to_string()
        };

        let mut agent = TreeNode::new(NodeType::Agent, agent_id.to_string(), display_name)
            .with_session_id(session_id.to_string());
        agent.is_active = true;

        session.children.push(agent);
        self.rebuild_flat_list();
    }

    /// Add a background task under an agent or main
    pub fn add_background_task(
        &mut self,
        session_id: &str,
        parent_agent_id: &str,
        tool_id: &str,
        tool_name: &str,
        output_path: PathBuf,
        is_complete: bool,
    ) {
        let session = self
            .root
            .children
            .iter_mut()
            .find(|c| c.node_type == NodeType::Session && c.id == session_id);

        let session = match session {
            Some(s) => s,
            None => return,
        };

        // Find parent (Main or Agent)
        let parent = session.children.iter_mut().find(|c| {
            if parent_agent_id.is_empty() {
                c.node_type == NodeType::Main
            } else {
                c.node_type == NodeType::Agent && c.id == parent_agent_id
            }
        });

        let parent = match parent {
            Some(p) => p,
            None => return,
        };

        // Check if task already exists
        if let Some(existing) = parent
            .children
            .iter_mut()
            .find(|c| c.node_type == NodeType::BackgroundTask && c.id == tool_id)
        {
            existing.is_complete = is_complete;
            return;
        }

        let display_name = styles::truncate(tool_name, 25);
        let mut task = TreeNode::new(NodeType::BackgroundTask, tool_id.to_string(), display_name)
            .with_session_id(session_id.to_string());
        task.parent_agent_id = parent_agent_id.to_string();
        task.output_path = Some(output_path);
        task.is_complete = is_complete;
        task.is_active = !is_complete;

        parent.children.push(task);
        self.rebuild_flat_list();
    }

    /// Remove a session from the tree
    pub fn remove_session(&mut self, session_id: &str) {
        self.root
            .children
            .retain(|c| c.node_type != NodeType::Session || c.id != session_id);
        self.rebuild_flat_list();
    }

    /// Move cursor up
    pub fn move_up(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
        }
    }

    /// Move cursor down
    pub fn move_down(&mut self) {
        if self.cursor < self.flat_nodes.len().saturating_sub(1) {
            self.cursor += 1;
        }
    }

    /// Toggle the current node's visibility.
    ///
    /// On a session node, space collapses/expands (hides children in the tree
    /// and filters them from the stream). Manually expanding pins the session
    /// so auto-collapse won't re-collapse it until the session wakes again.
    ///
    /// On Main/Agent nodes, space still toggles the Enabled flag.
    pub fn toggle(&mut self) {
        let needs_rebuild = if let Some(node) = self.get_selected_node_mut() {
            if node.node_type == NodeType::Session {
                node.collapsed = !node.collapsed;
                if !node.collapsed {
                    node.pinned = true;
                }
                true
            } else {
                node.enabled = !node.enabled;
                false
            }
        } else {
            false
        };
        if needs_rebuild {
            self.rebuild_flat_list();
        }
    }

    /// Update a session's collapse state. When collapsing, move the cursor to
    /// the session row if it was sitting on a now-hidden child.
    pub fn set_collapsed(&mut self, session_id: &str, collapsed: bool) {
        let (sess_idx, was_collapsed) = match self
            .root
            .children
            .iter()
            .enumerate()
            .find(|(_, c)| c.node_type == NodeType::Session && c.id == session_id)
        {
            Some((i, c)) => (i, c.collapsed),
            None => return,
        };
        if was_collapsed == collapsed {
            return;
        }

        // Was cursor on a child of this session?
        let cursor_was_inside_subtree = self
            .flat_nodes
            .get(self.cursor)
            .map(|f| f.node_idx.first() == Some(&sess_idx) && f.node_idx.len() > 1)
            .unwrap_or(false);

        self.root.children[sess_idx].collapsed = collapsed;
        self.rebuild_flat_list();

        // Cursor may have landed on now-hidden child; move it to the session row.
        if collapsed && cursor_was_inside_subtree {
            if let Some(i) = self
                .flat_nodes
                .iter()
                .position(|f| f.node_idx.as_slice() == [sess_idx])
            {
                self.cursor = i;
            }
        }
    }

    /// Set the user-pinned flag for a session. Pinned sessions are exempt
    /// from auto-collapse until they next wake up.
    pub fn set_pinned(&mut self, session_id: &str, pinned: bool) {
        if let Some(session) = self
            .root
            .children
            .iter_mut()
            .find(|c| c.node_type == NodeType::Session && c.id == session_id)
        {
            session.pinned = pinned;
        }
    }

    /// Update a session node's display label. Used to replace the project-
    /// path fallback when a JSONL `agent-name` or `custom-title` line
    /// provides a human-readable session title.
    pub fn set_session_title(&mut self, session_id: &str, title: &str) {
        if title.is_empty() {
            return;
        }
        if let Some(session) = self
            .root
            .children
            .iter_mut()
            .find(|c| c.node_type == NodeType::Session && c.id == session_id)
        {
            session.name = styles::truncate(title, 25);
        }
    }

    /// Read-only access to top-level session nodes (for collapse policy).
    pub fn sessions(&self) -> &[TreeNode] {
        &self.root.children
    }

    /// Solo the selected node: disable all others, enable only this one.
    /// If already soloed (i.e. it's the only enabled node), re-enable all.
    pub fn solo(&mut self) {
        let selected_path = match self.flat_nodes.get(self.cursor) {
            Some(flat) => flat.node_idx.clone(),
            None => return,
        };

        if self.is_soloed(&selected_path) {
            // Un-solo: re-enable everything
            Self::set_all_enabled(&mut self.root, true);
        } else {
            // Disable all sessions and their children (but keep root enabled
            // so collect_enabled_filters can traverse it)
            for session in &mut self.root.children {
                Self::set_all_enabled(session, false);
            }

            // Enable the selected node and the path to it
            match selected_path.len() {
                // Selected a session node — enable it and all children.
                // If collapsed, force-expand + pin so the stream actually
                // shows its output (the whole point of soloing).
                1 => {
                    if let Some(session) = self.root.children.get_mut(selected_path[0]) {
                        if session.collapsed {
                            session.collapsed = false;
                            session.pinned = true;
                        }
                        Self::set_all_enabled(session, true);
                    }
                }
                // Selected a child (Main/Agent) — enable it and its parent session
                _ => {
                    if let Some(session) = self.root.children.get_mut(selected_path[0]) {
                        session.enabled = true;
                        if let Some(child) = self.get_node_by_path_mut(&selected_path) {
                            child.enabled = true;
                        }
                    }
                }
            }
        }
    }

    /// Check if the selected node is currently soloed
    fn is_soloed(&self, selected_path: &[usize]) -> bool {
        let selected = match self.get_node_by_path(selected_path) {
            Some(n) => n,
            None => return false,
        };
        if !selected.enabled {
            return false;
        }

        // For a session: check all other sessions are fully disabled
        // For a child: check all other Main/Agent nodes across all sessions are disabled
        for (si, session) in self.root.children.iter().enumerate() {
            if selected.node_type == NodeType::Session {
                if si != selected_path[0] && session.enabled {
                    return false;
                }
            } else {
                for (ci, child) in session.children.iter().enumerate() {
                    if child.node_type == NodeType::BackgroundTask {
                        continue;
                    }
                    let is_selected = si == selected_path[0] && selected_path.get(1) == Some(&ci);
                    if !is_selected && child.enabled {
                        return false;
                    }
                }
            }
        }
        true
    }

    fn set_all_enabled(node: &mut TreeNode, enabled: bool) {
        node.enabled = enabled;
        for child in &mut node.children {
            Self::set_all_enabled(child, enabled);
        }
    }

    /// Get the currently selected node
    pub fn get_selected_node(&self) -> Option<&TreeNode> {
        self.get_node_by_path(&self.flat_nodes.get(self.cursor)?.node_idx)
    }

    /// Get the currently selected node (mutable)
    fn get_selected_node_mut(&mut self) -> Option<&mut TreeNode> {
        let path = self.flat_nodes.get(self.cursor)?.node_idx.clone();
        self.get_node_by_path_mut(&path)
    }

    /// Get the session ID of the currently selected node
    pub fn get_selected_session(&self) -> Option<String> {
        let node = self.get_selected_node()?;
        match node.node_type {
            NodeType::Session => Some(node.id.clone()),
            NodeType::Main | NodeType::Agent | NodeType::BackgroundTask => {
                Some(node.session_id.clone())
            }
            NodeType::Root => None,
        }
    }

    /// Get list of enabled session/agent combinations
    pub fn get_enabled_filters(&self) -> Vec<EnabledFilter> {
        let mut filters = Vec::new();
        self.collect_enabled_filters(&self.root, &mut filters);
        filters
    }

    fn collect_enabled_filters(&self, node: &TreeNode, filters: &mut Vec<EnabledFilter>) {
        if !node.enabled {
            return;
        }

        match node.node_type {
            NodeType::Main => {
                filters.push(EnabledFilter {
                    session_id: node.session_id.clone(),
                    agent_id: String::new(),
                });
            }
            NodeType::Agent => {
                filters.push(EnabledFilter {
                    session_id: node.session_id.clone(),
                    agent_id: node.id.clone(),
                });
            }
            _ => {}
        }

        // Collapsed sessions intentionally drop their children from the
        // stream filter — the whole point is to stop sleeping sessions
        // from crowding the view.
        if node.node_type == NodeType::Session && node.collapsed {
            return;
        }
        for child in &node.children {
            self.collect_enabled_filters(child, filters);
        }
    }

    /// Update the per-(session, agent) context-size snapshot. Overwrites
    /// rather than accumulates — context size is a rolling value reported
    /// fresh by every assistant turn. Empty `agent_id` targets the Main row.
    pub fn update_context(&mut self, session_id: &str, agent_id: &str, tokens: i64, window: i64) {
        for child in self.root.children.iter_mut() {
            if child.node_type != NodeType::Session || child.id != session_id {
                continue;
            }
            for c in child.children.iter_mut() {
                let is_target = (c.node_type == NodeType::Main && agent_id.is_empty())
                    || (c.node_type == NodeType::Agent && c.id == agent_id);
                if is_target {
                    c.context_tokens = tokens;
                    c.context_window = window;
                    return;
                }
            }
        }
    }

    /// Update activity status for a session/agent
    pub fn update_activity(&mut self, session_id: &str, agent_id: &str, is_active: bool) {
        // Find and update the session
        let mut session_idx = None;
        for (i, child) in self.root.children.iter_mut().enumerate() {
            if child.node_type == NodeType::Session && child.id == session_id {
                let mut session_has_active = false;

                for c in &mut child.children {
                    let is_target = (c.node_type == NodeType::Main && agent_id.is_empty())
                        || (c.node_type == NodeType::Agent && c.id == agent_id);
                    if is_target {
                        c.is_active = is_active;
                    }
                    if c.is_active {
                        session_has_active = true;
                    }
                }
                child.is_active = session_has_active;
                session_idx = Some(i);
                break;
            }
        }

        // Sort children of the found session
        if let Some(idx) = session_idx {
            Self::sort_children_static(&mut self.root.children[idx]);
        }

        // Sort root children
        Self::sort_children_static(&mut self.root);
        self.rebuild_flat_list();
    }

    fn sort_children_static(parent: &mut TreeNode) {
        if parent.children.len() <= 1 {
            return;
        }

        // Bubble sort: active nodes up (but Main stays first)
        for i in 1..parent.children.len() {
            for j in (1..=i).rev() {
                let curr_active = parent.children[j].is_active;
                let prev_active = parent.children[j - 1].is_active;
                let prev_is_main = parent.children[j - 1].node_type == NodeType::Main;

                if curr_active && !prev_active && !prev_is_main {
                    parent.children.swap(j, j - 1);
                }
            }
        }
    }

    /// Set dimensions
    pub fn set_size(&mut self, width: u16, height: u16) {
        self.width = width;
        self.height = height;
    }

    /// Rebuild the flattened node list
    fn rebuild_flat_list(&mut self) {
        self.flat_nodes.clear();
        let num_children = self.root.children.len();
        for i in 0..num_children {
            let is_last = i == num_children - 1;
            self.flatten_node_recursive(vec![i], 0, is_last);
        }

        // Ensure cursor is valid
        if self.cursor >= self.flat_nodes.len() {
            self.cursor = self.flat_nodes.len().saturating_sub(1);
        }
    }

    fn flatten_node_recursive(&mut self, path: Vec<usize>, depth: usize, is_last: bool) {
        self.flat_nodes.push(FlatNode {
            depth,
            is_last,
            node_idx: path.clone(),
        });

        // Collapsed sessions hide their children from navigation AND from
        // the stream's enabled-filter set (which walks flat_nodes via
        // collect_enabled_filters through the tree — see below).
        let (children_len, skip_children) = match self.get_node_by_path(&path) {
            Some(n) => (
                n.children.len(),
                n.node_type == NodeType::Session && n.collapsed,
            ),
            None => (0, false),
        };
        if skip_children {
            return;
        }

        for i in 0..children_len {
            let mut child_path = path.clone();
            child_path.push(i);
            self.flatten_node_recursive(child_path, depth + 1, i == children_len - 1);
        }
    }

    fn get_node_by_path(&self, path: &[usize]) -> Option<&TreeNode> {
        let mut node = &self.root;
        for &idx in path {
            node = node.children.get(idx)?;
        }
        Some(node)
    }

    fn get_node_by_path_mut(&mut self, path: &[usize]) -> Option<&mut TreeNode> {
        let mut node = &mut self.root;
        for &idx in path {
            node = node.children.get_mut(idx)?;
        }
        Some(node)
    }

    /// Render the tree to lines
    pub fn render_lines(&self) -> Vec<Line<'static>> {
        if self.flat_nodes.is_empty() {
            return vec![Line::from(Span::styled(
                "Waiting for Claude Code sessions...",
                muted_style(),
            ))];
        }

        let mut lines = Vec::new();

        for (i, flat) in self.flat_nodes.iter().enumerate() {
            let node = match self.get_node_by_path(&flat.node_idx) {
                Some(n) => n,
                None => continue,
            };

            let is_selected = i == self.cursor;
            let line = self.render_node_line(node, flat.depth, flat.is_last, is_selected);
            lines.push(line);
        }

        lines
    }

    fn render_node_line(
        &self,
        node: &TreeNode,
        depth: usize,
        is_last: bool,
        is_selected: bool,
    ) -> Line<'static> {
        let mut spans = Vec::new();

        // Indent
        let indent = "  ".repeat(depth);
        spans.push(Span::raw(indent));

        // Branch character
        if depth > 0 {
            let branch = if is_last { "└─" } else { "├─" };
            spans.push(Span::raw(branch));
        }

        // Icon. Session nodes additionally carry a ▾/▸ collapse arrow.
        let session_arrow = if node.node_type == NodeType::Session {
            if node.collapsed {
                "▸"
            } else {
                "▾"
            }
        } else {
            ""
        };
        let icon = match node.node_type {
            NodeType::Root => "",
            NodeType::Session => {
                if node.is_active {
                    SESSION_ACTIVE_ICON
                } else {
                    SESSION_INACTIVE_ICON
                }
            }
            NodeType::Main => {
                if node.is_active {
                    MAIN_ACTIVE_ICON
                } else {
                    MAIN_INACTIVE_ICON
                }
            }
            NodeType::Agent => {
                if node.is_active {
                    AGENT_ACTIVE_ICON
                } else {
                    AGENT_INACTIVE_ICON
                }
            }
            NodeType::BackgroundTask => {
                if node.is_complete {
                    TASK_COMPLETE_ICON
                } else {
                    TASK_RUNNING_ICON
                }
            }
        };
        // Session: "📁▾ " / "📂▸ "; others: "💬 " / "🤖 " / etc.
        if !session_arrow.is_empty() {
            spans.push(Span::raw(format!("{}{} ", icon, session_arrow)));
        } else {
            spans.push(Span::raw(format!("{} ", icon)));
        }

        // Name. Collapsed sessions append "(+N)" showing hidden agent count
        // so no information is lost just because the subtree is hidden.
        let display_name = if node.node_type == NodeType::Session && node.collapsed {
            let agents = node
                .children
                .iter()
                .filter(|c| c.node_type == NodeType::Agent)
                .count();
            if agents > 0 {
                format!("{} (+{})", node.name, agents)
            } else {
                node.name.clone()
            }
        } else {
            node.name.clone()
        };
        let name_style = if is_selected {
            tree_selected_style()
        } else if !node.enabled || (!node.is_active && node.node_type != NodeType::Session) {
            muted_style()
        } else {
            tree_normal_style()
        };
        spans.push(Span::styled(display_name, name_style));

        // Per-(Main/Agent) context size as "<pct>%" — only renders once
        // we've parsed an assistant message with usage + model info.
        let suffix = context_suffix(node);
        if !suffix.is_empty() {
            spans.push(Span::raw(" "));
            spans.push(Span::styled(suffix, muted_style()));
        }

        Line::from(spans)
    }
}

/// Build "XX%" for Main/Agent nodes once we've recorded a context-size
/// snapshot. Empty for sessions, background tasks, and any node we
/// haven't seen an assistant turn for yet.
fn context_suffix(node: &TreeNode) -> String {
    if node.node_type != NodeType::Main && node.node_type != NodeType::Agent {
        return String::new();
    }
    if node.context_tokens <= 0 || node.context_window <= 0 {
        return String::new();
    }
    let pct = node.context_tokens.saturating_mul(100) / node.context_window;
    format!("{}%", pct)
}

impl Default for TreeView {
    fn default() -> Self {
        Self::new()
    }
}

impl Widget for &TreeView {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let lines = self.render_lines();

        for (i, line) in lines.iter().enumerate() {
            if i >= area.height as usize {
                break;
            }

            let y = area.y + i as u16;
            let mut x = area.x;

            for span in line.spans.iter() {
                let width = UnicodeWidthStr::width(span.content.as_ref()) as u16;
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
    fn toggle_on_session_collapses_not_disables() {
        let mut tv = TreeView::new();
        tv.add_session("s1", "project");
        tv.add_agent("s1", "agent1", "");
        tv.cursor = 0; // session row

        tv.toggle();
        let sess = &tv.root.children[0];
        assert!(sess.collapsed, "first toggle should collapse");
        assert!(sess.enabled, "Enabled flag untouched by collapse");

        tv.toggle();
        let sess = &tv.root.children[0];
        assert!(!sess.collapsed, "second toggle expands");
        assert!(sess.pinned, "manual expand pins");
    }

    #[test]
    fn collapse_hides_children_from_flatten_and_filters() {
        let mut tv = TreeView::new();
        tv.add_session("s1", "project");
        tv.add_agent("s1", "a1", "");
        assert_eq!(tv.flat_nodes.len(), 3, "pre-collapse: session+main+agent");

        tv.set_collapsed("s1", true);
        assert_eq!(tv.flat_nodes.len(), 1, "post-collapse: only session");
        assert_eq!(
            tv.get_enabled_filters().len(),
            0,
            "collapsed children dropped from filters"
        );
    }

    #[test]
    fn collapse_moves_cursor_out_of_hidden_subtree() {
        let mut tv = TreeView::new();
        tv.add_session("s1", "project");
        tv.add_agent("s1", "a1", "");
        tv.cursor = 2; // agent row

        tv.set_collapsed("s1", true);
        assert_eq!(tv.cursor, 0, "cursor should snap to session row");
    }

    #[test]
    fn solo_force_expands_collapsed_session() {
        let mut tv = TreeView::new();
        tv.add_session("s1", "p1");
        tv.add_agent("s1", "a1", "");
        tv.add_session("s2", "p2"); // need 2 sessions so solo isn't a no-op
        tv.set_collapsed("s1", true);
        tv.cursor = 0;

        tv.solo();
        let sess = &tv.root.children[0];
        assert!(!sess.collapsed, "solo should expand target");
        assert!(sess.pinned, "solo should pin target");
    }

    #[test]
    fn update_context_targets_main_and_agent() {
        let mut tv = TreeView::new();
        tv.add_session("s1", "p1");
        tv.add_agent("s1", "a1", "Explore");

        tv.update_context("s1", "", 180_000, 1_000_000);
        tv.update_context("s1", "a1", 18_000, 200_000);

        let main = tv.root.children[0]
            .children
            .iter()
            .find(|c| c.node_type == NodeType::Main)
            .unwrap();
        assert_eq!(main.context_tokens, 180_000);
        assert_eq!(main.context_window, 1_000_000);
        assert_eq!(context_suffix(main), "18%");

        let agent = tv.root.children[0]
            .children
            .iter()
            .find(|c| c.node_type == NodeType::Agent)
            .unwrap();
        assert_eq!(context_suffix(agent), "9%");
    }

    #[test]
    fn context_suffix_empty_until_observed() {
        let mut tv = TreeView::new();
        tv.add_session("s1", "p1");
        let main = tv.root.children[0]
            .children
            .iter()
            .find(|c| c.node_type == NodeType::Main)
            .unwrap();
        // No update_context call yet → no suffix.
        assert_eq!(context_suffix(main), "");
    }

    #[test]
    fn set_session_title_updates_name() {
        let mut tv = TreeView::new();
        tv.add_session("s1", "original-name");
        tv.set_session_title("s1", "human-readable-label");
        assert_eq!(tv.root.children[0].name, "human-readable-label");
    }
}
