//! Compositing a window's panes (with borders) into a canvas.

use std::collections::HashMap;

use super::copy::CopyMode;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use super::layout::{PaneId, Rect, Spacing};
use super::sidebar::truncate;
use super::state::{Dock, Pane, Window};
use crate::term::canvas::Canvas;
use crate::term::cell::{flags, Attrs, Cell};
use crate::theme::Theme;

/// How a window is drawn: the style, whether its panes have the keyboard, and the colours.
#[derive(Clone, Copy)]
pub struct RenderOpts<'a> {
    pub islands: bool,
    /// False while the sidebar, dock, popup or a prompt has the keyboard, so no pane looks focused.
    pub focused: bool,
    pub theme: &'a Theme,
}

const UP: u8 = 1;
const DOWN: u8 = 2;
const LEFT: u8 = 4;
const RIGHT: u8 = 8;

fn border_glyph(mask: u8) -> char {
    match mask {
        m if m == UP | DOWN | LEFT | RIGHT => '┼',
        m if m == UP | DOWN | RIGHT => '├',
        m if m == UP | DOWN | LEFT => '┤',
        m if m == DOWN | LEFT | RIGHT => '┬',
        m if m == UP | LEFT | RIGHT => '┴',
        m if m == DOWN | RIGHT => '┌',
        m if m == DOWN | LEFT => '┐',
        m if m == UP | RIGHT => '└',
        m if m == UP | LEFT => '┘',
        m if m & (LEFT | RIGHT) != 0 && m & (UP | DOWN) == 0 => '─',
        _ => '│',
    }
}

/// Render `window` into a canvas of `size` (cols, rows).
/// A rounded box with its corners at (`x`, `y`) and `w` by `h` cells.
pub(super) fn draw_box(canvas: &mut Canvas, (x, y, w, h): (usize, usize, usize, usize), attrs: Attrs) {
    if w < 2 || h < 2 {
        return;
    }
    let edge = |ch| Cell { ch, width: 1, attrs };
    for dx in 1..w - 1 {
        canvas.set(x + dx, y, edge('─'));
        canvas.set(x + dx, y + h - 1, edge('─'));
    }
    for dy in 1..h - 1 {
        canvas.set(x, y + dy, edge('│'));
        canvas.set(x + w - 1, y + dy, edge('│'));
    }
    canvas.set(x, y, edge('╭'));
    canvas.set(x + w - 1, y, edge('╮'));
    canvas.set(x, y + h - 1, edge('╰'));
    canvas.set(x + w - 1, y + h - 1, edge('╯'));
}

/// A pane's text area inside its layout rect, which is a box in islands style.
fn area(r: &Rect, islands: bool) -> (usize, usize, usize, usize) {
    let (x, y, w, h) = (r.x as usize, r.y as usize, r.w as usize, r.h as usize);
    if islands { (x + 1, y + 1, w.saturating_sub(2).max(1), h.saturating_sub(2).max(1)) } else { (x, y, w, h) }
}

/// Render `window` at `size`.
pub fn render_window(window: &Window, panes: &HashMap<PaneId, Pane>, size: (u16, u16), opts: RenderOpts) -> Canvas {
    let RenderOpts { islands, focused, theme } = opts;
    let (cols, rows) = (size.0 as usize, size.1 as usize);
    let mut canvas = Canvas::new(cols, rows);
    if window.pending {
        // The placeholder is an island of its own, so the layout does not jump on loading.
        if islands {
            draw_box(&mut canvas, (0, 0, cols, rows), theme.border(focused));
        }
        let text = "Not loaded";
        let attrs = theme.muted();
        let (inner_w, inner_h) = if islands { (cols.saturating_sub(2), rows.saturating_sub(2)) } else { (cols, rows) };
        let (ox, oy) = if islands { (1, 1) } else { (0, 0) };
        canvas.put_str(ox + inner_w.saturating_sub(text.len()) / 2, oy + inner_h / 2, text, attrs, inner_w);
        let hint = "press o or Enter to load";
        canvas.put_str(ox + inner_w.saturating_sub(hint.len()) / 2, oy + inner_h / 2 + 1, hint, attrs, inner_w);
        return canvas;
    }
    let active_box = |id: PaneId| focused && id == window.active;
    let rects = window.rects(size, if islands { Spacing::ISLANDS } else { Spacing::CLASSIC });
    let mut owner: Vec<Option<PaneId>> = vec![None; cols * rows];
    for (id, r) in &rects {
        let Some(pane) = panes.get(id) else { continue };
        if r.w == 0 || r.h == 0 {
            continue;
        }
        if islands {
            let outer = (r.x as usize, r.y as usize, r.w as usize, r.h as usize);
            draw_box(&mut canvas, outer, theme.border(active_box(*id)));
        }
        let (x, y, w, h) = area(r, islands);
        match &pane.copy {
            Some(copy) => draw_copy_view(&mut canvas, pane, copy, (x, y, w, h), theme),
            None => canvas.blit_screen(&pane.screen, x, y, w, h),
        }
        for yy in y..(y + h).min(rows) {
            for xx in x..(x + w).min(cols) {
                owner[yy * cols + xx] = Some(*id);
            }
        }
        // Panes whose screen is smaller than their rect leave blanks behind.
        let _ = pane;
    }
    if !window.zoomed && !islands {
        // No pane id is 0, so with nothing focused no border is highlighted.
        draw_borders(&mut canvas, &owner, if focused { window.active } else { 0 }, theme);
    }
    if let (Some(pane), Some((_, r))) = (
        panes.get(&window.active),
        rects.iter().find(|(id, _)| *id == window.active),
    ) {
        let (ax, ay, aw, ah) = area(r, islands);
        let m = pane.screen.modes();
        canvas.app_cursor = m.app_cursor;
        canvas.bracketed_paste = m.bracketed_paste;
        canvas.cursor_style = pane.cursor_style();
        if !focused {
            canvas.cursor = None;
        } else if let Some(copy) = &pane.copy {
            if copy.cx < aw && copy.cy < ah {
                canvas.cursor = Some((ax + copy.cx, ay + copy.cy));
            }
        } else if m.cursor_visible {
            let (cx, cy) = pane.screen.cursor_pos();
            if cx < aw && cy < ah {
                canvas.cursor = Some((ax + cx, ay + cy));
            }
        }
    }
    canvas
}


/// The dock's tab row: (entry, first column, end column) of each tab shown,
/// scrolled so the active one is visible.
pub fn dock_tabs(names: &[&str], active: usize, width: usize) -> Vec<(usize, usize, usize)> {
    let widths: Vec<usize> = names.iter().map(|n| n.width() + 2).collect();
    let active = active.min(names.len().saturating_sub(1));
    let mut first = 0;
    while first < active && widths[first..=active].iter().sum::<usize>() + (active - first) > width {
        first += 1;
    }
    let mut out = Vec::new();
    let mut x = 0;
    for (i, w) in widths.iter().enumerate().skip(first) {
        let w = (*w).min(width.saturating_sub(x));
        if w < 3 {
            break;
        }
        out.push((i, x, x + w));
        x += w + 1;
    }
    out
}

pub(super) fn draw_pane_view(canvas: &mut Canvas, pane: &Pane, area: (usize, usize, usize, usize), focused: bool, theme: &Theme) -> Option<(usize, usize)> {
    let (x, y, w, h) = area;
    match &pane.copy {
        Some(copy) => {
            draw_copy_view(canvas, pane, copy, area, theme);
            (focused && copy.cx < w && copy.cy < h).then_some((x + copy.cx, y + copy.cy))
        }
        None => {
            canvas.blit_screen(&pane.screen, x, y, w, h);
            let (cx, cy) = pane.screen.cursor_pos();
            (focused && pane.screen.modes().cursor_visible && cx < w && cy < h).then_some((x + cx, y + cy))
        }
    }
}

/// Draw the dock into columns `x..x+w`; returns the cursor when it has focus.
/// `sep_left` puts the separator line on the dock's left edge; `opts.focused` says whether the dock has the keyboard.
pub fn draw_dock(
    canvas: &mut Canvas,
    (x, w): (usize, usize),
    sep_left: bool,
    dock: &Dock,
    panes: &HashMap<PaneId, Pane>,
    opts: RenderOpts,
) -> Option<(usize, usize)> {
    let RenderOpts { islands, focused, theme } = opts;
    let rows = canvas.rows;
    if w < 2 {
        return None;
    }
    canvas.fill(x, 0, w, rows, Cell::BLANK);
    let (sep_x, mut px) = if sep_left { (x, x + 1) } else { (x + w - 1, x) };
    let mut pw = w - 1;
    // Rows above the pane (the tab row, and in islands the box's top border) and below it.
    let (top, bottom) = if islands { (1, 1) } else { (0, 0) };
    let sep = theme.border(focused);
    if islands {
        if w < 5 || rows < 4 {
            return None;
        }
        draw_box(canvas, (px, 0, pw, rows), sep);
        px += 1;
        pw -= 2;
    } else {
        for y in 0..rows {
            canvas.set(sep_x, y, Cell { ch: '│', width: 1, attrs: sep });
        }
    }
    let names: Vec<&str> = dock.entries.iter().map(|e| e.name.as_str()).collect();
    for (i, x0, x1) in dock_tabs(&names, dock.active, pw) {
        let attrs = if i == dock.active {
            if focused { theme.selection() } else { theme.active() }
        } else {
            theme.muted()
        };
        canvas.fill(px + x0, top, x1 - x0, 1, Cell::blank(attrs));
        canvas.put_str(px + x0, top, &format!(" {} ", truncate(names[i], x1 - x0 - 2)), attrs, x1 - x0);
    }
    if rows < 2 + top + bottom {
        return None;
    }
    let body = top + 1;
    if dock.entries.is_empty() {
        canvas.put_str(px + 1, body, "no panes", theme.muted(), pw.saturating_sub(1));
        canvas.put_str(px + 1, body + 1, "prefix A adds one", theme.muted(), pw.saturating_sub(1));
        return None;
    }
    match dock.entries.get(dock.active).and_then(|e| e.pane).and_then(|p| panes.get(&p)) {
        Some(pane) => draw_pane_view(canvas, pane, (px, body, pw, rows - body - bottom), focused, theme),
        None => {
            canvas.put_str(px + 1, body, "starting…", theme.muted(), pw.saturating_sub(1));
            None
        }
    }
}

/// Draw the popup box at `ox + outer`; returns the cursor (`focused` is false behind a prompt).
pub fn draw_popup(canvas: &mut Canvas, ox: usize, outer: Rect, pane: Option<&Pane>, focused: bool, theme: &Theme) -> Option<(usize, usize)> {
    let (x, y, w, h) = (ox + outer.x as usize, outer.y as usize, outer.w as usize, outer.h as usize);
    if w < 3 || h < 3 {
        return None;
    }
    canvas.fill(x, y, w, h, Cell::BLANK);
    let border = theme.border(focused);
    draw_box(canvas, (x, y, w, h), border);
    if w > 12 {
        canvas.put_str(x + 2, y, " popup ", border, w - 4);
    }
    draw_pane_view(canvas, pane?, (x + 1, y + 1, w - 2, h - 2), focused, theme)
}

/// How many list lines the help box shows on a screen `rows` high.
pub fn help_body_rows(rows: usize) -> usize {
    rows.saturating_sub(5).max(1)
}

/// Draw the key list as a box over the screen, from line `scroll` on. Lines without an
/// indent are headings.
/// `query` is the search text and whether it is being typed.
pub fn draw_help(canvas: &mut Canvas, lines: &[String], scroll: usize, query: (&str, bool), theme: &Theme) {
    let (cols, rows) = (canvas.cols, canvas.rows);
    if cols < 24 || rows < 8 {
        return;
    }
    let (x, y, w, h) = (2, 1, cols - 4, rows - 2);
    canvas.fill(x, y, w, h, Cell::BLANK);
    draw_box(canvas, (x, y, w, h), theme.border(true));
    canvas.put_str(x + 2, y, " help ", theme.title(), w - 4);
    let body = help_body_rows(rows);
    for (i, line) in lines.iter().skip(scroll).take(body).enumerate() {
        let attrs = if line.starts_with(' ') { Attrs::DEFAULT } else { theme.title() };
        canvas.put_str(x + 2, y + 1 + i, line, attrs, w - 4);
    }
    let shown_to = (scroll + body).min(lines.len());
    let hint = match query {
        (q, true) => format!("/{q}   enter keep · esc clear"),
        ("", false) => format!("/ search · j/k scroll · space/b page · q close · {shown_to}/{}", lines.len()),
        (q, false) => format!("filter: {q} · / edit · esc clear · q close · {shown_to}/{}", lines.len()),
    };
    canvas.put_str(x + 2, y + h - 2, &hint, theme.muted(), w - 4);
    if query.1 {
        // The cursor sits after what is typed.
        canvas.cursor = Some((x + 2 + 1 + query.0.chars().count(), y + h - 2));
    }
}

/// A small label in the middle of `area` (x, y, w, h), e.g. the size of a pane being resized.
pub fn draw_badge(canvas: &mut Canvas, (x, y, w, h): (usize, usize, usize, usize), text: &str, theme: &Theme) {
    let label = format!(" {text} ");
    let lw = label.width();
    if w < lw || h == 0 {
        return;
    }
    let attrs = theme.selection();
    canvas.put_str(x + (w - lw) / 2, y + h / 2, &label, attrs, lw);
}

/// Split `text` into lines of at most `width` columns, breaking at spaces.
fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for word in text.split_whitespace() {
        match lines.last_mut() {
            Some(l) if l.width() + 1 + word.width() <= width => {
                l.push(' ');
                l.push_str(word);
            }
            _ => lines.push(word.to_string()),
        }
    }
    // A word longer than the box is cut by put_str; keep at least one line.
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

/// The end of `text` that fits in `width` columns.
fn tail(text: &str, width: usize) -> String {
    let mut out: Vec<char> = Vec::new();
    let mut used = 0;
    for ch in text.chars().rev() {
        let w = ch.width().unwrap_or(0);
        if used + w > width {
            break;
        }
        used += w;
        out.push(ch);
    }
    out.iter().rev().collect()
}

/// Suggestions listed under a prompt's input: the names, and the one chosen. A prompt that
/// has a list keeps room for it even when it is empty, so the box never changes size.
pub type Suggestions<'a> = Option<(&'a [String], Option<usize>)>;

/// The most suggestions a prompt lists at once.
const MAX_SUGGESTIONS: usize = 8;

/// Draw a prompt as a box in the middle of the screen, with `suggest` listed under its input;
/// returns the cursor of a text prompt.
pub fn draw_prompt(canvas: &mut Canvas, label: &str, text: &str, confirm: bool, suggest: Suggestions, theme: &Theme) -> Option<(usize, usize)> {
    let (cols, rows) = (canvas.cols, canvas.rows);
    let label = label.trim_end_matches(" (y/n)");
    let hint = if confirm { "y confirm  ·  n cancel" } else { "enter confirm  ·  esc cancel" };
    let max_inner = cols.saturating_sub(8).clamp(8, 64);
    let inner = (label.width().max(hint.width()).max(if confirm { 0 } else { 36 }) + 2).clamp(8, max_inner);
    let lines = wrap(label, inner - 2);
    // Suggestions take rows only while the box still fits on the screen.
    let body0 = lines.len() + usize::from(!confirm) * 2 + 2;
    let listed = suggest.filter(|_| !confirm).map_or(&[][..], |(names, _)| names);
    let reserved = if confirm || suggest.is_none() { 0 } else { MAX_SUGGESTIONS };
    let n = reserved.min(rows.saturating_sub(body0 + 2));
    let body = body0 + n;
    // Padding rows are dropped when the screen is short.
    let pad = usize::from(rows >= body + 4);
    let h = body + 2 + pad * 2;
    let w = inner + 2;
    if h > rows || w > cols {
        // Too small for the box: a bar on the bottom row.
        let attrs = theme.active();
        let line = if confirm { format!(" {label}  (y/n)") } else { format!(" {label}: {text}") };
        canvas.fill(0, rows - 1, cols, 1, Cell::blank(attrs));
        let used = canvas.put_str(0, rows - 1, &line, attrs, cols);
        return (!confirm).then_some((used.min(cols.saturating_sub(1)), rows - 1));
    }
    let (x, y) = ((cols - w) / 2, (rows - h) / 2);
    let edge = |ch| Cell { ch, width: 1, attrs: theme.border(true) };
    canvas.fill(x, y, w, h, Cell::BLANK);
    for dx in 1..w - 1 {
        canvas.set(x + dx, y, edge('─'));
        canvas.set(x + dx, y + h - 1, edge('─'));
    }
    for dy in 1..h - 1 {
        canvas.set(x, y + dy, edge('│'));
        canvas.set(x + w - 1, y + dy, edge('│'));
    }
    canvas.set(x, y, edge('╭'));
    canvas.set(x + w - 1, y, edge('╮'));
    canvas.set(x, y + h - 1, edge('╰'));
    canvas.set(x + w - 1, y + h - 1, edge('╯'));
    let bold = theme.title();
    let (tx, mut ty) = (x + 2, y + 1 + pad);
    for l in &lines {
        canvas.put_str(tx, ty, l, bold, inner - 2);
        ty += 1;
    }
    ty += 1;
    let mut cursor = None;
    if !confirm {
        let shown = tail(text, inner - 5);
        canvas.put_str(tx, ty, "›", theme.border(true), 1);
        let used = canvas.put_str(tx + 2, ty, &shown, Attrs::DEFAULT, inner - 4);
        cursor = Some((tx + 2 + used, ty));
        ty += 1;
        // The window of suggestions that keeps the chosen one in view.
        let chosen = suggest.and_then(|(_, c)| c);
        let first = chosen.map_or(0, |c| (c + 1).saturating_sub(n));
        for (i, name) in listed.iter().enumerate().skip(first).take(n) {
            let attrs = if Some(i) == chosen { theme.selection() } else { theme.muted() };
            canvas.fill(tx, ty, inner - 2, 1, Cell::blank(attrs));
            canvas.put_str(tx + 2, ty, name, attrs, inner - 4);
            ty += 1;
        }
    }
    // The hint sticks to the bottom of the box, however many suggestions are listed above it.
    let ty = y + h - 2 - pad;
    canvas.put_str(tx, ty, hint, theme.muted(), inner - 2);
    // The keys themselves stand out.
    let (first, second) = if confirm { (1, 1) } else { ("enter".len(), "esc".len()) };
    let second_at = tx + hint.find("·").map_or(0, |i| hint[..i].width() + "·  ".width());
    restyle(canvas, tx, ty, first, bold);
    restyle(canvas, second_at, ty, second, bold);
    cursor
}

fn restyle(canvas: &mut Canvas, x: usize, y: usize, n: usize, attrs: Attrs) {
    for i in 0..n {
        let mut cell = *canvas.cell(x + i, y);
        cell.attrs = attrs;
        canvas.set(x + i, y, cell);
    }
}

/// Draw a pane scrolled back in copy mode, with the selection highlighted.
fn draw_copy_view(canvas: &mut Canvas, pane: &Pane, copy: &CopyMode, (x, y, w, h): (usize, usize, usize, usize), theme: &Theme) {
    let screen = &pane.screen;
    let top = copy.top(screen);
    let sel = copy.selection(screen);
    let rows: Vec<Vec<Cell>> = (0..h)
        .map(|row| {
            let line = screen.abs_line(top + row);
            // Search matches on this line; the one under the cursor is the current one.
            let found = match (&copy.search, line) {
                (Some(s), Some(l)) => super::copy::find_in_line(l, &s.query),
                _ => Vec::new(),
            };
            (0..w)
                .map(|col| {
                    let mut cell = line.and_then(|l| l.get(col)).copied().unwrap_or(Cell::BLANK);
                    if let Some(m) = found.iter().find(|m| (m.0..=m.1).contains(&col)) {
                        let current = top + row == copy.cursor_abs(screen).0 && m.0 == copy.cx;
                        cell.attrs = theme.found(current);
                    }
                    if copy.is_selected(sel, top + row, col) {
                        cell.attrs.flags ^= flags::REVERSE;
                    }
                    cell
                })
                .collect()
        })
        .collect();
    canvas.blit_cells(&rows, x, y);
    let query = copy.search.as_ref().map(|s| format!(" {}{}", if s.forward { '/' } else { '?' }, s.query)).unwrap_or_default();
    let label = format!("[copy {}/{}{query}]", copy.offset, screen.history_len());
    if label.len() < w {
        canvas.put_str(x + w - label.len(), y, &label, theme.active(), label.len());
    }
}

fn draw_borders(canvas: &mut Canvas, owner: &[Option<PaneId>], active: PaneId, theme: &Theme) {
    let (cols, rows) = (canvas.cols, canvas.rows);
    let is_border = |x: isize, y: isize| -> bool {
        x >= 0 && y >= 0 && (x as usize) < cols && (y as usize) < rows && owner[y as usize * cols + x as usize].is_none()
    };
    let is_active = |x: isize, y: isize| -> bool {
        x >= 0
            && y >= 0
            && (x as usize) < cols
            && (y as usize) < rows
            && owner[y as usize * cols + x as usize] == Some(active)
    };
    for y in 0..rows as isize {
        for x in 0..cols as isize {
            if owner[y as usize * cols + x as usize].is_some() {
                continue;
            }
            let mut mask = 0;
            if is_border(x, y - 1) {
                mask |= UP;
            }
            if is_border(x, y + 1) {
                mask |= DOWN;
            }
            if is_border(x - 1, y) {
                mask |= LEFT;
            }
            if is_border(x + 1, y) {
                mask |= RIGHT;
            }
            let touches_active = is_active(x, y - 1) || is_active(x, y + 1) || is_active(x - 1, y) || is_active(x + 1, y);
            let attrs = theme.border(touches_active);
            canvas.set(x as usize, y as usize, Cell { ch: border_glyph(mask), width: 1, attrs });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pty::{Pty, SpawnOpts};
    use crate::server::layout::{Axis, Node};

    static THEME: Theme = Theme::DEFAULT;

    fn opts(islands: bool) -> RenderOpts<'static> {
        RenderOpts { islands, focused: true, theme: &THEME }
    }

    async fn pane(id: PaneId, cols: u16, rows: u16, text: &str) -> Pane {
        let pty = Pty::spawn(
            SpawnOpts { shell: "/bin/sh".into(), args: vec!["-c".into(), "sleep 5".into()], env: vec![], cwd: None, cols, rows },
            |_| {},
        )
        .unwrap();
        let mut screen = crate::term::screen::Screen::new(cols as usize, rows as usize, 100);
        screen.feed(text.as_bytes());
        Pane { id, spec: Default::default(), screen, pty, last_title: String::new(), copy: None }
    }

    fn window(root: Node, active: PaneId) -> Window {
        Window {
            id: 1,
            pinned_name: None,
            auto_name: "w".into(),
            root,
            active,
            last: None,
            zoomed: false,
            activity: false,
            bell: false,
            plan: None,
            pending: false,
            group: 1,
        }
    }

    #[tokio::test]
    async fn islands_are_rounded_boxes_around_each_pane() {
        // 1 | 2 in a 9x4 area: the layout gives 4 + gap + 4, so two 4-wide boxes with a gap.
        let root = Node::Split { axis: Axis::Cols, ratio: 0.5, a: Box::new(Node::Leaf(1)), b: Box::new(Node::Leaf(2)) };
        let w = window(root, 2);
        let mut panes = HashMap::new();
        panes.insert(1, pane(1, 2, 2, "ab\r\ncd").await);
        panes.insert(2, pane(2, 2, 2, "ef").await);
        let c = render_window(&w, &panes, (9, 4), opts(true));
        assert_eq!(c.row_text(0), "╭──╮ ╭──╮");
        assert_eq!(c.row_text(1), "│ab│ │ef│");
        assert_eq!(c.row_text(2), "│cd│ │  │");
        assert_eq!(c.row_text(3), "╰──╯ ╰──╯");
        // the focused pane's box is highlighted, the other one is not
        assert_eq!(c.cell(5, 0).attrs, Theme::DEFAULT.border(true));
        assert_eq!(c.cell(0, 0).attrs, Theme::DEFAULT.border(false));
        assert_eq!(c.cursor, Some((7, 1)));
    }

    #[tokio::test]
    async fn stacked_islands_touch_each_other() {
        let root = Node::Split { axis: Axis::Rows, ratio: 0.5, a: Box::new(Node::Leaf(1)), b: Box::new(Node::Leaf(2)) };
        let w = window(root, 1);
        let mut panes = HashMap::new();
        panes.insert(1, pane(1, 4, 1, "up").await);
        panes.insert(2, pane(2, 4, 1, "dn").await);
        let c = render_window(&w, &panes, (6, 6), opts(true));
        assert_eq!(c.row_text(0), "╭────╮");
        assert_eq!(c.row_text(1), "│up  │");
        assert_eq!(c.row_text(2), "╰────╯");
        assert_eq!(c.row_text(3), "╭────╮");
        assert_eq!(c.row_text(4), "│dn  │");
        assert_eq!(c.row_text(5), "╰────╯");
    }

    #[tokio::test]
    async fn draws_split_with_junctions() {
        // 1 | (2 / 3) in a 9x5 area: left 4 wide, border at x=4, right 4 wide
        let mut root = Node::Leaf(1);
        root.split(1, Axis::Cols, 2);
        root.split(2, Axis::Rows, 3);
        let w = window(root, 1);
        let mut panes = HashMap::new();
        panes.insert(1, pane(1, 4, 5, "one").await);
        panes.insert(2, pane(2, 4, 2, "two").await);
        panes.insert(3, pane(3, 4, 2, "three").await);
        let c = render_window(&w, &panes, (9, 5), opts(false));
        let rows: Vec<String> = (0..5).map(|y| c.row_text(y)).collect();
        assert_eq!(rows, vec!["one │two", "    │", "    ├────", "    │thre", "    │e"]);
        assert_eq!(c.cell(4, 0).attrs, Theme::DEFAULT.border(true));
        assert_eq!(c.cursor, Some((3, 0)));
    }

    #[tokio::test]
    async fn copy_mode_shows_history_and_selection() {
        let w = window(Node::Leaf(1), 1);
        let mut p = pane(1, 8, 2, "").await;
        p.screen.feed(b"aaa\r\nbbb\r\nccc\r\nddd");
        let mut copy = CopyMode::new(&p.screen);
        copy.scroll(2, &p.screen);
        copy.cy = 0;
        copy.cx = 1;
        copy.anchor = Some((copy.top(&p.screen), 0));
        p.copy = Some(copy);
        let mut panes = HashMap::new();
        panes.insert(1, p);
        let c = render_window(&w, &panes, (14, 2), opts(false));
        assert_eq!(c.row_text(0), "aaa [copy 2/2]");
        assert_eq!(c.row_text(1), "bbb");
        assert!(c.cell(0, 0).attrs.has(flags::REVERSE) && c.cell(1, 0).attrs.has(flags::REVERSE));
        assert!(!c.cell(2, 0).attrs.has(flags::REVERSE));
        assert_eq!(c.cursor, Some((1, 0)));
    }

    #[tokio::test]
    async fn copy_mode_shows_a_steady_block_cursor_whatever_the_program_asked_for() {
        let w = window(Node::Leaf(1), 1);
        let mut p = pane(1, 8, 2, "").await;
        p.screen.feed(b"\x1b[6 qabc"); // the program wants a bar
        assert_eq!(p.cursor_style(), 6);
        let mut panes = HashMap::new();
        panes.insert(1, p);
        assert_eq!(render_window(&w, &panes, (8, 2), opts(false)).cursor_style, 6);
        let p = panes.get_mut(&1).unwrap();
        p.copy = Some(CopyMode::new(&p.screen));
        assert_eq!(p.cursor_style(), 2);
        assert_eq!(render_window(&w, &panes, (8, 2), opts(false)).cursor_style, 2, "a steady block");
        let p = panes.get_mut(&1).unwrap();
        p.copy = None;
        assert_eq!(render_window(&w, &panes, (8, 2), opts(false)).cursor_style, 6, "and back again");
    }

    #[test]
    fn dock_tabs_scroll_to_the_active_one() {
        let names = ["git", "claude", "logs"];
        assert_eq!(dock_tabs(&names, 0, 40), vec![(0, 0, 5), (1, 6, 14), (2, 15, 21)]);
        let t = dock_tabs(&names, 2, 12);
        assert_eq!(t.first().map(|t| t.0), Some(2));
        assert!(dock_tabs(&[], 0, 10).is_empty());
    }

    #[tokio::test]
    async fn zoomed_window_shows_only_active_pane() {
        let mut root = Node::Leaf(1);
        root.split(1, Axis::Cols, 2);
        let mut w = window(root, 2);
        w.zoomed = true;
        let mut panes = HashMap::new();
        panes.insert(1, pane(1, 3, 3, "left").await);
        panes.insert(2, pane(2, 9, 3, "zoom").await);
        let c = render_window(&w, &panes, (9, 3), opts(false));
        assert_eq!(c.row_text(0), "zoom");
        assert!(!(0..3).any(|y| c.row_text(y).contains('│')));
    }
}
