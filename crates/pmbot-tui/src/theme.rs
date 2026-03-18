// Tokyo Night color theme for the TUI
use ratatui::style::{Color, Modifier, Style};

// ── Base palette ────────────────────────────────────────────────
pub const BG: Color = Color::Rgb(0x1a, 0x1b, 0x26);
pub const BG_DARK: Color = Color::Rgb(0x16, 0x16, 0x1e);
pub const BG_HIGHLIGHT: Color = Color::Rgb(0x29, 0x2e, 0x42);
pub const FG: Color = Color::Rgb(0xc0, 0xca, 0xf5);
pub const FG_DIM: Color = Color::Rgb(0x56, 0x5f, 0x89);
pub const BORDER: Color = Color::Rgb(0x3b, 0x42, 0x61);

// ── Accent colors ──────────────────────────────────────────────
pub const RED: Color = Color::Rgb(0xf7, 0x76, 0x8e);
pub const GREEN: Color = Color::Rgb(0x9e, 0xce, 0x6a);
pub const YELLOW: Color = Color::Rgb(0xe0, 0xaf, 0x68);
pub const BLUE: Color = Color::Rgb(0x7a, 0xa2, 0xf7);
pub const MAGENTA: Color = Color::Rgb(0xbb, 0x9a, 0xf7);
pub const CYAN: Color = Color::Rgb(0x7d, 0xcf, 0xff);
pub const ORANGE: Color = Color::Rgb(0xff, 0x9e, 0x64);

// ── Semantic aliases ───────────────────────────────────────────
pub const PROFIT: Color = GREEN;
pub const LOSS: Color = RED;
pub const BID: Color = GREEN;
pub const ASK: Color = RED;
pub const BUY_SIDE: Color = GREEN;
pub const SELL_SIDE: Color = RED;
pub const LABEL: Color = FG_DIM;
pub const VALUE: Color = FG;
pub const TITLE: Color = BLUE;

// ── Reusable styles ────────────────────────────────────────────

/// Standard block border style
pub fn border_style() -> Style {
    Style::default().fg(BORDER)
}

/// Block title style
pub fn title_style() -> Style {
    Style::default().fg(TITLE).add_modifier(Modifier::BOLD)
}

/// Label text (dim, descriptive)
pub fn label() -> Style {
    Style::default().fg(LABEL)
}

/// Value text (bright, informational)
pub fn value() -> Style {
    Style::default().fg(VALUE)
}

/// Positive PnL / green values
pub fn profit() -> Style {
    Style::default().fg(PROFIT)
}

/// Negative PnL / red values
pub fn loss() -> Style {
    Style::default().fg(LOSS)
}

/// Style for PnL based on sign
pub fn pnl_style(is_positive: bool) -> Style {
    if is_positive {
        profit()
    } else {
        loss()
    }
}

/// Header row in tables
pub fn table_header() -> Style {
    Style::default().fg(BLUE).add_modifier(Modifier::BOLD)
}

/// Highlighted/selected row
pub fn highlight() -> Style {
    Style::default().bg(BG_HIGHLIGHT).fg(FG)
}
