//! `hiruzen ssh`: connects with ssh and starts the remote shell with
//! hiruzen's prompt markers, so suggestions keep working there.
//!
//! Nothing is installed on the other machine. The integration scripts travel
//! inside the ssh command, are written to a temporary directory there, and
//! are removed when the remote shell exits. hiruzen itself stays on this
//! machine and reads the remote shell's prompt through the connection.

use std::process::Command;

use crate::shell;

/// ssh options that take a value, so the word after them is not the host.
const OPTIONS_WITH_VALUE: &str = "BbcDEeFIiJLlmOoPpQRSWw";
/// ssh options that mean "this is not an interactive login".
const NON_INTERACTIVE: &str = "NfTGQVWs";

#[derive(Debug, PartialEq)]
enum Kind {
    /// A login shell on a host: the case hiruzen can help with.
    Interactive,
    /// A remote command, a tunnel, or anything else to pass through untouched.
    Other,
}

/// Decides from ssh's own arguments whether this is an interactive login.
fn classify(args: &[String]) -> Kind {
    let mut host_seen = false;
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        i += 1;
        if host_seen {
            // Anything after the host is a command to run there.
            return Kind::Other;
        }
        if arg == "--" {
            continue;
        }
        match arg.strip_prefix('-') {
            Some(flags) if !flags.is_empty() => {
                for (at, flag) in flags.char_indices() {
                    if NON_INTERACTIVE.contains(flag) {
                        return Kind::Other;
                    }
                    if OPTIONS_WITH_VALUE.contains(flag) {
                        // The value is the rest of this word, or the next word.
                        if at + flag.len_utf8() == flags.len() {
                            i += 1;
                        }
                        break;
                    }
                }
            }
            _ => host_seen = true,
        }
    }
    if host_seen {
        Kind::Interactive
    } else {
        Kind::Other
    }
}

fn base64(input: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let bytes = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = u32::from(bytes[0]) << 16 | u32::from(bytes[1]) << 8 | u32::from(bytes[2]);
        out.push(ALPHABET[(n >> 18) as usize & 63] as char);
        out.push(ALPHABET[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

/// The script run on the remote machine, in POSIX sh. It writes the
/// integration scripts for the user's login shell to a temporary directory,
/// starts that shell with them, and removes the directory afterwards. A
/// machine or shell it cannot handle gets an ordinary login shell.
fn remote_script() -> String {
    let file = |name: &str, content: &str| format!("w {name} <<'HIRUZEN_EOF'\n{}\nHIRUZEN_EOF\n", content.trim_end());
    let mut script = String::from(
        "d=$(mktemp -d 2>/dev/null) || exec \"${SHELL:-/bin/sh}\" -l\n\
         trap 'rm -rf \"$d\"' EXIT HUP TERM\n\
         export HIRUZEN=1 HIRUZEN_REMOTE=1\n\
         w() { cat > \"$d/$1\"; }\n\
         case \"${SHELL##*/}\" in\n\
         zsh)\n",
    );
    script.push_str(&file(".zshenv", shell::ZSH_ENV));
    script.push_str(&file(".zprofile", shell::ZSH_PROFILE));
    script.push_str(&file(".zshrc", shell::ZSH_RC));
    script.push_str(&file(".zlogin", shell::ZSH_LOGIN));
    script.push_str("HIRUZEN_USER_ZDOTDIR=\"${ZDOTDIR:-$HOME}\" ZDOTDIR=\"$d\" zsh -l\n;;\nbash)\n");
    script.push_str(&file("rc", shell::BASH));
    script.push_str("HIRUZEN_LOGIN=1 bash --rcfile \"$d/rc\" -i\n;;\nfish)\n");
    script.push_str(&file("init.fish", shell::FISH));
    script.push_str("fish -l --init-command \"source '$d/init.fish'\"\n;;\n*)\n\"${SHELL:-/bin/sh}\" -l\n;;\nesac\n");
    script
}

/// The remote command line. It is one single-quoted word made only of
/// characters that every common login shell (sh, bash, zsh, fish, csh)
/// reads the same way, with the script itself carried as base64.
fn remote_command() -> String {
    let encoded = base64(remote_script().as_bytes());
    format!(
        "exec sh -c 'b={encoded}; s=$(echo $b | base64 -d 2>/dev/null) || s=$(echo $b | base64 --decode); eval \"$s\"'"
    )
}

/// `hiruzen ssh [SSH ARGUMENTS]`
pub fn run(args: &[String]) -> Result<i32, String> {
    let ssh = shell::find_on_path("ssh").ok_or("ssh was not found on PATH")?;
    let mut command = Command::new(ssh);
    // Outside a hiruzen session nobody would read the markers, and anything
    // other than a login shell must reach ssh exactly as typed.
    let in_session = std::env::var_os("HIRUZEN").is_some();
    if in_session && classify(args) == Kind::Interactive {
        command.arg("-t").args(args).arg(remote_command());
    } else {
        command.args(args);
    }
    let status = command.status().map_err(|e| format!("cannot start ssh: {e}"))?;
    Ok(status.code().unwrap_or(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kind(line: &str) -> Kind {
        classify(&line.split_whitespace().map(str::to_string).collect::<Vec<_>>())
    }

    #[test]
    fn recognises_an_interactive_login() {
        assert_eq!(kind("host"), Kind::Interactive);
        assert_eq!(kind("-p 2222 -i key me@host"), Kind::Interactive);
        assert_eq!(kind("-vA -p2222 -o StrictHostKeyChecking=no host"), Kind::Interactive);
        assert_eq!(kind("-J jump host"), Kind::Interactive);
    }

    #[test]
    fn passes_everything_else_through() {
        assert_eq!(kind("host uptime"), Kind::Other);
        assert_eq!(kind("-N -L 8080:localhost:80 host"), Kind::Other);
        assert_eq!(kind("-T git@github.com"), Kind::Other);
        assert_eq!(kind("-V"), Kind::Other);
        assert_eq!(kind("-fN host"), Kind::Other);
        assert_eq!(kind(""), Kind::Other);
    }

    #[test]
    fn encodes_base64() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn the_remote_command_is_safe_inside_single_quotes() {
        let command = remote_command();
        let body = command
            .strip_prefix("exec sh -c '")
            .and_then(|rest| rest.strip_suffix('\''))
            .expect("quoted");
        assert!(!body.contains('\''), "a quote would end the word early");
        assert!(!body.contains('\\'), "shells disagree about backslashes in quotes");
        // The integration scripts must not contain the here-document marker.
        for script in [
            shell::ZSH_RC,
            shell::ZSH_ENV,
            shell::ZSH_PROFILE,
            shell::ZSH_LOGIN,
            shell::BASH,
            shell::FISH,
        ] {
            assert!(!script.contains("HIRUZEN_EOF"));
        }
    }
}
