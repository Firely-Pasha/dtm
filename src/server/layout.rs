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

/// Smallest size a pane can be shrunk to by resizing.
const MIN_PANE: u16 = 2;

fn split_rect(axis: Axis, ratio: f32, r: Rect) -> (Rect, Rect) {
    let total = match axis {
        Axis::Cols => r.w,
        Axis::Rows => r.h,
    };
    if total < 3 {
        // No room for two panes and a border: the second gets nothing.
        let empty = Rect::new(r.x, r.y, 0, 0);
        return (r, empty);
    }
    let avail = total - 1;
    let alen = ((avail as f32 * ratio).round() as u16).clamp(1, avail - 1);
    match axis {
        Axis::Cols => (
            Rect::new(r.x, r.y, alen, r.h),
            Rect::new(r.x + alen + 1, r.y, avail - alen, r.h),
        ),
        Axis::Rows => (
            Rect::new(r.x, r.y, r.w, alen),
            Rect::new(r.x, r.y + alen + 1, r.w, avail - alen),
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

    pub fn layout(&self, rect: Rect, out: &mut Vec<(PaneId, Rect)>) {
        match self {
            Node::Leaf(id) => out.push((*id, rect)),
            Node::Split { axis, ratio, a, b } => {
                let (ra, rb) = split_rect(*axis, *ratio, rect);
                a.layout(ra, out);
                b.layout(rb, out);
            }
        }
    }

    /// Move the border nearest to `target` in direction `dir` by `amount` cells.
    pub fn resize(&mut self, target: PaneId, dir: Dir, amount: u16, rect: Rect) -> bool {
        let Node::Split { axis, ratio, a, b } = self else { return false };
        let (ra, rb) = split_rect(*axis, *ratio, rect);
        if a.contains(target) {
            if a.resize(target, dir, amount, ra) {
                return true;
            }
        } else if b.contains(target) && b.resize(target, dir, amount, rb) {
            return true;
        }
        let (applies, sign, total) = match (*axis, dir) {
            (Axis::Cols, Dir::Left) => (true, -1.0, rect.w),
            (Axis::Cols, Dir::Right) => (true, 1.0, rect.w),
            (Axis::Rows, Dir::Up) => (true, -1.0, rect.h),
            (Axis::Rows, Dir::Down) => (true, 1.0, rect.h),
            _ => (false, 0.0, 0),
        };
        if !applies || total < 2 * MIN_PANE + 1 {
            return false;
        }
        let avail = (total - 1) as f32;
        let min = MIN_PANE as f32 / avail;
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
        t.layout(Rect::new(0, 0, w, h), &mut v);
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
        assert!(t.resize(1, Dir::Right, 10, area));
        let r = rects(&t, 81, 25);
        assert_eq!(r[0].1.w, 50);
        // pane 3 growing down moves the horizontal border
        assert!(t.resize(3, Dir::Up, 4, Rect::new(51, 0, 30, 25)));
        let r = rects(&t, 81, 25);
        assert!(r[2].1.h > r[1].1.h);
        // no vertical split above pane 1 in the Rows direction
        assert!(!t.resize(1, Dir::Down, 1, area));
    }

    #[test]
    fn resize_clamps_to_min_size() {
        let mut t = tree();
        let area = Rect::new(0, 0, 81, 25);
        t.resize(1, Dir::Left, 1000, area);
        assert_eq!(rects(&t, 81, 25)[0].1.w, 2);
        t.resize(1, Dir::Right, 1000, area);
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
}
