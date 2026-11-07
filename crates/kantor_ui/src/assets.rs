//! The app's asset source: GPUI Kit's default component icons plus the few
//! extra Lucide icons Kantor uses (embedded one by one, not the whole catalog).

use std::borrow::Cow;

use gpui_kit::assets::{Assets, icon_assets};
use gpui_kit::{AssetSource, Result, SharedString};

icon_assets!(
    ExtraIcons,
    [
        Bold,
        Italic,
        Underline,
        Strikethrough,
        TextAlignStart,
        TextAlignCenter,
        TextAlignEnd,
        TextAlignJustify,
        Heading1,
        Heading2,
        Heading3,
        Pilcrow,
        ZoomIn,
        ZoomOut,
        List,
        ListOrdered,
        ListIndentIncrease,
        ListIndentDecrease,
        Table,
        ImagePlus,
        BetweenHorizontalEnd,
        BetweenVerticalEnd,
        Rows3,
        Columns3,
        Trash
    ]
);

/// Register with `application().with_assets(KantorAssets)`. Extra icons are
/// named with `gpui_kit::assets::IconName` (e.g. `IconName::Bold`).
pub struct KantorAssets;

impl AssetSource for KantorAssets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        if let Some(bytes) = ExtraIcons.load(path)? {
            return Ok(Some(bytes));
        }
        Assets.load(path)
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut paths = Assets.list(path)?;
        paths.extend(ExtraIcons.list(path)?);
        paths.sort();
        paths.dedup();
        Ok(paths)
    }
}
