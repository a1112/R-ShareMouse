//! Output formatting utilities

use colored::Colorize;

/// Print a success message (green)
pub fn success(message: &str) {
    println!("{}", format!("✓ {}", message).green());
}

/// Print an info message (blue/cyan)
pub fn info(message: &str) {
    println!("{}", format!("  {}", message).cyan());
}

/// Print a warning message (yellow)
pub fn warning(message: &str) {
    eprintln!("{}", format!("⚠ {}", message).yellow());
}

/// Print an error message (red)
pub fn error(message: &str) {
    eprintln!("{}", format!("✗ {}", message).red());
}

/// Print a header/section title
pub fn header(title: &str) {
    println!();
    println!("{}", title.bold().underline());
    println!("{}", "─".repeat(title.len()));
}

/// Print a key-value pair
pub fn kv(key: &str, value: &str) {
    println!("  {}: {}", key.bold(), value);
}

/// Print a table header
pub fn table_header(columns: &[&str]) {
    let row = columns.join("  ");
    println!("{}", row.bold());
}

/// Print a table row
pub fn table_row(columns: &[&str]) {
    println!("{}", columns.join("  "));
}

/// Print a status indicator
pub fn status_ok(label: &str) {
    println!("  [{}] {}", "OK".green(), label);
}

/// Print a status indicator
pub fn status_err(label: &str) {
    println!("  [{}] {}", "ERR".red(), label);
}
