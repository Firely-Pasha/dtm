//! Copy mode: scroll back through history, select text and yank it.

use crate::action::CopyCmd;
use crate::term::screen::Screen;

#[derive(Clone, Debug, Default)]
pub struct CopyMode {
    /// Lines scrolled back from the live screen.
    pub offset: usize,
    /// Cursor in viewport coordinates.
    pub cx: usize,
    pub cy: usize,
    /// Selection start as (absolute line, column).
    pub anchor: Option<(usize, usize)>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    Stay,
    Exit,
    Yanked(String),
}

impl CopyMode {
    pub fn new(screen: &Screen) -> CopyMode {
        let (cx, cy) = screen.cursor_pos();
        CopyMode { offset: 0, cx, cy, anchor: None }
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
            CopyCmd::LineStart => self.cx = 0,
            CopyCmd::LineEnd => {
                let (abs, _) = self.cursor_abs(s);
                let last = s
                    .abs_line(abs)
                    .and_then(|l| l.iter().rposition(|c| c.width != 0 && c.ch != ' '))
                    .unwrap_or(0);
                self.cx = last;
            }
            CopyCmd::Select => {
                self.anchor = if self.anchor.is_some() { None } else { Some(self.cursor_abs(s)) };
            }
            CopyCmd::Yank => return Outcome::Yanked(self.selected_text(s)),
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
        Some(if a <= b { (a, b) } else { (b, a) })
    }

    pub fn is_selected(&self, sel: Option<((usize, usize), (usize, usize))>, line: usize, x: usize) -> bool {
        match sel {
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
            let from = if abs == start.0 { start.1 } else { 0 };
            let to = if abs == end.0 { end.1.saturating_add(1) } else { usize::MAX };
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
}
