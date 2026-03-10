use std::collections::VecDeque;
use std::fs::File;
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};

use crate::bridge::{parse_wrapped_output, summarize_command};
use crate::avatar::frame_for;
use crate::event::{clean_value, parse_event_line, AgentState, StatusUpdate};
use crate::terminal::{terminal_size, TerminalMode, TerminalSession};

const TICK_RATE: Duration = Duration::from_millis(120);
const ANIMATION_RATE: Duration = Duration::from_millis(360);
const MOTION_RATE: Duration = Duration::from_millis(120);
const FEED_POLL_RATE: Duration = Duration::from_millis(220);
const LOG_CAPACITY: usize = 8;

pub struct RunOptions {
    pub events_path: Option<PathBuf>,
    pub demo_mode: bool,
    pub initial_update: Option<StatusUpdate>,
    pub wrapped_command: Option<Vec<String>>,
    pub avatar_only: bool,
    pub watch_codex_cwd: Option<PathBuf>,
    pub watch_since_unix_ms: Option<u128>,
    pub shutdown_flag: Option<PathBuf>,
}

pub fn run(options: RunOptions) -> io::Result<()> {
    let (tx, rx) = mpsc::channel();
    if !options.avatar_only {
        spawn_input_listener(tx.clone());
    }
    if let Some(path) = options.events_path.clone() {
        spawn_feed_listener(path, tx.clone());
    }
    if let Some(cwd) = options.watch_codex_cwd.clone() {
        spawn_codex_session_listener(
            cwd,
            options.watch_since_unix_ms.unwrap_or(0),
            options.shutdown_flag.clone(),
            tx.clone(),
        );
    }
    let wrapped_label = options
        .wrapped_command
        .as_ref()
        .map(|command| summarize_command(command));
    if let Some(command) = options.wrapped_command {
        spawn_wrapped_process(command, tx)?;
    }

    let terminal_mode = if options.avatar_only {
        TerminalMode::Sidecar
    } else {
        TerminalMode::Fullscreen
    };
    let mut terminal = TerminalSession::enter(terminal_mode)?;
    let mut app = App::new(
        options.events_path,
        options.demo_mode,
        options.initial_update,
        wrapped_label,
        options.avatar_only,
    );

    loop {
        let size = terminal_size().unwrap_or((100, 32));
        drain_events(&rx, &mut app);
        app.tick(size.0, size.1);
        let frame = render(&app, size.0, size.1);
        terminal.render(&frame)?;

        if app.should_quit {
            break;
        }

        thread::sleep(TICK_RATE);
    }

    Ok(())
}

#[derive(Debug)]
enum RuntimeEvent {
    Key(u8),
    External(StatusUpdate),
    Wrapped(StatusUpdate),
    WrappedLog(String),
    WrappedExit(Option<i32>),
    Shutdown,
}

struct App {
    state: AgentState,
    message: String,
    tool: Option<String>,
    badge: Option<String>,
    log: VecDeque<String>,
    start_time: Instant,
    last_state_change: Instant,
    last_animation_step: Instant,
    last_motion_step: Instant,
    last_demo_switch: Instant,
    frame_tick: usize,
    demo_cursor: usize,
    demo_mode: bool,
    show_help: bool,
    should_quit: bool,
    exit_deadline: Option<Instant>,
    idle_deadline: Option<Instant>,
    events_path: Option<PathBuf>,
    wrapped_command: Option<String>,
    avatar_only: bool,
    avatar_motion: AvatarMotion,
}

#[derive(Clone, Copy)]
struct AvatarMotion {
    x: isize,
    y: isize,
    vx: isize,
    vy: isize,
}

impl App {
    fn new(
        events_path: Option<PathBuf>,
        demo_mode: bool,
        initial_update: Option<StatusUpdate>,
        wrapped_command: Option<String>,
        avatar_only: bool,
    ) -> Self {
        let now = Instant::now();
        let mut app = Self {
            state: AgentState::Idle,
            message: "Companion online".to_string(),
            tool: None,
            badge: Some("ready".to_string()),
            log: VecDeque::new(),
            start_time: now,
            last_state_change: now,
            last_animation_step: now,
            last_motion_step: now,
            last_demo_switch: now,
            frame_tick: 0,
            demo_cursor: 0,
            demo_mode,
            show_help: false,
            should_quit: false,
            exit_deadline: None,
            idle_deadline: None,
            events_path,
            wrapped_command,
            avatar_only,
            avatar_motion: AvatarMotion {
                x: 0,
                y: 0,
                vx: 1,
                vy: 1,
            },
        };
        app.push_log("Pixlair booted");
        if app.demo_mode {
            app.push_log("Demo cycle enabled");
        }
        if let Some(update) = initial_update {
            app.apply_initial_update(update);
        }
        app
    }

    fn tick(&mut self, width: usize, height: usize) {
        let now = Instant::now();

        if now.duration_since(self.last_animation_step) >= ANIMATION_RATE {
            self.last_animation_step = now;
            self.frame_tick = self.frame_tick.wrapping_add(1);
        }

        if self.avatar_only && now.duration_since(self.last_motion_step) >= MOTION_RATE {
            self.last_motion_step = now;
            self.step_avatar_motion(width, height);
        }

        if self.demo_mode && now.duration_since(self.last_demo_switch) >= demo_step_duration(self.demo_cursor) {
            self.last_demo_switch = now;
            self.demo_cursor = (self.demo_cursor + 1) % DEMO_STATES.len();
            self.set_state(DEMO_STATES[self.demo_cursor], "Demo cycle update");
            if self.state == AgentState::Tool && self.tool.is_none() {
                self.tool = Some("hammer".to_string());
            }
        }

        if let Some(deadline) = self.exit_deadline {
            if now >= deadline {
                self.should_quit = true;
            }
        }

        if let Some(deadline) = self.idle_deadline {
            if now >= deadline {
                self.idle_deadline = None;
                self.state = AgentState::Idle;
                self.frame_tick = 0;
                self.last_state_change = now;
                self.message = "Waiting for next prompt".to_string();
                self.badge = Some("ready".to_string());
                self.tool = None;
                self.push_log("Auto state -> Idle");
            }
        }
    }

    fn step_avatar_motion(&mut self, width: usize, height: usize) {
        let art = frame_for(self.state, self.frame_tick);
        let art_width = art
            .iter()
            .map(|line| display_width(line))
            .max()
            .unwrap_or(0) as isize;
        let art_height = art.len() as isize;

        let max_x = width.saturating_sub(art_width.max(0) as usize + 1) as isize;
        let max_y = height.saturating_sub(art_height.max(0) as usize + 1) as isize;
        let (speed_x, speed_y) = avatar_speed_for(self.state);
        let dir_x = if self.avatar_motion.vx < 0 { -1 } else { 1 };
        let dir_y = if self.avatar_motion.vy < 0 { -1 } else { 1 };

        self.avatar_motion.vx = dir_x * speed_x;
        self.avatar_motion.vy = dir_y * speed_y;

        if max_x <= 0 {
            self.avatar_motion.x = 0;
            self.avatar_motion.vx = speed_x;
        } else {
            self.avatar_motion.x += self.avatar_motion.vx;
            if self.avatar_motion.x <= 0 {
                self.avatar_motion.x = 0;
                self.avatar_motion.vx = speed_x;
            } else if self.avatar_motion.x >= max_x {
                self.avatar_motion.x = max_x;
                self.avatar_motion.vx = -speed_x;
            }
        }

        if max_y <= 0 {
            self.avatar_motion.y = 0;
            self.avatar_motion.vy = speed_y;
        } else {
            self.avatar_motion.y += self.avatar_motion.vy;
            if self.avatar_motion.y <= 0 {
                self.avatar_motion.y = 0;
                self.avatar_motion.vy = speed_y;
            } else if self.avatar_motion.y >= max_y {
                self.avatar_motion.y = max_y;
                self.avatar_motion.vy = -speed_y;
            }
        }
    }

    fn handle_key(&mut self, key: u8) {
        match key as char {
            'q' => self.should_quit = true,
            'h' | '?' => self.show_help = !self.show_help,
            'd' => {
                self.demo_mode = !self.demo_mode;
                self.last_demo_switch = Instant::now();
                self.push_log(if self.demo_mode {
                    "Demo cycle enabled"
                } else {
                    "Demo cycle disabled"
                });
            }
            '1' => self.manual_state(AgentState::Idle),
            '2' => self.manual_state(AgentState::AwaitingInput),
            '3' => self.manual_state(AgentState::Thinking),
            '4' => self.manual_state(AgentState::Working),
            '5' => self.manual_state(AgentState::Success),
            '6' => self.manual_state(AgentState::Error),
            '7' => self.manual_state(AgentState::Sleeping),
            '8' => self.manual_state(AgentState::Tool),
            _ => {}
        }
    }

    fn apply_update(&mut self, update: StatusUpdate) {
        if let Some(state) = update.state {
            self.idle_deadline = None;
            self.state = state;
            self.last_state_change = Instant::now();
            self.demo_mode = false;
            self.frame_tick = 0;
            self.push_log(&format!("External state -> {}", state.label()));
        }

        if let Some(message) = update.message {
            self.message = clean_value(&message);
        }

        if let Some(tool) = update.tool {
            self.tool = Some(clean_value(&tool));
        }

        if let Some(badge) = update.badge {
            self.badge = Some(clean_value(&badge));
        }
    }

    fn apply_wrapped_update(&mut self, update: StatusUpdate) {
        if let Some(state) = update.state {
            self.idle_deadline = if state == AgentState::Success {
                Some(Instant::now() + Duration::from_secs(2))
            } else {
                None
            };
            self.state = state;
            self.last_state_change = Instant::now();
            self.demo_mode = false;
            self.frame_tick = 0;
            self.push_log(&format!("Wrapped state -> {}", state.label()));
        }

        if let Some(message) = update.message {
            self.message = clean_value(&message);
        }

        if let Some(tool) = update.tool {
            self.tool = Some(clean_value(&tool));
        }

        if let Some(badge) = update.badge {
            self.badge = Some(clean_value(&badge));
        }
    }

    fn apply_initial_update(&mut self, update: StatusUpdate) {
        let initial_state = update.state;
        let initial_message = update.message.clone();
        let initial_badge = update.badge.clone();

        if let Some(state) = update.state {
            self.state = state;
            self.last_state_change = Instant::now();
            self.frame_tick = 0;
            self.idle_deadline = None;
        }

        if let Some(message) = update.message {
            self.message = clean_value(&message);
        }

        if let Some(tool) = update.tool {
            self.tool = Some(clean_value(&tool));
        }

        if let Some(badge) = update.badge {
            self.badge = Some(clean_value(&badge));
        }

        if let Some(state) = initial_state {
            self.push_log(&format!("Startup state -> {}", state.label()));
        } else if initial_message.is_some() || initial_badge.is_some() {
            self.push_log("Startup profile applied");
        }
    }

    fn handle_wrapped_log(&mut self, entry: String) {
        self.push_log(&entry);
    }

    fn handle_wrapped_exit(&mut self, code: Option<i32>) {
        let now = Instant::now();
        self.last_state_change = now;
        self.frame_tick = 0;
        self.demo_mode = false;
        self.idle_deadline = None;

        match code {
            Some(0) => {
                self.state = AgentState::Success;
                self.badge = Some("done".to_string());
                self.message = "Wrapped command finished cleanly".to_string();
                self.push_log("Wrapped command exited with status 0");
                self.exit_deadline = Some(now + Duration::from_secs(2));
            }
            Some(value) => {
                self.state = AgentState::Error;
                self.badge = Some(format!("exit:{value}"));
                self.message = format!("Wrapped command exited with status {value}");
                self.push_log(&format!("Wrapped command exited with status {value}"));
                self.exit_deadline = Some(now + Duration::from_secs(4));
            }
            None => {
                self.state = AgentState::Error;
                self.badge = Some("exit:signal".to_string());
                self.message = "Wrapped command exited without a status code".to_string();
                self.push_log("Wrapped command exited without a status code");
                self.exit_deadline = Some(now + Duration::from_secs(4));
            }
        }
    }

    fn manual_state(&mut self, state: AgentState) {
        self.demo_mode = false;
        self.set_state(state, "Manual state change");
        if state == AgentState::Tool && self.tool.is_none() {
            self.tool = Some("wrench".to_string());
        }
    }

    fn set_state(&mut self, state: AgentState, note: &str) {
        self.state = state;
        self.frame_tick = 0;
        self.last_state_change = Instant::now();
        self.idle_deadline = None;
        self.push_log(&format!("{note}: {}", state.label()));
    }

    fn push_log(&mut self, entry: &str) {
        let stamp = format_elapsed(self.start_time.elapsed());
        self.log.push_back(format!("[{stamp}] {entry}"));
        while self.log.len() > LOG_CAPACITY {
            self.log.pop_front();
        }
    }

    fn mode_label(&self) -> &'static str {
        if self.demo_mode {
            "demo"
        } else if self.wrapped_command.is_some() {
            "wrapped"
        } else {
            "manual"
        }
    }

    fn signal_label(&self) -> &'static str {
        match self.state {
            AgentState::Idle => "green / calm",
            AgentState::AwaitingInput => "amber / watch",
            AgentState::Thinking => "amber / thinking",
            AgentState::Working => "blue / busy",
            AgentState::Success => "green / success",
            AgentState::Error => "red / alert",
            AgentState::Sleeping => "dim / long-run",
            AgentState::Tool => "blue / tools",
        }
    }
}

const DEMO_STATES: [AgentState; 8] = [
    AgentState::Idle,
    AgentState::AwaitingInput,
    AgentState::Thinking,
    AgentState::Working,
    AgentState::Tool,
    AgentState::Success,
    AgentState::Error,
    AgentState::Sleeping,
];

fn demo_step_duration(index: usize) -> Duration {
    match DEMO_STATES[index % DEMO_STATES.len()] {
        AgentState::Sleeping => Duration::from_secs(6),
        AgentState::Thinking | AgentState::Working | AgentState::Tool => Duration::from_secs(5),
        _ => Duration::from_secs(4),
    }
}

fn drain_events(rx: &Receiver<RuntimeEvent>, app: &mut App) {
    while let Ok(event) = rx.try_recv() {
        match event {
            RuntimeEvent::Key(key) => app.handle_key(key),
            RuntimeEvent::External(update) => app.apply_update(update),
            RuntimeEvent::Wrapped(update) => app.apply_wrapped_update(update),
            RuntimeEvent::WrappedLog(entry) => app.handle_wrapped_log(entry),
            RuntimeEvent::WrappedExit(code) => app.handle_wrapped_exit(code),
            RuntimeEvent::Shutdown => app.should_quit = true,
        }
    }
}

fn spawn_input_listener(tx: Sender<RuntimeEvent>) {
    thread::spawn(move || {
        let mut stdin = io::stdin();
        let mut buffer = [0_u8; 1];

        loop {
            match stdin.read(&mut buffer) {
                Ok(0) => thread::sleep(Duration::from_millis(15)),
                Ok(_) => {
                    if tx.send(RuntimeEvent::Key(buffer[0])).is_err() {
                        break;
                    }
                }
                Err(_) => thread::sleep(Duration::from_millis(30)),
            }
        }
    });
}

fn spawn_feed_listener(path: PathBuf, tx: Sender<RuntimeEvent>) {
    thread::spawn(move || {
        let mut offset = 0_u64;

        loop {
            if let Ok(mut file) = File::open(&path) {
                if let Ok(meta) = file.metadata() {
                    if meta.len() < offset {
                        offset = 0;
                    }
                }

                if file.seek(SeekFrom::Start(offset)).is_ok() {
                    let mut chunk = String::new();
                    if file.read_to_string(&mut chunk).is_ok() {
                        if let Ok(position) = file.stream_position() {
                            offset = position;
                        }

                        for line in chunk.lines() {
                            if let Some(update) = parse_event_line(line) {
                                if tx.send(RuntimeEvent::External(update)).is_err() {
                                    return;
                                }
                            }
                        }
                    }
                }
            }

            thread::sleep(FEED_POLL_RATE);
        }
    });
}

fn spawn_codex_session_listener(
    cwd: PathBuf,
    since_unix_ms: u128,
    shutdown_flag: Option<PathBuf>,
    tx: Sender<RuntimeEvent>,
) {
    thread::spawn(move || {
        let mut offset = 0_u64;
        let mut session_path: Option<PathBuf> = None;

        loop {
            if let Some(flag) = shutdown_flag.as_ref() {
                if flag.exists() {
                    let _ = tx.send(RuntimeEvent::Shutdown);
                    break;
                }
            }

            if session_path.is_none() {
                session_path = crate::bridge::find_codex_session(&cwd, since_unix_ms);
                if session_path.is_some() {
                    offset = 0;
                }
            }

            if let Some(path) = session_path.as_ref() {
                if let Ok(mut file) = File::open(path) {
                    if let Ok(meta) = file.metadata() {
                        if meta.len() < offset {
                            offset = 0;
                        }
                    }

                    if file.seek(SeekFrom::Start(offset)).is_ok() {
                        let mut chunk = String::new();
                        if file.read_to_string(&mut chunk).is_ok() {
                            if let Ok(position) = file.stream_position() {
                                offset = position;
                            }

                            for line in chunk.lines() {
                                if let Some(update) = crate::bridge::parse_codex_session_line(line) {
                                    if tx.send(RuntimeEvent::Wrapped(update)).is_err() {
                                        return;
                                    }
                                }
                            }
                        }
                    }
                } else {
                    session_path = None;
                    offset = 0;
                }
            }

            thread::sleep(FEED_POLL_RATE);
        }
    });
}

fn spawn_wrapped_process(command: Vec<String>, tx: Sender<RuntimeEvent>) -> io::Result<()> {
    let label = summarize_command(&command);
    let mut child = Command::new(&command[0])
        .args(&command[1..])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    let _ = tx.send(RuntimeEvent::WrappedLog(format!("Wrapped command started: {label}")));

    if let Some(stdout) = child.stdout.take() {
        let tx_stdout = tx.clone();
        thread::spawn(move || {
            read_wrapped_stream(stdout, true, tx_stdout);
        });
    }

    if let Some(stderr) = child.stderr.take() {
        let tx_stderr = tx.clone();
        thread::spawn(move || {
            read_wrapped_stream(stderr, false, tx_stderr);
        });
    }

    thread::spawn(move || {
        let code = child.wait().ok().and_then(|status| status.code());
        let _ = tx.send(RuntimeEvent::WrappedExit(code));
    });

    Ok(())
}

fn read_wrapped_stream<R>(stream: R, parse_events: bool, tx: Sender<RuntimeEvent>)
where
    R: Read + Send + 'static,
{
    let reader = BufReader::new(stream);
    for line in reader.lines().map_while(Result::ok) {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        if parse_events {
            if let Some(update) = parse_wrapped_output(trimmed) {
                if tx.send(RuntimeEvent::Wrapped(update)).is_err() {
                    return;
                }
                continue;
            }
        }

        if tx
            .send(RuntimeEvent::WrappedLog(truncate_log_line(trimmed)))
            .is_err()
        {
            return;
        }
    }
}

fn truncate_log_line(line: &str) -> String {
    const MAX: usize = 88;
    let mut output = String::new();
    let mut count = 0;

    for ch in line.chars() {
        if count >= MAX {
            output.push_str("...");
            break;
        }
        output.push(ch);
        count += 1;
    }

    output
}

fn avatar_speed_for(state: AgentState) -> (isize, isize) {
    match state {
        AgentState::Idle => (1, 1),
        AgentState::AwaitingInput => (1, 1),
        AgentState::Thinking => (1, 1),
        AgentState::Working => (2, 1),
        AgentState::Success => (2, 2),
        AgentState::Error => (2, 2),
        AgentState::Sleeping => (1, 1),
        AgentState::Tool => (2, 1),
    }
}

fn render(app: &App, width: usize, height: usize) -> String {
    if app.avatar_only {
        return render_avatar_only(app, width, height);
    }

    let safe_width = width.max(88);
    let left_width = 28;
    let gap = 2;
    let right_width = safe_width.saturating_sub(left_width + gap);

    let mut avatar = render_avatar_box(app, left_width);
    let mut status = render_status_box(app, right_width);
    let column_height = avatar.len().max(status.len());
    pad_box_height(&mut avatar, column_height);
    pad_box_height(&mut status, column_height);

    let mut sections = join_columns(&avatar, &status, gap);

    sections.push(String::new());
    sections.extend(render_log_box(app, safe_width));

    if app.show_help {
        sections.push(String::new());
        sections.extend(render_help_box(safe_width));
    }

    let max_lines = height.saturating_sub(1);
    if sections.len() > max_lines {
        sections.truncate(max_lines);
    }

    sections.join("\n")
}

fn render_avatar_only(app: &App, width: usize, height: usize) -> String {
    let art = frame_for(app.state, app.frame_tick);
    let x = app.avatar_motion.x.max(0) as usize;
    let y = app.avatar_motion.y.max(0) as usize;
    let canvas_width = width.max(1);
    let max_lines = height.saturating_sub(1).max(art.len());
    let mut lines = Vec::with_capacity(max_lines);

    for row in 0..max_lines {
        if row < y || row >= y + art.len() {
            lines.push(" ".repeat(canvas_width));
            continue;
        }

        let art_line = art[row - y];
        let art_width = display_width(art_line);
        let left_pad = x.min(width.saturating_sub(art_width));
        let mut content = String::with_capacity(left_pad + art_line.len());
        content.push_str(&" ".repeat(left_pad));
        content.push_str(art_line);
        lines.push(pad_right(&content, canvas_width));
    }

    lines.join("\n")
}

fn render_avatar_box(app: &App, width: usize) -> Vec<String> {
    let mut body = Vec::new();
    for line in frame_for(app.state, app.frame_tick) {
        body.push(line.to_string());
    }

    body.push(String::new());
    body.push(format!("State : {}", app.state.label()));
    body.push(format!("Mood  : {}", app.state.mood()));
    body.push(format!("Signal: {}", app.signal_label()));
    boxify_avatar("Avatar", width, &body, frame_for(app.state, app.frame_tick).len())
}

fn render_status_box(app: &App, width: usize) -> Vec<String> {
    let mut body = Vec::new();
    body.push(format!("Banner : {}", app.state.banner()));
    body.push(format!("Mode   : {}", app.mode_label()));
    body.push(format!("Uptime : {}", format_elapsed(app.start_time.elapsed())));
    body.push(format!(
        "Feed   : {}",
        app.wrapped_command
            .clone()
            .or_else(|| app.events_path.as_ref().map(|path| path.display().to_string()))
            .unwrap_or_else(|| "<manual>".to_string())
    ));
    body.push(format!(
        "Tool   : {}",
        app.tool.as_deref().unwrap_or("<none>")
    ));
    body.push(format!(
        "Badge  : {}",
        app.badge.as_deref().unwrap_or("<none>")
    ));
    body.push(format!(
        "Since  : {}",
        format_elapsed(app.last_state_change.elapsed())
    ));
    body.push(String::new());
    body.push("Message:".to_string());
    body.extend(wrap_text(&app.message, width.saturating_sub(4)));
    boxify("Status", width, &body)
}

fn render_log_box(app: &App, width: usize) -> Vec<String> {
    let mut body: Vec<String> = app.log.iter().cloned().collect();
    if body.is_empty() {
        body.push("No activity yet".to_string());
    }
    boxify("Activity", width, &body)
}

fn render_help_box(width: usize) -> Vec<String> {
    let body = vec![
        "Keys:".to_string(),
        "1 idle   2 input   3 thinking   4 working".to_string(),
        "5 success 6 error  7 sleeping   8 tool".to_string(),
        "d toggle demo mode   h toggle help   q quit".to_string(),
        "External feed: append lines such as".to_string(),
        r#"  state=thinking message="Planning the change""#.to_string(),
        r#"  {"state":"tool","tool":"wrench","message":"Running formatter"}"#.to_string(),
        "zellij sidecar example:".to_string(),
        r#"  pixlair --codex"#.to_string(),
    ];
    boxify("Help", width, &body)
}

fn boxify(title: &str, width: usize, body: &[String]) -> Vec<String> {
    let inner = width.saturating_sub(4).max(10);
    let mut lines = Vec::new();
    lines.push(titled_border(title, width));
    for line in body {
        let wrapped = wrap_text(line, inner);
        if wrapped.is_empty() {
            lines.push(format!("│ {} │", " ".repeat(inner)));
        } else {
            for chunk in wrapped {
                lines.push(format!("│ {} │", pad_right(&chunk, inner)));
            }
        }
    }
    lines.push(format!("└{}┘", "─".repeat(width.saturating_sub(2))));
    lines
}

fn boxify_avatar(title: &str, width: usize, body: &[String], art_lines: usize) -> Vec<String> {
    let inner = width.saturating_sub(4).max(10);
    let mut lines = Vec::new();
    lines.push(titled_border(title, width));

    for (index, line) in body.iter().enumerate() {
        if index < art_lines {
            lines.push(format!("│ {} │", center_text(line, inner)));
            continue;
        }

        let wrapped = wrap_text(line, inner);
        if wrapped.is_empty() {
            lines.push(format!("│ {} │", " ".repeat(inner)));
        } else {
            for chunk in wrapped {
                lines.push(format!("│ {} │", pad_right(&chunk, inner)));
            }
        }
    }

    lines.push(format!("└{}┘", "─".repeat(width.saturating_sub(2))));
    lines
}

fn pad_box_height(lines: &mut Vec<String>, target_height: usize) {
    if lines.len() >= target_height || lines.len() < 2 {
        return;
    }

    let width = lines
        .iter()
        .map(|line| display_width(line))
        .max()
        .unwrap_or(0);
    let inner = width.saturating_sub(4);
    let blank = format!("│ {} │", " ".repeat(inner));

    while lines.len() < target_height {
        let insert_at = lines.len().saturating_sub(1);
        lines.insert(insert_at, blank.clone());
    }
}

fn join_columns(left: &[String], right: &[String], gap: usize) -> Vec<String> {
    let left_width = left
        .iter()
        .map(|line| display_width(line))
        .max()
        .unwrap_or(0);
    let height = left.len().max(right.len());
    let spacer = " ".repeat(gap);

    let mut rows = Vec::with_capacity(height);
    for index in 0..height {
        let left_line = left.get(index).map(String::as_str).unwrap_or("");
        let right_line = right.get(index).map(String::as_str).unwrap_or("");
        rows.push(format!("{}{}{}", pad_right(left_line, left_width), spacer, right_line));
    }
    rows
}

fn wrap_text(text: &str, width: usize) -> Vec<String> {
    if width < 4 {
        return vec![text.to_string()];
    }

    let mut output = Vec::new();
    for raw_line in text.lines() {
        if raw_line.trim().is_empty() {
            output.push(String::new());
            continue;
        }

        let mut current = String::new();
        for word in raw_line.split_whitespace() {
            let segments = split_for_width(word, width);

            for segment in segments {
                if current.is_empty() {
                    current.push_str(&segment);
                    continue;
                }

                if display_width(&current) + 1 + display_width(&segment) > width {
                    output.push(current);
                    current = segment;
                } else {
                    current.push(' ');
                    current.push_str(&segment);
                }
            }
        }

        if current.is_empty() && !raw_line.contains(char::is_whitespace) {
            output.extend(split_for_width(raw_line, width));
            continue;
        }

        if !current.is_empty() {
            output.push(current);
        }
    }

    if output.is_empty() {
        output.push(String::new());
    }

    output
}

fn split_for_width(text: &str, width: usize) -> Vec<String> {
    if display_width(text) <= width {
        return vec![text.to_string()];
    }

    let mut parts = Vec::new();
    let mut current = String::new();
    let mut current_width = 0;

    for ch in text.chars() {
        let ch_width = char_display_width(ch);
        if current_width + ch_width > width && !current.is_empty() {
            parts.push(current);
            current = String::new();
            current_width = 0;
        }

        current.push(ch);
        current_width += ch_width;
    }

    if !current.is_empty() {
        parts.push(current);
    }

    parts
}

fn titled_border(title: &str, width: usize) -> String {
    let content_width = width.saturating_sub(2);
    let title = format!(" {} ", title);
    let title_width = display_width(&title);
    let right_fill = content_width.saturating_sub(title_width);
    format!("┌{}{}┐", title, "─".repeat(right_fill))
}

fn pad_right(text: &str, width: usize) -> String {
    let padding = width.saturating_sub(display_width(text));
    format!("{text}{}", " ".repeat(padding))
}

fn center_text(text: &str, width: usize) -> String {
    let text_width = display_width(text);
    if text_width >= width {
        return text.to_string();
    }

    let total_padding = width - text_width;
    let left = total_padding / 2;
    let right = total_padding - left;
    format!("{}{}{}", " ".repeat(left), text, " ".repeat(right))
}

fn display_width(text: &str) -> usize {
    text.chars().map(char_display_width).sum()
}

fn char_display_width(ch: char) -> usize {
    if ch.is_ascii() {
        return 1;
    }

    if matches!(
        ch as u32,
        0x0300..=0x036F
            | 0x0483..=0x0489
            | 0x0591..=0x05BD
            | 0x05BF
            | 0x05C1..=0x05C2
            | 0x05C4..=0x05C5
            | 0x0610..=0x061A
            | 0x064B..=0x065F
            | 0x0670
            | 0x06D6..=0x06DC
            | 0x06DF..=0x06E4
            | 0x06E7..=0x06E8
            | 0x06EA..=0x06ED
            | 0x0711
            | 0x0730..=0x074A
            | 0x07A6..=0x07B0
            | 0x07EB..=0x07F3
            | 0x0816..=0x0819
            | 0x081B..=0x0823
            | 0x0825..=0x0827
            | 0x0829..=0x082D
            | 0x0859..=0x085B
            | 0x08D3..=0x08E1
            | 0x08E3..=0x0903
            | 0x093A..=0x093C
            | 0x093E..=0x094F
            | 0x0951..=0x0957
            | 0x0962..=0x0963
            | 0x0981..=0x0983
            | 0x09BC
            | 0x09BE..=0x09C4
            | 0x09C7..=0x09C8
            | 0x09CB..=0x09CD
            | 0x09D7
            | 0x09E2..=0x09E3
            | 0x0A01..=0x0A03
            | 0x0A3C
            | 0x0A3E..=0x0A42
            | 0x0A47..=0x0A48
            | 0x0A4B..=0x0A4D
            | 0x0A51
            | 0x0A70..=0x0A71
            | 0x0A75
            | 0x0ABE..=0x0AC5
            | 0x0AC7..=0x0AC9
            | 0x0ACB..=0x0ACD
            | 0x0AE2..=0x0AE3
            | 0x0B01..=0x0B03
            | 0x0B3C
            | 0x0B3E..=0x0B44
            | 0x0B47..=0x0B48
            | 0x0B4B..=0x0B4D
            | 0x0B56..=0x0B57
            | 0x0B62..=0x0B63
            | 0x0B82
            | 0x0BBE..=0x0BC2
            | 0x0BC6..=0x0BC8
            | 0x0BCA..=0x0BCD
            | 0x0BD7
            | 0x0C00..=0x0C04
            | 0x0C3E..=0x0C44
            | 0x0C46..=0x0C48
            | 0x0C4A..=0x0C4D
            | 0x0C55..=0x0C56
            | 0x0C62..=0x0C63
            | 0x0C81..=0x0C83
            | 0x0CBC
            | 0x0CBE..=0x0CC4
            | 0x0CC6..=0x0CC8
            | 0x0CCA..=0x0CCD
            | 0x0CD5..=0x0CD6
            | 0x0CE2..=0x0CE3
            | 0x0D00..=0x0D03
            | 0x0D3B..=0x0D3C
            | 0x0D3E..=0x0D44
            | 0x0D46..=0x0D48
            | 0x0D4A..=0x0D4D
            | 0x0D57
            | 0x0D62..=0x0D63
            | 0x0D82..=0x0D83
            | 0x0DCF..=0x0DD4
            | 0x0DD6
            | 0x0DD8..=0x0DDF
            | 0x0DF2..=0x0DF3
            | 0x0E31
            | 0x0E34..=0x0E3A
            | 0x0E47..=0x0E4E
            | 0x0EB1
            | 0x0EB4..=0x0EBC
            | 0x0EC8..=0x0ECD
            | 0x0F18..=0x0F19
            | 0x0F35
            | 0x0F37
            | 0x0F39
            | 0x0F3E..=0x0F3F
            | 0x0F71..=0x0F84
            | 0x0F86..=0x0F87
            | 0x0F8D..=0x0F97
            | 0x0F99..=0x0FBC
            | 0x0FC6
            | 0x102B..=0x103E
            | 0x1056..=0x1059
            | 0x105E..=0x1060
            | 0x1062..=0x1064
            | 0x1067..=0x106D
            | 0x1071..=0x1074
            | 0x1082..=0x108D
            | 0x108F
            | 0x109A..=0x109D
            | 0x135D..=0x135F
            | 0x1712..=0x1715
            | 0x1732..=0x1734
            | 0x1752..=0x1753
            | 0x1772..=0x1773
            | 0x17B4..=0x17D3
            | 0x17DD
            | 0x180B..=0x180F
            | 0x1885..=0x1886
            | 0x18A9
            | 0x1920..=0x192B
            | 0x1930..=0x193B
            | 0x1A17..=0x1A1B
            | 0x1A55..=0x1A5E
            | 0x1A60..=0x1A7C
            | 0x1A7F
            | 0x1AB0..=0x1ACE
            | 0x1B00..=0x1B04
            | 0x1B34..=0x1B44
            | 0x1B6B..=0x1B73
            | 0x1B80..=0x1B82
            | 0x1BA1..=0x1BAD
            | 0x1BE6..=0x1BF3
            | 0x1C24..=0x1C37
            | 0x1CD0..=0x1CD2
            | 0x1CD4..=0x1CE8
            | 0x1CED
            | 0x1CF2..=0x1CF4
            | 0x1CF7
            | 0x1DC0..=0x1DFF
            | 0x200C..=0x200D
            | 0x20D0..=0x20FF
            | 0x2CEF..=0x2CF1
            | 0x2D7F
            | 0x2DE0..=0x2DFF
            | 0x302A..=0x302F
            | 0x3099..=0x309A
            | 0xA66F..=0xA672
            | 0xA674..=0xA67D
            | 0xA69E..=0xA69F
            | 0xA6F0..=0xA6F1
            | 0xA802
            | 0xA806
            | 0xA80B
            | 0xA823..=0xA827
            | 0xA880..=0xA881
            | 0xA8B4..=0xA8C5
            | 0xA8E0..=0xA8F1
            | 0xA8FF
            | 0xA926..=0xA92D
            | 0xA947..=0xA953
            | 0xA980..=0xA983
            | 0xA9B3..=0xA9C0
            | 0xA9E5
            | 0xAA29..=0xAA36
            | 0xAA43
            | 0xAA4C..=0xAA4D
            | 0xAA7B..=0xAA7D
            | 0xAAB0
            | 0xAAB2..=0xAAB4
            | 0xAAB7..=0xAAB8
            | 0xAABE..=0xAABF
            | 0xAAC1
            | 0xAAEB..=0xAAEF
            | 0xAAF5..=0xAAF6
            | 0xABE3..=0xABEA
            | 0xABEC..=0xABED
            | 0xFB1E
            | 0xFE00..=0xFE0F
            | 0xFE20..=0xFE2F
    ) {
        return 0;
    }

    if matches!(
        ch as u32,
        0x1100..=0x115F
            | 0x231A..=0x231B
            | 0x2329..=0x232A
            | 0x23E9..=0x23EC
            | 0x23F0
            | 0x23F3
            | 0x25FD..=0x25FE
            | 0x2614..=0x2615
            | 0x2648..=0x2653
            | 0x267F
            | 0x2693
            | 0x26A1
            | 0x26AA..=0x26AB
            | 0x26BD..=0x26BE
            | 0x26C4..=0x26C5
            | 0x26CE
            | 0x26D4
            | 0x26EA
            | 0x26F2..=0x26F3
            | 0x26F5
            | 0x26FA
            | 0x26FD
            | 0x2705
            | 0x270A..=0x270B
            | 0x2728
            | 0x274C
            | 0x274E
            | 0x2753..=0x2755
            | 0x2757
            | 0x2795..=0x2797
            | 0x27B0
            | 0x27BF
            | 0x2B1B..=0x2B1C
            | 0x2B50
            | 0x2B55
            | 0x2E80..=0x303E
            | 0x3040..=0xA4CF
            | 0xAC00..=0xD7A3
            | 0xF900..=0xFAFF
            | 0xFE10..=0xFE19
            | 0xFE30..=0xFE6F
            | 0xFF00..=0xFF60
            | 0xFFE0..=0xFFE6
            | 0x1F300..=0x1F64F
            | 0x1F900..=0x1F9FF
            | 0x20000..=0x2FFFD
            | 0x30000..=0x3FFFD
    ) {
        2
    } else {
        1
    }
}

fn format_elapsed(duration: Duration) -> String {
    let seconds = duration.as_secs();
    let hours = seconds / 3600;
    let minutes = (seconds % 3600) / 60;
    let seconds = seconds % 60;

    if hours > 0 {
        format!("{hours:02}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes:02}:{seconds:02}")
    }
}
