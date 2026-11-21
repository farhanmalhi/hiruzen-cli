//! User settings: a flat JSON file of `"section.name": value` pairs, and the
//! colour themes the dropdown can use.

use std::path::PathBuf;
use std::time::SystemTime;

use serde_json::{Map, Value};

use crate::keys;
use crate::shell;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ValueKind {
    Bool,
    /// A whole number within the given inclusive range.
    Int(i64, i64),
    /// One of a fixed set of names; an empty set accepts any text.
    Text(&'static [&'static str]),
    /// A list of words.
    List,
    /// A comma-separated list of key names.
    Keys,
}

pub struct Definition {
    pub key: &'static str,
    pub kind: ValueKind,
    pub default: &'static str,
    pub help: &'static str,
}

/// Every setting hiruzen understands.
pub const DEFINITIONS: &[Definition] = &[
    Definition {
        key: "autocomplete.disable",
        kind: ValueKind::Bool,
        default: "false",
        help: "Turn suggestions off without removing hiruzen from your shell",
    },
    Definition {
        key: "autocomplete.theme",
        kind: ValueKind::Text(THEME_NAMES),
        default: "dark",
        help: "Colours of the dropdown",
    },
    Definition {
        key: "autocomplete.height",
        kind: ValueKind::Int(1, 20),
        default: "8",
        help: "Most suggestions shown at once",
    },
    Definition {
        key: "autocomplete.descriptions",
        kind: ValueKind::Bool,
        default: "true",
        help: "Show each suggestion's description beside it",
    },
    Definition {
        key: "autocomplete.fuzzySearch",
        kind: ValueKind::Bool,
        default: "true",
        help: "Match anywhere in a suggestion (`ch` finds `branch`), not only at its start",
    },
    Definition {
        key: "autocomplete.history",
        kind: ValueKind::Bool,
        default: "true",
        help: "Rank and suggest using your shell history",
    },
    Definition {
        key: "autocomplete.disableForCommands",
        kind: ValueKind::List,
        default: "",
        help: "Commands that get no suggestions, for example: ssh,mysql",
    },
    Definition {
        key: "autocomplete.onlineSuggestions",
        kind: ValueKind::Bool,
        default: "true",
        help: "Let specs fetch suggestions from the network, such as package names from a registry",
    },
    Definition {
        key: "keys.accept",
        kind: ValueKind::Keys,
        default: "tab",
        help: "Keys that insert the highlighted suggestion",
    },
    Definition {
        key: "keys.next",
        kind: ValueKind::Keys,
        default: "down",
        help: "Keys that move the highlight down",
    },
    Definition {
        key: "keys.previous",
        kind: ValueKind::Keys,
        default: "up",
        help: "Keys that move the highlight up",
    },
    Definition {
        key: "keys.dismiss",
        kind: ValueKind::Keys,
        default: "escape",
        help: "Keys that hide suggestions for the current word",
    },
    Definition {
        key: "autocomplete.enterAccepts",
        kind: ValueKind::Bool,
        default: "true",
        help: "Enter inserts the highlighted suggestion once you have moved the highlight",
    },
    Definition {
        key: "prompt.enable",
        kind: ValueKind::Bool,
        default: "true",
        help: "Give zsh's stock prompt colours, a short directory and the git branch (new sessions)",
    },
    Definition {
        key: "prompt.showHost",
        kind: ValueKind::Bool,
        default: "false",
        help: "Start the prompt with your user and machine name, as in you@host",
    },
    Definition {
        key: "prompt.directoryLength",
        kind: ValueKind::Int(4, 80),
        default: "24",
        help: "Longest directory name the prompt shows before cutting out its middle",
    },
    Definition {
        key: "prompt.branchLength",
        kind: ValueKind::Int(4, 80),
        default: "80",
        help: "Longest branch name the prompt shows; a narrow window shortens it sooner",
    },
];

pub fn definition(key: &str) -> Option<&'static Definition> {
    DEFINITIONS.iter().find(|d| d.key.eq_ignore_ascii_case(key))
}

/// The directory hiruzen keeps its files in.
pub fn config_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("HIRUZEN_CONFIG_DIR").filter(|d| !d.is_empty()) {
        return Some(PathBuf::from(dir));
    }
    if cfg!(windows) {
        if let Some(appdata) = std::env::var_os("APPDATA").filter(|d| !d.is_empty()) {
            return Some(PathBuf::from(appdata).join("hiruzen"));
        }
    }
    if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME").filter(|d| !d.is_empty()) {
        return Some(PathBuf::from(xdg).join("hiruzen"));
    }
    Some(shell::home_dir()?.join(".config").join("hiruzen"))
}

pub fn settings_file() -> Option<PathBuf> {
    Some(config_dir()?.join("settings.json"))
}

/// Parses text typed on the command line into a value of the setting's kind.
pub fn parse_value(definition: &Definition, text: &str) -> Result<Value, String> {
    let text = text.trim();
    match definition.kind {
        ValueKind::Bool => match text.to_ascii_lowercase().as_str() {
            "true" | "on" | "yes" | "1" => Ok(Value::Bool(true)),
            "false" | "off" | "no" | "0" => Ok(Value::Bool(false)),
            _ => Err(format!("{} takes true or false", definition.key)),
        },
        ValueKind::Int(min, max) => match text.parse::<i64>() {
            Ok(n) if (min..=max).contains(&n) => Ok(Value::from(n)),
            _ => Err(format!("{} takes a whole number from {min} to {max}", definition.key)),
        },
        ValueKind::Text(allowed) => {
            if allowed.is_empty() || allowed.contains(&text) {
                Ok(Value::from(text))
            } else {
                Err(format!("{} takes one of: {}", definition.key, allowed.join(", ")))
            }
        }
        ValueKind::Keys => keys::parse_list(text).map(Value::from),
        ValueKind::List => Ok(Value::from(
            text.split([',', ' '])
                .map(str::trim)
                .filter(|w| !w.is_empty())
                .collect::<Vec<_>>(),
        )),
    }
}

/// The raw contents of the settings file. A missing file is an empty map.
pub fn read_raw() -> Result<Map<String, Value>, String> {
    let Some(path) = settings_file() else {
        return Ok(Map::new());
    };
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Map::new()),
        Err(error) => return Err(format!("cannot read {}: {error}", path.display())),
    };
    if text.trim().is_empty() {
        return Ok(Map::new());
    }
    match serde_json::from_str::<Value>(&text) {
        Ok(Value::Object(map)) => Ok(map),
        Ok(_) => Err(format!("{} must hold a JSON object", path.display())),
        Err(error) => Err(format!("{} is not valid JSON: {error}", path.display())),
    }
}

pub fn write_raw(map: &Map<String, Value>) -> Result<(), String> {
    let path = settings_file().ok_or("cannot work out where settings are kept")?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    }
    let text = serde_json::to_string_pretty(map).map_err(|e| e.to_string())?;
    std::fs::write(&path, text + "\n").map_err(|e| format!("cannot write {}: {e}", path.display()))
}

/// The settings in effect: the file's values where valid, defaults elsewhere.
#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    pub disable: bool,
    pub theme: &'static Theme,
    pub height: usize,
    pub descriptions: bool,
    pub fuzzy: bool,
    pub history: bool,
    pub disabled_commands: Vec<String>,
    pub enter_accepts: bool,
    pub online: bool,
    pub keys: Bindings,
    pub prompt: bool,
    pub prompt_host: bool,
    pub prompt_directory_length: usize,
    pub prompt_branch_length: usize,
}

/// The byte sequences bound to each dropdown action.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Bindings {
    pub accept: Vec<Vec<u8>>,
    pub next: Vec<Vec<u8>>,
    pub previous: Vec<Vec<u8>>,
    pub dismiss: Vec<Vec<u8>>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings::from_map(&Map::new())
    }
}

impl Settings {
    pub fn from_map(map: &Map<String, Value>) -> Settings {
        // A value of the wrong type in the file falls back to the default.
        let value = |key: &str| -> Value {
            let definition = definition(key).expect("a defined setting");
            let stored = map.iter().find(|(k, _)| k.eq_ignore_ascii_case(key)).map(|(_, v)| v);
            let valid = stored.and_then(|v| {
                let text = match v {
                    Value::String(s) => s.clone(),
                    Value::Array(items) => items.iter().filter_map(|i| i.as_str()).collect::<Vec<_>>().join(","),
                    other => other.to_string(),
                };
                parse_value(definition, &text).ok()
            });
            valid.unwrap_or_else(|| parse_value(definition, definition.default).expect("a valid default"))
        };
        Settings {
            disable: value("autocomplete.disable").as_bool().unwrap_or(false),
            theme: theme(value("autocomplete.theme").as_str().unwrap_or("dark")),
            height: value("autocomplete.height").as_u64().unwrap_or(8) as usize,
            descriptions: value("autocomplete.descriptions").as_bool().unwrap_or(true),
            fuzzy: value("autocomplete.fuzzySearch").as_bool().unwrap_or(true),
            history: value("autocomplete.history").as_bool().unwrap_or(true),
            disabled_commands: value("autocomplete.disableForCommands")
                .as_array()
                .map(|items| items.iter().filter_map(|i| i.as_str().map(str::to_string)).collect())
                .unwrap_or_default(),
            enter_accepts: value("autocomplete.enterAccepts").as_bool().unwrap_or(true),
            online: value("autocomplete.onlineSuggestions").as_bool().unwrap_or(true),
            keys: {
                let bound = |key: &str| -> Vec<Vec<u8>> {
                    let names: Vec<String> = value(key)
                        .as_array()
                        .map(|items| items.iter().filter_map(|i| i.as_str().map(str::to_string)).collect())
                        .unwrap_or_default();
                    keys::all_sequences(&names)
                };
                Bindings {
                    accept: bound("keys.accept"),
                    next: bound("keys.next"),
                    previous: bound("keys.previous"),
                    dismiss: bound("keys.dismiss"),
                }
            },
            prompt: value("prompt.enable").as_bool().unwrap_or(true),
            prompt_host: value("prompt.showHost").as_bool().unwrap_or(false),
            prompt_directory_length: value("prompt.directoryLength").as_u64().unwrap_or(24) as usize,
            prompt_branch_length: value("prompt.branchLength").as_u64().unwrap_or(80) as usize,
        }
    }

    /// Loads the settings file. An unreadable or invalid file gives the defaults.
    pub fn load() -> Settings {
        Settings::from_map(&read_raw().unwrap_or_default())
    }
}

/// When the settings file last changed, for noticing edits during a session.
pub fn modified() -> Option<SystemTime> {
    std::fs::metadata(settings_file()?).ok()?.modified().ok()
}

/// The dropdown's colours, as SGR sequences.
#[derive(Debug, PartialEq)]
pub struct Theme {
    pub name: &'static str,
    pub row: &'static str,
    pub description: &'static str,
    pub marker: &'static str,
    pub selected: &'static str,
    pub selected_description: &'static str,
    pub selected_marker: &'static str,
}

pub const THEME_NAMES: &[&str] = &["dark", "light", "dracula", "nord", "gruvbox", "solarized"];

const THEMES: &[Theme] = &[
    Theme {
        name: "dark",
        row: "\x1b[0;38;5;252;48;5;236m",
        description: "\x1b[0;38;5;245;48;5;236m",
        marker: "\x1b[0;38;5;75;48;5;236m",
        selected: "\x1b[0;1;38;5;231;48;5;25m",
        selected_description: "\x1b[0;38;5;253;48;5;25m",
        selected_marker: "\x1b[0;38;5;159;48;5;25m",
    },
    Theme {
        name: "light",
        row: "\x1b[0;38;5;236;48;5;254m",
        description: "\x1b[0;38;5;243;48;5;254m",
        marker: "\x1b[0;38;5;25;48;5;254m",
        selected: "\x1b[0;1;38;5;231;48;5;32m",
        selected_description: "\x1b[0;38;5;255;48;5;32m",
        selected_marker: "\x1b[0;38;5;195;48;5;32m",
    },
    Theme {
        name: "dracula",
        row: "\x1b[0;38;5;253;48;5;236m",
        description: "\x1b[0;38;5;61;48;5;236m",
        marker: "\x1b[0;38;5;212;48;5;236m",
        selected: "\x1b[0;1;38;5;231;48;5;61m",
        selected_description: "\x1b[0;38;5;189;48;5;61m",
        selected_marker: "\x1b[0;38;5;84;48;5;61m",
    },
    Theme {
        name: "nord",
        row: "\x1b[0;38;5;188;48;5;237m",
        description: "\x1b[0;38;5;103;48;5;237m",
        marker: "\x1b[0;38;5;110;48;5;237m",
        selected: "\x1b[0;1;38;5;231;48;5;67m",
        selected_description: "\x1b[0;38;5;189;48;5;67m",
        selected_marker: "\x1b[0;38;5;159;48;5;67m",
    },
    Theme {
        name: "gruvbox",
        row: "\x1b[0;38;5;223;48;5;237m",
        description: "\x1b[0;38;5;245;48;5;237m",
        marker: "\x1b[0;38;5;214;48;5;237m",
        selected: "\x1b[0;1;38;5;235;48;5;214m",
        selected_description: "\x1b[0;38;5;237;48;5;214m",
        selected_marker: "\x1b[0;38;5;124;48;5;214m",
    },
    Theme {
        name: "solarized",
        row: "\x1b[0;38;5;247;48;5;235m",
        description: "\x1b[0;38;5;241;48;5;235m",
        marker: "\x1b[0;38;5;37;48;5;235m",
        selected: "\x1b[0;1;38;5;230;48;5;33m",
        selected_description: "\x1b[0;38;5;254;48;5;33m",
        selected_marker: "\x1b[0;38;5;229;48;5;33m",
    },
];

/// The theme with this name, or the default theme for an unknown name.
pub fn theme(name: &str) -> &'static Theme {
    THEMES.iter().find(|t| t.name == name).unwrap_or(&THEMES[0])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_setting_has_a_valid_default_and_every_theme_exists() {
        for definition in DEFINITIONS {
            assert!(
                parse_value(definition, definition.default).is_ok(),
                "{}",
                definition.key
            );
        }
        for name in THEME_NAMES {
            assert_eq!(theme(name).name, *name);
        }
        assert_eq!(THEME_NAMES.len(), THEMES.len());
    }

    #[test]
    fn parses_values_by_kind() {
        let height = definition("autocomplete.height").unwrap();
        assert_eq!(parse_value(height, "12").unwrap(), Value::from(12));
        assert!(parse_value(height, "0").is_err());
        assert!(parse_value(height, "many").is_err());
        let theme = definition("autocomplete.theme").unwrap();
        assert!(parse_value(theme, "nord").is_ok());
        assert!(parse_value(theme, "neon").is_err());
        let list = definition("AUTOCOMPLETE.disableForCommands").unwrap();
        assert_eq!(
            parse_value(list, "ssh, mysql").unwrap(),
            serde_json::json!(["ssh", "mysql"])
        );
        assert_eq!(
            parse_value(definition("autocomplete.disable").unwrap(), "on").unwrap(),
            Value::Bool(true)
        );
    }

    #[test]
    fn invalid_stored_values_fall_back_to_defaults() {
        let map: Map<String, Value> = serde_json::from_str(
            r#"{"autocomplete.height": 99, "autocomplete.theme": "nord", "autocomplete.fuzzySearch": "off",
                "autocomplete.disableForCommands": ["ssh"], "unknown.key": 1}"#,
        )
        .unwrap();
        let settings = Settings::from_map(&map);
        assert_eq!(settings.height, 8);
        assert_eq!(settings.theme.name, "nord");
        assert!(!settings.fuzzy);
        assert_eq!(settings.disabled_commands, vec!["ssh"]);
        assert_eq!(Settings::default().theme.name, "dark");
        assert_eq!(Settings::default().keys.accept, vec![b"\t".to_vec()]);
        let rebound: Map<String, Value> =
            serde_json::from_str(r#"{"keys.accept": ["ctrl+y", "tab"], "keys.next": "ctrl+n"}"#).unwrap();
        let settings = Settings::from_map(&rebound);
        assert_eq!(settings.keys.accept, vec![vec![25], b"\t".to_vec()]);
        assert_eq!(settings.keys.next, vec![vec![14]]);
        assert_eq!(settings.keys.previous.len(), 2);
    }
}
