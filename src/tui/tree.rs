use ratatui::{
    buffer::Buffer,
    layout::Rect,
    text::{Line, Span},
    widgets::Widget,
};
use std::path::PathBuf;

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
    pub fn add_agent(&mut self, session_id: &str, agent_id: &str) {
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

        let display_id = &agent_id[..agent_id.len().min(AGENT_ID_DISPLAY_LENGTH)];
        let mut agent = TreeNode::new(
            NodeType::Agent,
            agent_id.to_string(),
            format!("Agent-{}", display_id),
        )
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

    /// Toggle enabled state of current node
    pub fn toggle(&mut self) {
        if let Some(node) = self.get_selected_node_mut() {
            node.enabled = !node.enabled;

            // If toggling a session, toggle all children too
            if node.node_type == NodeType::Session {
                let enabled = node.enabled;
                for child in &mut node.children {
                    child.enabled = enabled;
                }
            }
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

        for child in &node.children {
            self.collect_enabled_filters(child, filters);
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

        // Get children count without holding a reference
        let children_len = self
            .get_node_by_path(&path)
            .map(|n| n.children.len())
            .unwrap_or(0);

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

        // Checkbox
        let (checkbox, check_style) = if node.enabled {
            (CHECKBOX_CHECKED, tree_checked_style())
        } else {
            (CHECKBOX_UNCHECKED, tree_unchecked_style())
        };
        spans.push(Span::styled(checkbox, check_style));
        spans.push(Span::raw(" "));

        // Icon
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
        spans.push(Span::raw(format!("{} ", icon)));

        // Name
        let name_style = if is_selected {
            tree_selected_style()
        } else if !node.is_active && node.node_type != NodeType::Session {
            muted_style()
        } else {
            tree_normal_style()
        };
        spans.push(Span::styled(node.name.clone(), name_style));

        Line::from(spans)
    }
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
                let width = span.content.chars().count() as u16;
                if x + width > area.x + area.width {
                    break;
                }
                buf.set_span(x, y, span, width);
                x += width;
            }
        }
    }
}
