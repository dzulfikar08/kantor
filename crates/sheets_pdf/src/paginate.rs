//! Splitting a print range into pages, as Excel does: rows and columns are
//! laid onto pages greedily at the sheet's scale, a manual break starts a
//! new page, print titles are repeated on every page after the one they
//! appear on, and "fit to" picks the largest whole-percent scale (at least
//! 10 %) that keeps within the asked number of pages (ignoring manual
//! breaks, as Excel does). Pure maths: sizes in, bands out.

/// Slack for float comparisons, points.
const EPS: f64 = 0.01;

/// One axis (rows or columns) of a print range.
#[derive(Debug, Clone)]
pub struct Axis<'a> {
    /// Index of the first row / column of the range.
    pub first: u32,
    /// Size of each row / column of the range in points (0 when hidden).
    pub sizes: &'a [f64],
    /// Rows / columns repeated on every page, inclusive (absolute indices),
    /// and their total size.
    pub titles: Option<((u32, u32), f64)>,
    /// A new page starts at each of these (absolute indices).
    pub breaks: &'a [u32],
    /// Space taken on every page besides the cells (headings), points.
    pub fixed: f64,
}

impl Axis<'_> {
    /// Split into bands (inclusive absolute indices) that each fit in
    /// `avail` points. A row or column larger than a page gets a page of
    /// its own (clipped). Bands with nothing visible are left out.
    pub fn split(&self, avail: f64, use_breaks: bool) -> Vec<(u32, u32)> {
        let mut out = Vec::new();
        let n = self.sizes.len() as u32;
        if n == 0 {
            return out;
        }
        let mut breaks = self.breaks.iter().copied().peekable();
        let room = |start: u32| -> f64 {
            let titles = match self.titles {
                Some(((_, last), size)) if start > last => size,
                _ => 0.0,
            };
            avail - self.fixed - titles
        };
        let mut start = self.first;
        let mut used = 0.0;
        let mut visible = false;
        for i in 0..n {
            let at = self.first + i;
            let size = self.sizes[i as usize];
            let manual = use_breaks && {
                while breaks.peek().is_some_and(|&b| b < at) {
                    breaks.next();
                }
                breaks.peek() == Some(&at)
            };
            let full = visible && size > 0.0 && used + size > room(start) + EPS;
            if at > start && (manual || full) {
                if visible {
                    out.push((start, at - 1));
                }
                start = at;
                used = 0.0;
                visible = false;
            }
            used += size;
            visible |= size > 0.0;
        }
        if visible {
            out.push((start, self.first + n - 1));
        }
        out
    }
}

/// How a print range splits: column bands, row bands and the scale.
#[derive(Debug, Clone, PartialEq)]
pub struct Grid {
    pub cols: Vec<(u32, u32)>,
    pub rows: Vec<(u32, u32)>,
    /// 1.0 = 100 %.
    pub scale: f64,
}

/// Split a range onto pages whose printable area is `width` × `height`
/// points. `fit`: Excel's fit to `(pages wide, pages tall)`, 0 meaning no
/// limit; otherwise `scale_pct` applies.
pub fn grid(
    cols: &Axis,
    rows: &Axis,
    width: f64,
    height: f64,
    scale_pct: u32,
    fit: Option<(u32, u32)>,
) -> Grid {
    let at = |pct: u32, breaks: bool| {
        let k = f64::from(pct) / 100.0;
        Grid {
            cols: cols.split(width / k, breaks),
            rows: rows.split(height / k, breaks),
            scale: k,
        }
    };
    let Some((wide, tall)) = fit else {
        return at(scale_pct.clamp(10, 400), true);
    };
    let fits = |g: &Grid| {
        (wide == 0 || g.cols.len() <= wide as usize) && (tall == 0 || g.rows.len() <= tall as usize)
    };
    // Start from the scale that would fit the total size, then step down.
    let total = |a: &Axis, avail: f64, pages: u32| -> u32 {
        let sum: f64 = a.sizes.iter().sum::<f64>() + a.fixed;
        if pages == 0 || sum <= 0.0 {
            return 100;
        }
        ((avail * f64::from(pages) / sum) * 100.0)
            .floor()
            .clamp(10.0, 100.0) as u32
    };
    let start = total(cols, width, wide).min(total(rows, height, tall));
    let mut pct = start;
    loop {
        let g = at(pct, false);
        if fits(&g) || pct <= 10 {
            return g;
        }
        pct -= 1;
    }
}

/// Pages of a grid in printing order: `(column band, row band)` indices.
/// Excel's default prints down, then over.
pub fn order(grid: &Grid, over_then_down: bool) -> Vec<(usize, usize)> {
    let (nc, nr) = (grid.cols.len(), grid.rows.len());
    if over_then_down {
        (0..nr).flat_map(|r| (0..nc).map(move |c| (c, r))).collect()
    } else {
        (0..nc).flat_map(|c| (0..nr).map(move |r| (c, r))).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn axis<'a>(sizes: &'a [f64], breaks: &'a [u32]) -> Axis<'a> {
        Axis {
            first: 0,
            sizes,
            titles: None,
            breaks,
            fixed: 0.0,
        }
    }

    #[test]
    fn greedy_bands() {
        let sizes = [10.0; 10];
        assert_eq!(
            axis(&sizes, &[]).split(35.0, true),
            vec![(0, 2), (3, 5), (6, 8), (9, 9)]
        );
        // Exactly full pages.
        assert_eq!(axis(&sizes, &[]).split(50.0, true), vec![(0, 4), (5, 9)]);
        // A manual break starts a new page.
        assert_eq!(
            axis(&sizes, &[2, 7]).split(50.0, true),
            vec![(0, 1), (2, 6), (7, 9)]
        );
        assert_eq!(
            axis(&sizes, &[2, 7]).split(50.0, false),
            vec![(0, 4), (5, 9)]
        );
    }

    #[test]
    fn oversized_and_hidden() {
        // A column wider than the page gets its own page.
        let sizes = [10.0, 100.0, 10.0];
        assert_eq!(
            axis(&sizes, &[]).split(50.0, true),
            vec![(0, 0), (1, 1), (2, 2)]
        );
        // Hidden rows take no room; a band of hidden rows only is dropped.
        let sizes = [10.0, 0.0, 0.0, 10.0, 10.0];
        assert_eq!(axis(&sizes, &[]).split(20.0, true), vec![(0, 3), (4, 4)]);
        let sizes = [10.0, 0.0, 0.0];
        assert_eq!(axis(&sizes, &[1]).split(100.0, true), vec![(0, 0)]);
    }

    #[test]
    fn titles_repeat_after_their_page() {
        // Rows 0-1 repeat; the first page holds them already.
        let sizes = [10.0; 10];
        let a = Axis {
            titles: Some(((0, 1), 20.0)),
            ..axis(&sizes, &[])
        };
        assert_eq!(a.split(50.0, true), vec![(0, 4), (5, 7), (8, 9)]);
        // A print area below its titles repeats them on every page.
        let a = Axis {
            first: 10,
            titles: Some(((0, 1), 20.0)),
            ..axis(&sizes, &[])
        };
        assert_eq!(
            a.split(50.0, true),
            vec![(10, 12), (13, 15), (16, 18), (19, 19)]
        );
    }

    #[test]
    fn headings_take_room() {
        let sizes = [10.0; 4];
        let a = Axis {
            fixed: 15.0,
            ..axis(&sizes, &[])
        };
        assert_eq!(a.split(35.0, true), vec![(0, 1), (2, 3)]);
    }

    #[test]
    fn fit_to_width() {
        let cols = [100.0; 6]; // 600 pt wide
        let rows = [20.0; 100]; // 2000 pt tall
        let c = axis(&cols, &[3]);
        let r = axis(&rows, &[]);
        // 300 pt wide pages: one page wide needs 50 %; height free.
        let g = grid(&c, &r, 300.0, 500.0, 100, Some((1, 0)));
        assert_eq!(g.scale, 0.5);
        assert_eq!(g.cols, vec![(0, 5)]);
        assert_eq!(g.rows.len(), 2);
        // One page in both directions: the height decides (25 %).
        let g = grid(&c, &r, 300.0, 500.0, 100, Some((1, 1)));
        assert_eq!(g.scale, 0.25);
        assert_eq!((g.cols.len(), g.rows.len()), (1, 1));
        // Never more than 100 %.
        let small = [10.0; 2];
        let g = grid(
            &axis(&small, &[]),
            &axis(&small, &[]),
            300.0,
            500.0,
            100,
            Some((1, 1)),
        );
        assert_eq!(g.scale, 1.0);
        // Without fit: the scale applies and breaks count.
        let g = grid(&c, &r, 300.0, 500.0, 50, None);
        assert_eq!(g.cols, vec![(0, 2), (3, 5)]);
        assert_eq!(g.rows.len(), 2);
    }

    #[test]
    fn page_order() {
        let g = Grid {
            cols: vec![(0, 1), (2, 3)],
            rows: vec![(0, 9), (10, 19), (20, 29)],
            scale: 1.0,
        };
        assert_eq!(order(&g, false)[..3], [(0, 0), (0, 1), (0, 2)]);
        assert_eq!(order(&g, true)[..3], [(0, 0), (1, 0), (0, 1)]);
    }
}
