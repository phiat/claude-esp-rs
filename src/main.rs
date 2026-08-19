use anyhow::Result;
use clap::Parser;
use std::sync::Arc;
use std::time::Duration;

use claude_esp::parser;
use claude_esp::tui::styles::truncate;
use claude_esp::tui::App;
use claude_esp::watcher::{list_active_sessions, list_sessions, Watcher};
use std::sync::atomic::Ordering;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

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

    /// List active sessions (uses -w window)
    #[arg(short = 'a')]
    active: bool,

    /// Start from newest (skip history, live only)
    #[arg(short = 'n')]
    skip_history: bool,

    /// Poll interval in ms, fallback mode only (min 100)
    #[arg(short = 'p', default_value = "500")]
    poll_ms: u64,

    /// Active window in seconds (default 300 = 5 min)
    #[arg(short = 'w', default_value = "300")]
    active_window_secs: u64,

    /// Max sessions to show in tree (0=unlimited)
    #[arg(short = 'm', default_value = "0")]
    max_sessions: usize,

    /// Auto-collapse sessions inactive ≥ N seconds (0=disabled, e.g. 120 for 2 min)
    #[arg(short = 'c', default_value = "0")]
    collapse_after_secs: u64,

    /// Debug: surface raw type:subtype for every JSONL line type the parser would otherwise drop
    #[arg(short = 'D')]
    debug_all: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    parser::DEBUG_ALL.store(cli.debug_all, Ordering::Relaxed);

    let active_window = Duration::from_secs(cli.active_window_secs);

    if cli.active {
        return list_active_sessions_cmd(active_window);
    }

    if cli.list {
        return list_sessions_cmd();
    }

    // Validate poll interval
    let poll_ms = cli.poll_ms.max(100);

    let collapse_after = if cli.collapse_after_secs == 0 {
        None
    } else {
        Some(Duration::from_secs(cli.collapse_after_secs))
    };

    // Run TUI
    run_tui(
        cli.session.as_deref(),
        cli.skip_history,
        poll_ms,
        active_window,
        cli.max_sessions,
        collapse_after,
    )
    .await
}

fn list_active_sessions_cmd(active_window: Duration) -> Result<()> {
    let sessions = list_active_sessions(active_window)?;

    if sessions.is_empty() {
        println!(
            "No active sessions (none modified in last {}s)",
            active_window.as_secs()
        );
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

async fn run_tui(
    session_id: Option<&str>,
    skip_history: bool,
    poll_ms: u64,
    active_window: Duration,
    max_sessions: usize,
    collapse_after: Option<Duration>,
) -> Result<()> {
    let (watcher, channels) =
        Watcher::new(session_id, poll_ms, active_window, max_sessions).await?;

    if skip_history {
        watcher.set_skip_history(true);
    }

    let watcher = Arc::new(watcher);
    let mut app = App::new(watcher, channels, collapse_after).await;

    app.run().await
}

/// Truncate a path to `max` terminal columns, keeping the tail and prefixing
/// "..." if truncated.
///
/// Like `styles::truncate`, this measures with unicode-width and cuts on char
/// boundaries. Project paths are ASCII today because Claude Code encodes them
/// into directory names, but the byte slicing this replaces would panic the
/// moment a non-ASCII path reached it.
fn truncate_path(s: &str, max: usize) -> String {
    if UnicodeWidthStr::width(s) <= max {
        return s.to_string();
    }
    if max <= 3 {
        // Same head-cut behavior as before, without the byte slicing.
        return truncate(s, max);
    }
    // Keep the tail — the rightmost path segments carry the information.
    let budget = max - 3;
    let mut kept = String::new();
    let mut used = 0;
    for ch in s.chars().rev() {
        let w = UnicodeWidthChar::width(ch).unwrap_or(0);
        if used + w > budget {
            break;
        }
        kept.push(ch);
        used += w;
    }
    format!("...{}", kept.chars().rev().collect::<String>())
}

#[cfg(test)]
mod tests {
    use super::truncate_path;
    use unicode_width::UnicodeWidthStr;

    #[test]
    fn test_truncate_path_ascii() {
        assert_eq!(truncate_path("/short/path", 30), "/short/path");

        let full = "/very/long/path/that/exceeds/the/budget";
        let result = truncate_path(full, 20);
        assert!(
            UnicodeWidthStr::width(result.as_str()) <= 20,
            "got: {result:?}"
        );
        assert!(result.starts_with("..."), "got: {result:?}");
        assert!(
            full.ends_with(&result[3..]),
            "not a tail of the input: {result:?}"
        );
    }

    #[test]
    fn test_truncate_path_cjk_never_splits_char() {
        // Byte slicing panicked here, and the tail cut made it worse by
        // landing inside a character from the other direction.
        let path = "/Users/me/Documents/創世紀元網站專案";
        for max in 0..=50 {
            let result = truncate_path(path, max);
            assert!(
                UnicodeWidthStr::width(result.as_str()) <= max.max(3),
                "max {max}: {result:?}"
            );
        }
    }
}
