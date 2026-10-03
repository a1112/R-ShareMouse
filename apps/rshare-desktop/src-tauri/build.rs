use std::path::Path;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

fn main() {
    emit_build_metadata();
    tauri_build::build();
}

fn emit_build_metadata() {
    // Worktree HEAD lives outside this package; Tauri's config watches alone
    // cannot invalidate a cached source identity after the release commit.
    for name in [
        Some("HEAD".to_string()),
        git_output(&["symbolic-ref", "-q", "HEAD"]),
        Some("index".to_string()),
    ]
    .into_iter()
    .flatten()
    {
        if let Some(path) = git_output(&["rev-parse", "--git-path", &name]) {
            println!("cargo:rerun-if-changed={path}");
        }
    }
    println!(
        "cargo:rustc-env=RSHARE_BUILD_TIMESTAMP={}",
        unix_timestamp()
    );

    let git_hash =
        git_output(&["rev-parse", "--short", "HEAD"]).unwrap_or_else(|| "unknown".to_string());
    println!("cargo:rustc-env=RSHARE_BUILD_GIT_HASH={git_hash}");

    let git_dirty = match git_output(&["status", "--porcelain"]) {
        Some(status) if status.is_empty() => "clean",
        Some(_) => "dirty",
        None => "unknown",
    };
    println!("cargo:rustc-env=RSHARE_BUILD_DIRTY={git_dirty}");
}

fn git_output(args: &[&str]) -> Option<String> {
    let git = git_exe()?;
    let output = Command::new(git).args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }

    let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Some(value)
}

fn git_exe() -> Option<&'static str> {
    if has_command("git") {
        return Some("git");
    }

    #[cfg(windows)]
    {
        for candidate in [
            "C:\\Program Files\\Git\\bin\\git.exe",
            "C:\\Program Files\\Git\\cmd\\git.exe",
        ] {
            if has_path(candidate) {
                return Some(Box::leak(candidate.to_string().into_boxed_str()));
            }
        }
    }

    None
}

fn has_command(name: &str) -> bool {
    if !cfg!(windows) {
        return Command::new(name)
            .arg("--version")
            .output()
            .is_ok_and(|o| o.status.success());
    }

    Command::new(name)
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
}

fn has_path(candidate: &str) -> bool {
    Path::new(candidate).exists()
}

fn unix_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}
