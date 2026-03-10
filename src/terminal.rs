use std::io::{self, IsTerminal, Write};
use std::os::fd::AsRawFd;
use std::process::{Command, Stdio};

#[derive(Clone, Copy)]
pub enum TerminalMode {
    Fullscreen,
    Sidecar,
}

pub struct TerminalSession {
    mode: TerminalMode,
    last_frame: Option<String>,
    saved_stty: Option<String>,
}

impl TerminalSession {
    pub fn enter(mode: TerminalMode) -> io::Result<Self> {
        if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
            return Err(io::Error::new(
                io::ErrorKind::Other,
                "Pixlair requires an interactive TTY",
            ));
        }

        let saved_stty = match mode {
            TerminalMode::Fullscreen => {
                let saved_stty = stty_capture().ok();
                let _ = stty_apply(&["raw", "-echo"]);
                saved_stty
            }
            TerminalMode::Sidecar => None,
        };

        let mut stdout = io::stdout();
        match mode {
            TerminalMode::Fullscreen => write!(stdout, "\x1b[?1049h\x1b[?25l\x1b[2J\x1b[H")?,
            TerminalMode::Sidecar => write!(stdout, "\x1b[?25l\x1b[2J\x1b[H")?,
        }
        stdout.flush()?;

        Ok(Self {
            mode,
            last_frame: None,
            saved_stty,
        })
    }

    pub fn render(&mut self, frame: &str) -> io::Result<()> {
        if self.last_frame.as_deref() == Some(frame) {
            return Ok(());
        }

        let mut stdout = io::stdout();
        write!(stdout, "\x1b[H{frame}\x1b[J")?;
        stdout.flush()?;
        self.last_frame = Some(frame.to_string());
        Ok(())
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        if let Some(state) = &self.saved_stty {
            let _ = stty_apply(&[state]);
        }

        let mut stdout = io::stdout();
        match self.mode {
            TerminalMode::Fullscreen => {
                let _ = write!(stdout, "\x1b[?25h\x1b[?1049l");
            }
            TerminalMode::Sidecar => {
                let _ = write!(stdout, "\x1b[?25h\x1b[H\x1b[J");
            }
        }
        let _ = stdout.flush();
    }
}

pub fn terminal_size() -> Option<(usize, usize)> {
    if let Some(size) = ioctl_size() {
        return Some(size);
    }

    if let Some(size) = stty_size() {
        return Some(size);
    }

    if let (Ok(columns), Ok(lines)) = (std::env::var("COLUMNS"), std::env::var("LINES")) {
        if let (Ok(columns), Ok(lines)) = (columns.parse::<usize>(), lines.parse::<usize>()) {
            return Some((columns, lines));
        }
    }

    None
}

fn ioctl_size() -> Option<(usize, usize)> {
    #[repr(C)]
    struct WinSize {
        ws_row: u16,
        ws_col: u16,
        ws_xpixel: u16,
        ws_ypixel: u16,
    }

    unsafe extern "C" {
        fn ioctl(fd: i32, request: usize, ...) -> i32;
    }

    const TIOCGWINSZ: usize = 0x5413;

    let stdout = io::stdout();
    let fd = stdout.as_raw_fd();
    let mut size = WinSize {
        ws_row: 0,
        ws_col: 0,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };

    let status = unsafe { ioctl(fd, TIOCGWINSZ, &mut size) };
    if status != 0 || size.ws_col == 0 || size.ws_row == 0 {
        return None;
    }

    Some((usize::from(size.ws_col), usize::from(size.ws_row)))
}

fn stty_size() -> Option<(usize, usize)> {
    let output = Command::new("stty")
        .arg("size")
        .stdin(Stdio::inherit())
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let text = String::from_utf8_lossy(&output.stdout);
    let mut parts = text.split_whitespace();
    let rows = parts.next()?.parse::<usize>().ok()?;
    let cols = parts.next()?.parse::<usize>().ok()?;
    Some((cols, rows))
}

fn stty_capture() -> io::Result<String> {
    let output = Command::new("stty")
        .arg("-g")
        .stdin(Stdio::inherit())
        .output()?;

    if !output.status.success() {
        return Err(io::Error::new(
            io::ErrorKind::Other,
            "failed to read terminal mode",
        ));
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn stty_apply(args: &[&str]) -> io::Result<()> {
    let status = Command::new("stty")
        .args(args)
        .stdin(Stdio::inherit())
        .status()?;

    if status.success() {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::Other,
            "failed to apply terminal mode",
        ))
    }
}
