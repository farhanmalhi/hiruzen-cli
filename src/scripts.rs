//! Saved scripts: commands with named blanks, kept as plain files and run
//! with `hiruzen run`.
//!
//! A script is a text file. Comment lines at the top describe it:
//!
//! ```text
//! # description: Restart a service
//! # param: service - Which service (choices: api, web)
//! # param: env=staging - Environment to act on
//! kubectl rollout restart deploy/{{service}} -n {{env}}
//! ```

use std::collections::HashMap;
use std::io::{BufRead, IsTerminal, Write};
use std::path::PathBuf;
use std::process::Command;

use crate::settings;
use crate::shell::Shell;

#[derive(Debug, PartialEq, Default)]
pub struct Param {
    pub name: String,
    pub default: Option<String>,
    pub help: Option<String>,
    pub choices: Vec<String>,
}

#[derive(Debug, PartialEq, Default)]
pub struct Script {
    pub description: Option<String>,
    pub params: Vec<Param>,
    /// The command text, with `{{name}}` where parameters go.
    pub body: String,
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('.')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/// Every `{{name}}` in `text`, in order of first appearance.
fn placeholders(text: &str) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find("{{") {
        let after = &rest[start + 2..];
        let Some(end) = after.find("}}") else { break };
        let name = after[..end].trim();
        if valid_name(name) && !found.iter().any(|f| f == name) {
            found.push(name.to_string());
        }
        rest = &after[end + 2..];
    }
    found
}

/// `NAME[=DEFAULT] [- help] [(choices: a, b)]`
fn parse_param(text: &str) -> Option<Param> {
    let mut text = text.trim().to_string();
    let mut choices = Vec::new();
    if let (Some(open), true) = (text.rfind("(choices:"), text.ends_with(')')) {
        choices = text[open + "(choices:".len()..text.len() - 1]
            .split(',')
            .map(|c| c.trim().to_string())
            .filter(|c| !c.is_empty())
            .collect();
        text.truncate(open);
    }
    let (head, help) = match text.split_once(" - ") {
        Some((head, help)) => (head.trim(), Some(help.trim().to_string()).filter(|h| !h.is_empty())),
        None => (text.trim(), None),
    };
    let (name, default) = match head.split_once('=') {
        Some((name, default)) => (name.trim(), Some(default.trim().to_string())),
        None => (head, None),
    };
    valid_name(name).then(|| Param {
        name: name.to_string(),
        default,
        help,
        choices,
    })
}

pub fn parse(text: &str) -> Script {
    let mut script = Script::default();
    let mut body: Vec<&str> = Vec::new();
    let mut in_header = true;
    for (index, line) in text.lines().enumerate() {
        if in_header {
            if index == 0 && line.starts_with("#!") {
                body.push(line);
                continue;
            }
            if let Some(comment) = line.strip_prefix('#') {
                let comment = comment.trim();
                if let Some(description) = comment.strip_prefix("description:") {
                    script.description = Some(description.trim().to_string());
                } else if let Some(param) = comment.strip_prefix("param:") {
                    script.params.extend(parse_param(param));
                }
                continue;
            }
            in_header = false;
        }
        body.push(line);
    }
    script.body = body.join("\n").trim().to_string();
    // A blank used in the body but never declared is still a parameter.
    for name in placeholders(&script.body) {
        if !script.params.iter().any(|p| p.name == name) {
            script.params.push(Param {
                name,
                ..Param::default()
            });
        }
    }
    script
}

/// Fills the blanks. Returns the names still missing when values are incomplete.
pub fn render(script: &Script, values: &HashMap<String, String>) -> Result<String, Vec<String>> {
    let mut missing = Vec::new();
    let mut text = script.body.clone();
    for param in &script.params {
        match values.get(&param.name).or(param.default.as_ref()) {
            Some(value) => {
                for form in [format!("{{{{{}}}}}", param.name), format!("{{{{ {} }}}}", param.name)] {
                    text = text.replace(&form, value);
                }
            }
            None => missing.push(param.name.clone()),
        }
    }
    if missing.is_empty() {
        Ok(text)
    } else {
        Err(missing)
    }
}

fn scripts_dir() -> Result<PathBuf, String> {
    Ok(settings::config_dir()
        .ok_or("cannot work out where scripts are kept")?
        .join("scripts"))
}

fn script_path(name: &str) -> Result<PathBuf, String> {
    if !valid_name(name) {
        return Err(format!(
            "'{name}' is not a usable script name; use letters, digits, '-', '_' and '.'"
        ));
    }
    Ok(scripts_dir()?.join(name))
}

fn load(name: &str) -> Result<Script, String> {
    let path = script_path(name)?;
    let text = std::fs::read_to_string(&path).map_err(|_| {
        format!("no script named '{name}'; run `hiruzen run` to list them, or `hiruzen scripts new {name}`")
    })?;
    Ok(parse(&text))
}

fn names() -> Vec<String> {
    let Ok(dir) = scripts_dir() else { return Vec::new() };
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| entry.path().is_file())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| valid_name(name))
        .collect();
    names.sort();
    names
}

fn list(names_only: bool) -> Result<i32, String> {
    let all = names();
    if all.is_empty() && !names_only {
        println!("No scripts yet. Create one with `hiruzen scripts new <name>`.");
        return Ok(0);
    }
    for name in all {
        match load(&name)
            .ok()
            .and_then(|script| script.description)
            .filter(|_| !names_only)
        {
            Some(description) => println!("{name:<24} {description}"),
            None => println!("{name}"),
        }
    }
    Ok(0)
}

fn ask(param: &Param) -> Option<String> {
    let mut prompt = param.name.clone();
    if let Some(help) = &param.help {
        prompt.push_str(&format!(" ({help})"));
    }
    if !param.choices.is_empty() {
        prompt.push_str(&format!(" [{}]", param.choices.join(", ")));
    }
    if let Some(default) = &param.default {
        prompt.push_str(&format!(" (default: {default})"));
    }
    loop {
        eprint!("{prompt}: ");
        let _ = std::io::stderr().flush();
        let mut line = String::new();
        if std::io::stdin().lock().read_line(&mut line).ok()? == 0 {
            return None;
        }
        let answer = line.trim();
        let value = if answer.is_empty() {
            param.default.clone()?
        } else {
            answer.to_string()
        };
        if param.choices.is_empty() || param.choices.contains(&value) {
            return Some(value);
        }
        eprintln!("Choose one of: {}", param.choices.join(", "));
    }
}

/// `hiruzen run [NAME [key=value...] [--print]]`
pub fn run(args: &[String]) -> Result<i32, String> {
    let Some((name, rest)) = args.split_first() else {
        return list(false);
    };
    let script = load(name)?;
    if script.body.is_empty() {
        return Err(format!("script '{name}' has no command in it"));
    }
    let mut print_only = false;
    let mut values: HashMap<String, String> = HashMap::new();
    for arg in rest {
        if arg == "--print" || arg == "-p" {
            print_only = true;
        } else if let Some((key, value)) = arg.split_once('=') {
            match script.params.iter().find(|p| p.name == key) {
                Some(param) if !param.choices.is_empty() && !param.choices.iter().any(|c| c == value) => {
                    return Err(format!("{key} must be one of: {}", param.choices.join(", ")));
                }
                Some(_) => {
                    values.insert(key.to_string(), value.to_string());
                }
                None => return Err(format!("script '{name}' has no parameter '{key}'")),
            }
        } else {
            return Err(format!("expected key=value or --print, found '{arg}'"));
        }
    }

    // Ask for whatever was not given, when there is someone to ask.
    if std::io::stdin().is_terminal() {
        for param in &script.params {
            if !values.contains_key(&param.name) {
                if let Some(value) = ask(param) {
                    values.insert(param.name.clone(), value);
                }
            }
        }
    }
    let command = render(&script, &values)
        .map_err(|missing| format!("missing {}; give them as key=value", missing.join(", ")))?;
    if print_only {
        println!("{command}");
        return Ok(0);
    }

    let shell = Shell::detect();
    let program = crate::shell::find_on_path(shell.name()).unwrap_or_else(|| PathBuf::from(shell.name()));
    let mut process = Command::new(program);
    match shell {
        Shell::Pwsh | Shell::Powershell => process.args(["-NoLogo", "-NoProfile", "-Command", &command]),
        Shell::Cmd => process.args(["/C", &command]),
        _ => process.args(["-c", &command]),
    };
    let status = process
        .status()
        .map_err(|e| format!("cannot start {}: {e}", shell.name()))?;
    Ok(status.code().unwrap_or(1))
}

const TEMPLATE: &str =
    "# description: What this script does\n# param: name=world - Who to greet\necho \"Hello, {{name}}\"\n";

fn edit(path: &std::path::Path) {
    let editor = std::env::var("VISUAL")
        .or_else(|_| std::env::var("EDITOR"))
        .ok()
        .filter(|e| !e.trim().is_empty());
    match editor {
        Some(editor) if std::io::stdin().is_terminal() => {
            // The variable may carry arguments, as in `code --wait`.
            let mut words = editor.split_whitespace();
            if let Some(program) = words.next() {
                let _ = Command::new(program).args(words).arg(path).status();
            }
        }
        _ => println!("Edit it at {}", path.display()),
    }
}

/// `hiruzen scripts <list|new|edit|show|remove|dir> [NAME]`
pub fn scripts(args: &[String]) -> Result<i32, String> {
    let words: Vec<&str> = args.iter().map(String::as_str).collect();
    match words.as_slice() {
        [] | ["list"] => list(false),
        ["list", "--names"] => list(true),
        ["dir"] => {
            println!("{}", scripts_dir()?.display());
            Ok(0)
        }
        ["new", name] => {
            let path = script_path(name)?;
            if path.exists() {
                return Err(format!(
                    "a script named '{name}' already exists; use `hiruzen scripts edit {name}`"
                ));
            }
            let dir = scripts_dir()?;
            std::fs::create_dir_all(&dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
            std::fs::write(&path, TEMPLATE).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
            println!("Created {}", path.display());
            edit(&path);
            Ok(0)
        }
        ["edit", name] => {
            let path = script_path(name)?;
            if !path.is_file() {
                return Err(format!("no script named '{name}'"));
            }
            edit(&path);
            Ok(0)
        }
        ["show", name] => {
            let script = load(name)?;
            if let Some(description) = &script.description {
                println!("{description}\n");
            }
            for param in &script.params {
                let mut line = format!("  {}", param.name);
                if let Some(default) = &param.default {
                    line.push_str(&format!(" (default: {default})"));
                }
                if !param.choices.is_empty() {
                    line.push_str(&format!(" [{}]", param.choices.join(", ")));
                }
                if let Some(help) = &param.help {
                    line.push_str(&format!("  {help}"));
                }
                println!("{line}");
            }
            println!("\n{}", script.body);
            Ok(0)
        }
        ["remove" | "rm", name] => {
            let path = script_path(name)?;
            std::fs::remove_file(&path).map_err(|_| format!("no script named '{name}'"))?;
            println!("Removed {name}");
            Ok(0)
        }
        _ => Err("usage: hiruzen scripts <list|new|edit|show|remove|dir> [NAME]".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "#!/bin/sh\n# description: Restart a service\n# param: service - Which service (choices: api, web)\n# param: env=staging - Environment to act on\n\nkubectl rollout restart deploy/{{service}} -n {{ env }} --context {{ctx}}\n# not a header\n";

    #[test]
    fn parses_the_header_and_finds_undeclared_blanks() {
        let script = parse(SAMPLE);
        assert_eq!(script.description.as_deref(), Some("Restart a service"));
        let names: Vec<&str> = script.params.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, vec!["service", "env", "ctx"]);
        assert_eq!(script.params[0].choices, vec!["api", "web"]);
        assert_eq!(script.params[0].help.as_deref(), Some("Which service"));
        assert_eq!(script.params[1].default.as_deref(), Some("staging"));
        assert!(script.body.starts_with("#!/bin/sh\n"));
        assert!(script.body.ends_with("# not a header"));
    }

    #[test]
    fn fills_blanks_from_values_and_defaults() {
        let script = parse(SAMPLE);
        let mut values = HashMap::new();
        values.insert("service".to_string(), "api".to_string());
        assert_eq!(render(&script, &values), Err(vec!["ctx".to_string()]));
        values.insert("ctx".to_string(), "prod".to_string());
        let command = render(&script, &values).unwrap();
        assert!(command.contains("deploy/api -n staging --context prod"), "{command}");
    }

    #[test]
    fn rejects_names_that_could_leave_the_scripts_directory() {
        assert!(valid_name("deploy-api_v2.sh"));
        assert!(!valid_name("../evil"));
        assert!(!valid_name(".hidden"));
        assert!(!valid_name("a b"));
        assert!(!valid_name(""));
    }
}
