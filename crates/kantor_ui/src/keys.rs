//! Platform shortcuts: bindings and labels are written once with `ctrl-`
//! / "Ctrl", and become Cmd (⌘) on macOS.
//!
//! GPUI's own `secondary-` alias can't be used: `KeyBinding::new` parses
//! without an OS and rejects it.

/// A binding written with `ctrl-` for this platform: `cmd-` on macOS.
/// Ctrl+Tab / Ctrl+Shift+Tab stay Ctrl (Cmd+Tab is the macOS app switcher).
pub fn primary(keys: &'static str) -> &'static str {
    if cfg!(target_os = "macos") {
        to_mac(keys).map_or(keys, |s| Box::leak(s.into_boxed_str()))
    } else {
        keys
    }
}

fn to_mac(keys: &str) -> Option<String> {
    if !keys.contains("ctrl-") || keys.contains("tab") {
        return None;
    }
    Some(keys.replace("ctrl-", "cmd-"))
}

/// The two bindings of a primary shortcut written with `ctrl-` ("ctrl-x"):
/// [`primary`] and `super-` (Super as Mac-style Cmd on Linux and Windows).
///
/// The primary one comes last so it takes precedence: menus and tooltips
/// label an action with its highest-precedence binding, and GPUI-component
/// shows `super-` as "Win" off macOS ("Win+X" instead of "Ctrl+X").
pub fn primary_pair<A: gpui_kit::Action + Clone>(
    keys: &'static str,
    action: A,
    context: Option<&str>,
) -> [gpui_kit::KeyBinding; 2] {
    let rest = keys.strip_prefix("ctrl-").unwrap_or(keys);
    // Leaked like `primary` on macOS: bindings are made once, at start-up.
    let sup: &'static str = Box::leak(format!("super-{rest}").into_boxed_str());
    [
        gpui_kit::KeyBinding::new(sup, action.clone(), context),
        gpui_kit::KeyBinding::new(primary(keys), action, context),
    ]
}

/// A shortcut label written for Linux ("Ctrl+Shift+B", "Ctrl K") as shown on
/// this platform: "⌘⇧B", "⌘K" on macOS.
pub fn label(keys: &str) -> String {
    if cfg!(target_os = "macos") {
        mac_label(keys)
    } else {
        keys.to_owned()
    }
}

fn mac_label(keys: &str) -> String {
    if keys.contains("Tab") {
        return keys.replace("Ctrl", "⌃");
    }
    let mut out = String::new();
    // Labels list several shortcuts as "Ctrl+Z / Ctrl+Y"; keep separators.
    for (i, part) in keys.split(" / ").enumerate() {
        if i > 0 {
            out.push_str(" / ");
        }
        let words: Vec<&str> = part.split(['+', ' ']).filter(|w| !w.is_empty()).collect();
        let mut mods = String::new();
        let mut rest = Vec::new();
        for w in words {
            match w {
                "Ctrl" | "Super" | "Cmd" => mods.push('⌘'),
                "Shift" => mods.push('⇧'),
                "Alt" => mods.push('⌥'),
                other => rest.push(other),
            }
        }
        // Order modifiers as macOS does: ⌃⌥⇧⌘.
        let order = ['⌃', '⌥', '⇧', '⌘'];
        let mut sorted: Vec<char> = mods.chars().collect();
        sorted.sort_by_key(|c| order.iter().position(|o| o == c));
        sorted.dedup();
        out.extend(sorted);
        out.push_str(&rest.join(" "));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[std::prelude::v1::test]
    fn bindings_become_cmd_on_mac_except_tab() {
        assert_eq!(to_mac("ctrl-shift-b").as_deref(), Some("cmd-shift-b"));
        assert_eq!(to_mac("ctrl-tab"), None);
        assert_eq!(to_mac("super-q"), None);
    }

    gpui_kit::actions!(keys_test, [Cut]);

    #[std::prelude::v1::test]
    fn primary_binding_takes_precedence_over_super() {
        let keymap = gpui_kit::Keymap::new(primary_pair("ctrl-x", Cut, Some("Docs")).into());
        let strokes: Vec<_> = keymap
            .bindings_for_action(&Cut)
            .map(|b| b.keystrokes()[0].inner().clone())
            .collect();
        assert_eq!(strokes.len(), 2);
        // Later bindings take precedence, so labels show the last one:
        // Ctrl+X off macOS (not "Win+X"), ⌘X on macOS.
        let top = strokes.last().unwrap();
        assert_eq!(top.key, "x");
        assert_eq!(top.modifiers.control, !cfg!(target_os = "macos"));
        assert_eq!(top.modifiers.platform, cfg!(target_os = "macos"));
    }

    #[std::prelude::v1::test]
    fn labels_use_mac_symbols() {
        assert_eq!(mac_label("Ctrl K"), "⌘K");
        assert_eq!(mac_label("Ctrl+Shift+B"), "⇧⌘B");
        assert_eq!(mac_label("Ctrl+Alt+N"), "⌥⌘N");
        assert_eq!(mac_label("Ctrl+Z / Ctrl+Y"), "⌘Z / ⌘Y");
        assert_eq!(mac_label("F4"), "F4");
        assert_eq!(mac_label("Ctrl+Tab"), "⌃+Tab");
    }
}
