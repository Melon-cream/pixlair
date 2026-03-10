use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use crate::event::{parse_event_line, AgentState, StatusUpdate};

pub fn preview_state_for(value: &str) -> AgentState {
    AgentState::from_name(value).unwrap_or(AgentState::Thinking)
}

pub fn validate_wrapped_command(command: &[String]) -> Result<(), String> {
    if command.is_empty() {
        return Err("wrapped command requires at least one executable".to_string());
    }

    if is_codex_command(command) {
        if !is_codex_exec_json(command) {
            return Err(
                "wrapped codex is supported via `codex exec --json ...` for now".to_string(),
            );
        }

        if !has_codex_exec_payload(command) {
            return Err(
                "wrapped `codex exec --json` needs a prompt or subcommand, for example: \
pixlair --wrap codex exec --json \"fix failing tests\""
                    .to_string(),
            );
        }
    }

    Ok(())
}

pub fn initial_update_for_wrapped_command(command: &[String]) -> StatusUpdate {
    let label = summarize_command(command);
    let badge = command
        .first()
        .map(|entry| basename(entry))
        .unwrap_or_else(|| "run".to_string());

    StatusUpdate {
        state: Some(AgentState::Thinking),
        message: Some(format!("Watching {label}")),
        badge: Some(badge),
        tool: None,
    }
}

pub fn summarize_command(command: &[String]) -> String {
    let mut parts = Vec::new();
    for part in command.iter().take(4) {
        parts.push(part.as_str());
    }

    let mut label = parts.join(" ");
    if command.len() > 4 {
        label.push_str(" ...");
    }
    label
}

pub fn parse_wrapped_output(line: &str) -> Option<StatusUpdate> {
    parse_event_line(line).or_else(|| parse_codex_json(line))
}

pub fn find_codex_session(cwd: &Path, since_unix_ms: u128) -> Option<PathBuf> {
    let mut candidates = Vec::new();
    collect_session_files(&codex_sessions_root()?, &mut candidates);
    candidates.sort_by(|left, right| right.0.cmp(&left.0));

    for (modified_ms, path) in candidates {
        if modified_ms < since_unix_ms {
            continue;
        }

        if session_matches_cwd(&path, cwd) {
            return Some(path);
        }
    }

    None
}

pub fn parse_codex_session_line(line: &str) -> Option<StatusUpdate> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return None;
    }

    if trimmed.contains(r#""type":"session_meta""#) {
        return Some(StatusUpdate {
            state: Some(AgentState::AwaitingInput),
            message: Some("Codex session attached".to_string()),
            badge: Some("codex".to_string()),
            tool: None,
        });
    }

    if trimmed.contains(r#""type":"event_msg""#) {
        if trimmed.contains(r#""type":"task_started""#) {
            return Some(StatusUpdate {
                state: Some(AgentState::Thinking),
                message: Some("Codex is thinking".to_string()),
                badge: Some("thinking".to_string()),
                tool: None,
            });
        }

        if trimmed.contains(r#""type":"agent_message""#) {
            return Some(StatusUpdate {
                state: Some(AgentState::Working),
                message: Some("Codex is replying".to_string()),
                badge: Some("reply".to_string()),
                tool: None,
            });
        }

        if trimmed.contains(r#""type":"task_complete""#) {
            return Some(StatusUpdate {
                state: Some(AgentState::Success),
                message: Some("Turn completed".to_string()),
                badge: Some("done".to_string()),
                tool: None,
            });
        }

        return None;
    }

    if trimmed.contains(r#""type":"response_item""#) {
        if trimmed.contains(r#""type":"reasoning""#) {
            return Some(StatusUpdate {
                state: Some(AgentState::Thinking),
                message: Some("Codex is reasoning".to_string()),
                badge: Some("thinking".to_string()),
                tool: None,
            });
        }

        if trimmed.contains(r#""type":"function_call""#) {
            let tool = extract_json_string(trimmed, "name")
                .unwrap_or_else(|| "tool".to_string());
            return Some(StatusUpdate {
                state: Some(AgentState::Tool),
                message: Some(format!("{tool} in progress")),
                badge: Some("tool".to_string()),
                tool: Some(tool),
            });
        }

        if trimmed.contains(r#""type":"function_call_output""#) {
            return Some(StatusUpdate {
                state: Some(AgentState::Thinking),
                message: Some("Codex is processing tool output".to_string()),
                badge: Some("thinking".to_string()),
                tool: None,
            });
        }

        if trimmed.contains(r#""role":"assistant""#) && trimmed.contains(r#""type":"message""#) {
            return Some(StatusUpdate {
                state: Some(AgentState::Working),
                message: Some("Codex is writing a response".to_string()),
                badge: Some("reply".to_string()),
                tool: None,
            });
        }

        if trimmed.contains(r#""role":"user""#) && trimmed.contains(r#""type":"message""#) {
            return Some(StatusUpdate {
                state: Some(AgentState::AwaitingInput),
                message: Some("Prompt submitted".to_string()),
                badge: Some("prompt".to_string()),
                tool: None,
            });
        }
    }

    None
}

fn parse_codex_json(line: &str) -> Option<StatusUpdate> {
    let trimmed = line.trim();
    if !trimmed.starts_with('{') || !trimmed.contains(r#""type":"#) {
        return None;
    }

    if trimmed.contains(r#""type":"thread.started""#) {
        return Some(StatusUpdate {
            state: Some(AgentState::Thinking),
            message: Some("Codex session started".to_string()),
            badge: Some("run".to_string()),
            tool: None,
        });
    }

    if trimmed.contains(r#""type":"turn.started""#) {
        return Some(StatusUpdate {
            state: Some(AgentState::Thinking),
            message: Some("Codex is thinking".to_string()),
            badge: Some("thinking".to_string()),
            tool: None,
        });
    }

    if trimmed.contains(r#""type":"turn.completed""#) {
        return Some(StatusUpdate {
            state: Some(AgentState::Success),
            message: Some("Codex finished the turn".to_string()),
            badge: Some("done".to_string()),
            tool: None,
        });
    }

    if trimmed.contains(r#""type":"error""#) {
        return Some(StatusUpdate {
            state: Some(AgentState::Error),
            message: Some(
                extract_json_string(trimmed, "message")
                    .unwrap_or_else(|| "Codex reported an error".to_string()),
            ),
            badge: Some("error".to_string()),
            tool: None,
        });
    }

    if trimmed.contains(r#""type":"item.completed""#) || trimmed.contains(r#""type":"item.started""#) {
        if contains_any(
            trimmed,
            &[
                "\"exec_command\"",
                "\"apply_patch\"",
                "\"web_search\"",
                "\"mcp_tool_call\"",
                "\"tool_call\"",
                "\"function_call\"",
            ],
        ) {
            let tool = infer_tool_name(trimmed);
            return Some(StatusUpdate {
                state: Some(AgentState::Tool),
                message: Some(format!("{tool} in progress")),
                badge: Some("tool".to_string()),
                tool: Some(tool),
            });
        }

        if contains_any(trimmed, &["\"reasoning\"", "\"analysis\""]) {
            return Some(StatusUpdate {
                state: Some(AgentState::Thinking),
                message: Some("Codex is reasoning".to_string()),
                badge: Some("thinking".to_string()),
                tool: None,
            });
        }

        if contains_any(trimmed, &["\"message\"", "\"assistant_message\""]) {
            return Some(StatusUpdate {
                state: Some(AgentState::Working),
                message: Some("Codex is writing a response".to_string()),
                badge: Some("reply".to_string()),
                tool: None,
            });
        }
    }

    None
}

fn infer_tool_name(line: &str) -> String {
    for key in ["tool_name", "name", "call_name"] {
        if let Some(value) = extract_json_string(line, key) {
            return value;
        }
    }

    if line.contains("\"apply_patch\"") {
        "apply_patch".to_string()
    } else if line.contains("\"exec_command\"") {
        "exec_command".to_string()
    } else if line.contains("\"web_search\"") {
        "web_search".to_string()
    } else if line.contains("\"mcp_tool_call\"") {
        "mcp_tool_call".to_string()
    } else {
        "tool".to_string()
    }
}

fn extract_json_string(line: &str, key: &str) -> Option<String> {
    let marker = format!("\"{key}\":\"");
    let start = line.find(&marker)? + marker.len();
    let mut escaped = false;
    let mut output = String::new();

    for ch in line[start..].chars() {
        if escaped {
            output.push(match ch {
                'n' => '\n',
                'r' => '\r',
                't' => '\t',
                '"' => '"',
                '\\' => '\\',
                other => other,
            });
            escaped = false;
            continue;
        }

        match ch {
            '\\' => escaped = true,
            '"' => return Some(output),
            other => output.push(other),
        }
    }

    None
}

fn contains_any(line: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| line.contains(needle))
}

fn is_codex_command(command: &[String]) -> bool {
    command
        .first()
        .map(|entry| basename(entry) == "codex")
        .unwrap_or(false)
}

fn is_codex_exec_json(command: &[String]) -> bool {
    let has_exec = command.iter().any(|entry| entry == "exec");
    let has_json = command.iter().any(|entry| entry == "--json");
    has_exec && has_json
}

fn has_codex_exec_payload(command: &[String]) -> bool {
    let Some(exec_index) = command.iter().position(|entry| entry == "exec") else {
        return false;
    };

    let mut index = exec_index + 1;
    while index < command.len() {
        let arg = &command[index];

        if arg == "--" {
            return index + 1 < command.len();
        }

        if takes_value(arg) {
            index += 2;
            continue;
        }

        if is_flag(arg) {
            index += 1;
            continue;
        }

        return true;
    }

    false
}

fn takes_value(arg: &str) -> bool {
    matches!(
        arg,
        "-c"
            | "--config"
            | "--enable"
            | "--disable"
            | "-i"
            | "--image"
            | "-m"
            | "--model"
            | "--local-provider"
            | "-s"
            | "--sandbox"
            | "-p"
            | "--profile"
            | "-C"
            | "--cd"
            | "--add-dir"
            | "--output-schema"
            | "-o"
            | "--output-last-message"
            | "--color"
    )
}

fn is_flag(arg: &str) -> bool {
    arg.starts_with('-')
}

fn basename(input: &str) -> String {
    Path::new(input)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(input)
        .to_string()
}

fn codex_sessions_root() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join(".codex").join("sessions"))
}

fn collect_session_files(root: &Path, output: &mut Vec<(u128, PathBuf)>) {
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_session_files(&path, output);
            continue;
        }

        if path.extension().and_then(|ext| ext.to_str()) != Some("jsonl") {
            continue;
        }

        let modified_ms = entry
            .metadata()
            .ok()
            .and_then(|meta| meta.modified().ok())
            .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
            .map(|duration| duration.as_millis())
            .unwrap_or(0);

        output.push((modified_ms, path));
    }
}

fn session_matches_cwd(path: &Path, cwd: &Path) -> bool {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(_) => return false,
    };
    let mut reader = BufReader::new(file);
    let mut first_line = String::new();
    if reader.read_line(&mut first_line).is_err() {
        return false;
    }

    let escaped_cwd = cwd.to_string_lossy().replace('\\', "\\\\").replace('"', "\\\"");
    first_line.contains(&format!("\"cwd\":\"{escaped_cwd}\""))
}
