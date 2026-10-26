//! Copy mode: scroll back through history, select text and yank it.

use crate::action::CopyCmd;
use crate::term::cell::Cell;
use crate::term::screen::Screen;

/// The last search, repeated by `n` and `N`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Search {
    pub query: String,
    /// The direction it was typed in.
    pub forward: bool,
}

/// Matches of `query` in a line as (first column, last column), left to right. A query
/// without capital letters ignores case.
pub fn find_in_line(cells: &[Cell], query: &str) -> Vec<(usize, usize)> {
    let ignore_case = !query.chars().any(char::is_uppercase);
    let fold = |c: char| if ignore_case { c.to_lowercase().next().unwrap_or(c) } else { c };
    let q: Vec<char> = query.chars().map(fold).collect();
    if q.is_empty() {
        return Vec::new();
    }
    let chars: Vec<(usize, char)> =
        cells.iter().enumerate().filter(|(_, c)| c.width != 0).map(|(x, c)| (x, fold(c.ch))).collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i + q.len() <= chars.len() {
        if chars[i..i + q.len()].iter().map(|(_, c)| *c).eq(q.iter().copied()) {
            out.push((chars[i].0, chars[i + q.len() - 1].0));
            i += q.len();
        } else {
            i += 1;
        }
    }
    out
}

/// 0 for blanks, 1 for word characters, 2 for punctuation.
fn class(s: &Screen, line: usize, col: usize) -> u8 {
    match s.abs_line(line).and_then(|l| l.get(col)).map_or(' ', |c| c.ch) {
        ' ' | '\0' => 0,
        c if c.is_alphanumeric() || c == '_' => 1,
        _ => 2,
    }
}

fn next_pos(s: &Screen, (line, col): (usize, usize)) -> Option<(usize, usize)> {
    if col + 1 < s.cols() {
        Some((line, col + 1))
    } else if line + 1 < s.history_len() + s.rows() {
        Some((line + 1, 0))
    } else {
        None
    }
}

fn prev_pos(s: &Screen, (line, col): (usize, usize)) -> Option<(usize, usize)> {
    if col > 0 {
        Some((line, col - 1))
    } else if line > 0 {
        Some((line - 1, s.cols() - 1))
    } else {
        None
    }
}

/// The columns of the word (or run of punctuation, or blanks) at (`line`, `col`): first and last.
pub fn word_at(s: &Screen, line: usize, col: usize) -> (usize, usize) {
    let c = class(s, line, col);
    let (mut first, mut last) = (col, col);
    while first > 0 && class(s, line, first - 1) == c {
        first -= 1;
    }
    while last + 1 < s.cols() && class(s, line, last + 1) == c {
        last += 1;
    }
    (first, last)
}

/// `w`: the start of the next word.
fn word_next(s: &Screen, from: (usize, usize)) -> (usize, usize) {
    let mut p = from;
    let first = class(s, p.0, p.1);
    if first != 0 {
        while class(s, p.0, p.1) == first {
            match next_pos(s, p) {
                Some(n) => p = n,
                None => return p,
            }
        }
    }
    while class(s, p.0, p.1) == 0 {
        match next_pos(s, p) {
            Some(n) => p = n,
            None => break,
        }
    }
    p
}

/// `e`: the end of this word, or of the next one when already at the end.
fn word_end(s: &Screen, from: (usize, usize)) -> (usize, usize) {
    let Some(mut p) = next_pos(s, from) else { return from };
    while class(s, p.0, p.1) == 0 {
        match next_pos(s, p) {
            Some(n) => p = n,
            None => return p,
        }
    }
    let c = class(s, p.0, p.1);
    while let Some(n) = next_pos(s, p).filter(|n| class(s, n.0, n.1) == c) {
        p = n;
    }
    p
}

/// `b`: the start of this word, or of the previous one when already at the start.
fn word_prev(s: &Screen, from: (usize, usize)) -> (usize, usize) {
    let Some(mut p) = prev_pos(s, from) else { return from };
    while class(s, p.0, p.1) == 0 {
        match prev_pos(s, p) {
            Some(n) => p = n,
            None => return p,
        }
    }
    let c = class(s, p.0, p.1);
    while let Some(n) = prev_pos(s, p).filter(|n| class(s, n.0, n.1) == c) {
        p = n;
    }
    p
}

#[derive(Clone, Debug, Default)]
pub struct CopyMode {
    /// Lines scrolled back from the live screen.
    pub offset: usize,
    /// Cursor in viewport coordinates.
    pub cx: usize,
    pub cy: usize,
    /// Selection start as (absolute line, column).
    pub anchor: Option<(usize, usize)>,
    /// The selection covers whole lines.
    pub line_mode: bool,
    /// The selection is a rectangle between the anchor and the cursor.
    pub rect: bool,
    pub search: Option<Search>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    Stay,
    Exit,
    Yanked(String),
    /// Ask for a search term (`true`: searching forward).
    Search(bool),
    /// Something to tell the user.
    Notice(String),
}

impl CopyMode {
    pub fn new(screen: &Screen) -> CopyMode {
        let (cx, cy) = screen.cursor_pos();
        CopyMode { offset: 0, cx, cy, anchor: None, line_mode: false, rect: false, search: None }
    }

    /// Absolute index of the first visible line.
    pub fn top(&self, s: &Screen) -> usize {
        s.history_len().saturating_sub(self.offset)
    }

    pub fn cursor_abs(&self, s: &Screen) -> (usize, usize) {
        (self.top(s) + self.cy, self.cx)
    }

    /// Keep the view valid after the screen changed (output, resize).
    pub fn clamp(&mut self, s: &Screen) {
        self.offset = self.offset.min(s.history_len());
        self.cy = self.cy.min(s.rows() - 1);
        self.cx = self.cx.min(s.cols() - 1);
    }

    /// New history lines arrived: keep showing the same content. Absolute
    /// line numbers (and so the selection anchor) stay valid.
    pub fn history_grew(&mut self, by: usize) {
        if self.offset > 0 {
            self.offset += by;
        }
    }

    /// Scroll the view by `lines` (positive = back in time). Returns true when
    /// it ends up at the live screen.
    pub fn scroll(&mut self, lines: isize, s: &Screen) -> bool {
        let max = s.history_len();
        self.offset = if lines >= 0 {
            (self.offset + lines as usize).min(max)
        } else {
            self.offset.saturating_sub(lines.unsigned_abs())
        };
        self.offset == 0
    }

    /// Put the cursor on (`line`, `col`), scrolling just enough to show it (or centring it).
    pub fn reveal(&mut self, s: &Screen, line: usize, col: usize, center: bool) {
        let rows = s.rows();
        let top = self.top(s);
        let new_top = if center {
            line.saturating_sub(rows / 2)
        } else if line < top {
            line
        } else if line >= top + rows {
            line + 1 - rows
        } else {
            top
        };
        let new_top = new_top.min(s.history_len());
        self.offset = s.history_len() - new_top;
        self.cy = line.saturating_sub(new_top).min(rows - 1);
        self.cx = col.min(s.cols() - 1);
    }

    /// Search for `query` from the cursor (a match under it counts), and remember it for `n` / `N`.
    pub fn search(&mut self, s: &Screen, query: String, forward: bool) -> bool {
        self.search = Some(Search { query, forward });
        self.find(s, forward, true)
    }

    /// Move to the next match in the given direction, wrapping around the history.
    fn find(&mut self, s: &Screen, forward: bool, inclusive: bool) -> bool {
        let Some(query) = self.search.as_ref().map(|x| x.query.clone()) else { return false };
        let total = s.history_len() + s.rows();
        let (line0, col0) = self.cursor_abs(s);
        for step in 0..=total {
            let abs = if forward { (line0 + step) % total } else { (line0 + total - step % total) % total };
            let found = s.abs_line(abs).map(|l| find_in_line(l, &query)).unwrap_or_default();
            let hit = if step == 0 {
                match (forward, inclusive) {
                    (true, true) => found.iter().find(|m| m.0 >= col0),
                    (true, false) => found.iter().find(|m| m.0 > col0),
                    (false, true) => found.iter().rev().find(|m| m.0 <= col0),
                    (false, false) => found.iter().rev().find(|m| m.0 < col0),
                }
            } else if step == total {
                // All the way round: the part of the first line that was skipped.
                if forward { found.iter().find(|m| m.0 <= col0) } else { found.iter().rev().find(|m| m.0 >= col0) }
            } else if forward {
                found.first()
            } else {
                found.last()
            };
            if let Some(&(col, _)) = hit {
                self.reveal(s, abs, col, true);
                return true;
            }
        }
        false
    }

    pub fn apply(&mut self, cmd: CopyCmd, s: &Screen) -> Outcome {
        self.clamp(s);
        let rows = s.rows();
        match cmd {
            CopyCmd::Up => {
                if self.cy > 0 {
                    self.cy -= 1;
                } else if self.offset < s.history_len() {
                    self.offset += 1;
                }
            }
            CopyCmd::Down => {
                if self.cy + 1 < rows {
                    self.cy += 1;
                } else if self.offset > 0 {
                    self.offset -= 1;
                }
            }
            CopyCmd::Left => self.cx = self.cx.saturating_sub(1),
            CopyCmd::Right => self.cx = (self.cx + 1).min(s.cols() - 1),
            CopyCmd::PageUp => self.page(rows, true, s),
            CopyCmd::PageDown => self.page(rows, false, s),
            CopyCmd::HalfPageUp => self.page((rows / 2).max(1), true, s),
            CopyCmd::HalfPageDown => self.page((rows / 2).max(1), false, s),
            CopyCmd::Top => {
                self.offset = s.history_len();
                self.cy = 0;
            }
            CopyCmd::Bottom => {
                self.offset = 0;
                self.cy = rows - 1;
            }
            CopyCmd::ViewTop => self.cy = 0,
            CopyCmd::ViewMiddle => self.cy = rows / 2,
            CopyCmd::ViewBottom => self.cy = rows - 1,
            CopyCmd::WordNext => {
                let p = word_next(s, self.cursor_abs(s));
                self.reveal(s, p.0, p.1, false);
            }
            CopyCmd::WordEnd => {
                let p = word_end(s, self.cursor_abs(s));
                self.reveal(s, p.0, p.1, false);
            }
            CopyCmd::WordPrev => {
                let p = word_prev(s, self.cursor_abs(s));
                self.reveal(s, p.0, p.1, false);
            }
            CopyCmd::SearchForward => return Outcome::Search(true),
            CopyCmd::SearchBackward => return Outcome::Search(false),
            CopyCmd::SearchNext | CopyCmd::SearchPrev => {
                let Some(found) = self.search.clone() else { return Outcome::Notice("no previous search".into()) };
                let forward = found.forward == (cmd == CopyCmd::SearchNext);
                if !self.find(s, forward, false) {
                    return Outcome::Notice(format!("no match for {}", found.query));
                }
            }
            CopyCmd::LineStart => self.cx = 0,
            CopyCmd::LineEnd => {
                let (abs, _) = self.cursor_abs(s);
                let last = s
                    .abs_line(abs)
                    .and_then(|l| l.iter().rposition(|c| c.width != 0 && c.ch != ' '))
                    .unwrap_or(0);
                self.cx = last;
            }
            CopyCmd::SelectRect => {
                self.line_mode = false;
                match (self.anchor, self.rect) {
                    (None, _) => {
                        self.anchor = Some(self.cursor_abs(s));
                        self.rect = true;
                    }
                    (Some(_), true) => {
                        self.anchor = None;
                        self.rect = false;
                    }
                    (Some(_), false) => self.rect = true,
                }
            }
            CopyCmd::Select => {
                self.line_mode = false;
                self.rect = false;
                self.anchor = if self.anchor.is_some() { None } else { Some(self.cursor_abs(s)) };
            }
            CopyCmd::SelectLine => {
                self.rect = false;
                match (self.anchor, self.line_mode) {
                    (None, _) => {
                        self.anchor = Some(self.cursor_abs(s));
                        self.line_mode = true;
                    }
                    (Some(_), true) => {
                        self.anchor = None;
                        self.line_mode = false;
                    }
                    (Some(_), false) => self.line_mode = true,
                }
            }
            CopyCmd::Yank => {
                // Copying keeps copy mode and the cursor where they are; only the selection ends.
                let text = self.selected_text(s);
                self.anchor = None;
                self.line_mode = false;
                self.rect = false;
                return Outcome::Yanked(text);
            }
            CopyCmd::Cancel => return Outcome::Exit,
        }
        Outcome::Stay
    }

    fn page(&mut self, n: usize, back: bool, s: &Screen) {
        let before = self.offset;
        if back {
            self.offset = (self.offset + n).min(s.history_len());
            if self.offset == before {
                self.cy = 0;
            }
        } else {
            self.offset = self.offset.saturating_sub(n);
            if self.offset == before {
                self.cy = s.rows() - 1;
            }
        }
    }

    /// Ordered selection bounds (start, end), both inclusive.
    pub fn selection(&self, s: &Screen) -> Option<((usize, usize), (usize, usize))> {
        let a = self.anchor?;
        let b = self.cursor_abs(s);
        if self.rect {
            // Top-left and bottom-right corners.
            return Some(((a.0.min(b.0), a.1.min(b.1)), (a.0.max(b.0), a.1.max(b.1))));
        }
        if self.line_mode {
            return Some(((a.0.min(b.0), 0), (a.0.max(b.0), usize::MAX)));
        }
        Some(if a <= b { (a, b) } else { (b, a) })
    }

    pub fn is_selected(&self, sel: Option<((usize, usize), (usize, usize))>, line: usize, x: usize) -> bool {
        match sel {
            Some((start, end)) if self.rect => line >= start.0 && line <= end.0 && x >= start.1 && x <= end.1,
            Some((start, end)) => (line, x) >= start && (line, x) <= end,
            None => false,
        }
    }

    /// The selected text, or the cursor's line when nothing is selected.
    pub fn selected_text(&self, s: &Screen) -> String {
        let (start, end) = self.selection(s).unwrap_or_else(|| {
            let (abs, _) = self.cursor_abs(s);
            ((abs, 0), (abs, usize::MAX))
        });
        let mut lines = Vec::new();
        for abs in start.0..=end.0 {
            let Some(cells) = s.abs_line(abs) else { break };
            let (from, to) = if self.rect {
                (start.1, end.1.saturating_add(1))
            } else {
                (if abs == start.0 { start.1 } else { 0 }, if abs == end.0 { end.1.saturating_add(1) } else { usize::MAX })
            };
            let text: String = cells
                .iter()
                .enumerate()
                .filter(|(x, c)| *x >= from && *x < to && c.width != 0)
                .map(|(_, c)| c.ch)
                .collect();
            lines.push(text.trim_end().to_string());
        }
        lines.join("\n")
    }
}

pub fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let n = (chunk[0] as u32) << 16 | (*chunk.get(1).unwrap_or(&0) as u32) << 8 | *chunk.get(2).unwrap_or(&0) as u32;
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 10x3 screen holding lines "line1".."line8": 5 in history, 3 visible.
    fn screen() -> Screen {
        let mut s = Screen::new(10, 3, 100);
        s.feed(b"line1\r\nline2\r\nline3\r\nline4\r\nline5\r\nline6\r\nline7\r\nline8");
        s
    }

    fn viewport(m: &CopyMode, s: &Screen) -> Vec<String> {
        (0..3)
            .map(|y| {
                let l = s.abs_line(m.top(s) + y).unwrap();
                l.iter().map(|c| c.ch).collect::<String>().trim_end().to_string()
            })
            .collect()
    }

    #[test]
    fn starts_at_the_live_screen() {
        let s = screen();
        let m = CopyMode::new(&s);
        assert_eq!(viewport(&m, &s), vec!["line6", "line7", "line8"]);
        assert_eq!((m.offset, m.cy), (0, 2));
    }

    #[test]
    fn cursor_up_scrolls_at_the_edge() {
        let s = screen();
        let mut m = CopyMode::new(&s);
        for _ in 0..3 {
            m.apply(CopyCmd::Up, &s);
        }
        assert_eq!((m.cy, m.offset), (0, 1));
        assert_eq!(viewport(&m, &s), vec!["line5", "line6", "line7"]);
        for _ in 0..20 {
            m.apply(CopyCmd::Up, &s);
        }
        assert_eq!(m.offset, 5);
        assert_eq!(viewport(&m, &s)[0], "line1");
        m.apply(CopyCmd::Down, &s);
        assert_eq!(m.cy, 1);
    }

    #[test]
    fn paging_and_jumping() {
        let s = screen();
        let mut m = CopyMode::new(&s);
        m.apply(CopyCmd::PageUp, &s);
        assert_eq!(viewport(&m, &s), vec!["line3", "line4", "line5"]);
        m.apply(CopyCmd::HalfPageDown, &s);
        assert_eq!(m.offset, 2);
        m.apply(CopyCmd::Top, &s);
        assert_eq!((m.offset, m.cy), (5, 0));
        m.apply(CopyCmd::PageUp, &s);
        assert_eq!(m.cy, 0);
        m.apply(CopyCmd::Bottom, &s);
        assert_eq!((m.offset, m.cy), (0, 2));
    }

    #[test]
    fn yanking_ends_the_selection_but_keeps_the_cursor_and_the_view() {
        let s = screen();
        let mut m = CopyMode::new(&s);
        m.apply(CopyCmd::Top, &s);
        m.cx = 2;
        m.apply(CopyCmd::Select, &s);
        m.apply(CopyCmd::Down, &s);
        let (offset, cx, cy) = (m.offset, m.cx, m.cy);
        assert!(matches!(m.apply(CopyCmd::Yank, &s), Outcome::Yanked(_)));
        assert_eq!((m.offset, m.cx, m.cy), (offset, cx, cy));
        assert!(m.anchor.is_none() && m.selection(&s).is_none());
        // and the next yank (nothing selected) copies the cursor's line
        assert_eq!(m.apply(CopyCmd::Yank, &s), Outcome::Yanked("line2".into()));
    }

    #[test]
    fn yanks_current_line_without_selection() {
        let s = screen();
        let mut m = CopyMode::new(&s);
        m.apply(CopyCmd::Up, &s);
        assert_eq!(m.apply(CopyCmd::Yank, &s), Outcome::Yanked("line7".into()));
    }

    #[test]
    fn yanks_multi_line_selection() {
        let s = screen();
        let mut m = CopyMode::new(&s);
        m.apply(CopyCmd::Top, &s);
        m.apply(CopyCmd::LineStart, &s);
        m.apply(CopyCmd::Right, &s);
        m.apply(CopyCmd::Select, &s);
        m.apply(CopyCmd::Down, &s);
        m.apply(CopyCmd::Down, &s);
        m.apply(CopyCmd::Right, &s);
        // from (line1, col 1) to (line3, col 2)
        assert_eq!(m.apply(CopyCmd::Yank, &s), Outcome::Yanked("ine1\nline2\nlin".into()));
    }

    #[test]
    fn selection_works_backwards_and_toggles() {
        let s = screen();
        let mut m = CopyMode::new(&s);
        m.apply(CopyCmd::LineEnd, &s);
        assert_eq!(m.cx, 4);
        m.apply(CopyCmd::Select, &s);
        m.apply(CopyCmd::LineStart, &s);
        assert_eq!(m.selected_text(&s), "line8");
        m.apply(CopyCmd::Select, &s);
        assert!(m.anchor.is_none());
    }

    #[test]
    fn line_mode_selects_whole_lines() {
        let s = screen();
        let mut m = CopyMode::new(&s);
        m.apply(CopyCmd::Top, &s);
        m.apply(CopyCmd::Right, &s);
        m.apply(CopyCmd::SelectLine, &s);
        m.apply(CopyCmd::Down, &s);
        assert_eq!(m.apply(CopyCmd::Yank, &s), Outcome::Yanked("line1\nline2".into()));
        // selecting upwards covers the same lines
        let mut m = CopyMode::new(&s);
        m.apply(CopyCmd::SelectLine, &s);
        m.apply(CopyCmd::Up, &s);
        assert_eq!(m.selected_text(&s), "line7\nline8");
    }

    #[test]
    fn select_line_toggles_and_switches_mode() {
        let s = screen();
        let mut m = CopyMode::new(&s);
        m.apply(CopyCmd::Select, &s);
        m.apply(CopyCmd::SelectLine, &s);
        assert!(m.anchor.is_some() && m.line_mode);
        m.apply(CopyCmd::SelectLine, &s);
        assert!(m.anchor.is_none() && !m.line_mode);
        m.apply(CopyCmd::SelectLine, &s);
        m.apply(CopyCmd::Select, &s);
        assert!(m.anchor.is_none() && !m.line_mode);
    }

    #[test]
    fn cancel_exits() {
        let s = screen();
        let mut m = CopyMode::new(&s);
        assert_eq!(m.apply(CopyCmd::Cancel, &s), Outcome::Exit);
    }

    #[test]
    fn wheel_scrolling_reports_when_live() {
        let s = screen();
        let mut m = CopyMode::new(&s);
        assert!(!m.scroll(3, &s));
        assert_eq!(m.offset, 3);
        assert!(!m.scroll(-2, &s));
        assert!(m.scroll(-5, &s));
        assert!(!m.scroll(100, &s));
        assert_eq!(m.offset, 5);
    }

    #[test]
    fn view_stays_put_when_history_grows() {
        let mut s = screen();
        let mut m = CopyMode::new(&s);
        m.scroll(2, &s);
        let before = viewport(&m, &s);
        let h = s.history_len();
        s.feed(b"\r\nline9");
        m.history_grew(s.history_len() - h);
        assert_eq!(viewport(&m, &s), before);
    }

    #[test]
    fn encodes_base64() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }

    fn words() -> Screen {
        let mut s = Screen::new(20, 3, 100);
        s.feed(b"foo bar.baz  qux");
        s
    }

    fn at(m: &CopyMode, s: &Screen) -> (usize, usize) {
        m.cursor_abs(s)
    }

    #[test]
    fn searches_backward_into_history_and_centres_the_match() {
        let s = screen();
        let mut m = CopyMode::new(&s);
        assert!(m.search(&s, "line2".into(), false));
        assert_eq!(at(&m, &s), (1, 0));
        assert_eq!(viewport(&m, &s), vec!["line1", "line2", "line3"]);
        assert_eq!(m.search.as_ref().map(|x| x.forward), Some(false));
    }

    #[test]
    fn n_repeats_the_direction_and_capital_n_reverses_it() {
        let s = screen();
        let mut m = CopyMode::new(&s);
        m.cy = 2; // on "line8"
        assert!(m.search(&s, "line".into(), false), "a match on the cursor's line counts, going back");
        assert_eq!(at(&m, &s), (7, 0));
        assert_eq!(m.apply(CopyCmd::SearchNext, &s), Outcome::Stay);
        assert_eq!(at(&m, &s), (6, 0), "n goes on the way the search was typed");
        assert_eq!(m.apply(CopyCmd::SearchPrev, &s), Outcome::Stay);
        assert_eq!(at(&m, &s), (7, 0), "N goes the other way");
    }

    #[test]
    fn searching_wraps_around_and_reports_a_miss() {
        let s = screen();
        let mut m = CopyMode::new(&s);
        m.cy = 2;
        assert!(m.search(&s, "line1".into(), true), "forward from the last line wraps to the start");
        assert_eq!(at(&m, &s), (0, 0));
        assert_eq!(m.apply(CopyCmd::SearchNext, &s), Outcome::Stay, "the only match is the one it is on");
        assert_eq!(at(&m, &s), (0, 0));
        m.search = Some(Search { query: "zzz".into(), forward: true });
        assert_eq!(m.apply(CopyCmd::SearchNext, &s), Outcome::Notice("no match for zzz".into()));
        let mut fresh = CopyMode::new(&s);
        assert_eq!(fresh.apply(CopyCmd::SearchNext, &s), Outcome::Notice("no previous search".into()));
        assert_eq!(fresh.apply(CopyCmd::SearchForward, &s), Outcome::Search(true));
        assert_eq!(fresh.apply(CopyCmd::SearchBackward, &s), Outcome::Search(false));
    }

    #[test]
    fn search_is_case_insensitive_unless_the_query_has_a_capital() {
        let mut s = Screen::new(20, 2, 100);
        s.feed(b"Foo foo FOO");
        let line = s.abs_line(0).unwrap();
        assert_eq!(find_in_line(line, "foo"), vec![(0, 2), (4, 6), (8, 10)]);
        assert_eq!(find_in_line(line, "Foo"), vec![(0, 2)]);
        assert_eq!(find_in_line(line, ""), vec![]);
    }

    #[test]
    fn word_motions_follow_words_punctuation_and_blanks() {
        let s = words();
        let mut m = CopyMode::new(&s);
        m.cx = 0;
        m.cy = 0;
        let mut stops = Vec::new();
        for _ in 0..4 {
            m.apply(CopyCmd::WordNext, &s);
            stops.push(at(&m, &s).1);
        }
        assert_eq!(stops, vec![4, 7, 8, 13]);
        let mut back = Vec::new();
        for _ in 0..3 {
            m.apply(CopyCmd::WordPrev, &s);
            back.push(at(&m, &s).1);
        }
        assert_eq!(back, vec![8, 7, 4]);
        m.cx = 0;
        let mut ends = Vec::new();
        for _ in 0..3 {
            m.apply(CopyCmd::WordEnd, &s);
            ends.push(at(&m, &s).1);
        }
        assert_eq!(ends, vec![2, 6, 7]);
    }

    #[test]
    fn view_jumps_go_to_the_top_middle_and_bottom_row() {
        let s = screen();
        let mut m = CopyMode::new(&s);
        m.apply(CopyCmd::ViewTop, &s);
        assert_eq!(m.cy, 0);
        m.apply(CopyCmd::ViewMiddle, &s);
        assert_eq!(m.cy, 1);
        m.apply(CopyCmd::ViewBottom, &s);
        assert_eq!(m.cy, 2);
    }

    #[test]
    fn rectangle_selection_takes_the_same_columns_from_every_line() {
        let s = screen();
        let mut m = CopyMode::new(&s);
        m.cy = 0;
        m.cx = 1;
        m.apply(CopyCmd::SelectRect, &s);
        m.cy = 2;
        m.cx = 3;
        let sel = m.selection(&s);
        assert!(m.is_selected(sel, m.top(&s) + 1, 2) && !m.is_selected(sel, m.top(&s) + 1, 4));
        assert_eq!(m.apply(CopyCmd::Yank, &s), Outcome::Yanked("ine\nine\nine".into()));
        assert!(m.anchor.is_none() && !m.rect, "yanking ends the selection");
        // C-v again starts a new rectangle, and a second press clears it
        m.apply(CopyCmd::SelectRect, &s);
        assert!(m.anchor.is_some() && m.rect);
        m.apply(CopyCmd::SelectRect, &s);
        assert!(m.anchor.is_none() && !m.rect);
    }
}

