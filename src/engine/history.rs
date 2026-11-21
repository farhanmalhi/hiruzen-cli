//! What the user actually runs, learned from their shell history. It ranks
//! spec suggestions and supplies suggestions for commands that have no spec.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::engine::tokenizer;
use crate::shell::Shell;

/// Only the most recent history lines are read.
const MAX_LINES: usize = 20_000;
/// How many leading tokens of a command form a context.
const MAX_DEPTH: usize = 4;

#[derive(Debug, Default)]
pub struct History {
    /// Context (the tokens typed so far, joined by a space) to the tokens that
    /// followed it, weighted towards recent use.
    next: HashMap<String, HashMap<String, f64>>,
    /// What tends to come after a command: the previous command's key to the
    /// first word of the next one, and that key plus a first word to the
    /// second. Kept as decaying probabilities, so recent habits dominate.
    after: HashMap<String, HashMap<String, f64>>,
    /// The key of the command recorded last.
    last: Option<String>,
}

/// Exit status a shell gives a command it could not find.
const NOT_FOUND: i32 = 127;

/// How much to learn from a command given how it ended: fully from one that
/// succeeded or whose outcome is unknown, less from one that failed, and
/// `None` for one that was never a real command.
fn trust(status: Option<i32>) -> Option<f64> {
    match status {
        Some(NOT_FOUND) => None,
        Some(0) | None => Some(1.0),
        Some(_) => Some(0.3),
    }
}

/// How quickly an old habit of what follows a command fades. Each new
/// observation scales what was known before by this much. The value is the
/// one Davison and Hirsh found best for predicting UNIX commands (IPAM, 1998).
const SEQUENCE_DECAY: f64 = 0.8;
/// Separates the parts of a sequence key; it cannot occur in a command word.
const KEY_SEPARATOR: char = '\u{1}';

/// What identifies a command for sequence purposes: its first two words, so
/// `git add` and `git push` are different predecessors.
fn sequence_key(tokens: &[String]) -> String {
    tokens.iter().take(2).map(String::as_str).collect::<Vec<_>>().join(" ")
}

impl History {
    /// Learns from commands given oldest first.
    pub fn from_commands(commands: &[String], shell: Shell) -> History {
        let mut history = History::default();
        let total = commands.len().max(1);
        for (i, command) in commands.iter().enumerate() {
            // Older entries count for less: weights run from 0.25 up to 1.
            let weight = 0.25 + 0.75 * (i + 1) as f64 / total as f64;
            history.add(command, shell, weight);
        }
        // The last command in a history file was not run just now.
        history.last = None;
        history
    }

    /// How likely `token` is to come next given the command run just before,
    /// from 0 to 1 (1 for the most likely). Only a command's first two words
    /// are predicted this way; `context` is the words typed so far.
    pub fn sequence_affinity(&self, context: &[&str], token: &str) -> f64 {
        let Some(last) = &self.last else { return 0.0 };
        let key = match context {
            [] => last.clone(),
            [first] => format!("{last}{KEY_SEPARATOR}{first}"),
            _ => return 0.0,
        };
        let Some(row) = self.after.get(&key) else { return 0.0 };
        let Some(weight) = row.get(token) else { return 0.0 };
        let max = row.values().cloned().fold(0.0_f64, f64::max);
        if max > 0.0 {
            weight / max
        } else {
            0.0
        }
    }

    fn observe_sequence(&mut self, tokens: &[String]) {
        if let Some(last) = self.last.take() {
            let mut observe = |key: String, next: &String| {
                let row = self.after.entry(key).or_default();
                for weight in row.values_mut() {
                    *weight *= SEQUENCE_DECAY;
                }
                *row.entry(next.clone()).or_insert(0.0) += 1.0 - SEQUENCE_DECAY;
                // Habits that have faded to nothing are dropped.
                row.retain(|_, weight| *weight > 1e-4);
            };
            observe(last.clone(), &tokens[0]);
            if let Some(second) = tokens.get(1) {
                observe(format!("{last}{KEY_SEPARATOR}{}", tokens[0]), second);
            }
        }
        self.last = Some(sequence_key(tokens));
    }

    /// Learns from the shell's history file, leaving out commands whose name
    /// is a known typing mistake.
    pub fn load_without(shell: Shell, mistyped: &HashSet<String>) -> History {
        let commands: Vec<String> = Self::commands(shell)
            .into_iter()
            .filter(|command| {
                command
                    .split_whitespace()
                    .next()
                    .is_none_or(|name| !mistyped.contains(name))
            })
            .collect();
        Self::from_commands(&commands, shell)
    }

    /// The commands in the shell's history file, oldest first.
    pub fn commands(shell: Shell) -> Vec<String> {
        shell
            .history_file()
            .map(|path| read_history_file(&path, shell))
            .unwrap_or_default()
    }

    /// Records a command run in this session. Fresh commands outweigh
    /// anything read from the history file.
    ///
    /// `status` is the command's exit status when known. A command the shell
    /// could not find is a typing mistake and is not learned at all; one that
    /// failed is learned from less, since many tools fail in normal use.
    pub fn record(&mut self, command: &str, shell: Shell, status: Option<i32>) {
        if let Some(trust) = trust(status) {
            self.add(command, shell, 2.0 * trust);
        }
    }

    fn add(&mut self, command: &str, shell: Shell, weight: f64) {
        let parsed = tokenizer::parse(command.trim(), shell);
        let mut tokens: Vec<String> = parsed.done.into_iter().map(|t| t.text).collect();
        if !parsed.active.text.is_empty() {
            tokens.push(parsed.active.text);
        }
        if tokens.is_empty() {
            return;
        }
        self.observe_sequence(&tokens);
        let mut context = String::new();
        for (depth, token) in tokens.iter().take(MAX_DEPTH + 1).enumerate() {
            if token.len() > 200 {
                break;
            }
            *self
                .next
                .entry(context.clone())
                .or_default()
                .entry(token.clone())
                .or_insert(0.0) += weight;
            if depth > 0 {
                context.push(' ');
            }
            context.push_str(token);
        }
    }

    fn key(context: &[&str]) -> Option<String> {
        (context.len() <= MAX_DEPTH).then(|| context.join(" "))
    }

    /// How often `token` followed `context`, from 0 (never) to 1 (the most
    /// frequent follower).
    pub fn affinity(&self, context: &[&str], token: &str) -> f64 {
        let Some(followers) = Self::key(context).and_then(|key| self.next.get(&key)) else {
            return 0.0;
        };
        let Some(weight) = followers.get(token) else { return 0.0 };
        let max = followers.values().cloned().fold(0.0_f64, f64::max);
        if max > 0.0 {
            weight / max
        } else {
            0.0
        }
    }

    /// The tokens that have followed `context`, most used first, each with
    /// its affinity.
    pub fn followers(&self, context: &[&str]) -> Vec<(&str, f64)> {
        let Some(followers) = Self::key(context).and_then(|key| self.next.get(&key)) else {
            return Vec::new();
        };
        let max = followers
            .values()
            .cloned()
            .fold(0.0_f64, f64::max)
            .max(f64::MIN_POSITIVE);
        let mut list: Vec<(&str, f64)> = followers
            .iter()
            .map(|(token, weight)| (token.as_str(), weight / max))
            .collect();
        list.sort_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.0.cmp(b.0))
        });
        list
    }
}

/// How much a habit from other projects counts where this project has none.
const ELSEWHERE: f64 = 0.4;
/// Most commands kept in hiruzen's own record.
const MAX_RECORDED: usize = 20_000;

/// What the user tends to type: in the project they are in first, and
/// everywhere else as a weaker fallback.
pub struct Recall<'a> {
    pub project: Option<&'a History>,
    pub global: &'a History,
    /// How much the command run just before counts, against overall habit.
    /// Zero ignores it.
    pub sequence: f64,
}

impl Recall<'_> {
    /// How strongly `token` follows `context`, from 0 to 1. A habit in this
    /// project counts in full; one seen only elsewhere counts for less.
    pub fn affinity(&self, context: &[&str], token: &str) -> f64 {
        let here = self.project.map_or(0.0, |project| project.affinity(context, token));
        let habit = here.max(ELSEWHERE * self.global.affinity(context, token));
        // Only the command's name is predicted from what ran before. That is
        // where it has been measured to help; for the word after the name it
        // made the one history it was tried on slightly worse.
        if self.sequence <= 0.0 || !context.is_empty() {
            return habit;
        }
        // What usually follows the command just run, here or anywhere.
        let follows = self
            .project
            .map_or(0.0, |project| project.sequence_affinity(context, token))
            .max(self.global.sequence_affinity(context, token));
        (habit + self.sequence * follows) / (1.0 + self.sequence)
    }

    /// The tokens that have followed `context`, this project's first.
    ///
    /// Habits from other projects are offered only for a command's name and
    /// its first word (`git` then `push`), which mean the same everywhere.
    /// Later words are values such as branch names, hosts and paths, which
    /// belong to the project they were typed in.
    pub fn followers(&self, context: &[&str]) -> Vec<(String, f64)> {
        let mut out: Vec<(String, f64)> = Vec::new();
        if let Some(project) = self.project {
            out.extend(
                project
                    .followers(context)
                    .into_iter()
                    .map(|(token, affinity)| (token.to_string(), affinity)),
            );
        }
        let elsewhere = if context.len() <= 1 {
            self.global.followers(context)
        } else {
            Vec::new()
        };
        for (token, affinity) in elsewhere {
            if !out.iter().any(|(seen, _)| seen == token) {
                out.push((token.to_string(), ELSEWHERE * affinity));
            }
        }
        out
    }
}

/// The directory that identifies the project `cwd` belongs to: the nearest
/// one at or above it holding a `.git`, or `cwd` itself.
pub fn project_root(cwd: &Path) -> PathBuf {
    cwd.ancestors()
        .find(|dir| dir.join(".git").exists())
        .unwrap_or(cwd)
        .to_path_buf()
}

/// hiruzen's own record of commands and the project each was run in. Shell
/// history files do not say where a command ran, so this is what makes
/// ranking specific to a project.
#[derive(Debug, Default)]
pub struct ProjectHistory {
    projects: HashMap<PathBuf, History>,
    /// Where the record is kept; `None` keeps it in memory only.
    file: Option<PathBuf>,
    /// Identifies this run of hiruzen in the record, so commands from
    /// different terminals can be told apart.
    session: String,
    /// Command names the shell could not find, and never could: typing
    /// mistakes. The shell's own history file keeps them, so they are
    /// filtered out of what is learned from it.
    mistyped: HashSet<String>,
    /// Command names that have run at least once.
    found: HashSet<String>,
}

/// Tracks which command names are typing mistakes: not found, and never
/// seen to run. A name that later runs (a tool installed since) is cleared.
fn note_outcome(mistyped: &mut HashSet<String>, found: &mut HashSet<String>, command: &str, status: Option<i32>) {
    let Some(name) = command.split_whitespace().next() else {
        return;
    };
    if status == Some(NOT_FOUND) {
        if !found.contains(name) {
            mistyped.insert(name.to_string());
        }
    } else {
        mistyped.remove(name);
        found.insert(name.to_string());
    }
}

fn unix_time() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

impl ProjectHistory {
    /// Reads the record, oldest first. Each line is
    /// `project<TAB>command<TAB>time<TAB>session<TAB>status<TAB>shell`;
    /// older lines hold fewer fields. A command is split into words by the
    /// rules of the shell it was typed in, which differ: a backslash escapes
    /// a space in bash and is an ordinary character in PowerShell.
    pub fn load(file: PathBuf, shell: Shell) -> ProjectHistory {
        let text = std::fs::read(&file)
            .map(|raw| String::from_utf8_lossy(&raw).into_owned())
            .unwrap_or_default();
        let mut lines: Vec<&str> = text.lines().filter(|line| line.contains('\t')).collect();
        if lines.len() > MAX_RECORDED {
            // Keep the newer half, so the file is trimmed rarely, not on every start.
            lines.drain(..lines.len() - MAX_RECORDED / 2);
            let _ = std::fs::write(&file, lines.join("\n") + "\n");
        }
        let total = lines.len().max(1);
        let mut projects: HashMap<PathBuf, History> = HashMap::new();
        let mut sessions: HashMap<PathBuf, String> = HashMap::new();
        let (mut mistyped, mut found) = (HashSet::new(), HashSet::new());
        for (i, line) in lines.iter().enumerate() {
            let mut fields = line.split('\t');
            let (Some(project), Some(command)) = (fields.next(), fields.next()) else {
                continue;
            };
            let _time = fields.next();
            let session = fields.next().unwrap_or("");
            let status = fields.next().and_then(|status| status.parse::<i32>().ok());
            let typed_in = fields.next().and_then(Shell::from_name).unwrap_or(shell);
            note_outcome(&mut mistyped, &mut found, command, status);
            let Some(trust) = trust(status) else { continue };
            let project = PathBuf::from(project);
            let history = projects.entry(project.clone()).or_default();
            // One terminal's commands are a sequence; another terminal's
            // command in between is not what came "before".
            if sessions.get(&project).is_some_and(|previous| previous != session) {
                history.last = None;
            }
            sessions.insert(project, session.to_string());
            // Older entries count for less: weights run from 0.25 up to 1.
            let recency = 0.25 + 0.75 * (i + 1) as f64 / total as f64;
            history.add(command, typed_in, recency * trust);
        }
        for history in projects.values_mut() {
            history.last = None;
        }
        ProjectHistory {
            projects,
            file: Some(file),
            session: format!("{}-{}", std::process::id(), unix_time()),
            mistyped,
            found,
        }
    }

    /// Command names known to be typing mistakes.
    pub fn mistyped(&self) -> &HashSet<String> {
        &self.mistyped
    }

    pub fn get(&self, project: &Path) -> Option<&History> {
        self.projects.get(project)
    }

    /// Notes a command run in `project`, in memory and in the record.
    pub fn record(&mut self, project: &Path, command: &str, shell: Shell, status: Option<i32>) {
        let command = command.trim_end();
        // A leading space is the usual way to ask that a command not be
        // remembered. Multi-line commands do not fit the one-line record.
        if command.is_empty() || command.starts_with(' ') || command.contains(['\n', '\t']) {
            return;
        }
        note_outcome(&mut self.mistyped, &mut self.found, command, status);
        self.projects
            .entry(project.to_path_buf())
            .or_default()
            .record(command, shell, status);
        let Some(file) = &self.file else { return };
        if let Some(dir) = file.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let mut options = std::fs::OpenOptions::new();
        options.create(true).append(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            // Commands can hold secrets; the record is readable by its owner only.
            options.mode(0o600);
        }
        if let Ok(mut handle) = options.open(file) {
            use std::io::Write;
            let status = status.map(|status| status.to_string()).unwrap_or_default();
            let _ = writeln!(
                handle,
                "{}\t{}\t{}\t{}\t{}\t{}",
                project.display(),
                command,
                unix_time(),
                self.session,
                status,
                shell.name()
            );
        }
    }
}

fn read_history_file(path: &Path, shell: Shell) -> Vec<String> {
    let Ok(bytes) = std::fs::read(path) else {
        return Vec::new();
    };
    let text = String::from_utf8_lossy(&bytes);
    let mut commands: Vec<String> = match shell {
        Shell::Fish => text
            .lines()
            .filter_map(|line| line.strip_prefix("- cmd: "))
            .map(|cmd| cmd.replace("\\n", "\n").replace("\\\\", "\\"))
            .collect(),
        Shell::Zsh => text.lines().map(strip_zsh_timestamp).map(str::to_string).collect(),
        _ => text
            .lines()
            .filter(|line| !line.starts_with('#'))
            .map(str::to_string)
            .collect(),
    };
    commands.retain(|c| !c.trim().is_empty());
    if commands.len() > MAX_LINES {
        commands.drain(..commands.len() - MAX_LINES);
    }
    commands
}

/// zsh's extended history prefixes each line with `: <start>:<elapsed>;`.
fn strip_zsh_timestamp(line: &str) -> &str {
    if let Some(rest) = line.strip_prefix(": ") {
        if let Some((meta, command)) = rest.split_once(';') {
            if meta.chars().all(|c| c.is_ascii_digit() || c == ':') {
                return command;
            }
        }
    }
    line
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn learns_what_follows_a_context() {
        let mut history = History::default();
        for _ in 0..3 {
            history.record("git checkout main", Shell::Zsh, None);
        }
        history.record("git commit -m x", Shell::Zsh, None);
        assert_eq!(history.affinity(&["git"], "checkout"), 1.0);
        assert!(history.affinity(&["git"], "commit") < 1.0);
        assert_eq!(history.affinity(&["git"], "push"), 0.0);
        assert_eq!(history.followers(&["git", "checkout"])[0].0, "main");
        assert_eq!(history.followers(&[])[0].0, "git");
    }

    #[test]
    fn a_project_habit_outranks_a_habit_from_elsewhere() {
        let mut global = History::default();
        for _ in 0..5 {
            global.record("git push origin main", Shell::Zsh, None);
        }
        let mut project = History::default();
        project.record("git push origin release", Shell::Zsh, None);
        let recall = Recall {
            project: Some(&project),
            global: &global,
            sequence: 0.0,
        };
        let context = ["git", "push", "origin"];
        assert!(recall.affinity(&context, "release") > recall.affinity(&context, "main"));
        let followers = recall.followers(&context);
        // A branch name typed in another project is not offered here...
        assert_eq!(followers, vec![("release".to_string(), 1.0)]);
        // ...but a subcommand habit from elsewhere is.
        assert_eq!(recall.followers(&["git"])[0].0, "push");
        // Without project history the global habit still counts.
        let elsewhere = Recall {
            project: None,
            global: &global,
            sequence: 0.0,
        };
        assert!(elsewhere.affinity(&context, "main") > 0.0);
    }

    #[test]
    fn records_commands_per_project_and_reads_them_back() {
        let dir = std::env::temp_dir().join(format!("hiruzen-record-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let file = dir.join("history.tsv");
        let (a, b) = (PathBuf::from("/work/a"), PathBuf::from("/work/b"));
        let mut record = ProjectHistory::load(file.clone(), Shell::Zsh);
        record.record(&a, "make deploy", Shell::Zsh, None);
        record.record(&b, "make test", Shell::Zsh, None);
        record.record(&a, " secret --token x", Shell::Zsh, None);
        let reloaded = ProjectHistory::load(file, Shell::Zsh);
        assert_eq!(reloaded.get(&a).unwrap().followers(&["make"])[0].0, "deploy");
        assert_eq!(reloaded.get(&b).unwrap().followers(&["make"])[0].0, "test");
        assert!(reloaded.get(&a).unwrap().followers(&["secret"]).is_empty());
        assert!(reloaded.get(Path::new("/work/c")).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn finds_the_project_a_directory_belongs_to() {
        let root = std::env::temp_dir().join(format!("hiruzen-root-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::create_dir_all(root.join("src").join("deep")).unwrap();
        assert_eq!(project_root(&root.join("src").join("deep")), root);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn learns_what_follows_the_previous_command() {
        let mut history = History::default();
        for _ in 0..3 {
            for command in ["git add .", "git commit -m x", "git push", "ls"] {
                history.record(command, Shell::Zsh, None);
            }
        }
        history.record("git add .", Shell::Zsh, None);
        // After `git add`, the next command is `git`, and then `commit`.
        assert_eq!(history.sequence_affinity(&[], "git"), 1.0);
        assert_eq!(history.sequence_affinity(&[], "ls"), 0.0);
        assert_eq!(history.sequence_affinity(&["git"], "commit"), 1.0);
        assert_eq!(history.sequence_affinity(&["git"], "push"), 0.0);
        // `git` and `ls` are both common; after `git add` the sequence favours `git`.
        let with = Recall {
            project: None,
            global: &history,
            sequence: 2.0,
        };
        let without = Recall {
            project: None,
            global: &history,
            sequence: 0.0,
        };
        let gap = |recall: &Recall| recall.affinity(&[], "git") - recall.affinity(&[], "ls");
        assert!(gap(&with) > gap(&without));
        // The word after the name is ranked by habit alone.
        assert_eq!(with.affinity(&["git"], "commit"), without.affinity(&["git"], "commit"));
        // Words past the second are not predicted from the sequence.
        assert_eq!(history.sequence_affinity(&["git", "commit"], "-m"), 0.0);
    }

    #[test]
    fn a_recent_change_of_habit_wins_over_an_old_one() {
        let mut history = History::default();
        for _ in 0..10 {
            history.record("make build", Shell::Zsh, None);
            history.record("make test", Shell::Zsh, None);
        }
        for _ in 0..3 {
            history.record("make build", Shell::Zsh, None);
            history.record("make deploy", Shell::Zsh, None);
        }
        history.record("make build", Shell::Zsh, None);
        assert!(history.sequence_affinity(&["make"], "deploy") > history.sequence_affinity(&["make"], "test"));
    }

    #[test]
    fn typos_are_not_learned_and_failures_count_for_less() {
        let mut history = History::default();
        history.record("gti status", Shell::Zsh, Some(127));
        assert!(history.followers(&[]).is_empty());
        history.record("make build", Shell::Zsh, Some(0));
        history.record("make broken", Shell::Zsh, Some(2));
        assert!(history.affinity(&["make"], "build") > history.affinity(&["make"], "broken"));
        assert!(history.affinity(&["make"], "broken") > 0.0);
    }

    #[test]
    fn the_record_keeps_exit_status_and_separates_sessions() {
        let dir = std::env::temp_dir().join(format!("hiruzen-status-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("history.tsv");
        std::fs::write(
            &file,
            "/p\told format\n/p\tgti push\t100\ts1\t127\n/p\tgit add .\t101\ts1\t0\n/p\tls\t102\ts2\t0\n/p\tgit commit\t103\ts1\t0\n",
        )
        .unwrap();
        let record = ProjectHistory::load(file.clone(), Shell::Zsh);
        let project = record.get(Path::new("/p")).unwrap();
        let names: Vec<&str> = project.followers(&[]).into_iter().map(|(name, _)| name).collect();
        assert!(
            names.contains(&"old") && names.contains(&"git") && names.contains(&"ls"),
            "{names:?}"
        );
        assert!(!names.contains(&"gti"), "{names:?}");
        assert!(record.mistyped().contains("gti") && !record.mistyped().contains("git"));
        // `ls` came from another terminal, so nothing is learned about what
        // follows `git add` from it.
        assert!(!project.after.contains_key("git add"));
        let mut record = record;
        record.record(Path::new("/p"), "make test", Shell::Zsh, Some(1));
        let text = std::fs::read_to_string(&file).unwrap();
        let last: Vec<&str> = text.lines().last().unwrap().split('\t').collect();
        assert_eq!((last[0], last[1], last[4]), ("/p", "make test", "1"));
        assert_eq!((last.len(), last[5]), (6, "zsh"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_command_is_read_by_the_rules_of_the_shell_it_was_typed_in() {
        let dir = std::env::temp_dir().join(format!("hiruzen-shells-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("history.tsv");
        // Typed in bash, where the backslash joins the two words into one.
        std::fs::write(&file, "/p\tcat my\\ file.txt\t1\ts\t0\tbash\n").unwrap();
        let record = ProjectHistory::load(file, Shell::Pwsh);
        let followers: Vec<&str> = record
            .get(Path::new("/p"))
            .unwrap()
            .followers(&["cat"])
            .into_iter()
            .map(|f| f.0)
            .collect();
        assert_eq!(followers, vec!["my file.txt"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn strips_zsh_extended_history_prefix() {
        assert_eq!(strip_zsh_timestamp(": 1700000000:0;git status"), "git status");
        assert_eq!(strip_zsh_timestamp("git status"), "git status");
        assert_eq!(strip_zsh_timestamp(": not a stamp;x"), ": not a stamp;x");
    }
}
