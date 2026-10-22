//! Terminal screen model: applies parser actions to a grid of cells.

use std::collections::VecDeque;

use unicode_width::UnicodeWidthChar;

use super::cell::{flags, Attrs, Cell, Color};
use super::grid::{fix_range, fix_row, Grid};
use super::parser::{Action, Parser};

#[derive(Clone, Copy, Debug, Default)]
struct Cursor {
    x: usize,
    y: usize,
    attrs: Attrs,
    pending_wrap: bool,
    g0_gfx: bool,
    g1_gfx: bool,
    shift_out: bool,
}

#[derive(Clone, Copy, Debug, Default)]
struct Saved {
    cursor: Cursor,
    origin: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct Modes {
    pub app_cursor: bool,
    pub origin: bool,
    pub autowrap: bool,
    pub cursor_visible: bool,
    pub insert: bool,
    pub bracketed_paste: bool,
    pub alt_screen: bool,
}

impl Default for Modes {
    fn default() -> Self {
        Modes {
            app_cursor: false,
            origin: false,
            autowrap: true,
            cursor_visible: true,
            insert: false,
            bracketed_paste: false,
            alt_screen: false,
        }
    }
}

pub struct Screen {
    cols: usize,
    rows: usize,
    primary: Grid,
    alt: Grid,
    cursor: Cursor,
    saved: Saved,
    saved_alt: Saved,
    top: usize,
    bottom: usize,
    tabs: Vec<bool>,
    modes: Modes,
    scrollback: VecDeque<Vec<Cell>>,
    history_limit: usize,
    last_char: char,
    replies: Vec<u8>,
    bell: bool,
    pub title: String,
    parser: Parser,
}

fn default_tabs(cols: usize) -> Vec<bool> {
    (0..cols).map(|i| i % 8 == 0).collect()
}

fn line_drawing(c: char) -> char {
    match c {
        '`' => '◆',
        'a' => '▒',
        'f' => '°',
        'g' => '±',
        'j' => '┘',
        'k' => '┐',
        'l' => '┌',
        'm' => '└',
        'n' => '┼',
        'o' => '⎺',
        'p' => '⎻',
        'q' => '─',
        'r' => '⎼',
        's' => '⎽',
        't' => '├',
        'u' => '┤',
        'v' => '┴',
        'w' => '┬',
        'x' => '│',
        'y' => '≤',
        'z' => '≥',
        '{' => 'π',
        '|' => '≠',
        '}' => '£',
        '~' => '·',
        c => c,
    }
}

impl Screen {
    pub fn new(cols: usize, rows: usize, history_limit: usize) -> Self {
        let cols = cols.max(1);
        let rows = rows.max(1);
        Screen {
            cols,
            rows,
            primary: Grid::new(cols, rows),
            alt: Grid::new(cols, rows),
            cursor: Cursor::default(),
            saved: Saved::default(),
            saved_alt: Saved::default(),
            top: 0,
            bottom: rows - 1,
            tabs: default_tabs(cols),
            modes: Modes::default(),
            scrollback: VecDeque::new(),
            history_limit,
            last_char: ' ',
            replies: Vec::new(),
            bell: false,
            title: String::new(),
            parser: Parser::new(),
        }
    }

    pub fn cols(&self) -> usize {
        self.cols
    }

    pub fn rows(&self) -> usize {
        self.rows
    }

    pub fn modes(&self) -> Modes {
        self.modes
    }

    pub fn cursor_pos(&self) -> (usize, usize) {
        (self.cursor.x, self.cursor.y)
    }

    pub fn scrollback_len(&self) -> usize {
        self.scrollback.len()
    }

    pub fn cell(&self, x: usize, y: usize) -> &Cell {
        &self.grid().rows[y][x]
    }

    /// Bytes the terminal wants to send back to the application (DSR, DA).
    pub fn take_replies(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.replies)
    }

    pub fn take_bell(&mut self) -> bool {
        std::mem::take(&mut self.bell)
    }

    pub fn row_text(&self, y: usize) -> String {
        let s: String = self.grid().rows[y]
            .iter()
            .filter(|c| c.width != 0)
            .map(|c| c.ch)
            .collect();
        s.trim_end().to_string()
    }

    pub fn text(&self) -> Vec<String> {
        (0..self.rows).map(|y| self.row_text(y)).collect()
    }

    fn grid(&self) -> &Grid {
        if self.modes.alt_screen { &self.alt } else { &self.primary }
    }

    fn grid_mut(&mut self) -> &mut Grid {
        if self.modes.alt_screen { &mut self.alt } else { &mut self.primary }
    }

    fn blank(&self) -> Cell {
        Cell::blank(Attrs { fg: Color::Default, bg: self.cursor.attrs.bg, flags: 0 })
    }

    pub fn feed(&mut self, bytes: &[u8]) {
        let mut p = std::mem::take(&mut self.parser);
        p.feed(bytes, &mut |a| self.apply(a));
        self.parser = p;
    }

    pub fn apply(&mut self, a: Action) {
        match a {
            Action::Print(c) => self.print(c),
            Action::Execute(b) => self.execute(b),
            Action::Esc { intermediates, fin } => self.esc(&intermediates, fin),
            Action::Csi { params, intermediates, private, fin } => {
                self.csi(&params, &intermediates, private, fin)
            }
            Action::Osc(parts) => self.osc(parts),
        }
    }

    // ---- printing and C0 ----

    fn print(&mut self, ch: char) {
        let gfx = if self.cursor.shift_out { self.cursor.g1_gfx } else { self.cursor.g0_gfx };
        let ch = if gfx { line_drawing(ch) } else { ch };
        let w = ch.width().unwrap_or(0);
        if w == 0 {
            return;
        }
        self.last_char = ch;
        if self.cursor.pending_wrap {
            if self.modes.autowrap {
                self.cursor.x = 0;
                self.linefeed();
            }
            self.cursor.pending_wrap = false;
        }
        let cols = self.cols;
        if w == 2 && self.cursor.x + 1 >= cols {
            if self.modes.autowrap && cols >= 2 {
                let (x, y) = (self.cursor.x, self.cursor.y);
                let b = self.blank();
                self.grid_mut().rows[y][x] = b;
                self.cursor.x = 0;
                self.linefeed();
            } else {
                return;
            }
        }
        let (x, y) = (self.cursor.x, self.cursor.y);
        if self.modes.insert {
            self.insert_chars(w);
        }
        let attrs = self.cursor.attrs;
        let row = &mut self.grid_mut().rows[y];
        row[x] = Cell { ch, width: w as u8, attrs };
        if w == 2 {
            row[x + 1] = Cell { ch: ' ', width: 0, attrs };
        }
        fix_range(row, x.saturating_sub(1), x + w + 1);
        self.cursor.x += w;
        if self.cursor.x >= cols {
            self.cursor.x = cols - 1;
            self.cursor.pending_wrap = true;
        }
    }

    fn execute(&mut self, b: u8) {
        match b {
            0x07 => self.bell = true,
            0x08 => {
                self.cursor.x = self.cursor.x.saturating_sub(1);
                self.cursor.pending_wrap = false;
            }
            0x09 => self.tab_forward(1),
            0x0A..=0x0C => self.linefeed(),
            0x0D => {
                self.cursor.x = 0;
                self.cursor.pending_wrap = false;
            }
            0x0E => self.cursor.shift_out = true,
            0x0F => self.cursor.shift_out = false,
            _ => {}
        }
    }

    fn tab_forward(&mut self, n: usize) {
        for _ in 0..n {
            let mut x = self.cursor.x + 1;
            while x < self.cols - 1 && !self.tabs[x] {
                x += 1;
            }
            self.cursor.x = x.min(self.cols - 1);
        }
        self.cursor.pending_wrap = false;
    }

    fn tab_back(&mut self, n: usize) {
        for _ in 0..n {
            let mut x = self.cursor.x;
            while x > 0 {
                x -= 1;
                if self.tabs[x] {
                    break;
                }
            }
            self.cursor.x = x;
        }
        self.cursor.pending_wrap = false;
    }

    fn linefeed(&mut self) {
        if self.cursor.y == self.bottom {
            self.scroll_up(1);
        } else if self.cursor.y + 1 < self.rows {
            self.cursor.y += 1;
        }
    }

    fn reverse_index(&mut self) {
        if self.cursor.y == self.top {
            self.scroll_down(1);
        } else if self.cursor.y > 0 {
            self.cursor.y -= 1;
        }
    }

    // ---- scrolling and editing ----

    fn scroll_up(&mut self, n: usize) {
        let (t, b) = (self.top, self.bottom);
        let save = t == 0 && b == self.rows - 1 && !self.modes.alt_screen;
        self.region_up(t, b, n, save);
    }

    fn scroll_down(&mut self, n: usize) {
        let (t, b) = (self.top, self.bottom);
        self.region_down(t, b, n);
    }

    fn region_up(&mut self, t: usize, b: usize, n: usize, save: bool) {
        let n = n.min(b + 1 - t);
        let blank = vec![self.blank(); self.cols];
        for _ in 0..n {
            let g = self.grid_mut();
            let removed = g.rows.remove(t);
            g.rows.insert(b, blank.clone());
            if save && self.history_limit > 0 {
                self.scrollback.push_back(removed);
                if self.scrollback.len() > self.history_limit {
                    self.scrollback.pop_front();
                }
            }
        }
    }

    fn region_down(&mut self, t: usize, b: usize, n: usize) {
        let n = n.min(b + 1 - t);
        let blank = vec![self.blank(); self.cols];
        let g = self.grid_mut();
        for _ in 0..n {
            g.rows.remove(b);
            g.rows.insert(t, blank.clone());
        }
    }

    fn insert_chars(&mut self, n: usize) {
        let (x, y) = (self.cursor.x, self.cursor.y);
        let n = n.min(self.cols - x);
        let blank = self.blank();
        let row = &mut self.grid_mut().rows[y];
        row.truncate(row.len() - n);
        for _ in 0..n {
            row.insert(x, blank);
        }
        fix_row(row);
    }

    fn delete_chars(&mut self, n: usize) {
        let (x, y) = (self.cursor.x, self.cursor.y);
        let n = n.min(self.cols - x);
        let blank = self.blank();
        let row = &mut self.grid_mut().rows[y];
        row.drain(x..x + n);
        row.extend(std::iter::repeat_n(blank, n));
        fix_row(row);
    }

    fn erase_in_row(&mut self, y: usize, a: usize, b: usize) {
        let blank = self.blank();
        let b = b.min(self.cols);
        if a >= b {
            return;
        }
        let row = &mut self.grid_mut().rows[y];
        row[a..b].fill(blank);
        fix_row(row);
    }

    fn erase_rows(&mut self, a: usize, b: usize) {
        for y in a..b.min(self.rows) {
            let cols = self.cols;
            self.erase_in_row(y, 0, cols);
        }
    }

    fn erase_display(&mut self, mode: u16) {
        let (x, y) = (self.cursor.x, self.cursor.y);
        let (cols, rows) = (self.cols, self.rows);
        match mode {
            0 => {
                self.erase_in_row(y, x, cols);
                self.erase_rows(y + 1, rows);
            }
            1 => {
                self.erase_rows(0, y);
                self.erase_in_row(y, 0, x + 1);
            }
            2 => self.erase_rows(0, rows),
            3 => self.scrollback.clear(),
            _ => {}
        }
    }

    fn erase_line(&mut self, mode: u16) {
        let (x, y, cols) = (self.cursor.x, self.cursor.y, self.cols);
        match mode {
            0 => self.erase_in_row(y, x, cols),
            1 => self.erase_in_row(y, 0, x + 1),
            2 => self.erase_in_row(y, 0, cols),
            _ => {}
        }
    }

    // ---- cursor ----

    fn move_to(&mut self, row: usize, col: usize) {
        let y = if self.modes.origin {
            (self.top + row).min(self.bottom)
        } else {
            row.min(self.rows - 1)
        };
        self.cursor.y = y;
        self.cursor.x = col.min(self.cols - 1);
        self.cursor.pending_wrap = false;
    }

    fn save_cursor(&mut self) {
        self.saved = Saved { cursor: self.cursor, origin: self.modes.origin };
    }

    fn restore_cursor(&mut self) {
        let s = self.saved;
        self.cursor = s.cursor;
        self.modes.origin = s.origin;
        self.cursor.x = self.cursor.x.min(self.cols - 1);
        self.cursor.y = self.cursor.y.min(self.rows - 1);
    }

    // ---- ESC ----

    fn esc(&mut self, inter: &[u8], fin: u8) {
        match (inter, fin) {
            ([], b'7') => self.save_cursor(),
            ([], b'8') => self.restore_cursor(),
            ([], b'D') => self.linefeed(),
            ([], b'E') => {
                self.cursor.x = 0;
                self.cursor.pending_wrap = false;
                self.linefeed();
            }
            ([], b'M') => self.reverse_index(),
            ([], b'H') => {
                let x = self.cursor.x;
                self.tabs[x] = true;
            }
            ([], b'c') => self.reset(),
            ([b'('], f) => self.cursor.g0_gfx = f == b'0',
            ([b')'], f) => self.cursor.g1_gfx = f == b'0',
            ([b'#'], b'8') => {
                for y in 0..self.rows {
                    let attrs = Attrs::DEFAULT;
                    self.grid_mut().rows[y].fill(Cell { ch: 'E', width: 1, attrs });
                }
            }
            _ => {}
        }
    }

    fn reset(&mut self) {
        let replies = std::mem::take(&mut self.replies);
        let parser = std::mem::take(&mut self.parser);
        *self = Screen::new(self.cols, self.rows, self.history_limit);
        self.replies = replies;
        self.parser = parser;
    }

    fn soft_reset(&mut self) {
        self.modes = Modes { alt_screen: self.modes.alt_screen, ..Modes::default() };
        self.cursor.attrs = Attrs::DEFAULT;
        self.cursor.g0_gfx = false;
        self.cursor.g1_gfx = false;
        self.cursor.shift_out = false;
        self.top = 0;
        self.bottom = self.rows - 1;
        self.saved = Saved::default();
    }

    // ---- OSC ----

    fn osc(&mut self, parts: Vec<Vec<u8>>) {
        let Some(cmd) = parts.first() else { return };
        if (cmd == b"0" || cmd == b"2") && parts.len() > 1 {
            let joined = parts[1..].join(&b';');
            self.title = String::from_utf8_lossy(&joined).into_owned();
        }
    }

    // ---- CSI ----

    fn csi(&mut self, params: &[Vec<u16>], inter: &[u8], private: Option<u8>, fin: u8) {
        let arg = |i: usize, d: usize| -> usize {
            match params.get(i).and_then(|p| p.first()) {
                Some(&v) if v > 0 => v as usize,
                _ => d,
            }
        };
        let raw = |i: usize| -> u16 { params.get(i).and_then(|p| p.first()).copied().unwrap_or(0) };
        if !inter.is_empty() {
            if inter == b"!" && fin == b'p' {
                self.soft_reset();
            }
            return;
        }
        match (private, fin) {
            (None, b'A') => {
                let min = if self.cursor.y >= self.top { self.top } else { 0 };
                self.cursor.y = self.cursor.y.saturating_sub(arg(0, 1)).max(min);
                self.cursor.pending_wrap = false;
            }
            (None, b'B' | b'e') => {
                let max = if self.cursor.y <= self.bottom { self.bottom } else { self.rows - 1 };
                self.cursor.y = (self.cursor.y + arg(0, 1)).min(max);
                self.cursor.pending_wrap = false;
            }
            (None, b'C' | b'a') => {
                self.cursor.x = (self.cursor.x + arg(0, 1)).min(self.cols - 1);
                self.cursor.pending_wrap = false;
            }
            (None, b'D') => {
                self.cursor.x = self.cursor.x.saturating_sub(arg(0, 1));
                self.cursor.pending_wrap = false;
            }
            (None, b'E') => {
                let max = if self.cursor.y <= self.bottom { self.bottom } else { self.rows - 1 };
                self.cursor.y = (self.cursor.y + arg(0, 1)).min(max);
                self.cursor.x = 0;
                self.cursor.pending_wrap = false;
            }
            (None, b'F') => {
                let min = if self.cursor.y >= self.top { self.top } else { 0 };
                self.cursor.y = self.cursor.y.saturating_sub(arg(0, 1)).max(min);
                self.cursor.x = 0;
                self.cursor.pending_wrap = false;
            }
            (None, b'G' | b'`') => {
                self.cursor.x = (arg(0, 1) - 1).min(self.cols - 1);
                self.cursor.pending_wrap = false;
            }
            (None, b'H' | b'f') => self.move_to(arg(0, 1) - 1, arg(1, 1) - 1),
            (None, b'd') => {
                let x = self.cursor.x;
                self.move_to(arg(0, 1) - 1, x);
            }
            (None, b'I') => self.tab_forward(arg(0, 1)),
            (None, b'Z') => self.tab_back(arg(0, 1)),
            (None, b'J') => self.erase_display(raw(0)),
            (None, b'K') => self.erase_line(raw(0)),
            (None, b'L') => {
                let (y, b) = (self.cursor.y, self.bottom);
                if y >= self.top && y <= b {
                    self.region_down(y, b, arg(0, 1));
                    self.cursor.x = 0;
                    self.cursor.pending_wrap = false;
                }
            }
            (None, b'M') => {
                let (y, b) = (self.cursor.y, self.bottom);
                if y >= self.top && y <= b {
                    self.region_up(y, b, arg(0, 1), false);
                    self.cursor.x = 0;
                    self.cursor.pending_wrap = false;
                }
            }
            (None, b'@') => self.insert_chars(arg(0, 1)),
            (None, b'P') => self.delete_chars(arg(0, 1)),
            (None, b'X') => {
                let (x, y) = (self.cursor.x, self.cursor.y);
                self.erase_in_row(y, x, x + arg(0, 1));
            }
            (None, b'S') => self.scroll_up(arg(0, 1)),
            (None, b'T') => self.scroll_down(arg(0, 1)),
            (None, b'b') => {
                let c = self.last_char;
                for _ in 0..arg(0, 1).min(self.cols * self.rows) {
                    self.print(c);
                }
            }
            (None, b'g') => match raw(0) {
                0 => {
                    let x = self.cursor.x;
                    self.tabs[x] = false;
                }
                3 => self.tabs.fill(false),
                _ => {}
            },
            (None, b'r') => {
                let t = arg(0, 1) - 1;
                let b = arg(1, self.rows).min(self.rows) - 1;
                if t < b {
                    self.top = t;
                    self.bottom = b;
                    self.move_to(0, 0);
                }
            }
            (None, b'm') => self.sgr(params),
            (None, b'n') => match raw(0) {
                5 => self.replies.extend_from_slice(b"\x1b[0n"),
                6 => {
                    let row = if self.modes.origin {
                        self.cursor.y.saturating_sub(self.top)
                    } else {
                        self.cursor.y
                    };
                    let s = format!("\x1b[{};{}R", row + 1, self.cursor.x + 1);
                    self.replies.extend_from_slice(s.as_bytes());
                }
                _ => {}
            },
            (None, b'c') if raw(0) == 0 => self.replies.extend_from_slice(b"\x1b[?1;2c"),
            (Some(b'>'), b'c') => self.replies.extend_from_slice(b"\x1b[>0;0;0c"),
            (None, b's') => self.save_cursor(),
            (None, b'u') => self.restore_cursor(),
            (None, b'h') => self.set_ansi_mode(params, true),
            (None, b'l') => self.set_ansi_mode(params, false),
            (Some(b'?'), b'h') => self.set_private_mode(params, true),
            (Some(b'?'), b'l') => self.set_private_mode(params, false),
            _ => {}
        }
    }

    fn set_ansi_mode(&mut self, params: &[Vec<u16>], on: bool) {
        for p in params {
            if p[0] == 4 {
                self.modes.insert = on;
            }
        }
    }

    fn set_private_mode(&mut self, params: &[Vec<u16>], on: bool) {
        for p in params {
            match p[0] {
                1 => self.modes.app_cursor = on,
                6 => {
                    self.modes.origin = on;
                    self.move_to(0, 0);
                }
                7 => self.modes.autowrap = on,
                25 => self.modes.cursor_visible = on,
                47 | 1047 => self.set_alt_screen(on, false, p[0] == 1047),
                1048 => {
                    if on {
                        self.save_cursor();
                    } else {
                        self.restore_cursor();
                    }
                }
                1049 => self.set_alt_screen(on, true, false),
                2004 => self.modes.bracketed_paste = on,
                _ => {}
            }
        }
    }

    fn set_alt_screen(&mut self, on: bool, save: bool, clear_on_exit: bool) {
        if on == self.modes.alt_screen {
            return;
        }
        if on {
            if save {
                self.saved_alt = Saved { cursor: self.cursor, origin: self.modes.origin };
            }
            self.modes.alt_screen = true;
            self.alt.clear();
        } else {
            if clear_on_exit {
                self.alt.clear();
            }
            self.modes.alt_screen = false;
            if save {
                let s = self.saved_alt;
                self.cursor = s.cursor;
                self.modes.origin = s.origin;
                self.cursor.x = self.cursor.x.min(self.cols - 1);
                self.cursor.y = self.cursor.y.min(self.rows - 1);
            }
        }
    }

    fn sgr(&mut self, params: &[Vec<u16>]) {
        if params.is_empty() {
            self.cursor.attrs = Attrs::DEFAULT;
            return;
        }
        let mut i = 0;
        while i < params.len() {
            let v = params[i][0];
            let a = &mut self.cursor.attrs;
            match v {
                0 => *a = Attrs::DEFAULT,
                1 => a.flags |= flags::BOLD,
                2 => a.flags |= flags::DIM,
                3 => a.flags |= flags::ITALIC,
                4 | 21 => a.flags |= flags::UNDERLINE,
                5 | 6 => a.flags |= flags::BLINK,
                7 => a.flags |= flags::REVERSE,
                8 => a.flags |= flags::HIDDEN,
                9 => a.flags |= flags::STRIKE,
                22 => a.flags &= !(flags::BOLD | flags::DIM),
                23 => a.flags &= !flags::ITALIC,
                24 => a.flags &= !flags::UNDERLINE,
                25 => a.flags &= !flags::BLINK,
                27 => a.flags &= !flags::REVERSE,
                28 => a.flags &= !flags::HIDDEN,
                29 => a.flags &= !flags::STRIKE,
                30..=37 => a.fg = Color::Idx((v - 30) as u8),
                39 => a.fg = Color::Default,
                40..=47 => a.bg = Color::Idx((v - 40) as u8),
                49 => a.bg = Color::Default,
                90..=97 => a.fg = Color::Idx((v - 90 + 8) as u8),
                100..=107 => a.bg = Color::Idx((v - 100 + 8) as u8),
                38 | 48 => {
                    if let Some(c) = ext_color(params, &mut i) {
                        if v == 38 {
                            self.cursor.attrs.fg = c;
                        } else {
                            self.cursor.attrs.bg = c;
                        }
                    }
                }
                _ => {}
            }
            i += 1;
        }
    }

    // ---- resize ----

    pub fn resize(&mut self, cols: usize, rows: usize) {
        let cols = cols.max(1);
        let rows = rows.max(1);
        if cols == self.cols && rows == self.rows {
            return;
        }
        if !self.modes.alt_screen {
            if rows < self.rows {
                // Push rows off the top until the cursor fits, then drop blanks below.
                let shift = (self.cursor.y + 1).saturating_sub(rows);
                for _ in 0..shift {
                    let r = self.primary.rows.remove(0);
                    if self.history_limit > 0 {
                        self.scrollback.push_back(r);
                        if self.scrollback.len() > self.history_limit {
                            self.scrollback.pop_front();
                        }
                    }
                }
                self.cursor.y -= shift;
            } else if rows > self.rows {
                let pull = (rows - self.rows).min(self.scrollback.len());
                for _ in 0..pull {
                    let r = self.scrollback.pop_back().unwrap();
                    self.primary.rows.insert(0, r);
                }
                self.cursor.y += pull;
            }
        }
        for g in [&mut self.primary, &mut self.alt] {
            g.rows.truncate(rows);
            while g.rows.len() < rows {
                g.rows.push(vec![Cell::BLANK; g.cols]);
            }
            for r in &mut g.rows {
                r.resize(cols, Cell::BLANK);
                fix_row(r);
            }
            g.cols = cols;
        }
        for r in &mut self.scrollback {
            r.resize(cols, Cell::BLANK);
            fix_row(r);
        }
        self.cols = cols;
        self.rows = rows;
        self.top = 0;
        self.bottom = rows - 1;
        self.tabs = default_tabs(cols);
        self.cursor.x = self.cursor.x.min(cols - 1);
        self.cursor.y = self.cursor.y.min(rows - 1);
        self.cursor.pending_wrap = false;
    }
}

fn ext_color(params: &[Vec<u16>], i: &mut usize) -> Option<Color> {
    let p = &params[*i];
    if p.len() > 1 {
        return match p[1] {
            5 => p.get(2).map(|&n| Color::Idx(n as u8)),
            2 if p.len() >= 6 => Some(Color::Rgb(p[3] as u8, p[4] as u8, p[5] as u8)),
            2 if p.len() == 5 => Some(Color::Rgb(p[2] as u8, p[3] as u8, p[4] as u8)),
            _ => None,
        };
    }
    let kind = params.get(*i + 1)?[0];
    match kind {
        5 => {
            let n = params.get(*i + 2)?[0];
            *i += 2;
            Some(Color::Idx(n as u8))
        }
        2 => {
            let r = params.get(*i + 2)?[0];
            let g = params.get(*i + 3)?[0];
            let b = params.get(*i + 4)?[0];
            *i += 4;
            Some(Color::Rgb(r as u8, g as u8, b as u8))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen(cols: usize, rows: usize, input: &str) -> Screen {
        let mut s = Screen::new(cols, rows, 100);
        s.feed(input.as_bytes());
        s
    }

    #[test]
    fn text_and_newlines() {
        let s = screen(10, 3, "hello\r\nworld");
        assert_eq!(s.text(), vec!["hello", "world", ""]);
        assert_eq!(s.cursor_pos(), (5, 1));
    }

    #[test]
    fn lf_without_cr_keeps_column() {
        let s = screen(10, 3, "ab\ncd");
        assert_eq!(s.text(), vec!["ab", "  cd", ""]);
    }

    #[test]
    fn autowrap_uses_pending_wrap() {
        let s = screen(5, 3, "abcde");
        assert_eq!(s.cursor_pos(), (4, 0));
        let s = screen(5, 3, "abcdef");
        assert_eq!(s.text(), vec!["abcde", "f", ""]);
        assert_eq!(s.cursor_pos(), (1, 1));
    }

    #[test]
    fn no_autowrap_overwrites_last_column() {
        let s = screen(5, 2, "\x1b[?7labcdefg");
        assert_eq!(s.text(), vec!["abcdg", ""]);
    }

    #[test]
    fn scrolls_into_scrollback() {
        let s = screen(4, 2, "a\r\nb\r\nc");
        assert_eq!(s.text(), vec!["b", "c"]);
        assert_eq!(s.scrollback_len(), 1);
    }

    #[test]
    fn cup_and_erase() {
        let s = screen(10, 3, "xxxxxxxxxx\x1b[1;4H\x1b[K");
        assert_eq!(s.text()[0], "xxx");
        let s = screen(6, 3, "aaaaaa\r\nbbbbbb\r\ncccccc\x1b[2;3H\x1b[1J");
        assert_eq!(s.text(), vec!["", "   bbb", "cccccc"]);
        let s = screen(6, 3, "aaaaaa\r\nbbbbbb\r\ncccccc\x1b[2J");
        assert_eq!(s.text(), vec!["", "", ""]);
    }

    #[test]
    fn sgr_colors_and_flags() {
        let s = screen(10, 1, "\x1b[1;31ma\x1b[38;5;200mb\x1b[48;2;1;2;3mc\x1b[0md");
        let a = s.cell(0, 0).attrs;
        assert_eq!(a.fg, Color::Idx(1));
        assert!(a.has(flags::BOLD));
        assert_eq!(s.cell(1, 0).attrs.fg, Color::Idx(200));
        assert_eq!(s.cell(2, 0).attrs.bg, Color::Rgb(1, 2, 3));
        assert_eq!(s.cell(3, 0).attrs, Attrs::DEFAULT);
    }

    #[test]
    fn sgr_colon_truecolor() {
        let s = screen(4, 1, "\x1b[38:2::10:20:30ma");
        assert_eq!(s.cell(0, 0).attrs.fg, Color::Rgb(10, 20, 30));
    }

    #[test]
    fn bright_colors() {
        let s = screen(4, 1, "\x1b[91;102ma");
        assert_eq!(s.cell(0, 0).attrs.fg, Color::Idx(9));
        assert_eq!(s.cell(0, 0).attrs.bg, Color::Idx(10));
    }

    #[test]
    fn scroll_region() {
        let s = screen(4, 4, "1\r\n2\r\n3\r\n4\x1b[2;3r\x1b[3;1H\n");
        assert_eq!(s.text(), vec!["1", "3", "", "4"]);
        assert_eq!(s.scrollback_len(), 0);
    }

    #[test]
    fn reverse_index_scrolls_down_at_top() {
        let s = screen(4, 3, "a\r\nb\r\nc\x1b[H\x1bM");
        assert_eq!(s.text(), vec!["", "a", "b"]);
    }

    #[test]
    fn insert_delete_lines() {
        let s = screen(4, 4, "1\r\n2\r\n3\r\n4\x1b[2;1H\x1b[L");
        assert_eq!(s.text(), vec!["1", "", "2", "3"]);
        let s = screen(4, 4, "1\r\n2\r\n3\r\n4\x1b[2;1H\x1b[M");
        assert_eq!(s.text(), vec!["1", "3", "4", ""]);
    }

    #[test]
    fn insert_delete_erase_chars() {
        let s = screen(6, 1, "abcdef\x1b[1;2H\x1b[2@");
        assert_eq!(s.text(), vec!["a  bcd"]);
        let s = screen(6, 1, "abcdef\x1b[1;2H\x1b[2P");
        assert_eq!(s.text(), vec!["adef"]);
        let s = screen(6, 1, "abcdef\x1b[1;2H\x1b[2X");
        assert_eq!(s.text(), vec!["a  def"]);
    }

    #[test]
    fn alt_screen_roundtrip() {
        let mut s = screen(6, 2, "main\x1b[?1049h");
        assert!(s.modes().alt_screen);
        assert_eq!(s.text(), vec!["", ""]);
        s.feed(b"\x1b[Halt");
        assert_eq!(s.text()[0], "alt");
        s.feed(b"\x1b[?1049l");
        assert_eq!(s.text()[0], "main");
        assert_eq!(s.cursor_pos(), (4, 0));
    }

    #[test]
    fn wide_chars() {
        let s = screen(6, 2, "a日b");
        assert_eq!(s.cell(1, 0).width, 2);
        assert_eq!(s.cell(2, 0).width, 0);
        assert_eq!(s.cursor_pos(), (4, 0));
        assert_eq!(s.text()[0], "a日b");
    }

    #[test]
    fn wide_char_wraps_at_margin() {
        let s = screen(3, 2, "ab日");
        assert_eq!(s.text(), vec!["ab", "日"]);
    }

    #[test]
    fn overwriting_half_of_wide_char_clears_it() {
        let s = screen(6, 1, "日\x1b[1;2Hx");
        assert_eq!(s.text(), vec![" x"]);
    }

    #[test]
    fn dsr_and_da_replies() {
        let mut s = screen(10, 5, "\x1b[3;4H\x1b[6n\x1b[5n\x1b[c");
        assert_eq!(s.take_replies(), b"\x1b[3;4R\x1b[0n\x1b[?1;2c");
    }

    #[test]
    fn osc_title() {
        let s = screen(10, 1, "\x1b]0;my;title\x07");
        assert_eq!(s.title, "my;title");
    }

    #[test]
    fn line_drawing_charset() {
        let s = screen(4, 1, "\x1b(0lqk\x1b(Bq");
        assert_eq!(s.text(), vec!["┌─┐q"]);
    }

    #[test]
    fn save_restore_cursor() {
        let s = screen(10, 3, "ab\x1b7\x1b[3;9H\x1b8c");
        assert_eq!(s.text()[0], "abc");
    }

    #[test]
    fn tabs() {
        let s = screen(20, 1, "a\tb\tc");
        assert_eq!(s.text()[0], "a       b       c");
    }

    #[test]
    fn origin_mode_is_region_relative() {
        let s = screen(5, 5, "\x1b[2;4r\x1b[?6h\x1b[1;1Hx");
        assert_eq!(s.text(), vec!["", "x", "", "", ""]);
    }

    #[test]
    fn resize_shrink_keeps_cursor_row_and_pushes_history() {
        let mut s = screen(5, 4, "1\r\n2\r\n3\r\n4");
        s.resize(5, 2);
        assert_eq!(s.text(), vec!["3", "4"]);
        assert_eq!(s.scrollback_len(), 2);
        s.resize(5, 4);
        assert_eq!(s.text(), vec!["1", "2", "3", "4"]);
        assert_eq!(s.cursor_pos(), (1, 3));
    }

    #[test]
    fn resize_columns() {
        let mut s = screen(6, 2, "abcdef");
        s.resize(3, 2);
        assert_eq!(s.text()[0], "abc");
        s.resize(8, 2);
        assert_eq!(s.cols(), 8);
    }

    #[test]
    fn rep_repeats_last_char() {
        let s = screen(10, 1, "x\x1b[3b");
        assert_eq!(s.text(), vec!["xxxx"]);
    }

    #[test]
    fn mode_flags() {
        let s = screen(4, 1, "\x1b[?1h\x1b[?2004h\x1b[?25l");
        let m = s.modes();
        assert!(m.app_cursor && m.bracketed_paste && !m.cursor_visible);
    }
}
