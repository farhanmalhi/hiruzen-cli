//! Key names as written in settings (`tab`, `ctrl+n`, `shift+tab`) and the
//! bytes a terminal sends for them.

/// The byte sequences a terminal may send for the named key, or `None` for a
/// name hiruzen does not know or will not take over.
pub fn sequences(name: &str) -> Option<Vec<Vec<u8>>> {
    let name = name.trim().to_ascii_lowercase().replace(' ', "");
    let one = |bytes: &[u8]| Some(vec![bytes.to_vec()]);
    // Arrow keys arrive in one of two forms depending on the terminal's mode.
    let arrow = |letter: u8| Some(vec![vec![0x1b, b'[', letter], vec![0x1b, b'O', letter]]);
    match name.as_str() {
        "tab" => one(b"\t"),
        "shift+tab" => one(b"\x1b[Z"),
        "enter" | "return" => Some(vec![b"\r".to_vec(), b"\n".to_vec()]),
        "escape" | "esc" => one(b"\x1b"),
        "up" => arrow(b'A'),
        "down" => arrow(b'B'),
        "right" => arrow(b'C'),
        "left" => arrow(b'D'),
        "pageup" => one(b"\x1b[5~"),
        "pagedown" => one(b"\x1b[6~"),
        "ctrl+space" => one(b"\0"),
        _ => {
            let (modifier, key) = name.split_once('+')?;
            let mut chars = key.chars();
            let (Some(ch), None) = (chars.next(), chars.next()) else {
                return None;
            };
            match modifier {
                // Ctrl+C, Ctrl+D and Ctrl+Z interrupt, end and suspend; taking
                // them over would leave no way to do those things.
                "ctrl" if ch.is_ascii_lowercase() && !matches!(ch, 'c' | 'd' | 'z') => one(&[ch as u8 - b'a' + 1]),
                "alt" if ch.is_ascii_graphic() => one(&[0x1b, ch as u8]),
                _ => None,
            }
        }
    }
}

/// Parses a comma-separated list of key names into their normalised names.
pub fn parse_list(text: &str) -> Result<Vec<String>, String> {
    let names: Vec<String> = text
        .split(',')
        .map(|name| name.trim().to_ascii_lowercase().replace(' ', ""))
        .filter(|name| !name.is_empty())
        .collect();
    if names.is_empty() {
        return Err("give at least one key".to_string());
    }
    match names.iter().find(|name| sequences(name).is_none()) {
        Some(bad) => Err(format!(
            "'{bad}' is not a key hiruzen can bind; use tab, shift+tab, enter, escape, up, down, left, right, pageup, pagedown, ctrl+space, ctrl+<letter> (not c, d or z) or alt+<character>"
        )),
        None => Ok(names),
    }
}

/// Every byte sequence for a list of key names.
pub fn all_sequences(names: &[String]) -> Vec<Vec<u8>> {
    names.iter().filter_map(|name| sequences(name)).flatten().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_key_names_to_bytes() {
        assert_eq!(sequences("Tab"), Some(vec![b"\t".to_vec()]));
        assert_eq!(sequences("ctrl+n"), Some(vec![vec![14]]));
        assert_eq!(sequences("Ctrl + J"), Some(vec![vec![10]]));
        assert_eq!(sequences("alt+."), Some(vec![vec![0x1b, b'.']]));
        assert_eq!(sequences("down").unwrap().len(), 2);
        assert_eq!(sequences("ctrl+c"), None);
        assert_eq!(sequences("f13"), None);
        assert_eq!(sequences("ctrl+nn"), None);
    }

    #[test]
    fn parses_lists_and_reports_the_bad_name() {
        assert_eq!(parse_list("Tab, ctrl+y").unwrap(), vec!["tab", "ctrl+y"]);
        assert!(parse_list("tab,banana").unwrap_err().contains("'banana'"));
        assert!(parse_list(" , ").is_err());
    }
}
