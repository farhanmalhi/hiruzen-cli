//! `hiruzen learn`: builds a completion spec for a tool from its `--help`
//! output, for tools the built-in library does not cover.
//!
//! It understands the layout most command-line frameworks print: sections
//! such as "Options:" and "Commands:" whose entries are indented, each with
//! a description. Subcommands are followed by running their own `--help`.
//! Only `--help` is ever run, and only when the user asks for it.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::engine::generators::{Runner, SystemRunner};
use crate::settings;
use crate::shell;
use crate::specs::SpecStore;

const MAX_DESCRIPTION: usize = 140;
/// Subcommand levels followed below the tool itself.
const MAX_DEPTH: usize = 2;
/// Most `--help` runs for one tool.
const MAX_RUNS: usize = 80;

pub fn specs_dir() -> Option<PathBuf> {
    Some(settings::config_dir()?.join("specs"))
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('.')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '+'))
}

/// Section title to its entries, each a heading (the option or command as
/// printed) and its description.
fn sections(text: &str) -> HashMap<String, Vec<(String, String)>> {
    let mut found: HashMap<String, Vec<(String, String)>> = HashMap::new();
    let mut title: Option<String> = None;
    // Entries of a section share one indent; deeper lines continue a description.
    let mut entry_indent: Option<usize> = None;
    for line in text.lines() {
        let trimmed = line.trim_end();
        if trimmed.trim().is_empty() {
            continue;
        }
        let indent = trimmed.len() - trimmed.trim_start().len();
        let heading = indent == 0 && trimmed.ends_with(':') && trimmed.len() < 40 && !trimmed.contains("  ");
        if heading {
            title = Some(trimmed.trim_end_matches(':').to_lowercase());
            entry_indent = None;
            continue;
        }
        let Some(current) = &title else { continue };
        if indent == 0 {
            // Prose between sections ends the section.
            title = None;
            continue;
        }
        let entries = found.entry(current.clone()).or_default();
        // Frameworks indent an option that has no short form a little
        // further, to line its name up with the others'.
        let aligned_option =
            trimmed.trim_start().starts_with("--") && entry_indent.is_some_and(|expected| indent <= expected + 6);
        if aligned_option || entry_indent.is_none_or(|expected| indent <= expected) {
            if !aligned_option {
                entry_indent = Some(indent);
            }
            let body = trimmed.trim_start();
            let (head, description) = match body.find("  ") {
                Some(at) => (body[..at].trim(), body[at..].trim()),
                None => (body, ""),
            };
            entries.push((head.to_string(), description.to_string()));
        } else if let Some(last) = entries.last_mut() {
            if !last.1.is_empty() {
                last.1.push(' ');
            }
            last.1.push_str(trimmed.trim());
        }
    }
    found
}

fn shorten(description: &str) -> String {
    let joined = description.split_whitespace().collect::<Vec<_>>().join(" ");
    let first = joined
        .split_inclusive(['.', '!', '?'])
        .next()
        .unwrap_or("")
        .trim_end_matches('.');
    let mut out: String = first.chars().take(MAX_DESCRIPTION).collect();
    if first.chars().count() > MAX_DESCRIPTION {
        out.push('…');
    }
    out
}

/// `<name>`, `[name]` and `<name>...` in a usage fragment, as spec arguments.
fn arguments(text: &str) -> Vec<Value> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find(['<', '[']) {
        let close = if rest.as_bytes()[start] == b'<' { '>' } else { ']' };
        let Some(len) = rest[start + 1..].find(close) else {
            break;
        };
        let name = rest[start + 1..start + 1 + len].trim().to_string();
        let after = &rest[start + len + 2..];
        if !name.is_empty()
            && !name.eq_ignore_ascii_case("options")
            && !name.eq_ignore_ascii_case("flags")
            && !name.starts_with('-')
        {
            let mut arg = json!({ "name": name.trim_end_matches('.') });
            if name.ends_with("...") || after.starts_with("...") {
                arg["isVariadic"] = json!(true);
            }
            if close == ']' {
                arg["isOptional"] = json!(true);
            }
            out.push(arg);
        }
        rest = after;
    }
    out
}

fn option(head: &str, description: &str) -> Option<Value> {
    let mut names: Vec<String> = Vec::new();
    let mut usage = String::new();
    for part in head.split(", ") {
        let part = part.trim();
        if !part.starts_with('-') {
            continue;
        }
        let end = part.find([' ', '=', '<', '[']).unwrap_or(part.len());
        let name = &part[..end];
        if name.len() > 1
            && name
                .trim_start_matches('-')
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        {
            names.push(name.to_string());
            if end < part.len() {
                usage = part[end..].to_string();
            }
        }
    }
    if names.is_empty() {
        return None;
    }
    let mut spec = json!({ "description": shorten(description) });
    spec["name"] = if names.len() == 1 {
        json!(names[0])
    } else {
        json!(names)
    };
    let mut args = arguments(&usage);
    // `--color WHEN` and `--color=WHEN` name the value without brackets.
    if args.is_empty() {
        let value = usage.trim_start_matches(['=', ' ']).trim();
        if !value.is_empty() && value.chars().all(|c| c.is_ascii_uppercase() || matches!(c, '_' | '-')) {
            args.push(json!({ "name": value.to_lowercase() }));
        }
    }
    if let Some(first) = args.first_mut() {
        // "(choices: a, b)" or "[possible values: a, b]" lists what it accepts.
        for marker in ["choices:", "possible values:"] {
            if let Some(at) = description.find(marker) {
                let list = description[at + marker.len()..].split([')', ']']).next().unwrap_or("");
                let values: Vec<&str> = list
                    .split(',')
                    .map(|v| v.trim().trim_matches('"'))
                    .filter(|v| !v.is_empty())
                    .collect();
                if !values.is_empty() {
                    first["suggestions"] = json!(values);
                }
            }
        }
        spec["args"] = if args.len() == 1 { args.remove(0) } else { json!(args) };
    }
    Some(spec)
}

struct Learner<'a> {
    runner: &'a dyn Runner,
    cwd: &'a Path,
    runs: usize,
}

impl Learner<'_> {
    fn help(&mut self, argv: &[String]) -> String {
        if self.runs >= MAX_RUNS {
            return String::new();
        }
        self.runs += 1;
        let mut command = argv.to_vec();
        command.push("--help".to_string());
        let output = self.runner.exec(&command, self.cwd, &[]);
        // Some tools print help on standard error.
        if output.stdout.trim().is_empty() {
            output.stderr
        } else {
            output.stdout
        }
    }

    fn build(&mut self, argv: &[String], description: &str, depth: usize, parent_help: &str) -> Value {
        let text = self.help(argv);
        // A tool that ignores the subcommand prints its top-level help again.
        // Reading that as the subcommand's own would nest the whole tool
        // inside each of its commands.
        let found = if text.trim() == parent_help.trim() {
            HashMap::new()
        } else {
            sections(&text)
        };
        let text_of_parent = text.clone();
        let mut spec = json!({ "name": argv.last().cloned().unwrap_or_default(), "description": shorten(description) });
        let mut options: Vec<Value> = Vec::new();
        let mut subcommands: Vec<Value> = Vec::new();
        let mut titles: Vec<&String> = found.keys().collect();
        titles.sort();
        for title in titles {
            let entries = &found[title];
            if title.contains("option") || title.contains("flag") {
                options.extend(entries.iter().filter_map(|(head, text)| option(head, text)));
            } else if title.contains("command") {
                for (head, text) in entries {
                    let mut words = head.split_whitespace();
                    let Some(first) = words.next() else { continue };
                    let names: Vec<&str> = first
                        .split(['|', ','])
                        .map(str::trim)
                        .filter(|n| valid_name(n))
                        .collect();
                    let Some(name) = names.first() else { continue };
                    if *name == "help" {
                        continue;
                    }
                    let mut sub = if depth < MAX_DEPTH {
                        let mut deeper = argv.to_vec();
                        deeper.push(name.to_string());
                        self.build(&deeper, text, depth + 1, &text_of_parent)
                    } else {
                        json!({ "description": shorten(text) })
                    };
                    sub["name"] = if names.len() == 1 {
                        json!(names[0])
                    } else {
                        json!(names)
                    };
                    let mut args = arguments(&words.collect::<Vec<_>>().join(" "));
                    if !args.is_empty() {
                        sub["args"] = if args.len() == 1 { args.remove(0) } else { json!(args) };
                    }
                    subcommands.push(sub);
                }
            }
        }
        if !options.is_empty() {
            spec["options"] = json!(options);
        }
        if !subcommands.is_empty() {
            spec["subcommands"] = json!(subcommands);
        }
        spec
    }
}

/// Builds a spec for `command` by reading its help.
pub fn build_spec(command: &str, runner: &dyn Runner, cwd: &Path) -> Option<Value> {
    let mut learner = Learner { runner, cwd, runs: 0 };
    let spec = learner.build(&[command.to_string()], "", 0, "");
    (spec.get("options").is_some() || spec.get("subcommands").is_some()).then_some(spec)
}

fn count(spec: &Value, key: &str) -> usize {
    let own = spec.get(key).and_then(Value::as_array).map_or(0, Vec::len);
    let nested: usize = spec
        .get("subcommands")
        .and_then(Value::as_array)
        .map_or(0, |subs| subs.iter().map(|sub| count(sub, key)).sum());
    own + nested
}

/// `hiruzen learn <COMMAND> | --list | --forget <COMMAND>`
pub fn learn(args: &[String]) -> Result<i32, String> {
    let dir = specs_dir().ok_or("cannot work out where learned specs are kept")?;
    let words: Vec<&str> = args.iter().map(String::as_str).collect();
    match words.as_slice() {
        ["--list"] => {
            let mut names: Vec<String> = std::fs::read_dir(&dir)
                .into_iter()
                .flatten()
                .flatten()
                .filter_map(|entry| {
                    entry
                        .file_name()
                        .to_string_lossy()
                        .strip_suffix(".json")
                        .map(str::to_string)
                })
                .collect();
            names.sort();
            if names.is_empty() {
                println!("Nothing learned yet. Teach hiruzen a tool with `hiruzen learn <command>`.");
            }
            for name in names {
                println!("{name}");
            }
            Ok(0)
        }
        ["--forget", name] if valid_name(name) => {
            std::fs::remove_file(dir.join(format!("{name}.json")))
                .map_err(|_| format!("nothing was learned for '{name}'"))?;
            println!("Forgot {name}");
            Ok(0)
        }
        [name] if valid_name(name) => {
            if SpecStore::embedded().has_builtin(name) {
                return Err(format!(
                    "hiruzen already has a spec for '{name}', which is more complete than one read from its help"
                ));
            }
            if shell::find_on_path(name).is_none() {
                return Err(format!("'{name}' was not found on PATH"));
            }
            let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
            let spec = build_spec(name, &SystemRunner, &cwd)
                .ok_or_else(|| format!("could not find options or commands in the output of `{name} --help`"))?;
            std::fs::create_dir_all(&dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
            let path = dir.join(format!("{name}.json"));
            let text = serde_json::to_string_pretty(&spec).map_err(|e| e.to_string())?;
            std::fs::write(&path, text + "\n").map_err(|e| format!("cannot write {}: {e}", path.display()))?;
            println!(
                "Learned {name}: {} commands and {} options. Saved to {}",
                count(&spec, "subcommands"),
                count(&spec, "options"),
                path.display()
            );
            Ok(0)
        }
        _ => Err("usage: hiruzen learn <COMMAND> | --list | --forget <COMMAND>".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::generators::Output;

    struct Help;

    impl Runner for Help {
        fn run(&self, _argv: &[String], _cwd: &Path) -> Option<String> {
            None
        }
        fn exec(&self, argv: &[String], _cwd: &Path, _env: &[(String, Option<String>)]) -> Output {
            let stdout = match argv.join(" ").as_str() {
                "tool --help" => {
                    "Usage: tool [OPTIONS] <COMMAND>\n\nA tool.\n\nCommands:\n  build [options] <target>  Build a target. More words.\n  deploy|ship <env>         Send it somewhere\n  help                      Print help\n\nOptions:\n  -v, --verbose             Say more\n      --color <WHEN>        Colour output (choices: \"auto\", \"always\", \"never\")\n  -o, --output=FILE         Where to write\n                            the result\n"
                }
                "tool build --help" => "Usage: tool build\n\nFlags:\n    --release    Optimise\n",
                _ => "",
            };
            Output {
                stdout: stdout.to_string(),
                ..Output::default()
            }
        }
    }

    #[test]
    fn builds_a_spec_from_help_text() {
        let spec = build_spec("tool", &Help, Path::new(".")).unwrap();
        let commands = spec["subcommands"].as_array().unwrap();
        assert_eq!(commands.len(), 2, "{commands:?}");
        assert_eq!(commands[0]["name"], "build");
        assert_eq!(commands[0]["description"], "Build a target");
        assert_eq!(commands[0]["args"]["name"], "target");
        assert_eq!(commands[0]["options"][0]["name"], "--release");
        assert_eq!(commands[1]["name"], json!(["deploy", "ship"]));
        let options = spec["options"].as_array().unwrap();
        assert_eq!(options[0]["name"], json!(["-v", "--verbose"]));
        assert!(options[0].get("args").is_none());
        assert_eq!(options[1]["args"]["suggestions"], json!(["auto", "always", "never"]));
        assert_eq!(options[2]["name"], json!(["-o", "--output"]));
        assert_eq!(options[2]["args"]["name"], "file");
        assert_eq!(options[2]["description"], "Where to write the result");
    }

    #[test]
    fn a_tool_that_repeats_its_help_for_subcommands_is_not_nested_in_itself() {
        struct Same;
        impl Runner for Same {
            fn run(&self, _argv: &[String], _cwd: &Path) -> Option<String> {
                None
            }
            fn exec(&self, _argv: &[String], _cwd: &Path, _env: &[(String, Option<String>)]) -> Output {
                Output {
                    stdout:
                        "Commands:\n  launch  Launch it\n  status  Show status\n\nOptions:\n  -q, --quiet  Say less\n"
                            .to_string(),
                    ..Output::default()
                }
            }
        }
        let spec = build_spec("same", &Same, Path::new(".")).unwrap();
        assert_eq!(count(&spec, "subcommands"), 2);
        assert_eq!(count(&spec, "options"), 1);
    }

    #[test]
    fn reports_a_tool_whose_help_has_nothing_usable() {
        assert!(build_spec("silent", &Help, Path::new(".")).is_none());
    }

    #[test]
    fn a_learned_spec_parses_as_a_spec() {
        let spec = build_spec("tool", &Help, Path::new(".")).unwrap();
        let parsed: crate::specs::Subcommand = serde_json::from_value(spec).unwrap();
        assert_eq!(parsed.subcommands.len(), 2);
        assert_eq!(parsed.options.len(), 3);
    }
}
