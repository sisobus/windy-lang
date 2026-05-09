//! Sparse grid and instruction pointer (SPEC §3.1, §3.2).
//!
//! Cell values are `BigInt` — the grid stores arbitrary-precision
//! integers per SPEC §3.1 ("G : ℤ × ℤ → ℤ"). Coordinates are `i64`;
//! this is a pragmatic departure from SPEC (which implies arbitrary-
//! precision coords), justified by: no realistic Windy program comes
//! near `i64` bounds, and unboxed `Copy` coords make the VM inner loop
//! substantially cheaper. If a Windy program ever needs BigInt coords
//! we'll know because it'll fail a conformance test — at which point
//! we upgrade.

use num_bigint::BigInt;
use std::collections::HashMap;

/// ASCII space (NOP) — the default value of every unpopulated cell.
pub const SPACE: u32 = 0x20;

#[derive(Debug, Clone, Default)]
pub struct Grid {
    pub cells: HashMap<(i64, i64), BigInt>,
}

impl Grid {
    pub fn new() -> Self {
        Self::default()
    }

    /// Read a cell; absent cells return `SPACE` (NOP) per SPEC §3.1.
    pub fn get(&self, x: i64, y: i64) -> BigInt {
        self.cells
            .get(&(x, y))
            .cloned()
            .unwrap_or_else(|| BigInt::from(SPACE))
    }

    /// Write a cell; writing `SPACE` deletes the entry so the map stays sparse.
    pub fn put(&mut self, x: i64, y: i64, value: BigInt) {
        if value == BigInt::from(SPACE) {
            self.cells.remove(&(x, y));
        } else {
            self.cells.insert((x, y), value);
        }
    }
}

#[cfg(feature = "metrics")]
impl Grid {
    /// `(width, height)` of the bounding box that encloses every populated
    /// cell, treating the top-left of the rendered source as `(0, 0)`.
    /// Negative coordinates pull the box left/up; the returned values are
    /// the maximum extent observed in each direction (always ≥ 1 if any
    /// cell exists). Empty grid returns `(0, 0)`.
    pub fn bounding_box(&self) -> (u32, u32) {
        if self.cells.is_empty() {
            return (0, 0);
        }
        let mut min_x: i64 = i64::MAX;
        let mut max_x: i64 = i64::MIN;
        let mut min_y: i64 = i64::MAX;
        let mut max_y: i64 = i64::MIN;
        for &(x, y) in self.cells.keys() {
            if x < min_x { min_x = x; }
            if x > max_x { max_x = x; }
            if y < min_y { min_y = y; }
            if y > max_y { max_y = y; }
        }
        let width = (max_x - min_x).saturating_add(1) as u32;
        let height = (max_y - min_y).saturating_add(1) as u32;
        (width, height)
    }

    /// `width × height` of the bounding box. Saturates on overflow.
    pub fn total_grid_cells(&self) -> u32 {
        let (w, h) = self.bounding_box();
        w.saturating_mul(h)
    }

    /// Cells whose contents decode to a *meaningful* opcode — anything other
    /// than `Op::Nop` (a literal space) or `Op::Unknown` (an unrecognised
    /// glyph that the runtime treats as NOP). String-mode behaviour is
    /// not considered: a `,` cell counts as effective whether or not it
    /// happens to lie inside an active string literal at runtime.
    pub fn effective_cells(&self) -> u32 {
        use crate::opcodes::{decode_cell, Op};
        let mut n: u32 = 0;
        for cell in self.cells.values() {
            let (op, _) = decode_cell(cell);
            if !matches!(op, Op::Nop | Op::Unknown) {
                n = n.saturating_add(1);
            }
        }
        n
    }
}

/// Instruction pointer: position `(x, y)` and direction `(dx, dy)`.
/// Initial value is `(0, 0)` going east (SPEC §3.2).
#[derive(Debug, Clone, Copy)]
pub struct Ip {
    pub x: i64,
    pub y: i64,
    pub dx: i64,
    pub dy: i64,
}

impl Default for Ip {
    fn default() -> Self {
        Self { x: 0, y: 0, dx: 1, dy: 0 }
    }
}

impl Ip {
    pub fn advance(&mut self) {
        self.x += self.dx;
        self.y += self.dy;
    }

    pub fn set_dir(&mut self, dx: i64, dy: i64) {
        self.dx = dx;
        self.dy = dy;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grid_missing_cell_is_space() {
        let g = Grid::new();
        assert_eq!(g.get(7, -3), BigInt::from(SPACE));
    }

    #[test]
    fn grid_put_and_get_roundtrip() {
        let mut g = Grid::new();
        g.put(2, 5, BigInt::from(b'@'));
        assert_eq!(g.get(2, 5), BigInt::from(b'@'));
    }

    #[test]
    fn grid_put_space_is_sparse() {
        let mut g = Grid::new();
        g.put(0, 0, BigInt::from(b'@'));
        g.put(0, 0, BigInt::from(SPACE));
        assert!(!g.cells.contains_key(&(0, 0)));
        assert_eq!(g.get(0, 0), BigInt::from(SPACE));
    }

    #[test]
    fn ip_advances_by_direction() {
        let mut ip = Ip::default();
        ip.advance();
        assert_eq!((ip.x, ip.y), (1, 0));
        ip.set_dir(0, 1);
        ip.advance();
        assert_eq!((ip.x, ip.y), (1, 1));
    }
}

#[cfg(all(test, feature = "metrics"))]
mod metrics_tests {
    use super::*;
    use crate::parser::parse;

    #[test]
    fn empty_grid_has_zero_metrics() {
        let g = Grid::new();
        assert_eq!(g.bounding_box(), (0, 0));
        assert_eq!(g.total_grid_cells(), 0);
        assert_eq!(g.effective_cells(), 0);
    }

    #[test]
    fn hello_wnd_bbox_and_effective_count() {
        // hello.wnd: a 29-cell single-line program that reads
        //   "!dlroW ,olleH",,,,,,,,,,,,,@
        // Effective cells:
        //   2 × `"`         StrMode  (open + close)
        //   1 × `!`         NOT      (decodes to Op::Not — coincidentally part of the literal)
        //   1 × `,` in str  PutChr   (still effective — string-mode is a runtime concern)
        //   13 × `,` out    PutChr
        //   1 × `@`         Halt
        // Letters d, l, r, o, W, o, l, l, e, H decode to Op::Unknown
        // and don't count. The single space inside the string literal
        // is the lone Op::Nop and also doesn't count.
        // Total = 2 + 1 + 1 + 13 + 1 = 18.
        let src = "\"!dlroW ,olleH\",,,,,,,,,,,,,@";
        let (g, _) = parse(src);
        let (w, h) = g.bounding_box();
        assert_eq!(h, 1);
        assert!(w >= 28 && w <= 29);
        assert_eq!(g.effective_cells(), 18);
    }

    #[test]
    fn density_distinguishes_dense_from_spam() {
        // Two cells worth of meaningful code ("@" plus a digit) inside a
        // fat 1×80 row with 78 spaces is the "huge grid + sprinkled hard
        // opcodes" failure mode the Phase 2 mining policy guards against.
        // We don't enforce the policy here — just expose enough numbers
        // that downstream code can compute the ratio.
        let mut spam_src = String::new();
        spam_src.push('1');
        for _ in 0..78 { spam_src.push(' '); }
        spam_src.push('@');
        let (g, _) = parse(&spam_src);
        // total_grid_cells counts the bounding box; spaces are
        // sparse-removed by the parser, but the bbox still spans the
        // populated cells at columns 0 and 79.
        let total = g.total_grid_cells();
        let effective = g.effective_cells();
        assert_eq!(effective, 2); // '1' + '@'
        assert_eq!(total, 80);     // bounding box width
        // Density 2/80 = 0.025 — fails Phase 2's 0.20 cutoff. Verifying
        // the *ratio* is beyond this crate's scope; we just commit to
        // exposing both numbers.
        assert!(effective * 5 < total);
    }
}
