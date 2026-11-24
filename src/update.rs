//! Updates: the daily check for a newer release, the question asked when a
//! terminal starts, and `hiruzen update`.

use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::settings::{self, Settings};
use crate::shell;

const REPOSITORY: &str = env!("CARGO_PKG_REPOSITORY");
const CURRENT: &str = env!("CARGO_PKG_VERSION");
/// Seconds between two checks made on hiruzen's own initiative.
const CHECK_INTERVAL: u64 = 24 * 60 * 60;
/// Set for the new version when a session restarts into it after an update,
/// so that it does not ask again.
const UPDATED_VARIABLE: &str = "HIRUZEN_UPDATED";

/// What hiruzen remembers between checks, kept beside the settings.
#[derive(Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
struct State {
    /// When GitHub was last asked, in seconds since 1970.
    checked: u64,
    /// The newest release tag GitHub reported, such as `v0.2.0`.
    latest: String,
    /// A release the user said no to; it is not offered again at startup.
    declined: String,
}

fn state_file() -> Option<PathBuf> {
    Some(settings::config_dir()?.join("update.json"))
}

impl State {
    fn load() -> State {
        state_file()
            .and_then(|path| std::fs::read_to_string(path).ok())
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    fn save(&self) {
        let Some(path) = state_file() else { return };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(text) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(path, text + "\n");
        }
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}

/// `1.2.3` as numbers, ignoring a leading `v` and any pre-release suffix.
fn version_numbers(version: &str) -> Vec<u64> {
    version
        .trim_start_matches('v')
        .split(['-', '+'])
        .next()
        .unwrap_or("")
        .split('.')
        .map(|part| part.parse().unwrap_or(0))
        .collect()
}

/// A tag is placed in a download address and a command line, so only the
/// characters a version is made of are accepted.
fn valid_tag(tag: &str) -> bool {
    tag.chars().next().is_some_and(|first| first.is_ascii_alphanumeric())
        && tag
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '+'))
}

fn is_newer(tag: &str) -> bool {
    valid_tag(tag) && version_numbers(tag) > version_numbers(CURRENT)
}

/// Asks GitHub for the tag of the latest release.
fn fetch_latest() -> Result<String, String> {
    let releases = format!("{REPOSITORY}/releases");
    let api = REPOSITORY.replace("https://github.com/", "https://api.github.com/repos/") + "/releases/latest";
    let agent = format!("hiruzen/{CURRENT}");
    let output = Command::new("curl")
        .args(["-fsSL", "--max-time", "10", "-A", &agent, &api])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .map_err(|_| format!("curl is needed to check for updates; see {releases}"))?;
    if !output.status.success() {
        return Err(format!("could not reach GitHub to check for updates; see {releases}"));
    }
    serde_json::from_slice::<Value>(&output.stdout)
        .ok()
        .and_then(|release| release.get("tag_name")?.as_str().map(str::to_string))
        .filter(|tag| valid_tag(tag))
        .ok_or_else(|| format!("GitHub gave an answer hiruzen could not read; see {releases}"))
}

/// How this copy of hiruzen was installed, which decides how it is updated.
#[derive(Debug, PartialEq)]
enum Method {
    Brew,
    Cargo,
    /// The installer script from the release page, which installs to `~/.local/bin`.
    Installer,
    /// Somewhere hiruzen cannot update on its own.
    Manual,
}

fn method_for(exe: &Path, home: Option<&Path>) -> Method {
    let has = |name: &str| exe.components().any(|part| part.as_os_str() == name);
    if has("Cellar") || has("homebrew") || has("linuxbrew") {
        Method::Brew
    } else if has(".cargo") {
        Method::Cargo
    } else if home.is_some_and(|home| exe.parent() == Some(&home.join(".local").join("bin"))) {
        Method::Installer
    } else {
        Method::Manual
    }
}

/// The running binary, with links followed: Homebrew starts hiruzen through
/// a link in its `bin` directory.
fn current_exe() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    Some(exe.canonicalize().unwrap_or(exe))
}

fn method() -> Method {
    match current_exe() {
        Some(exe) => method_for(&exe, shell::home_dir().as_deref()),
        None => Method::Manual,
    }
}

/// The command that installs the release `tag`, as words.
fn update_command(method: &Method, tag: &str) -> Option<Vec<String>> {
    let words = |words: &[&str]| Some(words.iter().map(|word| word.to_string()).collect());
    match method {
        Method::Brew => words(&["brew", "upgrade", "hiruzen"]),
        Method::Cargo => words(&["cargo", "install", "--locked", "--git", REPOSITORY, "--tag", tag]),
        Method::Installer if cfg!(windows) => {
            let url = format!("{REPOSITORY}/releases/download/{tag}/hiruzen-cli-installer.ps1");
            words(&[
                "powershell",
                "-ExecutionPolicy",
                "Bypass",
                "-c",
                &format!("irm {url} | iex"),
            ])
        }
        Method::Installer => {
            let url = format!("{REPOSITORY}/releases/download/{tag}/hiruzen-cli-installer.sh");
            // The script is fetched whole before any of it runs, so a failed
            // download installs nothing.
            words(&[
                "sh",
                "-c",
                &format!("script=$(curl --proto '=https' --tlsv1.2 -LsSf '{url}') && printf '%s\\n' \"$script\" | sh"),
            ])
        }
        Method::Manual => None,
    }
}

/// The command as the user would type it.
fn shown(command: &[String]) -> String {
    command
        .iter()
        .map(|word| {
            if word.contains(' ') {
                format!("\"{word}\"")
            } else {
                word.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn manual_instructions() -> String {
    format!("Update by installing it again; see {REPOSITORY}/releases/latest")
}

/// Where Windows keeps the running program while its replacement is installed.
fn set_aside(exe: &Path) -> PathBuf {
    exe.with_extension("exe.old")
}

/// Installs the release `tag` with the command for `method`.
fn install(method: &Method, tag: &str) -> Result<(), String> {
    let command = update_command(method, tag).ok_or_else(manual_instructions)?;
    // Windows will not overwrite a program that is running, but it lets one
    // be renamed, which leaves the name free for the new version.
    let moved = (cfg!(windows) && *method == Method::Installer)
        .then(current_exe)
        .flatten()
        .and_then(|exe| {
            let old = set_aside(&exe);
            let _ = std::fs::remove_file(&old);
            std::fs::rename(&exe, &old).is_ok().then_some((exe, old))
        });
    let outcome = Command::new(&command[0]).args(&command[1..]).status();
    let result = match outcome {
        Ok(status) if status.success() => Ok(()),
        Ok(_) => Err("the update did not finish; run `hiruzen update` to try again".to_string()),
        Err(_) => Err(format!(
            "{} is needed to update; see {REPOSITORY}/releases/latest",
            command[0]
        )),
    };
    if let Some((exe, old)) = moved {
        if !exe.exists() {
            let _ = std::fs::rename(&old, &exe);
        }
    }
    result
}

/// Asks a yes-or-no question on the terminal. Enter alone means yes.
fn ask(question: &str) -> bool {
    print!("{question} [Y/n] ");
    let _ = std::io::stdout().flush();
    let mut answer = String::new();
    if std::io::stdin().read_line(&mut answer).is_err() {
        return false;
    }
    matches!(answer.trim().to_ascii_lowercase().as_str(), "" | "y" | "yes")
}

fn available(tag: &str) -> String {
    format!(
        "hiruzen {} is available; you have {CURRENT}.",
        tag.trim_start_matches('v')
    )
}

/// Replaces this process with the version just installed, so the terminal
/// being opened already runs it. Returns if that cannot be done.
fn restart() {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        if let Some(exe) = shell::find_on_path("hiruzen") {
            let _ = Command::new(exe)
                .args(std::env::args_os().skip(1))
                .env(UPDATED_VARIABLE, "1")
                .exec();
        }
    }
}

/// Called as a session starts. Offers a release found by an earlier check,
/// and once a day looks for a newer one in the background, so that starting
/// a terminal never waits for the network. What the user should be able to
/// read once the session has started is returned for the session to show.
pub fn offer() -> Option<String> {
    if std::env::var_os(UPDATED_VARIABLE).is_some() || !Settings::load().update_check {
        return None;
    }
    let mut notice = None;
    let mut state = State::load();
    if cfg!(windows) {
        if let Some(exe) = current_exe() {
            let _ = std::fs::remove_file(set_aside(&exe));
        }
    }

    if is_newer(&state.latest) && state.latest != state.declined {
        let tag = state.latest.clone();
        let method = method();
        // Whatever happens next, this release is not brought up again when a
        // terminal starts; `hiruzen update` still offers it.
        state.declined = tag.clone();
        state.save();
        notice = Some(if method == Method::Manual {
            format!("{} {}", available(&tag), manual_instructions())
        } else if ask(&format!("{} Update now?", available(&tag))) {
            match install(&method, &tag) {
                Ok(()) => {
                    restart();
                    format!(
                        "hiruzen is now {}. New terminals will use it.",
                        tag.trim_start_matches('v')
                    )
                }
                Err(message) => format!("hiruzen: {message}"),
            }
        } else {
            "Run `hiruzen update` when you want it. `hiruzen settings updates.check false` stops these checks."
                .to_string()
        });
    }

    let time = now();
    if time.saturating_sub(state.checked) >= CHECK_INTERVAL {
        // Recorded before asking, so terminals opened together ask once.
        state.checked = time;
        state.save();
        std::thread::spawn(|| {
            if let Ok(latest) = fetch_latest() {
                let mut state = State::load();
                state.latest = latest;
                state.save();
            }
        });
    }
    notice
}

/// `hiruzen update [-y]`: asks GitHub for the latest release and installs it
/// once the user agrees.
pub fn update(args: &[String]) -> Result<i32, String> {
    let mut yes = false;
    for arg in args {
        match arg.as_str() {
            "-y" | "--yes" => yes = true,
            other => return Err(format!("unknown argument '{other}'; usage: hiruzen update [-y]")),
        }
    }
    let latest = fetch_latest()?;
    let mut state = State::load();
    state.checked = now();
    state.latest = latest.clone();
    state.save();
    if !is_newer(&latest) {
        println!("hiruzen {CURRENT} is the latest version.");
        return Ok(0);
    }
    println!("{}", available(&latest));
    let method = method();
    let Some(command) = update_command(&method, &latest) else {
        println!("{}", manual_instructions());
        return Ok(0);
    };
    let interactive = std::io::stdin().is_terminal() && std::io::stdout().is_terminal();
    if yes || (interactive && ask("Update now?")) {
        install(&method, &latest)?;
        println!(
            "hiruzen is now {}. New terminals will use it.",
            latest.trim_start_matches('v')
        );
    } else {
        println!("Update with: {}", shown(&command));
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_versions_by_number() {
        assert!(version_numbers("v0.10.0") > version_numbers("0.9.9"));
        assert_eq!(version_numbers("v1.2.3"), vec![1, 2, 3]);
        assert_eq!(version_numbers("v0.2.0"), version_numbers("0.2.0-rc.1"));
        assert!(!is_newer(CURRENT));
        assert!(is_newer("v999.0.0"));
        assert!(!is_newer(""));
    }

    #[test]
    fn accepts_only_tags_that_look_like_versions() {
        assert!(valid_tag("v0.2.0"));
        assert!(valid_tag("0.2.0-rc.1+build5"));
        assert!(!valid_tag(""));
        assert!(!valid_tag("v1.0.0'; rm -rf ~"));
        assert!(!valid_tag("-v1"));
        assert!(!is_newer("v999.0.0$(id)"));
    }

    #[test]
    fn works_out_how_hiruzen_was_installed() {
        let home = Path::new("/home/me");
        let method = |exe: &str| method_for(Path::new(exe), Some(home));
        assert_eq!(method("/opt/homebrew/Cellar/hiruzen/0.1.0/bin/hiruzen"), Method::Brew);
        assert_eq!(method("/usr/local/Cellar/hiruzen/0.1.0/bin/hiruzen"), Method::Brew);
        assert_eq!(method("/home/linuxbrew/.linuxbrew/bin/hiruzen"), Method::Brew);
        assert_eq!(
            method("/home/linuxbrew/.linuxbrew/Cellar/hiruzen/0.1.0/bin/hiruzen"),
            Method::Brew
        );
        assert_eq!(method("/home/me/.cargo/bin/hiruzen"), Method::Cargo);
        assert_eq!(method("/home/me/.local/bin/hiruzen"), Method::Installer);
        assert_eq!(method("/home/me/code/hiruzen/target/debug/hiruzen"), Method::Manual);
        assert_eq!(
            method_for(Path::new("/home/me/.local/bin/hiruzen"), None),
            Method::Manual
        );
    }

    #[test]
    fn builds_the_command_for_each_way_of_installing() {
        assert_eq!(
            shown(&update_command(&Method::Brew, "v0.2.0").unwrap()),
            "brew upgrade hiruzen"
        );
        let cargo = shown(&update_command(&Method::Cargo, "v0.2.0").unwrap());
        assert!(cargo.ends_with("--tag v0.2.0"), "{cargo}");
        let installer = update_command(&Method::Installer, "v0.2.0").unwrap();
        assert!(installer
            .last()
            .unwrap()
            .contains("/releases/download/v0.2.0/hiruzen-cli-installer."));
        assert_eq!(update_command(&Method::Manual, "v0.2.0"), None);
    }

    #[test]
    fn state_survives_a_missing_or_partial_file() {
        let state: State = serde_json::from_str(r#"{"latest": "v0.2.0"}"#).unwrap();
        assert_eq!(state.latest, "v0.2.0");
        assert_eq!(state.checked, 0);
        assert_eq!(state.declined, "");
    }
}
