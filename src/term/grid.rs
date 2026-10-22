use super::cell::Cell;

#[derive(Clone, Debug)]
pub struct Grid {
    pub cols: usize,
    pub rows: Vec<Vec<Cell>>,
}

impl Grid {
    pub fn new(cols: usize, rows: usize) -> Self {
        Grid { cols, rows: (0..rows).map(|_| vec![Cell::BLANK; cols]).collect() }
    }

    pub fn height(&self) -> usize {
        self.rows.len()
    }

    pub fn clear(&mut self) {
        for r in &mut self.rows {
            r.fill(Cell::BLANK);
        }
    }
}

/// Repair wide-character halves that were split by an edit, looking at
/// indices `lo..=hi` (clamped) with the rest of the row as context.
pub fn fix_range(row: &mut [Cell], lo: usize, hi: usize) {
    let n = row.len();
    if n == 0 {
        return;
    }
    let hi = hi.min(n - 1);
    for i in lo..=hi {
        match row[i].width {
            2 if i + 1 >= n || row[i + 1].width != 0 => {
                row[i] = Cell::blank(row[i].attrs);
            }
            0 if i == 0 || row[i - 1].width != 2 => {
                row[i] = Cell::blank(row[i].attrs);
            }
            _ => {}
        }
    }
}

pub fn fix_row(row: &mut [Cell]) {
    if !row.is_empty() {
        fix_range(row, 0, row.len() - 1);
    }
}
