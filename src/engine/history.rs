//! What the user actually runs, learned from their shell history. It ranks
//! spec suggestions and supplies suggestions for commands that have no spec.

use std::collections::HashMap;
use std::path::Path;

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
}

impl History {
    pub fn load(shell: Shell) -> History {
        let mut history = History::default();
        let Some(path) = shell.history_file() else {
            return history;
        };
        let commands = read_history_file(&path, shell);
        let total = commands.len().max(1);
        for (i, command) in commands.iter().enumerate() {
            // Older entries count for less: weights run from 0.25 up to 1.
            let weight = 0.25 + 0.75 * (i + 1) as f64 / total as f64;
            history.add(command, shell, weight);
        }
        history
    }

    /// Records a command run in this session. Fresh commands outweigh
    /// anything read from the history file.
    pub fn record(&mut self, command: &str, shell: Shell) {
        self.add(command, shell, 2.0);
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
            history.record("git checkout main", Shell::Zsh);
        }
        history.record("git commit -m x", Shell::Zsh);
        assert_eq!(history.affinity(&["git"], "checkout"), 1.0);
        assert!(history.affinity(&["git"], "commit") < 1.0);
        assert_eq!(history.affinity(&["git"], "push"), 0.0);
        assert_eq!(history.followers(&["git", "checkout"])[0].0, "main");
        assert_eq!(history.followers(&[])[0].0, "git");
    }

    #[test]
    fn strips_zsh_extended_history_prefix() {
        assert_eq!(strip_zsh_timestamp(": 1700000000:0;git status"), "git status");
        assert_eq!(strip_zsh_timestamp("git status"), "git status");
        assert_eq!(strip_zsh_timestamp(": not a stamp;x"), ": not a stamp;x");
    }
}
