//! Binary split tree describing how a window's panes tile its area.

use crate::action::Dir;

pub type PaneId = u32;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x: u16,
    pub y: u16,
    pub w: u16,
    pub h: u16,
}

impl Rect {
    pub fn new(x: u16, y: u16, w: u16, h: u16) -> Rect {
        Rect { x, y, w, h }
    }

    fn right(&self) -> u16 {
        self.x + self.w
    }

    fn bottom(&self) -> u16 {
        self.y + self.h
    }
}

/// A split's border found under the mouse: see [`Node::hit_border`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BorderHit {
    pub path: Vec<bool>,
    pub axis: Axis,
    /// The area the split divides.
    pub rect: Rect,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Axis {
    /// Children sit side by side, separated by a vertical border.
    Cols,
    /// Children are stacked, separated by a horizontal border.
    Rows,
}

#[derive(Clone, Debug)]
pub enum Node {
    Leaf(PaneId),
    Split { axis: Axis, ratio: f32, a: Box<Node>, b: Box<Node> },
}

/// How panes are spaced: what separates them and how small resizing may make one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Spacing {
    /// Cells between stacked panes (side by side panes always have one).
    pub row_gap: u16,
    /// Smallest size a pane can be shrunk to by resizing.
    pub min_pane: u16,
    /// Cells on each side of the separation that count as the border when grabbing it with the
    /// mouse (a box has its own border cells next to the gap).
    pub grab: u16,
}

impl Spacing {
    /// Panes split by single lines.
    pub const CLASSIC: Spacing = Spacing { row_gap: 1, min_pane: 2, grab: 0 };
    /// Every pane is a box of its own: boxes sit directly on top of each other.
    pub const ISLANDS: Spacing = Spacing { row_gap: 0, min_pane: 3, grab: 1 };

    fn gap(&self, axis: Axis) -> u16 {
        match axis {
            Axis::Cols => 1,
            Axis::Rows => self.row_gap,
        }
    }
}

fn split_rect(axis: Axis, ratio: f32, r: Rect, sp: Spacing) -> (Rect, Rect) {
    let total = match axis {
        Axis::Cols => r.w,
        Axis::Rows => r.h,
    };
    let gap = sp.gap(axis);
    if total < 2 + gap {
        // No room for two panes and what separates them: the second gets nothing.
        let empty = Rect::new(r.x, r.y, 0, 0);
        return (r, empty);
    }
    let avail = total - gap;
    let alen = ((avail as f32 * ratio).round() as u16).clamp(1, avail - 1);
    match axis {
        Axis::Cols => (
            Rect::new(r.x, r.y, alen, r.h),
            Rect::new(r.x + alen + gap, r.y, avail - alen, r.h),
        ),
        Axis::Rows => (
            Rect::new(r.x, r.y, r.w, alen),
            Rect::new(r.x, r.y + alen + gap, r.w, avail - alen),
        ),
    }
}

impl Node {
    pub fn leaves(&self) -> Vec<PaneId> {
        let mut v = Vec::new();
        self.collect(&mut v);
        v
    }

    fn collect(&self, out: &mut Vec<PaneId>) {
        match self {
            Node::Leaf(id) => out.push(*id),
            Node::Split { a, b, .. } => {
                a.collect(out);
                b.collect(out);
            }
        }
    }

    pub fn first_leaf(&self) -> PaneId {
        match self {
            Node::Leaf(id) => *id,
            Node::Split { a, .. } => a.first_leaf(),
        }
    }

    pub fn contains(&self, target: PaneId) -> bool {
        match self {
            Node::Leaf(id) => *id == target,
            Node::Split { a, b, .. } => a.contains(target) || b.contains(target),
        }
    }

    /// Replace leaf `target` with a split holding it and `new`.
    pub fn split(&mut self, target: PaneId, axis: Axis, new: PaneId) -> bool {
        match self {
            Node::Leaf(id) if *id == target => {
                *self = Node::Split {
                    axis,
                    ratio: 0.5,
                    a: Box::new(Node::Leaf(target)),
                    b: Box::new(Node::Leaf(new)),
                };
                true
            }
            Node::Leaf(_) => false,
            Node::Split { a, b, .. } => a.split(target, axis, new) || b.split(target, axis, new),
        }
    }

    /// Put `new` where leaf `target` is.
    pub fn replace(&mut self, target: PaneId, new: PaneId) -> bool {
        match self {
            Node::Leaf(id) if *id == target => {
                *id = new;
                true
            }
            Node::Leaf(_) => false,
            Node::Split { a, b, .. } => a.replace(target, new) || b.replace(target, new),
        }
    }

    /// Remove `target`. Returns the remaining tree (None if it was the only
    /// pane) and the pane that should receive focus.
    pub fn remove(self, target: PaneId) -> (Option<Node>, Option<PaneId>) {
        match self {
            Node::Leaf(id) if id == target => (None, None),
            Node::Leaf(_) => (Some(self), None),
            Node::Split { axis, ratio, a, b } => {
                if a.contains(target) {
                    match a.remove(target) {
                        (None, _) => {
                            let focus = b.first_leaf();
                            (Some(*b), Some(focus))
                        }
                        (Some(na), f) => (Some(Node::Split { axis, ratio, a: Box::new(na), b }), f),
                    }
                } else if b.contains(target) {
                    match b.remove(target) {
                        (None, _) => {
                            let focus = a.first_leaf();
                            (Some(*a), Some(focus))
                        }
                        (Some(nb), f) => (Some(Node::Split { axis, ratio, a, b: Box::new(nb) }), f),
                    }
                } else {
                    (Some(Node::Split { axis, ratio, a, b }), None)
                }
            }
        }
    }

    /// The split whose border holds (`x`, `y`): the cells between its two children, widened by
    /// `sp.grab` towards each side. Gives the path to it (`false` = first child), its axis and rect.
    /// A deeper split wins over the widened zone of an outer one, so only exact gaps beat it.
    pub fn hit_border(&self, rect: Rect, sp: Spacing, x: u16, y: u16) -> Option<BorderHit> {
        let Node::Split { axis, ratio, a, b } = self else { return None };
        if x < rect.x || y < rect.y || x >= rect.right() || y >= rect.bottom() {
            return None;
        }
        let (ra, rb) = split_rect(*axis, *ratio, rect, sp);
        if rb.w == 0 || rb.h == 0 {
            return None;
        }
        let (pos, gap_from, gap_to) = match axis {
            Axis::Cols => (x, ra.right(), rb.x),
            Axis::Rows => (y, ra.bottom(), rb.y),
        };
        let here = BorderHit { path: Vec::new(), axis: *axis, rect };
        if (gap_from..gap_to).contains(&pos) {
            return Some(here);
        }
        for (second, child, r) in [(false, a, ra), (true, b, rb)] {
            if let Some(mut hit) = child.hit_border(r, sp, x, y) {
                hit.path.insert(0, second);
                return Some(hit);
            }
        }
        // Next to the gap: a box's own border cells.
        let (from, to) = (gap_from.saturating_sub(sp.grab), gap_to + sp.grab);
        (from..to).contains(&pos).then_some(here)
    }

    /// Move the border of the split at `path` so the first child is `first_len` cells long.
    pub fn set_first_len(&mut self, path: &[bool], first_len: u16, rect: Rect, sp: Spacing) -> bool {
        let Node::Split { axis, ratio, a, b } = self else { return false };
        if let Some((&second, rest)) = path.split_first() {
            let (ra, rb) = split_rect(*axis, *ratio, rect, sp);
            return if second { b.set_first_len(rest, first_len, rb, sp) } else { a.set_first_len(rest, first_len, ra, sp) };
        }
        let total = match axis {
            Axis::Cols => rect.w,
            Axis::Rows => rect.h,
        };
        let gap = sp.gap(*axis);
        if total < 2 * sp.min_pane + gap {
            return false;
        }
        let avail = (total - gap) as f32;
        let min = sp.min_pane as f32 / avail;
        *ratio = (first_len as f32 / avail).clamp(min, 1.0 - min);
        true
    }

    pub fn layout(&self, rect: Rect, sp: Spacing, out: &mut Vec<(PaneId, Rect)>) {
        match self {
            Node::Leaf(id) => out.push((*id, rect)),
            Node::Split { axis, ratio, a, b } => {
                let (ra, rb) = split_rect(*axis, *ratio, rect, sp);
                a.layout(ra, sp, out);
                b.layout(rb, sp, out);
            }
        }
    }

    /// Move the border nearest to `target` in direction `dir` by `amount` cells.
    pub fn resize(&mut self, target: PaneId, dir: Dir, amount: u16, rect: Rect, sp: Spacing) -> bool {
        let Node::Split { axis, ratio, a, b } = self else { return false };
        let (ra, rb) = split_rect(*axis, *ratio, rect, sp);
        if a.contains(target) {
            if a.resize(target, dir, amount, ra, sp) {
                return true;
            }
        } else if b.contains(target) && b.resize(target, dir, amount, rb, sp) {
            return true;
        }
        let (applies, sign, total) = match (*axis, dir) {
            (Axis::Cols, Dir::Left) => (true, -1.0, rect.w),
            (Axis::Cols, Dir::Right) => (true, 1.0, rect.w),
            (Axis::Rows, Dir::Up) => (true, -1.0, rect.h),
            (Axis::Rows, Dir::Down) => (true, 1.0, rect.h),
            _ => (false, 0.0, 0),
        };
        let gap = sp.gap(*axis);
        if !applies || total < 2 * sp.min_pane + gap {
            return false;
        }
        let avail = (total - gap) as f32;
        let min = sp.min_pane as f32 / avail;
        *ratio = (*ratio + sign * amount as f32 / avail).clamp(min, 1.0 - min);
        true
    }
}

/// Pick the pane adjacent to `active` in direction `dir`.
pub fn neighbor(rects: &[(PaneId, Rect)], active: PaneId, dir: Dir) -> Option<PaneId> {
    let ar = rects.iter().find(|(id, _)| *id == active)?.1;
    let overlap = |a0: u16, a1: u16, b0: u16, b1: u16| a1.min(b1).saturating_sub(a0.max(b0));
    rects
        .iter()
        .filter(|(id, r)| *id != active && r.w > 0 && r.h > 0)
        .filter_map(|(id, r)| {
            let (gap, ov) = match dir {
                Dir::Left if r.right() <= ar.x => (ar.x - r.right(), overlap(r.y, r.bottom(), ar.y, ar.bottom())),
                Dir::Right if r.x >= ar.right() => (r.x - ar.right(), overlap(r.y, r.bottom(), ar.y, ar.bottom())),
                Dir::Up if r.bottom() <= ar.y => (ar.y - r.bottom(), overlap(r.x, r.right(), ar.x, ar.right())),
                Dir::Down if r.y >= ar.bottom() => (r.y - ar.bottom(), overlap(r.x, r.right(), ar.x, ar.right())),
                _ => return None,
            };
            (ov > 0).then_some((*id, gap, ov))
        })
        .min_by_key(|&(_, gap, ov)| (gap, std::cmp::Reverse(ov)))
        .map(|(id, _, _)| id)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree() -> Node {
        // 1 | (2 / 3)
        let mut t = Node::Leaf(1);
        t.split(1, Axis::Cols, 2);
        t.split(2, Axis::Rows, 3);
        t
    }

    fn rects(t: &Node, w: u16, h: u16) -> Vec<(PaneId, Rect)> {
        let mut v = Vec::new();
        t.layout(Rect::new(0, 0, w, h), Spacing::CLASSIC, &mut v);
        v
    }

    #[test]
    fn layout_leaves_room_for_borders() {
        let r = rects(&tree(), 81, 25);
        assert_eq!(r[0], (1, Rect::new(0, 0, 40, 25)));
        assert_eq!(r[1], (2, Rect::new(41, 0, 40, 12)));
        assert_eq!(r[2], (3, Rect::new(41, 13, 40, 12)));
    }

    #[test]
    fn layout_degrades_in_tiny_areas() {
        let r = rects(&tree(), 2, 2);
        assert_eq!(r[0].1, Rect::new(0, 0, 2, 2));
        assert_eq!(r[1].1.w, 0);
    }

    #[test]
    fn replace_swaps_a_leaf() {
        let mut t = tree();
        assert!(t.replace(2, 9));
        assert!(!t.replace(2, 10));
        assert_eq!(t.leaves(), vec![1, 9, 3]);
    }

    #[test]
    fn remove_promotes_sibling() {
        let (t, focus) = tree().remove(2);
        let t = t.unwrap();
        assert_eq!(t.leaves(), vec![1, 3]);
        assert_eq!(focus, Some(3));
        let (t, focus) = t.remove(1);
        assert_eq!(t.unwrap().leaves(), vec![3]);
        assert_eq!(focus, Some(3));
        let (t, _) = Node::Leaf(9).remove(9);
        assert!(t.is_none());
    }

    #[test]
    fn resize_moves_the_right_border() {
        let mut t = tree();
        let area = Rect::new(0, 0, 81, 25);
        assert!(t.resize(1, Dir::Right, 10, area, Spacing::CLASSIC));
        let r = rects(&t, 81, 25);
        assert_eq!(r[0].1.w, 50);
        // pane 3 growing down moves the horizontal border
        assert!(t.resize(3, Dir::Up, 4, Rect::new(51, 0, 30, 25), Spacing::CLASSIC));
        let r = rects(&t, 81, 25);
        assert!(r[2].1.h > r[1].1.h);
        // no vertical split above pane 1 in the Rows direction
        assert!(!t.resize(1, Dir::Down, 1, area, Spacing::CLASSIC));
    }

    #[test]
    fn resize_clamps_to_min_size() {
        let mut t = tree();
        let area = Rect::new(0, 0, 81, 25);
        t.resize(1, Dir::Left, 1000, area, Spacing::CLASSIC);
        assert_eq!(rects(&t, 81, 25)[0].1.w, 2);
        t.resize(1, Dir::Right, 1000, area, Spacing::CLASSIC);
        assert_eq!(rects(&t, 81, 25)[1].1.w, 2);
    }

    #[test]
    fn focus_neighbors() {
        let r = rects(&tree(), 81, 25);
        assert_eq!(neighbor(&r, 1, Dir::Right), Some(2));
        assert_eq!(neighbor(&r, 2, Dir::Left), Some(1));
        assert_eq!(neighbor(&r, 3, Dir::Left), Some(1));
        assert_eq!(neighbor(&r, 2, Dir::Down), Some(3));
        assert_eq!(neighbor(&r, 3, Dir::Up), Some(2));
        assert_eq!(neighbor(&r, 1, Dir::Left), None);
        assert_eq!(neighbor(&r, 1, Dir::Up), None);
        assert_eq!(neighbor(&r, 3, Dir::Down), None);
    }

    #[test]
    fn islands_stack_boxes_with_no_gap_but_keep_the_one_between_columns() {
        let sp = Spacing::ISLANDS;
        let mut v = Vec::new();
        tree().layout(Rect::new(0, 0, 21, 10), sp, &mut v);
        // 1 | (2 / 3): a gap column between 1 and the right side, none between 2 and 3
        assert_eq!(v[0], (1, Rect::new(0, 0, 10, 10)));
        assert_eq!(v[1], (2, Rect::new(11, 0, 10, 5)));
        assert_eq!(v[2], (3, Rect::new(11, 5, 10, 5)));
    }

    #[test]
    fn islands_never_resize_a_box_below_three_cells() {
        let mut t = Node::Leaf(1);
        t.split(1, Axis::Rows, 2);
        let area = Rect::new(0, 0, 20, 10);
        assert!(t.resize(1, Dir::Down, 100, area, Spacing::ISLANDS));
        let mut v = Vec::new();
        t.layout(area, Spacing::ISLANDS, &mut v);
        assert_eq!(v[1].1.h, 3, "{v:?}");
        assert!(!t.resize(1, Dir::Down, 1, Rect::new(0, 0, 20, 5), Spacing::ISLANDS), "too small to move");
    }

    #[test]
    fn hit_border_finds_gaps_and_nested_splits() {
        let t = tree();
        let area = Rect::new(0, 0, 21, 10);
        let sp = Spacing::CLASSIC;
        let hit = |x, y| t.hit_border(area, sp, x, y);
        assert_eq!(hit(10, 3), Some(BorderHit { path: vec![], axis: Axis::Cols, rect: area }));
        assert_eq!(hit(15, 5), Some(BorderHit { path: vec![true], axis: Axis::Rows, rect: Rect::new(11, 0, 10, 10) }));
        assert_eq!(hit(3, 5), None, "inside a pane");
        assert_eq!(hit(15, 2), None);
        assert_eq!(hit(40, 2), None, "outside the area");
        assert_eq!(Node::Leaf(1).hit_border(area, sp, 1, 1), None);
    }

    #[test]
    fn hit_border_in_islands_includes_the_boxes_own_border_cells() {
        let t = tree();
        let area = Rect::new(0, 0, 21, 10);
        let sp = Spacing::ISLANDS;
        let hit = |x, y| t.hit_border(area, sp, x, y).map(|h| (h.path, h.axis));
        // the gap column, and the border columns of the boxes on either side of it
        assert_eq!(hit(10, 3), Some((vec![], Axis::Cols)));
        assert_eq!(hit(9, 3), Some((vec![], Axis::Cols)));
        assert_eq!(hit(11, 2), Some((vec![], Axis::Cols)));
        // the seam between the stacked boxes: the bottom border of 2 and the top border of 3
        assert_eq!(hit(15, 4), Some((vec![true], Axis::Rows)));
        assert_eq!(hit(15, 5), Some((vec![true], Axis::Rows)));
        assert_eq!(hit(15, 2), None, "inside a box");
        assert_eq!(hit(5, 5), None);
    }

    #[test]
    fn set_first_len_moves_a_border_within_the_limits() {
        let mut t = tree();
        let area = Rect::new(0, 0, 21, 10);
        let first = |t: &Node| rects(t, 21, 10)[0].1.w;
        assert!(t.set_first_len(&[], 5, area, Spacing::CLASSIC));
        assert_eq!(first(&t), 5);
        t.set_first_len(&[], 0, area, Spacing::CLASSIC);
        assert_eq!(first(&t), 2, "never below the minimum");
        t.set_first_len(&[], 100, area, Spacing::CLASSIC);
        assert_eq!(rects(&t, 21, 10)[1].1.w, 2, "nor leaving the other side below it");
        // a nested split
        let mut t = tree();
        assert!(t.set_first_len(&[true], 7, Rect::new(11, 0, 10, 10), Spacing::CLASSIC));
        let r = rects(&t, 21, 10);
        assert_eq!((r[1].1.h, r[2].1.y), (7, 8));
        assert!(!Node::Leaf(1).set_first_len(&[], 3, area, Spacing::CLASSIC));
    }
}

