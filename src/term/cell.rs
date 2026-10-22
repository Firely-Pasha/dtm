#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Color {
    #[default]
    Default,
    Idx(u8),
    Rgb(u8, u8, u8),
}

pub mod flags {
    pub const BOLD: u16 = 1;
    pub const DIM: u16 = 2;
    pub const ITALIC: u16 = 4;
    pub const UNDERLINE: u16 = 8;
    pub const BLINK: u16 = 16;
    pub const REVERSE: u16 = 32;
    pub const HIDDEN: u16 = 64;
    pub const STRIKE: u16 = 128;
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Attrs {
    pub fg: Color,
    pub bg: Color,
    pub flags: u16,
}

impl Attrs {
    pub const DEFAULT: Attrs = Attrs { fg: Color::Default, bg: Color::Default, flags: 0 };

    pub fn has(&self, flag: u16) -> bool {
        self.flags & flag != 0
    }
}

/// One terminal cell. `width` is 1 for normal cells, 2 for the leading half of
/// a wide character and 0 for its trailing continuation cell.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cell {
    pub ch: char,
    pub width: u8,
    pub attrs: Attrs,
}

impl Cell {
    pub const BLANK: Cell = Cell { ch: ' ', width: 1, attrs: Attrs::DEFAULT };

    pub fn blank(attrs: Attrs) -> Cell {
        Cell { ch: ' ', width: 1, attrs }
    }
}

impl Default for Cell {
    fn default() -> Self {
        Cell::BLANK
    }
}
