use claude_esp::parser::parse_line;
use claude_esp::types::StreamItemType;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;

fn find_jsonl_files() -> Vec<PathBuf> {
    let home = std::env::var("HOME").unwrap();
    let claude_dir = PathBuf::from(home).join(".claude/projects");

    if !claude_dir.exists() {
        return vec![];
    }

    let mut files = vec![];
    for entry in std::fs::read_dir(&claude_dir).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        if path.is_dir() {
            for sub in std::fs::read_dir(&path).unwrap().flatten() {
                if sub.path().extension().is_some_and(|e| e == "jsonl") {
                    files.push(sub.path());
                }
            }
        }
    }
    files
}

#[test]
fn test_real_jsonl_parsing() {
    let files = find_jsonl_files();

    if files.is_empty() {
        println!("No JSONL files found, skipping test");
        return;
    }

    // Find a larger file for more thorough testing
    let jsonl_path = files.iter()
        .max_by_key(|f| std::fs::metadata(f).map(|m| m.len()).unwrap_or(0))
        .unwrap();

    println!("Testing with: {:?}", jsonl_path);

    let file = File::open(jsonl_path).unwrap();
    let reader = BufReader::new(file);

    let mut total = 0;
    let mut parsed = 0;
    let mut parse_errors = 0;

    let mut thinking_count = 0;
    let mut text_count = 0;
    let mut tool_input_count = 0;
    let mut tool_output_count = 0;

    for line in reader.lines() {
        let line = line.unwrap();
        total += 1;

        match parse_line(&line) {
            Ok(items) => {
                parsed += 1;
                for item in items {
                    match item.item_type {
                        StreamItemType::Thinking => thinking_count += 1,
                        StreamItemType::Text => text_count += 1,
                        StreamItemType::ToolInput => tool_input_count += 1,
                        StreamItemType::ToolOutput => tool_output_count += 1,
                    }
                }
            }
            Err(e) => {
                parse_errors += 1;
                eprintln!("Parse error at line {}: {}", total, e);
            }
        }
    }

    let total_items = thinking_count + text_count + tool_input_count + tool_output_count;

    println!("--- Results ---");
    println!("Total lines: {}", total);
    println!("Parsed OK: {}", parsed);
    println!("Parse errors: {}", parse_errors);
    println!("");
    println!("Items found:");
    println!("  Thinking: {}", thinking_count);
    println!("  Text: {}", text_count);
    println!("  Tool Input: {}", tool_input_count);
    println!("  Tool Output: {}", tool_output_count);
    println!("  Total: {}", total_items);

    // Verify no parse errors
    assert_eq!(parse_errors, 0, "Should have no parse errors");
    // Verify we parsed all lines
    assert_eq!(parsed, total, "Should have parsed all lines");
    // Verify we found some items
    assert!(total_items > 0, "Should have found some items");
}

#[test]
fn test_multiple_jsonl_files() {
    let files = find_jsonl_files();

    if files.is_empty() {
        println!("No JSONL files found, skipping test");
        return;
    }

    let mut files_tested = 0;
    let mut total_errors = 0;

    for jsonl_path in files.iter().take(5) {
        let file = match File::open(jsonl_path) {
            Ok(f) => f,
            Err(_) => continue,
        };
        let reader = BufReader::new(file);

        files_tested += 1;

        for (i, line) in reader.lines().enumerate() {
            let line = match line {
                Ok(l) => l,
                Err(_) => continue,
            };

            if let Err(e) = parse_line(&line) {
                total_errors += 1;
                eprintln!("Error in {:?} line {}: {}", jsonl_path, i + 1, e);
            }
        }
    }

    println!("Tested {} files", files_tested);
    println!("Total errors: {}", total_errors);

    assert_eq!(total_errors, 0, "Should have no parse errors across files");
}
