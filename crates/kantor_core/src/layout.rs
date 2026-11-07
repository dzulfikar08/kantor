//! Split-pane layout: a binary tree of panes (ported from Voltrus Tools' `layout.rs`).
//!
//! Pure data, serde-serialisable, so layouts persist in the session file.

use serde::{Deserialize, Serialize};

pub type PaneId = u64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Axis {
    /// Side by side.
    Horizontal,
    /// Stacked.
    Vertical,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Branch {
    First,
    Second,
}

/// A path from the root to a node.
pub type Path = Vec<Branch>;

pub const MIN_RATIO: f32 = 0.15;
pub const MAX_RATIO: f32 = 0.85;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Layout {
    Leaf(PaneId),
    Split {
        axis: Axis,
        ratio: f32,
        first: Box<Layout>,
        second: Box<Layout>,
    },
}

impl Layout {
    /// Pane ids in reading order.
    pub fn panes(&self) -> Vec<PaneId> {
        match self {
            Self::Leaf(id) => vec![*id],
            Self::Split { first, second, .. } => {
                let mut v = first.panes();
                v.extend(second.panes());
                v
            }
        }
    }

    pub fn contains(&self, pane: PaneId) -> bool {
        self.panes().contains(&pane)
    }

    /// Split `target` along `axis`, putting `new` after it. Returns `None` if
    /// `target` isn't in the layout.
    pub fn split(&self, target: PaneId, new: PaneId, axis: Axis) -> Option<Self> {
        match self {
            Self::Leaf(id) if *id == target => Some(Self::Split {
                axis,
                ratio: 0.5,
                first: Box::new(Self::Leaf(target)),
                second: Box::new(Self::Leaf(new)),
            }),
            Self::Leaf(_) => None,
            Self::Split {
                axis: a,
                ratio,
                first,
                second,
            } => {
                if let Some(f) = first.split(target, new, axis) {
                    Some(Self::Split {
                        axis: *a,
                        ratio: *ratio,
                        first: Box::new(f),
                        second: second.clone(),
                    })
                } else {
                    second.split(target, new, axis).map(|s| Self::Split {
                        axis: *a,
                        ratio: *ratio,
                        first: first.clone(),
                        second: Box::new(s),
                    })
                }
            }
        }
    }

    /// The layout without `pane`; its sibling takes its place. `None` if the
    /// layout would become empty.
    pub fn without(&self, pane: PaneId) -> Option<Self> {
        match self {
            Self::Leaf(id) if *id == pane => None,
            Self::Leaf(_) => Some(self.clone()),
            Self::Split {
                axis,
                ratio,
                first,
                second,
            } => match (first.without(pane), second.without(pane)) {
                (None, None) => None,
                (Some(f), None) => Some(f),
                (None, Some(s)) => Some(s),
                (Some(f), Some(s)) => Some(Self::Split {
                    axis: *axis,
                    ratio: *ratio,
                    first: Box::new(f),
                    second: Box::new(s),
                }),
            },
        }
    }

    /// Set the ratio of the split at `path` (clamped to `MIN_RATIO..=MAX_RATIO`).
    pub fn set_ratio(&mut self, path: &[Branch], new_ratio: f32) {
        match (self, path.split_first()) {
            (Self::Split { ratio, .. }, None) => {
                *ratio = new_ratio.clamp(MIN_RATIO, MAX_RATIO);
            }
            (Self::Split { first, .. }, Some((Branch::First, rest))) => {
                first.set_ratio(rest, new_ratio)
            }
            (Self::Split { second, .. }, Some((Branch::Second, rest))) => {
                second.set_ratio(rest, new_ratio)
            }
            (Self::Leaf(_), _) => {}
        }
    }

    /// Replace pane ids (used when restoring a session).
    pub fn map_panes(&self, f: &mut impl FnMut(PaneId) -> Option<PaneId>) -> Option<Self> {
        match self {
            Self::Leaf(id) => f(*id).map(Self::Leaf),
            Self::Split {
                axis,
                ratio,
                first,
                second,
            } => match (first.map_panes(f), second.map_panes(f)) {
                (None, None) => None,
                (Some(x), None) | (None, Some(x)) => Some(x),
                (Some(a), Some(b)) => Some(Self::Split {
                    axis: *axis,
                    ratio: *ratio,
                    first: Box::new(a),
                    second: Box::new(b),
                }),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_and_remove() {
        let l = Layout::Leaf(1);
        let l = l.split(1, 2, Axis::Horizontal).unwrap();
        let l = l.split(2, 3, Axis::Vertical).unwrap();
        assert_eq!(l.panes(), vec![1, 2, 3]);
        assert!(l.split(9, 4, Axis::Vertical).is_none());

        let l2 = l.without(2).unwrap();
        assert_eq!(l2.panes(), vec![1, 3]);
        assert_eq!(Layout::Leaf(1).without(1), None);
    }

    #[test]
    fn ratio_is_clamped_and_addressed_by_path() {
        let mut l = Layout::Leaf(1)
            .split(1, 2, Axis::Horizontal)
            .unwrap()
            .split(2, 3, Axis::Vertical)
            .unwrap();
        l.set_ratio(&[], 0.99);
        l.set_ratio(&[Branch::Second], 0.3);
        match &l {
            Layout::Split { ratio, second, .. } => {
                assert_eq!(*ratio, MAX_RATIO);
                match second.as_ref() {
                    Layout::Split { ratio, .. } => assert!((ratio - 0.3).abs() < 1e-6),
                    _ => panic!("expected split"),
                }
            }
            _ => panic!("expected split"),
        }
    }

    #[test]
    fn map_panes_drops_missing() {
        let l = Layout::Leaf(1).split(1, 2, Axis::Horizontal).unwrap();
        let mapped = l.map_panes(&mut |id| (id == 2).then_some(20)).unwrap();
        assert_eq!(mapped, Layout::Leaf(20));
        let json = serde_json::to_string(&l).unwrap();
        assert_eq!(serde_json::from_str::<Layout>(&json).unwrap(), l);
    }
}
