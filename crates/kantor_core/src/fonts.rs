//! Free, metric-compatible stand-ins for common Office fonts (plan §5).
//! Kantor never bundles proprietary fonts; when a document asks for one that
//! isn't installed, its substitute keeps line breaks and column fits close.

/// The metric-compatible free substitute for `font`, if it has one
/// (case-insensitive): Calibri → Carlito, Arial → Liberation Sans, ...
pub fn metric_substitute(font: &str) -> Option<&'static str> {
    Some(match font.to_ascii_lowercase().as_str() {
        "calibri" | "calibri light" => "Carlito",
        "cambria" | "cambria math" => "Caladea",
        "arial" | "helvetica" => "Liberation Sans",
        "times new roman" | "times" => "Liberation Serif",
        "courier new" | "courier" => "Liberation Mono",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn substitutes() {
        assert_eq!(metric_substitute("Calibri"), Some("Carlito"));
        assert_eq!(metric_substitute("ARIAL"), Some("Liberation Sans"));
        assert_eq!(metric_substitute("Inter"), None);
    }
}
