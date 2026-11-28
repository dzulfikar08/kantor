//! LaTeX command names for Unicode math characters, and the spacing
//! class of each character.

/// LaTeX commands that stand for one character. The first entry for a
/// character is the name LaTeX output uses.
pub const SYMBOLS: &[(&str, char)] = &[
    // Greek
    ("alpha", 'α'),
    ("beta", 'β'),
    ("gamma", 'γ'),
    ("delta", 'δ'),
    ("epsilon", 'ϵ'),
    ("varepsilon", 'ε'),
    ("zeta", 'ζ'),
    ("eta", 'η'),
    ("theta", 'θ'),
    ("vartheta", 'ϑ'),
    ("iota", 'ι'),
    ("kappa", 'κ'),
    ("varkappa", 'ϰ'),
    ("lambda", 'λ'),
    ("mu", 'μ'),
    ("nu", 'ν'),
    ("xi", 'ξ'),
    ("omicron", 'ο'),
    ("pi", 'π'),
    ("varpi", 'ϖ'),
    ("rho", 'ρ'),
    ("varrho", 'ϱ'),
    ("sigma", 'σ'),
    ("varsigma", 'ς'),
    ("tau", 'τ'),
    ("upsilon", 'υ'),
    ("phi", 'ϕ'),
    ("varphi", 'φ'),
    ("chi", 'χ'),
    ("psi", 'ψ'),
    ("omega", 'ω'),
    ("Gamma", 'Γ'),
    ("Delta", 'Δ'),
    ("Theta", 'Θ'),
    ("Lambda", 'Λ'),
    ("Xi", 'Ξ'),
    ("Pi", 'Π'),
    ("Sigma", 'Σ'),
    ("Upsilon", 'Υ'),
    ("Phi", 'Φ'),
    ("Psi", 'Ψ'),
    ("Omega", 'Ω'),
    ("digamma", 'ϝ'),
    // Hebrew and letter-like
    ("aleph", 'ℵ'),
    ("beth", 'ℶ'),
    ("gimel", 'ℷ'),
    ("hbar", 'ℏ'),
    ("hslash", 'ℏ'),
    ("ell", 'ℓ'),
    ("wp", '℘'),
    ("Re", 'ℜ'),
    ("Im", 'ℑ'),
    ("partial", '∂'),
    ("infty", '∞'),
    ("nabla", '∇'),
    ("forall", '∀'),
    ("exists", '∃'),
    ("nexists", '∄'),
    ("emptyset", '∅'),
    ("varnothing", '∅'),
    ("imath", 'ı'),
    ("jmath", 'ȷ'),
    ("complement", '∁'),
    ("eth", 'ð'),
    ("mho", '℧'),
    // Binary operators
    ("pm", '±'),
    ("mp", '∓'),
    ("times", '×'),
    ("div", '÷'),
    ("cdot", '⋅'),
    ("ast", '∗'),
    ("star", '⋆'),
    ("circ", '∘'),
    ("bullet", '∙'),
    ("cap", '∩'),
    ("cup", '∪'),
    ("sqcap", '⊓'),
    ("sqcup", '⊔'),
    ("wedge", '∧'),
    ("land", '∧'),
    ("vee", '∨'),
    ("lor", '∨'),
    ("oplus", '⊕'),
    ("ominus", '⊖'),
    ("otimes", '⊗'),
    ("oslash", '⊘'),
    ("odot", '⊙'),
    ("setminus", '∖'),
    ("smallsetminus", '∖'),
    ("uplus", '⊎'),
    ("amalg", '⨿'),
    ("diamond", '⋄'),
    ("bigtriangleup", '△'),
    ("bigtriangledown", '▽'),
    ("triangleleft", '◁'),
    ("triangleright", '▷'),
    ("dagger", '†'),
    ("ddagger", '‡'),
    ("wr", '≀'),
    ("divideontimes", '⋇'),
    ("ltimes", '⋉'),
    ("rtimes", '⋊'),
    // Relations
    ("leq", '≤'),
    ("le", '≤'),
    ("geq", '≥'),
    ("ge", '≥'),
    ("leqslant", '⩽'),
    ("geqslant", '⩾'),
    ("neq", '≠'),
    ("ne", '≠'),
    ("approx", '≈'),
    ("equiv", '≡'),
    ("sim", '∼'),
    ("simeq", '≃'),
    ("cong", '≅'),
    ("propto", '∝'),
    ("subset", '⊂'),
    ("supset", '⊃'),
    ("subseteq", '⊆'),
    ("supseteq", '⊇'),
    ("subsetneq", '⊊'),
    ("supsetneq", '⊋'),
    ("sqsubseteq", '⊑'),
    ("sqsupseteq", '⊒'),
    ("in", '∈'),
    ("notin", '∉'),
    ("ni", '∋'),
    ("owns", '∋'),
    ("perp", '⊥'),
    ("parallel", '∥'),
    ("nparallel", '∦'),
    ("mid", '∣'),
    ("nmid", '∤'),
    ("ll", '≪'),
    ("gg", '≫'),
    ("lll", '⋘'),
    ("ggg", '⋙'),
    ("prec", '≺'),
    ("succ", '≻'),
    ("preceq", '⪯'),
    ("succeq", '⪰'),
    ("asymp", '≍'),
    ("doteq", '≐'),
    ("models", '⊨'),
    ("vdash", '⊢'),
    ("dashv", '⊣'),
    ("vDash", '⊨'),
    ("bowtie", '⋈'),
    ("smile", '⌣'),
    ("frown", '⌢'),
    ("approxeq", '≊'),
    ("lesssim", '≲'),
    ("gtrsim", '≳'),
    ("triangleq", '≜'),
    ("coloneqq", '≔'),
    ("nless", '≮'),
    ("ngtr", '≯'),
    ("nleq", '≰'),
    ("ngeq", '≱'),
    ("nsim", '≁'),
    ("ncong", '≇'),
    ("nsubseteq", '⊈'),
    ("nsupseteq", '⊉'),
    ("therefore", '∴'),
    ("because", '∵'),
    // Arrows
    ("rightarrow", '→'),
    ("to", '→'),
    ("leftarrow", '←'),
    ("gets", '←'),
    ("leftrightarrow", '↔'),
    ("Rightarrow", '⇒'),
    ("Leftarrow", '⇐'),
    ("Leftrightarrow", '⇔'),
    ("longrightarrow", '⟶'),
    ("longleftarrow", '⟵'),
    ("longleftrightarrow", '⟷'),
    ("Longrightarrow", '⟹'),
    ("implies", '⟹'),
    ("Longleftarrow", '⟸'),
    ("impliedby", '⟸'),
    ("Longleftrightarrow", '⟺'),
    ("iff", '⟺'),
    ("mapsto", '↦'),
    ("longmapsto", '⟼'),
    ("uparrow", '↑'),
    ("downarrow", '↓'),
    ("updownarrow", '↕'),
    ("Uparrow", '⇑'),
    ("Downarrow", '⇓'),
    ("Updownarrow", '⇕'),
    ("nearrow", '↗'),
    ("searrow", '↘'),
    ("swarrow", '↙'),
    ("nwarrow", '↖'),
    ("hookrightarrow", '↪'),
    ("hookleftarrow", '↩'),
    ("rightharpoonup", '⇀'),
    ("rightharpoondown", '⇁'),
    ("leftharpoonup", '↼'),
    ("leftharpoondown", '↽'),
    ("rightleftharpoons", '⇌'),
    ("leftrightarrows", '⇆'),
    ("rightleftarrows", '⇄'),
    ("leadsto", '⇝'),
    ("circlearrowleft", '↺'),
    ("circlearrowright", '↻'),
    // Big operators
    ("sum", '∑'),
    ("prod", '∏'),
    ("coprod", '∐'),
    ("int", '∫'),
    ("iint", '∬'),
    ("iiint", '∭'),
    ("oint", '∮'),
    ("oiint", '∯'),
    ("oiiint", '∰'),
    ("bigcup", '⋃'),
    ("bigcap", '⋂'),
    ("bigsqcup", '⨆'),
    ("bigvee", '⋁'),
    ("bigwedge", '⋀'),
    ("bigoplus", '⨁'),
    ("bigotimes", '⨂'),
    ("bigodot", '⨀'),
    ("biguplus", '⨄'),
    // Delimiters and punctuation
    ("langle", '⟨'),
    ("rangle", '⟩'),
    ("lfloor", '⌊'),
    ("rfloor", '⌋'),
    ("lceil", '⌈'),
    ("rceil", '⌉'),
    ("vert", '|'),
    ("Vert", '‖'),
    ("lvert", '|'),
    ("rvert", '|'),
    ("lVert", '‖'),
    ("rVert", '‖'),
    ("lbrace", '{'),
    ("rbrace", '}'),
    ("lbrack", '['),
    ("rbrack", ']'),
    ("backslash", '∖'),
    ("colon", ':'),
    // Dots
    ("ldots", '…'),
    ("dots", '…'),
    ("dotsc", '…'),
    ("dotso", '…'),
    ("cdots", '⋯'),
    ("dotsb", '⋯'),
    ("dotsm", '⋯'),
    ("vdots", '⋮'),
    ("ddots", '⋱'),
    ("iddots", '⋰'),
    // Miscellaneous
    ("prime", '′'),
    ("angle", '∠'),
    ("measuredangle", '∡'),
    ("triangle", '△'),
    ("square", '□'),
    ("Box", '□'),
    ("blacksquare", '■'),
    ("lozenge", '◊'),
    ("neg", '¬'),
    ("lnot", '¬'),
    ("top", '⊤'),
    ("bot", '⊥'),
    ("degree", '°'),
    ("circledR", '®'),
    ("checkmark", '✓'),
    ("clubsuit", '♣'),
    ("diamondsuit", '♢'),
    ("heartsuit", '♡'),
    ("spadesuit", '♠'),
    ("flat", '♭'),
    ("natural", '♮'),
    ("sharp", '♯'),
    ("surd", '√'),
    ("S", '§'),
    ("P", '¶'),
    ("copyright", '©'),
    ("pounds", '£'),
    ("euro", '€'),
    ("dag", '†'),
    ("ddag", '‡'),
];

/// LaTeX spaces: command and width in em.
pub const SPACES: &[(&str, char)] = &[
    (",", '\u{2009}'),
    ("thinspace", '\u{2009}'),
    (":", '\u{205F}'),
    (">", '\u{205F}'),
    ("medspace", '\u{205F}'),
    (";", '\u{2004}'),
    ("thickspace", '\u{2004}'),
    ("quad", '\u{2003}'),
    ("qquad", '\u{2001}'),
    ("enspace", '\u{2002}'),
    (" ", ' '),
    ("!", '\u{200B}'),
    ("negthinspace", '\u{200B}'),
];

/// Function names typeset upright (`\sin`, `\log`…).
pub const FUNCTIONS: &[&str] = &[
    "sin", "cos", "tan", "cot", "sec", "csc", "arcsin", "arccos", "arctan", "arccot", "arcsec",
    "arccsc", "sinh", "cosh", "tanh", "coth", "sech", "csch", "log", "ln", "lg", "exp", "lim",
    "limsup", "liminf", "max", "min", "sup", "inf", "det", "dim", "ker", "gcd", "lcm", "deg",
    "arg", "Pr", "hom", "mod", "tg", "ctg", "arctg", "sgn",
];

/// Functions whose limits go under them in display math.
pub const LIMIT_FUNCTIONS: &[&str] = &[
    "lim", "limsup", "liminf", "max", "min", "sup", "inf", "det", "gcd", "Pr",
];

/// The character of a LaTeX symbol command.
pub fn symbol(name: &str) -> Option<char> {
    SYMBOLS.iter().find(|(n, _)| *n == name).map(|&(_, c)| c)
}

/// The LaTeX command for a character, if it needs one.
pub fn command_for(c: char) -> Option<&'static str> {
    SYMBOLS.iter().find(|(_, ch)| *ch == c).map(|&(n, _)| n)
}

/// The space a LaTeX spacing command stands for.
pub fn space(name: &str) -> Option<char> {
    SPACES.iter().find(|(n, _)| *n == name).map(|&(_, c)| c)
}

/// The LaTeX command for a space character.
pub fn space_command(c: char) -> Option<&'static str> {
    SPACES.iter().find(|(_, ch)| *ch == c).map(|&(n, _)| n)
}

/// Width of a math space character in em, `None` for other characters.
pub fn space_width(c: char) -> Option<f32> {
    Some(match c {
        '\u{2009}' => 3.0 / 18.0,
        '\u{205F}' => 4.0 / 18.0,
        '\u{2004}' => 5.0 / 18.0,
        '\u{2003}' => 1.0,
        '\u{2001}' => 2.0,
        '\u{2002}' => 0.5,
        '\u{200A}' => 1.0 / 18.0,
        '\u{2005}' => 0.25,
        '\u{2006}' => 1.0 / 6.0,
        '\u{2007}' | '\u{2008}' => 0.25,
        '\u{200B}' => -3.0 / 18.0,
        '\u{00A0}' | ' ' => 0.25,
        _ => return None,
    })
}

/// Spacing class of a math character (TeX's atom types).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class {
    Ord,
    Op,
    Bin,
    Rel,
    Open,
    Close,
    Punct,
    Inner,
}

/// The class of a character.
pub fn class(c: char) -> Class {
    match c {
        '+' | '-' | '−' | '±' | '∓' | '×' | '÷' | '⋅' | '·' | '∗' | '⋆' | '∘' | '∙' | '∩' | '∪'
        | '⊓' | '⊔' | '∧' | '∨' | '⊕' | '⊖' | '⊗' | '⊘' | '⊙' | '∖' | '⊎' | '⨿' | '⋄' | '△'
        | '▽' | '◁' | '▷' | '†' | '‡' | '≀' | '⋉' | '⋊' | '⋇' => Class::Bin,
        '=' | '<' | '>' | '≤' | '≥' | '⩽' | '⩾' | '≠' | '≈' | '≡' | '∼' | '≃' | '≅' | '∝' | '⊂'
        | '⊃' | '⊆' | '⊇' | '⊊' | '⊋' | '⊑' | '⊒' | '∈' | '∉' | '∋' | '⊥' | '∥' | '∦' | '∣'
        | '∤' | '≪' | '≫' | '⋘' | '⋙' | '≺' | '≻' | '⪯' | '⪰' | '≍' | '≐' | '⊨' | '⊢' | '⊣'
        | '⋈' | '⌣' | '⌢' | '≊' | '≲' | '≳' | '≜' | '≔' | '≮' | '≯' | '≰' | '≱' | '≁' | '≇'
        | '⊈' | '⊉' | '∴' | '∵' | ':' | '≝' | '≞' | '≟' => Class::Rel,
        '\u{2190}'..='\u{21FF}' | '\u{27F0}'..='\u{27FF}' | '\u{2900}'..='\u{297F}' => Class::Rel,
        '(' | '[' | '{' | '⟨' | '⌊' | '⌈' | '⟦' | '⦃' | '⦅' => Class::Open,
        ')' | ']' | '}' | '⟩' | '⌋' | '⌉' | '⟧' | '⦄' | '⦆' | '!' => Class::Close,
        ',' | ';' => Class::Punct,
        c if is_nary(c) => Class::Op,
        _ => Class::Ord,
    }
}

/// A big operator that takes limits (OMML `m:nary`).
pub fn is_nary(c: char) -> bool {
    matches!(
        c,
        '∑' | '∏'
            | '∐'
            | '∫'
            | '∬'
            | '∭'
            | '∮'
            | '∯'
            | '∰'
            | '⨌'
            | '∱'
            | '∲'
            | '∳'
            | '⋃'
            | '⋂'
            | '⨆'
            | '⨅'
            | '⋁'
            | '⋀'
            | '⨁'
            | '⨂'
            | '⨀'
            | '⨄'
    )
}

/// An integral: limits beside it by default.
pub fn is_integral(c: char) -> bool {
    matches!(c, '∫' | '∬' | '∭' | '∮' | '∯' | '∰' | '⨌' | '∱' | '∲' | '∳')
}

/// LaTeX accent commands and their combining characters.
pub const ACCENTS: &[(&str, char)] = &[
    ("hat", '\u{0302}'),
    ("widehat", '\u{0302}'),
    ("check", '\u{030C}'),
    ("widecheck", '\u{030C}'),
    ("tilde", '\u{0303}'),
    ("widetilde", '\u{0303}'),
    ("acute", '\u{0301}'),
    ("grave", '\u{0300}'),
    ("dot", '\u{0307}'),
    ("ddot", '\u{0308}'),
    ("dddot", '\u{20DB}'),
    ("breve", '\u{0306}'),
    ("bar", '\u{0305}'),
    ("vec", '\u{20D7}'),
    ("overrightarrow", '\u{20D7}'),
    ("overleftarrow", '\u{20D6}'),
    ("overleftrightarrow", '\u{20E1}'),
    ("mathring", '\u{030A}'),
];

pub fn accent(name: &str) -> Option<char> {
    ACCENTS.iter().find(|(n, _)| *n == name).map(|&(_, c)| c)
}

pub fn accent_command(c: char) -> Option<&'static str> {
    ACCENTS.iter().find(|(_, ch)| *ch == c).map(|&(n, _)| n)
}

/// A spacing (non-combining) form of an accent, as Word sometimes stores
/// accents: the combining character it stands for.
pub fn combining(c: char) -> char {
    match c {
        '^' | 'ˆ' => '\u{0302}',
        '~' | '˜' => '\u{0303}',
        '¯' | '‾' => '\u{0305}',
        '˙' => '\u{0307}',
        '¨' => '\u{0308}',
        'ˇ' => '\u{030C}',
        '˘' => '\u{0306}',
        '´' => '\u{0301}',
        '`' => '\u{0300}',
        '→' | '⃗' => '\u{20D7}',
        '←' => '\u{20D6}',
        '↔' => '\u{20E1}',
        c => c,
    }
}

/// Delimiters: LaTeX spelling (after `\left`) of a bracket character.
pub fn delim_latex(s: &str) -> String {
    match s {
        "" => ".".into(),
        "{" => "\\{".into(),
        "}" => "\\}".into(),
        "⟨" | "〈" => "\\langle".into(),
        "⟩" | "〉" => "\\rangle".into(),
        "⌊" => "\\lfloor".into(),
        "⌋" => "\\rfloor".into(),
        "⌈" => "\\lceil".into(),
        "⌉" => "\\rceil".into(),
        "‖" | "∥" => "\\|".into(),
        "⟦" => "\\llbracket".into(),
        "⟧" => "\\rrbracket".into(),
        s => s.into(),
    }
}
