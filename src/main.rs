mod app;
mod avatar;
mod bridge;
mod event;
mod terminal;
mod zellij;

use std::env;
use std::path::PathBuf;

use avatar::frame_for;
use bridge::{
    initial_update_for_wrapped_command, preview_state_for, validate_wrapped_command,
};
use event::AgentState;
use zellij::run_codex_sidecar;

use app::RunOptions;

fn print_help() {
    println!(
        "\
Pixlair

Usage:
  pixlair --codex [args...]
  pixlair --demo
  pixlair --help

Options:
  --codex            Launch `codex` inside the current zellij pane
  --demo             Start in automatic state-cycle mode
  --help             Show this help text

Notes:
  Run Pixlair from inside an attached zellij session.
  Example: pixlair --codex resume --last
"
    );
}

enum MainOutcome {
    Exit(i32),
    Success,
}

fn main() {
    match real_main() {
        Ok(MainOutcome::Success) => {}
        Ok(MainOutcome::Exit(code)) => std::process::exit(code),
        Err(error) => {
            eprintln!("pixlair: {error}");
            std::process::exit(1);
        }
    }
}

fn real_main() -> Result<MainOutcome, String> {
    let mut args = env::args().skip(1);
    let mut events_path: Option<PathBuf> = None;
    let mut demo_mode = false;
    let mut wrapped_command: Option<Vec<String>> = None;
    let mut codex_command: Option<Vec<String>> = None;
    let mut preview_state: Option<AgentState> = None;
    let mut avatar_only = false;
    let mut watch_codex = false;
    let mut watch_codex_cwd: Option<PathBuf> = None;
    let mut since_unix_ms: Option<u128> = None;
    let mut shutdown_flag: Option<PathBuf> = None;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--help" | "-h" => {
                print_help();
                return Ok(MainOutcome::Success);
            }
            "--demo" => demo_mode = true,
            "--avatar-only" => avatar_only = true,
            "--watch-codex" => watch_codex = true,
            "--watch-codex-cwd" => {
                let value = args
                    .next()
                    .ok_or_else(|| "--watch-codex-cwd requires a path".to_string())?;
                watch_codex_cwd = Some(PathBuf::from(value));
            }
            "--since" => {
                let value = args
                    .next()
                    .ok_or_else(|| "--since requires a unix timestamp in milliseconds".to_string())?;
                since_unix_ms = Some(
                    value
                        .parse::<u128>()
                        .map_err(|_| format!("invalid unix timestamp: {value}"))?,
                );
            }
            "--shutdown-flag" => {
                let value = args
                    .next()
                    .ok_or_else(|| "--shutdown-flag requires a path".to_string())?;
                shutdown_flag = Some(PathBuf::from(value));
            }
            "--events" => {
                let value = args
                    .next()
                    .ok_or_else(|| "--events requires a path".to_string())?;
                events_path = Some(PathBuf::from(value));
            }
            "--codex" => {
                if codex_command.is_some() {
                    return Err("`--codex` was specified more than once".to_string());
                }
                let command: Vec<String> = args.by_ref().collect();
                codex_command = Some(command);
                break;
            }
            "--preview" => {
                let value = args
                    .next()
                    .ok_or_else(|| "--preview requires a state or agent".to_string())?;
                preview_state = Some(preview_state_for(&value));
            }
            "--wrap" | "--" => {
                if wrapped_command.is_some() {
                    return Err("wrapped command was specified more than once".to_string());
                }

                let command: Vec<String> = args.by_ref().collect();
                if command.is_empty() {
                    return Err("wrapped command requires at least one executable".to_string());
                }
                wrapped_command = Some(command);
                break;
            }
            other if other.starts_with('-') => return Err(format!("unknown argument: {other}")),
            other => return Err(format!("unexpected positional argument: {other}")),
        }
    }

    if let Some(state) = preview_state {
        for line in frame_for(state, 0) {
            println!("{line}");
        }
        return Ok(MainOutcome::Success);
    }

    if let Some(command) = codex_command {
        return Ok(MainOutcome::Exit(run_codex_sidecar(command)?));
    }

    let initial_update = if let Some(command) = wrapped_command.as_ref() {
        validate_wrapped_command(command)?;
        Some(initial_update_for_wrapped_command(command))
    } else {
        None
    };

    let watch_codex_cwd = if watch_codex {
        Some(
            watch_codex_cwd
                .unwrap_or(std::env::current_dir().map_err(|error| error.to_string())?),
        )
    } else {
        None
    };

    if !demo_mode
        && wrapped_command.is_none()
        && !avatar_only
        && watch_codex_cwd.is_none()
        && events_path.is_none()
    {
        print_help();
        return Ok(MainOutcome::Success);
    }

    app::run(RunOptions {
        events_path,
        demo_mode,
        initial_update,
        wrapped_command,
        avatar_only,
        watch_codex_cwd,
        watch_since_unix_ms: since_unix_ms,
        shutdown_flag,
    })
    .map_err(|error| error.to_string())?;

    Ok(MainOutcome::Success)
}
