//! Drawing of the vertical tab sidebar.

use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::config::Position;
use crate::term::canvas::Canvas;
use crate::term::cell::{flags, Attrs, Cell, Color};

pub struct Item {
    pub label: String,
    /// Right-aligned status markers (activity, bell, zoom, ...).
    pub marks: String,
    pub active: bool,
    /// The keyboard selection while the sidebar has focus.
    pub selected: bool,
    /// A group's name above its windows, not an item of its own.
    pub heading: bool,
}

impl Item {
    /// A group's title, with its tag at the right edge.
    pub fn heading(label: String, tag: char) -> Item {
        Item { label, marks: tag.to_string(), active: false, selected: false, heading: true }
    }

    /// An empty line between groups.
    pub fn gap() -> Item {
        Item { label: String::new(), marks: String::new(), active: false, selected: false, heading: true }
    }
}

/// A sidebar line: a group heading (by group index), a gap between groups, or a row of
/// `Server::sidebar_rows`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Line {
    Group(usize),
    Gap,
    Row(usize),
}

pub struct Footer {
    pub text: String,
    pub prompt: bool,
}

pub struct SidebarView {
    pub header: String,
    pub subheader: String,
    /// A group's tag, at the right edge of the subheader.
    pub subtag: Option<char>,
    pub items: Vec<Item>,
    pub footer: Option<Footer>,
    pub focused: bool,
    /// Drawn as a rounded box (with the gap column facing the panes) instead of a column.
    pub islands: bool,
}

pub fn truncate(s: &str, max: usize) -> String {
    if s.width() <= max {
        return s.to_string();
    }
    if max == 0 {
        return String::new();
    }
    let mut out = String::new();
    let mut w = 0;
    for c in s.chars() {
        let cw = c.width().unwrap_or(0);
        if w + cw > max - 1 {
            break;
        }
        out.push(c);
        w += cw;
    }
    out.push('…');
    out
}

/// First item shown when the list is scrolled to keep the active one visible.
fn scroll_offset(active_idx: usize, capacity: usize) -> usize {
    if capacity > 0 && active_idx >= capacity { active_idx + 1 - capacity } else { 0 }
}

const FIRST_ITEM_ROW: usize = 2;

/// Which item is drawn on screen row `row`, given how `draw` lays them out
/// (`islands`: the box's bottom border takes the last row).
pub fn item_at(items: usize, active_idx: usize, rows: usize, has_footer: bool, islands: bool, row: usize) -> Option<usize> {
    let capacity = rows.saturating_sub(FIRST_ITEM_ROW + usize::from(has_footer) + usize::from(islands));
    let idx = scroll_offset(active_idx, capacity) + row.checked_sub(FIRST_ITEM_ROW)?;
    (row < FIRST_ITEM_ROW + capacity && idx < items).then_some(idx)
}

const BOLD: Attrs = Attrs { fg: Color::Default, bg: Color::Default, flags: flags::BOLD };
const DIM: Attrs = Attrs { fg: Color::Idx(8), bg: Color::Default, flags: 0 };
/// Colour-independent, so it stays readable in any terminal theme.
const SELECTED: Attrs = Attrs { fg: Color::Default, bg: Color::Default, flags: flags::BOLD | flags::REVERSE };
const FOCUS_SEP: Attrs = Attrs { fg: Color::Idx(2), bg: Color::Default, flags: 0 };
const REVERSE: Attrs = Attrs { fg: Color::Default, bg: Color::Default, flags: flags::REVERSE };

/// Draw the sidebar into columns `x0..x0+width` of the full height of `canvas`.
pub fn draw(canvas: &mut Canvas, x0: usize, width: usize, position: Position, view: &SidebarView) {
    if width < 2 {
        return;
    }
    let rows = canvas.rows;
    let (sep_x, text_x) = match position {
        Position::Left => (x0 + width - 1, x0),
        Position::Right => (x0, x0 + 1),
    };
    let mut text_w = width - 1;
    let mut text_x = text_x;
    let bottom = usize::from(view.islands);
    canvas.fill(x0, 0, width, rows, Cell::BLANK);
    if view.islands {
        // The separator column is the gap; the box takes the rest, borders included.
        if width < 5 || rows < 3 {
            return;
        }
        let box_x = if position == Position::Left { x0 } else { x0 + 1 };
        super::render::draw_box(canvas, (box_x, 0, width - 1, rows), if view.focused { FOCUS_SEP } else { DIM });
        text_x = box_x + 1;
        text_w = width - 3;
        // The session's name sits in the top border.
        let title = format!(" {} ", truncate(&view.header, text_w.saturating_sub(3)));
        canvas.put_str(box_x + 1, 0, &title, BOLD, text_w);
    } else {
        for y in 0..rows {
            canvas.set(sep_x, y, Cell { ch: '│', width: 1, attrs: if view.focused { FOCUS_SEP } else { DIM } });
        }
    }
    if rows == 0 {
        return;
    }
    if !view.islands {
        canvas.put_str(text_x, 0, &format!(" {}", truncate(&view.header, text_w.saturating_sub(1))), BOLD, text_w);
    }
    if rows > 1 {
        let tag_w = view.subtag.map_or(0, |_| 2);
        let title = truncate(&view.subheader, text_w.saturating_sub(1 + tag_w));
        canvas.put_str(text_x, 1, &format!(" {title}"), DIM, text_w);
        if let Some(tag) = view.subtag.filter(|_| text_w > 2) {
            canvas.put_str(text_x + text_w - 2, 1, &tag.to_string(), DIM, 1);
        }
    }
    let footer_rows = usize::from(view.footer.is_some());
    let first = FIRST_ITEM_ROW;
    let capacity = rows.saturating_sub(first + footer_rows + bottom);
    let active_idx = view.items.iter().position(|i| i.active).unwrap_or(0);
    let offset = scroll_offset(active_idx, capacity);
    for (row, item) in view.items.iter().skip(offset).take(capacity).enumerate() {
        let y = first + row;
        if item.heading {
            // Drawn like the subheader: dim, the tag at the right edge.
            canvas.fill(text_x, y, text_w, 1, Cell::BLANK);
            let label_w = text_w.saturating_sub(item.marks.width() + 2);
            canvas.put_str(text_x, y, &format!(" {}", truncate(&item.label, label_w)), DIM, text_w);
            if text_w > 2 {
                canvas.put_str(text_x + text_w - item.marks.width() - 1, y, &item.marks, DIM, item.marks.width());
            }
            continue;
        }
        let attrs = if item.selected {
            SELECTED
        } else if item.active && view.focused {
            // Only the selection is inverted while navigating.
            BOLD
        } else if item.active {
            REVERSE
        } else {
            Attrs::DEFAULT
        };
        let lead = if item.selected { '›' } else { ' ' };
        canvas.fill(text_x, y, text_w, 1, Cell::blank(attrs));
        let marks_w = item.marks.width();
        let label_w = text_w.saturating_sub(marks_w + 2);
        canvas.put_str(text_x, y, &format!("{lead}{}", truncate(&item.label, label_w)), attrs, text_w);
        if marks_w > 0 && marks_w < text_w {
            canvas.put_str(text_x + text_w - marks_w - 1, y, &item.marks, attrs, marks_w);
        }
    }
    if let Some(f) = &view.footer {
        let y = rows - 1 - bottom;
        let attrs = if f.prompt { REVERSE } else { DIM };
        canvas.fill(text_x, y, text_w, 1, Cell::blank(attrs));
        // The leading space goes first when the text only just fits (a narrow islands box).
        let lead = if f.text.width() < text_w { " " } else { "" };
        let room = text_w.saturating_sub(lead.len());
        canvas.put_str(text_x, y, &format!("{lead}{}", truncate(&f.text, room)), attrs, text_w);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(label: &str, marks: &str, active: bool) -> Item {
        Item { label: label.into(), marks: marks.into(), active, selected: false, heading: false }
    }

    fn view(items: Vec<Item>) -> SidebarView {
        SidebarView { header: "main".into(), subheader: "windows".into(), subtag: None, items, footer: None, focused: false, islands: false }
    }

    #[test]
    fn truncates_by_display_width() {
        assert_eq!(truncate("hello", 10), "hello");
        assert_eq!(truncate("hello world", 6), "hello…");
        assert_eq!(truncate("日本語日本語", 5), "日本…");
        assert_eq!(truncate("abc", 0), "");
    }

    #[test]
    fn draws_left_sidebar() {
        let mut c = Canvas::new(20, 6);
        draw(&mut c, 0, 12, Position::Left, &view(vec![item("1 vim", "●", false), item("2 sh", "", true)]));
        assert_eq!(c.row_text(0), " main      │");
        assert_eq!(c.row_text(1), " windows   │");
        assert_eq!(c.row_text(2), " 1 vim   ● │");
        assert_eq!(c.row_text(3), " 2 sh      │");
        assert!(c.cell(1, 3).attrs.has(flags::REVERSE));
        assert!(!c.cell(1, 2).attrs.has(flags::REVERSE));
        assert_eq!(c.cell(11, 5).ch, '│');
    }

    #[test]
    fn draws_right_sidebar() {
        let mut c = Canvas::new(20, 4);
        draw(&mut c, 8, 12, Position::Right, &view(vec![item("1 sh", "", true)]));
        assert_eq!(c.cell(8, 0).ch, '│');
        assert_eq!(c.cell(9, 0).ch, ' ');
        assert_eq!(c.cell(10, 0).ch, 'm');
    }

    #[test]
    fn long_labels_keep_marks() {
        let mut c = Canvas::new(14, 4);
        draw(&mut c, 0, 14, Position::Left, &view(vec![item("1 a-very-long-window-name", "!", false)]));
        let row = c.row_text(2);
        assert!(row.contains('…') && row.contains('!'), "{row:?}");
        assert_eq!(row.chars().count(), 14);
    }

    #[test]
    fn scrolls_to_keep_active_visible() {
        let items: Vec<Item> = (1..=10).map(|i| item(&format!("w{i}"), "", i == 10)).collect();
        let mut c = Canvas::new(14, 6);
        draw(&mut c, 0, 14, Position::Left, &view(items));
        // capacity is 4 rows (6 - 2 header rows)
        assert!(c.row_text(2).contains("w7"));
        assert!(c.row_text(5).contains("w10"));
    }

    #[test]
    fn item_at_matches_the_drawn_layout() {
        // 10 items, 6 rows, footer: capacity 3, active last -> items 7..9 shown
        assert_eq!(item_at(10, 9, 6, true, false, 2), Some(7));
        assert_eq!(item_at(10, 9, 6, true, false, 4), Some(9));
        assert_eq!(item_at(10, 9, 6, true, false, 5), None);
        assert_eq!(item_at(10, 9, 6, true, false, 1), None);
        assert_eq!(item_at(2, 0, 10, false, false, 3), Some(1));
        assert_eq!(item_at(2, 0, 10, false, false, 4), None);
        // islands: the bottom border takes a row
        assert_eq!(item_at(10, 9, 7, true, true, 2), Some(7));
        assert_eq!(item_at(10, 9, 7, true, true, 4), Some(9));
        assert_eq!(item_at(10, 9, 7, true, true, 5), None);
    }

    #[test]
    fn selected_row_is_marked_when_focused() {
        let mut c = Canvas::new(14, 5);
        let mut v = view(vec![item("1 a", "", true), item("2 b", "", false)]);
        v.items[1].selected = true;
        v.focused = true;
        draw(&mut c, 0, 14, Position::Left, &v);
        assert_eq!(c.cell(0, 3).ch, '›');
        // the selection is inverted and bold; the active row is only bold while focused
        assert!(c.cell(1, 3).attrs.has(flags::BOLD) && c.cell(1, 3).attrs.has(flags::REVERSE));
        assert!(c.cell(1, 2).attrs.has(flags::BOLD) && !c.cell(1, 2).attrs.has(flags::REVERSE));
        assert_eq!(c.cell(1, 3).attrs.fg, Color::Default);
        assert_eq!(c.cell(1, 3).attrs.bg, Color::Default);
        assert_eq!(c.cell(0, 2).ch, ' ');
        assert_eq!(c.cell(13, 0).attrs.fg, Color::Idx(2));
    }

    #[test]
    fn headings_are_dim_with_the_tag_at_the_right_edge() {
        let mut c = Canvas::new(14, 5);
        draw(&mut c, 0, 14, Position::Left, &view(vec![Item::heading("code".into(), 'g'), item("1 sh", "", true)]));
        assert_eq!(c.row_text(2), " code      g │");
        assert_eq!(c.cell(1, 2).attrs, DIM);
        assert!(c.cell(1, 3).attrs.has(flags::REVERSE));
        let mut v = view(vec![]);
        v.subtag = Some('m');
        draw(&mut c, 0, 14, Position::Left, &v);
        assert_eq!(c.row_text(1), " windows   m │");
    }

    #[test]
    fn islands_draw_a_rounded_box_with_the_gap_towards_the_panes() {
        let mut c = Canvas::new(14, 6);
        let mut v = view(vec![item("1 vim", "●", false), item("2 sh", "", true)]);
        v.islands = true;
        v.footer = Some(Footer { text: "hi".into(), prompt: false });
        draw(&mut c, 0, 14, Position::Left, &v);
        assert_eq!(c.row_text(0), "╭ main ─────╮");
        assert_eq!(c.row_text(1), "│ windows   │");
        assert_eq!(c.row_text(2), "│ 1 vim   ● │");
        assert_eq!(c.row_text(3), "│ 2 sh      │");
        assert_eq!(c.row_text(4), "│ hi        │");
        assert_eq!(c.row_text(5), "╰───────────╯");
        let mut c = Canvas::new(14, 4);
        draw(&mut c, 0, 14, Position::Right, &v);
        assert_eq!((c.cell(0, 0).ch, c.cell(1, 0).ch, c.cell(13, 0).ch), (' ', '╭', '╮'));
    }

    #[test]
    fn footer_is_drawn_on_last_row() {
        let mut c = Canvas::new(14, 5);
        let mut v = view(vec![item("1 sh", "", true)]);
        v.footer = Some(Footer { text: "rename: x".into(), prompt: true });
        draw(&mut c, 0, 14, Position::Left, &v);
        assert!(c.row_text(4).contains("rename: x"));
    }
}
