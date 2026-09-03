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
