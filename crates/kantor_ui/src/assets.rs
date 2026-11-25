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
        // Slides toolbar: speaker notes
        NotepadText,
        // Paparan AI chat
        Paperclip,
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

#[cfg(test)]
mod tests {
    use std::path::Path;

    use gpui_kit::assets::IconName;

    use super::*;

    fn rust_files(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if path.file_name().is_some_and(|n| n != "target") {
                    rust_files(&path, out);
                }
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push(path);
            }
        }
    }

    /// Icon names written as `Lucide::Name` or `IconName::Name` in a file.
    fn icon_names(text: &str) -> Vec<String> {
        let mut names = Vec::new();
        for prefix in ["Lucide::", "IconName::"] {
            for (at, _) in text.match_indices(prefix) {
                let before = text[..at].chars().next_back();
                if before.is_some_and(|c| c.is_alphanumeric() || c == '_') {
                    continue;
                }
                let name: String = text[at + prefix.len()..]
                    .chars()
                    .take_while(|c| c.is_ascii_alphanumeric())
                    .collect();
                if name.starts_with(|c: char| c.is_ascii_uppercase()) {
                    names.push(name);
                }
            }
        }
        names
    }

    /// Every Lucide icon the workspace crates name must load from
    /// `KantorAssets`: icons outside GPUI Kit's default bundle paint nothing
    /// unless they are listed in `icon_assets!` above.
    #[test]
    fn every_used_icon_is_embedded() {
        let crates = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let mut files = Vec::new();
        rust_files(crates, &mut files);
        assert!(files.len() > 50, "found only {} source files", files.len());
        let mut missing = Vec::new();
        let mut checked = 0;
        for file in files {
            let text = std::fs::read_to_string(&file).unwrap_or_default();
            for name in icon_names(&text) {
                let Some(icon) = IconName::ALL
                    .iter()
                    .find(|icon| format!("{icon:?}") == name)
                else {
                    continue;
                };
                checked += 1;
                if !matches!(KantorAssets.load(&icon.path()), Ok(Some(_))) {
                    missing.push(format!("{name} ({})", file.display()));
                }
            }
        }
        assert!(checked > 100, "checked only {checked} icon uses");
        missing.sort();
        missing.dedup();
        assert!(
            missing.is_empty(),
            "icons not in KantorAssets (add them to icon_assets! in kantor_ui/src/assets.rs): {missing:#?}"
        );
    }

    #[test]
    fn toolbar_icons_load() {
        for icon in [
            IconName::NotepadText,
            IconName::Presentation,
            IconName::Bold,
            IconName::Search,
        ] {
            assert!(
                matches!(KantorAssets.load(&icon.path()), Ok(Some(_))),
                "{icon:?}"
            );
        }
    }
}
