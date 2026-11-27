//! UserForm designers ([MS-OFORMS]): a form's caption, size, colours and
//! font, and its controls with their positions and properties, read from
//! the form's storage in `vbaProject.bin` (the storage named after the
//! form, next to `VBA`).
//!
//! A form's storage holds `f` (the form itself, a FormControl, followed by
//! a "site" per control: name, ID, position, tab index, type), `o` (the
//! controls' own data, one after another, each as long as its site says),
//! `\x03VBFrame` (the designer's text header: the client size in twips and
//! the start-up position) and a storage `i<ID>` per Frame or MultiPage (and
//! per Page of a MultiPage) with its own `f` and `o`.
//!
//! Each structure is a version, a size, a property mask and then the
//! properties the mask says are stored, in mask order: those of 4 bytes or
//! less first (each aligned to its size), then the larger ones (sizes,
//! positions, strings), then pictures and fonts. [`read_props`] reads any of
//! them from a table of (bit, name, kind), and [`write_props`] writes one
//! (for tests, see [`crate::synth`]). A property that doesn't decode keeps
//! its default; a control that doesn't decode is still listed, and a form
//! whose `f` stream is missing is an error only for that form.
//!
//! Positions and sizes are in points (the file's HIMETRIC × 72 / 2540),
//! relative to the parent's client area (the form, a Frame or a Page).
//! Colours are OLE_COLORs as stored (`0x80000005` is the system colour
//! COLOR_WINDOW); [`ole_rgb`] turns them into RGB.
//!
//! [MS-OFORMS]: https://learn.microsoft.com/en-us/openspecs/office_file_formats/ms-oforms/

use std::collections::HashMap;
use std::io::{Read, Seek};

use crate::{Error, Result};

/// Points per HIMETRIC unit (0.01 mm).
const PT_PER_HIMETRIC: f32 = 72.0 / 2540.0;

/// The type of a control.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ControlKind {
    Label,
    TextBox,
    CommandButton,
    CheckBox,
    OptionButton,
    ToggleButton,
    ComboBox,
    ListBox,
    Frame,
    MultiPage,
    /// A page of a MultiPage (its parent).
    Page,
    Image,
    SpinButton,
    ScrollBar,
    TabStrip,
    /// Another ActiveX control (its ClsidCacheIndex, or 0x7FFF for one in
    /// the class table): listed with its site, drawn as an empty box.
    Other(u16),
}

impl ControlKind {
    /// The type name VBA's `TypeName` gives.
    pub fn type_name(self) -> &'static str {
        match self {
            ControlKind::Label => "Label",
            ControlKind::TextBox => "TextBox",
            ControlKind::CommandButton => "CommandButton",
            ControlKind::CheckBox => "CheckBox",
            ControlKind::OptionButton => "OptionButton",
            ControlKind::ToggleButton => "ToggleButton",
            ControlKind::ComboBox => "ComboBox",
            ControlKind::ListBox => "ListBox",
            ControlKind::Frame => "Frame",
            ControlKind::MultiPage => "MultiPage",
            ControlKind::Page => "Page",
            ControlKind::Image => "Image",
            ControlKind::SpinButton => "SpinButton",
            ControlKind::ScrollBar => "ScrollBar",
            ControlKind::TabStrip => "TabStrip",
            ControlKind::Other(_) => "Control",
        }
    }

    /// The ClsidCacheIndex the file uses for it.
    pub fn clsid_index(self) -> u16 {
        match self {
            ControlKind::Page => 7,
            ControlKind::Image => 12,
            ControlKind::Frame => 14,
            ControlKind::SpinButton => 16,
            ControlKind::CommandButton => 17,
            ControlKind::TabStrip => 18,
            ControlKind::Label => 21,
            ControlKind::TextBox => 23,
            ControlKind::ListBox => 24,
            ControlKind::ComboBox => 25,
            ControlKind::CheckBox => 26,
            ControlKind::OptionButton => 27,
            ControlKind::ToggleButton => 28,
            ControlKind::ScrollBar => 47,
            ControlKind::MultiPage => 57,
            ControlKind::Other(n) => n,
        }
    }

    fn from_clsid(n: u16) -> ControlKind {
        match n {
            7 => ControlKind::Page,
            12 => ControlKind::Image,
            14 => ControlKind::Frame,
            16 => ControlKind::SpinButton,
            17 => ControlKind::CommandButton,
            18 => ControlKind::TabStrip,
            21 => ControlKind::Label,
            15 | 23 => ControlKind::TextBox,
            24 => ControlKind::ListBox,
            25 => ControlKind::ComboBox,
            26 => ControlKind::CheckBox,
            27 => ControlKind::OptionButton,
            28 => ControlKind::ToggleButton,
            47 => ControlKind::ScrollBar,
            57 => ControlKind::MultiPage,
            other => ControlKind::Other(other),
        }
    }

    /// Whether the control uses the MorphData layout.
    fn is_morph(self) -> bool {
        matches!(
            self,
            ControlKind::TextBox
                | ControlKind::ListBox
                | ControlKind::ComboBox
                | ControlKind::CheckBox
                | ControlKind::OptionButton
                | ControlKind::ToggleButton
        )
    }

    /// Whether it has a storage of its own (`i<ID>`) holding children.
    pub fn is_container(self) -> bool {
        matches!(
            self,
            ControlKind::Frame | ControlKind::MultiPage | ControlKind::Page
        )
    }

    /// fmDisplayStyle of a MorphData control.
    fn display_style(self) -> u32 {
        match self {
            ControlKind::ListBox => 2,
            ControlKind::ComboBox => 3,
            ControlKind::CheckBox => 4,
            ControlKind::OptionButton => 5,
            ControlKind::ToggleButton => 6,
            _ => 1,
        }
    }

    /// The file format default BackColor.
    fn default_back(self) -> u32 {
        match self {
            ControlKind::TextBox
            | ControlKind::ListBox
            | ControlKind::ComboBox
            | ControlKind::CheckBox
            | ControlKind::OptionButton
            | ControlKind::ToggleButton => 0x8000_0005,
            _ => 0x8000_000F,
        }
    }

    /// The file format default ForeColor.
    fn default_fore(self) -> u32 {
        match self {
            ControlKind::TextBox | ControlKind::ListBox | ControlKind::ComboBox => 0x8000_0008,
            _ => 0x8000_0012,
        }
    }

    fn default_various(self) -> u32 {
        if self.is_morph() {
            0x2C80_081B
        } else if self == ControlKind::Label {
            0x0080_001B
        } else {
            0x0000_001B
        }
    }
}

/// A font.
#[derive(Clone, Debug, PartialEq)]
pub struct Font {
    pub name: String,
    /// Points.
    pub size: f32,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strikeout: bool,
    pub charset: u8,
}

impl Default for Font {
    /// Tahoma 8, what the Visual Basic Editor gives new forms.
    fn default() -> Font {
        Font {
            name: "Tahoma".into(),
            size: 8.0,
            bold: false,
            italic: false,
            underline: false,
            strikeout: false,
            charset: 0,
        }
    }
}

/// What a picture's bytes are.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PictureFormat {
    Bmp,
    Gif,
    Jpeg,
    Png,
    Wmf,
    Emf,
    Icon,
    Unknown,
}

/// A picture (an Image control's, a form's), as stored. A device-
/// independent bitmap without a file header gets one, so the bytes are a
/// `.bmp` file.
#[derive(Clone, Debug, PartialEq)]
pub struct Picture {
    pub format: PictureFormat,
    pub bytes: Vec<u8>,
}

impl Picture {
    pub fn new(bytes: Vec<u8>) -> Picture {
        let b = &bytes;
        let format = if b.starts_with(b"BM") {
            PictureFormat::Bmp
        } else if b.starts_with(b"GIF8") {
            PictureFormat::Gif
        } else if b.starts_with(&[0xFF, 0xD8]) {
            PictureFormat::Jpeg
        } else if b.starts_with(&[0x89, b'P', b'N', b'G']) {
            PictureFormat::Png
        } else if b.starts_with(&[0xD7, 0xCD, 0xC6, 0x9A])
            || b.starts_with(&[1, 0, 9, 0])
            || b.starts_with(&[2, 0, 9, 0])
        {
            PictureFormat::Wmf
        } else if b.get(40..44) == Some(b" EMF") {
            PictureFormat::Emf
        } else if b.starts_with(&[0, 0, 1, 0]) {
            PictureFormat::Icon
        } else if b.len() > 40 && u32_le(b, 0) == Some(40) {
            // A bare DIB: add BITMAPFILEHEADER.
            let bits = u16::from_le_bytes([b[14], b[15]]);
            let used = u32_le(b, 32).unwrap_or(0) as usize;
            let colours = if used > 0 {
                used
            } else if bits <= 8 {
                1usize << bits
            } else {
                0
            };
            let off = 14 + 40 + colours * 4;
            let mut out = Vec::with_capacity(b.len() + 14);
            out.extend_from_slice(b"BM");
            out.extend_from_slice(&((b.len() + 14) as u32).to_le_bytes());
            out.extend_from_slice(&[0, 0, 0, 0]);
            out.extend_from_slice(&(off as u32).to_le_bytes());
            out.extend_from_slice(b);
            return Picture {
                format: PictureFormat::Bmp,
                bytes: out,
            };
        } else {
            PictureFormat::Unknown
        };
        Picture { format, bytes }
    }
}

/// A control on a form.
#[derive(Clone, Debug, PartialEq)]
pub struct Control {
    pub name: String,
    pub kind: ControlKind,
    /// The site's ID (unique in its parent).
    pub id: i32,
    /// The index (in [`FormDesign::controls`]) of the Frame, MultiPage or
    /// Page holding it; `None` on the form itself.
    pub parent: Option<usize>,
    /// Points, relative to the parent's client area.
    pub left: f32,
    pub top: f32,
    pub width: f32,
    pub height: f32,
    pub tab_index: i16,
    pub tab_stop: bool,
    pub visible: bool,
    pub enabled: bool,
    pub locked: bool,
    /// A CommandButton clicked by Enter.
    pub default: bool,
    /// A CommandButton clicked by Esc.
    pub cancel: bool,
    pub caption: String,
    /// The Value property as stored (a TextBox's text, "1"/"0" for a
    /// CheckBox, a ComboBox's text).
    pub value: String,
    pub tag: String,
    pub tip: String,
    /// A cell linked to the value (`Sheet1!A1`).
    pub control_source: String,
    /// The cells listed by a ListBox or ComboBox.
    pub row_source: String,
    /// OptionButtons with the same group name are exclusive (empty: those
    /// with the same parent).
    pub group_name: String,
    pub font: Font,
    pub fore_color: u32,
    pub back_color: u32,
    /// BackStyle: opaque (true) or transparent.
    pub back_opaque: bool,
    pub border_style: u8,
    pub border_color: u32,
    pub special_effect: u8,
    /// 0: no limit.
    pub max_length: u32,
    pub multi_line: bool,
    pub word_wrap: bool,
    pub auto_size: bool,
    pub password_char: Option<char>,
    pub accelerator: Option<char>,
    /// 1 left, 2 right, 3 centre.
    pub text_align: u8,
    /// A CheckBox or OptionButton with its caption on the left.
    pub caption_left: bool,
    pub column_count: i32,
    /// 0: the row number; n: column n is the Value.
    pub bound_column: i32,
    pub text_column: i32,
    pub column_heads: bool,
    pub list_rows: i32,
    /// fmListStyle: 0 plain, 1 option buttons / check boxes.
    pub list_style: u8,
    /// fmMultiSelect: 0 single, 1 multi, 2 extended.
    pub multi_select: u8,
    /// A ComboBox whose text can't be typed (fmStyleDropDownList).
    pub drop_list: bool,
    pub match_entry: u8,
    pub min: i32,
    pub max: i32,
    /// A ScrollBar's or SpinButton's Value.
    pub position: i32,
    pub small_change: i32,
    pub large_change: i32,
    /// -1 automatic, 0 vertical, 1 horizontal.
    pub orientation: i32,
    pub picture: Option<Picture>,
    /// fmPictureSizeMode: 0 clip, 1 stretch, 3 zoom.
    pub picture_size_mode: u8,
    /// A TabStrip's tab captions, or a MultiPage's page captions.
    pub tabs: Vec<String>,
    /// A TabStrip's or MultiPage's selected tab (-1: none).
    pub tab_index_selected: i32,
}

impl Control {
    /// A control of `kind` with the file format defaults.
    pub fn new(name: &str, kind: ControlKind) -> Control {
        let various = kind.default_various();
        let mut c = Control {
            name: name.to_owned(),
            kind,
            id: 0,
            parent: None,
            left: 0.0,
            top: 0.0,
            width: 72.0,
            height: 18.0,
            tab_index: -1,
            tab_stop: true,
            visible: true,
            enabled: true,
            locked: false,
            default: false,
            cancel: false,
            caption: String::new(),
            value: String::new(),
            tag: String::new(),
            tip: String::new(),
            control_source: String::new(),
            row_source: String::new(),
            group_name: String::new(),
            font: Font::default(),
            fore_color: kind.default_fore(),
            back_color: kind.default_back(),
            back_opaque: true,
            border_style: u8::from(kind == ControlKind::Image),
            border_color: 0x8000_0006,
            special_effect: match kind {
                ControlKind::CheckBox
                | ControlKind::ComboBox
                | ControlKind::ListBox
                | ControlKind::OptionButton
                | ControlKind::TextBox
                | ControlKind::ToggleButton => 2,
                _ => 0,
            },
            max_length: 0,
            multi_line: false,
            word_wrap: false,
            auto_size: false,
            password_char: None,
            accelerator: None,
            text_align: 1,
            caption_left: false,
            column_count: 1,
            bound_column: 1,
            text_column: -1,
            column_heads: false,
            list_rows: 8,
            list_style: 0,
            multi_select: 0,
            drop_list: false,
            match_entry: if kind == ControlKind::ListBox { 0 } else { 1 },
            min: 0,
            max: if kind == ControlKind::SpinButton {
                100
            } else {
                32767
            },
            position: 0,
            small_change: 1,
            large_change: 1,
            orientation: -1,
            picture: None,
            picture_size_mode: 0,
            tabs: Vec::new(),
            tab_index_selected: -1,
        };
        c.apply_various(various);
        if kind == ControlKind::Frame {
            c.caption = name.to_owned();
        }
        c
    }

    fn apply_various(&mut self, v: u32) {
        self.enabled = v & (1 << 1) != 0;
        self.locked = v & (1 << 2) != 0;
        self.back_opaque = v & (1 << 3) != 0;
        self.column_heads = v & (1 << 10) != 0;
        self.caption_left = v & (1 << 13) != 0;
        self.word_wrap = v & (1 << 23) != 0;
        self.auto_size = v & (1 << 28) != 0;
        self.multi_line = v & (1 << 31) != 0;
    }

    /// VariousPropertyBits for the control's flags.
    fn various(&self) -> u32 {
        let mut v = self.kind.default_various() & !((1 << 1) | (1 << 3) | (1 << 23));
        let mut set = |bit: u32, on: bool| {
            if on {
                v |= 1 << bit;
            } else {
                v &= !(1 << bit);
            }
        };
        set(1, self.enabled);
        set(2, self.locked);
        set(3, self.back_opaque);
        set(10, self.column_heads);
        set(13, self.caption_left);
        set(23, self.word_wrap);
        set(28, self.auto_size);
        set(31, self.multi_line);
        v
    }
}

/// A UserForm's design.
#[derive(Clone, Debug, PartialEq)]
pub struct FormDesign {
    /// The form's name (its module).
    pub name: String,
    pub caption: String,
    /// The client area, in points.
    pub width: f32,
    pub height: f32,
    pub back_color: u32,
    pub fore_color: u32,
    pub border_color: u32,
    pub border_style: u8,
    pub special_effect: u8,
    pub enabled: bool,
    pub font: Font,
    pub picture: Option<Picture>,
    /// 0 manual, 1 centre on the owner (the default), 2 centre on the
    /// screen, 3 Windows default.
    pub start_up_position: i32,
    /// Every control, parents before their children.
    pub controls: Vec<Control>,
}

impl FormDesign {
    /// An empty form as the Visual Basic Editor makes one (240 × 180 pt).
    pub fn new(name: &str) -> FormDesign {
        FormDesign {
            name: name.to_owned(),
            caption: name.to_owned(),
            width: 240.0,
            height: 180.0,
            back_color: 0x8000_000F,
            fore_color: 0x8000_0012,
            border_color: 0x8000_0012,
            border_style: 0,
            special_effect: 0,
            enabled: true,
            font: Font::default(),
            picture: None,
            start_up_position: 1,
            controls: Vec::new(),
        }
    }

    /// The control called `name` (any case).
    pub fn control(&self, name: &str) -> Option<&Control> {
        self.controls
            .iter()
            .find(|c| c.name.eq_ignore_ascii_case(name))
    }

    /// The indices of the controls whose parent is `parent`.
    pub fn children(&self, parent: Option<usize>) -> Vec<usize> {
        (0..self.controls.len())
            .filter(|&i| self.controls[i].parent == parent)
            .collect()
    }
}

/// An OLE_COLOR as `0xRRGGBB`; system colours as Windows 10 draws them.
pub fn ole_rgb(c: u32) -> u32 {
    if c & 0x8000_0000 != 0 {
        return match c & 0xFF {
            0 => 0xC8C8C8,                   // scroll bar
            1 => 0x000000,                   // desktop
            2 => 0x99B4D1,                   // active caption
            3 => 0xBFCDDB,                   // inactive caption
            4 => 0xF0F0F0,                   // menu
            5 => 0xFFFFFF,                   // window
            6 => 0x646464,                   // window frame
            7 | 8 | 9 | 18 | 23 => 0x000000, // texts
            10 => 0xB4B4B4,
            11 => 0xF4F7FC,
            12 => 0xABABAB,
            13 => 0x0078D7,           // highlight
            14 => 0xFFFFFF,           // highlight text
            15 | 22 | 25 => 0xF0F0F0, // button face
            16 => 0xA0A0A0,           // button shadow
            17 => 0x6D6D6D,           // grey text
            19 => 0x000000,
            20 => 0xFFFFFF, // button highlight
            21 => 0x696969, // dark shadow
            24 => 0xFFFFE1, // tooltip back
            _ => 0xF0F0F0,
        };
    }
    let r = c & 0xFF;
    let g = (c >> 8) & 0xFF;
    let b = (c >> 16) & 0xFF;
    (r << 16) | (g << 8) | b
}

/// RGB (`0xRRGGBB`) as an OLE_COLOR.
pub fn rgb_ole(rgb: u32) -> u32 {
    let r = (rgb >> 16) & 0xFF;
    let g = (rgb >> 8) & 0xFF;
    let b = rgb & 0xFF;
    r | (g << 8) | (b << 16)
}

// ---- Property tables ----

/// How a property is stored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum K {
    U8,
    U16,
    U32,
    /// A CountOfBytesWithCompressionFlag in the DataBlock, the fmString in
    /// the ExtraDataBlock.
    Str,
    /// 8 bytes (fmSize, fmPosition) in the ExtraDataBlock.
    Pair,
    /// 0xFFFF in the DataBlock, a GuidAndPicture or GuidAndFont after the
    /// ExtraDataBlock.
    Stream,
    /// A size in the DataBlock, an array of ArrayStrings in the
    /// ExtraDataBlock.
    Array,
    /// No data: the bit is the value.
    Flag,
}

pub(crate) type Table = &'static [(u8, &'static str, K)];

const FORM: Table = &[
    (1, "BackColor", K::U32),
    (2, "ForeColor", K::U32),
    (3, "NextAvailableID", K::U32),
    (6, "BooleanProperties", K::U32),
    (7, "BorderStyle", K::U8),
    (8, "MousePointer", K::U8),
    (9, "ScrollBars", K::U8),
    (10, "DisplayedSize", K::Pair),
    (11, "LogicalSize", K::Pair),
    (12, "ScrollPosition", K::Pair),
    (13, "GroupCnt", K::U32),
    (15, "MouseIcon", K::Stream),
    (16, "Cycle", K::U8),
    (17, "SpecialEffect", K::U8),
    (18, "BorderColor", K::U32),
    (19, "Caption", K::Str),
    (20, "Font", K::Stream),
    (21, "Picture", K::Stream),
    (22, "Zoom", K::U32),
    (23, "PictureAlignment", K::U8),
    (24, "PictureTiling", K::Flag),
    (25, "PictureSizeMode", K::U8),
    (26, "ShapeCookie", K::U32),
    (27, "DrawBuffer", K::U32),
];

const SITE: Table = &[
    (0, "Name", K::Str),
    (1, "Tag", K::Str),
    (2, "ID", K::U32),
    (3, "HelpContextID", K::U32),
    (4, "BitFlags", K::U32),
    (5, "ObjectStreamSize", K::U32),
    (6, "TabIndex", K::U16),
    (7, "ClsidCacheIndex", K::U16),
    (8, "Position", K::Pair),
    (9, "GroupID", K::U16),
    (11, "ControlTipText", K::Str),
    (12, "RuntimeLicKey", K::Str),
    (13, "ControlSource", K::Str),
    (14, "RowSource", K::Str),
];

const COMMAND_BUTTON: Table = &[
    (0, "ForeColor", K::U32),
    (1, "BackColor", K::U32),
    (2, "VariousPropertyBits", K::U32),
    (3, "Caption", K::Str),
    (4, "PicturePosition", K::U32),
    (5, "Size", K::Pair),
    (6, "MousePointer", K::U8),
    (7, "Picture", K::Stream),
    (8, "Accelerator", K::U16),
    (9, "TakeFocusOnClick", K::Flag),
    (10, "MouseIcon", K::Stream),
];

const LABEL: Table = &[
    (0, "ForeColor", K::U32),
    (1, "BackColor", K::U32),
    (2, "VariousPropertyBits", K::U32),
    (3, "Caption", K::Str),
    (4, "PicturePosition", K::U32),
    (5, "Size", K::Pair),
    (6, "MousePointer", K::U8),
    (7, "BorderColor", K::U32),
    (8, "BorderStyle", K::U16),
    (9, "SpecialEffect", K::U16),
    (10, "Picture", K::Stream),
    (11, "Accelerator", K::U16),
    (12, "MouseIcon", K::Stream),
];

const MORPH: Table = &[
    (0, "VariousPropertyBits", K::U32),
    (1, "BackColor", K::U32),
    (2, "ForeColor", K::U32),
    (3, "MaxLength", K::U32),
    (4, "BorderStyle", K::U8),
    (5, "ScrollBars", K::U8),
    (6, "DisplayStyle", K::U8),
    (7, "MousePointer", K::U8),
    (8, "Size", K::Pair),
    (9, "PasswordChar", K::U16),
    (10, "ListWidth", K::U32),
    (11, "BoundColumn", K::U16),
    (12, "TextColumn", K::U16),
    (13, "ColumnCount", K::U16),
    (14, "ListRows", K::U16),
    (15, "cColumnInfo", K::U16),
    (16, "MatchEntry", K::U8),
    (17, "ListStyle", K::U8),
    (18, "ShowDropButtonWhen", K::U8),
    (20, "DropButtonStyle", K::U8),
    (21, "MultiSelect", K::U8),
    (22, "Value", K::Str),
    (23, "Caption", K::Str),
    (24, "PicturePosition", K::U32),
    (25, "BorderColor", K::U32),
    (26, "SpecialEffect", K::U32),
    (27, "MouseIcon", K::Stream),
    (28, "Picture", K::Stream),
    (29, "Accelerator", K::U16),
    (31, "Reserved", K::Flag),
    (32, "GroupName", K::Str),
];

const IMAGE: Table = &[
    (2, "AutoSize", K::Flag),
    (3, "BorderColor", K::U32),
    (4, "BackColor", K::U32),
    (5, "BorderStyle", K::U8),
    (6, "MousePointer", K::U8),
    (7, "PictureSizeMode", K::U8),
    (8, "SpecialEffect", K::U8),
    (9, "Size", K::Pair),
    (10, "Picture", K::Stream),
    (11, "PictureAlignment", K::U8),
    (12, "PictureTiling", K::Flag),
    (13, "VariousPropertyBits", K::U32),
    (14, "MouseIcon", K::Stream),
];

const SCROLL_BAR: Table = &[
    (0, "ForeColor", K::U32),
    (1, "BackColor", K::U32),
    (2, "VariousPropertyBits", K::U32),
    (3, "Size", K::Pair),
    (4, "MousePointer", K::U8),
    (5, "Min", K::U32),
    (6, "Max", K::U32),
    (7, "Position", K::U32),
    (9, "PrevEnabled", K::U32),
    (10, "NextEnabled", K::U32),
    (11, "SmallChange", K::U32),
    (12, "LargeChange", K::U32),
    (13, "Orientation", K::U32),
    (14, "ProportionalThumb", K::U16),
    (15, "Delay", K::U32),
    (16, "MouseIcon", K::Stream),
];

const SPIN_BUTTON: Table = &[
    (0, "ForeColor", K::U32),
    (1, "BackColor", K::U32),
    (2, "VariousPropertyBits", K::U32),
    (3, "Size", K::Pair),
    (5, "Min", K::U32),
    (6, "Max", K::U32),
    (7, "Position", K::U32),
    (8, "PrevEnabled", K::U32),
    (9, "NextEnabled", K::U32),
    (10, "SmallChange", K::U32),
    (11, "Orientation", K::U32),
    (12, "Delay", K::U32),
    (13, "MouseIcon", K::Stream),
    (14, "MousePointer", K::U8),
];

const TAB_STRIP: Table = &[
    (0, "ListIndex", K::U32),
    (1, "BackColor", K::U32),
    (2, "ForeColor", K::U32),
    (4, "Size", K::Pair),
    (5, "Items", K::Array),
    (6, "MousePointer", K::U8),
    (8, "TabOrientation", K::U32),
    (9, "TabStyle", K::U32),
    (10, "MultiRow", K::Flag),
    (11, "TabFixedWidth", K::U32),
    (12, "TabFixedHeight", K::U32),
    (13, "Tooltips", K::Flag),
    (15, "TipStrings", K::Array),
    (17, "Names", K::Array),
    (18, "VariousPropertyBits", K::U32),
    (19, "NewVersion", K::Flag),
    (20, "TabsAllocated", K::U32),
    (21, "Tags", K::Array),
    (22, "TabData", K::U32),
    (23, "Accelerators", K::Array),
    (24, "MouseIcon", K::Stream),
];

const TEXT_PROPS: Table = &[
    (0, "FontName", K::Str),
    (1, "FontEffects", K::U32),
    (2, "FontHeight", K::U32),
    (4, "FontCharSet", K::U8),
    (5, "FontPitchAndFamily", K::U8),
    (6, "ParagraphAlign", K::U8),
    (7, "FontWeight", K::U16),
];

/// GUIDs of GuidAndFont / GuidAndPicture, as stored.
const GUID_STD_FONT: [u8; 16] = [
    0x03, 0x52, 0xE3, 0x0B, 0x91, 0x8F, 0xCE, 0x11, 0x9D, 0xE3, 0x00, 0xAA, 0x00, 0x4B, 0xB8, 0x51,
];
const GUID_TEXT_PROPS: [u8; 16] = [
    0x20, 0x09, 0xC2, 0xAF, 0x4E, 0xDA, 0xCE, 0x11, 0xB9, 0x43, 0x00, 0xAA, 0x00, 0x68, 0x87, 0xB4,
];
const GUID_STD_PICTURE: [u8; 16] = [
    0x04, 0x52, 0xE3, 0x0B, 0x91, 0x8F, 0xCE, 0x11, 0x9D, 0xE3, 0x00, 0xAA, 0x00, 0x4B, 0xB8, 0x51,
];

/// A property value read from (or to write to) a stream.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum PVal {
    Int(u32),
    Str(String),
    Pair(i32, i32),
    Flag,
    Array(Vec<String>),
}

/// The properties of one structure.
#[derive(Clone, Debug, Default)]
pub(crate) struct Props {
    values: HashMap<&'static str, PVal>,
    /// The stream properties stored, in order (they follow the
    /// ExtraDataBlock).
    streams: Vec<&'static str>,
}

impl Props {
    fn int(&self, name: &str) -> Option<u32> {
        match self.values.get(name)? {
            PVal::Int(n) => Some(*n),
            _ => None,
        }
    }

    fn str(&self, name: &str) -> Option<&str> {
        match self.values.get(name)? {
            PVal::Str(s) => Some(s),
            _ => None,
        }
    }

    fn pair(&self, name: &str) -> Option<(i32, i32)> {
        match self.values.get(name)? {
            PVal::Pair(a, b) => Some((*a, *b)),
            _ => None,
        }
    }

    fn array(&self, name: &str) -> Option<&[String]> {
        match self.values.get(name)? {
            PVal::Array(a) => Some(a),
            _ => None,
        }
    }

    fn flag(&self, name: &str) -> bool {
        self.values.contains_key(name)
    }
}

fn u16_le(d: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(d.get(at..at + 2)?.try_into().ok()?))
}

fn u32_le(d: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(d.get(at..at + 4)?.try_into().ok()?))
}

fn align(pos: usize, base: usize, n: usize) -> usize {
    let rel = pos - base;
    base + rel.div_ceil(n) * n
}

/// An fmString: `count` bytes (compressed: one byte per character), at
/// `at`; returns the text and where the next property starts (padded to
/// 4 bytes relative to `base` when `pad`).
fn fm_string(d: &[u8], at: usize, count: u32) -> Option<String> {
    let cb = (count & 0x7FFF_FFFF) as usize;
    let compressed = count & 0x8000_0000 != 0;
    let raw = d.get(at..at + cb)?;
    Some(if compressed {
        raw.iter().map(|&b| char::from(b)).collect()
    } else {
        let units: Vec<u16> = raw
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        String::from_utf16_lossy(&units)
    })
}

/// Reads the properties `mask` says are stored, from the DataBlock at `at`
/// (alignment relative to `base`, the start of the structure). Returns the
/// properties and where the ExtraDataBlock ended. A property that runs past
/// the data ends the reading; what was read is kept.
pub(crate) fn read_props(
    d: &[u8],
    base: usize,
    at: usize,
    mask: u64,
    table: Table,
) -> (Props, usize) {
    let mut props = Props::default();
    let mut pos = at;
    let mut extra: Vec<(&'static str, K, u32)> = Vec::new();
    for &(bit, name, kind) in table {
        if mask & (1u64 << bit) == 0 {
            continue;
        }
        let size = match kind {
            K::U8 => 1,
            K::U16 | K::Stream => 2,
            K::U32 | K::Str | K::Array => 4,
            K::Pair => {
                extra.push((name, kind, 0));
                continue;
            }
            K::Flag => {
                props.values.insert(name, PVal::Flag);
                continue;
            }
        };
        pos = align(pos, base, size);
        let value = match size {
            1 => d.get(pos).map(|&b| u32::from(b)),
            2 => u16_le(d, pos).map(u32::from),
            _ => u32_le(d, pos),
        };
        let Some(value) = value else {
            return (props, pos);
        };
        pos += size;
        match kind {
            K::Str | K::Array => extra.push((name, kind, value)),
            K::Stream => props.streams.push(name),
            _ => {
                props.values.insert(name, PVal::Int(value));
            }
        }
    }
    pos = align(pos, base, 4);
    for (name, kind, count) in extra {
        match kind {
            K::Pair => {
                let (Some(a), Some(b)) = (u32_le(d, pos), u32_le(d, pos + 4)) else {
                    return (props, pos);
                };
                props.values.insert(name, PVal::Pair(a as i32, b as i32));
                pos += 8;
            }
            K::Str => {
                let Some(s) = fm_string(d, pos, count) else {
                    return (props, pos);
                };
                props.values.insert(name, PVal::Str(s));
                pos = align(pos + (count & 0x7FFF_FFFF) as usize, base, 4);
            }
            K::Array => {
                let end = pos + count as usize;
                let mut items = Vec::new();
                let mut p = pos;
                while p + 4 <= end {
                    let Some(cc) = u32_le(d, p) else { break };
                    let start = p;
                    let n = (cc & 0x7FFF_FFFF) as usize;
                    let compressed = cc & 0x8000_0000 != 0;
                    let bytes = if compressed { n } else { n * 2 };
                    let flag = if compressed { 0x8000_0000 } else { 0 };
                    let Some(s) = fm_string(d, p + 4, bytes as u32 | flag) else {
                        break;
                    };
                    items.push(s);
                    p = align(p + 4 + bytes, start, 4);
                }
                props.values.insert(name, PVal::Array(items));
                pos = end;
            }
            _ => {}
        }
    }
    (props, pos)
}

/// A StdFont at `at`; returns it and its end.
fn read_std_font(d: &[u8], at: usize) -> Option<(Font, usize)> {
    let charset = u16_le(d, at + 1)?;
    let flags = *d.get(at + 3)?;
    let weight = u16_le(d, at + 4)?;
    let height = u32_le(d, at + 6)?;
    let len = *d.get(at + 10)? as usize;
    let face = d.get(at + 11..at + 11 + len)?;
    Some((
        Font {
            name: face.iter().map(|&b| char::from(b)).collect(),
            size: height as f32 / 10_000.0,
            bold: weight >= 600 || flags & 1 != 0,
            italic: flags & 2 != 0,
            underline: flags & 4 != 0,
            strikeout: flags & 8 != 0,
            charset: charset as u8,
        },
        at + 11 + len,
    ))
}

/// A TextProps at `at`; returns the font and alignment and its end.
fn read_text_props(d: &[u8], at: usize) -> Option<(Font, u8, usize)> {
    let cb = u16_le(d, at + 2)? as usize;
    let mask = u32_le(d, at + 4)?;
    let (p, _) = read_props(d, at, at + 8, u64::from(mask), TEXT_PROPS);
    let effects = p.int("FontEffects").unwrap_or(0);
    let font = Font {
        name: p
            .str("FontName")
            .map(str::to_owned)
            .unwrap_or_else(|| "MS Sans Serif".into()),
        size: p.int("FontHeight").unwrap_or(160) as f32 / 20.0,
        bold: effects & 1 != 0 || p.int("FontWeight").is_some_and(|w| w >= 600),
        italic: effects & 2 != 0,
        underline: effects & 4 != 0,
        strikeout: effects & 8 != 0,
        charset: p.int("FontCharSet").unwrap_or(1) as u8,
    };
    let align = p.int("ParagraphAlign").unwrap_or(1) as u8;
    Some((font, align, at + 4 + cb))
}

/// A GuidAndPicture at `at`; returns the picture and its end.
fn read_picture(d: &[u8], at: usize) -> Option<(Picture, usize)> {
    let size = u32_le(d, at + 20)? as usize;
    let bytes = d.get(at + 24..at + 24 + size)?;
    Some((Picture::new(bytes.to_vec()), at + 24 + size))
}

/// The stream properties (pictures, a font) after the ExtraDataBlock.
struct StreamData {
    picture: Option<Picture>,
    font: Option<Font>,
    end: usize,
}

fn read_stream_data(d: &[u8], at: usize, names: &[&str]) -> StreamData {
    let mut out = StreamData {
        picture: None,
        font: None,
        end: at,
    };
    let mut pos = at;
    for &name in names {
        if name == "Font" {
            let Some(guid) = d.get(pos..pos + 16) else {
                break;
            };
            if guid == GUID_TEXT_PROPS {
                match read_text_props(d, pos + 16) {
                    Some((f, _, end)) => {
                        out.font = Some(f);
                        pos = end;
                    }
                    None => break,
                }
            } else {
                match read_std_font(d, pos + 16) {
                    Some((f, end)) => {
                        out.font = Some(f);
                        pos = end;
                    }
                    None => break,
                }
            }
        } else {
            match read_picture(d, pos) {
                Some((p, end)) => {
                    if name == "Picture" {
                        out.picture = Some(p);
                    }
                    pos = end;
                }
                None => break,
            }
        }
    }
    out.end = pos;
    out
}

fn pt(himetric: i32) -> f32 {
    (himetric as f32 * PT_PER_HIMETRIC * 100.0).round() / 100.0
}

fn himetric(points: f32) -> i32 {
    (points / PT_PER_HIMETRIC).round() as i32
}

fn char_of(n: u32) -> Option<char> {
    (n != 0).then(|| char::from_u32(n)).flatten()
}

/// One site of a FormControl.
struct Site {
    props: Props,
}

impl Site {
    fn id(&self) -> i32 {
        self.props.int("ID").unwrap_or(0) as i32
    }
    fn flags(&self) -> u32 {
        self.props.int("BitFlags").unwrap_or(0x33)
    }
    fn kind(&self) -> ControlKind {
        ControlKind::from_clsid(self.props.int("ClsidCacheIndex").unwrap_or(0x7FFF) as u16)
    }
    fn object_size(&self) -> usize {
        self.props.int("ObjectStreamSize").unwrap_or(0) as usize
    }
}

/// A FormControl (a form, Frame or Page) and its sites.
struct FormStream {
    props: Props,
    font: Option<Font>,
    picture: Option<Picture>,
    sites: Vec<Site>,
}

fn read_form_stream(f: &[u8]) -> Result<FormStream> {
    let cb = u16_le(f, 2).ok_or_else(|| Error::new("form stream too short"))? as usize;
    let mask = u32_le(f, 4).ok_or_else(|| Error::new("form stream too short"))?;
    let (props, _) = read_props(f, 0, 8, u64::from(mask), FORM);
    let stream = read_stream_data(f, 4 + cb, &props.streams);
    let mut out = FormStream {
        font: stream.font,
        picture: stream.picture,
        props,
        sites: Vec::new(),
    };
    let mut pos = stream.end;
    let flags = out.props.int("BooleanProperties").unwrap_or(4);
    if flags & 0x8000 == 0 {
        // The class table: entries for controls outside the cache.
        let Some(n) = u16_le(f, pos) else {
            return Ok(out);
        };
        pos += 2;
        for _ in 0..n {
            let Some(cb) = u16_le(f, pos + 2) else {
                return Ok(out);
            };
            pos += 4 + cb as usize;
        }
    }
    let (Some(count), Some(_bytes)) = (u32_le(f, pos), u32_le(f, pos + 4)) else {
        return Ok(out);
    };
    pos += 8;
    let depths_at = pos;
    let mut seen = 0u32;
    while seen < count {
        let Some(&tc) = f.get(pos + 1) else {
            return Ok(out);
        };
        if tc & 0x80 != 0 {
            seen += u32::from(tc & 0x7F).max(1);
            pos += 3;
        } else {
            seen += 1;
            pos += 2;
        }
    }
    pos = align(pos, depths_at, 4);
    for _ in 0..count {
        let (Some(cb), Some(mask)) = (u16_le(f, pos + 2), u32_le(f, pos + 4)) else {
            break;
        };
        let (props, _) = read_props(f, pos, pos + 8, u64::from(mask), SITE);
        out.sites.push(Site { props });
        pos += 4 + cb as usize;
    }
    Ok(out)
}

/// Fills `c` from a control's data in an `o` stream.
fn read_control_data(c: &mut Control, d: &[u8]) {
    if d.len() < 8 {
        return;
    }
    let cb = u16_le(d, 2).unwrap_or(0) as usize;
    let (table, mask, body) = if c.kind.is_morph() {
        let lo = u32_le(d, 4).unwrap_or(0);
        let hi = u32_le(d, 8).unwrap_or(0);
        (MORPH, u64::from(lo) | (u64::from(hi) << 32), 12)
    } else {
        let mask = u64::from(u32_le(d, 4).unwrap_or(0));
        let table = match c.kind {
            ControlKind::CommandButton => COMMAND_BUTTON,
            ControlKind::Label => LABEL,
            ControlKind::Image => IMAGE,
            ControlKind::ScrollBar => SCROLL_BAR,
            ControlKind::SpinButton => SPIN_BUTTON,
            ControlKind::TabStrip => TAB_STRIP,
            _ => return,
        };
        (table, mask, 8)
    };
    let (p, _) = read_props(d, 0, body, mask, table);
    if let Some(v) = p.int("VariousPropertyBits") {
        c.apply_various(v);
    }
    if let Some(v) = p.int("ForeColor") {
        c.fore_color = v;
    }
    if let Some(v) = p.int("BackColor") {
        c.back_color = v;
    }
    if let Some(v) = p.int("BorderColor") {
        c.border_color = v;
    }
    if let Some(v) = p.int("BorderStyle") {
        c.border_style = v as u8;
    }
    if let Some(v) = p.int("SpecialEffect") {
        c.special_effect = v as u8;
    }
    if let Some(s) = p.str("Caption") {
        c.caption = s.to_owned();
    }
    if let Some(s) = p.str("Value") {
        c.value = s.to_owned();
    }
    if let Some(s) = p.str("GroupName") {
        c.group_name = s.to_owned();
    }
    if let Some((w, h)) = p.pair("Size") {
        c.width = pt(w);
        c.height = pt(h);
    }
    if let Some(v) = p.int("Accelerator") {
        c.accelerator = char_of(v);
    }
    if let Some(v) = p.int("PasswordChar") {
        c.password_char = char_of(v);
    }
    if let Some(v) = p.int("MaxLength") {
        c.max_length = v;
    }
    if let Some(v) = p.int("ColumnCount") {
        c.column_count = i32::from(v as u16 as i16);
    }
    if let Some(v) = p.int("BoundColumn") {
        c.bound_column = i32::from(v as u16);
    }
    if let Some(v) = p.int("TextColumn") {
        c.text_column = i32::from(v as u16 as i16);
    }
    if let Some(v) = p.int("ListRows") {
        c.list_rows = v as i32;
    }
    if let Some(v) = p.int("ListStyle") {
        c.list_style = v as u8;
    }
    if let Some(v) = p.int("MultiSelect") {
        c.multi_select = v as u8;
    }
    if let Some(v) = p.int("MatchEntry") {
        c.match_entry = v as u8;
    }
    // A MorphData control is whatever its DisplayStyle says (the
    // ClsidCacheIndex 15 is any of them).
    match p.int("DisplayStyle") {
        Some(1) => c.kind = ControlKind::TextBox,
        Some(2) => c.kind = ControlKind::ListBox,
        Some(3) => c.kind = ControlKind::ComboBox,
        Some(4) => c.kind = ControlKind::CheckBox,
        Some(5) => c.kind = ControlKind::OptionButton,
        Some(6) => c.kind = ControlKind::ToggleButton,
        Some(7) => {
            c.kind = ControlKind::ComboBox;
            c.drop_list = true;
        }
        _ => {}
    }
    if let Some(v) = p.int("Min") {
        c.min = v as i32;
    }
    if let Some(v) = p.int("Max") {
        c.max = v as i32;
    }
    if let Some(v) = p.int("Position") {
        c.position = v as i32;
    }
    if let Some(v) = p.int("SmallChange") {
        c.small_change = v as i32;
    }
    if let Some(v) = p.int("LargeChange") {
        c.large_change = v as i32;
    }
    if let Some(v) = p.int("Orientation") {
        c.orientation = v as i32;
    }
    if let Some(v) = p.int("PictureSizeMode") {
        c.picture_size_mode = v as u8;
    }
    if p.flag("AutoSize") {
        c.auto_size = true;
    }
    if let Some(items) = p.array("Items") {
        c.tabs = items.to_vec();
    }
    if let Some(v) = p.int("ListIndex") {
        c.tab_index_selected = v as i32;
    }
    let stream = read_stream_data(d, 4 + cb, &p.streams);
    c.picture = stream.picture;
    if c.kind != ControlKind::Image
        && let Some((font, align, _)) = read_text_props(d, stream.end)
    {
        c.font = font;
        c.text_align = align;
    }
}

/// A form's streams and storages, read through a callback that returns a
/// stream's bytes by its path relative to the form's storage (`"f"`,
/// `"i05/o"`).
fn read_design(name: &str, stream: &mut dyn FnMut(&str) -> Option<Vec<u8>>) -> Result<FormDesign> {
    let f = stream("f").ok_or_else(|| Error::new(format!("{name}: the form has no f stream")))?;
    let top = read_form_stream(&f)?;
    let mut design = FormDesign::new(name);
    let p = &top.props;
    if let Some(s) = p.str("Caption") {
        design.caption = s.to_owned();
    }
    if let Some((w, h)) = p.pair("DisplayedSize") {
        design.width = pt(w);
        design.height = pt(h);
    }
    if let Some(v) = p.int("BackColor") {
        design.back_color = v;
    }
    if let Some(v) = p.int("ForeColor") {
        design.fore_color = v;
    }
    if let Some(v) = p.int("BorderColor") {
        design.border_color = v;
    }
    if let Some(v) = p.int("BorderStyle") {
        design.border_style = v as u8;
    }
    if let Some(v) = p.int("SpecialEffect") {
        design.special_effect = v as u8;
    }
    design.enabled = p.int("BooleanProperties").unwrap_or(4) & 4 != 0;
    if let Some(font) = top.font.clone() {
        design.font = font;
    }
    design.picture = top.picture.clone();
    if let Some(frame) = stream("\u{3}VBFrame") {
        let text = String::from_utf8_lossy(&frame);
        for line in text.lines() {
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let value = value.trim();
            let number = value
                .split(['\'', ' '])
                .next()
                .and_then(|v| v.parse::<f32>().ok());
            match key.trim() {
                "StartUpPosition" => {
                    if let Some(n) = number {
                        design.start_up_position = n as i32;
                    }
                }
                // Twips, when f has no DisplayedSize.
                "ClientWidth" if p.pair("DisplayedSize").is_none() => {
                    if let Some(n) = number {
                        design.width = n / 20.0;
                    }
                }
                "ClientHeight" if p.pair("DisplayedSize").is_none() => {
                    if let Some(n) = number {
                        design.height = n / 20.0;
                    }
                }
                _ => {}
            }
        }
    }
    let o = stream("o").unwrap_or_default();
    read_children(&mut design, None, "", &top, &o, stream, 0);
    Ok(design)
}

/// Adds the controls of the sites of `form` (its object stream `o`, its
/// storage `prefix`) under `parent`.
fn read_children(
    design: &mut FormDesign,
    parent: Option<usize>,
    prefix: &str,
    form: &FormStream,
    o: &[u8],
    stream: &mut dyn FnMut(&str) -> Option<Vec<u8>>,
    depth: usize,
) {
    let mut offset = 0usize;
    let font = parent
        .map(|i| design.controls[i].font.clone())
        .unwrap_or_else(|| design.font.clone());
    for site in &form.sites {
        let sp = &site.props;
        let flags = site.flags();
        let name = sp.str("Name").unwrap_or_default().to_owned();
        let mut c = Control::new(&name, site.kind());
        c.font = font.clone();
        c.id = site.id();
        c.parent = parent;
        c.tab_index = sp.int("TabIndex").map_or(-1, |v| v as u16 as i16);
        c.tab_stop = flags & 1 != 0;
        c.visible = flags & 2 != 0;
        c.default = flags & 4 != 0;
        c.cancel = flags & 8 != 0;
        if let Some((x, y)) = sp.pair("Position") {
            c.left = pt(x);
            c.top = pt(y);
        }
        c.tag = sp.str("Tag").unwrap_or_default().to_owned();
        c.tip = sp.str("ControlTipText").unwrap_or_default().to_owned();
        c.control_source = sp.str("ControlSource").unwrap_or_default().to_owned();
        c.row_source = sp.str("RowSource").unwrap_or_default().to_owned();
        let streamed = flags & 0x10 != 0;
        let size = site.object_size();
        if streamed {
            if let Some(data) = o.get(offset..offset + size) {
                read_control_data(&mut c, data);
            }
            offset += size;
        }
        let index = design.controls.len();
        design.controls.push(c);
        if !streamed && site.kind().is_container() && depth < 8 {
            let storage = format!("{prefix}i{:02}/", site.id());
            read_container(design, index, &storage, stream, depth + 1);
        }
    }
}

/// A Frame's, MultiPage's or Page's own storage.
fn read_container(
    design: &mut FormDesign,
    index: usize,
    storage: &str,
    stream: &mut dyn FnMut(&str) -> Option<Vec<u8>>,
    depth: usize,
) {
    let Some(f) = stream(&format!("{storage}f")) else {
        return;
    };
    let Ok(form) = read_form_stream(&f) else {
        return;
    };
    let o = stream(&format!("{storage}o")).unwrap_or_default();
    {
        let c = &mut design.controls[index];
        let p = &form.props;
        if let Some(s) = p.str("Caption") {
            c.caption = s.to_owned();
        }
        if let Some((w, h)) = p.pair("DisplayedSize") {
            c.width = pt(w);
            c.height = pt(h);
        }
        if let Some(v) = p.int("BackColor") {
            c.back_color = v;
        }
        if let Some(v) = p.int("ForeColor") {
            c.fore_color = v;
        }
        if let Some(v) = p.int("BorderColor") {
            c.border_color = v;
        }
        if let Some(v) = p.int("BorderStyle") {
            c.border_style = v as u8;
        }
        if let Some(v) = p.int("SpecialEffect") {
            c.special_effect = v as u8;
        }
        c.enabled = p.int("BooleanProperties").unwrap_or(4) & 4 != 0;
        if let Some(font) = form.font.clone() {
            c.font = font;
        }
        if form.picture.is_some() {
            c.picture = form.picture.clone();
        }
    }
    if design.controls[index].kind == ControlKind::MultiPage {
        // `o` holds the TabStrip with the page captions; the sites are
        // the pages, each with a storage of its own.
        let mut strip = Control::new("", ControlKind::TabStrip);
        read_control_data(&mut strip, &o);
        let first_page = design.controls.len();
        read_children(design, Some(index), storage, &form, &[], stream, depth);
        let pages: Vec<usize> = (first_page..design.controls.len())
            .filter(|&i| design.controls[i].parent == Some(index))
            .collect();
        for (n, &i) in pages.iter().enumerate() {
            if let Some(caption) = strip.tabs.get(n) {
                design.controls[i].caption = caption.clone();
            }
        }
        let mp = &mut design.controls[index];
        mp.tabs = strip.tabs;
        mp.tab_index_selected = strip.tab_index_selected.max(0);
        mp.font = strip.font;
        return;
    }
    read_children(design, Some(index), storage, &form, &o, stream, depth);
}

/// The design of form `name` from a project's compound file; `root` is the
/// storage holding the project (`""`, or `/_VBA_PROJECT_CUR` in `.xls`).
pub(crate) fn read_from_cfb<F: Read + Seek>(
    cfb: &mut cfb::CompoundFile<F>,
    root: &str,
    name: &str,
) -> Result<FormDesign> {
    let base = format!("{root}/{name}");
    if !cfb.is_storage(&base) {
        return Err(Error::new(format!(
            "{name}: the form's designer storage is missing"
        )));
    }
    let mut read = |rel: &str| -> Option<Vec<u8>> {
        let path = format!("{base}/{}", rel.trim_end_matches('/'));
        let mut s = cfb.open_stream(&path).ok()?;
        let mut out = Vec::new();
        s.read_to_end(&mut out).ok()?;
        Some(out)
    };
    read_design(name, &mut read)
}

/// Reads a form design from a map of streams (paths relative to the form's
/// storage: `"f"`, `"o"`, `"i03/f"`), as a `.frx`-style test or a tool that
/// already took the storage apart has them.
pub fn read_streams(name: &str, streams: &HashMap<String, Vec<u8>>) -> Result<FormDesign> {
    let mut read = |rel: &str| streams.get(rel).cloned();
    read_design(name, &mut read)
}

// ---- Writing (tests, `synth`) ----

/// Writes the properties `values` (bit → value) of a structure with
/// `table`: version `(minor, major)`, size, mask (4 or 8 bytes), DataBlock
/// and ExtraDataBlock. Stream properties' data goes after it, written by
/// the caller.
pub(crate) fn write_props(
    version: (u8, u8),
    mask_bytes: usize,
    table: Table,
    values: &[(u8, PVal)],
) -> Vec<u8> {
    let mut mask = 0u64;
    for (bit, _) in values {
        mask |= 1u64 << bit;
    }
    let mut out = vec![version.0, version.1, 0, 0];
    out.extend_from_slice(&mask.to_le_bytes()[..mask_bytes]);
    let mut extra: Vec<u8> = Vec::new();
    let value = |bit: u8| values.iter().find(|(b, _)| *b == bit).map(|(_, v)| v);
    let pad_to = |out: &mut Vec<u8>, n: usize| {
        while out.len() % n != 0 {
            out.push(0);
        }
    };
    // ExtraDataBlock entries in mask order.
    let mut extras: Vec<&PVal> = Vec::new();
    for &(bit, _, kind) in table {
        let Some(v) = value(bit) else { continue };
        match (kind, v) {
            (K::U8, PVal::Int(n)) => out.push(*n as u8),
            (K::U16, PVal::Int(n)) => {
                pad_to(&mut out, 2);
                out.extend_from_slice(&(*n as u16).to_le_bytes());
            }
            (K::U32, PVal::Int(n)) => {
                pad_to(&mut out, 4);
                out.extend_from_slice(&n.to_le_bytes());
            }
            (K::Stream, _) => {
                pad_to(&mut out, 2);
                out.extend_from_slice(&0xFFFFu16.to_le_bytes());
            }
            (K::Str, PVal::Str(s)) => {
                pad_to(&mut out, 4);
                out.extend_from_slice(&string_count(s).to_le_bytes());
                extras.push(v);
            }
            (K::Array, PVal::Array(items)) => {
                pad_to(&mut out, 4);
                let size: usize = items.iter().map(|s| array_string(s).len()).sum();
                out.extend_from_slice(&(size as u32).to_le_bytes());
                extras.push(v);
            }
            (K::Pair, PVal::Pair(..)) => extras.push(v),
            _ => {}
        }
    }
    pad_to(&mut out, 4);
    for v in extras {
        match v {
            PVal::Pair(a, b) => {
                extra.extend_from_slice(&a.to_le_bytes());
                extra.extend_from_slice(&b.to_le_bytes());
            }
            PVal::Str(s) => {
                extra.extend_from_slice(&string_bytes(s));
                while extra.len() % 4 != 0 {
                    extra.push(0);
                }
            }
            PVal::Array(items) => {
                for s in items {
                    extra.extend_from_slice(&array_string(s));
                }
            }
            _ => {}
        }
    }
    out.extend_from_slice(&extra);
    let cb = (out.len() - 4) as u16;
    out[2..4].copy_from_slice(&cb.to_le_bytes());
    out
}

fn compressible(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| (c as u32) < 0x100)
}

fn string_bytes(s: &str) -> Vec<u8> {
    if compressible(s) {
        s.chars().map(|c| c as u32 as u8).collect()
    } else {
        s.encode_utf16().flat_map(u16::to_le_bytes).collect()
    }
}

fn string_count(s: &str) -> u32 {
    let n = string_bytes(s).len() as u32;
    if compressible(s) { n | 0x8000_0000 } else { n }
}

fn array_string(s: &str) -> Vec<u8> {
    let bytes = string_bytes(s);
    let chars = if compressible(s) {
        bytes.len() as u32 | 0x8000_0000
    } else {
        (bytes.len() / 2) as u32
    };
    let mut out = chars.to_le_bytes().to_vec();
    out.extend_from_slice(&bytes);
    while out.len() % 4 != 0 {
        out.push(0);
    }
    out
}

fn text_props(font: &Font, align: u8) -> Vec<u8> {
    let mut v = vec![(0, PVal::Str(font.name.clone()))];
    let effects = u32::from(font.bold)
        | (u32::from(font.italic) << 1)
        | (u32::from(font.underline) << 2)
        | (u32::from(font.strikeout) << 3);
    if effects != 0 {
        v.push((1, PVal::Int(effects)));
    }
    v.push((2, PVal::Int((font.size * 20.0).round() as u32)));
    v.push((4, PVal::Int(u32::from(font.charset))));
    if align != 1 {
        v.push((6, PVal::Int(u32::from(align))));
    }
    if font.bold {
        v.push((7, PVal::Int(700)));
    }
    write_props((0, 2), 4, TEXT_PROPS, &v)
}

fn std_font(font: &Font) -> Vec<u8> {
    let mut out = GUID_STD_FONT.to_vec();
    out.push(1);
    out.extend_from_slice(&u16::from(font.charset).to_le_bytes());
    let flags = u8::from(font.bold)
        | (u8::from(font.italic) << 1)
        | (u8::from(font.underline) << 2)
        | (u8::from(font.strikeout) << 3);
    out.push(flags);
    out.extend_from_slice(&(if font.bold { 700u16 } else { 400 }).to_le_bytes());
    out.extend_from_slice(&((font.size * 10_000.0).round() as u32).to_le_bytes());
    let face: Vec<u8> = font.name.chars().map(|c| c as u32 as u8).take(31).collect();
    out.push(face.len() as u8);
    out.extend_from_slice(&face);
    out
}

fn guid_and_picture(p: &Picture) -> Vec<u8> {
    let mut out = GUID_STD_PICTURE.to_vec();
    out.extend_from_slice(&0x0000_746Cu32.to_le_bytes());
    out.extend_from_slice(&(p.bytes.len() as u32).to_le_bytes());
    out.extend_from_slice(&p.bytes);
    out
}

/// A control's data for an `o` stream.
fn write_control(c: &Control) -> Vec<u8> {
    let d = Control::new(&c.name, c.kind);
    let mut v: Vec<(u8, PVal)> = Vec::new();
    let size = PVal::Pair(himetric(c.width), himetric(c.height));
    let mut pictures: Vec<&Picture> = Vec::new();
    let mut out = match c.kind {
        k if k.is_morph() => {
            if c.various() != d.various() {
                v.push((0, PVal::Int(c.various())));
            }
            if c.back_color != d.back_color {
                v.push((1, PVal::Int(c.back_color)));
            }
            if c.fore_color != d.fore_color {
                v.push((2, PVal::Int(c.fore_color)));
            }
            if c.max_length != 0 {
                v.push((3, PVal::Int(c.max_length)));
            }
            let style = if c.drop_list { 7 } else { k.display_style() };
            if style != 1 {
                v.push((6, PVal::Int(style)));
            }
            v.push((8, size));
            if let Some(ch) = c.password_char {
                v.push((9, PVal::Int(ch as u32)));
            }
            if c.bound_column != 1 {
                v.push((11, PVal::Int(c.bound_column as u32)));
            }
            if c.text_column != -1 {
                v.push((12, PVal::Int(c.text_column as u16 as u32)));
            }
            if c.column_count != 1 {
                v.push((13, PVal::Int(c.column_count as u16 as u32)));
            }
            if c.list_style != 0 {
                v.push((17, PVal::Int(u32::from(c.list_style))));
            }
            if c.multi_select != 0 {
                v.push((21, PVal::Int(u32::from(c.multi_select))));
            }
            if !c.value.is_empty() {
                v.push((22, PVal::Str(c.value.clone())));
            }
            if !c.caption.is_empty() {
                v.push((23, PVal::Str(c.caption.clone())));
            }
            if let Some(ch) = c.accelerator {
                v.push((29, PVal::Int(ch as u32)));
            }
            v.push((31, PVal::Flag));
            if !c.group_name.is_empty() {
                v.push((32, PVal::Str(c.group_name.clone())));
            }
            write_props((0, 2), 8, MORPH, &v)
        }
        ControlKind::CommandButton | ControlKind::Label => {
            if c.fore_color != d.fore_color {
                v.push((0, PVal::Int(c.fore_color)));
            }
            if c.back_color != d.back_color {
                v.push((1, PVal::Int(c.back_color)));
            }
            if c.various() != d.various() {
                v.push((2, PVal::Int(c.various())));
            }
            if !c.caption.is_empty() {
                v.push((3, PVal::Str(c.caption.clone())));
            }
            v.push((5, size));
            let accel = if c.kind == ControlKind::Label { 11 } else { 8 };
            if let Some(ch) = c.accelerator {
                v.push((accel, PVal::Int(ch as u32)));
            }
            if let Some(p) = &c.picture {
                v.push((
                    if c.kind == ControlKind::Label { 10 } else { 7 },
                    PVal::Flag,
                ));
                pictures.push(p);
            }
            let table = if c.kind == ControlKind::Label {
                LABEL
            } else {
                COMMAND_BUTTON
            };
            write_props((0, 2), 4, table, &v)
        }
        ControlKind::Image => {
            if c.back_color != d.back_color {
                v.push((4, PVal::Int(c.back_color)));
            }
            if c.picture_size_mode != 0 {
                v.push((7, PVal::Int(u32::from(c.picture_size_mode))));
            }
            v.push((9, size));
            if let Some(p) = &c.picture {
                v.push((10, PVal::Flag));
                pictures.push(p);
            }
            write_props((0, 2), 4, IMAGE, &v)
        }
        ControlKind::ScrollBar | ControlKind::SpinButton => {
            let spin = c.kind == ControlKind::SpinButton;
            v.push((3, size));
            if c.min != d.min {
                v.push((5, PVal::Int(c.min as u32)));
            }
            if c.max != d.max {
                v.push((6, PVal::Int(c.max as u32)));
            }
            if c.position != 0 {
                v.push((7, PVal::Int(c.position as u32)));
            }
            if c.small_change != 1 {
                v.push((if spin { 10 } else { 11 }, PVal::Int(c.small_change as u32)));
            }
            if !spin && c.large_change != 1 {
                v.push((12, PVal::Int(c.large_change as u32)));
            }
            if c.orientation != -1 {
                v.push((if spin { 11 } else { 13 }, PVal::Int(c.orientation as u32)));
            }
            write_props((0, 2), 4, if spin { SPIN_BUTTON } else { SCROLL_BAR }, &v)
        }
        ControlKind::TabStrip => {
            if c.tab_index_selected != -1 {
                v.push((0, PVal::Int(c.tab_index_selected as u32)));
            }
            v.push((4, size));
            v.push((5, PVal::Array(c.tabs.clone())));
            v.push((20, PVal::Int(c.tabs.len() as u32)));
            v.push((22, PVal::Int(c.tabs.len() as u32)));
            write_props((0, 2), 4, TAB_STRIP, &v)
        }
        _ => return Vec::new(),
    };
    for p in pictures {
        out.extend_from_slice(&guid_and_picture(p));
    }
    match c.kind {
        ControlKind::Image | ControlKind::ScrollBar | ControlKind::SpinButton => {}
        ControlKind::TabStrip => {
            out.extend_from_slice(&text_props(&c.font, c.text_align));
            for _ in &c.tabs {
                out.extend_from_slice(&3u32.to_le_bytes());
            }
        }
        _ => out.extend_from_slice(&text_props(&c.font, c.text_align)),
    }
    out
}

#[allow(clippy::too_many_arguments)]
/// A FormControl with sites for `sites` (indices into `design.controls`)
/// and their `o` stream.
fn write_form_control(
    caption: Option<&str>,
    size: (f32, f32),
    back: u32,
    font: &Font,
    picture: Option<&Picture>,
    design: &FormDesign,
    sites: &[usize],
    tab_strip: Option<&Control>,
) -> (Vec<u8>, Vec<u8>) {
    let mut v = Vec::new();
    if back != 0x8000_000F {
        v.push((1, PVal::Int(back)));
    }
    let next = sites
        .iter()
        .map(|&i| design.controls[i].id)
        .max()
        .unwrap_or(0)
        + 1;
    v.push((3, PVal::Int(next as u32)));
    v.push((10, PVal::Pair(himetric(size.0), himetric(size.1))));
    if let Some(c) = caption {
        v.push((19, PVal::Str(c.to_owned())));
    }
    v.push((20, PVal::Flag));
    if picture.is_some() {
        v.push((21, PVal::Flag));
    }
    v.push((27, PVal::Int(32000)));
    let mut f = write_props((0, 4), 4, FORM, &v);
    f.extend_from_slice(&std_font(font));
    if let Some(p) = picture {
        f.extend_from_slice(&guid_and_picture(p));
    }
    // An empty class table.
    f.extend_from_slice(&0u16.to_le_bytes());
    let mut o = Vec::new();
    if let Some(strip) = tab_strip {
        o = write_control(strip);
    }
    let mut site_bytes = Vec::new();
    for &i in sites {
        let c = &design.controls[i];
        let data = if c.kind.is_container() {
            Vec::new()
        } else {
            write_control(c)
        };
        let flags = u32::from(c.tab_stop)
            | (u32::from(c.visible) << 1)
            | (u32::from(c.default) << 2)
            | (u32::from(c.cancel) << 3)
            | (u32::from(!c.kind.is_container()) << 4)
            | if c.kind.is_container() { 1 << 18 } else { 0 };
        let mut sv = vec![(0, PVal::Str(c.name.clone()))];
        if !c.tag.is_empty() {
            sv.push((1, PVal::Str(c.tag.clone())));
        }
        sv.push((2, PVal::Int(c.id as u32)));
        sv.push((4, PVal::Int(flags)));
        if !data.is_empty() {
            sv.push((5, PVal::Int(data.len() as u32)));
        }
        sv.push((6, PVal::Int(c.tab_index as u16 as u32)));
        sv.push((7, PVal::Int(u32::from(c.kind.clsid_index()))));
        sv.push((8, PVal::Pair(himetric(c.left), himetric(c.top))));
        if !c.tip.is_empty() {
            sv.push((11, PVal::Str(c.tip.clone())));
        }
        if !c.control_source.is_empty() {
            sv.push((13, PVal::Str(c.control_source.clone())));
        }
        if !c.row_source.is_empty() {
            sv.push((14, PVal::Str(c.row_source.clone())));
        }
        site_bytes.extend_from_slice(&write_props((0, 0), 4, SITE, &sv));
        o.extend_from_slice(&data);
    }
    // SiteDepthsAndTypes: one entry per site (depth 0, ST_Ole).
    let mut depths = Vec::new();
    for _ in sites {
        depths.extend_from_slice(&[0, 1]);
    }
    while depths.len() % 4 != 0 {
        depths.push(0);
    }
    f.extend_from_slice(&(sites.len() as u32).to_le_bytes());
    f.extend_from_slice(&((depths.len() + site_bytes.len()) as u32).to_le_bytes());
    f.extend_from_slice(&depths);
    f.extend_from_slice(&site_bytes);
    (f, o)
}

/// The streams of a form's storage (paths relative to it, `"f"`, `"o"`,
/// `"\x03VBFrame"`, `"i01/f"`…) for `design`. Controls' IDs are given by
/// position when they're 0. What is written is what [`read_streams`]
/// reads back; it's not checked against Excel.
pub fn write_streams(design: &FormDesign) -> Vec<(String, Vec<u8>)> {
    let mut design = design.clone();
    let mut used: Vec<i32> = design.controls.iter().map(|c| c.id).collect();
    for i in 0..design.controls.len() {
        if design.controls[i].id == 0 {
            let id = (1..).find(|n| !used.contains(n)).unwrap_or(1);
            design.controls[i].id = id;
            used.push(id);
        }
    }
    let mut out = Vec::new();
    out.push((
        "\u{3}VBFrame".to_owned(),
        format!(
            "VERSION 5.00\r\nBegin {{C62A69F0-16DC-11CE-9E98-00AA00574A4F}} {}\r\n   Caption         =   \"{}\"\r\n   ClientHeight    =   {}\r\n   ClientWidth     =   {}\r\n   StartUpPosition =   {}\r\nEnd\r\n",
            design.name,
            design.caption,
            (design.height * 20.0).round(),
            (design.width * 20.0).round(),
            design.start_up_position
        )
        .into_bytes(),
    ));
    let top = design.children(None);
    let (f, o) = write_form_control(
        Some(&design.caption),
        (design.width, design.height),
        design.back_color,
        &design.font,
        design.picture.as_ref(),
        &design,
        &top,
        None,
    );
    out.push(("f".into(), f));
    out.push(("o".into(), o));
    write_containers(&design, &top, "", &mut out);
    out
}

fn write_containers(
    design: &FormDesign,
    sites: &[usize],
    prefix: &str,
    out: &mut Vec<(String, Vec<u8>)>,
) {
    for &i in sites {
        let c = &design.controls[i];
        if !c.kind.is_container() {
            continue;
        }
        let storage = format!("{prefix}i{:02}/", c.id);
        let children = design.children(Some(i));
        let (f, o) = if c.kind == ControlKind::MultiPage {
            let mut strip = Control::new("", ControlKind::TabStrip);
            strip.tabs = children
                .iter()
                .map(|&p| design.controls[p].caption.clone())
                .collect();
            strip.tab_index_selected = c.tab_index_selected;
            strip.width = c.width;
            strip.height = c.height;
            strip.font = c.font.clone();
            write_form_control(
                None,
                (c.width, c.height),
                c.back_color,
                &c.font,
                None,
                design,
                &children,
                Some(&strip),
            )
        } else {
            let caption = (c.kind == ControlKind::Frame).then_some(c.caption.as_str());
            write_form_control(
                caption,
                (c.width, c.height),
                c.back_color,
                &c.font,
                c.picture.as_ref(),
                design,
                &children,
                None,
            )
        };
        out.push((format!("{storage}f"), f));
        out.push((format!("{storage}o"), o));
        write_containers(design, &children, &storage, out);
    }
}
