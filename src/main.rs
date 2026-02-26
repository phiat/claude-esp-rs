use anyhow::Result;
use clap::Parser;
use std::sync::Arc;
use std::time::Duration;

use claude_esp::tui::App;
use claude_esp::watcher::{list_active_sessions, list_sessions, Watcher};

const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Parser)]
#[command(
    name = "claude-esp",
    version = VERSION,
    about = "Stream Claude Code's hidden output to a separate terminal"
)]
struct Cli {
    /// Watch a specific session by ID
    #[arg(short = 's', value_name = "ID")]
    session: Option<String>,

    /// List recent sessions
    #[arg(short = 'l')]
    list: bool,

    /// List active sessions (modified in last 5 min)
    #[arg(short = 'a')]
    active: bool,

    /// Start from newest (skip history, live only)
    #[arg(short = 'n')]
    skip_history: bool,

    /// Poll interval in milliseconds (min 100)
    #[arg(short = 'p', default_value = "500")]
    poll_ms: u64,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    if cli.active {
        return list_active_sessions_cmd();
    }

    if cli.list {
        return list_sessions_cmd();
    }

    // Validate poll interval
    let poll_ms = cli.poll_ms.max(100);

    // Run TUI
    run_tui(cli.session.as_deref(), cli.skip_history, poll_ms).await
}

fn list_active_sessions_cmd() -> Result<()> {
    let sessions = list_active_sessions(Duration::from_secs(5 * 60))?;

    if sessions.is_empty() {
        println!("No active sessions (none modified in last 5 minutes)");
        return Ok(());
    }

    println!("Active sessions:");
    for s in sessions {
        let status = if s.is_active { "● " } else { "  " };
        let id_display = &s.id[..s.id.len().min(12)];
        let path_display = truncate_path(&s.project_path, 40);
        println!("  {}{}  {}", status, id_display, path_display);
    }

    Ok(())
}

fn list_sessions_cmd() -> Result<()> {
    let sessions = list_sessions(10)?;

    if sessions.is_empty() {
        println!("No sessions found");
        return Ok(());
    }

    println!("Recent sessions:");
    for s in sessions {
        let status = if s.is_active { "● " } else { "  " };
        let time = s.modified.format("%H:%M:%S");
        let id_display = &s.id[..s.id.len().min(12)];
        let path_display = truncate_path(&s.project_path, 30);
        println!("  {}{}  {}  {}", status, time, id_display, path_display);
    }

    Ok(())
}

async fn run_tui(session_id: Option<&str>, skip_history: bool, poll_ms: u64) -> Result<()> {
    let (watcher, channels) = Watcher::new(session_id, poll_ms).await?;

    if skip_history {
        watcher.set_skip_history(true);
    }

    let watcher = Arc::new(watcher);
    let mut app = App::new(watcher, channels).await;

    app.run().await
}

fn truncate_path(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else if max <= 3 {
        s[..max].to_string()
    } else {
        format!("...{}", &s[s.len() - max + 3..])
    }
}
