//! A flat frame of cells plus terminal mode state, and a differ that turns
//! the change between two frames into escape sequences for a real terminal.

use super::cell::{flags, Attrs, Cell, Color};
use super::grid::fix_row;
use super::screen::Screen;

#[derive(Clone, Debug, PartialEq)]
pub struct Canvas {
    pub cols: usize,
    pub rows: usize,
    pub cells: Vec<Cell>,
    pub cursor: Option<(usize, usize)>,
    pub app_cursor: bool,
    pub bracketed_paste: bool,
    /// Ask the terminal for SGR mouse reports.
    pub mouse: bool,
}

impl Canvas {
    pub fn new(cols: usize, rows: usize) -> Self {
        Canvas {
            cols,
            rows,
            cells: vec![Cell::BLANK; cols * rows],
            cursor: None,
            app_cursor: false,
            bracketed_paste: false,
            mouse: false,
        }
    }

    pub fn cell(&self, x: usize, y: usize) -> &Cell {
        &self.cells[y * self.cols + x]
    }

    pub fn set(&mut self, x: usize, y: usize, cell: Cell) {
        if x < self.cols && y < self.rows {
            self.cells[y * self.cols + x] = cell;
        }
    }

    pub fn fill(&mut self, x: usize, y: usize, w: usize, h: usize, cell: Cell) {
        for yy in y..(y + h).min(self.rows) {
            for xx in x..(x + w).min(self.cols) {
                self.cells[yy * self.cols + xx] = cell;
            }
        }
    }

    /// Write `s` at (x, y), clipped to `max_w` columns. Returns columns used.
    pub fn put_str(&mut self, x: usize, y: usize, s: &str, attrs: Attrs, max_w: usize) -> usize {
        use unicode_width::UnicodeWidthChar;
        let mut cx = 0;
        for ch in s.chars() {
            let w = ch.width().unwrap_or(0);
            if w == 0 {
                continue;
            }
            if cx + w > max_w {
                break;
            }
            self.set(x + cx, y, Cell { ch, width: w as u8, attrs });
            if w == 2 {
                self.set(x + cx + 1, y, Cell { ch: ' ', width: 0, attrs });
            }
            cx += w;
        }
        cx
    }

    fn fix_edges(&mut self, dx: usize, dy: usize, w: usize, h: usize) {
        for y in dy..(dy + h).min(self.rows) {
            let row = &mut self.cells[y * self.cols..(y + 1) * self.cols];
            let right = (dx + w).min(self.cols);
            if right > dx && row[right - 1].width == 2 {
                row[right - 1] = Cell::blank(row[right - 1].attrs);
            }
            if row[dx].width == 0 {
                row[dx] = Cell::blank(row[dx].attrs);
            }
            fix_row(row);
        }
    }

    /// Copy the top-left `w` x `h` part of a screen to (dx, dy).
    pub fn blit_screen(&mut self, screen: &Screen, dx: usize, dy: usize, w: usize, h: usize) {
        let w = w.min(screen.cols()).min(self.cols.saturating_sub(dx));
        let h = h.min(screen.rows()).min(self.rows.saturating_sub(dy));
        if w == 0 || h == 0 {
            return;
        }
        for y in 0..h {
            for x in 0..w {
                self.cells[(dy + y) * self.cols + dx + x] = *screen.cell(x, y);
            }
        }
        self.fix_edges(dx, dy, w, h);
    }

    /// Copy a block of cells (all rows the same length) to (dx, dy).
    pub fn blit_cells(&mut self, rows: &[Vec<Cell>], dx: usize, dy: usize) {
        let w = rows.first().map_or(0, |r| r.len()).min(self.cols.saturating_sub(dx));
        let h = rows.len().min(self.rows.saturating_sub(dy));
        if w == 0 || h == 0 {
            return;
        }
        for (y, row) in rows.iter().take(h).enumerate() {
            self.cells[(dy + y) * self.cols + dx..(dy + y) * self.cols + dx + w].copy_from_slice(&row[..w]);
        }
        self.fix_edges(dx, dy, w, h);
    }

    /// Copy the top-left `w` x `h` part of another canvas to (dx, dy).
    pub fn blit_canvas(&mut self, src: &Canvas, dx: usize, dy: usize, w: usize, h: usize) {
        let w = w.min(src.cols).min(self.cols.saturating_sub(dx));
        let h = h.min(src.rows).min(self.rows.saturating_sub(dy));
        if w == 0 || h == 0 {
            return;
        }
        for y in 0..h {
            for x in 0..w {
                self.cells[(dy + y) * self.cols + dx + x] = *src.cell(x, y);
            }
        }
        self.fix_edges(dx, dy, w, h);
    }

    pub fn row_text(&self, y: usize) -> String {
        let s: String = self.cells[y * self.cols..(y + 1) * self.cols]
            .iter()
            .filter(|c| c.width != 0)
            .map(|c| c.ch)
            .collect();
        s.trim_end().to_string()
    }

    /// Escape sequences that turn a terminal showing `prev` into this frame.
    /// With no `prev` (or a different size) the whole screen is redrawn.
    /// Assumes the terminal has autowrap disabled.
    pub fn diff(&self, prev: Option<&Canvas>) -> Vec<u8> {
        let full = prev.is_none_or(|p| p.cols != self.cols || p.rows != self.rows);
        let blank;
        let prev = if full {
            blank = Canvas::new(self.cols, self.rows);
            &blank
        } else {
            prev.unwrap()
        };
        let cols = self.cols;
        let mut body: Vec<u8> = Vec::new();
        let mut cur: Option<Attrs> = None;
        if full {
            body.extend_from_slice(b"\x1b[0m\x1b[2J");
            cur = Some(Attrs::DEFAULT);
        }
        for y in 0..self.rows {
            let a = &self.cells[y * cols..(y + 1) * cols];
            let b = &prev.cells[y * cols..(y + 1) * cols];
            if a == b {
                continue;
            }
            let mut x = 0;
            while x < cols {
                if a[x] == b[x] {
                    x += 1;
                    continue;
                }
                let mut cx = if a[x].width == 0 && x > 0 { x - 1 } else { x };
                push_cup(&mut body, y, cx);
                loop {
                    let cell = &a[cx];
                    if cell.width != 0 {
                        if cur != Some(cell.attrs) {
                            push_sgr(&mut body, &cell.attrs);
                            cur = Some(cell.attrs);
                        }
                        let mut buf = [0u8; 4];
                        body.extend_from_slice(cell.ch.encode_utf8(&mut buf).as_bytes());
                    }
                    cx += 1;
                    if cx >= cols {
                        break;
                    }
                    // Bridge short unchanged gaps instead of repositioning.
                    let mut g = cx;
                    while g < cols && g < cx + 4 && a[g] == b[g] {
                        g += 1;
                    }
                    if g < cols && g < cx + 4 {
                        continue;
                    }
                    break;
                }
                x = cx;
            }
        }
        let modes_changed = self.app_cursor != prev.app_cursor
            || self.bracketed_paste != prev.bracketed_paste
            || self.mouse != prev.mouse;
        if body.is_empty() && !modes_changed && self.cursor == prev.cursor {
            return body;
        }
        let mut out = Vec::with_capacity(body.len() + 32);
        out.extend_from_slice(b"\x1b[?25l");
        out.extend_from_slice(&body);
        if self.app_cursor != prev.app_cursor {
            out.extend_from_slice(if self.app_cursor { b"\x1b[?1h" } else { b"\x1b[?1l" });
        }
        if self.bracketed_paste != prev.bracketed_paste {
            out.extend_from_slice(if self.bracketed_paste { b"\x1b[?2004h" } else { b"\x1b[?2004l" });
        }
        if self.mouse != prev.mouse {
            out.extend_from_slice(if self.mouse { b"\x1b[?1000h\x1b[?1006h" } else { b"\x1b[?1000l\x1b[?1006l" });
        }
        if let Some((x, y)) = self.cursor {
            push_cup(&mut out, y, x);
            out.extend_from_slice(b"\x1b[?25h");
        }
        out
    }
}

fn push_cup(out: &mut Vec<u8>, y: usize, x: usize) {
    out.extend_from_slice(format!("\x1b[{};{}H", y + 1, x + 1).as_bytes());
}

fn push_sgr(out: &mut Vec<u8>, a: &Attrs) {
    let mut s = String::from("\x1b[0");
    for (flag, code) in [
        (flags::BOLD, 1),
        (flags::DIM, 2),
        (flags::ITALIC, 3),
        (flags::UNDERLINE, 4),
        (flags::BLINK, 5),
        (flags::REVERSE, 7),
        (flags::HIDDEN, 8),
        (flags::STRIKE, 9),
    ] {
        if a.has(flag) {
            s.push_str(&format!(";{code}"));
        }
    }
    match a.fg {
        Color::Default => {}
        Color::Idx(n) => s.push_str(&format!(";38;5;{n}")),
        Color::Rgb(r, g, b) => s.push_str(&format!(";38;2;{r};{g};{b}")),
    }
    match a.bg {
        Color::Default => {}
        Color::Idx(n) => s.push_str(&format!(";48;5;{n}")),
        Color::Rgb(r, g, b) => s.push_str(&format!(";48;2;{r};{g};{b}")),
    }
    s.push('m');
    out.extend_from_slice(s.as_bytes());
}

/// Build a canvas from a whole screen, including its cursor and modes.
pub fn canvas_from_screen(screen: &Screen) -> Canvas {
    let mut c = Canvas::new(screen.cols(), screen.rows());
    c.blit_screen(screen, 0, 0, screen.cols(), screen.rows());
    let m = screen.modes();
    c.app_cursor = m.app_cursor;
    c.bracketed_paste = m.bracketed_paste;
    if m.cursor_visible {
        c.cursor = Some(screen.cursor_pos());
    }
    c
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cells_equal(screen: &Screen, canvas: &Canvas) {
        for y in 0..canvas.rows {
            for x in 0..canvas.cols {
                assert_eq!(screen.cell(x, y), canvas.cell(x, y), "cell {x},{y}");
            }
        }
    }

    /// Replay diffs into a fresh emulator (as a stand-in for a real terminal).
    fn replay(canvases: &[Canvas]) {
        let (cols, rows) = (canvases[0].cols, canvases[0].rows);
        let mut term = Screen::new(cols, rows, 0);
        term.feed(b"\x1b[?7l");
        let mut prev: Option<&Canvas> = None;
        for c in canvases {
            term.feed(&c.diff(prev));
            cells_equal(&term, c);
            assert_eq!(term.modes().app_cursor, c.app_cursor);
            assert_eq!(term.modes().bracketed_paste, c.bracketed_paste);
            assert_eq!(term.modes().cursor_visible, c.cursor.is_some());
            if let Some((x, y)) = c.cursor {
                assert_eq!(term.cursor_pos(), (x, y));
            }
            prev = Some(c);
        }
    }

    fn render(input: &str) -> Canvas {
        let mut s = Screen::new(20, 5, 0);
        s.feed(input.as_bytes());
        canvas_from_screen(&s)
    }

    #[test]
    fn full_redraw_reproduces_screen() {
        replay(&[render("hello \x1b[1;31mred\x1b[0m\r\n\x1b[48;2;1;2;3mbg\x1b[0m 日本語 end")]);
    }

    #[test]
    fn incremental_updates() {
        replay(&[
            render("one\r\ntwo"),
            render("one\r\ntwo three\r\nfour"),
            render("\x1b[2Jfresh"),
            render("\x1b[2Jfresh\x1b[?25l\x1b[?1h\x1b[?2004h"),
            render("日本\r\n日本"),
            render("日\r\n本x"),
        ]);
    }

    #[test]
    fn identical_frames_produce_no_output() {
        let c = render("abc");
        assert!(c.diff(Some(&c.clone())).is_empty());
    }

    #[test]
    fn size_change_forces_full_redraw() {
        let a = render("abc");
        let mut s = Screen::new(10, 3, 0);
        s.feed(b"xyz");
        let b = canvas_from_screen(&s);
        let out = b.diff(Some(&a));
        assert!(out.windows(4).any(|w| w == b"\x1b[2J"));
    }

    #[test]
    fn bridges_small_gaps_in_one_run() {
        let mut a = Canvas::new(20, 1);
        let mut b = a.clone();
        a.put_str(0, 0, "a b c", Attrs::DEFAULT, 20);
        b.put_str(0, 0, "a x c", Attrs::DEFAULT, 20);
        let out = String::from_utf8(b.diff(Some(&a))).unwrap();
        assert_eq!(out.matches('H').count(), 1, "{out:?}");
    }
}
