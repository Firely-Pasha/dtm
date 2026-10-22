//! Compositing a window's panes (with borders) into a canvas.

use std::collections::HashMap;

use super::copy::CopyMode;
use super::layout::PaneId;
use super::state::{Pane, Window};
use crate::term::canvas::Canvas;
use crate::term::cell::{flags, Attrs, Cell, Color};

const BORDER: Attrs = Attrs { fg: Color::Idx(8), bg: Color::Default, flags: 0 };
const BORDER_ACTIVE: Attrs = Attrs { fg: Color::Idx(2), bg: Color::Default, flags: 0 };

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
pub fn render_window(window: &Window, panes: &HashMap<PaneId, Pane>, size: (u16, u16)) -> Canvas {
    let (cols, rows) = (size.0 as usize, size.1 as usize);
    let mut canvas = Canvas::new(cols, rows);
    let rects = window.rects(size);
    let mut owner: Vec<Option<PaneId>> = vec![None; cols * rows];
    for (id, r) in &rects {
        let Some(pane) = panes.get(id) else { continue };
        if r.w == 0 || r.h == 0 {
            continue;
        }
        let (x, y, w, h) = (r.x as usize, r.y as usize, r.w as usize, r.h as usize);
        match &pane.copy {
            Some(copy) => draw_copy_view(&mut canvas, pane, copy, (x, y, w, h)),
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
    if !window.zoomed {
        draw_borders(&mut canvas, &owner, window.active);
    }
    if let (Some(pane), Some((_, r))) = (
        panes.get(&window.active),
        rects.iter().find(|(id, _)| *id == window.active),
    ) {
        let m = pane.screen.modes();
        canvas.app_cursor = m.app_cursor;
        canvas.bracketed_paste = m.bracketed_paste;
        if let Some(copy) = &pane.copy {
            if copy.cx < r.w as usize && copy.cy < r.h as usize {
                canvas.cursor = Some((r.x as usize + copy.cx, r.y as usize + copy.cy));
            }
        } else if m.cursor_visible {
            let (cx, cy) = pane.screen.cursor_pos();
            if cx < r.w as usize && cy < r.h as usize {
                canvas.cursor = Some((r.x as usize + cx, r.y as usize + cy));
            }
        }
    }
    canvas
}

/// Draw a pane scrolled back in copy mode, with the selection highlighted.
fn draw_copy_view(canvas: &mut Canvas, pane: &Pane, copy: &CopyMode, (x, y, w, h): (usize, usize, usize, usize)) {
    let screen = &pane.screen;
    let top = copy.top(screen);
    let sel = copy.selection(screen);
    let rows: Vec<Vec<Cell>> = (0..h)
        .map(|row| {
            let line = screen.abs_line(top + row);
            (0..w)
                .map(|col| {
                    let mut cell = line.and_then(|l| l.get(col)).copied().unwrap_or(Cell::BLANK);
                    if copy.is_selected(sel, top + row, col) {
                        cell.attrs.flags ^= flags::REVERSE;
                    }
                    cell
                })
                .collect()
        })
        .collect();
    canvas.blit_cells(&rows, x, y);
    let label = format!("[copy {}/{}]", copy.offset, screen.history_len());
    if label.len() < w {
        let attrs = Attrs { fg: Color::Default, bg: Color::Default, flags: flags::REVERSE };
        canvas.put_str(x + w - label.len(), y, &label, attrs, label.len());
    }
}

fn draw_borders(canvas: &mut Canvas, owner: &[Option<PaneId>], active: PaneId) {
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
            let attrs = if touches_active { BORDER_ACTIVE } else { BORDER };
            canvas.set(x as usize, y as usize, Cell { ch: border_glyph(mask), width: 1, attrs });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pty::{Pty, SpawnOpts};
    use crate::server::layout::{Axis, Node};

    async fn pane(id: PaneId, cols: u16, rows: u16, text: &str) -> Pane {
        let pty = Pty::spawn(
            SpawnOpts { shell: "/bin/sh".into(), args: vec!["-c".into(), "sleep 5".into()], env: vec![], cwd: None, cols, rows },
            |_| {},
        )
        .unwrap();
        let mut screen = crate::term::screen::Screen::new(cols as usize, rows as usize, 100);
        screen.feed(text.as_bytes());
        Pane { id, screen, pty, last_title: String::new(), copy: None }
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
        }
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
        let c = render_window(&w, &panes, (9, 5));
        let rows: Vec<String> = (0..5).map(|y| c.row_text(y)).collect();
        assert_eq!(rows, vec!["one │two", "    │", "    ├────", "    │thre", "    │e"]);
        assert_eq!(c.cell(4, 0).attrs, BORDER_ACTIVE);
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
        let c = render_window(&w, &panes, (14, 2));
        assert_eq!(c.row_text(0), "aaa [copy 2/2]");
        assert_eq!(c.row_text(1), "bbb");
        assert!(c.cell(0, 0).attrs.has(flags::REVERSE) && c.cell(1, 0).attrs.has(flags::REVERSE));
        assert!(!c.cell(2, 0).attrs.has(flags::REVERSE));
        assert_eq!(c.cursor, Some((1, 0)));
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
        let c = render_window(&w, &panes, (9, 3));
        assert_eq!(c.row_text(0), "zoom");
        assert!(!(0..3).any(|y| c.row_text(y).contains('│')));
    }
}
