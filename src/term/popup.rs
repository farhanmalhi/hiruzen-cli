//! Lays out the suggestion dropdown as rows of styled text.
//!
//! This module only decides what the dropdown looks like and where it goes;
//! writing it to the terminal and restoring what it covered is the session's job.

use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::engine::{Candidate, Kind};
use crate::settings::{self, Theme};

const MIN_NAME_WIDTH: usize = 12;
const MAX_NAME_WIDTH: usize = 36;
const MAX_DESCRIPTION_WIDTH: usize = 48;
/// Narrowest terminal the dropdown is drawn in.
const MIN_COLS: usize = 20;
/// Shown while suggestions are being fetched from the network.
pub const LOADING: &str = "Searching online…";
/// Shown when they could not be, over what this machine knows by itself.
pub const OFFLINE: &str = "No internet: local suggestions only";
/// The same, when this machine knows nothing to list.
pub const OFFLINE_EMPTY: &str = "No internet connection";

const RESET: &str = "\x1b[0m";

/// How the dropdown looks, from the user's settings.
#[derive(Debug, Clone, Copy)]
pub struct Style {
    pub theme: &'static Theme,
    /// Most suggestions visible at once; the list scrolls beyond that.
    pub max_visible: usize,
    pub descriptions: bool,
    /// A symbol for the key that inserts the highlighted suggestion, shown
    /// right after its name. `None` shows nothing.
    pub accept_key: Option<&'static str>,
}

impl Default for Style {
    fn default() -> Self {
        Style {
            theme: settings::theme("dark"),
            max_visible: 8,
            descriptions: true,
            accept_key: None,
        }
    }
}

/// Where the dropdown sits on screen and what each of its rows contains.
#[derive(Debug, Clone, PartialEq)]
pub struct Layout {
    /// Zero-based screen row of the first dropdown row.
    pub top: usize,
    /// Zero-based screen column of the dropdown's left edge.
    pub left: usize,
    pub width: usize,
    /// One styled string per row, each exactly `width` cells wide.
    pub rows: Vec<String>,
}

pub struct Request<'a> {
    pub items: &'a [Candidate],
    pub selected: usize,
    /// Shown instead of a list when there are no items.
    pub hint: Option<&'a str>,
    /// A line about the list as a whole, shown under it: that more is being
    /// fetched, or could not be.
    pub note: Option<&'a str>,
    pub cols: usize,
    pub rows: usize,
    pub cursor_row: usize,
    /// Row the command starts on; above the cursor row when the command wraps.
    pub command_row: usize,
    /// Column where the word being completed starts.
    pub anchor_col: usize,
    /// First row the dropdown may cover. The rows above it hold text that
    /// could not be put back afterwards.
    pub min_top: usize,
    pub style: Style,
}

/// A one-character tag for the kind of thing a suggestion is. Plain ASCII so
/// it renders in every terminal font.
fn marker(kind: Kind) -> char {
    match kind {
        Kind::Subcommand => '>',
        Kind::Option => '-',
        Kind::Arg => '=',
        Kind::File => ' ',
        Kind::Folder => '/',
        Kind::Command => '$',
        Kind::History => '~',
    }
}

/// Cuts or pads `text` to exactly `width` cells, ending a cut with `…`.
pub fn fit(text: &str, width: usize) -> String {
    let mut out = String::new();
    let mut used = 0;
    let total = text.width();
    for ch in text.chars() {
        // Control characters would move the cursor; never emit them.
        let ch = if ch.is_control() { ' ' } else { ch };
        let w = ch.width().unwrap_or(0);
        let reserve = if total > width { 1 } else { 0 };
        if used + w + reserve > width {
            break;
        }
        out.push(ch);
        used += w;
    }
    if total > width && used < width {
        out.push('…');
        used += 1;
    }
    out.extend(std::iter::repeat_n(' ', width.saturating_sub(used)));
    out
}

/// Computes the dropdown for `request`, or `None` when it cannot be shown:
/// nothing to show, or no room.
pub fn layout(request: &Request) -> Option<Layout> {
    let Request {
        items,
        selected,
        hint,
        note,
        cols,
        rows,
        cursor_row,
        command_row,
        anchor_col,
        min_top,
        style,
    } = *request;
    let theme = style.theme;
    // A dropdown drawn above must clear every row of a wrapped command.
    let command_row = command_row.min(cursor_row);
    if cols < MIN_COLS || rows < 3 {
        return None;
    }
    let below = rows.saturating_sub(cursor_row + 1);
    let above = command_row.saturating_sub(min_top);
    let room = below.max(above);
    if room == 0 {
        return None;
    }

    let (width, lines): (usize, Vec<String>) = if items.is_empty() {
        let hint = note.or(hint)?;
        let width = (hint.width() + 2).min(cols).min(MAX_DESCRIPTION_WIDTH + MAX_NAME_WIDTH);
        (
            width,
            vec![format!("{} {}{RESET}", theme.description, fit(hint, width - 1))],
        )
    } else {
        let selected = selected.min(items.len() - 1);
        // The note takes a row of its own under the list, where there is one to spare.
        let note = note.filter(|_| room > 1);
        let visible = style
            .max_visible
            .max(1)
            .min(items.len())
            .min(room - usize::from(note.is_some()));
        // Scroll in whole pages so the list does not shift on every key press.
        let first = (selected / visible) * visible;
        let page = &items[first..(first + visible).min(items.len())];

        // Room for the key symbol is kept on every row, so the list does not
        // change width as the highlight moves.
        let key_width = style.accept_key.map_or(0, |key| key.width() + 1);
        let name_width = (page.iter().map(|c| c.label().width()).max().unwrap_or(0) + key_width)
            .clamp(MIN_NAME_WIDTH, MAX_NAME_WIDTH);
        let description_width = page
            .iter()
            .map(|c| {
                c.description
                    .as_deref()
                    .map(first_line)
                    .map(UnicodeWidthStr::width)
                    .unwrap_or(0)
            })
            .max()
            .unwrap_or(0)
            .min(if style.descriptions { MAX_DESCRIPTION_WIDTH } else { 0 });
        // marker, space, name, [two spaces, description], trailing space
        let fixed = 2 + 1;
        let mut name_width = name_width;
        let mut description_width = description_width;
        if fixed + name_width + 2 + description_width > cols {
            description_width = cols.saturating_sub(fixed + name_width + 2);
            if description_width < 8 {
                description_width = 0;
                name_width = name_width.min(cols - fixed);
            }
        }
        let width = fixed
            + name_width
            + if description_width > 0 {
                2 + description_width
            } else {
                0
            };

        let more_above = first > 0;
        let more_below = first + page.len() < items.len();
        let mut lines: Vec<String> = page
            .iter()
            .enumerate()
            .map(|(i, candidate)| {
                let is_selected = first + i == selected;
                let (marker_style, name_style, description_style) = if is_selected {
                    (theme.selected_marker, theme.selected, theme.selected_description)
                } else {
                    (theme.marker, theme.row, theme.description)
                };
                let name = match style.accept_key.filter(|_| is_selected && name_width > key_width) {
                    // The key that inserts this suggestion, right after its name.
                    Some(key) => {
                        let shown = candidate.label().width().min(name_width - key_width);
                        format!(
                            "{} {marker_style}{key}{name_style}{}",
                            fit(candidate.label(), shown),
                            " ".repeat(name_width - key_width - shown)
                        )
                    }
                    None => fit(candidate.label(), name_width),
                };
                let mut line = format!("{marker_style}{} {name_style}{name}", marker(candidate.kind));
                if description_width > 0 {
                    let description = candidate.description.as_deref().map(first_line).unwrap_or("");
                    line.push_str(&format!("{description_style}  {}", fit(description, description_width)));
                }
                // The last column doubles as a scroll indicator.
                let edge = if i == 0 && more_above {
                    '^'
                } else if i + 1 == page.len() && more_below {
                    'v'
                } else {
                    ' '
                };
                line.push_str(&format!("{description_style}{edge}{RESET}"));
                line
            })
            .collect();
        if let Some(note) = note {
            lines.push(format!("{} {}{RESET}", theme.description, fit(note, width - 1)));
        }
        (width, lines)
    };

    let height = lines.len();
    let top = if below >= height {
        cursor_row + 1
    } else if above >= height {
        command_row - height
    } else {
        return None;
    };
    let left = anchor_col.min(cols.saturating_sub(width));
    Some(Layout {
        top,
        left,
        width,
        rows: lines,
    })
}

fn first_line(text: &str) -> &str {
    text.lines().next().unwrap_or("").trim()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn items(names: &[&str]) -> Vec<Candidate> {
        names
            .iter()
            .map(|n| Candidate::new(*n, Kind::Subcommand).described("does a thing"))
            .collect()
    }

    /// Removes SGR sequences, leaving the visible text.
    fn plain(row: &str) -> String {
        let mut out = String::new();
        let mut chars = row.chars();
        while let Some(ch) = chars.next() {
            if ch == '\x1b' {
                for c in chars.by_ref() {
                    if c == 'm' {
                        break;
                    }
                }
            } else {
                out.push(ch);
            }
        }
        out
    }

    fn request<'a>(items: &'a [Candidate], cursor_row: usize, anchor_col: usize) -> Request<'a> {
        Request {
            items,
            selected: 0,
            hint: None,
            note: None,
            cols: 80,
            rows: 24,
            cursor_row,
            command_row: cursor_row,
            anchor_col,
            min_top: 0,
            style: Style::default(),
        }
    }

    #[test]
    fn fits_text_to_an_exact_width() {
        assert_eq!(fit("abc", 5), "abc  ");
        assert_eq!(fit("abcdef", 4), "abc…");
        assert_eq!(fit("日本語", 4).width(), 4);
        assert_eq!(fit("a\tb", 3), "a b");
    }

    #[test]
    fn draws_below_the_cursor_when_there_is_room() {
        let list = items(&["checkout", "cherry-pick"]);
        let layout = layout(&request(&list, 3, 4)).unwrap();
        assert_eq!((layout.top, layout.left), (4, 4));
        assert_eq!(layout.rows.len(), 2);
        for row in &layout.rows {
            assert_eq!(plain(row).width(), layout.width);
        }
        assert!(plain(&layout.rows[0]).starts_with("> checkout"));
        assert!(plain(&layout.rows[0]).contains("does a thing"));
    }

    #[test]
    fn draws_above_the_cursor_at_the_bottom_of_the_screen() {
        let list = items(&["a", "b", "c"]);
        let layout = layout(&request(&list, 23, 0)).unwrap();
        assert_eq!(layout.top, 20);
    }

    #[test]
    fn draws_above_the_first_row_of_a_wrapped_command() {
        let list = items(&["a", "b"]);
        let mut req = request(&list, 23, 0);
        req.command_row = 21;
        assert_eq!(layout(&req).unwrap().top, 19);
    }

    #[test]
    fn stays_off_rows_it_could_not_put_back() {
        let list = items(&["a", "b", "c", "d", "e"]);
        let mut req = request(&list, 23, 0);
        // Rows 0 to 19 hold output from before the session: only three fit.
        req.min_top = 20;
        let layout = layout(&req).unwrap();
        assert_eq!((layout.top, layout.rows.len()), (20, 3));
        req.min_top = 23;
        assert!(super::layout(&req).is_none());
    }

    #[test]
    fn shows_the_accept_key_after_the_highlighted_name_only() {
        let list = items(&["checkout", "cherry-pick"]);
        let mut req = request(&list, 3, 4);
        req.style.accept_key = Some("↵");
        let first = layout(&req).unwrap();
        assert!(
            plain(&first.rows[0]).starts_with("> checkout ↵"),
            "{}",
            plain(&first.rows[0])
        );
        assert!(!plain(&first.rows[1]).contains('↵'));
        // Every row keeps the same width, with or without the symbol.
        for row in &first.rows {
            assert_eq!(plain(row).width(), first.width);
        }
        req.selected = 1;
        let second = layout(&req).unwrap();
        assert!(plain(&second.rows[1]).starts_with("> cherry-pick ↵"));
        assert_eq!(second.width, first.width);
        // A name too long for the column still shows the symbol.
        let long = items(&["a-very-long-suggestion-name-that-does-not-fit-the-column"]);
        let mut req = request(&long, 0, 0);
        req.style.accept_key = Some("⇥");
        let row = plain(&layout(&req).unwrap().rows[0]);
        assert!(row.contains("… ⇥"), "{row}");
    }

    #[test]
    fn keeps_the_box_inside_the_right_edge() {
        let list = items(&["checkout"]);
        let layout = layout(&request(&list, 0, 78)).unwrap();
        assert_eq!(layout.left + layout.width, 80);
    }

    #[test]
    fn pages_through_long_lists() {
        let names: Vec<String> = (0..20).map(|i| format!("item{i}")).collect();
        let refs: Vec<&str> = names.iter().map(String::as_str).collect();
        let list = items(&refs);
        let mut req = request(&list, 0, 0);
        let first = layout(&req).unwrap();
        assert_eq!(first.rows.len(), 8);
        assert!(plain(first.rows.last().unwrap()).ends_with('v'));
        req.selected = 9;
        let second = layout(&req).unwrap();
        assert!(plain(&second.rows[0]).contains("item8"));
        assert!(plain(&second.rows[0]).ends_with('^'));
    }

    #[test]
    fn shows_a_hint_when_there_is_nothing_to_list() {
        let req = Request {
            items: &[],
            selected: 0,
            hint: Some("message: the commit message"),
            note: None,
            cols: 80,
            rows: 24,
            cursor_row: 0,
            command_row: 0,
            anchor_col: 2,
            min_top: 0,
            style: Style::default(),
        };
        let layout = layout(&req).unwrap();
        assert_eq!(layout.rows.len(), 1);
        assert!(plain(&layout.rows[0]).contains("the commit message"));
        let none = Request { hint: None, ..req };
        assert!(super::layout(&none).is_none());
    }

    #[test]
    fn says_when_more_suggestions_are_being_fetched() {
        let list = items(&["react", "redux"]);
        let mut req = request(&list, 3, 0);
        req.note = Some(LOADING);
        let with_items = layout(&req).unwrap();
        assert_eq!(with_items.rows.len(), 3);
        assert!(plain(&with_items.rows[2]).contains("Searching"));
        for row in &with_items.rows {
            assert_eq!(plain(row).width(), with_items.width);
        }
        // With nothing to list yet, the note is the whole dropdown.
        req.items = &[];
        let alone = layout(&req).unwrap();
        assert_eq!(alone.rows.len(), 1);
        assert_eq!(plain(&alone.rows[0]).trim(), LOADING);
        // One free row: the suggestion matters more than the note.
        let mut tight = request(&list, 22, 0);
        tight.note = Some(OFFLINE);
        tight.command_row = 1;
        let tight = layout(&tight).unwrap();
        assert_eq!(tight.rows.len(), 1);
        assert!(plain(&tight.rows[0]).contains("react"));
    }

    #[test]
    fn hides_when_the_terminal_is_too_small() {
        let list = items(&["a"]);
        let req = Request {
            items: &list,
            selected: 0,
            hint: None,
            note: None,
            cols: 10,
            rows: 24,
            cursor_row: 0,
            command_row: 0,
            anchor_col: 0,
            min_top: 0,
            style: Style::default(),
        };
        assert!(layout(&req).is_none());
    }
}
