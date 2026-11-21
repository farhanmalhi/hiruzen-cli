//! Shell plugins: directories of shell code that hiruzen loads into each new
//! shell. A plugin is added from a local directory or cloned with git.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};

use crate::settings;
use crate::shell::Shell;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Plugin {
    pub name: String,
    /// Where it came from: a git URL or a local path.
    pub source: String,
    pub path: PathBuf,
    /// Cloned by hiruzen into its own directory, so hiruzen may update and delete it.
    pub managed: bool,
}

fn registry_file() -> Result<PathBuf, String> {
    Ok(settings::config_dir()
        .ok_or("cannot work out where plugins are kept")?
        .join("plugins.json"))
}

fn clones_dir() -> Result<PathBuf, String> {
    Ok(settings::config_dir()
        .ok_or("cannot work out where plugins are kept")?
        .join("plugins"))
}

pub fn installed() -> Vec<Plugin> {
    registry_file()
        .ok()
        .and_then(|path| std::fs::read(path).ok())
        .and_then(|raw| serde_json::from_slice(&raw).ok())
        .unwrap_or_default()
}

fn save(plugins: &[Plugin]) -> Result<(), String> {
    let path = registry_file()?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    }
    let text = serde_json::to_string_pretty(plugins).map_err(|e| e.to_string())?;
    std::fs::write(&path, text + "\n").map_err(|e| format!("cannot write {}: {e}", path.display()))
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('.')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/// `zsh-autosuggestions` for `https://github.com/zsh-users/zsh-autosuggestions.git`.
fn name_from_source(source: &str) -> String {
    let trimmed = source.trim_end_matches(['/', '\\']);
    let last = trimmed.rsplit(['/', '\\', ':']).next().unwrap_or(trimmed);
    last.strip_suffix(".git").unwrap_or(last).to_string()
}

/// The files of a plugin that a shell should load, in order.
pub fn entry_files(plugin: &Plugin, shell: Shell) -> Vec<PathBuf> {
    let root = &plugin.path;
    if root.is_file() {
        return vec![root.clone()];
    }
    let name = &plugin.name;
    let with_extension = |extension: &str| -> Vec<PathBuf> {
        let mut found: Vec<PathBuf> = std::fs::read_dir(root)
            .into_iter()
            .flatten()
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.is_file() && path.to_string_lossy().ends_with(extension))
            .collect();
        found.sort();
        found
    };
    let first_existing = |candidates: &[String]| -> Option<PathBuf> {
        candidates
            .iter()
            .map(|file| root.join(file))
            .find(|path| path.is_file())
    };
    match shell {
        Shell::Zsh => first_existing(&[format!("{name}.plugin.zsh")])
            .or_else(|| with_extension(".plugin.zsh").into_iter().next())
            .or_else(|| first_existing(&[format!("{name}.zsh"), "init.zsh".to_string(), format!("{name}.sh")]))
            .into_iter()
            .collect(),
        Shell::Bash => first_existing(&[format!("{name}.plugin.bash")])
            .or_else(|| with_extension(".plugin.bash").into_iter().next())
            .or_else(|| {
                first_existing(&[
                    format!("{name}.bash"),
                    "init.bash".to_string(),
                    format!("{name}.sh"),
                    "init.sh".to_string(),
                ])
            })
            .into_iter()
            .collect(),
        Shell::Fish => {
            let mut files: Vec<PathBuf> = std::fs::read_dir(root.join("conf.d"))
                .into_iter()
                .flatten()
                .flatten()
                .map(|entry| entry.path())
                .filter(|path| path.extension().is_some_and(|e| e == "fish"))
                .collect();
            files.sort();
            files.extend(first_existing(&["init.fish".to_string(), format!("{name}.fish")]));
            files
        }
        Shell::Pwsh | Shell::Powershell => {
            first_existing(&[format!("{name}.psm1"), format!("{name}.ps1"), "init.ps1".to_string()])
                .into_iter()
                .collect()
        }
        Shell::Cmd => Vec::new(),
    }
}

/// The lines that load every installed plugin, for the end of `hiruzen init`.
pub fn init_lines(shell: Shell) -> String {
    let mut out = String::new();
    for plugin in installed() {
        let posix = |path: &Path| format!("'{}'", path.to_string_lossy().replace('\'', "'\\''"));
        let powershell = |path: &Path| format!("'{}'", path.to_string_lossy().replace('\'', "''"));
        if shell == Shell::Fish && plugin.path.join("functions").is_dir() {
            let dir = plugin.path.join("functions");
            out.push_str(&format!("set -p fish_function_path {}\n", posix(&dir)));
        }
        for file in entry_files(&plugin, shell) {
            match shell {
                Shell::Zsh | Shell::Bash => {
                    let quoted = posix(&file);
                    out.push_str(&format!("[ -r {quoted} ] && . {quoted}\n"));
                }
                Shell::Fish => {
                    let quoted = posix(&file);
                    out.push_str(&format!("test -r {quoted}; and source {quoted}\n"));
                }
                Shell::Pwsh | Shell::Powershell => {
                    let quoted = powershell(&file);
                    if file.extension().is_some_and(|e| e == "psm1") {
                        out.push_str(&format!("if (Test-Path {quoted}) {{ Import-Module {quoted} }}\n"));
                    } else {
                        out.push_str(&format!("if (Test-Path {quoted}) {{ . {quoted} }}\n"));
                    }
                }
                Shell::Cmd => {}
            }
        }
    }
    out
}

fn git(args: &[&str], cwd: Option<&Path>) -> Result<(), String> {
    let mut command = Command::new("git");
    command.args(args);
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    match command.status() {
        Ok(status) if status.success() => Ok(()),
        Ok(_) => Err(format!("git {} failed", args.first().unwrap_or(&""))),
        Err(error) => Err(format!("cannot run git: {error}")),
    }
}

fn add(source: &str, name: Option<&str>) -> Result<i32, String> {
    let name = name.map(str::to_string).unwrap_or_else(|| name_from_source(source));
    if !valid_name(&name) {
        return Err(format!("'{name}' is not a usable plugin name; give one with --name"));
    }
    let mut plugins = installed();
    if plugins.iter().any(|p| p.name == name) {
        return Err(format!("a plugin named '{name}' is already installed"));
    }
    let local = Path::new(source);
    let plugin = if local.exists() {
        let path = local.canonicalize().map_err(|e| format!("cannot read {source}: {e}"))?;
        Plugin {
            name: name.clone(),
            source: path.to_string_lossy().into_owned(),
            path,
            managed: false,
        }
    } else {
        let dir = clones_dir()?;
        std::fs::create_dir_all(&dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
        let path = dir.join(&name);
        if path.exists() {
            return Err(format!("{} already exists; remove it first", path.display()));
        }
        git(&["clone", "--depth", "1", "--", source, &path.to_string_lossy()], None)?;
        Plugin {
            name: name.clone(),
            source: source.to_string(),
            path,
            managed: true,
        }
    };
    let shells: Vec<&str> = Shell::ALL
        .into_iter()
        .filter(|s| !entry_files(&plugin, *s).is_empty())
        .map(Shell::name)
        .collect();
    plugins.push(plugin);
    save(&plugins)?;
    if shells.is_empty() {
        println!("Added {name}, but found nothing in it that hiruzen knows how to load.");
    } else {
        println!(
            "Added {name} (loads in: {}). Open a new terminal to use it.",
            shells.join(", ")
        );
    }
    Ok(0)
}

fn remove(name: &str) -> Result<i32, String> {
    let mut plugins = installed();
    let Some(index) = plugins.iter().position(|p| p.name == name) else {
        return Err(format!("no plugin named '{name}'"));
    };
    let plugin = plugins.remove(index);
    // Only a directory hiruzen created is deleted; a plugin added from a
    // local path is left where it is.
    if plugin.managed && plugin.path.starts_with(clones_dir()?) && plugin.path.is_dir() {
        std::fs::remove_dir_all(&plugin.path).map_err(|e| format!("cannot remove {}: {e}", plugin.path.display()))?;
    }
    save(&plugins)?;
    println!("Removed {name}");
    Ok(0)
}

fn update(name: Option<&str>) -> Result<i32, String> {
    let plugins = installed();
    if let Some(name) = name {
        if !plugins.iter().any(|p| p.name == name) {
            return Err(format!("no plugin named '{name}'"));
        }
    }
    let mut failed = false;
    for plugin in plugins.iter().filter(|p| name.is_none_or(|n| n == p.name)) {
        if !plugin.managed {
            println!("{}: added from a local path; nothing to update", plugin.name);
            continue;
        }
        match git(&["pull", "--ff-only", "--quiet"], Some(&plugin.path)) {
            Ok(()) => println!("{}: up to date", plugin.name),
            Err(error) => {
                failed = true;
                println!("{}: {error}", plugin.name);
            }
        }
    }
    Ok(if failed { 1 } else { 0 })
}

/// `hiruzen plugins <list|add|remove|update> ...`
pub fn plugins(args: &[String]) -> Result<i32, String> {
    let words: Vec<&str> = args.iter().map(String::as_str).collect();
    match words.as_slice() {
        [] | ["list"] => {
            let plugins = installed();
            if plugins.is_empty() {
                println!("No plugins yet. Add one with `hiruzen plugins add <git-url-or-directory>`.");
            }
            for plugin in plugins {
                println!("{:<28} {}", plugin.name, plugin.source);
            }
            Ok(0)
        }
        ["list", "--names"] => {
            for plugin in installed() {
                println!("{}", plugin.name);
            }
            Ok(0)
        }
        ["add", source] => add(source, None),
        ["add", source, "--name", name] | ["add", "--name", name, source] => add(source, Some(name)),
        ["remove" | "rm", name] => remove(name),
        ["update"] => update(None),
        ["update", name] => update(Some(name)),
        _ => Err(
            "usage: hiruzen plugins <list | add <git-url-or-directory> [--name NAME] | remove NAME | update [NAME]>"
                .to_string(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_a_plugin_after_its_source() {
        assert_eq!(
            name_from_source("https://github.com/zsh-users/zsh-autosuggestions.git"),
            "zsh-autosuggestions"
        );
        assert_eq!(name_from_source("git@github.com:me/my-plugin"), "my-plugin");
        assert_eq!(name_from_source("/home/me/plugins/local/"), "local");
    }

    #[test]
    fn finds_what_each_shell_should_load() {
        let root = std::env::temp_dir().join(format!("hiruzen-plugin-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("conf.d")).unwrap();
        for file in [
            "demo.plugin.zsh",
            "demo.zsh",
            "demo.sh",
            "README.md",
            "conf.d/demo.fish",
        ] {
            std::fs::write(root.join(file), "").unwrap();
        }
        let plugin = Plugin {
            name: "demo".into(),
            source: "x".into(),
            path: root.clone(),
            managed: false,
        };
        assert_eq!(entry_files(&plugin, Shell::Zsh), vec![root.join("demo.plugin.zsh")]);
        assert_eq!(entry_files(&plugin, Shell::Bash), vec![root.join("demo.sh")]);
        assert_eq!(
            entry_files(&plugin, Shell::Fish),
            vec![root.join("conf.d").join("demo.fish")]
        );
        assert!(entry_files(&plugin, Shell::Pwsh).is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }
}
