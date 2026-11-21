//! Removes secrets from a command before hiruzen remembers it.
//!
//! People pass tokens and passwords on the command line. The command is
//! still run as typed; only the copy hiruzen keeps is changed.

pub const REDACTED: &str = "<redacted>";

/// Words in an option or variable name that mark its value as a secret.
const SECRET_NAMES: &[&str] = &[
    "token",
    "secret",
    "password",
    "passwd",
    "passphrase",
    "apikey",
    "api_key",
    "api-key",
    "auth",
    "credential",
    "private",
    "access_key",
    "access-key",
];

/// Prefixes that well-known services give their tokens.
const SECRET_PREFIXES: &[&str] = &[
    "ghp_",
    "gho_",
    "ghu_",
    "ghs_",
    "ghr_",
    "github_pat_",
    "glpat-",
    "sk-",
    "pk_live_",
    "sk_live_",
    "rk_live_",
    "xoxb-",
    "xoxp-",
    "xoxa-",
    "xoxs-",
    "AKIA",
    "ASIA",
    "AIza",
    "ya29.",
    "npm_",
    "dop_v1_",
    "shpat_",
    "hf_",
];

fn names_a_secret(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    SECRET_NAMES.iter().any(|secret| lower.contains(secret))
}

/// True for text shaped like a token: a known prefix, a JWT, or a long run
/// of mixed letters and digits. A commit hash is long but not secret, so
/// plain hexadecimal is left alone.
fn looks_like_a_secret(word: &str) -> bool {
    let word = word.trim_matches(|c| matches!(c, '"' | '\''));
    if SECRET_PREFIXES.iter().any(|prefix| word.starts_with(prefix)) && word.len() >= 16 {
        return true;
    }
    if word.starts_with("eyJ") && word.matches('.').count() == 2 && word.len() >= 40 {
        return true;
    }
    let token_chars = word
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '/' | '=' | '_' | '-'));
    let hex = word.chars().all(|c| c.is_ascii_hexdigit());
    token_chars
        && !hex
        && word.len() >= 32
        && word.chars().any(|c| c.is_ascii_digit())
        && word.chars().any(|c| c.is_ascii_lowercase())
        && word.chars().any(|c| c.is_ascii_uppercase())
}

/// `scheme://user:password@host` with the password replaced.
fn without_url_password(word: &str) -> Option<String> {
    let scheme_end = word.find("://")? + 3;
    let rest = &word[scheme_end..];
    let at = rest.find('@')?;
    let colon = rest[..at].find(':')?;
    Some(format!(
        "{}{}:{REDACTED}{}",
        &word[..scheme_end],
        &rest[..colon],
        &rest[at..]
    ))
}

/// The command with anything that looks like a secret replaced.
pub fn redact(command: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    // The value of `--password hunter2` is the word after the option.
    let mut next_is_secret = false;
    for word in command.split(' ') {
        if word.is_empty() {
            out.push(String::new());
            continue;
        }
        if std::mem::take(&mut next_is_secret) && !word.starts_with('-') {
            out.push(REDACTED.to_string());
            continue;
        }
        if let Some((name, value)) = word.split_once('=') {
            if !value.is_empty() && (names_a_secret(name) || looks_like_a_secret(value)) {
                out.push(format!("{name}={REDACTED}"));
                continue;
            }
        }
        if let Some(clean) = without_url_password(word) {
            out.push(clean);
            continue;
        }
        if looks_like_a_secret(word) {
            out.push(REDACTED.to_string());
            continue;
        }
        let bare = word.trim_matches(|c| matches!(c, '"' | '\''));
        if (word.starts_with('-') && names_a_secret(word))
            || bare.eq_ignore_ascii_case("bearer")
            || bare.eq_ignore_ascii_case("basic")
        {
            next_is_secret = true;
        }
        out.push(word.to_string());
    }
    out.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_values_of_secret_options_and_variables() {
        assert_eq!(
            redact("mysql --password hunter2 -u me"),
            "mysql --password <redacted> -u me"
        );
        assert_eq!(redact("tool --api-key=abc123 run"), "tool --api-key=<redacted> run");
        assert_eq!(
            redact("GITHUB_TOKEN=abc gh pr list"),
            "GITHUB_TOKEN=<redacted> gh pr list"
        );
        assert_eq!(
            redact("export AWS_SECRET_ACCESS_KEY=xyz"),
            "export AWS_SECRET_ACCESS_KEY=<redacted>"
        );
        assert_eq!(
            redact("curl -H 'Authorization: Bearer abc.def' x"),
            "curl -H 'Authorization: Bearer <redacted> x"
        );
    }

    #[test]
    fn redacts_things_shaped_like_tokens() {
        assert_eq!(
            redact("gh auth login --with-token ghp_abcdefghijklmnopqrstuvwxyz0123456789"),
            "gh auth login --with-token <redacted>"
        );
        assert_eq!(redact("echo sk-abcdefghijklmnopqrstuvwxyz012345"), "echo <redacted>");
        assert_eq!(redact("use Zx9Kq2Lm8Np4Rt6Vw1Yb3Cd5Fg7Hj0Ks2Lm"), "use <redacted>");
        assert_eq!(
            redact("git clone https://me:s3cret@example.com/r.git"),
            "git clone https://me:<redacted>@example.com/r.git"
        );
    }

    #[test]
    fn leaves_ordinary_commands_alone() {
        for command in [
            "git checkout a3c0f1e2b4d5a6978877665544332211ffeeddcc",
            "git commit -m 'fix the token parser'",
            "ls -la  src",
            "docker run --name web -p 8080:80 nginx",
            "cargo build --release",
            "ssh me@server",
            "curl https://example.com/path?x=1",
            "make PREFIX=/usr/local install",
        ] {
            assert_eq!(redact(command), command);
        }
    }
}
