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
        // Slides toolbar
        Type,
        Circle,
        ImagePlus,
        Presentation,
        CopyPlus,
        SquarePlus,
        // Docs toolbar
        List,
        ListOrdered,
        ListIndentIncrease,
        ListIndentDecrease,
        Table,
        BetweenHorizontalEnd,
        BetweenVerticalEnd,
        Rows3,
        Columns3,
        Trash,
        // Find & Replace bar
        CaseSensitive,
        WholeWord,
        SquareFunction,
        ChevronUp,
        ChevronDown,
        Replace,
        ReplaceAll,
        // Sheets toolbar
        SpellCheck,
        PaintBucket,
        Baseline,
        AArrowUp,
        AArrowDown,
        Grid3x3,
        Square,
        SquareDashed,
        Shapes,
        TextWrap,
        TableCellsMerge,
        TableCellsSplit,
        Funnel,
        Snowflake,
        Printer,
        FileDown,
        // PivotTables
        TableProperties,
        ListFilter,
        Columns2,
        Rows2,
        Sigma,
        // Docs: headers and footers, review
        PanelTop,
        Hash,
        MessageSquare,
        MessageSquarePlus,
        FileDiff,
        CheckCheck,
        Reply,
        CircleCheck,
        // Slides: table and chart editing
        SquarePen,
        // Sheets: conditional formatting, data validation, charts
        Highlighter,
        ListChecks,
        ChartColumn,
        // AI
        Sparkles,
        X,
        // Cloud storage
        Cloud,
        CloudOff,
        CloudUpload,
        FileSpreadsheet,
        Lock,
        RotateCcw,
        Bookmark,
        MoveHorizontal,
        // PDF page tools
        LayoutGrid,
        Wrench,
        // PDF Comment and Fill & Sign
        StickyNote,
        PenLine,
        Signature,
        Stamp,
        MoveUpRight,
        Pentagon,
        MousePointer2,
        MessageSquareText,
        // Docs: Mailings
        Mail,
        ChevronsLeft,
        ChevronsRight,
        FilePlus,
        FileText,
        // PDF signature checker
        ShieldCheck,
        ShieldX,
        ShieldQuestionMark,
        // Slide show: pointer tools, presenter view
        Eraser,
        CircleDot,
        ArrowLeftRight,
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
