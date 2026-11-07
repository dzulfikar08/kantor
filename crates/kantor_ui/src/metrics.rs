//! Sizes. Values follow Voltrus Tools' `vt-ui` metrics.

use gpui_kit::{Pixels, px};

// Type scale
pub const FONT_CAPTION: Pixels = px(10.5);
pub const FONT_SMALL: Pixels = px(11.5);
pub const FONT_BODY: Pixels = px(12.5);
pub const FONT_EMPHASIS: Pixels = px(13.);
pub const FONT_TITLE: Pixels = px(15.);
pub const FONT_DISPLAY: Pixels = px(22.);

/// Root rem size; GPUI Kit components scale from it (rem 14 = compact, desktop-like).
pub const REM: Pixels = px(14.);

// Spacing scale
pub const SPACE_1: Pixels = px(2.);
pub const SPACE_2: Pixels = px(4.);
pub const SPACE_3: Pixels = px(6.);
pub const SPACE_4: Pixels = px(8.);
pub const SPACE_5: Pixels = px(10.);
pub const SPACE_6: Pixels = px(12.);
pub const SPACE_7: Pixels = px(16.);
pub const SPACE_8: Pixels = px(20.);
pub const SPACE_9: Pixels = px(24.);
pub const SPACE_10: Pixels = px(32.);

// Radii
pub const RADIUS_SM: Pixels = px(4.);
pub const RADIUS_MD: Pixels = px(6.);
pub const RADIUS_LG: Pixels = px(8.);
pub const RADIUS_XL: Pixels = px(12.);

// Controls and icons
pub const CONTROL_SM: Pixels = px(22.);
pub const CONTROL_MD: Pixels = px(26.);
pub const CONTROL_LG: Pixels = px(32.);
pub const ICON_SM: Pixels = px(12.);
pub const ICON_MD: Pixels = px(14.);
pub const ICON_LG: Pixels = px(16.);

// Shell
pub const TITLE_BAR_HEIGHT: Pixels = px(38.);
pub const STATUS_BAR_HEIGHT: Pixels = px(26.);
pub const TAB_HEIGHT: Pixels = px(28.);
/// Text only truncates inside a known width, so tab titles get a fixed one.
pub const TAB_TITLE_WIDTH: Pixels = px(150.);
pub const ROW_HEIGHT: Pixels = px(28.);
pub const PALETTE_WIDTH: Pixels = px(600.);
pub const PALETTE_TOP: Pixels = px(96.);
pub const PALETTE_ROW_HEIGHT: Pixels = px(36.);
/// Theme rows show a name and a swatch strip.
pub const PALETTE_THEME_ROW_HEIGHT: Pixels = px(50.);
pub const PALETTE_FOOTER_HEIGHT: Pixels = px(34.);
pub const PALETTE_MAX_ROWS: usize = 10;

// Docs page view
/// Space around and between pages on the desk, in unzoomed page px.
pub const PAGE_GAP: f32 = 24.;
/// Width of the text caret on the page (screen px, not zoomed).
pub const CARET_WIDTH: Pixels = px(1.5);
/// Minimum / maximum page zoom and the step of Ctrl+= / Ctrl+-.
pub const ZOOM_MIN: f32 = 0.25;
pub const ZOOM_MAX: f32 = 4.0;
pub const ZOOM_STEP: f32 = 1.1;
