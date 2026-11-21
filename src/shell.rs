//! The shells hiruzen can wrap, and how each is launched with the integration
//! script that marks where the prompt ends.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Shell {
    Zsh,
    Bash,
    Fish,
    /// PowerShell 7+ (`pwsh`).
    Pwsh,
    /// Windows PowerShell 5 (`powershell.exe`).
    Powershell,
    Cmd,
}

pub const ZSH_RC: &str = include_str!("../shell/integration-rc.zsh");
pub const ZSH_ENV: &str = include_str!("../shell/integration-env.zsh");
pub const ZSH_PROFILE: &str = include_str!("../shell/integration-profile.zsh");
pub const ZSH_LOGIN: &str = include_str!("../shell/integration-login.zsh");
pub const BASH: &str = include_str!("../shell/integration.bash");
pub const FISH: &str = include_str!("../shell/integration.fish");
pub const POWERSHELL: &str = include_str!("../shell/integration.ps1");

impl Shell {
    pub const ALL: [Shell; 6] = [
        Shell::Zsh,
        Shell::Bash,
        Shell::Fish,
        Shell::Pwsh,
        Shell::Powershell,
        Shell::Cmd,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Shell::Zsh => "zsh",
            Shell::Bash => "bash",
            Shell::Fish => "fish",
            Shell::Pwsh => "pwsh",
            Shell::Powershell => "powershell",
            Shell::Cmd => "cmd",
        }
    }

    pub fn from_name(name: &str) -> Option<Shell> {
        let lower = name.to_ascii_lowercase();
        let stem = Path::new(&lower)
            .file_stem()?
            .to_str()?
            .trim_start_matches('-')
            .to_string();
        Shell::ALL.into_iter().find(|shell| shell.name() == stem)
    }

    /// The shell hiruzen should wrap when none is named: the user's login
    /// shell on Unix, PowerShell on Windows.
    pub fn detect() -> Shell {
        if cfg!(windows) {
            if find_on_path("pwsh").is_some() {
                return Shell::Pwsh;
            }
            return Shell::Powershell;
        }
        std::env::var("SHELL")
            .ok()
            .and_then(|s| Shell::from_name(&s))
            .unwrap_or(Shell::Bash)
    }

    pub fn is_posix(self) -> bool {
        matches!(self, Shell::Zsh | Shell::Bash | Shell::Fish)
    }

    /// The character that escapes the next one outside quotes.
    pub fn escape_char(self) -> char {
        match self {
            Shell::Zsh | Shell::Bash | Shell::Fish => '\\',
            Shell::Pwsh | Shell::Powershell => '`',
            Shell::Cmd => '^',
        }
    }

    pub fn path_separator(self) -> char {
        match self {
            Shell::Zsh | Shell::Bash | Shell::Fish => '/',
            _ => std::path::MAIN_SEPARATOR,
        }
    }

    /// Makes a file name safe to insert as one argument.
    pub fn quote_path(self, path: &str) -> String {
        if self.is_posix() {
            let mut out = String::with_capacity(path.len());
            for ch in path.chars() {
                if matches!(
                    ch,
                    ' ' | '\t'
                        | '\''
                        | '"'
                        | '\\'
                        | '('
                        | ')'
                        | '['
                        | ']'
                        | '{'
                        | '}'
                        | '&'
                        | ';'
                        | '|'
                        | '<'
                        | '>'
                        | '*'
                        | '?'
                        | '$'
                        | '`'
                        | '!'
                        | '#'
                ) {
                    out.push('\\');
                }
                out.push(ch);
            }
            out
        } else if path
            .chars()
            .any(|c| c.is_whitespace() || matches!(c, '&' | '(' | ')' | ';' | '\''))
        {
            match self {
                Shell::Cmd => format!("\"{path}\""),
                _ => format!("'{}'", path.replace('\'', "''")),
            }
        } else {
            path.to_string()
        }
    }

    /// The user's history file for this shell, if it can be located.
    pub fn history_file(self) -> Option<PathBuf> {
        let home = home_dir()?;
        match self {
            Shell::Zsh => Some(
                std::env::var_os("HISTFILE")
                    .map(PathBuf::from)
                    .filter(|p| p.is_file())
                    .unwrap_or_else(|| zdotdir().unwrap_or_else(|| home.clone()).join(".zsh_history")),
            ),
            Shell::Bash => Some(home.join(".bash_history")),
            Shell::Fish => Some(
                std::env::var_os("XDG_DATA_HOME")
                    .map(PathBuf::from)
                    .unwrap_or_else(|| home.join(".local").join("share"))
                    .join("fish")
                    .join("fish_history"),
            ),
            Shell::Pwsh | Shell::Powershell => {
                if cfg!(windows) {
                    std::env::var_os("APPDATA").map(|appdata| {
                        PathBuf::from(appdata)
                            .join("Microsoft")
                            .join("Windows")
                            .join("PowerShell")
                            .join("PSReadLine")
                            .join("ConsoleHost_history.txt")
                    })
                } else {
                    Some(
                        home.join(".local")
                            .join("share")
                            .join("powershell")
                            .join("PSReadLine")
                            .join("ConsoleHost_history.txt"),
                    )
                }
            }
            Shell::Cmd => None,
        }
    }
}

pub fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
}

/// The directory zsh reads the user's own startup files from. Inside a
/// hiruzen session `ZDOTDIR` points at hiruzen's files and the user's is kept
/// in `HIRUZEN_USER_ZDOTDIR`.
pub fn zdotdir() -> Option<PathBuf> {
    std::env::var_os("HIRUZEN_USER_ZDOTDIR")
        .or_else(|| std::env::var_os("ZDOTDIR"))
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
}

/// Finds an executable by name on `PATH`, trying Windows executable
/// extensions where they apply.
pub fn find_on_path(name: &str) -> Option<PathBuf> {
    let direct = Path::new(name);
    if direct.components().count() > 1 {
        return direct.is_file().then(|| direct.to_path_buf());
    }
    let path = std::env::var_os("PATH")?;
    let extensions: &[&str] = if cfg!(windows) {
        &["", ".exe", ".cmd", ".bat"]
    } else {
        &[""]
    };
    for dir in std::env::split_paths(&path) {
        for ext in extensions {
            let candidate = dir.join(format!("{name}{ext}"));
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

/// Locates Git Bash, which is the `bash` Windows users mean.
#[cfg(windows)]
fn git_bash() -> Option<PathBuf> {
    let mut roots: Vec<PathBuf> = Vec::new();
    if let Some(git) = find_on_path("git") {
        // <root>\cmd\git.exe or <root>\bin\git.exe
        if let Some(root) = git.parent().and_then(Path::parent) {
            roots.push(root.to_path_buf());
        }
    }
    for var in ["ProgramW6432", "ProgramFiles", "ProgramFiles(x86)"] {
        if let Some(dir) = std::env::var_os(var) {
            roots.push(PathBuf::from(dir).join("Git"));
        }
    }
    if let Some(local) = std::env::var_os("LocalAppData") {
        roots.push(PathBuf::from(local).join("Programs").join("Git"));
    }
    if let Some(profile) = std::env::var_os("UserProfile") {
        roots.push(
            PathBuf::from(profile)
                .join("scoop")
                .join("apps")
                .join("git")
                .join("current"),
        );
    }
    roots
        .into_iter()
        .flat_map(|root| {
            [
                root.join("bin").join("bash.exe"),
                root.join("usr").join("bin").join("bash.exe"),
            ]
        })
        .find(|candidate| candidate.is_file())
}

/// Everything needed to start a shell under hiruzen.
pub struct Launch {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    /// Temporary directory holding integration scripts; removed on exit.
    pub scratch: Option<PathBuf>,
}

impl Shell {
    fn program(self) -> Result<PathBuf, String> {
        #[cfg(windows)]
        if self == Shell::Bash {
            return git_bash().ok_or_else(|| "Git Bash was not found; install Git for Windows".to_string());
        }
        find_on_path(self.name()).ok_or_else(|| format!("{} was not found on PATH", self.name()))
    }

    /// Writes the integration scripts to a private directory and returns the
    /// command line that starts this shell with them loaded after the user's
    /// own startup files.
    pub fn launch(self, login: bool) -> Result<Launch, String> {
        let program = self.program()?;
        let scratch = std::env::temp_dir().join(format!("hiruzen-{}", std::process::id()));
        std::fs::create_dir_all(&scratch).map_err(|e| format!("cannot create {}: {e}", scratch.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&scratch, std::fs::Permissions::from_mode(0o700));
        }
        let write = |name: &str, content: &str| -> Result<PathBuf, String> {
            let path = scratch.join(name);
            std::fs::write(&path, content).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
            Ok(path)
        };

        let mut args: Vec<String> = Vec::new();
        let mut env: Vec<(String, String)> = vec![("HIRUZEN".to_string(), "1".to_string())];
        if login {
            env.push(("HIRUZEN_LOGIN".to_string(), "1".to_string()));
        }

        match self {
            Shell::Zsh => {
                write(".zshrc", ZSH_RC)?;
                write(".zshenv", ZSH_ENV)?;
                write(".zprofile", ZSH_PROFILE)?;
                write(".zlogin", ZSH_LOGIN)?;
                let user = std::env::var_os("ZDOTDIR")
                    .map(PathBuf::from)
                    .filter(|p| !p.as_os_str().is_empty())
                    .or_else(home_dir)
                    .unwrap_or_else(|| PathBuf::from("/"));
                env.push(("HIRUZEN_USER_ZDOTDIR".to_string(), user.to_string_lossy().into_owned()));
                env.push(("ZDOTDIR".to_string(), scratch.to_string_lossy().into_owned()));
                let settings = crate::settings::Settings::load();
                if settings.prompt {
                    env.push(("HIRUZEN_PROMPT".to_string(), "1".to_string()));
                    if settings.prompt_host {
                        env.push(("HIRUZEN_PROMPT_HOST".to_string(), "1".to_string()));
                    }
                    env.push((
                        "HIRUZEN_PROMPT_DIRECTORY".to_string(),
                        settings.prompt_directory_length.to_string(),
                    ));
                    env.push((
                        "HIRUZEN_PROMPT_BRANCH".to_string(),
                        settings.prompt_branch_length.to_string(),
                    ));
                }
                if login {
                    args.push("--login".to_string());
                }
            }
            Shell::Bash => {
                let script = write("integration.bash", BASH)?;
                args.push("--init-file".to_string());
                args.push(script.to_string_lossy().into_owned());
            }
            Shell::Fish => {
                let script = write("integration.fish", FISH)?;
                if login {
                    args.push("--login".to_string());
                }
                args.push("--init-command".to_string());
                args.push(format!("source '{}'", script.to_string_lossy().replace('\'', "\\'")));
            }
            Shell::Pwsh | Shell::Powershell => {
                let script = write("integration.ps1", POWERSHELL)?;
                if login && self == Shell::Pwsh && !cfg!(windows) {
                    args.push("-Login".to_string());
                }
                args.push("-NoLogo".to_string());
                args.push("-NoExit".to_string());
                args.push("-ExecutionPolicy".to_string());
                args.push("Bypass".to_string());
                args.push("-Command".to_string());
                args.push(format!(
                    "try {{ . '{}' }} catch {{}}",
                    script.to_string_lossy().replace('\'', "''")
                ));
            }
            Shell::Cmd => {
                // cmd has no startup script hook that fits; its prompt string carries the markers.
                let prompt = std::env::var("PROMPT")
                    .ok()
                    .filter(|p| !p.is_empty())
                    .unwrap_or_else(|| "$P$G".to_string());
                env.push(("PROMPT".to_string(), format!("$E]7391;PS$E\\{prompt}$E]7391;PE$E\\")));
            }
        }

        Ok(Launch {
            program,
            args,
            env,
            scratch: Some(scratch),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognises_shell_names_and_paths() {
        assert_eq!(Shell::from_name("/bin/zsh"), Some(Shell::Zsh));
        assert_eq!(Shell::from_name("-bash"), Some(Shell::Bash));
        assert_eq!(Shell::from_name("pwsh.exe"), Some(Shell::Pwsh));
        assert_eq!(Shell::from_name("PowerShell"), Some(Shell::Powershell));
        assert_eq!(Shell::from_name("nu"), None);
    }

    #[test]
    fn quotes_paths_per_shell() {
        assert_eq!(Shell::Zsh.quote_path("my file (1).txt"), r"my\ file\ \(1\).txt");
        assert_eq!(Shell::Pwsh.quote_path("my file.txt"), "'my file.txt'");
        assert_eq!(Shell::Cmd.quote_path("my file.txt"), "\"my file.txt\"");
        assert_eq!(Shell::Cmd.quote_path("plain.txt"), "plain.txt");
    }
}
