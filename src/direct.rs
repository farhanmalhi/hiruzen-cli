//! `hiruzen direct COMMAND [ARGS]`: runs a command straight on the real
//! terminal, outside hiruzen's pseudo-terminal.
//!
//! Inside a session the shell defines a function for each command named in
//! `terminal.directCommands` that calls this. It runs in the inner shell, so
//! it carries that shell's environment, directory and arguments exactly.
//! It sends them over a private socket to the session, which pauses, runs
//! the command as the terminal's foreground process, and replies with its
//! exit status once it ends. The shell, with its variables and directory,
//! carries on as it was.
//!
//! Anywhere it cannot do that (outside a session, in a pipeline, in a
//! background job, on Windows), the command simply runs as usual.

#[cfg(unix)]
use serde::{Deserialize, Serialize};

/// Names the socket the session listens on.
#[cfg(unix)]
pub const SOCKET_VARIABLE: &str = "HIRUZEN_DIRECT_SOCKET";
/// Space-separated commands the shell runs through `hiruzen direct`.
#[cfg(unix)]
pub const COMMANDS_VARIABLE: &str = "HIRUZEN_DIRECT";
/// The hiruzen binary, for the shell functions to call.
#[cfg(unix)]
pub const EXE_VARIABLE: &str = "HIRUZEN_EXE";

/// What to run. Byte strings, because names, arguments and the environment
/// need not be UTF-8.
#[cfg(unix)]
#[derive(Debug, Serialize, Deserialize)]
pub struct Request {
    pub program: Vec<u8>,
    pub name: Vec<u8>,
    pub args: Vec<Vec<u8>>,
    pub cwd: Vec<u8>,
    pub env: Vec<(Vec<u8>, Vec<u8>)>,
}

#[cfg(unix)]
#[derive(Debug, Serialize, Deserialize)]
pub struct Reply {
    pub status: i32,
    pub error: Option<String>,
}

/// Whether a command name can be put in the shell's function definitions
/// as it is: no quoting, spaces or option-like leading dash.
#[cfg(unix)]
pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('-')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '+' | '-'))
}

#[cfg(unix)]
pub fn run(args: &[std::ffi::OsString]) -> Result<i32, String> {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::ffi::{OsStrExt, OsStringExt};
    use std::os::unix::net::UnixStream;
    use std::os::unix::process::CommandExt;
    use std::path::PathBuf;

    let args = match args.first() {
        Some(first) if first == "--" => &args[1..],
        _ => args,
    };
    let Some((name, rest)) = args.split_first() else {
        return Err("usage: hiruzen direct COMMAND [ARGUMENTS]".to_string());
    };
    let program = if name.as_bytes().contains(&b'/') {
        Some(PathBuf::from(name))
    } else {
        name.to_str().and_then(crate::shell::find_on_path)
    };
    let Some(program) = program else {
        eprintln!("hiruzen: {}: command not found", name.to_string_lossy());
        return Ok(127);
    };

    let run_here = |error: Option<String>| -> Result<i32, String> {
        if let Some(error) = error {
            eprintln!("hiruzen: {error}; running {} inside hiruzen", name.to_string_lossy());
        }
        let error = std::process::Command::new(&program).arg0(name).args(rest).exec();
        eprintln!("hiruzen: {}: {error}", name.to_string_lossy());
        Ok(126)
    };

    let Some(socket) = std::env::var_os(SOCKET_VARIABLE).filter(|s| !s.is_empty()) else {
        return run_here(None);
    };
    // Only a command in the foreground with the terminal for all of its input
    // and output can move to the real terminal. In a pipeline or a
    // background job it stays where it is.
    // SAFETY: plain queries about this process and its standard streams.
    let foreground = unsafe {
        libc::isatty(0) == 1 && libc::isatty(1) == 1 && libc::isatty(2) == 1 && libc::tcgetpgrp(0) == libc::getpgrp()
    };
    if !foreground {
        return run_here(None);
    }
    let Ok(mut stream) = UnixStream::connect(&socket) else {
        return run_here(Some("cannot reach the hiruzen session".to_string()));
    };

    let cwd = std::env::current_dir().map_err(|e| format!("cannot read the current directory: {e}"))?;
    let request = Request {
        program: program.into_os_string().into_vec(),
        name: name.as_bytes().to_vec(),
        args: rest.iter().map(|arg| arg.as_bytes().to_vec()).collect(),
        cwd: cwd.into_os_string().into_vec(),
        env: std::env::vars_os()
            .filter(|(key, _)| key != SOCKET_VARIABLE)
            .map(|(key, value)| (key.into_vec(), value.into_vec()))
            .collect(),
    };
    let mut line = serde_json::to_vec(&request).map_err(|e| e.to_string())?;
    line.push(b'\n');
    stream
        .write_all(&line)
        .map_err(|e| format!("cannot reach the hiruzen session: {e}"))?;

    let mut answer = String::new();
    BufReader::new(stream)
        .read_line(&mut answer)
        .map_err(|e| format!("lost the hiruzen session: {e}"))?;
    let reply: Reply = serde_json::from_str(&answer)
        .map_err(|_| format!("lost the hiruzen session while {} ran", name.to_string_lossy()))?;
    if let Some(error) = reply.error {
        eprintln!("hiruzen: {error}");
    }
    Ok(reply.status)
}

#[cfg(not(unix))]
pub fn run(args: &[std::ffi::OsString]) -> Result<i32, String> {
    let args = match args.first() {
        Some(first) if first == "--" => &args[1..],
        _ => args,
    };
    let Some((name, rest)) = args.split_first() else {
        return Err("usage: hiruzen direct COMMAND [ARGUMENTS]".to_string());
    };
    let status = std::process::Command::new(name)
        .args(rest)
        .status()
        .map_err(|e| format!("{}: {e}", name.to_string_lossy()))?;
    Ok(status.code().unwrap_or(1))
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn accepts_only_names_safe_in_a_function_definition() {
        for name in ["claude", "codex", "gh-copilot", "node.js", "c++", "a_b"] {
            assert!(valid_name(name), "{name}");
        }
        for name in ["", "-rf", "a b", "x;rm", "$(x)", "a'b", "ä"] {
            assert!(!valid_name(name), "{name}");
        }
    }
}
