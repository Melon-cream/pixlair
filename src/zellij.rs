use std::env;
use std::fs;
use std::process::Command;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub fn run_codex_sidecar(codex_args: Vec<String>) -> Result<i32, String> {
    if env::var_os("ZELLIJ").is_none() {
        return Err("Pixlair must be run inside an active zellij session".to_string());
    }

    let current_exe = env::current_exe().map_err(|error| error.to_string())?;
    let current_dir = env::current_dir().map_err(|error| error.to_string())?;
    let since_unix_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| error.to_string())?
        .as_millis();

    let runtime_dir = env::temp_dir().join(format!(
        "pixlair-zellij-{}-{}",
        std::process::id(),
        since_unix_ms
    ));
    fs::create_dir_all(&runtime_dir).map_err(|error| error.to_string())?;
    let shutdown_flag = runtime_dir.join("shutdown.flag");

    let probe = Command::new("zellij")
        .args(["action", "query-tab-names"])
        .output()
        .map_err(|error| error.to_string())?;
    if !probe.status.success() {
        return Err("Pixlair must be run from inside an attached zellij session".to_string());
    }

    let pane_status = Command::new("zellij")
        .args([
            "run",
            "--direction",
            "left",
            "--width",
            "18%",
            "--name",
            "pixlair",
            "--close-on-exit",
            "--cwd",
        ])
        .arg(&current_dir)
        .arg("--")
        .arg(&current_exe)
        .args([
            "--avatar-only",
            "--watch-codex",
            "--watch-codex-cwd",
        ])
        .arg(&current_dir)
        .args(["--since", &since_unix_ms.to_string(), "--shutdown-flag"])
        .arg(&shutdown_flag)
        .status()
        .map_err(|error| error.to_string())?;

    if !pane_status.success() {
        return Err("failed to create pixlair pane in zellij".to_string());
    }

    let status = Command::new("codex")
        .args(&codex_args)
        .status()
        .map_err(|error| error.to_string())?;

    let _ = fs::write(&shutdown_flag, b"done");
    thread::sleep(Duration::from_millis(250));
    let _ = fs::remove_file(&shutdown_flag);
    let _ = fs::remove_dir(&runtime_dir);

    Ok(status.code().unwrap_or(1))
}
