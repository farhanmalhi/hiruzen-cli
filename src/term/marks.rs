//! Finds hiruzen's markers in the shell's output.
//!
//! The shell integration scripts emit `ESC ] 7391 ; <kind> BEL` around each
//! prompt. The scanner splits the output stream at those markers and removes
//! them, so the position of a marker is the cursor position once the bytes
//! before it have been applied to the screen.

const PREFIX: &[u8] = b"\x1b]7391;";
const MAX_PAYLOAD: usize = 8192;

#[derive(Debug, Clone, PartialEq)]
pub enum Mark {
    /// The shell is about to draw its prompt.
    PromptStart,
    /// The prompt is drawn; what follows on screen is the user's command.
    PromptEnd,
    /// The shell's working directory.
    Cwd(String),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Chunk {
    Data(Vec<u8>),
    Mark(Mark),
}

#[derive(Debug, Default)]
enum State {
    #[default]
    Normal,
    /// Matched this many bytes of `PREFIX`.
    Prefix(usize),
    Payload,
    /// Saw ESC inside the payload; a following `\` ends the sequence.
    PayloadEscape,
}

#[derive(Debug, Default)]
pub struct Scanner {
    state: State,
    payload: Vec<u8>,
}

impl Scanner {
    /// Splits `input` into ordinary output and markers. A marker cut off at
    /// the end of `input` is completed by the next call.
    pub fn feed(&mut self, input: &[u8]) -> Vec<Chunk> {
        let mut chunks = Vec::new();
        let mut data: Vec<u8> = Vec::with_capacity(input.len());
        let mut i = 0;
        while i < input.len() {
            let byte = input[i];
            match self.state {
                State::Normal => {
                    if byte == PREFIX[0] {
                        self.state = State::Prefix(1);
                    } else {
                        data.push(byte);
                    }
                    i += 1;
                }
                State::Prefix(matched) => {
                    if byte == PREFIX[matched] {
                        i += 1;
                        if matched + 1 == PREFIX.len() {
                            self.payload.clear();
                            self.state = State::Payload;
                        } else {
                            self.state = State::Prefix(matched + 1);
                        }
                    } else {
                        // Some other escape sequence: pass it through and look at this byte again.
                        data.extend_from_slice(&PREFIX[..matched]);
                        self.state = State::Normal;
                    }
                }
                State::Payload => {
                    i += 1;
                    match byte {
                        0x07 => {
                            self.finish(&mut data, &mut chunks);
                        }
                        0x1b => self.state = State::PayloadEscape,
                        _ if self.payload.len() >= MAX_PAYLOAD => {
                            // Not one of ours after all; never hold output back indefinitely.
                            data.extend_from_slice(PREFIX);
                            data.append(&mut self.payload);
                            data.push(byte);
                            self.state = State::Normal;
                        }
                        _ => self.payload.push(byte),
                    }
                }
                State::PayloadEscape => {
                    if byte == b'\\' {
                        i += 1;
                    }
                    self.finish(&mut data, &mut chunks);
                }
            }
        }
        if !data.is_empty() {
            chunks.push(Chunk::Data(data));
        }
        chunks
    }

    fn finish(&mut self, data: &mut Vec<u8>, chunks: &mut Vec<Chunk>) {
        self.state = State::Normal;
        let payload = std::mem::take(&mut self.payload);
        let text = String::from_utf8_lossy(&payload);
        let mark = match text.split_once(';') {
            None if text == "PS" => Some(Mark::PromptStart),
            None if text == "PE" => Some(Mark::PromptEnd),
            Some(("CWD", path)) => Some(Mark::Cwd(unescape(path))),
            _ => None,
        };
        if let Some(mark) = mark {
            if !data.is_empty() {
                chunks.push(Chunk::Data(std::mem::take(data)));
            }
            chunks.push(Chunk::Mark(mark));
        }
    }
}

/// Reverses the scripts' escaping: `\\` and `\xHH`.
fn unescape(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\' && i + 1 < bytes.len() {
            if bytes[i + 1] == b'\\' {
                out.push(b'\\');
                i += 2;
                continue;
            }
            if bytes[i + 1] == b'x' && i + 3 < bytes.len() {
                if let Some(byte) = std::str::from_utf8(&bytes[i + 2..i + 4])
                    .ok()
                    .and_then(|h| u8::from_str_radix(h, 16).ok())
                {
                    out.push(byte);
                    i += 4;
                    continue;
                }
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_output_at_markers() {
        let mut scanner = Scanner::default();
        let chunks = scanner.feed(b"\x1b]7391;PS\x07user@host $ \x1b]7391;PE\x07");
        assert_eq!(
            chunks,
            vec![
                Chunk::Mark(Mark::PromptStart),
                Chunk::Data(b"user@host $ ".to_vec()),
                Chunk::Mark(Mark::PromptEnd),
            ]
        );
    }

    #[test]
    fn completes_a_marker_split_across_reads() {
        let mut scanner = Scanner::default();
        assert_eq!(scanner.feed(b"abc\x1b]73"), vec![Chunk::Data(b"abc".to_vec())]);
        assert_eq!(scanner.feed(b"91;P"), vec![]);
        assert_eq!(
            scanner.feed(b"E\x07x"),
            vec![Chunk::Mark(Mark::PromptEnd), Chunk::Data(b"x".to_vec())]
        );
    }

    #[test]
    fn passes_other_escape_sequences_through() {
        let mut scanner = Scanner::default();
        let input = b"\x1b[31mred\x1b]0;title\x07\x1b]7;file://x\x1b\\";
        assert_eq!(scanner.feed(input), vec![Chunk::Data(input.to_vec())]);
        // ESC directly followed by the start of a marker.
        assert_eq!(
            scanner.feed(b"\x1b\x1b]7391;PE\x07"),
            vec![Chunk::Data(b"\x1b".to_vec()), Chunk::Mark(Mark::PromptEnd)]
        );
    }

    #[test]
    fn decodes_the_working_directory() {
        let mut scanner = Scanner::default();
        let chunks = scanner.feed(b"\x1b]7391;CWD;/tmp/a\\x3bb\\\\c\x1b\\");
        assert_eq!(chunks, vec![Chunk::Mark(Mark::Cwd("/tmp/a;b\\c".to_string()))]);
    }
}
