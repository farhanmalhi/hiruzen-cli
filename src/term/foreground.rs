//! Direct access to the real terminal on Unix: reading keys without blocking
//! for good, asking where the cursor is, and handing the terminal over to a
//! program for as long as it runs.
//!
//! A command named in `terminal.directCommands` runs on the real terminal as
//! its foreground process, not inside hiruzen's pseudo-terminal. Tools that
//! watch a terminal by its foreground process (multiplexers that track
//! coding agents, for example) then see that command instead of hiruzen.

use std::ffi::{OsStr, OsString};
use std::io::Write;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

const STDIN: libc::c_int = 0;

/// Waits up to `timeout` for keys and reads what has arrived. `Ok(None)`
/// means none came in time; `Ok(Some(0))` means the terminal is gone.
pub fn read_input(buf: &mut [u8], timeout: Duration) -> std::io::Result<Option<usize>> {
    let mut poll = libc::pollfd {
        fd: STDIN,
        events: libc::POLLIN,
        revents: 0,
    };
    let millis = libc::c_int::try_from(timeout.as_millis()).unwrap_or(libc::c_int::MAX);
    // SAFETY: one valid pollfd for the duration of the call.
    let ready = unsafe { libc::poll(&mut poll, 1, millis) };
    if ready < 0 {
        let error = std::io::Error::last_os_error();
        return if error.kind() == std::io::ErrorKind::Interrupted {
            Ok(None)
        } else {
            Err(error)
        };
    }
    if ready == 0 {
        return Ok(None);
    }
    // SAFETY: reads into a buffer of the length given.
    let read = unsafe { libc::read(STDIN, buf.as_mut_ptr().cast(), buf.len()) };
    if read < 0 {
        let error = std::io::Error::last_os_error();
        return match error.kind() {
            std::io::ErrorKind::Interrupted | std::io::ErrorKind::WouldBlock => Ok(None),
            _ => Err(error),
        };
    }
    Ok(Some(read as usize))
}

/// Finds a cursor position report, `ESC [ row ; col R`, in `data`. Returns
/// the zero-based position and the byte range the report takes.
fn find_report(data: &[u8]) -> Option<((u16, u16), std::ops::Range<usize>)> {
    let mut start = 0;
    while let Some(offset) = data[start..].windows(2).position(|w| w == b"\x1b[") {
        let at = start + offset;
        let rest = &data[at + 2..];
        let end = rest.iter().position(|b| !(b.is_ascii_digit() || *b == b';'));
        if let Some(end) = end.filter(|&end| rest[end] == b'R') {
            let text = std::str::from_utf8(&rest[..end]).ok()?;
            if let Some((row, col)) = text.split_once(';') {
                if let (Ok(row), Ok(col)) = (row.parse::<u16>(), col.parse::<u16>()) {
                    return Some(((row.saturating_sub(1), col.saturating_sub(1)), at..at + 2 + end + 1));
                }
            }
        }
        start = at + 1;
    }
    None
}

/// Asks the terminal where the cursor is. The terminal must be in raw mode
/// and nothing else may be reading the keyboard. Keys typed meanwhile are
/// returned alongside, to be passed on to the shell.
pub fn query_cursor(timeout: Duration) -> (Option<(u16, u16)>, Vec<u8>) {
    let mut out = std::io::stdout().lock();
    if out.write_all(b"\x1b[6n").and_then(|()| out.flush()).is_err() {
        return (None, Vec::new());
    }
    drop(out);
    let deadline = Instant::now() + timeout;
    let mut received = Vec::new();
    let mut buf = [0u8; 1024];
    loop {
        if let Some((position, range)) = find_report(&received) {
            received.drain(range);
            return (Some(position), received);
        }
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return (None, received);
        }
        match read_input(&mut buf, left) {
            Ok(Some(0)) | Err(_) => return (None, received),
            Ok(Some(n)) => received.extend_from_slice(&buf[..n]),
            Ok(None) => {}
        }
    }
}

/// Makes `group` the terminal's foreground process group. A process outside
/// that group is normally stopped for trying; SIGTTOU is ignored for the call.
///
/// # Safety
/// Only async-signal-safe calls, so it may run between fork and exec.
unsafe fn give_terminal(group: libc::pid_t) {
    let previous = libc::signal(libc::SIGTTOU, libc::SIG_IGN);
    libc::tcsetpgrp(STDIN, group);
    libc::signal(libc::SIGTTOU, previous);
}

/// Runs a program on the real terminal as its foreground process and waits
/// for it. hiruzen's raw mode is lifted while it runs. Returns its exit
/// status as a shell would report it: 128 plus the signal number when a
/// signal ended it.
pub fn run(
    program: &Path,
    name: &OsStr,
    args: &[OsString],
    cwd: &Path,
    env: &[(OsString, OsString)],
) -> Result<i32, String> {
    let _ = crossterm::terminal::disable_raw_mode();
    let result = spawn_and_wait(program, name, args, cwd, env);
    // SAFETY: plain system calls on hiruzen's own process.
    unsafe { give_terminal(libc::getpgrp()) };
    let _ = crossterm::terminal::enable_raw_mode();
    result
}

fn spawn_and_wait(
    program: &Path,
    name: &OsStr,
    args: &[OsString],
    cwd: &Path,
    env: &[(OsString, OsString)],
) -> Result<i32, String> {
    let mut command = Command::new(program);
    command
        .arg0(name)
        .args(args)
        .current_dir(cwd)
        .env_clear()
        .envs(env.iter().map(|(key, value)| (key, value)));
    // SAFETY: the closure makes only async-signal-safe calls.
    unsafe {
        command.pre_exec(|| {
            // Its own process group takes over the terminal, so it gets the
            // keyboard and Ctrl+C. Done before the program starts, so it
            // never reads the terminal from the background and is stopped.
            libc::setpgid(0, 0);
            give_terminal(libc::getpid());
            Ok(())
        });
    }
    let child = command
        .spawn()
        .map_err(|e| format!("{}: {e}", name.to_string_lossy()))?;
    let pid = child.id() as libc::pid_t;
    // The same call in the parent closes the window before the child has
    // made it, as shells do.
    // SAFETY: plain system calls on a child of this process.
    unsafe {
        libc::setpgid(pid, pid);
    }
    loop {
        let mut status: libc::c_int = 0;
        // SAFETY: waits on our own child with a valid status pointer.
        let waited = unsafe { libc::waitpid(pid, &mut status, libc::WUNTRACED) };
        if waited < 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return Err(format!("waiting for {}: {error}", name.to_string_lossy()));
        }
        if libc::WIFSTOPPED(status) {
            // Ctrl+Z stops it while it holds the terminal, and there is no
            // job control out here to bring it back, so it carries on.
            // SAFETY: signals our own child's process group.
            unsafe {
                libc::kill(-pid, libc::SIGCONT);
            }
            continue;
        }
        if libc::WIFEXITED(status) {
            return Ok(libc::WEXITSTATUS(status));
        }
        if libc::WIFSIGNALED(status) {
            return Ok(128 + libc::WTERMSIG(status));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_a_cursor_report_among_typed_keys() {
        let data = b"ls\x1b[12;5Rx";
        let (position, range) = find_report(data).unwrap();
        assert_eq!(position, (11, 4));
        assert_eq!(&data[range], b"\x1b[12;5R");
        // An arrow key is not a report.
        assert!(find_report(b"\x1b[A").is_none());
        assert_eq!(find_report(b"\x1b[A\x1b[1;1R").unwrap().0, (0, 0));
    }
}
