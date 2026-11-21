//! The suggestion engine: given the command line typed so far, work out what
//! the next word can be and rank the possibilities.
//!
//! Suggestions come from three sources that are ranked together: the spec for
//! the command (valid subcommands, options and arguments), generators (live
//! values such as files and git branches) and the user's history (what they
//! actually run).

pub mod generators;
pub mod history;
mod js;
pub mod tokenizer;

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{OnceLock, RwLock};

use unicode_width::UnicodeWidthStr;

use crate::shell::Shell;
use crate::specs::{Arg, Opt, SpecStore, SpecSuggestion, Subcommand};
use generators::Generators;
use history::{History, ProjectHistory, Recall};
use tokenizer::Token;

/// Most suggestions returned for one request.
const MAX_RESULTS: usize = 200;
/// How much a suggestion the user always picks in this context is lifted.
/// Spec priorities run from 0 to 100, so this moves a habitual choice past
/// its neighbours without burying a spec's deliberate top entries.
const HISTORY_BOOST: f64 = 40.0;
/// How much the command run just before counts against overall habit when
/// ranking the name of the next command.
///
/// The idea is from Davison and Hirsh's work on predicting UNIX commands
/// (IPAM, 1998). Measured with `hiruzen evaluate --file` on the first 5,000
/// commands of each of the 50 users in Schonlau's public command data set:
/// with two letters typed, the right command was the first suggestion 91.2%
/// of the time without it and 96.8% with this weight. Larger weights gain
/// nothing further.
const SEQUENCE_WEIGHT: f64 = 2.0;
/// Spec nesting followed through `loadSpec` and command arguments (`sudo git ...`).
const MAX_DEPTH: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Subcommand,
    Option,
    Arg,
    File,
    Folder,
    Command,
    History,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    /// What the suggestion is matched against, and what is inserted unless
    /// `insert` says otherwise.
    pub name: String,
    /// Other spellings that also match, such as `-m` for `--message`.
    pub aliases: Vec<String>,
    /// Text shown in the list when it differs from `name`.
    pub display: Option<String>,
    pub description: Option<String>,
    /// Exact text to insert, including any trailing space.
    pub insert: Option<String>,
    pub kind: Kind,
    pub priority: f64,
}

impl Candidate {
    pub fn new(name: impl Into<String>, kind: Kind) -> Self {
        Candidate {
            name: name.into(),
            aliases: Vec::new(),
            display: None,
            description: None,
            insert: None,
            kind,
            priority: 50.0,
        }
    }

    pub fn described(mut self, description: &str) -> Self {
        self.description = Some(description.to_string());
        self
    }

    pub fn with_priority(mut self, priority: f64) -> Self {
        self.priority = priority;
        self
    }

    pub fn label(&self) -> &str {
        self.display.as_deref().unwrap_or(&self.name)
    }
}

#[derive(Debug, Clone, Default)]
pub struct Completion {
    /// Ranked suggestions, best first.
    pub items: Vec<Candidate>,
    /// The token the suggestions replace.
    pub token: Token,
    /// What the current argument is, shown when there is nothing to list.
    pub hint: Option<String>,
    /// Suggestions fetched from the network are still to come.
    pub loading: bool,
    /// Some of the suggestions come, or will come, from the network.
    pub online: bool,
    /// Suggestions were to be fetched but there is no connection; what is
    /// listed is only what this machine knows.
    pub offline: bool,
}

impl Completion {
    /// While fetched suggestions are on their way, keeps those `previous`
    /// had for the same word that still match it. Without this the list
    /// would empty and refill on every key typed into a search.
    pub fn carry_over(&mut self, previous: &Completion) {
        let (old, new) = (&previous.token.text, &self.token.text);
        if !self.loading || !previous.online || !(new.starts_with(old.as_str()) || old.starts_with(new.as_str())) {
            return;
        }
        let needle = new.to_lowercase();
        for candidate in &previous.items {
            if candidate.name.to_lowercase().contains(&needle) && !self.items.iter().any(|c| c.name == candidate.name) {
                self.items.push(candidate.clone());
            }
        }
    }
}

/// Checks that the engine that runs spec code works, by running a trivial
/// generator through it.
pub fn self_test() -> bool {
    let module = || Some("({ args: { generators: { custom: async () => [{ name: 'ok' }] } } })".to_string());
    let output = js::run(
        "__selftest__",
        &module,
        &serde_json::json!(["args", "generators"]),
        &["selftest".to_string()],
        ".",
        &std::collections::HashMap::new(),
        &|_| String::new(),
    );
    output.is_some_and(|output| output.items.len() == 1)
}

/// The keystrokes that turn the typed token into the accepted suggestion.
#[derive(Debug, Clone, PartialEq)]
pub struct Edit {
    pub backspaces: usize,
    pub text: String,
    /// Left-arrow presses after inserting, for suggestions that place the
    /// cursor inside what they insert.
    pub cursor_back: usize,
}

pub fn edit_for(token: &Token, candidate: &Candidate) -> Option<Edit> {
    let mut target = match &candidate.insert {
        Some(insert) if !insert.chars().any(|c| c.is_control()) => insert.clone(),
        _ => format!("{} ", candidate.name),
    };
    let mut cursor_back = 0;
    if let Some(at) = target.find("{cursor}") {
        let after = target[at + "{cursor}".len()..].to_string();
        cursor_back = after.width();
        target = format!("{}{}", &target[..at], after);
    }
    // When the user typed a plain prefix of the result, only the rest is sent,
    // which avoids visibly erasing and retyping the word.
    let typed_plainly = !token.quoted && token.width == token.text.width();
    if typed_plainly && target.starts_with(&token.text) {
        let rest = target[token.text.len()..].to_string();
        if rest.is_empty() {
            return None;
        }
        return Some(Edit {
            backspaces: 0,
            text: rest,
            cursor_back,
        });
    }
    Some(Edit {
        backspaces: token.width,
        text: target,
        cursor_back,
    })
}

pub struct Engine {
    shell: Shell,
    specs: SpecStore,
    generators: Generators,
    history: RwLock<History>,
    /// hiruzen's own record of what was run in each project.
    projects: RwLock<ProjectHistory>,
    /// Aliases and functions the user's shell reported.
    shell_names: RwLock<ShellNames>,
    options: RwLock<Options>,
    path_commands: OnceLock<Vec<String>>,
}

/// Names the user's shell defines that are not programs on `PATH`.
#[derive(Debug, Default)]
struct ShellNames {
    /// Alias to what it stands for.
    aliases: HashMap<String, String>,
    functions: Vec<String>,
}

/// The parts of the user's settings that change what is suggested.
#[derive(Debug, Clone)]
pub struct Options {
    /// Match anywhere in a suggestion, not only at its start.
    pub fuzzy: bool,
    /// Use shell history to rank and to suggest.
    pub history: bool,
    /// Commands that get no suggestions.
    pub disabled_commands: Vec<String>,
    /// How far a suggestion the user always picks in this context is lifted.
    pub history_boost: f64,
    /// Let specs fetch suggestions from the network.
    pub online: bool,
    /// How much the command run just before counts when ranking the first
    /// two words of the next one. Zero ignores it.
    pub sequence_weight: f64,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            fuzzy: true,
            history: true,
            disabled_commands: Vec::new(),
            history_boost: HISTORY_BOOST,
            online: false,
            sequence_weight: SEQUENCE_WEIGHT,
        }
    }
}

impl Engine {
    pub fn new(shell: Shell, history: History, generators: Generators) -> Self {
        Engine {
            shell,
            specs: SpecStore::embedded(),
            generators,
            history: RwLock::new(history),
            projects: RwLock::new(ProjectHistory::default()),
            shell_names: RwLock::new(ShellNames::default()),
            options: RwLock::new(Options::default()),
            path_commands: OnceLock::new(),
        }
    }

    /// An engine wired to the real machine: commands are run, and the shell's
    /// history file is read.
    pub fn system(shell: Shell) -> Self {
        let projects = crate::settings::config_dir()
            .map(|dir| ProjectHistory::load(dir.join("history.tsv"), shell))
            .unwrap_or_default();
        // The shell's history file keeps mistyped commands; hiruzen's own
        // record knows which they were.
        let history = History::load_without(shell, projects.mistyped());
        let mut engine = Self::new(shell, history, Generators::system());
        engine.projects = RwLock::new(projects);
        engine
    }

    pub fn add_alias(&self, name: String, value: String) {
        if let Ok(mut names) = self.shell_names.write() {
            names.aliases.insert(name, value);
        }
    }

    pub fn add_functions(&self, functions: Vec<String>) {
        if let Ok(mut names) = self.shell_names.write() {
            names.functions = functions;
        }
    }

    pub fn options(&self) -> Options {
        self.options.read().map(|options| options.clone()).unwrap_or_default()
    }

    pub fn set_options(&self, options: Options) {
        self.generators.set_online(options.online);
        if let Ok(mut current) = self.options.write() {
            *current = options;
        }
    }

    /// Notes a command the user just ran so it influences later ranking.
    /// `cwd` is where it ran, when that is a directory on this machine.
    /// `status` is its exit status, when the shell reported one.
    pub fn record(&self, command: &str, cwd: Option<&Path>, status: Option<i32>) {
        if !self.options().history {
            return;
        }
        if let Ok(mut history) = self.history.write() {
            history.record(command, self.shell, status);
        }
        if let (Some(cwd), Ok(mut projects)) = (cwd, self.projects.write()) {
            projects.record(&history::project_root(cwd), command, self.shell, status);
        }
    }

    /// Suggestions for the word under the cursor, where `line` is the command
    /// line up to the cursor.
    pub fn complete(&self, line: &str, cwd: &Path) -> Option<Completion> {
        self.complete_at(line, cwd, false)
    }

    /// As [`complete`](Self::complete). With `remote`, the command line
    /// belongs to a shell on another machine: only what specs and history
    /// know is suggested, because files and live values here would describe
    /// the wrong machine.
    pub fn complete_at(&self, line: &str, cwd: &Path, remote: bool) -> Option<Completion> {
        self.complete_with(line, cwd, remote, &generators::Network::default())
    }

    /// As [`complete_at`](Self::complete_at), but without waiting for
    /// anything to be fetched from the network. When that leaves suggestions
    /// out, the result says so with `loading`, and `complete_at` gets them.
    pub fn complete_quick(&self, line: &str, cwd: &Path, remote: bool) -> Option<Completion> {
        self.complete_with(line, cwd, remote, &generators::Network::deferring())
    }

    fn complete_with(&self, line: &str, cwd: &Path, remote: bool, network: &generators::Network) -> Option<Completion> {
        let parsed = tokenizer::parse(line, self.shell);
        let active = parsed.active;
        // `FOO=bar cmd` runs cmd; the assignments are not part of what is completed.
        let mut done: Vec<Token> = parsed.done.into_iter().skip_while(|t| is_assignment(&t.text)).collect();
        // An alias is completed as the command it stands for: with
        // `alias gco='git checkout'`, `gco ` lists branches.
        if !remote {
            let expansion = done.first().and_then(|first| {
                let names = self.shell_names.read().ok()?;
                let value = names.aliases.get(&first.text)?;
                let expanded = tokenizer::parse(&format!("{value} "), self.shell).done;
                // An alias that wraps its own name (`alias ls='ls -G'`) keeps
                // its options but must not be expanded again.
                (!expanded.is_empty() && !first.quoted).then_some(expanded)
            });
            if let Some(expanded) = expansion {
                done.splice(0..1, expanded);
            }
        }

        let options = self.options.read().ok()?.clone();
        if let Some(root) = done.first() {
            if options.disabled_commands.contains(&command_name(&root.text)) {
                return None;
            }
        }
        let recorded = self.history.read().ok()?;
        let projects = self.projects.read().ok()?;
        let no_history = History::default();
        let project = if remote {
            None
        } else {
            projects.get(&history::project_root(cwd))
        };
        let recall = if options.history {
            Recall {
                project,
                global: &recorded,
                sequence: options.sequence_weight,
            }
        } else {
            Recall {
                project: None,
                global: &no_history,
                sequence: 0.0,
            }
        };
        let history = &recall;
        let mut has_spec = false;
        let (mut items, hint) = match done.split_first() {
            None => {
                if active.text.is_empty() {
                    return None;
                }
                (self.command_names(history, remote), None)
            }
            Some((root, rest)) => {
                let root_name = command_name(&root.text);
                match self.specs.load(&root_name) {
                    Some(spec) => {
                        has_spec = true;
                        let words: Vec<String> = done
                            .iter()
                            .map(|t| t.text.clone())
                            .chain(std::iter::once(active.text.clone()))
                            .collect();
                        let walker = Walker {
                            engine: self,
                            cwd,
                            active: &active,
                            words: &words,
                            remote,
                            network,
                        };
                        walker.walk(&spec, &root_name, &root_name, rest, Vec::new(), 0)
                    }
                    None => (Vec::new(), None),
                }
            }
        };

        let context: Vec<&str> = done.iter().map(|t| t.text.as_str()).collect();
        if !done.is_empty() {
            // What the user has typed at this point before. A spec lists what
            // is valid; history also knows the free-form values (hosts, paths,
            // names) no spec can. Where a spec has answers, its own entries
            // come first and history only adds what is missing from them.
            {
                let (limit, priority) = if items.is_empty() { (50, 55.0) } else { (20, 45.0) };
                items.extend(
                    history
                        .followers(&context)
                        .into_iter()
                        .take(limit)
                        .filter(|(token, _)| !token.starts_with('-') || active.text.starts_with('-'))
                        // Values that need quoting cannot be inserted as they are.
                        .filter(|(token, _)| !token.chars().any(|c| c.is_whitespace() || c.is_control()))
                        .map(|(token, _)| {
                            Candidate::new(token, Kind::History)
                                .described("From your history")
                                .with_priority(priority)
                        }),
                );
            }
            // A command without a spec most often takes files. One with a spec
            // gets files only when the word being typed is clearly a path.
            let has_paths = items.iter().any(|c| matches!(c.kind, Kind::File | Kind::Folder));
            let wants_path = looks_like_path(&active.text) || (!has_spec && !active.text.starts_with('-'));
            if !has_paths && wants_path && !remote {
                items.extend(generators::files(cwd, &active.text, self.shell, false));
            }
        }

        // A value already given to this command is not worth offering again,
        // as with the second file passed to `git add`.
        items.retain(|c| {
            !matches!(c.kind, Kind::Arg | Kind::File) || !context.get(1..).unwrap_or(&[]).contains(&c.name.as_str())
        });

        let items = rank(items, &active.text, &context, history, &options);
        // The hint names the argument expected next; once the user is typing
        // it, a box that only repeats the argument's name is noise.
        let hint = hint.filter(|_| active.text.is_empty());
        let loading = network.deferred();
        let offline = network.unreachable();
        if items.is_empty() && hint.is_none() && !loading && !offline {
            return None;
        }
        Some(Completion {
            items,
            token: active,
            hint,
            loading,
            online: network.used(),
            offline,
        })
    }

    /// Candidates for the command name itself: what the user runs most, then
    /// everything installed.
    fn command_names(&self, history: &Recall, remote: bool) -> Vec<Candidate> {
        let index = self.specs.index();
        let describe = |name: &str| -> Option<String> {
            index
                .binary_search_by(|(key, _)| key.as_str().cmp(name))
                .ok()
                .map(|i| index[i].1.clone())
                .filter(|description| !description.is_empty())
        };
        let mut seen: HashSet<String> = HashSet::new();
        let mut out = Vec::new();
        for (token, affinity) in history.followers(&[]).into_iter().take(300) {
            if seen.insert(token.clone()) {
                let description = describe(&token).or_else(|| Some("From your history".to_string()));
                let mut candidate = Candidate::new(token, Kind::Command).with_priority(55.0 + 20.0 * affinity);
                candidate.description = description;
                out.push(candidate);
            }
        }
        if !remote {
            if let Ok(names) = self.shell_names.read() {
                for (alias, value) in &names.aliases {
                    if seen.insert(alias.clone()) {
                        out.push(
                            Candidate::new(alias.as_str(), Kind::Command)
                                .described(&format!("Alias for {value}"))
                                .with_priority(50.0),
                        );
                    }
                }
                for function in &names.functions {
                    if seen.insert(function.clone()) {
                        out.push(
                            Candidate::new(function.as_str(), Kind::Command)
                                .described("Shell function")
                                .with_priority(42.0),
                        );
                    }
                }
            }
        }
        // What is installed here says nothing about the other machine.
        let installed: &[String] = if remote {
            &[]
        } else {
            self.path_commands.get_or_init(scan_path)
        };
        for name in installed {
            if seen.insert(name.clone()) {
                let mut candidate = Candidate::new(name.as_str(), Kind::Command).with_priority(40.0);
                candidate.description = describe(name);
                out.push(candidate);
            }
        }
        out
    }
}

fn is_assignment(token: &str) -> bool {
    match token.split_once('=') {
        Some((name, _)) => {
            !name.is_empty()
                && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                && !name.starts_with(|c: char| c.is_ascii_digit())
        }
        None => false,
    }
}

/// The spec key for a command as typed: `./node_modules/.bin/tsc` and
/// `C:\tools\git.exe` are `tsc` and `git`.
fn command_name(typed: &str) -> String {
    let base = typed.rsplit(['/', '\\']).next().unwrap_or(typed);
    let lower = base.to_ascii_lowercase();
    for ext in [".exe", ".cmd", ".bat"] {
        if lower.ends_with(ext) && base.len() > ext.len() {
            return base[..base.len() - ext.len()].to_string();
        }
    }
    base.to_string()
}

fn looks_like_path(token: &str) -> bool {
    token.starts_with("./")
        || token.starts_with("../")
        || token.starts_with('/')
        || token.starts_with("~/")
        || token.starts_with(".\\")
        || token.starts_with("..\\")
        || (token.len() >= 3 && token.as_bytes()[1] == b':' && matches!(token.as_bytes()[2], b'\\' | b'/'))
        || (token.contains('/') && !token.contains("://"))
}

/// Every executable name on `PATH`.
fn scan_path() -> Vec<String> {
    let Some(path) = std::env::var_os("PATH") else {
        return Vec::new();
    };
    let mut names: HashSet<String> = HashSet::new();
    for dir in std::env::split_paths(&path) {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if cfg!(windows) {
                let lower = name.to_ascii_lowercase();
                if let Some(ext) = [".exe", ".cmd", ".bat", ".ps1"]
                    .iter()
                    .find(|ext| lower.ends_with(*ext))
                {
                    names.insert(name[..name.len() - ext.len()].to_string());
                }
            } else if !name.starts_with('.') {
                names.insert(name);
            }
        }
    }
    let mut list: Vec<String> = names.into_iter().collect();
    list.sort();
    list
}

struct Walker<'a> {
    engine: &'a Engine,
    cwd: &'a Path,
    active: &'a Token,
    /// Every word of the command, ending with the one being completed.
    words: &'a [String],
    /// The command line is being typed on another machine.
    remote: bool,
    network: &'a generators::Network,
}

/// An option's arguments that are still being consumed.
struct Pending {
    args: Vec<Arg>,
    index: usize,
    /// A variadic argument has taken at least one value.
    consumed: bool,
}

impl Walker<'_> {
    /// Consumes the finished tokens against `spec`, then lists what may come
    /// next. Returns the unranked candidates and a hint describing the
    /// argument expected at the cursor.
    fn walk(
        &self,
        spec: &Subcommand,
        root: &str,
        spec_key: &str,
        tokens: &[Token],
        inherited: Vec<Opt>,
        depth: usize,
    ) -> (Vec<Candidate>, Option<String>) {
        let specs = &self.engine.specs;
        let mut current = spec;
        let mut persistent = inherited;
        let mut used: Vec<String> = Vec::new();
        let mut arg_index = 0;
        let mut positional_seen = false;
        let mut options_done = false;
        let mut pending: Option<Pending> = None;

        let mut i = 0;
        while i < tokens.len() {
            let text = tokens[i].text.as_str();
            let quoted = tokens[i].quoted;
            i += 1;

            if let Some(state) = pending.as_mut() {
                let arg = &state.args[state.index];
                let starts_option = !quoted
                    && text.starts_with('-')
                    && (arg.is_optional || (arg.is_variadic && state.consumed))
                    && find_option(current, &persistent, option_name(text)).is_some();
                if !starts_option {
                    if arg.is_variadic {
                        state.consumed = true;
                    } else if state.index + 1 < state.args.len() {
                        state.index += 1;
                    } else {
                        pending = None;
                    }
                    continue;
                }
                pending = None;
            }

            if !quoted && !options_done && text == "--" {
                options_done = true;
                continue;
            }

            if !quoted && !options_done && text.starts_with('-') && text.len() > 1 {
                let name = option_name(text);
                let attached = text.len() > name.len();
                if let Some(option) = find_option(current, &persistent, name) {
                    used.extend(option.name.iter().cloned());
                    if !attached && !option.args.is_empty() && option.requires_separator.is_none() {
                        pending = Some(Pending {
                            args: option.args.clone(),
                            index: 0,
                            consumed: false,
                        });
                    }
                    continue;
                }
                let posix = !current
                    .parser_directives
                    .as_ref()
                    .is_some_and(|d| d.flags_are_posix_noncompliant);
                if posix && !text.starts_with("--") && !attached {
                    // `-abc` is `-a -b -c`; only the last may take an argument.
                    let flags: Vec<char> = text.chars().skip(1).collect();
                    for (n, flag) in flags.iter().enumerate() {
                        if let Some(option) = find_option(current, &persistent, &format!("-{flag}")) {
                            used.extend(option.name.iter().cloned());
                            if n + 1 == flags.len() && !option.args.is_empty() {
                                pending = Some(Pending {
                                    args: option.args.clone(),
                                    index: 0,
                                    consumed: false,
                                });
                            }
                        }
                    }
                }
                continue;
            }

            if !positional_seen {
                if let Some(sub) = current.subcommands.iter().find(|s| s.name.iter().any(|n| n == text)) {
                    persistent.extend(current.options.iter().filter(|o| o.is_persistent).cloned());
                    if let Some(key) = sub.load_spec.as_deref() {
                        if depth < MAX_DEPTH {
                            if let Some(loaded) = specs.load(key) {
                                return self.walk(&loaded, root, key, &tokens[i..], persistent, depth + 1);
                            }
                        }
                    }
                    current = sub;
                    used.clear();
                    arg_index = 0;
                    continue;
                }
            }

            positional_seen = true;
            if let Some(arg) = current.args.get(arg_index) {
                if arg.is_command {
                    // The rest of the line is another command, as in `sudo git push`.
                    let name = command_name(text);
                    return match specs.load(&name) {
                        Some(loaded) if depth < MAX_DEPTH => {
                            self.walk(&loaded, &name, &name, &tokens[i..], Vec::new(), depth + 1)
                        }
                        _ => (Vec::new(), None),
                    };
                }
                if !arg.is_variadic {
                    arg_index += 1;
                }
            }
        }

        let needle = self.active.text.as_str();
        let mut items: Vec<Candidate> = Vec::new();
        let mut hint: Option<String> = None;

        if let Some(state) = &pending {
            let arg = &state.args[state.index];
            items.extend(self.arg_candidates(arg, root, spec_key, needle));
            hint = arg_hint(arg);
            if !arg.is_optional && !(arg.is_variadic && state.consumed) {
                return (items, hint);
            }
        }

        // `--option=value`: complete the value, keeping the option attached.
        if !self.active.quoted && needle.starts_with('-') {
            let name = option_name(needle);
            if needle.len() > name.len() {
                if let Some(option) = find_option(current, &persistent, name) {
                    if let Some(arg) = option.args.first() {
                        let separator = &needle[name.len()..name.len() + 1];
                        let value = &needle[name.len() + 1..];
                        let values = self.arg_candidates(arg, root, spec_key, value);
                        return (
                            values
                                .into_iter()
                                .map(|mut candidate| {
                                    let full = format!("{name}{separator}{}", candidate.name);
                                    candidate.display = Some(candidate.label().to_string());
                                    candidate.insert = Some(match candidate.insert.take() {
                                        Some(insert) => format!("{name}{separator}{insert}"),
                                        None => format!("{full} "),
                                    });
                                    candidate.name = full;
                                    candidate
                                })
                                .collect(),
                            arg_hint(arg),
                        );
                    }
                }
            }
        }

        let wants_options = !self.active.quoted && !options_done && needle.starts_with('-');
        if !wants_options {
            if !positional_seen {
                for sub in current.subcommands.iter().filter(|s| !s.hidden) {
                    let Some((name, aliases)) = sub.name.split_first() else {
                        continue;
                    };
                    let mut candidate = Candidate::new(name.as_str(), Kind::Subcommand);
                    candidate.aliases = aliases.to_vec();
                    candidate.description = sub.description.clone();
                    candidate.priority = sub.priority.unwrap_or(50.0);
                    candidate.insert = sub.insert_value.clone();
                    items.push(candidate);
                }
                items.extend(
                    current
                        .additional_suggestions
                        .iter()
                        .filter_map(|s| suggestion_candidate(s, Kind::Subcommand)),
                );
            }
            if let Some(arg) = current.args.get(arg_index) {
                items.extend(self.arg_candidates(arg, root, spec_key, needle));
                if hint.is_none() {
                    hint = arg_hint(arg);
                }
            }
        }

        // Options stand in when nothing else fits. With a fetch still to
        // come that is not yet known, and they would only flash by.
        let nothing_fits = items.is_empty() && !self.network.deferred();
        if (wants_options || nothing_fits) && !options_done {
            let prefer_long = needle.starts_with("--");
            for option in current.options.iter().chain(persistent.iter()) {
                if option.hidden || option.name.is_empty() {
                    continue;
                }
                let already_used = option.name.iter().any(|n| used.contains(n));
                if (already_used && !option.is_repeatable) || option.exclusive_on.iter().any(|n| used.contains(n)) {
                    continue;
                }
                let preferred = option
                    .name
                    .iter()
                    .find(|n| n.starts_with("--") == prefer_long && n.starts_with(needle))
                    .or_else(|| option.name.iter().find(|n| n.starts_with(needle)))
                    .unwrap_or(&option.name[0]);
                let mut candidate = Candidate::new(preferred.as_str(), Kind::Option);
                candidate.aliases = option.name.iter().filter(|n| *n != preferred).cloned().collect();
                if option.name.len() > 1 {
                    candidate.display = Some(option.name.join(", "));
                }
                candidate.description = option.description.clone();
                // Options listed only because nothing else fits rank below real matches.
                candidate.priority = option.priority.unwrap_or(if wants_options { 50.0 } else { 35.0 });
                candidate.insert = match (&option.insert_value, &option.requires_separator) {
                    (Some(insert), _) => Some(insert.clone()),
                    (None, Some(separator)) => Some(format!("{preferred}{separator}")),
                    (None, None) => None,
                };
                items.push(candidate);
            }
        }

        (items, hint)
    }

    fn arg_candidates(&self, arg: &Arg, root: &str, spec_key: &str, needle: &str) -> Vec<Candidate> {
        let generators = &self.engine.generators;
        let shell = self.engine.shell;
        let mut out: Vec<Candidate> = arg
            .suggestions
            .iter()
            .filter_map(|s| suggestion_candidate(s, Kind::Arg))
            .collect();
        if self.remote {
            return out;
        }
        for template in &arg.template {
            out.extend(generators.template(template, needle, self.cwd, shell));
        }
        let request = generators::Request {
            root,
            spec_key,
            tokens: self.words,
            token: needle,
            cwd: self.cwd,
            shell,
            network: self.network,
        };
        for generator in &arg.generators {
            out.extend(generators.generate(generator, &request, &self.engine.specs));
        }
        out
    }
}

/// The option part of a token: `--color` for `--color=auto`.
fn option_name(token: &str) -> &str {
    match token.find('=') {
        Some(at) if at > 0 => &token[..at],
        _ => token,
    }
}

fn find_option<'a>(current: &'a Subcommand, persistent: &'a [Opt], name: &str) -> Option<&'a Opt> {
    current
        .options
        .iter()
        .chain(persistent.iter())
        .find(|option| option.name.iter().any(|n| n == name))
}

fn suggestion_candidate(suggestion: &SpecSuggestion, kind: Kind) -> Option<Candidate> {
    if suggestion.hidden {
        return None;
    }
    let (name, aliases) = suggestion.name.split_first()?;
    let mut candidate = Candidate::new(name.as_str(), kind);
    candidate.aliases = aliases.to_vec();
    candidate.description = suggestion.description.clone();
    candidate.insert = suggestion.insert_value.clone();
    candidate.priority = suggestion.priority.unwrap_or(55.0);
    Some(candidate)
}

fn arg_hint(arg: &Arg) -> Option<String> {
    match (&arg.name, &arg.description) {
        (Some(name), Some(description)) => Some(format!("{name}: {description}")),
        (Some(name), None) => Some(name.clone()),
        (None, Some(description)) => Some(description.clone()),
        (None, None) => None,
    }
}

/// How well a candidate matches what was typed. Higher tiers always outrank
/// lower ones, whatever the priorities.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Match {
    Fuzzy,
    Contains,
    Prefix,
    ExactPrefix,
}

fn match_one(text: &str, needle: &str, needle_lower: &str) -> Option<Match> {
    if text.starts_with(needle) {
        return Some(Match::ExactPrefix);
    }
    let lower = text.to_lowercase();
    if lower.starts_with(needle_lower) {
        return Some(Match::Prefix);
    }
    // One typed letter matches too much to be useful beyond prefixes.
    if needle_lower.chars().count() < 2 {
        return None;
    }
    if lower.contains(needle_lower) {
        return Some(Match::Contains);
    }
    let mut wanted = needle_lower.chars().peekable();
    for ch in lower.chars() {
        if wanted.peek() == Some(&ch) {
            wanted.next();
        }
    }
    wanted.peek().is_none().then_some(Match::Fuzzy)
}

fn match_candidate(candidate: &Candidate, needle: &str, needle_lower: &str) -> Option<Match> {
    let mut best = match_one(&candidate.name, needle, needle_lower);
    for alias in &candidate.aliases {
        best = best.max(match_one(alias, needle, needle_lower));
    }
    // A path is also matched on its last component, so `mai` finds `src/main.rs`
    // once `src/` has been typed and fuzzy matching ignores the directory part.
    if matches!(candidate.kind, Kind::File | Kind::Folder) {
        if let (Some(label), Some(at)) = (candidate.display.as_deref(), needle.rfind(['/', '\\'])) {
            let base = &needle[at + 1..];
            let base_lower = base.to_lowercase();
            if candidate.name.starts_with(&needle[..at + 1]) {
                best = best.max(match_one(label, base, &base_lower));
            } else {
                best = None;
            }
        }
    }
    best
}

/// Filters candidates to those matching `needle` and orders them best first.
fn rank(items: Vec<Candidate>, needle: &str, context: &[&str], history: &Recall, options: &Options) -> Vec<Candidate> {
    let needle_lower = needle.to_lowercase();
    let mut seen: HashSet<String> = HashSet::new();
    let mut scored: Vec<(Match, f64, usize, Candidate)> = Vec::new();
    for (order, candidate) in items.into_iter().enumerate() {
        if candidate.name.is_empty() {
            continue;
        }
        let quality = if needle.is_empty() {
            Match::ExactPrefix
        } else {
            match match_candidate(&candidate, needle, &needle_lower) {
                Some(quality) if options.fuzzy || quality >= Match::Prefix => quality,
                _ => continue,
            }
        };
        if !seen.insert(candidate.name.clone()) {
            continue;
        }
        let mut affinity = history.affinity(context, &candidate.name);
        for alias in &candidate.aliases {
            affinity = affinity.max(history.affinity(context, alias));
        }
        let score = candidate.priority + options.history_boost * affinity;
        scored.push((quality, score, order, candidate));
    }
    scored.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal))
            .then_with(|| a.2.cmp(&b.2))
    });
    scored
        .into_iter()
        .take(MAX_RESULTS)
        .map(|(_, _, _, candidate)| candidate)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use generators::Runner;

    /// Answers generator commands from a fixed table.
    struct FakeRunner(Vec<(&'static str, &'static str)>);

    impl Runner for FakeRunner {
        fn run(&self, argv: &[String], _cwd: &Path) -> Option<String> {
            let line = argv.join(" ");
            self.0
                .iter()
                .find(|(prefix, _)| line.starts_with(prefix))
                .map(|(_, output)| output.to_string())
        }
    }

    fn engine(history: &[&str]) -> Engine {
        let mut recorded = History::default();
        for command in history {
            recorded.record(command, Shell::Zsh, None);
        }
        let runner = FakeRunner(vec![
            ("git branch -r", "origin/main\norigin/feature/login\norigin/HEAD\n"),
            ("git branch", "  main\n* develop\n  feature/search\n"),
            ("git remote", "origin\nupstream\n"),
            ("git status", " M src/lib.rs\n?? notes.txt\n"),
            (
                "curl",
                r#"[{"package":{"name":"react","description":"UI library"}},{"package":{"name":"react-dom","description":"DOM renderer"}}]"#,
            ),
            (
                "docker ps",
                "{\"Names\":\"web\",\"Image\":\"nginx\"}\n{\"Names\":\"db\",\"Image\":\"postgres\"}\n",
            ),
        ]);
        Engine::new(Shell::Zsh, recorded, Generators::new(Box::new(runner)))
    }

    fn names(engine: &Engine, line: &str) -> Vec<String> {
        let cwd = std::env::temp_dir();
        engine
            .complete(line, &cwd)
            .map(|c| c.items.into_iter().map(|i| i.name).collect())
            .unwrap_or_default()
    }

    #[test]
    fn suggests_subcommands_by_prefix() {
        let engine = engine(&[]);
        let got = names(&engine, "git ch");
        assert!(
            got.starts_with(&["checkout".to_string()]) || got.contains(&"checkout".to_string()),
            "{got:?}"
        );
        assert!(got.contains(&"cherry-pick".to_string()));
        assert!(!got.contains(&"push".to_string()));
    }

    #[test]
    fn history_lifts_habitual_choices() {
        let plain = engine(&[]);
        let habitual = engine(&["git cherry-pick abc", "git cherry-pick def", "git cherry-pick 123"]);
        let before = names(&plain, "git ch");
        let after = names(&habitual, "git ch");
        assert_eq!(after[0], "cherry-pick", "{after:?}");
        assert!(before.iter().position(|n| n == "cherry-pick") >= after.iter().position(|n| n == "cherry-pick"));
    }

    #[test]
    fn a_remote_command_line_gets_no_local_values() {
        let engine = engine(&["git checkout release", "deploytool run"]);
        let cwd = std::env::temp_dir();
        let names = |line: &str| -> Vec<String> {
            engine
                .complete_at(line, &cwd, true)
                .map(|c| c.items.into_iter().map(|i| i.name).collect())
                .unwrap_or_default()
        };
        // Subcommands and options still come from the spec.
        assert!(names("git ch").contains(&"checkout".to_string()));
        // Branches would be read from this machine, and a branch typed in
        // some local project says nothing about the other machine either.
        let branches = names("git checkout ");
        assert!(!branches.contains(&"release".to_string()), "{branches:?}");
        assert!(
            !branches.contains(&"main".to_string()) && !branches.contains(&"develop".to_string()),
            "{branches:?}"
        );
        // No local files for a command without a spec.
        assert!(names("unknowntool ").is_empty());
        assert_eq!(names("deployt"), vec!["deploytool".to_string()]);
    }

    #[test]
    fn the_current_branch_leads_for_push_and_trails_for_checkout() {
        let engine = engine(&[]);
        assert_eq!(names(&engine, "git push origin ")[0], "develop");
        let checkout = names(&engine, "git checkout ");
        assert!(
            checkout.iter().position(|n| n == "develop") > checkout.iter().position(|n| n == "main"),
            "{checkout:?}"
        );
    }

    #[test]
    fn habits_are_specific_to_the_project() {
        let engine = engine(&["deploytool staging", "deploytool staging", "deploytool staging"]);
        let root = std::env::temp_dir().join(format!("hiruzen-habits-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (a, b) = (root.join("a"), root.join("b"));
        for dir in [&a, &b] {
            std::fs::create_dir_all(dir.join(".git")).unwrap();
        }
        engine.record("deploytool production", Some(&a), None);
        let first = |cwd: &Path| engine.complete("deploytool ", cwd).unwrap().items[0].name.clone();
        assert_eq!(first(&a), "production");
        assert_eq!(first(&b), "staging");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn an_alias_completes_as_the_command_it_stands_for() {
        let engine = engine(&[]);
        engine.add_alias("gco".to_string(), "git checkout".to_string());
        engine.add_alias("g".to_string(), "git".to_string());
        engine.add_functions(vec!["mkcd".to_string()]);
        assert!(names(&engine, "gco ").contains(&"main".to_string()));
        assert!(names(&engine, "g ch").contains(&"checkout".to_string()));
        // Aliases and functions are offered as command names.
        assert!(names(&engine, "gc").contains(&"gco".to_string()));
        assert!(names(&engine, "mkc").contains(&"mkcd".to_string()));
        // Not on another machine, where they do not exist.
        let remote = engine.complete_at("gco ", &std::env::temp_dir(), true);
        assert!(remote.is_none_or(|c| !c.items.iter().any(|i| i.name == "main")));
    }

    #[test]
    fn suggests_live_branches_for_checkout() {
        let engine = engine(&[]);
        let got = names(&engine, "git checkout ");
        assert!(got.contains(&"main".to_string()), "{got:?}");
        assert!(got.contains(&"feature/search".to_string()));
        let filtered = names(&engine, "git checkout dev");
        assert_eq!(filtered[0], "develop");
    }

    #[test]
    fn fetched_suggestions_arrive_in_a_second_pass() {
        let engine = engine(&[]);
        engine.set_options(Options {
            online: true,
            ..Options::default()
        });
        let cwd = std::env::temp_dir();
        let quick = engine
            .complete_quick("npm install rea", &cwd, false)
            .expect("a loading state");
        assert!(quick.loading && quick.online);
        assert!(!quick.items.iter().any(|c| c.name == "react"), "{:?}", quick.items);

        let full = engine.complete_at("npm install rea", &cwd, false).unwrap();
        assert!(!full.loading && full.online);
        assert_eq!(full.items[0].name, "react", "{:?}", full.items);

        // The next key: what was fetched stays listed while the search runs again.
        let mut next = engine.complete_quick("npm install reac", &cwd, false).unwrap();
        assert!(next.loading);
        next.carry_over(&full);
        assert!(next.items.iter().any(|c| c.name == "react-dom"), "{:?}", next.items);

        // Nothing is carried into a different word.
        let mut other = engine.complete_quick("npm install vu", &cwd, false).unwrap();
        other.carry_over(&full);
        assert!(other.items.is_empty(), "{:?}", other.items);
    }

    #[test]
    fn nothing_is_fetched_or_awaited_when_online_is_off() {
        let engine = engine(&[]);
        engine.set_options(Options {
            online: false,
            ..Options::default()
        });
        let quick = engine.complete_quick("npm install rea", &std::env::temp_dir(), false);
        assert!(quick.is_none_or(|c| !c.loading && !c.online));
    }

    #[test]
    fn suggests_options_when_a_dash_is_typed() {
        let engine = engine(&[]);
        let got = names(&engine, "git commit --am");
        assert_eq!(got[0], "--amend", "{got:?}");
        let all = names(&engine, "git commit -");
        assert!(all.iter().all(|n| n.starts_with('-')), "{all:?}");
        assert!(all.len() > 5);
    }

    #[test]
    fn option_arguments_take_precedence() {
        let engine = engine(&[]);
        let completion = engine
            .complete("git commit -m ", &std::env::temp_dir())
            .expect("a hint");
        assert!(completion.items.is_empty(), "{:?}", completion.items);
        assert!(completion.hint.is_some());
    }

    #[test]
    fn used_options_are_not_offered_again() {
        let engine = engine(&[]);
        let got = names(&engine, "git commit --amend --am");
        assert!(!got.contains(&"--amend".to_string()), "{got:?}");
    }

    #[test]
    fn follows_commands_nested_in_arguments() {
        let engine = engine(&[]);
        let got = names(&engine, "sudo git pu");
        assert!(
            got.contains(&"push".to_string()) && got.contains(&"pull".to_string()),
            "{got:?}"
        );
    }

    #[test]
    fn completes_only_the_last_piped_command() {
        let engine = engine(&[]);
        let got = names(&engine, "echo hi | git pus");
        assert_eq!(got[0], "push");
    }

    #[test]
    fn guesses_names_from_generator_output() {
        let engine = engine(&[]);
        let got = names(&engine, "docker stop ");
        assert!(
            got.contains(&"web".to_string()) && got.contains(&"db".to_string()),
            "{got:?}"
        );
    }

    #[test]
    fn unknown_commands_fall_back_to_history() {
        let engine = engine(&["mytool deploy staging", "mytool deploy staging", "mytool status"]);
        let got = names(&engine, "mytool ");
        assert_eq!(got[0], "deploy", "{got:?}");
        // Values further along are offered only where they were typed.
        assert!(!names(&engine, "mytool deploy ").contains(&"staging".to_string()));
        engine.record("mytool deploy staging", Some(&std::env::temp_dir()), None);
        assert_eq!(names(&engine, "mytool deploy ")[0], "staging");
    }

    #[test]
    fn suggests_command_names_from_history() {
        let engine = engine(&["kubectl get pods", "kubectl get pods", "kubens x"]);
        let got = names(&engine, "kub");
        assert_eq!(got[0], "kubectl", "{got:?}");
        assert!(names(&engine, "").is_empty());
    }

    #[test]
    fn computes_the_keystrokes_to_accept() {
        let token = Token {
            text: "ch".into(),
            width: 2,
            complete: false,
            quoted: false,
        };
        let edit = edit_for(&token, &Candidate::new("checkout", Kind::Subcommand)).unwrap();
        assert_eq!(
            edit,
            Edit {
                backspaces: 0,
                text: "eckout ".into(),
                cursor_back: 0
            }
        );

        let token = Token {
            text: "CH".into(),
            width: 2,
            complete: false,
            quoted: false,
        };
        let edit = edit_for(&token, &Candidate::new("checkout", Kind::Subcommand)).unwrap();
        assert_eq!(
            edit,
            Edit {
                backspaces: 2,
                text: "checkout ".into(),
                cursor_back: 0
            }
        );

        let token = Token {
            text: "my fi".into(),
            width: 6,
            complete: false,
            quoted: false,
        };
        let mut file = Candidate::new("my file.txt", Kind::File);
        file.insert = Some(r"my\ file.txt ".into());
        assert_eq!(edit_for(&token, &file).unwrap().backspaces, 6);

        let mut templated = Candidate::new("-m", Kind::Option);
        templated.insert = Some("-m '{cursor}'".into());
        let edit = edit_for(&Token::default(), &templated).unwrap();
        assert_eq!(
            edit,
            Edit {
                backspaces: 0,
                text: "-m ''".into(),
                cursor_back: 1
            }
        );
    }

    #[test]
    fn matching_prefers_prefixes_over_fuzzy_hits() {
        let empty = History::default();
        let history = Recall {
            project: None,
            global: &empty,
            sequence: 0.0,
        };
        let items = vec![
            Candidate::new("stash", Kind::Subcommand),
            Candidate::new("status", Kind::Subcommand),
            Candidate::new("reset", Kind::Subcommand),
            Candidate::new("restore", Kind::Subcommand),
        ];
        let got: Vec<String> = rank(items.clone(), "st", &["git"], &history, &Options::default())
            .into_iter()
            .map(|c| c.name)
            .collect();
        assert_eq!(got, vec!["stash", "status", "restore", "reset"]);
        let strict: Vec<String> = rank(
            items,
            "st",
            &["git"],
            &history,
            &Options {
                fuzzy: false,
                ..Options::default()
            },
        )
        .into_iter()
        .map(|c| c.name)
        .collect();
        assert_eq!(strict, vec!["stash", "status"]);
    }
}
