use ironcalc_base::types::{
    Alignment, Border, BorderItem, CellProtection, Color, Fill, HorizontalAlignment,
    VerticalAlignment,
};

pub(crate) fn get_color_xml(color: &Color, name: &str) -> String {
    match color {
        Color::Rgb(s) => {
            let hex = s.trim_start_matches('#');
            format!("<{name} rgb=\"FF{hex}\"/>")
        }
        Color::Theme(idx, tint) => {
            if *tint == 0.0 {
                format!("<{name} theme=\"{idx}\"/>")
            } else {
                format!("<{name} theme=\"{idx}\" tint=\"{}\"/>", tint)
            }
        }
        Color::None => "".to_string(),
    }
}

pub(crate) fn get_alignment(alignment: &Alignment) -> String {
    let wrap_text = if alignment.wrap_text {
        " wrapText=\"1\""
    } else {
        ""
    };
    let horizontal = if alignment.horizontal != HorizontalAlignment::default() {
        format!(" horizontal=\"{}\"", alignment.horizontal)
    } else {
        "".to_string()
    };
    let vertical = if alignment.vertical != VerticalAlignment::default() {
        format!(" vertical=\"{}\"", alignment.vertical)
    } else {
        "".to_string()
    };
    // KANTOR PATCH (fidelity): indent, rotation and shrink-to-fit.
    let indent = match alignment.indent {
        0 => "".to_string(),
        n => format!(" indent=\"{n}\""),
    };
    let rotation = match alignment.text_rotation {
        0 => "".to_string(),
        n => format!(" textRotation=\"{n}\""),
    };
    let shrink = if alignment.shrink_to_fit {
        " shrinkToFit=\"1\""
    } else {
        ""
    };
    format!("<alignment{wrap_text}{horizontal}{vertical}{indent}{rotation}{shrink}/>")
}

// KANTOR PATCH (fidelity): `<protection>` of a cell xf.
pub(crate) fn get_protection(protection: &CellProtection) -> String {
    let locked = if protection.locked {
        ""
    } else {
        " locked=\"0\""
    };
    let hidden = if protection.hidden {
        " hidden=\"1\""
    } else {
        ""
    };
    format!("<protection{locked}{hidden}/>")
}

fn get_border_xml_inner(border: &Option<BorderItem>, name: &str) -> String {
    if let Some(border_item) = border {
        let color = get_color_xml(&border_item.color, "color");
        // KANTOR PATCH (fidelity): OOXML's names (`Display` is lower case).
        return format!(
            "<{name} style=\"{}\">{color}</{name}>",
            border_item.style.ooxml_name()
        );
    }
    format!("<{name}/>")
}

pub(crate) fn get_border_xml(border: &Border) -> String {
    let left = get_border_xml_inner(&border.left, "left");
    let right = get_border_xml_inner(&border.right, "right");
    let top = get_border_xml_inner(&border.top, "top");
    let bottom = get_border_xml_inner(&border.bottom, "bottom");
    let diagonal = get_border_xml_inner(&border.diagonal, "diagonal");
    // KANTOR PATCH (fidelity): which diagonals a diagonal border draws.
    let up = if border.diagonal_up {
        " diagonalUp=\"1\""
    } else {
        ""
    };
    let down = if border.diagonal_down {
        " diagonalDown=\"1\""
    } else {
        ""
    };
    format!("<border{up}{down}>{left}{right}{top}{bottom}{diagonal}</border>")
}

pub(crate) fn get_fill_xml(fill: &Fill) -> String {
    if fill.color.is_some() {
        let fg_color = get_color_xml(&fill.color, "fgColor");
        let bg_color = "<bgColor indexed=\"64\"/>".to_string();
        format!(
            "<fill><patternFill patternType=\"solid\">{fg_color}{bg_color}</patternFill></fill>"
        )
    } else {
        "<fill><patternFill patternType=\"none\"/></fill>".to_string()
    }
}

pub(crate) fn get_dxf_fill_xml(fill: &Fill) -> String {
    let bg_color = get_color_xml(&fill.color, "bgColor");
    format!("<fill><patternFill>{bg_color}</patternFill></fill>")
}
