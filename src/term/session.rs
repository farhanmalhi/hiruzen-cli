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
use std::time::{Duration, SystemTime};

use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize};

use crate::engine::{self, Completion, Engine, Options};
use crate::settings::{self, Bindings, Settings};
use crate::shell::Shell;
use crate::term::marks::{Chunk, Mark, Scanner};
use crate::term::popup::{self, Request, Style};
use crate::term::screen::{Anchor, Screen};
use crate::term::stream::Stream;

/// How often the terminal size is checked for a change.
const RESIZE_POLL: Duration = Duration::from_millis(120);
/// How long typing must pause before suggestions are fetched from the
/// network, so a word typed quickly is searched for once and not per letter.
const FETCH_DELAY: Duration = Duration::from_millis(150);

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
    remote: bool,
}

/// What a key press means while the dropdown is showing.
#[derive(Debug, PartialEq, Clone, Copy)]
enum Action {
    Accept,
    Next,
    Previous,
    Dismiss,
}

/// The dropdown action bound to exactly this input, if any. Input holding
/// more than one key, such as pasted text, is never an action.
fn action(bindings: &Bindings, input: &[u8]) -> Option<Action> {
    let bound = |keys: &[Vec<u8>]| keys.iter().any(|key| key == input);
    if bound(&bindings.accept) {
        Some(Action::Accept)
    } else if bound(&bindings.next) {
        Some(Action::Next)
    } else if bound(&bindings.previous) {
        Some(Action::Previous)
    } else if bound(&bindings.dismiss) {
        Some(Action::Dismiss)
    } else {
        None
    }
}

/// Runs one step of the session, reporting whether it completed. A failure
/// inside it is contained here.
fn guarded(step: impl FnOnce()) -> bool {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(step)).is_ok()
}

/// Sends failure reports to a file in the settings directory. Printed to the
/// terminal they would land in the middle of the user's shell.
fn install_crash_log() {
    std::panic::set_hook(Box::new(|info| {
        let Some(dir) = settings::config_dir() else { return };
        let _ = std::fs::create_dir_all(&dir);
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join("crash.log"))
        {
            let _ = writeln!(file, "hiruzen {}: {info}", env!("CARGO_PKG_VERSION"));
        }
    }));
}

/// True when shell output erases the whole screen.
fn clears_screen(data: &[u8]) -> bool {
    let has = |needle: &[u8]| data.windows(needle.len()).any(|window| window == needle);
    has(b"\x1b[2J") || has(b"\x1b[3J") || has(b"\x1b[H\x1b[J")
}

/// The arrow keys shells use to walk their history.
fn is_history_key(input: &[u8]) -> bool {
    matches!(input, b"\x1b[A" | b"\x1bOA" | b"\x1b[B" | b"\x1bOB")
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
    /// The shell has said its prompt is drawn, but where it ends is not yet
    /// known. The Windows pseudo-console hands on the markers ahead of the
    /// prompt's text, so the anchor is taken at the next key press instead.
    anchor_pending: bool,
    cwd: PathBuf,
    /// The prompt on screen belongs to a shell on another machine.
    remote: bool,
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
    settings: Settings,
    /// The settings file's modification time when it was last read.
    settings_stamp: Option<SystemTime>,
    /// The user has moved the highlight in the dropdown now showing.
    moved: bool,
    /// The cursor is inside a word.
    mid_word: bool,
    /// The command just sent to the shell and where it was typed, kept
    /// until the shell says how it ended.
    pending: Option<(String, Option<PathBuf>)>,
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
        if self.settings.disable
            || self.mid_word
            || self.line.is_none()
            || self.browsing_history
            || self.dismissed.is_some()
            || self.screen.alternate()
        {
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
            note: if completion.loading {
                Some(popup::LOADING)
            } else if completion.offline && completion.items.is_empty() {
                Some(popup::OFFLINE_EMPTY)
            } else if completion.offline {
                Some(popup::OFFLINE)
            } else {
                None
            },
            cols: cols.into(),
            rows: rows.into(),
            cursor_row: cursor_row.into(),
            command_row: command_row.into(),
            anchor_col: self.popup_col.into(),
            style: Style {
                theme: self.settings.theme,
                max_visible: self.settings.height,
                descriptions: self.settings.descriptions,
                accept_key: self.accept_key(),
            },
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

    /// After the screen is cleared the shell redraws the prompt and the line
    /// at the top without marking them again. The line is still known, so
    /// where it starts follows from where the cursor ended up.
    fn anchor_after_clear(&mut self) {
        let Some(anchor) = self.anchor else { return };
        let (cols, _) = self.screen.size();
        let (cursor_row, _) = self.screen.cursor();
        let typed = self.line.as_deref().map_or(0, unicode_width::UnicodeWidthStr::width);
        let rows_spanned = (usize::from(anchor.col) + typed) / usize::from(cols.max(1));
        let row = usize::from(cursor_row).saturating_sub(rows_spanned) as u16;
        self.anchor = Some(self.screen.anchor_at(row, anchor.col));
    }

    /// Hands the command waiting for its exit status to the engine.
    fn record_pending(&mut self, status: Option<i32>) {
        if let Some((command, cwd)) = self.pending.take() {
            self.engine.record(&command, cwd.as_deref(), status);
        }
    }

    /// Reads the settings file and puts its values into effect.
    fn apply_settings(&mut self) {
        self.settings_stamp = settings::modified();
        self.settings = Settings::load();
        self.engine.set_options(Options {
            fuzzy: self.settings.fuzzy,
            history: self.settings.history,
            disabled_commands: self.settings.disabled_commands.clone(),
            online: self.settings.online,
            ..Default::default()
        });
        if self.settings.disable {
            self.completion = None;
        }
    }

    /// The symbol for the key that would insert the highlighted suggestion
    /// right now. Enter does so only once the highlight has been moved;
    /// until then it runs the command, and Tab is the key to show.
    fn accept_key(&self) -> Option<&'static str> {
        let bound = |key: &[u8]| self.settings.keys.accept.iter().any(|sequence| sequence == key);
        if (self.settings.enter_accepts && self.moved) || bound(b"\r") {
            Some("↵")
        } else if bound(b"\t") {
            Some("⇥")
        } else {
            None
        }
    }

    /// Types the highlighted suggestion into the shell.
    fn accept(&mut self) {
        let edit = self
            .completion
            .as_ref()
            .and_then(|completion| engine::edit_for(&completion.token, completion.items.get(self.selected)?));
        // What was taken, and what was listed above it and passed over.
        if let (Some(completion), Some(line)) = (&self.completion, &self.line) {
            if let Some(picked) = completion.items.get(self.selected) {
                let passed: Vec<&str> = completion.items[..self.selected.min(3)]
                    .iter()
                    .map(|c| c.name.as_str())
                    .collect();
                let cwd = (!self.remote).then_some(self.cwd.as_path());
                self.engine.note_pick(line, cwd, &picked.name, &passed);
            }
        }
        if let Some(edit) = edit {
            let mut keys = "\x7f".repeat(edit.backspaces);
            keys.push_str(&edit.text);
            keys.push_str(&"\x1b[D".repeat(edit.cursor_back));
            self.write_shell(keys.as_bytes());
        }
    }

    fn selectable(&self) -> bool {
        self.drawn.is_some() && self.completion.as_ref().is_some_and(|c| !c.items.is_empty())
    }

    /// Forgets the prompt: a command is running, or the line was abandoned.
    fn leave_prompt(&mut self) {
        self.anchor = None;
        self.anchor_pending = false;
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
        // Inside a word, inserting a suggestion would run into the letters
        // after the cursor, so none is offered there.
        let mid_word = line.is_some() && self.screen.cursor_on_text();
        if line == self.line && mid_word == self.mid_word {
            return;
        }
        self.mid_word = mid_word;
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
            Some(line)
                if !line.trim().is_empty()
                    && self.dismissed.is_none()
                    && !self.browsing_history
                    && !self.mid_word
                    && !self.settings.disable =>
            {
                let _ = self.requests.send(CompletionRequest {
                    version: self.version,
                    line: line.clone(),
                    cwd: self.cwd.clone(),
                    remote: self.remote,
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
                Chunk::Data(mut data) => {
                    if cfg!(windows) {
                        remove_win32_input_request(&mut data);
                    }
                    let cleared = self.anchor.is_some() && clears_screen(&data);
                    self.screen.feed(&data);
                    if cleared {
                        self.anchor_after_clear();
                    }
                    self.stream.feed(&data);
                    out.extend_from_slice(&data);
                }
                Chunk::Mark(Mark::PromptStart) => {
                    self.anchor = None;
                    self.anchor_pending = false;
                }
                Chunk::Mark(Mark::Alias(name, value)) => self.engine.add_alias(name, value),
                Chunk::Mark(Mark::Names(names)) => self.engine.add_functions(names),
                Chunk::Mark(Mark::Exit(status)) => {
                    self.record_pending(Some(status));
                }
                Chunk::Mark(Mark::PromptEnd) => {
                    // A shell that does not report exit statuses.
                    self.record_pending(None);
                    if cfg!(windows) {
                        self.anchor_pending = true;
                    } else {
                        self.anchor = Some(self.screen.anchor());
                    }
                    self.dismissed = None;
                    self.browsing_history = false;
                    // A new prompt is a cheap, natural moment to notice edits
                    // to the settings file.
                    if settings::modified() != self.settings_stamp {
                        self.apply_settings();
                    }
                }
                Chunk::Mark(Mark::RemoteCwd(path)) => {
                    self.cwd = PathBuf::from(path);
                    self.remote = true;
                }
                Chunk::Mark(Mark::Cwd(path)) => {
                    self.remote = false;
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

    fn on_completed(&mut self, version: u64, mut result: Option<Completion>) {
        if version != self.version {
            return;
        }
        if let (Some(completion), Some(previous)) = (result.as_mut(), self.completion.as_ref()) {
            completion.carry_over(previous);
        }
        let (_, cursor_col) = self.screen.cursor();
        self.popup_col = result
            .as_ref()
            .map(|completion| cursor_col.saturating_sub(completion.token.width as u16))
            .unwrap_or(cursor_col);
        self.completion = result;
        self.selected = 0;
        self.moved = false;
        self.refresh();
    }

    fn on_input(&mut self, bytes: &[u8]) {
        // For testing that a failure hands the shell back: with this
        // variable set, a marker in the input makes hiruzen fail on purpose.
        if std::env::var_os("HIRUZEN_TEST_PANIC").is_some() && bytes == b"\x1b[99~" {
            panic!("requested by HIRUZEN_TEST_PANIC");
        }
        if self.anchor_pending {
            // By the first key press the prompt is on screen, and the
            // cursor is where the command will start.
            self.anchor_pending = false;
            self.anchor = Some(self.screen.anchor());
        }
        if self.selectable() {
            let count = self.completion.as_ref().map_or(0, |c| c.items.len());
            match action(&self.settings.keys, bytes) {
                Some(Action::Accept) => {
                    self.accept();
                    return;
                }
                Some(Action::Next) => {
                    self.selected = (self.selected + 1) % count;
                    self.moved = true;
                    self.refresh();
                    return;
                }
                // At the top of the list the key goes to the shell, so Up
                // still reaches shell history.
                Some(Action::Previous) if self.selected > 0 => {
                    self.selected -= 1;
                    self.moved = true;
                    self.refresh();
                    return;
                }
                Some(Action::Dismiss) => {
                    self.dismissed = self.line.clone();
                    self.completion = None;
                    self.refresh();
                    return;
                }
                _ if self.settings.enter_accepts && self.moved && matches!(bytes, b"\r" | b"\n") => {
                    self.accept();
                    return;
                }
                _ => {}
            }
        }

        if is_history_key(bytes) {
            // The shell's own history navigation; suggestions for recalled
            // commands would capture the next arrow key.
            self.browsing_history = true;
            self.completion = None;
            self.refresh();
        } else {
            self.browsing_history = false;
        }

        if bytes.iter().any(|b| matches!(b, b'\r' | b'\n')) {
            if let Some(line) = self.line.clone().filter(|line| !line.trim().is_empty()) {
                // Recorded once the shell reports how the command ended.
                self.record_pending(None);
                let cwd = (!self.remote).then(|| self.cwd.clone());
                self.pending = Some((line, cwd));
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

/// Removes the Windows pseudo-console's request for win32 input mode.
///
/// Passed on, it makes the terminal send every key as `ESC [ ... _` with its
/// key codes, which the key bindings would not recognise: Tab, the arrows,
/// Escape and Enter would all go by unseen. Without it keys arrive as the
/// ordinary sequences, which the pseudo-console accepts just as well.
fn remove_win32_input_request(data: &mut Vec<u8>) {
    const REQUEST: &[u8] = b"\x1b[?9001h";
    while let Some(at) = data.windows(REQUEST.len()).position(|window| window == REQUEST) {
        data.drain(at..at + REQUEST.len());
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
pub fn run(shell: Shell, login: bool, notice: Option<&str>) -> Result<i32, String> {
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
        let mut next = pending.recv().ok();
        while let Some(mut request) = next.take() {
            // Only the newest request matters.
            while let Ok(newer) = pending.try_recv() {
                request = newer;
            }
            let version = request.version;
            // What is known on this machine is shown at once. Anything a
            // spec fetches from the network follows when it arrives.
            let result = engine.complete_quick(&request.line, &request.cwd, request.remote);
            let loading = result.as_ref().is_some_and(|completion| completion.loading);
            if tx.send(Event::Completed { version, result }).is_err() {
                break;
            }
            if loading {
                match pending.recv_timeout(FETCH_DELAY) {
                    Ok(newer) => {
                        next = Some(newer);
                        continue;
                    }
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                }
                let result = engine.complete_at(&request.line, &request.cwd, request.remote);
                if tx.send(Event::Completed { version, result }).is_err() {
                    break;
                }
            }
            next = pending.recv().ok();
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
        anchor_pending: false,
        cwd,
        remote: false,
        line: None,
        version: 0,
        completion: None,
        selected: 0,
        popup_col: 0,
        drawn: None,
        dismissed: None,
        browsing_history: false,
        settings: Settings::default(),
        settings_stamp: None,
        moved: false,
        mid_word: false,
        pending: None,
        requests,
        pty: writer,
    };

    session.apply_settings();

    // Start from an empty screen so the model and the terminal agree.
    session.write_terminal(b"\x1b[H\x1b[2J");
    if let Some(notice) = notice {
        let line = format!("{notice}\r\n");
        session.write_terminal(line.as_bytes());
        session.screen.feed(line.as_bytes());
    }

    // If hiruzen's own code fails, the user must not lose their shell. Each
    // event is handled under a guard; after a failure hiruzen stops drawing
    // suggestions and only carries bytes between the terminal and the shell.
    install_crash_log();
    let mut handed_back = false;
    let code = loop {
        let event = match inbox.recv() {
            Ok(event) => event,
            Err(_) => break 1,
        };
        if let Event::Exit(code) = event {
            // Output the shell wrote just before exiting may still be queued.
            while let Ok(event) = inbox.recv_timeout(Duration::from_millis(30)) {
                if let Event::Output(bytes) = event {
                    if handed_back || !guarded(|| session.on_output(&bytes)) {
                        handed_back = true;
                        session.write_terminal(&bytes);
                    }
                }
            }
            break code;
        }
        if handed_back {
            match event {
                Event::Output(bytes) => session.write_terminal(&bytes),
                Event::Input(bytes) => session.write_shell(&bytes),
                Event::Resize(cols, rows) => {
                    let _ = master.resize(PtySize {
                        rows,
                        cols,
                        pixel_width: 0,
                        pixel_height: 0,
                    });
                }
                Event::Completed { .. } | Event::Exit(_) => {}
            }
            continue;
        }
        let survived = match &event {
            Event::Output(bytes) => guarded(|| session.on_output(bytes)),
            Event::Input(bytes) => guarded(|| session.on_input(bytes)),
            Event::Completed { version, result } => guarded(|| session.on_completed(*version, result.clone())),
            Event::Resize(cols, rows) => guarded(|| session.on_resize(*cols, *rows, master.as_ref())),
            Event::Exit(_) => true,
        };
        if !survived {
            handed_back = true;
            // Whatever was being handled still has to reach its destination.
            match event {
                Event::Output(bytes) => session.write_terminal(&bytes),
                Event::Input(bytes) => session.write_shell(&bytes),
                _ => {}
            }
            session.write_terminal(b"\x1b[0m\x1b[?25h");
        }
    };

    session.record_pending(None);
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
    fn maps_single_keys_to_actions() {
        let keys = Settings::default().keys;
        assert_eq!(action(&keys, b"\t"), Some(Action::Accept));
        assert_eq!(action(&keys, b"\x1b[A"), Some(Action::Previous));
        assert_eq!(action(&keys, b"\x1bOB"), Some(Action::Next));
        assert_eq!(action(&keys, b"\x1b"), Some(Action::Dismiss));
        assert_eq!(action(&keys, b"a"), None);
        // Pasted text containing a tab is not a Tab key press.
        assert_eq!(action(&keys, b"a\tb"), None);
        assert!(is_history_key(b"\x1b[B") && !is_history_key(b"\t"));
    }

    #[test]
    fn drops_the_request_for_win32_input_mode() {
        let mut data = b"\x1b[6n\x1b[?9001h\x1b[?1004hPS C:\\> ".to_vec();
        remove_win32_input_request(&mut data);
        assert_eq!(data, b"\x1b[6n\x1b[?1004hPS C:\\> ");
    }
}
