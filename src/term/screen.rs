//! A model of what the terminal is showing, kept in step by feeding it the
//! same bytes the real terminal receives.
//!
//! hiruzen needs it for two things: reading the command the user is typing
//! off the screen, and putting back what the dropdown covered.

use std::fmt::Write;
use std::num::NonZeroU16;

/// Lines remembered above the screen. Only used to count how far the screen
/// has scrolled since the prompt was drawn, so it need not be large.
const SCROLLBACK: usize = 4000;

/// A screen position that stays attached to its line as the screen scrolls.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Anchor {
    row: u16,
    pub col: u16,
    /// Scrollback length when the anchor was taken.
    scrolled: usize,
}

pub struct Screen {
    parser: vt100::Parser,
}

fn non_zero(value: u16) -> NonZeroU16 {
    NonZeroU16::new(value.max(1)).unwrap_or(NonZeroU16::MIN)
}

impl Screen {
    pub fn new(cols: u16, rows: u16) -> Self {
        Screen {
            parser: vt100::Parser::new(non_zero(rows), non_zero(cols), SCROLLBACK),
        }
    }

    pub fn feed(&mut self, bytes: &[u8]) {
        self.parser.process(bytes);
    }

    pub fn resize(&mut self, cols: u16, rows: u16) {
        self.parser.set_size(non_zero(rows), non_zero(cols));
    }

    /// `(cols, rows)`.
    pub fn size(&self) -> (u16, u16) {
        let (rows, cols) = self.parser.screen().size();
        (cols.get(), rows.get())
    }

    /// `(row, col)`, zero-based.
    pub fn cursor(&self) -> (u16, u16) {
        self.parser.screen().cursor_position()
    }

    pub fn alternate(&self) -> bool {
        self.parser.screen().alternate_screen()
    }

    pub fn cursor_hidden(&self) -> bool {
        self.parser.screen().hide_cursor()
    }

    /// Marks the cursor position so it can be found again after scrolling.
    pub fn anchor(&mut self) -> Anchor {
        // Start counting scrolled lines from zero so the count cannot reach
        // the scrollback limit while one command is being typed.
        self.parser.screen_mut().clear_scrollback();
        let (row, col) = self.cursor();
        Anchor {
            row,
            col,
            scrolled: self.parser.screen().scrollback_len(),
        }
    }

    /// An anchor at a known position, for when the shell redraws the prompt
    /// somewhere else without saying so (after clearing the screen).
    pub fn anchor_at(&self, row: u16, col: u16) -> Anchor {
        Anchor {
            row,
            col,
            scrolled: self.parser.screen().scrollback_len(),
        }
    }

    /// True when the cursor sits on a character the user typed, which means
    /// it is inside a word, not at the end of one. Greyed-out text after the
    /// cursor is a shell's own inline suggestion and does not count.
    pub fn cursor_on_text(&self) -> bool {
        let (row, col) = self.cursor();
        let Some(cell) = self.parser.screen().cell(row, col) else {
            return false;
        };
        if !cell.has_contents() || cell.contents().trim().is_empty() {
            return false;
        }
        let greyed = cell.dim() || matches!(cell.fgcolor(), vt100::Color::Idx(8) | vt100::Color::Idx(232..=250));
        !greyed
    }

    /// The row an anchor is on now, or `None` once it has scrolled off the
    /// top or the scrollback it was counted against has been cleared.
    pub fn anchor_row(&self, anchor: &Anchor) -> Option<u16> {
        let scrolled = self.parser.screen().scrollback_len();
        let row = if scrolled >= anchor.scrolled {
            usize::from(anchor.row).checked_sub(scrolled - anchor.scrolled)?
        } else {
            // The screen grew taller and pulled lines back out of the scrollback.
            usize::from(anchor.row) + (anchor.scrolled - scrolled)
        };
        let (_, rows) = self.size();
        (row < usize::from(rows)).then_some(row as u16)
    }

    /// The text from `from` up to, not including, `to`; both `(row, col)`.
    /// Rows that wrap are joined; other row breaks become spaces.
    pub fn text_between(&self, from: (u16, u16), to: (u16, u16)) -> String {
        let screen = self.parser.screen();
        let (cols, _) = self.size();
        let mut out = String::new();
        for row in from.0..=to.0 {
            let start = if row == from.0 { from.1 } else { 0 };
            let end = if row == to.0 { to.1 } else { cols };
            let mut line = String::new();
            for col in start..end {
                match screen.cell(row, col) {
                    Some(cell) if cell.is_wide_continuation() => {}
                    Some(cell) if cell.has_contents() => line.push_str(cell.contents()),
                    _ => line.push(' '),
                }
            }
            if row != to.0 && !screen.row_wrapped(row) {
                // A continuation line of a multi-line command.
                let trimmed = line.trim_end().len();
                line.truncate(trimmed);
                line.push(' ');
            }
            out.push_str(&line);
        }
        out
    }

    /// Escape sequences that redraw one row exactly as the model has it,
    /// leaving attributes reset. The caller restores the cursor.
    pub fn repaint_row(&self, row: u16) -> String {
        let screen = self.parser.screen();
        let (cols, _) = self.size();
        let mut out = String::new();
        let _ = write!(out, "\x1b[{};1H\x1b[0m\x1b[2K", row + 1);
        let mut style = String::new();
        let mut col = 0;
        // Blank cells are skipped over, not printed, so nothing is written
        // past the last real character and the row cannot wrap.
        let mut at = 0;
        while col < cols {
            let Some(cell) = screen.cell(row, col) else { break };
            if cell.is_wide_continuation() {
                col += 1;
                continue;
            }
            let cell_style = sgr(cell);
            if !cell.has_contents() && cell_style.is_empty() {
                col += 1;
                continue;
            }
            if at != col {
                let _ = write!(out, "\x1b[{}G", col + 1);
            }
            if cell_style != style {
                let _ = write!(out, "\x1b[0{cell_style}m");
                style = cell_style;
            }
            out.push_str(if cell.has_contents() { cell.contents() } else { " " });
            col += if cell.is_wide() { 2 } else { 1 };
            at = col;
        }
        out.push_str("\x1b[0m");
        out
    }

    /// Escape sequences that redraw the whole screen from the model,
    /// including the cursor position.
    pub fn repaint_all(&self) -> Vec<u8> {
        let mut out = b"\x1b[0m\x1b[H\x1b[2J".to_vec();
        out.extend_from_slice(self.parser.screen().contents_formatted().as_bytes());
        out
    }
}

/// The SGR parameters for a cell, each prefixed with `;`. Empty for a cell
/// with default attributes.
fn sgr(cell: &vt100::Cell) -> String {
    let mut out = String::new();
    if cell.bold() {
        out.push_str(";1");
    }
    if cell.dim() {
        out.push_str(";2");
    }
    if cell.italic() {
        out.push_str(";3");
    }
    if cell.underline() {
        out.push_str(";4");
    }
    if cell.inverse() {
        out.push_str(";7");
    }
    match cell.fgcolor() {
        vt100::Color::Default => {}
        vt100::Color::Idx(i) => {
            let _ = write!(out, ";38;5;{i}");
        }
        vt100::Color::Rgb(r, g, b) => {
            let _ = write!(out, ";38;2;{r};{g};{b}");
        }
    }
    match cell.bgcolor() {
        vt100::Color::Default => {}
        vt100::Color::Idx(i) => {
            let _ = write!(out, ";48;5;{i}");
        }
        vt100::Color::Rgb(r, g, b) => {
            let _ = write!(out, ";48;2;{r};{g};{b}");
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_command_after_the_prompt() {
        let mut screen = Screen::new(40, 10);
        screen.feed(b"user@host $ ");
        let anchor = screen.anchor();
        screen.feed(b"git checkout ");
        let row = screen.anchor_row(&anchor).unwrap();
        assert_eq!(screen.text_between((row, anchor.col), screen.cursor()), "git checkout ");
    }

    #[test]
    fn follows_the_prompt_when_the_screen_scrolls() {
        let mut screen = Screen::new(20, 4);
        screen.feed(b"\r\n\r\n\r\n$ ");
        let anchor = screen.anchor();
        assert_eq!(screen.anchor_row(&anchor), Some(3));
        // A command long enough to wrap twice on the last row scrolls the screen.
        screen.feed(b"echo aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa b");
        let row = screen.anchor_row(&anchor).unwrap();
        assert_eq!(row, 1);
        assert_eq!(
            screen.text_between((row, anchor.col), screen.cursor()),
            "echo aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa b"
        );
    }

    #[test]
    fn loses_an_anchor_that_scrolls_off_screen() {
        let mut screen = Screen::new(20, 3);
        screen.feed(b"$ ");
        let anchor = screen.anchor();
        screen.feed(b"\r\n\r\n\r\n\r\n");
        assert_eq!(screen.anchor_row(&anchor), None);
    }

    #[test]
    fn repaints_a_row_with_its_colours() {
        let mut screen = Screen::new(20, 3);
        screen.feed(b"ab \x1b[1;31mred\x1b[0m");
        let painted = screen.repaint_row(0);
        assert!(painted.starts_with("\x1b[1;1H\x1b[0m\x1b[2K"), "{painted:?}");
        assert!(painted.contains("ab "));
        assert!(painted.contains("\x1b[0;1;38;5;1mred"), "{painted:?}");
        // Feeding the repaint to a fresh screen reproduces the row.
        let mut copy = Screen::new(20, 3);
        copy.feed(painted.as_bytes());
        assert_eq!(copy.text_between((0, 0), (0, 6)), "ab red");
    }

    #[test]
    fn tells_typed_text_under_the_cursor_from_a_greyed_suggestion() {
        let mut screen = Screen::new(40, 3);
        screen.feed(b"$ git checkout\x1b[5D");
        assert!(screen.cursor_on_text());
        let mut ghost = Screen::new(40, 3);
        ghost.feed(b"$ git ch\x1b[38;5;8meckout\x1b[0m\x1b[6D");
        assert!(!ghost.cursor_on_text());
        let mut end = Screen::new(40, 3);
        end.feed(b"$ git ch");
        assert!(!end.cursor_on_text());
    }

    #[test]
    fn wide_characters_occupy_two_cells() {
        let mut screen = Screen::new(20, 3);
        screen.feed("$ ".as_bytes());
        let anchor = screen.anchor();
        screen.feed("echo 日本 x".as_bytes());
        assert_eq!(screen.text_between((0, anchor.col), screen.cursor()), "echo 日本 x");
    }
}
