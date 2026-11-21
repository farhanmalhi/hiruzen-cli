//! Suggestions that depend on the machine's current state: files, git
//! branches, package scripts, and the output of commands a spec asks for.

use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::engine::{js, Candidate, Kind};
use crate::shell::{self, Shell};
use crate::specs::{Generator, SpecStore};

/// Longest a generator command may run before its result is abandoned.
const COMMAND_TIMEOUT: Duration = Duration::from_secs(3);
/// How long a command's output is reused. Typing a word re-filters the same
/// output instead of re-running the command on each keystroke.
const CACHE_TTL: Duration = Duration::from_secs(4);
const MAX_OUTPUT: usize = 1 << 20;
const MAX_ITEMS: usize = 2000;

/// What a command printed and how it ended.
#[derive(Debug, Clone, Default)]
pub struct Output {
    pub stdout: String,
    pub stderr: String,
    /// The exit status; 1 when the command could not be run or timed out.
    pub status: i32,
}

/// Environment changes for a command: a value to set, or `None` to unset.
pub type EnvChanges = [(String, Option<String>)];

/// Runs commands on behalf of generators.
pub trait Runner: Send + Sync {
    /// Runs a command and returns its standard output if it succeeded.
    fn run(&self, argv: &[String], cwd: &Path) -> Option<String>;

    /// Runs a command and returns everything about how it went.
    fn exec(&self, argv: &[String], cwd: &Path, _env: &EnvChanges) -> Output {
        match self.run(argv, cwd) {
            Some(stdout) => Output { stdout, stderr: String::new(), status: 0 },
            None => Output { status: 1, ..Output::default() },
        }
    }
}

pub struct SystemRunner;

impl Runner for SystemRunner {
    fn run(&self, argv: &[String], cwd: &Path) -> Option<String> {
        let output = self.exec(argv, cwd, &[]);
        (output.status == 0).then_some(output.stdout)
    }

    fn exec(&self, argv: &[String], cwd: &Path, env: &EnvChanges) -> Output {
        let failed = Output { status: 1, ..Output::default() };
        let Some((program, args)) = argv.split_first() else { return failed };
        let mut command = Command::new(program);
        command
            .args(args)
            .current_dir(cwd)
            .env("HIRUZEN", "1")
            .env("GIT_OPTIONAL_LOCKS", "0")
            .env("GIT_TERMINAL_PROMPT", "0")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (key, value) in env {
            match value {
                Some(value) => command.env(key, value),
                None => command.env_remove(key),
            };
        }
        let Ok(mut child) = command.spawn() else { return failed };
        // Both pipes are drained on their own threads so a chatty command
        // cannot block on a full pipe while we wait for it.
        let drain = |pipe: Option<Box<dyn Read + Send>>| {
            std::thread::spawn(move || {
                let mut buf = Vec::new();
                if let Some(pipe) = pipe {
                    let _ = pipe.take(MAX_OUTPUT as u64).read_to_end(&mut buf);
                }
                buf
            })
        };
        let stdout = drain(child.stdout.take().map(|p| Box::new(p) as Box<dyn Read + Send>));
        let stderr = drain(child.stderr.take().map(|p| Box::new(p) as Box<dyn Read + Send>));
        let deadline = Instant::now() + COMMAND_TIMEOUT;
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status.code().unwrap_or(1),
                Ok(None) if Instant::now() >= deadline => {
                    let _ = child.kill();
                    let _ = child.wait();
                    break 1;
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(2)),
                Err(_) => break 1,
            }
        };
        Output {
            stdout: String::from_utf8_lossy(&stdout.join().unwrap_or_default()).into_owned(),
            stderr: String::from_utf8_lossy(&stderr.join().unwrap_or_default()).into_owned(),
            status,
        }
    }
}

type CacheKey = (Vec<String>, PathBuf);
/// When a command was run and how it went.
type CacheEntry = (Instant, Arc<Output>);

/// What is being completed, for generators that depend on it.
pub struct Request<'a> {
    /// The command the spec belongs to, such as `git`.
    pub root: &'a str,
    /// The archive key of the spec the generator is in, such as `aws/s3`.
    pub spec_key: &'a str,
    /// Every word of the command, ending with the one being completed.
    pub tokens: &'a [String],
    /// The word being completed.
    pub token: &'a str,
    pub cwd: &'a Path,
    pub shell: Shell,
}

/// Produces the dynamic suggestions for a spec argument.
pub struct Generators {
    runner: Box<dyn Runner>,
    cache: Mutex<HashMap<CacheKey, CacheEntry>>,
}

impl Generators {
    pub fn new(runner: Box<dyn Runner>) -> Self {
        Generators {
            runner,
            cache: Mutex::new(HashMap::new()),
        }
    }

    pub fn system() -> Self {
        Self::new(Box::new(SystemRunner))
    }

    fn exec(&self, argv: Vec<String>, cwd: &Path, env: &EnvChanges) -> Arc<Output> {
        if !env.is_empty() {
            // The result depends on more than the command line; do not share it.
            return Arc::new(self.runner.exec(&argv, cwd, env));
        }
        let key = (argv, cwd.to_path_buf());
        if let Ok(cache) = self.cache.lock() {
            if let Some((at, output)) = cache.get(&key) {
                if at.elapsed() < CACHE_TTL {
                    return output.clone();
                }
            }
        }
        let output = Arc::new(self.runner.exec(&key.0, cwd, env));
        if let Ok(mut cache) = self.cache.lock() {
            cache.retain(|_, (at, _)| at.elapsed() < CACHE_TTL);
            cache.insert(key, (Instant::now(), output.clone()));
        }
        output
    }

    /// The command's standard output, if it succeeded.
    fn run(&self, argv: &[&str], cwd: &Path) -> Option<String> {
        let output = self.exec(argv.iter().map(|s| s.to_string()).collect(), cwd, &[]);
        (output.status == 0).then(|| output.stdout.clone())
    }

    fn lines(&self, argv: &[&str], cwd: &Path) -> Vec<String> {
        match self.run(argv, cwd) {
            Some(output) => output
                .lines()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .map(str::to_string)
                .collect(),
            None => Vec::new(),
        }
    }

    /// Suggestions for one generator of an argument.
    pub fn generate(&self, generator: &Generator, request: &Request, specs: &SpecStore) -> Vec<Candidate> {
        let Request { root, token, cwd, shell, .. } = *request;
        let mut out = Vec::new();
        for template in &generator.template {
            out.extend(self.template(template, token, cwd, shell));
        }
        if let Some(native) = generator.native.as_deref() {
            out.extend(self.native(native, cwd));
        } else if generator.js.is_some() && matches!(root, "make" | "gmake") {
            // The spec asks make to dump its rule database, which lists make's
            // built-in file names as targets. Reading the Makefile is exact.
            out.extend(make_targets(cwd));
        } else if let Some(path) = &generator.js {
            match self.run_js(path, request, specs) {
                Some(output) => out.extend(js_candidates(output, token)),
                // The spec's own code could not be run; make a best guess.
                None if !generator.script.is_empty() => {
                    let argv = script_argv(&generator.script);
                    let argv: Vec<&str> = argv.iter().map(String::as_str).collect();
                    if let Some(output) = self.run(&argv, cwd) {
                        out.extend(guess_suggestions(&output));
                    }
                }
                None => out.extend(self.fallback(root, token, cwd, shell)),
            }
        } else if !generator.script.is_empty() {
            let argv = script_argv(&generator.script);
            let argv: Vec<&str> = argv.iter().map(String::as_str).collect();
            if let Some(output) = self.run(&argv, cwd) {
                let separator = generator.split_on.as_deref().unwrap_or("\n");
                let items = output
                    .split(separator)
                    .map(str::trim)
                    .filter(|item| !item.is_empty())
                    .take(MAX_ITEMS)
                    .map(|item| Candidate::new(item, Kind::Arg))
                    .collect();
                let query = generator.get_query_term.as_deref().map(|separator| match token.rfind(separator) {
                    Some(at) => &token[at + separator.len()..],
                    None => token,
                });
                out.extend(narrow_to_query(items, token, query));
            }
        } else if generator.dynamic {
            out.extend(self.fallback(root, token, cwd, shell));
        }
        out
    }

    /// Runs the spec's own JavaScript for a generator.
    fn run_js(&self, path: &serde_json::Value, request: &Request, specs: &SpecStore) -> Option<js::Output> {
        let exec = |input: js::ExecRequest| -> String {
            let argv: Vec<String> = match (input.command, input.shell) {
                (Some(command), _) if !command.is_empty() => std::iter::once(command).chain(input.args).collect(),
                (_, Some(line)) if !line.is_empty() => script_argv(&[line]),
                _ => Vec::new(),
            };
            let cwd = match input.cwd.map(PathBuf::from) {
                Some(dir) if dir.is_absolute() => dir,
                Some(dir) => request.cwd.join(dir),
                None => request.cwd.to_path_buf(),
            };
            let env: Vec<(String, Option<String>)> = input.env.unwrap_or_default().into_iter().collect();
            let output = if argv.is_empty() {
                Arc::new(Output { status: 1, ..Output::default() })
            } else {
                self.exec(argv, &cwd, &env)
            };
            serde_json::json!({ "stdout": output.stdout, "stderr": output.stderr, "status": output.status }).to_string()
        };
        // Generators read a few well-known variables; the rest of the
        // environment stays out of the engine.
        let env: HashMap<String, String> = ["HOME", "USER", "PATH", "SHELL", "USERPROFILE"]
            .iter()
            .filter_map(|name| Some((name.to_string(), std::env::var(name).ok()?)))
            .chain(std::iter::once(("PWD".to_string(), request.cwd.to_string_lossy().into_owned())))
            .collect();
        js::run(
            request.spec_key,
            &|| specs.module(request.spec_key),
            path,
            request.tokens,
            &request.cwd.to_string_lossy(),
            &env,
            &exec,
        )
    }

    pub fn template(&self, template: &str, token: &str, cwd: &Path, shell: Shell) -> Vec<Candidate> {
        match template {
            "filepaths" => files(cwd, token, shell, false),
            "folders" => files(cwd, token, shell, true),
            _ => Vec::new(),
        }
    }

    /// Stand-ins for well-known commands whose spec computes suggestions in a
    /// JavaScript function hiruzen cannot run.
    fn fallback(&self, root: &str, token: &str, cwd: &Path, shell: Shell) -> Vec<Candidate> {
        match root {
            "cd" | "pushd" | "rmdir" | "z" => files(cwd, token, shell, true),
            _ => Vec::new(),
        }
    }

    fn native(&self, name: &str, cwd: &Path) -> Vec<Candidate> {
        let named = |items: Vec<String>, description: &str, priority: f64| -> Vec<Candidate> {
            items
                .into_iter()
                .take(MAX_ITEMS)
                .map(|item| {
                    Candidate::new(item, Kind::Arg)
                        .described(description)
                        .with_priority(priority)
                })
                .collect()
        };
        match name {
            "git:localBranches" => named(
                self.lines(
                    &[
                        "git",
                        "branch",
                        "--no-color",
                        "--sort=-committerdate",
                        "--format=%(refname:short)",
                    ],
                    cwd,
                ),
                "Branch",
                75.0,
            ),
            "git:remoteLocalBranches" | "git:localOrRemoteBranches" => {
                let mut out = named(
                    self.lines(
                        &[
                            "git",
                            "branch",
                            "--no-color",
                            "--sort=-committerdate",
                            "--format=%(refname:short)",
                        ],
                        cwd,
                    ),
                    "Branch",
                    75.0,
                );
                let remote: Vec<String> = self
                    .lines(
                        &[
                            "git",
                            "branch",
                            "-r",
                            "--no-color",
                            "--sort=-committerdate",
                            "--format=%(refname:short)",
                        ],
                        cwd,
                    )
                    .into_iter()
                    .filter(|branch| !branch.ends_with("/HEAD") && branch.contains('/'))
                    .collect();
                if name == "git:remoteLocalBranches" {
                    // `git checkout feature` creates a local branch from `origin/feature`.
                    let local: Vec<String> = out.iter().map(|c| c.name.clone()).collect();
                    let stripped: Vec<String> = remote
                        .iter()
                        .filter_map(|branch| branch.split_once('/').map(|(_, rest)| rest.to_string()))
                        .filter(|branch| !local.contains(branch))
                        .collect();
                    out.extend(named(stripped, "Remote branch", 70.0));
                } else {
                    out.extend(named(remote, "Remote branch", 70.0));
                }
                out
            }
            "git:remotes" => named(self.lines(&["git", "remote"], cwd), "Remote", 75.0),
            "git:tags" => named(
                self.lines(&["git", "tag", "--list", "--sort=-creatordate"], cwd),
                "Tag",
                60.0,
            ),
            "git:stashes" => self
                .lines(&["git", "stash", "list"], cwd)
                .into_iter()
                .filter_map(|line| {
                    let (id, message) = line.split_once(": ")?;
                    Some(Candidate::new(id, Kind::Arg).described(message))
                })
                .collect(),
            "git:commits" => self.commits(cwd),
            "git:revs" | "git:treeish" => {
                let mut out = self.native("git:localBranches", cwd);
                out.extend(self.native("git:tags", cwd));
                out.extend(self.commits(cwd));
                out
            }
            "git:files_for_staging" => match self
                .run(&["git", "status", "--short", "--untracked-files=all"], cwd)
                .as_ref()
            {
                Some(output) => parse_git_status(&output),
                None => Vec::new(),
            },
            "git:getChangedTrackedFiles" => {
                named(self.lines(&["git", "diff", "--name-only"], cwd), "Changed file", 100.0)
            }
            "git:aliases" => self
                .lines(&["git", "config", "--get-regexp", "^alias\\."], cwd)
                .into_iter()
                .filter_map(|line| {
                    let (key, value) = line.split_once(' ')?;
                    Some(Candidate::new(key.strip_prefix("alias.")?, Kind::Arg).described(value))
                })
                .collect(),
            "npm:npmScriptsGenerator" => package_json_keys(cwd, &["scripts"], 75.0),
            "npm:dependenciesGenerator" | "yarn:dependenciesGenerator" => {
                package_json_keys(cwd, &["dependencies", "devDependencies", "optionalDependencies"], 75.0)
            }
            "ssh:configHosts" => ssh_config_hosts(),
            "ssh:knownHosts" => ssh_known_hosts(),
            _ => Vec::new(),
        }
    }

    fn commits(&self, cwd: &Path) -> Vec<Candidate> {
        self.lines(&["git", "log", "--oneline", "--no-color", "-n", "40"], cwd)
            .into_iter()
            .filter_map(|line| {
                let (hash, subject) = line.split_once(' ')?;
                Some(Candidate::new(hash, Kind::Arg).described(subject).with_priority(40.0))
            })
            .collect()
    }
}

/// Turns what a spec's JavaScript returned into candidates.
fn js_candidates(output: js::Output, token: &str) -> Vec<Candidate> {
    let text = |value: &serde_json::Value| value.as_str().filter(|s| !s.is_empty()).map(str::to_string);
    let items = output
        .items
        .iter()
        .take(MAX_ITEMS)
        .filter_map(|item| {
            if item.hidden.as_bool() == Some(true) {
                return None;
            }
            let names: Vec<String> = match &item.name {
                serde_json::Value::String(name) => vec![name.clone()],
                serde_json::Value::Number(name) => vec![name.to_string()],
                serde_json::Value::Array(names) => names.iter().filter_map(|n| n.as_str().map(str::to_string)).collect(),
                _ => Vec::new(),
            };
            let (name, aliases) = names.split_first()?;
            if name.is_empty() {
                return None;
            }
            let kind = match item.kind.as_str() {
                Some("folder") => Kind::Folder,
                Some("file") => Kind::File,
                Some("subcommand") => Kind::Subcommand,
                Some("option") => Kind::Option,
                _ => Kind::Arg,
            };
            let mut candidate = Candidate::new(name.as_str(), kind);
            candidate.aliases = aliases.to_vec();
            candidate.description = text(&item.description);
            candidate.display = text(&item.display_name);
            candidate.insert = text(&item.insert_value);
            candidate.priority = item.priority.as_f64().unwrap_or(60.0);
            Some(candidate)
        })
        .collect();
    narrow_to_query(items, token, output.query.as_deref())
}

/// Some generators complete only the end of a word, such as the entry after
/// the last comma of `a,b,c` or the last segment of a path. Their candidates
/// are re-expressed against the whole word, so matching and replacement work
/// the same as for every other candidate.
fn narrow_to_query(items: Vec<Candidate>, token: &str, query: Option<&str>) -> Vec<Candidate> {
    let Some(prefix) = query.and_then(|query| token.strip_suffix(query)).filter(|prefix| !prefix.is_empty()) else {
        return items;
    };
    items
        .into_iter()
        .map(|mut candidate| {
            if candidate.display.is_none() {
                candidate.display = Some(candidate.name.clone());
            }
            candidate.insert = candidate.insert.take().map(|insert| format!("{prefix}{insert}"));
            candidate.aliases = candidate.aliases.iter().map(|alias| format!("{prefix}{alias}")).collect();
            candidate.name = format!("{prefix}{}", candidate.name);
            candidate
        })
        .collect()
}

/// A one-element script holding spaces is a shell command line; anything else is argv.
fn script_argv(script: &[String]) -> Vec<String> {
    if script.len() == 1 && script[0].contains(char::is_whitespace) {
        if cfg!(windows) {
            vec!["cmd".to_string(), "/C".to_string(), script[0].clone()]
        } else {
            vec!["sh".to_string(), "-c".to_string(), script[0].clone()]
        }
    } else {
        script.to_vec()
    }
}

/// `git status --short` lines are `XY path` or `XY old -> new`.
fn parse_git_status(output: &str) -> Vec<Candidate> {
    output
        .lines()
        .filter(|line| line.len() > 3)
        .take(MAX_ITEMS)
        .filter_map(|line| {
            let (status, rest) = (line.get(..2)?, line.get(3..)?);
            let path = rest.rsplit_once(" -> ").map(|(_, new)| new).unwrap_or(rest);
            let path = path.trim().trim_matches('"');
            let description = match status.trim() {
                "??" => "Untracked file",
                "M" | "MM" | "AM" => "Changed file",
                "D" => "Deleted file",
                "A" => "Added file",
                "R" => "Renamed file",
                "UU" | "AA" | "DD" => "Conflicted file",
                _ => "Changed file",
            };
            Some(
                Candidate::new(path, Kind::File)
                    .described(description)
                    .with_priority(100.0),
            )
        })
        .collect()
}

/// Interprets command output whose intended parsing lived in a JavaScript
/// function: JSON objects yield their name-like field, and plain lines yield
/// their first column with the remainder as the description.
pub fn guess_suggestions(output: &str) -> Vec<Candidate> {
    let trimmed = output.trim();
    if trimmed.starts_with('[') {
        if let Ok(serde_json::Value::Array(items)) = serde_json::from_str::<serde_json::Value>(trimmed) {
            return items.iter().take(MAX_ITEMS).filter_map(json_candidate).collect();
        }
    }
    if trimmed.starts_with('{') && !trimmed.lines().skip(1).any(|line| line.trim_start().starts_with('{')) {
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) {
            // A single document, not a list of things to suggest.
            return json_candidate(&value)
                .filter(|_| trimmed.lines().count() == 1)
                .into_iter()
                .collect();
        }
    }
    trimmed
        .lines()
        .take(MAX_ITEMS)
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() || line.len() > 300 {
                return None;
            }
            if line.starts_with('{') {
                if let Ok(value) = serde_json::from_str::<serde_json::Value>(line) {
                    return json_candidate(&value);
                }
            }
            let line = line
                .strip_prefix("* ")
                .or_else(|| line.strip_prefix("+ "))
                .unwrap_or(line)
                .trim_start();
            let (name, rest) = line.split_once(char::is_whitespace).unwrap_or((line, ""));
            let candidate = Candidate::new(name, Kind::Arg);
            Some(if rest.trim().is_empty() {
                candidate
            } else {
                candidate.described(rest.trim())
            })
        })
        .collect()
}

fn json_candidate(value: &serde_json::Value) -> Option<Candidate> {
    let object = match value {
        serde_json::Value::String(s) => return Some(Candidate::new(s.as_str(), Kind::Arg)),
        serde_json::Value::Object(object) => object,
        _ => return None,
    };
    let field = |keys: &[&str]| -> Option<String> {
        keys.iter().find_map(|key| match object.get(*key)? {
            serde_json::Value::String(s) if !s.is_empty() => Some(s.clone()),
            serde_json::Value::Number(n) => Some(n.to_string()),
            _ => None,
        })
    };
    let name = match (field(&["Repository"]), field(&["Tag"])) {
        (Some(repository), Some(tag)) if repository != "<none>" && tag != "<none>" => {
            Some(format!("{repository}:{tag}"))
        }
        (Some(repository), _) if repository != "<none>" => Some(repository),
        _ => None,
    }
    .or_else(|| field(&["Names", "Name", "name", "number", "ID", "Id", "id", "key"]))?;
    let description = field(&[
        "title",
        "Title",
        "Image",
        "Status",
        "description",
        "Description",
        "Driver",
        "ID",
    ])
    .filter(|description| *description != name);
    let candidate = Candidate::new(name, Kind::Arg);
    Some(match description {
        Some(description) => candidate.described(&description),
        None => candidate,
    })
}

/// Lists directory entries matching the path being typed.
///
/// Each candidate's name is the path as the user would type it (so it filters
/// against the token), and its display text is the entry name alone.
pub fn files(cwd: &Path, token: &str, shell: Shell, folders_only: bool) -> Vec<Candidate> {
    let is_separator = |c: char| c == '/' || (!shell.is_posix() && c == '\\');
    let (dir_part, base) = match token.rfind(is_separator) {
        Some(idx) => token.split_at(idx + 1),
        None => ("", token),
    };
    let dir = resolve_dir(cwd, dir_part);
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let separator = if dir_part.contains('\\') && !dir_part.contains('/') {
        '\\'
    } else {
        shell.path_separator()
    };
    let show_hidden = base.starts_with('.');

    let mut out: Vec<Candidate> = entries
        .flatten()
        .take(MAX_ITEMS * 2)
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') && !show_hidden {
                return None;
            }
            // Follow symlinks so a link to a directory completes as one.
            let is_dir = std::fs::metadata(entry.path()).map(|m| m.is_dir()).unwrap_or(false);
            if folders_only && !is_dir {
                return None;
            }
            let typed = format!("{dir_part}{name}");
            let mut candidate = if is_dir {
                let mut c = Candidate::new(typed.clone(), Kind::Folder);
                c.display = Some(format!("{name}{separator}"));
                // No trailing space: the user usually continues into the folder.
                c.insert = Some(format!("{}{separator}", shell.quote_path(&typed)));
                c
            } else {
                let mut c = Candidate::new(typed.clone(), Kind::File);
                c.display = Some(name.clone());
                c.insert = Some(format!("{} ", shell.quote_path(&typed)));
                c
            };
            candidate.priority = if is_dir { 46.0 } else { 45.0 };
            Some(candidate)
        })
        .collect();
    out.sort_by_key(|a| a.name.to_lowercase());
    out.truncate(MAX_ITEMS);
    out
}

fn resolve_dir(cwd: &Path, dir_part: &str) -> PathBuf {
    if dir_part.is_empty() {
        return cwd.to_path_buf();
    }
    if let Some(rest) = dir_part.strip_prefix('~') {
        if rest.is_empty() || rest.starts_with('/') || rest.starts_with('\\') {
            if let Some(home) = shell::home_dir() {
                return home.join(rest.trim_start_matches(['/', '\\']));
            }
        }
    }
    let path = Path::new(dir_part);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    }
}

/// Finds the nearest `package.json` at or above `cwd` and lists the keys of
/// the given sections, described by their values.
fn package_json_keys(cwd: &Path, sections: &[&str], priority: f64) -> Vec<Candidate> {
    let Some(manifest) = cwd
        .ancestors()
        .map(|dir| dir.join("package.json"))
        .find(|path| path.is_file())
    else {
        return Vec::new();
    };
    let Some(json) = std::fs::read(&manifest)
        .ok()
        .and_then(|raw| serde_json::from_slice::<serde_json::Value>(&raw).ok())
    else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for section in sections {
        if let Some(map) = json.get(*section).and_then(|v| v.as_object()) {
            for (key, value) in map {
                let mut candidate = Candidate::new(key.as_str(), Kind::Arg).with_priority(priority);
                if let Some(text) = value.as_str() {
                    candidate = candidate.described(text);
                }
                out.push(candidate);
            }
        }
    }
    out
}

/// Targets of the Makefile in `cwd`: rule names that are not patterns,
/// variables or special targets.
fn make_targets(cwd: &Path) -> Vec<Candidate> {
    let Some(content) = ["GNUmakefile", "makefile", "Makefile"]
        .iter()
        .find_map(|name| std::fs::read_to_string(cwd.join(name)).ok())
    else {
        return Vec::new();
    };
    let mut seen = Vec::new();
    let mut out = Vec::new();
    let mut previous_comment: Option<&str> = None;
    for line in content.lines() {
        if let Some(comment) = line.strip_prefix('#') {
            previous_comment = Some(comment.trim());
            continue;
        }
        let comment = previous_comment.take();
        if line.starts_with(char::is_whitespace) {
            continue;
        }
        let Some((targets, rest)) = line.split_once(':') else {
            continue;
        };
        if rest.starts_with('=') || targets.contains('=') || targets.contains('%') || targets.contains('$') {
            continue;
        }
        let inline = rest.split_once("##").map(|(_, doc)| doc.trim());
        for target in targets.split_whitespace() {
            if target.starts_with('.') || seen.iter().any(|s| s == target) {
                continue;
            }
            seen.push(target.to_string());
            let mut candidate = Candidate::new(target, Kind::Arg).with_priority(75.0);
            if let Some(doc) = inline.or(comment).filter(|doc| !doc.is_empty()) {
                candidate = candidate.described(doc);
            } else {
                candidate = candidate.described("Make target");
            }
            out.push(candidate);
        }
    }
    out
}

fn ssh_config_hosts() -> Vec<Candidate> {
    let Some(config) =
        shell::home_dir().and_then(|home| std::fs::read_to_string(home.join(".ssh").join("config")).ok())
    else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for line in config.lines() {
        let line = line.trim();
        let Some((keyword, hosts)) = line.split_once(char::is_whitespace) else {
            continue;
        };
        if !keyword.eq_ignore_ascii_case("host") {
            continue;
        }
        for host in hosts.split_whitespace() {
            if !host.contains(['*', '?', '!']) && !out.iter().any(|c: &Candidate| c.name == host) {
                out.push(
                    Candidate::new(host, Kind::Arg)
                        .described("SSH host")
                        .with_priority(80.0),
                );
            }
        }
    }
    out
}

fn ssh_known_hosts() -> Vec<Candidate> {
    let Some(known) =
        shell::home_dir().and_then(|home| std::fs::read_to_string(home.join(".ssh").join("known_hosts")).ok())
    else {
        return Vec::new();
    };
    let mut out: Vec<Candidate> = Vec::new();
    for line in known.lines().take(MAX_ITEMS) {
        let Some(hosts) = line.split_whitespace().next() else {
            continue;
        };
        // Hashed entries (`|1|...`) do not reveal the host name.
        if hosts.starts_with('|') || hosts.starts_with('#') || hosts.starts_with('@') {
            continue;
        }
        for host in hosts.split(',') {
            let host = host.trim_start_matches('[').split(']').next().unwrap_or(host);
            if !host.is_empty() && !out.iter().any(|c| c.name == host) {
                out.push(
                    Candidate::new(host, Kind::Arg)
                        .described("Known host")
                        .with_priority(60.0),
                );
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_git_status_lines() {
        let out = parse_git_status(" M src/main.rs\n?? new file.txt\nR  old.rs -> new.rs\n");
        let names: Vec<&str> = out.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, vec!["src/main.rs", "new file.txt", "new.rs"]);
        assert_eq!(out[1].description.as_deref(), Some("Untracked file"));
    }

    #[test]
    fn guesses_names_from_json_lines() {
        let out = guess_suggestions(
            "{\"Names\":\"web\",\"Image\":\"nginx\",\"ID\":\"abc\"}\n{\"Repository\":\"redis\",\"Tag\":\"7\",\"ID\":\"def\"}\n",
        );
        assert_eq!(out[0].name, "web");
        assert_eq!(out[0].description.as_deref(), Some("nginx"));
        assert_eq!(out[1].name, "redis:7");
    }

    #[test]
    fn guesses_names_from_json_arrays_and_columns() {
        let out = guess_suggestions(r#"[{"number":12,"title":"Fix bug"},{"number":13,"title":"Add thing"}]"#);
        assert_eq!(out[0].name, "12");
        assert_eq!(out[0].description.as_deref(), Some("Fix bug"));
        let out = guess_suggestions("* main\n  feature/x  some text\n");
        assert_eq!(out[0].name, "main");
        assert_eq!(out[1].name, "feature/x");
        assert_eq!(out[1].description.as_deref(), Some("some text"));
    }

    #[test]
    fn ignores_a_single_json_document() {
        assert!(guess_suggestions("{\n  \"packages\": [],\n  \"version\": 1\n}").is_empty());
    }

    #[test]
    fn lists_files_relative_to_the_typed_directory() {
        let dir = std::env::temp_dir().join(format!("hiruzen-files-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("src").join("my file.rs"), "").unwrap();
        std::fs::write(dir.join("README.md"), "").unwrap();
        std::fs::write(dir.join(".hidden"), "").unwrap();

        let top = files(&dir, "", Shell::Zsh, false);
        let names: Vec<&str> = top.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, vec!["README.md", "src"]);
        assert_eq!(top[1].insert.as_deref(), Some("src/"));
        assert_eq!(top[1].display.as_deref(), Some("src/"));

        let nested = files(&dir, "src/", Shell::Zsh, false);
        assert_eq!(nested[0].name, "src/my file.rs");
        assert_eq!(nested[0].insert.as_deref(), Some(r"src/my\ file.rs "));

        assert_eq!(files(&dir, ".", Shell::Zsh, false).len(), 3);
        assert_eq!(files(&dir, "", Shell::Zsh, true).len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reads_make_targets() {
        let dir = std::env::temp_dir().join(format!("hiruzen-make-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("Makefile"),
            "CC := gcc\n.PHONY: build test\n# Compile everything\nbuild: main.o\n\tcc -o app main.o\ntest: build ## Run the tests\n%.o: %.c\n\tcc -c $<\n",
        )
        .unwrap();
        let targets = make_targets(&dir);
        let names: Vec<&str> = targets.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, vec!["build", "test"]);
        assert_eq!(targets[0].description.as_deref(), Some("Compile everything"));
        assert_eq!(targets[1].description.as_deref(), Some("Run the tests"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
