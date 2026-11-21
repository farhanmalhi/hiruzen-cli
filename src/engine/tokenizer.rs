//! Splits the command line being typed into tokens.
//!
//! Only the last command of a pipeline or list is of interest: in
//! `cat x | git ch` the user is completing `git ch`.

use unicode_width::UnicodeWidthStr;

use crate::shell::Shell;

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Token {
    /// The token's value with quotes and escapes removed.
    pub text: String,
    /// Terminal cells the token occupies as typed, which is how many
    /// backspaces erase it.
    pub width: usize,
    /// Followed by whitespace, so the user has finished typing it.
    pub complete: bool,
    /// Started with a quote character.
    pub quoted: bool,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct CommandLine {
    /// Tokens the user has finished typing.
    pub done: Vec<Token>,
    /// The token under the cursor. Empty text when the cursor follows whitespace.
    pub active: Token,
}

/// Returns the text of the last command in `line`, after any `|`, `&&`, `||`, `;`
/// or `&` that is outside quotes.
pub fn last_command(line: &str, shell: Shell) -> &str {
    let escape = shell.escape_char();
    let mut quote: Option<char> = None;
    let mut start = 0;
    let mut prev: Option<char> = None;
    let mut chars = line.char_indices().peekable();
    while let Some((idx, ch)) = chars.next() {
        let escaped = prev == Some(escape);
        match quote {
            Some(q) => {
                if ch == q && !(escaped && q == '"') {
                    quote = None;
                }
            }
            None if escaped => {}
            None => match ch {
                '\'' | '"' | '`' => quote = Some(ch),
                '|' | ';' => start = idx + ch.len_utf8(),
                '&' => {
                    // `&&` and background `&` separate commands; `>&` and `&>` do not.
                    let redirect = prev == Some('>') || matches!(chars.peek(), Some((_, '>')));
                    if !redirect {
                        start = idx + ch.len_utf8();
                    }
                }
                _ => {}
            },
        }
        // An escaped escape character does not escape what follows.
        prev = if escaped && ch == escape { None } else { Some(ch) };
    }
    &line[start..]
}

/// Tokenizes the last command of `line`.
pub fn parse(line: &str, shell: Shell) -> CommandLine {
    let command = last_command(line, shell).trim_start();
    let escape = shell.escape_char();
    let mut done = Vec::new();

    let mut text = String::new();
    let mut raw = String::new();
    let mut in_token = false;
    let mut quoted = false;
    let mut quote: Option<char> = None;

    let mut chars = command.chars().peekable();
    while let Some(ch) = chars.next() {
        if let Some(q) = quote {
            raw.push(ch);
            if ch == q {
                quote = None;
            } else if ch == escape && q == '"' && matches!(chars.peek(), Some(&n) if n == q || n == escape) {
                let next = chars.next().unwrap();
                raw.push(next);
                text.push(next);
            } else {
                text.push(ch);
            }
            continue;
        }
        if ch.is_whitespace() {
            if in_token {
                done.push(Token {
                    text: std::mem::take(&mut text),
                    width: raw.width(),
                    complete: true,
                    quoted,
                });
                raw.clear();
                in_token = false;
                quoted = false;
            }
            continue;
        }
        if !in_token {
            in_token = true;
            quoted = matches!(ch, '\'' | '"');
        }
        raw.push(ch);
        match ch {
            '\'' | '"' => quote = Some(ch),
            c if c == escape && shell.is_posix() => {
                if let Some(next) = chars.next() {
                    raw.push(next);
                    text.push(next);
                }
            }
            c if c == escape => {
                // PowerShell's backtick and cmd's caret escape the next character.
                match chars.peek() {
                    Some(_) => {
                        let next = chars.next().unwrap();
                        raw.push(next);
                        text.push(next);
                    }
                    None => text.push(c),
                }
            }
            c => text.push(c),
        }
    }

    let active = if in_token {
        Token {
            text,
            width: raw.width(),
            complete: false,
            quoted,
        }
    } else {
        Token::default()
    };
    CommandLine { done, active }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(line: &str) -> (Vec<String>, String) {
        let parsed = parse(line, Shell::Zsh);
        (parsed.done.into_iter().map(|t| t.text).collect(), parsed.active.text)
    }

    #[test]
    fn splits_done_and_active_tokens() {
        assert_eq!(texts("git ch"), (vec!["git".into()], "ch".into()));
        assert_eq!(
            texts("git checkout "),
            (vec!["git".into(), "checkout".into()], "".into())
        );
        assert_eq!(texts(""), (vec![], "".into()));
        assert_eq!(texts("  git"), (vec![], "git".into()));
    }

    #[test]
    fn completes_only_the_last_command() {
        assert_eq!(texts("cat x | git ch"), (vec!["git".into()], "ch".into()));
        assert_eq!(texts("make && npm r"), (vec!["npm".into()], "r".into()));
        assert_eq!(texts("a; b "), (vec!["b".into()], "".into()));
        assert_eq!(
            texts("echo 'a | b' c"),
            (vec!["echo".into(), "a | b".into()], "c".into())
        );
        assert_eq!(texts("ls 2>&1 x"), (vec!["ls".into(), "2>&1".into()], "x".into()));
    }

    #[test]
    fn handles_quotes_and_escapes() {
        assert_eq!(
            texts(r#"git commit -m "hello wor"#),
            (vec!["git".into(), "commit".into(), "-m".into()], "hello wor".into())
        );
        assert_eq!(texts(r"cd my\ fol"), (vec!["cd".into()], "my fol".into()));
        let parsed = parse(r"cd my\ fol", Shell::Zsh);
        assert_eq!(parsed.active.width, 7);
        let parsed = parse(r#"cat "a b"#, Shell::Zsh);
        assert_eq!(parsed.active.width, 4);
        assert!(parsed.active.quoted);
    }

    #[test]
    fn powershell_keeps_backslashes() {
        let parsed = parse(r"cd C:\Users\me", Shell::Pwsh);
        assert_eq!(parsed.active.text, r"C:\Users\me");
    }

    #[test]
    fn measures_wide_characters() {
        let parsed = parse("echo 日本", Shell::Zsh);
        assert_eq!(parsed.active.width, 4);
    }
}
