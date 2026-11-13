pub fn spaces(size: u8) -> &'static str {
    const SPACES: &str = "                                                                                                                                                                                                                                                                ";
    &SPACES[..size as usize]
}

pub fn num_digits(i: usize) -> u8 {
    if i == 0 { 1 } else { i.ilog10() as u8 + 1 }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pos {
    pub row: usize,
    pub col: usize,
    pub offset: usize,
}

impl Pos {
    pub fn new(row: usize, col: usize, offset: usize) -> Self {
        Self { row, col, offset }
    }
}
