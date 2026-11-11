//! Where every grid row sits: per-row heights in whole device pixels and a
//! Fenwick tree of their prefix sums, so finding a row's top, the row under
//! a scroll offset or the total height costs O(log n), and changing one
//! row's height costs O(log n) too. Nothing here walks every row per frame;
//! the whole structure is rebuilt (O(n)) only when the sheet, the window's
//! scale or the workbook's row entries change.
//!
//! Heights are whole device pixels at the window's scale, so every row top
//! is a whole device pixel and neighbouring rows (and the slices of a merged
//! cell, see `look::merge_geometry`) meet without seams at 125 %, as column
//! widths do (`look::snap`). A hidden row is 0 pixels high.

/// The height of rows `0..len()` in device pixels, with prefix sums.
#[derive(Debug, Clone)]
pub(crate) struct RowHeights {
    heights: Vec<u32>,
    /// Fenwick tree (1-based) over `heights`.
    tree: Vec<u64>,
    /// Device pixels per logical pixel.
    scale: f32,
}

impl RowHeights {
    /// `len` rows of `default_dp` device pixels at `scale`.
    pub(crate) fn uniform(len: usize, default_dp: u32, scale: f32) -> Self {
        Self::from_heights(vec![default_dp; len], scale)
    }

    pub(crate) fn from_heights(heights: Vec<u32>, scale: f32) -> Self {
        let n = heights.len();
        let mut tree = vec![0u64; n + 1];
        // O(n) construction: each node adds itself to its parent.
        for i in 1..=n {
            tree[i] += u64::from(heights[i - 1]);
            let parent = i + i.isolate_lowest_one();
            if parent <= n {
                tree[parent] += tree[i];
            }
        }
        Self {
            heights,
            tree,
            scale: if scale > 0.0 { scale } else { 1.0 },
        }
    }

    pub(crate) fn len(&self) -> usize {
        self.heights.len()
    }

    pub(crate) fn scale(&self) -> f32 {
        self.scale
    }

    /// Height of `row` in device pixels (0 past the end).
    pub(crate) fn height_dp(&self, row: usize) -> u32 {
        self.heights.get(row).copied().unwrap_or(0)
    }

    /// Change one row's height; returns the change in device pixels.
    pub(crate) fn set_dp(&mut self, row: usize, dp: u32) -> i64 {
        let Some(old) = self.heights.get(row).copied() else {
            return 0;
        };
        if old == dp {
            return 0;
        }
        self.heights[row] = dp;
        let delta = i64::from(dp) - i64::from(old);
        let mut i = row + 1;
        while i < self.tree.len() {
            self.tree[i] = self.tree[i].wrapping_add_signed(delta);
            i += i.isolate_lowest_one();
        }
        delta
    }

    /// Top of `row` in device pixels: the sum of the rows above it
    /// (`row >= len()` gives the total).
    pub(crate) fn top_dp(&self, row: usize) -> u64 {
        let mut i = row.min(self.len());
        let mut sum = 0;
        while i > 0 {
            sum += self.tree[i];
            i &= i - 1;
        }
        sum
    }

    pub(crate) fn total_dp(&self) -> u64 {
        self.top_dp(self.len())
    }

    /// The row whose span contains device pixel `y` (rows of height 0 never
    /// do); the last row for `y` past the end. `None` for no rows.
    pub(crate) fn row_at_dp(&self, y: u64) -> Option<usize> {
        let n = self.len();
        if n == 0 {
            return None;
        }
        // Largest prefix `pos` with sum(0..pos) <= y, by binary lifting.
        let mut pos = 0;
        let mut rest = y;
        let mut step = 1usize << (usize::BITS - 1 - n.leading_zeros());
        while step > 0 {
            let next = pos + step;
            if next <= n && self.tree[next] <= rest {
                pos = next;
                rest -= self.tree[next];
            }
            step >>= 1;
        }
        // `pos` rows end at or above `y`, so row `pos` holds it.
        Some(pos.min(n - 1))
    }

    /// The first row at or after `row` with a height, if any.
    pub(crate) fn next_shown(&self, row: usize) -> Option<usize> {
        if row >= self.len() {
            return None;
        }
        if self.heights[row] > 0 {
            return Some(row);
        }
        // The row whose span starts at this row's top is the next shown one.
        let top = self.top_dp(row);
        if top >= self.total_dp() {
            return None;
        }
        self.row_at_dp(top)
    }

    /// The last row at or before `row` with a height, if any.
    pub(crate) fn prev_shown(&self, row: usize) -> Option<usize> {
        let row = row.min(self.len().checked_sub(1)?);
        if self.heights[row] > 0 {
            return Some(row);
        }
        let top = self.top_dp(row);
        if top == 0 {
            return None;
        }
        self.row_at_dp(top - 1)
    }

    /// The rows drawn in the device-pixel span `top..bottom`, hidden ones
    /// left out (a run of hidden rows costs one lookup, not one per row).
    pub(crate) fn shown_in(&self, top: u64, bottom: u64) -> Vec<usize> {
        let mut rows = Vec::new();
        let total = self.total_dp();
        let mut y = top;
        while y < bottom.min(total) {
            let Some(row) = self.row_at_dp(y) else { break };
            rows.push(row);
            y = self.top_dp(row) + u64::from(self.heights[row]);
        }
        rows
    }

    // Logical pixels, for layout.

    pub(crate) fn to_px(&self, dp: u64) -> f64 {
        dp as f64 / f64::from(self.scale)
    }

    pub(crate) fn to_dp(&self, px: f64) -> u64 {
        (px.max(0.0) * f64::from(self.scale)).round() as u64
    }

    pub(crate) fn top(&self, row: usize) -> f64 {
        self.to_px(self.top_dp(row))
    }

    pub(crate) fn height(&self, row: usize) -> f64 {
        self.to_px(u64::from(self.height_dp(row)))
    }

    #[cfg(test)]
    pub(crate) fn total(&self) -> f64 {
        self.to_px(self.total_dp())
    }

    pub(crate) fn row_at(&self, y: f64) -> Option<usize> {
        self.row_at_dp((y.max(0.0) * f64::from(self.scale)).floor() as u64)
    }
}

/// `px` logical pixels as whole device pixels at `scale`, rounded the way
/// `look::snap` rounds (half toward zero), at least 1.
pub(crate) fn device_px(px: f32, scale: f32) -> u32 {
    let scale = if scale > 0.0 { scale } else { 1.0 };
    let dp = px.max(0.0) * scale;
    ((dp - 0.5).ceil().max(1.0)) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn naive_top(h: &[u32], row: usize) -> u64 {
        h[..row.min(h.len())].iter().map(|&x| u64::from(x)).sum()
    }

    #[std::prelude::v1::test]
    fn prefix_sums_match_a_plain_sum() {
        // Mixed heights, hidden rows (0) and a long uniform tail.
        let mut h: Vec<u32> = (0..1000).map(|i| [37, 0, 60, 37, 12][i % 5]).collect();
        h.extend(std::iter::repeat_n(37, 5000));
        let rows = RowHeights::from_heights(h.clone(), 1.25);
        for row in [0, 1, 2, 3, 4, 5, 99, 500, 999, 1000, 3000, 5999, 6000, 9999] {
            assert_eq!(rows.top_dp(row), naive_top(&h, row), "row {row}");
        }
        assert_eq!(rows.total_dp(), naive_top(&h, h.len()));
    }

    #[std::prelude::v1::test]
    fn updates_keep_sums_right() {
        let mut h = vec![30u32; 777];
        let mut rows = RowHeights::from_heights(h.clone(), 1.0);
        for (row, dp) in [(0, 5), (776, 90), (300, 0), (301, 1), (300, 44), (17, 30)] {
            let old = h[row];
            assert_eq!(rows.set_dp(row, dp), i64::from(dp) - i64::from(old));
            h[row] = dp;
            for probe in [0, 1, 17, 18, 300, 301, 302, 776, 777] {
                assert_eq!(rows.top_dp(probe), naive_top(&h, probe));
            }
        }
        assert_eq!(rows.set_dp(5000, 1), 0, "past the end");
    }

    #[std::prelude::v1::test]
    fn hit_testing_finds_the_row_under_a_pixel() {
        // Rows 0..: 10, 0 (hidden), 0 (hidden), 20, 5.
        let rows = RowHeights::from_heights(vec![10, 0, 0, 20, 5], 1.0);
        assert_eq!(rows.row_at_dp(0), Some(0));
        assert_eq!(rows.row_at_dp(9), Some(0));
        // Hidden rows have no pixels: the next pixel is row 3's.
        assert_eq!(rows.row_at_dp(10), Some(3));
        assert_eq!(rows.row_at_dp(29), Some(3));
        assert_eq!(rows.row_at_dp(30), Some(4));
        assert_eq!(rows.row_at_dp(34), Some(4));
        assert_eq!(rows.row_at_dp(10_000), Some(4), "past the end: last row");
        assert_eq!(RowHeights::uniform(0, 30, 1.0).row_at_dp(0), None);
        // Every pixel of a big sheet against a linear scan.
        let h: Vec<u32> = (0..2000).map(|i| (i * 7 % 13) as u32).collect();
        let big = RowHeights::from_heights(h.clone(), 2.0);
        let total = big.total_dp();
        let mut row = 0;
        for y in 0..total {
            while naive_top(&h, row + 1) <= y {
                row += 1;
            }
            assert_eq!(big.row_at_dp(y), Some(row), "y {y}");
        }
    }

    #[std::prelude::v1::test]
    fn shown_rows_skip_hidden_ones() {
        let mut h = vec![10u32; 20];
        h[3..15].fill(0);
        let rows = RowHeights::from_heights(h, 1.0);
        assert_eq!(rows.shown_in(0, 45), vec![0, 1, 2, 15, 16]);
        assert_eq!(rows.next_shown(3), Some(15));
        assert_eq!(rows.next_shown(2), Some(2));
        assert_eq!(rows.prev_shown(14), Some(2));
        assert_eq!(rows.prev_shown(15), Some(15));
        let all_hidden = RowHeights::from_heights(vec![0, 0], 1.0);
        assert_eq!(all_hidden.next_shown(0), None);
        assert_eq!(all_hidden.prev_shown(1), None);
        assert!(all_hidden.shown_in(0, 100).is_empty());
    }

    #[std::prelude::v1::test]
    fn tops_are_whole_device_pixels_at_125() {
        let scale = 1.25;
        // 30 px rows are 37.5 device px: laid out as 37, like `look::snap`.
        assert_eq!(device_px(30.0, scale), 37);
        assert_eq!(device_px(36.0, scale), 45);
        assert_eq!(device_px(0.1, scale), 1, "never thinner than a pixel");
        let h: Vec<u32> = [30.0f32, 47.3, 12.2, 61.0]
            .iter()
            .cycle()
            .take(400)
            .map(|&px| device_px(px, scale))
            .collect();
        let rows = RowHeights::from_heights(h, scale);
        for row in 0..400 {
            let dp = rows.top(row) * f64::from(scale);
            assert!((dp - dp.round()).abs() < 1e-6, "row {row} top at {dp}");
            // Each row ends where the next begins.
            let end = rows.top(row) + rows.height(row);
            assert!((end - rows.top(row + 1)).abs() < 1e-9);
        }
        assert_eq!(rows.row_at(rows.top(123)), Some(123));
        assert_eq!(rows.row_at(rows.top(124) - 0.1), Some(123));
    }

    #[std::prelude::v1::test]
    fn a_million_rows_build_fast_and_answer_from_the_end() {
        let rows = RowHeights::uniform(1_048_576, 37, 1.25);
        assert_eq!(rows.total_dp(), 37 * 1_048_576);
        assert_eq!(rows.row_at_dp(rows.total_dp() - 1), Some(1_048_575));
        assert_eq!(rows.top_dp(1_000_000), 37_000_000);
    }
}
