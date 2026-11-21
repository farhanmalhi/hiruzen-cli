//! Runs the user's shell inside a pseudo-terminal and draws the suggestion
//! dropdown over its output.
//!
//! Everything the shell prints passes through to the real terminal and is
//! also applied to a [`Screen`] model. From the model the session reads the
//! command being typed, asks the engine for suggestions on a worker thread,
//! and draws them. Before the shell's next output is passed on, whatever the
//! dropdown covered is put back, so the shell never sees a changed screen.

use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::time::Duration;

use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize};

use crate::engine::{self, Completion, Engine};
use crate::shell::Shell;
use crate::term::marks::{Chunk, Mark, Scanner};
use crate::term::popup::{self, Request};
use crate::term::screen::{Anchor, Screen};
use crate::term::stream::Stream;

/// How often the terminal size is checked for a change.
const RESIZE_POLL: Duration = Duration::from_millis(120);

enum Event {
    Input(Vec<u8>),
    Output(Vec<u8>),
    Resize(u16, u16),
    Completed { version: u64, result: Option<Completion> },
    Exit(i32),
}

struct CompletionRequest {
    version: u64,
    line: String,
    cwd: PathBuf,
}

#[derive(Debug, PartialEq)]
enum Key {
    Tab,
    Up,
    Down,
    Escape,
    Other,
}

fn classify(input: &[u8]) -> Key {
    match input {
        b"\t" => Key::Tab,
        b"\x1b[A" | b"\x1bOA" => Key::Up,
        b"\x1b[B" | b"\x1bOB" => Key::Down,
        b"\x1b" => Key::Escape,
        _ => Key::Other,
    }
}

/// Puts the terminal in raw mode and restores it when dropped.
struct RawMode;

impl RawMode {
    fn enable() -> Result<RawMode, String> {
        crossterm::terminal::enable_raw_mode().map_err(|e| format!("cannot put the terminal in raw mode: {e}"))?;
        #[cfg(windows)]
        windows_console::enable_virtual_terminal();
        Ok(RawMode)
    }
}

impl Drop for RawMode {
    fn drop(&mut self) {
        let _ = crossterm::terminal::disable_raw_mode();
    }
}

#[cfg(windows)]
mod windows_console {
    use windows_sys::Win32::System::Console::{
        GetConsoleMode, GetStdHandle, SetConsoleMode, ENABLE_VIRTUAL_TERMINAL_INPUT,
        ENABLE_VIRTUAL_TERMINAL_PROCESSING, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE,
    };

    /// Keys arrive as escape sequences and output escape sequences are
    /// interpreted, as on Unix terminals.
    pub fn enable_virtual_terminal() {
        unsafe {
            let input = GetStdHandle(STD_INPUT_HANDLE);
            let mut mode = 0;
            if GetConsoleMode(input, &mut mode) != 0 {
                SetConsoleMode(input, mode | ENABLE_VIRTUAL_TERMINAL_INPUT);
            }
            let output = GetStdHandle(STD_OUTPUT_HANDLE);
            let mut mode = 0;
            if GetConsoleMode(output, &mut mode) != 0 {
                SetConsoleMode(output, mode | ENABLE_VIRTUAL_TERMINAL_PROCESSING);
            }
        }
    }
}

/// The rows the dropdown currently covers.
struct Drawn {
    top: u16,
    height: u16,
}

struct Session {
    shell: Shell,
    engine: Arc<Engine>,
    screen: Screen,
    scanner: Scanner,
    stream: Stream,
    /// Where the command being typed starts, once the shell has drawn a prompt.
    anchor: Option<Anchor>,
    cwd: PathBuf,
    /// The command line up to the cursor, while the user is at a prompt.
    line: Option<String>,
    /// Bumped whenever `line` changes; stale completions are discarded by it.
    version: u64,
    completion: Option<Completion>,
    selected: usize,
    /// Column the dropdown lines up with: where the word being completed starts.
    popup_col: u16,
    drawn: Option<Drawn>,
    /// The line at the time the user dismissed the dropdown with Escape.
    dismissed: Option<String>,
    /// The user is walking shell history with the arrow keys; stay out of the way.
    browsing_history: bool,
    requests: Sender<CompletionRequest>,
    pty: Box<dyn Write + Send>,
}

impl Session {
    fn write_terminal(&self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        let mut out = std::io::stdout().lock();
        // A failed write means the terminal is gone; the shell exiting ends the session.
        let _ = out.write_all(bytes);
        let _ = out.flush();
    }

    fn write_shell(&mut self, bytes: &[u8]) {
        let _ = self.pty.write_all(bytes);
        let _ = self.pty.flush();
    }

    /// Sequences that restore the rows under the dropdown.
    fn erase(&mut self) -> String {
        let Some(drawn) = self.drawn.take() else {
            return String::new();
        };
        let (_, rows) = self.screen.size();
        let mut out = String::from("\x1b7");
        for row in drawn.top..(drawn.top + drawn.height).min(rows) {
            out.push_str(&self.screen.repaint_row(row));
        }
        out.push_str("\x1b8");
        out
    }

    /// Sequences that draw the dropdown for the current completion, if it
    /// should be showing.
    fn draw(&mut self) -> String {
        if self.line.is_none() || self.browsing_history || self.dismissed.is_some() || self.screen.alternate() {
            return String::new();
        }
        let Some(completion) = &self.completion else {
            return String::new();
        };
        let (cols, rows) = self.screen.size();
        let (cursor_row, _) = self.screen.cursor();
        let command_row = self
            .anchor
            .and_then(|anchor| self.screen.anchor_row(&anchor))
            .unwrap_or(cursor_row);
        let request = Request {
            items: &completion.items,
            selected: self.selected,
            hint: completion.hint.as_deref(),
            cols: cols.into(),
            rows: rows.into(),
            cursor_row: cursor_row.into(),
            command_row: command_row.into(),
            anchor_col: self.popup_col.into(),
        };
        let Some(layout) = popup::layout(&request) else {
            return String::new();
        };
        let mut out = String::from("\x1b7");
        if !self.screen.cursor_hidden() {
            out.push_str("\x1b[?25l");
        }
        for (i, row) in layout.rows.iter().enumerate() {
            out.push_str(&format!("\x1b[{};{}H{row}", layout.top + i + 1, layout.left + 1));
        }
        out.push_str("\x1b8");
        if !self.screen.cursor_hidden() {
            out.push_str("\x1b[?25h");
        }
        self.drawn = Some(Drawn {
            top: layout.top as u16,
            height: layout.rows.len() as u16,
        });
        out
    }

    /// Erases and redraws the dropdown in one write, so it does not flicker.
    fn refresh(&mut self) {
        if !self.stream.is_idle() {
            return;
        }
        let mut out = self.erase();
        out.push_str(&self.draw());
        self.write_terminal(out.as_bytes());
    }

    fn selectable(&self) -> bool {
        self.drawn.is_some() && self.completion.as_ref().is_some_and(|c| !c.items.is_empty())
    }

    /// Forgets the prompt: a command is running, or the line was abandoned.
    fn leave_prompt(&mut self) {
        self.anchor = None;
        self.line = None;
        self.completion = None;
        self.dismissed = None;
        self.version += 1;
    }

    /// Re-reads the command line from the screen and requests suggestions
    /// when it has changed.
    fn sync_line(&mut self) {
        let line = if self.screen.alternate() {
            None
        } else {
            self.anchor.and_then(|anchor| {
                let row = self.screen.anchor_row(&anchor)?;
                let cursor = self.screen.cursor();
                (cursor >= (row, anchor.col)).then(|| self.screen.text_between((row, anchor.col), cursor))
            })
        };
        if line == self.line {
            return;
        }
        self.line = line;
        self.version += 1;

        if let (Some(dismissed), Some(line)) = (&self.dismissed, &self.line) {
            // Dismissal lasts while the same word is being edited.
            let same_word = |longer: &str, shorter: &str| {
                longer
                    .strip_prefix(shorter)
                    .is_some_and(|rest| !rest.contains(char::is_whitespace))
            };
            if !same_word(line, dismissed) && !same_word(dismissed, line) {
                self.dismissed = None;
            }
        }

        match &self.line {
            Some(line) if !line.trim().is_empty() && self.dismissed.is_none() && !self.browsing_history => {
                let _ = self.requests.send(CompletionRequest {
                    version: self.version,
                    line: line.clone(),
                    cwd: self.cwd.clone(),
                });
            }
            _ => self.completion = None,
        }
    }

    fn on_output(&mut self, bytes: &[u8]) {
        // Put back what the dropdown covers before the shell's output lands on it.
        let mut out = self.erase().into_bytes();
        for chunk in self.scanner.feed(bytes) {
            match chunk {
                Chunk::Data(data) => {
                    self.screen.feed(&data);
                    self.stream.feed(&data);
                    out.extend_from_slice(&data);
                }
                Chunk::Mark(Mark::PromptStart) => {
                    self.anchor = None;
                }
                Chunk::Mark(Mark::PromptEnd) => {
                    self.anchor = Some(self.screen.anchor());
                    self.dismissed = None;
                    self.browsing_history = false;
                }
                Chunk::Mark(Mark::Cwd(path)) => {
                    self.cwd = normalize_cwd(&path, self.shell);
                }
            }
        }
        self.sync_line();
        if self.stream.is_idle() {
            out.extend_from_slice(self.draw().as_bytes());
        }
        self.write_terminal(&out);
    }

    fn on_completed(&mut self, version: u64, result: Option<Completion>) {
        if version != self.version {
            return;
        }
        let (_, cursor_col) = self.screen.cursor();
        self.popup_col = result
            .as_ref()
            .map(|completion| cursor_col.saturating_sub(completion.token.width as u16))
            .unwrap_or(cursor_col);
        self.completion = result;
        self.selected = 0;
        self.refresh();
    }

    fn on_input(&mut self, bytes: &[u8]) {
        let key = classify(bytes);
        if self.selectable() {
            let count = self.completion.as_ref().map_or(0, |c| c.items.len());
            match key {
                Key::Tab => {
                    let edit = self.completion.as_ref().and_then(|completion| {
                        engine::edit_for(&completion.token, completion.items.get(self.selected)?)
                    });
                    if let Some(edit) = edit {
                        let mut keys = "\x7f".repeat(edit.backspaces);
                        keys.push_str(&edit.text);
                        keys.push_str(&"\x1b[D".repeat(edit.cursor_back));
                        self.write_shell(keys.as_bytes());
                    }
                    return;
                }
                Key::Down => {
                    self.selected = (self.selected + 1) % count;
                    self.refresh();
                    return;
                }
                Key::Up if self.selected > 0 => {
                    self.selected -= 1;
                    self.refresh();
                    return;
                }
                Key::Escape => {
                    self.dismissed = self.line.clone();
                    self.completion = None;
                    self.refresh();
                    return;
                }
                _ => {}
            }
        }

        match key {
            Key::Up | Key::Down => {
                // The shell's own history navigation; suggestions for recalled
                // commands would capture the next arrow key.
                self.browsing_history = true;
                self.completion = None;
                self.refresh();
            }
            _ => self.browsing_history = false,
        }

        if bytes.iter().any(|b| matches!(b, b'\r' | b'\n')) {
            if let Some(line) = self.line.as_deref().filter(|line| !line.trim().is_empty()) {
                self.engine.record(line);
            }
            self.leave_prompt();
            self.refresh();
        } else if bytes.contains(&0x03) {
            self.leave_prompt();
            self.refresh();
        }
        self.write_shell(bytes);
    }

    fn on_resize(&mut self, cols: u16, rows: u16, master: &dyn MasterPty) {
        // Terminals re-wrap their contents on resize in ways the model does
        // not reproduce, so the real screen is redrawn from the model to
        // bring the two back in step.
        self.drawn = None;
        self.screen.resize(cols, rows);
        let _ = master.resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        });
        if !self.screen.alternate() && self.stream.is_idle() {
            let mut out = self.screen.repaint_all();
            self.sync_line();
            out.extend_from_slice(self.draw().as_bytes());
            self.write_terminal(&out);
        }
    }
}

/// Git Bash reports `/c/Users/me`; the file system wants `C:\Users\me`.
fn normalize_cwd(path: &str, shell: Shell) -> PathBuf {
    if cfg!(windows) && shell == Shell::Bash {
        let bytes = path.as_bytes();
        if bytes.len() >= 2
            && bytes[0] == b'/'
            && bytes[1].is_ascii_alphabetic()
            && (bytes.len() == 2 || bytes[2] == b'/')
        {
            let rest = path[2..].replace('/', "\\");
            return PathBuf::from(format!(
                "{}:{}",
                (bytes[1] as char).to_ascii_uppercase(),
                if rest.is_empty() { "\\" } else { &rest }
            ));
        }
    }
    PathBuf::from(path)
}

/// Starts `shell` under hiruzen and runs until it exits. Returns the shell's
/// exit code.
pub fn run(shell: Shell, login: bool) -> Result<i32, String> {
    let (cols, rows) = crossterm::terminal::size().map_err(|e| format!("cannot read the terminal size: {e}"))?;
    let launch = shell.launch(login)?;
    let scratch = launch.scratch.clone();

    let pair = native_pty_system()
        .openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| format!("cannot open a pseudo-terminal: {e}"))?;
    let mut command = CommandBuilder::new(&launch.program);
    command.args(&launch.args);
    for (key, value) in &launch.env {
        command.env(key, value);
    }
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    command.cwd(&cwd);
    let mut child = pair
        .slave
        .spawn_command(command)
        .map_err(|e| format!("cannot start {}: {e}", launch.program.display()))?;
    drop(pair.slave);
    let master = pair.master;
    let mut reader = master
        .try_clone_reader()
        .map_err(|e| format!("cannot read from the shell: {e}"))?;
    let writer = master
        .take_writer()
        .map_err(|e| format!("cannot write to the shell: {e}"))?;

    let raw = RawMode::enable()?;
    let (events, inbox): (Sender<Event>, Receiver<Event>) = mpsc::channel();

    // Shell output.
    let tx = events.clone();
    std::thread::spawn(move || {
        let mut buf = [0u8; 16 * 1024];
        loop {
            match reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if tx.send(Event::Output(buf[..n].to_vec())).is_err() {
                        break;
                    }
                }
            }
        }
    });

    // Keyboard input.
    let tx = events.clone();
    std::thread::spawn(move || {
        let mut stdin = std::io::stdin().lock();
        let mut buf = [0u8; 4096];
        loop {
            match stdin.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if tx.send(Event::Input(buf[..n].to_vec())).is_err() {
                        break;
                    }
                }
            }
        }
    });

    // Shell exit.
    let tx = events.clone();
    std::thread::spawn(move || {
        let code = child.wait().map(|status| status.exit_code() as i32).unwrap_or(1);
        let _ = tx.send(Event::Exit(code));
    });

    // Terminal size changes.
    let tx = events.clone();
    std::thread::spawn(move || {
        let mut last = (cols, rows);
        loop {
            std::thread::sleep(RESIZE_POLL);
            if let Ok(size) = crossterm::terminal::size() {
                if size != last {
                    last = size;
                    if tx.send(Event::Resize(size.0, size.1)).is_err() {
                        break;
                    }
                }
            }
        }
    });

    // Suggestions are computed off the main thread so typing never waits on
    // them. The engine also loads here, in parallel with the shell starting.
    let (requests, pending) = mpsc::channel::<CompletionRequest>();
    let (engine_ready, engine_slot) = mpsc::channel::<Arc<Engine>>();
    let tx = events.clone();
    std::thread::spawn(move || {
        let engine = Arc::new(Engine::system(shell));
        let _ = engine_ready.send(engine.clone());
        while let Ok(mut request) = pending.recv() {
            // Only the newest request matters.
            while let Ok(newer) = pending.try_recv() {
                request = newer;
            }
            let result = engine.complete(&request.line, &request.cwd);
            if tx
                .send(Event::Completed {
                    version: request.version,
                    result,
                })
                .is_err()
            {
                break;
            }
        }
    });
    let engine = engine_slot
        .recv()
        .map_err(|_| "the suggestion engine failed to start".to_string())?;

    let mut session = Session {
        shell,
        engine,
        screen: Screen::new(cols, rows),
        scanner: Scanner::default(),
        stream: Stream::default(),
        anchor: None,
        cwd,
        line: None,
        version: 0,
        completion: None,
        selected: 0,
        popup_col: 0,
        drawn: None,
        dismissed: None,
        browsing_history: false,
        requests,
        pty: writer,
    };

    // Start from an empty screen so the model and the terminal agree.
    session.write_terminal(b"\x1b[H\x1b[2J");

    let code = loop {
        match inbox.recv() {
            Ok(Event::Output(bytes)) => session.on_output(&bytes),
            Ok(Event::Input(bytes)) => session.on_input(&bytes),
            Ok(Event::Completed { version, result }) => session.on_completed(version, result),
            Ok(Event::Resize(cols, rows)) => session.on_resize(cols, rows, master.as_ref()),
            Ok(Event::Exit(code)) => {
                // Output the shell wrote just before exiting may still be queued.
                while let Ok(event) = inbox.recv_timeout(Duration::from_millis(30)) {
                    if let Event::Output(bytes) = event {
                        session.on_output(&bytes);
                    }
                }
                break code;
            }
            Err(_) => break 1,
        }
    };

    let erase = session.erase();
    session.write_terminal(erase.as_bytes());
    session.write_terminal(b"\x1b[0m\x1b[?25h");
    drop(raw);
    if let Some(dir) = scratch {
        let _ = std::fs::remove_dir_all(dir);
    }
    Ok(code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_single_keys_only() {
        assert_eq!(classify(b"\t"), Key::Tab);
        assert_eq!(classify(b"\x1b[A"), Key::Up);
        assert_eq!(classify(b"\x1bOB"), Key::Down);
        assert_eq!(classify(b"\x1b"), Key::Escape);
        assert_eq!(classify(b"a"), Key::Other);
        // Pasted text containing a tab is not a Tab key press.
        assert_eq!(classify(b"a\tb"), Key::Other);
    }
}
