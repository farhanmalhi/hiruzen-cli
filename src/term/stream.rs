//! Tracks whether the shell's output stream is between escape sequences.
//!
//! hiruzen writes its dropdown into the same stream the shell writes to. It
//! may only do so at a point where the shell is not part-way through an
//! escape sequence or a multi-byte character, or both would be corrupted.

#[derive(Debug, Default, Clone, Copy, PartialEq)]
enum State {
    #[default]
    Ground,
    Escape,
    EscapeIntermediate,
    Csi,
    Osc,
    /// DCS, APC, PM and SOS strings, which end with ST like OSC does.
    Str,
    /// ESC seen inside a string; the next byte ends it.
    StringEscape,
    /// Inside a UTF-8 character with this many bytes still to come.
    Utf8(u8),
}

#[derive(Debug, Default)]
pub struct Stream {
    state: State,
}

impl Stream {
    pub fn feed(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.state = match (self.state, byte) {
                (State::Utf8(left), 0x80..=0xbf) => {
                    if left > 1 {
                        State::Utf8(left - 1)
                    } else {
                        State::Ground
                    }
                }
                (State::Ground | State::Utf8(_), _) => match byte {
                    0x1b => State::Escape,
                    0xc0..=0xdf => State::Utf8(1),
                    0xe0..=0xef => State::Utf8(2),
                    0xf0..=0xf7 => State::Utf8(3),
                    _ => State::Ground,
                },
                (State::Escape, b'[') => State::Csi,
                (State::Escape, b']') => State::Osc,
                (State::Escape, b'P' | b'_' | b'^' | b'X') => State::Str,
                (State::Escape, 0x20..=0x2f) => State::EscapeIntermediate,
                (State::Escape, 0x1b) => State::Escape,
                (State::Escape, _) => State::Ground,
                (State::EscapeIntermediate, 0x30..=0x7e) => State::Ground,
                (State::EscapeIntermediate, _) => State::EscapeIntermediate,
                (State::Csi, 0x40..=0x7e) => State::Ground,
                (State::Csi, 0x1b) => State::Escape,
                (State::Csi, _) => State::Csi,
                (State::Osc, 0x07) => State::Ground,
                (State::Osc | State::Str, 0x1b) => State::StringEscape,
                (State::Osc, _) => State::Osc,
                (State::Str, _) => State::Str,
                (State::StringEscape, _) => State::Ground,
            };
        }
    }

    /// True when anything written now lands between the shell's sequences.
    pub fn is_idle(&self) -> bool {
        self.state == State::Ground
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn idle_after(chunks: &[&[u8]]) -> bool {
        let mut stream = Stream::default();
        for chunk in chunks {
            stream.feed(chunk);
        }
        stream.is_idle()
    }

    #[test]
    fn plain_text_and_complete_sequences_are_idle() {
        assert!(idle_after(&[b"hello"]));
        assert!(idle_after(&[b"\x1b[1;31mred\x1b[0m"]));
        assert!(idle_after(&[b"\x1b]0;title\x07", b"\x1b]7;x\x1b\\"]));
        assert!(idle_after(&[b"\x1b(B", b"\x1b7\x1b8"]));
        assert!(idle_after(&["日本".as_bytes()]));
    }

    #[test]
    fn partial_sequences_are_not_idle() {
        assert!(!idle_after(&[b"\x1b"]));
        assert!(!idle_after(&[b"\x1b[1;3"]));
        assert!(!idle_after(&[b"\x1b]0;tit"]));
        assert!(!idle_after(&[b"\x1bPq"]));
        assert!(!idle_after(&[&"日".as_bytes()[..2]]));
    }

    #[test]
    fn sequences_complete_across_chunks() {
        assert!(idle_after(&[b"\x1b[1;3", b"1m"]));
        assert!(idle_after(&[&"日".as_bytes()[..1], &"日".as_bytes()[1..]]));
    }
}
