//! The first user interface: a full-screen terminal shell.
//!
//! The console has always had a command line and text listings; this module is
//! the first *shell* around them. It owns no lighting logic. It draws a header,
//! a window bar, the body of the selected window, the message line and the
//! command prompt, and it sends every entered line to
//! [`Engine::execute`](engine::Engine::execute) — the same path the REPL,
//! scripts and tests use. A GUI window later replaces the drawing, not the
//! command path.
//!
//! Drawing is a pure function of the engine and [`UiState`], so layouts and key
//! handling are covered by tests without a terminal. Only [`run`] touches the
//! tty: it puts the terminal into raw mode, draws on the alternate screen and
//! restores everything on exit, including a panic.

use std::io::{self, Write};
use std::sync::{Mutex, MutexGuard};

use engine::{Engine, Runtime};

/// How many entered lines the shell remembers for Up/Down.
const HISTORY: usize = 100;
/// Lines a PageUp or PageDown moves the window.
const PAGE: usize = 10;
/// Rows the shell spends on chrome: header, window bar, two rules, message,
/// prompt and footer.
const CHROME: usize = 7;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Sheet,
    Playback,
    Pools,
    Timecode,
    Status,
}

impl Tab {
    pub const ALL: [Tab; 5] = [
        Tab::Sheet,
        Tab::Playback,
        Tab::Pools,
        Tab::Timecode,
        Tab::Status,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Tab::Sheet => "Sheet",
            Tab::Playback => "Playback",
            Tab::Pools => "Pools",
            Tab::Timecode => "Timecode",
            Tab::Status => "Status",
        }
    }

    /// The listing commands that fill this window. Reusing the command line
    /// keeps one description of every pool instead of a second renderer.
    fn commands(self) -> &'static [&'static str] {
        match self {
            Tab::Sheet => &["Sheet"],
            Tab::Playback => &["Sequence", "Executor"],
            Tab::Pools => &["Group", "World", "Filter", "Effect"],
            Tab::Timecode => &["Timecode"],
            Tab::Status => &["Status"],
        }
    }

    fn next(self) -> Self {
        let index = Self::ALL
            .iter()
            .position(|tab| *tab == self)
            .unwrap_or_default();
        Self::ALL[(index + 1) % Self::ALL.len()]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Char(char),
    Enter,
    Backspace,
    Tab,
    Up,
    Down,
    PageUp,
    PageDown,
    Escape,
    Interrupt,
    Eof,
    /// A key the shell has no use for (function keys, other control codes).
    Ignore,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Redraw,
    Quit,
}

#[derive(Debug)]
pub struct UiState {
    pub tab: Tab,
    pub input: String,
    pub message: String,
    pub error: bool,
    history: Vec<String>,
    history_index: Option<usize>,
    scroll: usize,
}

impl Default for UiState {
    fn default() -> Self {
        Self {
            tab: Tab::Sheet,
            input: String::new(),
            message: "Ready. Type `Help` for commands.".into(),
            error: false,
            history: Vec::new(),
            history_index: None,
            scroll: 0,
        }
    }
}

impl UiState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Handle one key. Everything that reaches the engine goes through
    /// `Engine::execute`, so the shell cannot invent behaviour.
    pub fn handle(&mut self, key: Key, engine: &mut Engine) -> Action {
        match key {
            Key::Char(character) => {
                self.input.push(character);
                self.history_index = None;
            }
            Key::Backspace => {
                self.input.pop();
                self.history_index = None;
            }
            Key::Enter => return self.submit(engine),
            Key::Tab => {
                self.tab = self.tab.next();
                self.scroll = 0;
            }
            Key::Up => self.previous_line(),
            Key::Down => self.next_line(),
            Key::PageUp => self.scroll = self.scroll.saturating_sub(PAGE),
            Key::PageDown => self.scroll = self.scroll.saturating_add(PAGE),
            Key::Escape => {
                self.input.clear();
                self.message.clear();
                self.error = false;
                self.history_index = None;
                self.scroll = 0;
            }
            Key::Interrupt => {
                self.input.clear();
                self.message.clear();
                self.error = false;
                self.history_index = None;
            }
            Key::Eof => return Action::Quit,
            Key::Ignore => {}
        }
        Action::Redraw
    }

    fn submit(&mut self, engine: &mut Engine) -> Action {
        let line = self.input.trim().to_string();
        self.input.clear();
        self.history_index = None;
        if line.is_empty() {
            return Action::Redraw;
        }
        self.scroll = 0;
        if line.eq_ignore_ascii_case("exit") || line.eq_ignore_ascii_case("quit") {
            return Action::Quit;
        }
        if self
            .history
            .last()
            .map(|last| last != &line)
            .unwrap_or(true)
        {
            self.history.push(line.clone());
            if self.history.len() > HISTORY {
                self.history.remove(0);
            }
        }
        match engine.execute(&line) {
            Ok(outcome) => {
                self.error = false;
                self.message = outcome.lines.join("  ·  ");
            }
            Err(error) => {
                self.error = true;
                self.message = format!("error: {error}");
            }
        }
        Action::Redraw
    }

    fn previous_line(&mut self) {
        if self.history.is_empty() {
            return;
        }
        let index = match self.history_index {
            None => self.history.len() - 1,
            Some(0) => 0,
            Some(index) => index - 1,
        };
        self.history_index = Some(index);
        self.input = self.history[index].clone();
    }

    fn next_line(&mut self) {
        let Some(index) = self.history_index else {
            return;
        };
        if index + 1 >= self.history.len() {
            self.history_index = None;
            self.input.clear();
        } else {
            self.history_index = Some(index + 1);
            self.input = self.history[index + 1].clone();
        }
    }

    /// The whole screen as plain text lines: header, window bar, body, rules,
    /// message, prompt and footer. No colour or escapes, so it is testable.
    pub fn render(&self, engine: &mut Engine, width: usize, height: usize) -> Vec<String> {
        let width = width.max(40);
        let height = height.max(CHROME + 1);
        let body_height = height - CHROME;

        let left = format!(
            " OpenLight — {}{}{}",
            engine.show.name,
            if engine.desk.blackout.is_some() {
                "   BLACKOUT"
            } else {
                ""
            },
            if engine.desk.blind { "   BLIND" } else { "" }
        );
        let right = format!("{}  ", engine.timecode_status());
        let mut lines = vec![join(&left, &right, width)];

        let tabs: Vec<String> = Tab::ALL
            .iter()
            .map(|tab| {
                if *tab == self.tab {
                    format!("[{}]", tab.title())
                } else {
                    format!(" {} ", tab.title())
                }
            })
            .collect();
        lines.push(clip(&format!(" {}", tabs.join(" ")), width));
        lines.push("-".repeat(width));

        let body = self.body(engine);
        for line in body.iter().skip(self.scroll).take(body_height) {
            lines.push(pad(line, width));
        }
        while lines.len() < 3 + body_height {
            lines.push(" ".repeat(width));
        }
        lines.push("-".repeat(width));
        lines.push(clip(
            &format!(" {}{}", if self.error { "! " } else { "  " }, self.message),
            width,
        ));
        lines.push(clip(&format!("openlight> {}", self.input), width));
        lines.push(clip(
            " Tab window  Enter run  Up/Down history  PgUp/PgDn scroll  Esc clear  Ctrl-D quit",
            width,
        ));
        lines
    }

    pub fn prompt_column(&self, width: usize) -> usize {
        ("openlight> ".chars().count() + self.input.chars().count()).min(width.saturating_sub(1))
    }

    fn body(&self, engine: &mut Engine) -> Vec<String> {
        let mut lines = Vec::new();
        for command in self.tab.commands() {
            if !lines.is_empty() {
                lines.push(String::new());
            }
            match engine.execute(command) {
                Ok(outcome) => lines.extend(outcome.lines),
                Err(error) => lines.push(format!("error: {error}")),
            }
        }
        if lines.is_empty() {
            lines.push("Nothing to show.".into());
        }
        lines
    }
}

fn clip(text: &str, width: usize) -> String {
    if text.chars().count() <= width {
        return text.to_string();
    }
    if width == 0 {
        return String::new();
    }
    let mut result: String = text.chars().take(width - 1).collect();
    result.push('…');
    result
}

/// Clip to `width` and fill the rest so a redraw covers the previous frame.
fn pad(text: &str, width: usize) -> String {
    let clipped = clip(text, width);
    let missing = width.saturating_sub(clipped.chars().count());
    format!("{clipped}{}", " ".repeat(missing))
}

fn join(left: &str, right: &str, width: usize) -> String {
    let left_width = left.chars().count();
    let right_width = right.chars().count();
    if left_width + right_width + 1 > width {
        return clip(&format!("{left} {right}"), width);
    }
    let padding = " ".repeat(width - left_width - right_width);
    format!("{left}{padding}{right}")
}

/// Draw the shell until the user quits. The terminal is restored on the way
/// out, including when a command panics.
#[cfg(unix)]
pub fn run(runtime: &Runtime) -> io::Result<()> {
    let mut terminal = Terminal::enter()?;
    let handle = runtime.engine();
    let mut state = UiState::new();

    loop {
        let (width, height) = terminal.size();
        let lines = {
            let mut engine = lock(&handle);
            state.render(&mut engine, width, height)
        };
        let prompt_row = lines.len().saturating_sub(2);
        let column = state.prompt_column(width);
        terminal.draw(&lines, (prompt_row, column))?;

        let key = terminal.read_key()?;
        let action = {
            let mut engine = lock(&handle);
            state.handle(key, &mut engine)
        };
        if action == Action::Quit {
            break;
        }
    }
    Ok(())
}

#[cfg(not(unix))]
pub fn run(_runtime: &Runtime) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "--ui needs a Unix terminal",
    ))
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(unix)]
struct Terminal {
    original: libc::termios,
}

#[cfg(unix)]
impl Terminal {
    fn enter() -> io::Result<Self> {
        let fd = libc::STDIN_FILENO;
        if unsafe { libc::isatty(fd) } != 1 {
            return Err(io::Error::other(
                "--ui needs an interactive terminal (stdin is not a tty)",
            ));
        }
        let mut original: libc::termios = unsafe { std::mem::zeroed() };
        if unsafe { libc::tcgetattr(fd, &mut original) } != 0 {
            return Err(io::Error::last_os_error());
        }
        let mut raw = original;
        unsafe { libc::cfmakeraw(&mut raw) };
        raw.c_cc[libc::VMIN] = 1;
        raw.c_cc[libc::VTIME] = 0;
        if unsafe { libc::tcsetattr(fd, libc::TCSANOW, &raw) } != 0 {
            return Err(io::Error::last_os_error());
        }
        let mut terminal = Self { original };
        terminal.write("\x1b[?1049h")?;
        Ok(terminal)
    }

    fn size(&self) -> (usize, usize) {
        let mut size: libc::winsize = unsafe { std::mem::zeroed() };
        let result = unsafe { libc::ioctl(libc::STDOUT_FILENO, libc::TIOCGWINSZ, &mut size) };
        if result == 0 && size.ws_col > 0 && size.ws_row > 0 {
            (usize::from(size.ws_col), usize::from(size.ws_row))
        } else {
            (100, 30)
        }
    }

    fn draw(&mut self, lines: &[String], cursor: (usize, usize)) -> io::Result<()> {
        let mut screen = String::from("\x1b[H");
        for (index, line) in lines.iter().enumerate() {
            if index > 0 {
                screen.push_str("\r\n");
            }
            screen.push_str(line);
        }
        screen.push_str("\x1b[J");
        screen.push_str(&format!("\x1b[{};{}H", cursor.0 + 1, cursor.1 + 1));
        self.write(&screen)
    }

    fn write(&mut self, text: &str) -> io::Result<()> {
        let mut stdout = io::stdout();
        stdout.write_all(text.as_bytes())?;
        stdout.flush()
    }

    fn read_key(&self) -> io::Result<Key> {
        let byte = loop {
            let mut buffer = [0u8; 1];
            let read =
                unsafe { libc::read(libc::STDIN_FILENO, buffer.as_mut_ptr().cast(), buffer.len()) };
            if read == 1 {
                break buffer[0];
            }
            if read == 0 {
                return Ok(Key::Eof);
            }
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error);
        };

        match byte {
            b'\r' | b'\n' => Ok(Key::Enter),
            0x7f | 0x08 => Ok(Key::Backspace),
            b'\t' => Ok(Key::Tab),
            0x03 => Ok(Key::Interrupt),
            0x04 => Ok(Key::Eof),
            0x1b => self.read_escape(),
            0x00..=0x1f => Ok(Key::Ignore),
            0x20..=0x7e => Ok(Key::Char(char::from(byte))),
            _ => self.read_multibyte(byte),
        }
    }

    fn read_escape(&self) -> io::Result<Key> {
        let mut poll = libc::pollfd {
            fd: libc::STDIN_FILENO,
            events: libc::POLLIN,
            revents: 0,
        };
        let ready = unsafe { libc::poll(&mut poll, 1, 15) };
        if ready <= 0 {
            return Ok(Key::Escape);
        }
        let mut buffer = [0u8; 8];
        let read =
            unsafe { libc::read(libc::STDIN_FILENO, buffer.as_mut_ptr().cast(), buffer.len()) };
        if read <= 0 {
            return Ok(Key::Escape);
        }
        let bytes = &buffer[..read as usize];
        Ok(if bytes == b"[A" {
            Key::Up
        } else if bytes == b"[B" {
            Key::Down
        } else if bytes == b"[5~" {
            Key::PageUp
        } else if bytes == b"[6~" {
            Key::PageDown
        } else if bytes == b"[3~" {
            Key::Backspace
        } else {
            Key::Ignore
        })
    }

    fn read_multibyte(&self, first: u8) -> io::Result<Key> {
        let length = match first {
            0xc0..=0xdf => 2,
            0xe0..=0xef => 3,
            0xf0..=0xf7 => 4,
            _ => return Ok(Key::Ignore),
        };
        let mut bytes = [0u8; 4];
        bytes[0] = first;
        for slot in bytes.iter_mut().take(length).skip(1) {
            let mut buffer = [0u8; 1];
            let read =
                unsafe { libc::read(libc::STDIN_FILENO, buffer.as_mut_ptr().cast(), buffer.len()) };
            if read != 1 {
                return Ok(Key::Ignore);
            }
            *slot = buffer[0];
        }
        Ok(std::str::from_utf8(&bytes[..length])
            .ok()
            .and_then(|text| text.chars().next())
            .map(Key::Char)
            .unwrap_or(Key::Ignore))
    }
}

#[cfg(unix)]
impl Drop for Terminal {
    fn drop(&mut self) {
        let _ = self.write("\x1b[?1049l");
        unsafe {
            libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &self.original);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state_and_engine() -> (UiState, Engine) {
        (UiState::new(), Engine::empty())
    }

    fn type_line(state: &mut UiState, engine: &mut Engine, line: &str) {
        for character in line.chars() {
            state.handle(Key::Char(character), engine);
        }
        state.handle(Key::Enter, engine);
    }

    #[test]
    fn the_shell_draws_a_header_windows_body_and_prompt() {
        let (state, mut engine) = state_and_engine();
        let lines = state.render(&mut engine, 80, 24);
        assert_eq!(lines.len(), 24);
        assert!(lines[0].contains("OpenLight"), "{:?}", lines[0]);
        assert!(lines[0].contains("idle"), "{}", lines[0]);
        assert!(lines[1].contains("[Sheet]"), "{:?}", lines[1]);
        assert!(
            lines
                .iter()
                .any(|line| line.contains("No fixtures patched")),
            "{lines:#?}"
        );
        assert_eq!(lines.len() - 2, 22);
        assert!(lines[22].starts_with("openlight> "), "{:?}", lines[22]);
        assert!(lines[23].contains("Ctrl-D"), "{:?}", lines[23]);
        assert_eq!(lines[3].chars().count(), 80, "body is padded to width");
    }

    #[test]
    fn the_header_marks_blind() {
        let (mut state, mut engine) = state_and_engine();
        type_line(&mut state, &mut engine, "Blind");
        let lines = state.render(&mut engine, 80, 24);
        assert!(lines[0].contains("BLIND"), "{:?}", lines[0]);

        type_line(&mut state, &mut engine, "Blind Off");
        let lines = state.render(&mut engine, 80, 24);
        assert!(!lines[0].contains("BLIND"), "{:?}", lines[0]);
    }

    #[test]
    fn entering_lines_runs_them_and_reports_errors() {
        let (mut state, mut engine) = state_and_engine();
        type_line(&mut state, &mut engine, "Status");
        assert!(state.message.contains("Output:"), "{}", state.message);
        assert!(!state.error);

        type_line(&mut state, &mut engine, "Nonsense");
        assert!(state.error);
        assert!(
            state.message.contains("unknown command"),
            "{}",
            state.message
        );
    }

    #[test]
    fn typing_editing_and_history_work_on_the_command_line() {
        let (mut state, mut engine) = state_and_engine();
        for character in "Help".chars() {
            state.handle(Key::Char(character), &mut engine);
        }
        assert_eq!(state.input, "Help");
        state.handle(Key::Backspace, &mut engine);
        assert_eq!(state.input, "Hel");
        state.handle(Key::Escape, &mut engine);
        assert!(state.input.is_empty());

        type_line(&mut state, &mut engine, "Help");
        state.handle(Key::Up, &mut engine);
        assert_eq!(state.input, "Help");
        state.handle(Key::Down, &mut engine);
        assert!(state.input.is_empty());
    }

    #[test]
    fn tab_cycles_the_windows_and_scrolling_moves_the_body() {
        let (mut state, mut engine) = state_and_engine();
        assert_eq!(state.tab, Tab::Sheet);
        state.handle(Key::Tab, &mut engine);
        assert_eq!(state.tab, Tab::Playback);
        for _ in 0..4 {
            state.handle(Key::Tab, &mut engine);
        }
        assert_eq!(state.tab, Tab::Sheet);

        state.handle(Key::PageDown, &mut engine);
        state.handle(Key::PageDown, &mut engine);
        state.handle(Key::PageUp, &mut engine);
        state.handle(Key::Escape, &mut engine);
    }

    #[test]
    fn ctrl_d_quits_and_exit_quits() {
        let (mut state, mut engine) = state_and_engine();
        assert_eq!(state.handle(Key::Eof, &mut engine), Action::Quit);
        for character in "exit".chars() {
            state.handle(Key::Char(character), &mut engine);
        }
        assert_eq!(state.handle(Key::Enter, &mut engine), Action::Quit);
    }

    #[test]
    fn narrow_terminals_are_clipped_not_wrapped() {
        let (state, mut engine) = state_and_engine();
        let lines = state.render(&mut engine, 40, 10);
        assert_eq!(lines.len(), 10);
        for line in &lines {
            assert!(line.chars().count() <= 40, "{line:?}");
        }
    }
}
