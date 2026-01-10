pub mod stream;
pub mod styles;
pub mod tree;

use anyhow::Result;
use crossterm::{
    event::{self, Event, KeyCode, KeyModifiers},
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
    ExecutableCommand,
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame, Terminal,
};
use std::io::stdout;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

use self::stream::StreamView;
use self::styles::*;
use self::tree::{NodeType, TreeView};
use crate::types::{NewAgentMsg, NewBackgroundTaskMsg, NewSessionMsg, StreamItem};
use crate::watcher::{Watcher, WatcherChannels};

/// Which pane has focus
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Tree,
    Stream,
}

/// Cached session info for display
#[derive(Default)]
struct CachedSessionInfo {
    count: usize,
    single_session_id: Option<String>,
}

/// Main TUI application
pub struct App {
    tree: TreeView,
    stream: StreamView,
    watcher: Arc<Watcher>,
    focus: Focus,
    show_tree: bool,
    tree_width: u16,
    width: u16,
    height: u16,
    quitting: bool,
    error: Option<String>,
    cached_sessions: CachedSessionInfo,

    // Watcher channels
    item_rx: mpsc::Receiver<StreamItem>,
    new_agent_rx: mpsc::Receiver<NewAgentMsg>,
    new_session_rx: mpsc::Receiver<NewSessionMsg>,
    new_background_task_rx: mpsc::Receiver<NewBackgroundTaskMsg>,
    error_rx: mpsc::Receiver<anyhow::Error>,
}

impl App {
    /// Create a new App
    pub async fn new(
        watcher: Arc<Watcher>,
        channels: WatcherChannels,
    ) -> Self {
        let mut tree = TreeView::new();

        // Add existing sessions to tree and cache session info
        let sessions = watcher.get_sessions().await;
        let cached_sessions = CachedSessionInfo {
            count: sessions.len(),
            single_session_id: if sessions.len() == 1 {
                sessions.values().next().map(|s| s.id.clone())
            } else {
                None
            },
        };

        for session in sessions.values() {
            tree.add_session(&session.id, &session.project_path);

            // Add existing subagents
            let subagents = session.subagents.read().await;
            for agent_id in subagents.keys() {
                tree.add_agent(&session.id, agent_id);
            }
        }

        let mut stream = StreamView::new();
        stream.set_enabled_filters(tree.get_enabled_filters());

        Self {
            tree,
            stream,
            watcher,
            focus: Focus::Stream,
            show_tree: true,
            tree_width: 25,
            width: 0,
            height: 0,
            quitting: false,
            error: None,
            cached_sessions,
            item_rx: channels.items,
            new_agent_rx: channels.new_agent,
            new_session_rx: channels.new_session,
            new_background_task_rx: channels.new_background_task,
            error_rx: channels.errors,
        }
    }

    /// Run the TUI event loop
    pub async fn run(&mut self) -> Result<()> {
        // Setup terminal
        enable_raw_mode()?;
        stdout().execute(EnterAlternateScreen)?;
        let backend = CrosstermBackend::new(stdout());
        let mut terminal = Terminal::new(backend)?;

        // Start watcher
        self.watcher.clone().start();

        let result = self.event_loop(&mut terminal).await;

        // Cleanup
        self.watcher.stop();
        disable_raw_mode()?;
        stdout().execute(LeaveAlternateScreen)?;

        result
    }

    async fn event_loop(&mut self, terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>) -> Result<()> {
        loop {
            // Draw UI
            terminal.draw(|f| self.render(f))?;

            // Handle events with timeout for polling watcher
            if event::poll(Duration::from_millis(100))? {
                match event::read()? {
                    Event::Key(key) => {
                        self.handle_key(key.code, key.modifiers);
                    }
                    Event::Resize(w, h) => {
                        self.width = w;
                        self.height = h;
                        self.update_layout();
                    }
                    _ => {}
                }
            }

            if self.quitting {
                break;
            }

            // Poll watcher channels
            self.poll_watcher().await;

            // Update activity status
            self.update_activity().await;
        }

        Ok(())
    }

    fn handle_key(&mut self, code: KeyCode, modifiers: KeyModifiers) {
        match (code, modifiers) {
            (KeyCode::Char('q'), _) | (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
                self.quitting = true;
            }

            (KeyCode::Char('h'), KeyModifiers::NONE) => {
                self.show_tree = !self.show_tree;
                self.update_layout();
            }

            (KeyCode::Tab, _) => {
                self.focus = if self.focus == Focus::Tree {
                    Focus::Stream
                } else {
                    Focus::Tree
                };
            }

            (KeyCode::Char('t'), KeyModifiers::NONE) => {
                self.stream.toggle_thinking();
            }

            (KeyCode::Char('i'), KeyModifiers::NONE) => {
                self.stream.toggle_tool_input();
            }

            (KeyCode::Char('o'), KeyModifiers::NONE) => {
                self.stream.toggle_tool_output();
            }

            (KeyCode::Char('a'), KeyModifiers::NONE) => {
                self.stream.toggle_auto_scroll();
            }

            (KeyCode::Char('j'), KeyModifiers::NONE) | (KeyCode::Down, _) => {
                if self.focus == Focus::Tree {
                    self.tree.move_down();
                } else {
                    self.stream.scroll_down(3);
                }
            }

            (KeyCode::Char('k'), KeyModifiers::NONE) | (KeyCode::Up, _) => {
                if self.focus == Focus::Tree {
                    self.tree.move_up();
                } else {
                    self.stream.scroll_up(3);
                }
            }

            (KeyCode::Char(' '), _) | (KeyCode::Enter, _) => {
                if self.focus == Focus::Tree {
                    if let Some(node) = self.tree.get_selected_node() {
                        if node.node_type == NodeType::BackgroundTask {
                            self.load_background_task_output();
                        } else {
                            self.tree.toggle();
                            self.stream.set_enabled_filters(self.tree.get_enabled_filters());
                        }
                    }
                }
            }

            (KeyCode::Char('g'), KeyModifiers::NONE) => {
                self.stream.scroll_to_top();
            }

            (KeyCode::Char('G'), KeyModifiers::SHIFT) | (KeyCode::Char('G'), KeyModifiers::NONE) => {
                self.stream.scroll_to_bottom();
                // Enable auto-scroll when going to bottom (matches Go version)
                if !self.stream.is_auto_scroll_enabled() {
                    self.stream.toggle_auto_scroll();
                }
            }

            (KeyCode::Char('x'), KeyModifiers::NONE) | (KeyCode::Char('d'), KeyModifiers::NONE) => {
                if self.focus == Focus::Tree {
                    if let Some(session_id) = self.tree.get_selected_session() {
                        let watcher = self.watcher.clone();
                        let sid = session_id.clone();
                        tokio::spawn(async move {
                            watcher.remove_session(&sid).await;
                        });
                        self.tree.remove_session(&session_id);
                        self.stream.set_enabled_filters(self.tree.get_enabled_filters());
                        // Update cached session count
                        self.cached_sessions.count = self.cached_sessions.count.saturating_sub(1);
                        if self.cached_sessions.count != 1 {
                            self.cached_sessions.single_session_id = None;
                        }
                    }
                }
            }

            (KeyCode::Char('A'), KeyModifiers::SHIFT) | (KeyCode::Char('A'), KeyModifiers::NONE) => {
                self.watcher.toggle_auto_discovery();
            }

            _ => {}
        }
    }

    fn load_background_task_output(&mut self) {
        let node = match self.tree.get_selected_node() {
            Some(n) => n.clone(),
            None => return,
        };

        let output_path = match &node.output_path {
            Some(p) => p,
            None => return,
        };

        match std::fs::read_to_string(output_path) {
            Ok(content) => {
                let status_icon = if node.is_complete {
                    TASK_COMPLETE_ICON
                } else {
                    TASK_RUNNING_ICON
                };

                let item = StreamItem {
                    item_type: crate::types::StreamItemType::ToolOutput,
                    session_id: node.session_id.clone(),
                    agent_id: node.parent_agent_id.clone(),
                    agent_name: if node.parent_agent_id.is_empty() {
                        "Main".to_string()
                    } else {
                        format!("Agent-{}", &node.parent_agent_id[..node.parent_agent_id.len().min(7)])
                    },
                    timestamp: chrono::Utc::now(),
                    content,
                    tool_name: Some(format!("{} {}", status_icon, node.name)),
                    tool_id: Some(node.id.clone()),
                };

                self.stream.add_item(item);
                self.stream.scroll_to_bottom();
            }
            Err(e) => {
                self.error = Some(format!("Error reading task output: {}", e));
            }
        }
    }

    async fn poll_watcher(&mut self) {
        // Poll items channel
        while let Ok(item) = self.item_rx.try_recv() {
            self.stream.add_item(item);
        }

        // Poll new agent channel
        while let Ok(msg) = self.new_agent_rx.try_recv() {
            self.tree.add_agent(&msg.session_id, &msg.agent_id);
            self.stream.set_enabled_filters(self.tree.get_enabled_filters());
        }

        // Poll new session channel
        while let Ok(msg) = self.new_session_rx.try_recv() {
            self.tree.add_session(&msg.session_id, &msg.project_path);
            self.stream.set_enabled_filters(self.tree.get_enabled_filters());
            // Update cached session info
            self.cached_sessions.count += 1;
            if self.cached_sessions.count > 1 {
                self.cached_sessions.single_session_id = None;
            }
        }

        // Poll new background task channel
        while let Ok(msg) = self.new_background_task_rx.try_recv() {
            self.tree.add_background_task(
                &msg.session_id,
                &msg.parent_agent_id,
                &msg.tool_id,
                &msg.tool_name,
                msg.output_path,
                msg.is_complete,
            );
        }

        // Poll error channel
        while let Ok(err) = self.error_rx.try_recv() {
            self.error = Some(err.to_string());
        }
    }

    async fn update_activity(&mut self) {
        let activity = self.watcher.get_activity_info(Duration::from_secs(30)).await;
        for info in activity {
            self.tree.update_activity(&info.session_id, &info.agent_id, info.is_active);
        }
    }

    fn update_layout(&mut self) {
        if self.width == 0 || self.height == 0 {
            return;
        }

        let content_height = self.height.saturating_sub(4);

        if self.show_tree {
            self.tree.set_size(self.tree_width, content_height);
            self.stream.set_size(self.width - self.tree_width - 3, content_height);
        } else {
            self.stream.set_size(self.width - 2, content_height);
        }
    }

    fn render(&mut self, f: &mut Frame) {
        let size = f.area();
        self.width = size.width;
        self.height = size.height;
        self.update_layout();

        if self.quitting {
            let goodbye = Paragraph::new("Goodbye!");
            f.render_widget(goodbye, size);
            return;
        }

        if let Some(ref err) = self.error {
            let error = Paragraph::new(format!("Error: {}\n\nPress q to quit.", err));
            f.render_widget(error, size);
            return;
        }

        // Layout: header, content, help
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1), // Header
                Constraint::Min(1),    // Content
                Constraint::Length(1), // Help
            ])
            .split(size);

        // Render header
        self.render_header(f, chunks[0]);

        // Render content
        if self.show_tree {
            self.render_with_tree(f, chunks[1]);
        } else {
            self.render_stream_only(f, chunks[1]);
        }

        // Render help
        self.render_help(f, chunks[2]);
    }

    fn render_header(&self, f: &mut Frame, area: Rect) {
        let thinking_toggle = self.render_toggle("Thinking", self.stream.is_thinking_enabled(), "t");
        let tool_input_toggle = self.render_toggle("Tools", self.stream.is_tool_input_enabled(), "i");
        let tool_output_toggle = self.render_toggle("Output", self.stream.is_tool_output_enabled(), "o");
        let auto_scroll_toggle = self.render_toggle("Auto", self.stream.is_auto_scroll_enabled(), "a");
        let tree_toggle = self.render_toggle("Tree", self.show_tree, "h");

        // Session info from cache
        let auto_disc = if !self.watcher.is_auto_discovery_enabled() {
            " [paused]"
        } else {
            ""
        };

        let session_info = if let Some(ref id) = self.cached_sessions.single_session_id {
            format!("Session: {}{}", truncate(id, 12), auto_disc)
        } else {
            format!("{} sessions{}", self.cached_sessions.count, auto_disc)
        };

        let header_text = format!(
            "{}  {}  {}  {}  {}  │  {}",
            thinking_toggle,
            tool_input_toggle,
            tool_output_toggle,
            auto_scroll_toggle,
            tree_toggle,
            session_info
        );

        let header = Paragraph::new(Line::from(vec![Span::styled(header_text, header_style())]));
        f.render_widget(header, area);
    }

    fn render_toggle(&self, name: &str, enabled: bool, key: &str) -> String {
        let checkbox = if enabled {
            CHECKBOX_CHECKED
        } else {
            CHECKBOX_UNCHECKED
        };
        format!("{} {}[{}]", checkbox, name, key)
    }

    fn render_with_tree(&mut self, f: &mut Frame, area: Rect) {
        let chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Length(self.tree_width + 2),
                Constraint::Length(1),
                Constraint::Min(1),
            ])
            .split(area);

        // Tree pane
        let tree_border_style = if self.focus == Focus::Tree {
            focused_border_style()
        } else {
            border_style()
        };

        let tree_block = Block::default()
            .borders(Borders::ALL)
            .border_style(tree_border_style);

        let tree_inner = tree_block.inner(chunks[0]);
        f.render_widget(tree_block, chunks[0]);
        f.render_widget(&self.tree, tree_inner);

        // Stream pane
        let stream_border_style = if self.focus == Focus::Stream {
            focused_border_style()
        } else {
            border_style()
        };

        let stream_block = Block::default()
            .borders(Borders::ALL)
            .border_style(stream_border_style);

        let stream_inner = stream_block.inner(chunks[2]);
        f.render_widget(stream_block, chunks[2]);
        f.render_widget(&mut self.stream, stream_inner);
    }

    fn render_stream_only(&mut self, f: &mut Frame, area: Rect) {
        let stream_block = Block::default()
            .borders(Borders::ALL)
            .border_style(focused_border_style());

        let stream_inner = stream_block.inner(area);
        f.render_widget(stream_block, area);
        f.render_widget(&mut self.stream, stream_inner);
    }

    fn render_help(&self, f: &mut Frame, area: Rect) {
        let help_text = if self.focus == Focus::Tree {
            "j/k: navigate │ space: toggle │ x: remove │ A: auto-discover │ q: quit"
        } else {
            "j/k: scroll │ g/G: top/bottom │ A: auto-discover │ tab: tree │ q: quit"
        };

        let help = Paragraph::new(Line::from(vec![Span::styled(help_text, help_style())]));
        f.render_widget(help, area);
    }
}
