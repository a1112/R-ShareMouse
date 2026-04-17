//! Logs command implementation

use anyhow::Result;
use crate::output::{header, warning};

/// Execute the logs command
pub async fn execute(lines: usize, follow: bool) -> Result<()> {
    header("Service Logs");

    // Get log file path
    let log_file = get_log_file()?;

    if !log_file.exists() {
        warning(&format!("Log file not found: {}", log_file.display()));
        warning("Service may not be running or logging may not be enabled");
        return Ok(());
    }

    if follow {
        follow_logs(&log_file).await?;
    } else {
        show_logs(&log_file, lines)?;
    }

    Ok(())
}

/// Show last N lines of log file
fn show_logs(log_file: &std::path::Path, lines: usize) -> Result<()> {
    let content = std::fs::read_to_string(log_file)?;

    let log_lines: Vec<&str> = content.lines().collect();

    let start = if log_lines.len() > lines {
        log_lines.len() - lines
    } else {
        0
    };

    for line in log_lines.iter().skip(start) {
        println!("{}", line);
    }

    Ok(())
}

/// Follow log file (tail -f)
async fn follow_logs(log_file: &std::path::Path) -> Result<()> {
    use tokio::fs::File;
    use tokio::io::{AsyncBufReadExt, BufReader};
    use tokio::time::Duration;

    warning("Following logs (press Ctrl+C to stop)");

    let file = File::open(log_file).await?;
    let reader = BufReader::new(file);
    let mut lines = reader.lines();

    // Follow mode
    loop {
        match lines.next_line().await {
            Ok(Some(line)) => {
                println!("{}", line);
            }
            Ok(None) => {
                // End of file, wait and check again
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            Err(e) => {
                eprintln!("Error reading log: {}", e);
                break;
            }
        }

        // Check for Ctrl+C
        if tokio::signal::ctrl_c().await.is_ok() {
            break;
        }
    }

    Ok(())
}

/// Get the log file path
fn get_log_file() -> Result<std::path::PathBuf> {
    let config_dir = directories::UserDirs::new()
        .map(|d| d.home_dir().to_path_buf())
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join(".rshare");

    Ok(config_dir.join("rshare.log"))
}
