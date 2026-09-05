//! Placeholder palette. Components must not hard-code colors (docs/11-gpui.md
//! → Theming); these consts are the single seam to replace with `ThemeProvider`
//! tokens once the theme system lands (docs/TODO.md).

pub const SURFACE: u32 = 0x1e1e2e;
pub const SURFACE_RAISED: u32 = 0x181825;
pub const SURFACE_INPUT: u32 = 0x11111b;
pub const BORDER: u32 = 0x313244;
pub const TEXT: u32 = 0xcdd6f4;
pub const TEXT_DIM: u32 = 0x6c7086;
pub const ACCENT: u32 = 0x89b4fa;
pub const SELECTION: u32 = 0x89b4fa;
pub const ERROR: u32 = 0xf38ba8;
pub const SUCCESS: u32 = 0xa6e3a1;

// Syntax highlight colors keyed by tree-sitter capture name (queries/highlights.scm).
pub const HL_KEYWORD: u32 = 0x89b4fa;
pub const HL_KEYWORD_OPERATOR: u32 = 0xcba6f7;
pub const HL_STRING: u32 = 0xa6e3a1;
pub const HL_NUMBER: u32 = 0xfab387;
pub const HL_COMMENT: u32 = 0x6c7086;
pub const HL_FUNCTION: u32 = 0x89dceb;
pub const HL_TYPE: u32 = 0xf9e2af;
pub const HL_OPERATOR: u32 = 0x94e2d5;
pub const HL_PUNCTUATION: u32 = 0x9399b2;
pub const EDITOR_GUTTER: u32 = 0x45475a;
pub const EDITOR_CURRENT_LINE: u32 = 0x24243a;

/// Color for a highlight capture; `TEXT` for anything unmapped.
pub fn highlight_color(capture: &str) -> u32 {
    match capture {
        "keyword" | "conditional" | "attribute" | "storageclass" => HL_KEYWORD,
        "keyword.operator" => HL_KEYWORD_OPERATOR,
        "string" => HL_STRING,
        "number" | "float" | "boolean" => HL_NUMBER,
        "comment" => HL_COMMENT,
        "function.call" | "parameter" => HL_FUNCTION,
        "type" | "type.builtin" | "type.qualifier" => HL_TYPE,
        "operator" => HL_OPERATOR,
        "punctuation.delimiter" | "punctuation.bracket" => HL_PUNCTUATION,
        _ => TEXT,
    }
}
