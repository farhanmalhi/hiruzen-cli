//! The management commands: settings, themes, shell integrations, the
//! doctor, and issue reporting.

use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

use crate::settings::{self, Settings, ValueKind, DEFINITIONS, THEME_NAMES};
use crate::shell::{self, Shell};
use crate::specs::SpecStore;

const REPOSITORY: &str = env!("CARGO_PKG_REPOSITORY");
const BLOCK_START: &str = "# >>> hiruzen >>>";
const BLOCK_END: &str = "# <<< hiruzen <<<";

// --- settings ------------------------------------------------------------------

fn show(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Array(items) => items.iter().filter_map(|i| i.as_str()).collect::<Vec<_>>().join(","),
        other => other.to_string(),
    }
}

fn unknown_setting(key: &str) -> String {
    format!("unknown setting '{key}'; run `hiruzen settings` to list them")
}

/// `hiruzen settings [KEY [VALUE]]`, plus `all`, `open`, `docs` and `--delete KEY`.
pub fn settings(args: &[String]) -> Result<i32, String> {
    let words: Vec<&str> = args.iter().map(String::as_str).collect();
    match words.as_slice() {
        [] | ["all"] => {
            let stored = settings::read_raw()?;
            for definition in DEFINITIONS {
                let current = stored
                    .iter()
                    .find(|(key, _)| key.eq_ignore_ascii_case(definition.key))
                    .map(|(_, value)| show(value));
                match current {
                    Some(value) => println!("{} = {}", definition.key, value),
                    None => println!("{} = {}  (default)", definition.key, definition.default),
                }
            }
            Ok(0)
        }
        ["docs"] => {
            for definition in DEFINITIONS {
                let accepts = match definition.kind {
                    ValueKind::Bool => "true or false".to_string(),
                    ValueKind::Int(min, max) => format!("a whole number from {min} to {max}"),
                    ValueKind::Text([]) => "any text".to_string(),
                    ValueKind::Text(allowed) => allowed.join(", "),
                    ValueKind::List => "a comma-separated list".to_string(),
                    ValueKind::Keys => "key names separated by commas, such as tab,ctrl+y".to_string(),
                };
                let default = if definition.default.is_empty() {
                    "none"
                } else {
                    definition.default
                };
                println!(
                    "{}\n    {}\n    Accepts: {accepts}. Default: {default}.\n",
                    definition.key, definition.help
                );
            }
            Ok(0)
        }
        ["open"] => {
            let path = settings::settings_file().ok_or("cannot work out where settings are kept")?;
            if !path.exists() {
                settings::write_raw(&settings::read_raw()?)?;
            }
            println!("{}", path.display());
            open_in_default_app(&path.to_string_lossy());
            Ok(0)
        }
        ["--delete" | "-d", key] => {
            let definition = settings::definition(key).ok_or_else(|| unknown_setting(key))?;
            let mut stored = settings::read_raw()?;
            stored.retain(|stored_key, _| !stored_key.eq_ignore_ascii_case(definition.key));
            settings::write_raw(&stored)?;
            println!("{} = {}  (default)", definition.key, definition.default);
            Ok(0)
        }
        [key] => {
            let definition = settings::definition(key).ok_or_else(|| unknown_setting(key))?;
            let stored = settings::read_raw()?;
            let current = stored
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(definition.key))
                .map(|(_, v)| show(v));
            println!("{}", current.unwrap_or_else(|| definition.default.to_string()));
            Ok(0)
        }
        [key, value @ ..] => {
            let definition = settings::definition(key).ok_or_else(|| unknown_setting(key))?;
            let value = settings::parse_value(definition, &value.join(" "))?;
            let mut stored = settings::read_raw()?;
            stored.retain(|stored_key, _| !stored_key.eq_ignore_ascii_case(definition.key));
            println!("{} = {}", definition.key, show(&value));
            stored.insert(definition.key.to_string(), value);
            settings::write_raw(&stored)?;
            Ok(0)
        }
    }
}

/// `hiruzen theme [NAME | --list]`.
pub fn theme(args: &[String]) -> Result<i32, String> {
    match args.first().map(String::as_str) {
        None => {
            println!("{}", Settings::load().theme.name);
            Ok(0)
        }
        Some("--list" | "-l" | "list") => {
            let current = Settings::load().theme.name;
            for name in THEME_NAMES {
                let theme = settings::theme(name);
                // A sample row, so the colours can be compared before choosing.
                println!(
                    "{} {:<10} {}> checkout   {}  Switch branches \x1b[0m {}> commit     {}  Record changes \x1b[0m",
                    if *name == current { "*" } else { " " },
                    name,
                    theme.selected,
                    theme.selected_description,
                    theme.row,
                    theme.description,
                );
            }
            Ok(0)
        }
        Some(name) => settings(&["autocomplete.theme".to_string(), name.to_string()]),
    }
}

// --- shell integrations ------------------------------------------------------

/// Asks PowerShell where its profile lives; it differs by edition and platform.
fn powershell_profile(shell: Shell) -> Option<PathBuf> {
    let program = shell::find_on_path(shell.name())?;
    let output = Command::new(program)
        .args(["-NoLogo", "-NoProfile", "-Command", "$PROFILE.CurrentUserAllHosts"])
        .output()
        .ok()?;
    let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!path.is_empty()).then(|| PathBuf::from(path))
}

/// The startup file hiruzen's line goes in for each shell.
fn startup_file(shell: Shell) -> Option<PathBuf> {
    let home = shell::home_dir()?;
    match shell {
        Shell::Zsh => Some(
            std::env::var_os("HIRUZEN_USER_ZDOTDIR")
                .or_else(|| std::env::var_os("ZDOTDIR"))
                .map(PathBuf::from)
                .filter(|dir| !dir.as_os_str().is_empty())
                .unwrap_or(home)
                .join(".zshrc"),
        ),
        Shell::Bash => Some(home.join(".bashrc")),
        // fish reads every file in conf.d, so hiruzen gets one of its own.
        Shell::Fish => Some(
            std::env::var_os("XDG_CONFIG_HOME")
                .map(PathBuf::from)
                .filter(|dir| !dir.as_os_str().is_empty())
                .unwrap_or_else(|| home.join(".config"))
                .join("fish")
                .join("conf.d")
                .join("hiruzen.fish"),
        ),
        Shell::Pwsh | Shell::Powershell => powershell_profile(shell),
        Shell::Cmd => None,
    }
}

fn init_line(shell: Shell) -> &'static str {
    match shell {
        Shell::Zsh => "eval \"$(hiruzen init zsh)\"",
        Shell::Bash => "eval \"$(hiruzen init bash)\"",
        Shell::Fish => "hiruzen init fish | source",
        Shell::Pwsh => "hiruzen init pwsh | Out-String | Invoke-Expression",
        Shell::Powershell => "hiruzen init powershell | Out-String | Invoke-Expression",
        Shell::Cmd => "",
    }
}

#[derive(Debug, PartialEq)]
enum Installed {
    /// hiruzen's own marked block is present.
    Block,
    /// A `hiruzen init` line the user added by hand.
    Manual,
    No,
}

fn installed_in(content: &str) -> Installed {
    if content.contains(BLOCK_START) {
        Installed::Block
    } else if content
        .lines()
        .any(|line| !line.trim_start().starts_with('#') && line.contains("hiruzen init"))
    {
        Installed::Manual
    } else {
        Installed::No
    }
}

/// Removes hiruzen's marked block, and the blank line it was given above it.
fn without_block(content: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    let mut inside = false;
    for line in content.lines() {
        if line.trim() == BLOCK_START {
            inside = true;
            if out.last().is_some_and(|last| last.trim().is_empty()) {
                out.pop();
            }
        } else if line.trim() == BLOCK_END {
            inside = false;
        } else if !inside {
            out.push(line);
        }
    }
    let mut text = out.join("\n");
    if !text.is_empty() {
        text.push('\n');
    }
    text
}

fn with_block(content: &str, shell: Shell) -> String {
    let mut text = content.to_string();
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    if !text.is_empty() {
        text.push('\n');
    }
    text.push_str(&format!("{BLOCK_START}\n{}\n{BLOCK_END}\n", init_line(shell)));
    text
}

/// The shells to act on: those named, or every supported shell found on this machine.
fn target_shells(names: &[String]) -> Result<Vec<Shell>, String> {
    if names.is_empty() {
        return Ok(Shell::ALL
            .into_iter()
            .filter(|shell| *shell != Shell::Cmd && shell::find_on_path(shell.name()).is_some())
            .collect());
    }
    names
        .iter()
        .map(|name| Shell::from_name(name).ok_or_else(|| format!("unknown shell '{name}'")))
        .collect()
}

/// `hiruzen integrations <install|uninstall|status> [SHELL...]`.
pub fn integrations(args: &[String]) -> Result<i32, String> {
    let (action, names) = match args.split_first() {
        Some((action, names)) => (action.as_str(), names),
        None => ("status", &[][..]),
    };
    if !matches!(action, "install" | "uninstall" | "status") {
        return Err("usage: hiruzen integrations <install|uninstall|status> [SHELL...]".to_string());
    }
    let shells = target_shells(names)?;
    if shells.is_empty() {
        return Err("no supported shell was found on this machine".to_string());
    }
    let mut failed = false;
    for shell in shells {
        let name = shell.name();
        let Some(path) = startup_file(shell) else {
            println!("{name}: no startup file to use; start it with `hiruzen --shell {name}`");
            continue;
        };
        let content = std::fs::read_to_string(&path).unwrap_or_default();
        let state = installed_in(&content);
        let result = match (action, &state) {
            ("status", Installed::Block) => Ok(format!("installed in {}", path.display())),
            ("status", Installed::Manual) => Ok(format!("installed by hand in {}", path.display())),
            ("status", Installed::No) => Ok("not installed".to_string()),
            ("install", Installed::No) => write_startup_file(&path, &with_block(&content, shell))
                .map(|()| format!("installed in {}; open a new terminal to use it", path.display())),
            ("install", _) => Ok(format!("already installed in {}", path.display())),
            ("uninstall", Installed::Block) => {
                let remaining = without_block(&content);
                let outcome = if remaining.is_empty() && shell == Shell::Fish {
                    // The file was hiruzen's own.
                    std::fs::remove_file(&path).map_err(|e| format!("cannot remove {}: {e}", path.display()))
                } else {
                    write_startup_file(&path, &remaining)
                };
                outcome.map(|()| format!("removed from {}", path.display()))
            }
            ("uninstall", Installed::Manual) => Ok(format!(
                "has a `hiruzen init` line you added by hand in {}; remove that line yourself",
                path.display()
            )),
            (_, _) => Ok("not installed".to_string()),
        };
        match result {
            Ok(message) => println!("{name}: {message}"),
            Err(message) => {
                failed = true;
                println!("{name}: {message}");
            }
        }
    }
    Ok(if failed { 1 } else { 0 })
}

fn write_startup_file(path: &Path, content: &str) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    }
    std::fs::write(path, content).map_err(|e| format!("cannot write {}: {e}", path.display()))
}

// --- doctor ------------------------------------------------------------------

struct Report {
    problems: usize,
}

impl Report {
    fn ok(&mut self, what: &str, detail: &str) {
        println!("  ok    {what}: {detail}");
    }
    fn note(&mut self, what: &str, detail: &str) {
        println!("  note  {what}: {detail}");
    }
    fn problem(&mut self, what: &str, detail: &str, fix: &str) {
        self.problems += 1;
        println!("  FAIL  {what}: {detail}\n        Fix: {fix}");
    }
}

/// `hiruzen doctor`: checks that everything hiruzen needs is in place.
pub fn doctor() -> Result<i32, String> {
    let mut report = Report { problems: 0 };
    println!(
        "hiruzen {} on {} {}",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH
    );

    println!("\nInstallation");
    let exe = std::env::current_exe().ok();
    match shell::find_on_path("hiruzen") {
        Some(found) => report.ok("hiruzen on PATH", &found.display().to_string()),
        None => report.problem(
            "hiruzen on PATH",
            "not found, so the line in your shell's startup file cannot start it",
            &format!(
                "add {} to PATH",
                exe.as_deref()
                    .and_then(Path::parent)
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| "its directory".into())
            ),
        ),
    }
    let store = SpecStore::embedded();
    let commands = store.index().len();
    if commands > 0 && store.load("git").is_some() {
        report.ok("completion specs", &format!("{commands} commands"));
    } else {
        report.problem(
            "completion specs",
            "the embedded spec archive cannot be read",
            "reinstall hiruzen",
        );
    }
    match crate::engine::self_test() {
        true => report.ok("spec code engine", "runs"),
        false => report.problem(
            "spec code engine",
            "the JavaScript engine did not run",
            "suggestions computed by spec code are unavailable; report this with `hiruzen issue`",
        ),
    }

    println!("\nSettings");
    match (settings::settings_file(), settings::read_raw()) {
        (Some(path), Ok(stored)) => {
            let unknown: Vec<&String> = stored
                .keys()
                .filter(|key| settings::definition(key).is_none())
                .collect();
            if !path.exists() {
                report.ok(
                    "settings file",
                    &format!("{} (not created yet; defaults in use)", path.display()),
                );
            } else if unknown.is_empty() {
                report.ok("settings file", &path.display().to_string());
            } else {
                report.note(
                    "settings file",
                    &format!(
                        "{} has unknown settings, which are ignored: {unknown:?}",
                        path.display()
                    ),
                );
            }
            if Settings::from_map(&stored).disable {
                report.note("suggestions", "switched off by autocomplete.disable");
            }
        }
        (_, Err(error)) => report.problem(
            "settings file",
            &error,
            "correct the file, or delete it to return to the defaults",
        ),
        (None, _) => report.problem(
            "settings file",
            "no home directory found",
            "set HOME, or HIRUZEN_CONFIG_DIR",
        ),
    }

    println!("\nShells");
    let default = Shell::detect();
    for shell in Shell::ALL {
        let name = shell.name();
        if shell::find_on_path(name).is_none() {
            if shell == default {
                report.problem(
                    name,
                    "your default shell is not on PATH",
                    "install it, or start hiruzen with --shell",
                );
            }
            continue;
        }
        let Some(path) = startup_file(shell) else {
            report.note(name, &format!("found; start it with `hiruzen --shell {name}`"));
            continue;
        };
        match installed_in(&std::fs::read_to_string(&path).unwrap_or_default()) {
            Installed::No => report.note(
                name,
                &format!("found, not set to start hiruzen; run `hiruzen install {name}`"),
            ),
            _ => report.ok(name, &format!("starts hiruzen from {}", path.display())),
        }
    }
    match default.history_file() {
        Some(path) if path.is_file() => report.ok("shell history", &path.display().to_string()),
        Some(path) => report.note(
            "shell history",
            &format!("{} does not exist yet, so ranking starts without it", path.display()),
        ),
        None => report.note("shell history", "this shell keeps none that hiruzen can read"),
    }

    println!("\nThis terminal");
    if std::env::var_os("HIRUZEN").is_some() {
        report.ok("session", "running inside hiruzen");
    } else {
        report.note(
            "session",
            "not inside hiruzen; run `hiruzen`, or open a new terminal once installed",
        );
    }
    if std::io::stdin().is_terminal() && std::io::stdout().is_terminal() {
        match crossterm::terminal::size() {
            Ok((cols, rows)) if cols >= 20 && rows >= 3 => report.ok("size", &format!("{cols} x {rows}")),
            Ok((cols, rows)) => report.problem(
                "size",
                &format!("{cols} x {rows} is too small for the dropdown"),
                "enlarge the window",
            ),
            Err(error) => report.problem("size", &error.to_string(), "run hiruzen in a terminal window"),
        }
    } else {
        report.note(
            "terminal",
            "input or output is redirected, so the terminal itself was not checked",
        );
    }
    match std::env::var("TERM") {
        Ok(term) if term == "dumb" => report.problem(
            "TERM",
            "dumb",
            "use a terminal that supports colours and cursor movement",
        ),
        Ok(term) => report.ok("TERM", &term),
        Err(_) if cfg!(windows) => {}
        Err(_) => report.note("TERM", "not set"),
    }

    println!();
    if report.problems == 0 {
        println!("No problems found.");
        Ok(0)
    } else {
        println!("{} problem(s) found.", report.problems);
        Ok(1)
    }
}

// --- issue ---------------------------------------------------------------------

fn percent_encode(text: &str) -> String {
    let mut out = String::new();
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(byte as char),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

fn open_in_default_app(target: &str) -> bool {
    let mut command = if cfg!(target_os = "macos") {
        Command::new("open")
    } else if cfg!(windows) {
        let mut command = Command::new("cmd");
        command.args(["/C", "start", ""]);
        command
    } else {
        Command::new("xdg-open")
    };
    command
        .arg(target)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

/// `hiruzen issue [TITLE...]`: opens a new GitHub issue with this machine's
/// details filled in. Nothing is sent until the user submits the form.
pub fn issue(args: &[String]) -> Result<i32, String> {
    let title = args.join(" ");
    let shell = Shell::detect();
    let body = format!(
        "### What happened\n\n\n### What you expected\n\n\n### Details\n\n- hiruzen: {}\n- OS: {} {}\n- Shell: {}\n- TERM: {}\n- Inside a hiruzen session: {}\n",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH,
        shell.name(),
        std::env::var("TERM").unwrap_or_else(|_| "not set".to_string()),
        if std::env::var_os("HIRUZEN").is_some() { "yes" } else { "no" },
    );
    let url = format!(
        "{REPOSITORY}/issues/new?title={}&body={}",
        percent_encode(&title),
        percent_encode(&body)
    );
    if !open_in_default_app(&url) {
        println!("Open this link to file the issue:");
    }
    println!("{url}");
    Ok(0)
}

// --- evaluate ------------------------------------------------------------------

/// Answers no commands, so an evaluation measures specs and history alone and
/// runs nothing on the machine.
struct NoCommands;

impl crate::engine::generators::Runner for NoCommands {
    fn run(&self, _argv: &[String], _cwd: &Path) -> Option<String> {
        None
    }
}

#[derive(Default)]
struct Tally {
    cases: usize,
    top1: usize,
    top3: usize,
    top8: usize,
    /// Cases where the right word appeared anywhere in the suggestions.
    offered: usize,
}

impl Tally {
    fn add(&mut self, rank: Option<usize>) {
        self.cases += 1;
        if let Some(rank) = rank {
            self.offered += 1;
            self.top1 += usize::from(rank < 1);
            self.top3 += usize::from(rank < 3);
            self.top8 += usize::from(rank < 8);
        }
    }

    /// The same counts restricted to the cases where the right word was
    /// offered at all: how good the ordering is, apart from what is knowable.
    fn ranking_line(&self, label: &str) -> String {
        let percent = |n: usize| 100.0 * n as f64 / self.offered.max(1) as f64;
        format!(
            "{label:<44} {:>6} cases   first {:>5.1}%   top 3 {:>5.1}%   top 8 {:>5.1}%",
            self.offered,
            percent(self.top1),
            percent(self.top3),
            percent(self.top8)
        )
    }

    fn line(&self, label: &str) -> String {
        let percent = |n: usize| 100.0 * n as f64 / self.cases.max(1) as f64;
        format!(
            "{label:<44} {:>6} cases   first {:>5.1}%   top 3 {:>5.1}%   top 8 {:>5.1}%",
            self.cases,
            percent(self.top1),
            percent(self.top3),
            percent(self.top8)
        )
    }
}

/// `hiruzen evaluate [--boost N]`: measures suggestion quality against your
/// own shell history.
///
/// The older four fifths of the history are what hiruzen learns from. Each
/// command in the newest fifth is then replayed word by word, and the word
/// you really typed next is looked up in the suggestions. Only totals are
/// printed, never the commands.
pub fn evaluate(args: &[String]) -> Result<i32, String> {
    use crate::engine::generators::Generators;
    use crate::engine::history::History;
    use crate::engine::{tokenizer, Engine, Options};

    let mut options = Options::default();
    // A file of commands, one per line and oldest first, to use in place of
    // the shell's history: another person's, or a research data set.
    let mut file: Option<PathBuf> = None;
    for pair in args.chunks(2) {
        let [flag, value] = pair else {
            return Err("usage: hiruzen evaluate [--file PATH] [--boost N] [--sequence N]".to_string());
        };
        if flag == "--file" {
            file = Some(PathBuf::from(value));
            continue;
        }
        let number: f64 = value.parse().map_err(|_| format!("{flag} takes a number"))?;
        match flag.as_str() {
            "--boost" => options.history_boost = number,
            "--sequence" => options.sequence_weight = number,
            _ => return Err("usage: hiruzen evaluate [--file PATH] [--boost N] [--sequence N]".to_string()),
        }
    }
    let shell = Shell::detect();
    let commands: Vec<String> = match &file {
        Some(path) => std::fs::read_to_string(path)
            .map_err(|e| format!("cannot read {}: {e}", path.display()))?
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_string)
            .collect(),
        None => History::commands(shell),
    };
    if commands.len() < 200 {
        return Err(format!(
            "{} history holds {} commands; at least 200 are needed for a meaningful result",
            shell.name(),
            commands.len()
        ));
    }
    let split = commands.len() * 4 / 5;
    let (learn, test) = commands.split_at(split);
    let engine = Engine::new(
        shell,
        History::from_commands(learn, shell),
        Generators::new(Box::new(NoCommands)),
    );
    engine.set_options(options);
    let cwd = std::env::current_dir().map_err(|e| e.to_string())?;

    let timings = std::cell::RefCell::new(Vec::<std::time::Duration>::new());
    let rank_of = |line: &str, target: &str| -> Option<usize> {
        let started = std::time::Instant::now();
        let completion = engine.complete(line, &cwd);
        timings.borrow_mut().push(started.elapsed());
        let completion = completion?;
        completion
            .items
            .iter()
            .position(|item| item.name == target || item.aliases.iter().any(|alias| alias == target))
    };
    let (mut name_typed, mut next_blank, mut next_typed) = (Tally::default(), Tally::default(), Tally::default());
    // Why the next word was not among the top eight, with two letters typed.
    let mut misses: std::collections::BTreeMap<&str, usize> = std::collections::BTreeMap::new();
    for command in test {
        let parsed = tokenizer::parse(command.trim(), shell);
        let mut words: Vec<String> = parsed.done.into_iter().map(|t| t.text).collect();
        words.extend(Some(parsed.active.text).filter(|t| !t.is_empty()));
        // Words that needed quoting cannot be rebuilt into a line reliably.
        if words.is_empty()
            || words
                .iter()
                .any(|w| w.chars().any(|c| c.is_whitespace() || matches!(c, '\'' | '"' | '\\')))
        {
            continue;
        }
        let typed = |word: &str| word.chars().take(2).collect::<String>();
        if words[0].chars().count() > 2 {
            name_typed.add(rank_of(&typed(&words[0]), &words[0]));
        }
        for i in 1..words.len().min(5) {
            let before = words[..i].join(" ");
            next_blank.add(rank_of(&format!("{before} "), &words[i]));
            if words[i].chars().count() > 2 {
                let rank = rank_of(&format!("{before} {}", typed(&words[i])), &words[i]);
                next_typed.add(rank);
                if rank.is_none_or(|rank| rank >= 8) {
                    let word = words[i].as_str();
                    let reason = if rank.is_some() {
                        "offered, but below the top eight"
                    } else if word.starts_with('-') {
                        "an option the spec does not list"
                    } else if word.contains('/') || word.contains('.') || cwd.join(word).exists() {
                        "a path, file name, URL or version"
                    } else if word.chars().any(|c| c.is_ascii_digit()) {
                        "a value containing digits (an id, port, hash or number)"
                    } else {
                        "free text or a name never typed here before"
                    };
                    *misses.entry(reason).or_default() += 1;
                }
            }
        }
        // As in real use, each command is learned from once it has been run,
        // and becomes the "previous command" for the next one.
        engine.record(command, None, None);
    }
    println!(
        "Learned from {} commands, tested on {} (history boost {}).",
        learn.len(),
        test.len(),
        engine_boost(&engine)
    );
    let mut times = timings.into_inner();
    times.sort();
    if !times.is_empty() {
        let at = |fraction: f64| times[((times.len() - 1) as f64 * fraction) as usize];
        println!(
            "Time to compute suggestions: median {:.2} ms, 95% under {:.2} ms, slowest {:.2} ms ({} measurements).",
            at(0.5).as_secs_f64() * 1e3,
            at(0.95).as_secs_f64() * 1e3,
            at(1.0).as_secs_f64() * 1e3,
            times.len()
        );
    }
    println!("{}", name_typed.line("Command name, after 2 letters"));
    println!("{}", next_blank.line("Next word, nothing typed"));
    println!("{}", next_typed.line("Next word, after 2 letters"));
    println!("\nOf the cases where the right word was offered at all (how well suggestions are ordered):");
    println!("{}", name_typed.ranking_line("Command name, after 2 letters"));
    println!("{}", next_blank.ranking_line("Next word, nothing typed"));
    println!("{}", next_typed.ranking_line("Next word, after 2 letters"));
    println!();
    if !misses.is_empty() {
        println!("Next words not in the top eight after 2 letters, by kind:");
        for (reason, count) in &misses {
            println!("  {count:>5}  {reason}");
        }
    }
    println!("Live values (branches, files, containers) are not run here, so real use scores higher on those.");
    Ok(0)
}

fn engine_boost(engine: &crate::engine::Engine) -> f64 {
    engine.options().history_boost
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adds_and_removes_its_block_without_touching_the_rest() {
        let original = "export PATH=$HOME/bin:$PATH\nalias g=git\n";
        let installed = with_block(original, Shell::Zsh);
        assert_eq!(
            installed,
            "export PATH=$HOME/bin:$PATH\nalias g=git\n\n# >>> hiruzen >>>\neval \"$(hiruzen init zsh)\"\n# <<< hiruzen <<<\n"
        );
        assert_eq!(installed_in(&installed), Installed::Block);
        assert_eq!(without_block(&installed), original);
        assert_eq!(without_block(&with_block("", Shell::Fish)), "");
        assert_eq!(with_block("no newline", Shell::Bash).lines().next(), Some("no newline"));
    }

    #[test]
    fn recognises_a_line_added_by_hand() {
        assert_eq!(installed_in("eval \"$(hiruzen init zsh)\"\n"), Installed::Manual);
        assert_eq!(installed_in("# eval \"$(hiruzen init zsh)\"\n"), Installed::No);
        assert_eq!(installed_in("alias g=git\n"), Installed::No);
    }

    #[test]
    fn encodes_text_for_a_url() {
        assert_eq!(percent_encode("a b&c=d/é"), "a%20b%26c%3Dd%2F%C3%A9");
    }
}
