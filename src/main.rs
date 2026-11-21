//! hiruzen: dropdown autocomplete for the terminal.

mod commands;
mod engine;
mod settings;
mod shell;
mod specs;
mod term;

use std::io::IsTerminal;
use std::process::ExitCode;

use engine::Engine;
use shell::Shell;

const HELP: &str = "\
hiruzen - suggests the next word as you type in your terminal

USAGE:
    hiruzen [OPTIONS]              Start your shell with suggestions
    hiruzen init <SHELL>           Print the line that starts hiruzen with each new terminal
    hiruzen install [SHELL...]     Add that line to your shell's startup file for you
    hiruzen uninstall [SHELL...]   Remove it again
    hiruzen integrations status    Show which shells start hiruzen
    hiruzen settings [KEY [VALUE]] List, read or change settings (also: docs, open, --delete KEY)
    hiruzen theme [NAME | --list]  Show or change the dropdown's colours
    hiruzen doctor                 Check that hiruzen is set up correctly
    hiruzen issue [TITLE]          Open a new issue with your system details filled in
    hiruzen complete <COMMAND>     Print the suggestions for a partly typed command
    hiruzen --help | --version

OPTIONS:
    -s, --shell <SHELL>    Shell to start: zsh, bash, fish, pwsh, powershell or cmd
                           (default: your login shell; PowerShell on Windows)
    -l, --login            Start it as a login shell
        --auto             Exit quietly when hiruzen is already running or there is no
                           terminal; used by the line `hiruzen init` prints

KEYS (while suggestions are showing):
    Tab         Accept the highlighted suggestion
    Up / Down   Move the highlight
    Esc         Hide suggestions for the current word
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(code) => ExitCode::from(code.clamp(0, 255) as u8),
        Err(message) => {
            eprintln!("hiruzen: {message}");
            ExitCode::from(2)
        }
    }
}

fn run(args: &[String]) -> Result<i32, String> {
    match args.first().map(String::as_str) {
        Some("-h" | "--help" | "help") => {
            print!("{HELP}");
            Ok(0)
        }
        Some("-V" | "--version") => {
            println!("hiruzen {}", env!("CARGO_PKG_VERSION"));
            Ok(0)
        }
        Some("init") => init(args.get(1).map(String::as_str)),
        Some("complete") => complete(&args[1..]),
        Some("settings") => commands::settings(&args[1..]),
        Some("theme") => commands::theme(&args[1..]),
        Some("integrations" | "integration") => commands::integrations(&args[1..]),
        Some(action @ ("install" | "uninstall")) => {
            let mut forwarded = vec![action.to_string()];
            forwarded.extend_from_slice(&args[1..]);
            commands::integrations(&forwarded)
        }
        Some("doctor" | "diagnostic" | "diagnostics") => commands::doctor(),
        Some("issue") => commands::issue(&args[1..]),
        _ => start(args),
    }
}

fn parse_shell(name: &str) -> Result<Shell, String> {
    Shell::from_name(name)
        .ok_or_else(|| format!("unknown shell '{name}'; expected zsh, bash, fish, pwsh, powershell or cmd"))
}

fn start(args: &[String]) -> Result<i32, String> {
    let mut shell: Option<Shell> = None;
    let mut login = false;
    let mut auto = false;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "-s" | "--shell" => {
                let name = iter.next().ok_or("--shell needs a value")?;
                shell = Some(parse_shell(name)?);
            }
            "-l" | "--login" => login = true,
            "--auto" => auto = true,
            other => return Err(format!("unknown argument '{other}'; see hiruzen --help")),
        }
    }

    let nested = std::env::var_os("HIRUZEN").is_some();
    let interactive = std::io::stdin().is_terminal() && std::io::stdout().is_terminal();
    if auto && (nested || !interactive) {
        // Tell the calling shell to carry on as an ordinary shell.
        return Ok(3);
    }
    if nested {
        return Err("already running in this terminal".to_string());
    }
    if !interactive {
        return Err("needs an interactive terminal".to_string());
    }

    let code = term::session::run(shell.unwrap_or_else(Shell::detect), login)?;
    // Under --auto the calling shell exits when hiruzen returns 0, so a
    // session that ran reports success whatever its last command returned.
    Ok(if auto { 0 } else { code })
}

/// Prints the snippet that makes new terminals start inside hiruzen.
fn init(shell: Option<&str>) -> Result<i32, String> {
    let shell = parse_shell(shell.ok_or("usage: hiruzen init <zsh|bash|fish|pwsh|powershell>")?)?;
    // If hiruzen cannot start (exit status other than 0), the shell carries on
    // normally, so a broken install never locks anyone out of their terminal.
    let snippet = match shell {
        Shell::Zsh => {
            "if [[ -z \"$HIRUZEN\" && -o interactive && -z \"$ZSH_EXECUTION_STRING\" ]] && (( $+commands[hiruzen] )); then\n  if [[ -o login ]]; then hiruzen --auto --shell zsh --login && exit; else hiruzen --auto --shell zsh && exit; fi\nfi\n"
        }
        Shell::Bash => {
            "if [[ -z \"$HIRUZEN\" && $- == *i* ]] && command -v hiruzen >/dev/null 2>&1; then\n  if shopt -q login_shell; then hiruzen --auto --shell bash --login && exit; else hiruzen --auto --shell bash && exit; fi\nfi\n"
        }
        Shell::Fish => {
            "if test -z \"$HIRUZEN\"; and status is-interactive; and type -q hiruzen\n  if status is-login\n    hiruzen --auto --shell fish --login; and exit\n  else\n    hiruzen --auto --shell fish; and exit\n  end\nend\n"
        }
        Shell::Pwsh | Shell::Powershell => {
            if shell == Shell::Pwsh {
                "if (-not $env:HIRUZEN -and [Environment]::UserInteractive -and (Get-Command hiruzen -ErrorAction SilentlyContinue)) {\n  hiruzen --auto --shell pwsh\n  if ($LASTEXITCODE -eq 0) { exit }\n}\n"
            } else {
                "if (-not $env:HIRUZEN -and [Environment]::UserInteractive -and (Get-Command hiruzen -ErrorAction SilentlyContinue)) {\n  hiruzen --auto --shell powershell\n  if ($LASTEXITCODE -eq 0) { exit }\n}\n"
            }
        }
        Shell::Cmd => {
            return Err("cmd has no startup file; run `hiruzen --shell cmd`, or set that as the command line of a terminal profile".to_string())
        }
    };
    print!("{snippet}");
    Ok(0)
}

/// Prints suggestions for a command line without starting a session. Useful
/// for checking what hiruzen knows about a command.
fn complete(args: &[String]) -> Result<i32, String> {
    let mut shell = Shell::detect();
    let mut words: Vec<&str> = Vec::new();
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "-s" | "--shell" => shell = parse_shell(iter.next().ok_or("--shell needs a value")?)?,
            "--" => words.extend(iter.by_ref().map(String::as_str)),
            other => words.push(other),
        }
    }
    if words.is_empty() {
        return Err("usage: hiruzen complete \"git ch\"   (end with a space to complete the next word)".to_string());
    }
    let line = words.join(" ");
    let cwd = std::env::current_dir().map_err(|e| format!("cannot read the current directory: {e}"))?;
    let engine = Engine::system(shell);
    let settings = settings::Settings::load();
    if settings.disable {
        return Ok(1);
    }
    engine.set_options(engine::Options {
        fuzzy: settings.fuzzy,
        history: settings.history,
        disabled_commands: settings.disabled_commands,
    });
    let Some(completion) = engine.complete(&line, &cwd) else {
        return Ok(1);
    };
    for item in completion.items.iter().take(30) {
        match &item.description {
            Some(description) => println!("{:<28} {}", item.label(), description.lines().next().unwrap_or("")),
            None => println!("{}", item.label()),
        }
    }
    if completion.items.is_empty() {
        if let Some(hint) = completion.hint {
            println!("({hint})");
        }
    }
    Ok(0)
}
