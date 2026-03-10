#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AgentState {
    Idle,
    AwaitingInput,
    Thinking,
    Working,
    Success,
    Error,
    Sleeping,
    Tool,
}

impl AgentState {
    pub fn from_name(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "idle" | "standby" => Some(Self::Idle),
            "input" | "awaiting_input" | "awaiting-input" => Some(Self::AwaitingInput),
            "thinking" | "think" => Some(Self::Thinking),
            "working" | "work" | "running" => Some(Self::Working),
            "success" | "done" => Some(Self::Success),
            "error" | "failed" | "failure" => Some(Self::Error),
            "sleeping" | "sleep" => Some(Self::Sleeping),
            "tool" | "tools" => Some(Self::Tool),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Idle => "Idle",
            Self::AwaitingInput => "Input",
            Self::Thinking => "Thinking",
            Self::Working => "Working",
            Self::Success => "Success",
            Self::Error => "Error",
            Self::Sleeping => "Sleeping",
            Self::Tool => "Tool",
        }
    }

    pub fn mood(self) -> &'static str {
        match self {
            Self::Idle => "steady",
            Self::AwaitingInput => "alert",
            Self::Thinking => "focused",
            Self::Working => "busy",
            Self::Success => "bright",
            Self::Error => "guarded",
            Self::Sleeping => "drowsy",
            Self::Tool => "hands-on",
        }
    }

    pub fn banner(self) -> &'static str {
        match self {
            Self::Idle => "Breathing quietly",
            Self::AwaitingInput => "Watching for your next move",
            Self::Thinking => "Working through the problem",
            Self::Working => "Executing work in the background",
            Self::Success => "Task completed cleanly",
            Self::Error => "Something needs attention",
            Self::Sleeping => "Long task posture engaged",
            Self::Tool => "Tool usage in progress",
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct StatusUpdate {
    pub state: Option<AgentState>,
    pub message: Option<String>,
    pub tool: Option<String>,
    pub badge: Option<String>,
}

pub fn parse_event_line(line: &str) -> Option<StatusUpdate> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return None;
    }

    let pairs = if trimmed.starts_with('{') && trimmed.ends_with('}') {
        parse_json_like(trimmed)
    } else {
        parse_kv_like(trimmed)
    };

    if pairs.is_empty() {
        return None;
    }

    let mut update = StatusUpdate::default();
    for (key, value) in pairs {
        match key.as_str() {
            "state" => update.state = AgentState::from_name(&value),
            "message" => update.message = Some(value),
            "tool" => update.tool = Some(value),
            "badge" => update.badge = Some(value),
            _ => {}
        }
    }

    if update.state.is_none()
        && update.message.is_none()
        && update.tool.is_none()
        && update.badge.is_none()
    {
        None
    } else {
        Some(update)
    }
}

fn parse_json_like(input: &str) -> Vec<(String, String)> {
    let body = input.trim().trim_start_matches('{').trim_end_matches('}');
    split_quoted(body, ',')
        .into_iter()
        .filter_map(|entry| {
            let mut parts = split_quoted(&entry, ':').into_iter();
            let key = parts.next()?;
            let value = parts.next()?;
            Some((normalize_key(&key), clean_value(&value)))
        })
        .collect()
}

fn parse_kv_like(input: &str) -> Vec<(String, String)> {
    let mut result = Vec::new();
    let mut chars = input.chars().peekable();

    while chars.peek().is_some() {
        consume_whitespace(&mut chars);
        if chars.peek().is_none() {
            break;
        }

        let mut key = String::new();
        while let Some(ch) = chars.peek().copied() {
            if ch == '=' || ch.is_whitespace() {
                break;
            }
            key.push(ch);
            chars.next();
        }

        consume_whitespace(&mut chars);
        if chars.peek() != Some(&'=') {
            break;
        }
        chars.next();
        consume_whitespace(&mut chars);

        let value = if chars.peek() == Some(&'"') {
            read_quoted(&mut chars)
        } else {
            read_bare(&mut chars)
        };

        if !key.is_empty() && !value.is_empty() {
            result.push((normalize_key(&key), clean_value(&value)));
        }
    }

    result
}

fn split_quoted(input: &str, delimiter: char) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    let mut escaped = false;

    for ch in input.chars() {
        if escaped {
            current.push(ch);
            escaped = false;
            continue;
        }

        match ch {
            '\\' if quoted => {
                escaped = true;
                current.push(ch);
            }
            '"' => {
                quoted = !quoted;
                current.push(ch);
            }
            _ if ch == delimiter && !quoted => {
                let entry = current.trim();
                if !entry.is_empty() {
                    parts.push(entry.to_string());
                }
                current.clear();
            }
            _ => current.push(ch),
        }
    }

    let tail = current.trim();
    if !tail.is_empty() {
        parts.push(tail.to_string());
    }

    parts
}

fn consume_whitespace<I>(chars: &mut std::iter::Peekable<I>)
where
    I: Iterator<Item = char>,
{
    while matches!(chars.peek(), Some(ch) if ch.is_whitespace()) {
        chars.next();
    }
}

fn read_quoted<I>(chars: &mut std::iter::Peekable<I>) -> String
where
    I: Iterator<Item = char>,
{
    let mut value = String::new();
    if chars.peek() == Some(&'"') {
        chars.next();
    }

    let mut escaped = false;
    while let Some(ch) = chars.next() {
        if escaped {
            value.push(ch);
            escaped = false;
            continue;
        }
        match ch {
            '\\' => escaped = true,
            '"' => break,
            _ => value.push(ch),
        }
    }
    value
}

fn read_bare<I>(chars: &mut std::iter::Peekable<I>) -> String
where
    I: Iterator<Item = char>,
{
    let mut value = String::new();
    while let Some(ch) = chars.peek().copied() {
        if ch.is_whitespace() {
            break;
        }
        value.push(ch);
        chars.next();
    }
    value
}

fn normalize_key(input: &str) -> String {
    input
        .trim()
        .trim_matches('"')
        .trim_matches('\'')
        .to_ascii_lowercase()
}

pub fn clean_value(input: &str) -> String {
    input.trim().trim_matches('"').trim_matches('\'').to_string()
}
