use crate::config::UiConfig;
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::BorderType;

/// Raw colors a theme is built from.
struct Palette {
    fg: Color,
    muted: Color,
    on_accent: Color,
    accent: Color,
    accent_light: Color,
    magenta: Color,
    magenta_light: Color,
    green: Color,
    yellow: Color,
    red: Color,
    blue: Color,
    selection: Color,
    chip_bg: Color,
    chip_fg: Color,
}

impl Palette {
    /// Inherit the terminal's 16-color ANSI palette (pywal / base16 / Noctalia
    /// friendly). `Color::Reset` keeps the terminal's own foreground.
    fn terminal() -> Self {
        Self {
            fg: Color::Reset,
            muted: Color::DarkGray,
            on_accent: Color::Black,
            accent: Color::Cyan,
            accent_light: Color::LightCyan,
            magenta: Color::Magenta,
            magenta_light: Color::LightMagenta,
            green: Color::Green,
            yellow: Color::Yellow,
            red: Color::Red,
            blue: Color::Blue,
            selection: Color::Indexed(236),
            chip_bg: Color::DarkGray,
            chip_fg: Color::White,
        }
    }

    /// Built-in truecolor palette (Tokyo Night), independent of the terminal theme.
    fn builtin() -> Self {
        Self {
            fg: Color::Rgb(0xc0, 0xca, 0xf5),
            muted: Color::Rgb(0x56, 0x5f, 0x89),
            on_accent: Color::Rgb(0x1a, 0x1b, 0x26),
            accent: Color::Rgb(0x7d, 0xcf, 0xff),
            accent_light: Color::Rgb(0xb4, 0xf9, 0xf8),
            magenta: Color::Rgb(0xbb, 0x9a, 0xf7),
            magenta_light: Color::Rgb(0xff, 0x9e, 0xe4),
            green: Color::Rgb(0x9e, 0xce, 0x6a),
            yellow: Color::Rgb(0xe0, 0xaf, 0x68),
            red: Color::Rgb(0xf7, 0x76, 0x8e),
            blue: Color::Rgb(0x7a, 0xa2, 0xf7),
            selection: Color::Rgb(0x28, 0x34, 0x57),
            chip_bg: Color::Rgb(0x3b, 0x42, 0x61),
            chip_fg: Color::Rgb(0xc0, 0xca, 0xf5),
        }
    }
}

#[derive(Clone, Copy)]
pub struct Theme {
    pub border_normal: Style,
    pub border_focus: Style,
    pub text_normal: Style,
    pub text_muted: Style,
    pub text_bold: Style,
    pub text_sender_other: Style,
    pub text_sender_me: Style,
    pub selection_bg: Style,
    pub reply_quote: Style,
    pub badge_unread: Style,
    pub status_normal: Style,
    pub status_insert: Style,
    pub status_search: Style,
    pub status_confirm: Style,
    /// Selection markers, prompts, read ticks.
    pub accent: Style,
    /// INSERT mode, "online".
    pub success: Style,
    /// Status messages, "connecting…".
    pub warning: Style,
    /// Fuzzy finder border, chat type tags.
    pub highlight: Style,
    pub highlight_alt: Style,
    /// Active chat chip in the status bar.
    pub chip: Style,
    pub border_type: BorderType,
}

impl Theme {
    pub fn new(ui: &UiConfig) -> Self {
        let p = if ui.use_terminal_colors {
            Palette::terminal()
        } else {
            Palette::builtin()
        };
        let fg = |c: Color| Style::default().fg(c);
        let tag = |bg: Color, fg: Color| Style::default().bg(bg).fg(fg).add_modifier(Modifier::BOLD);

        Self {
            border_type: if ui.rounded_borders {
                BorderType::Rounded
            } else {
                BorderType::Plain
            },

            // Borders inherit terminal colors without forcing bg
            border_normal: fg(p.muted),
            border_focus: fg(p.accent),

            // Text styles - NO explicit bg to prevent punching black holes into selection
            text_normal: fg(p.fg),
            text_muted: fg(p.muted),
            text_bold: fg(p.fg).add_modifier(Modifier::BOLD),

            // Senders
            text_sender_other: fg(p.accent).add_modifier(Modifier::BOLD),
            text_sender_me: fg(p.magenta).add_modifier(Modifier::BOLD),

            // Message / Chat Selection highlight: subtle background only, so text inherits cleanly
            selection_bg: Style::default().bg(p.selection),
            reply_quote: fg(p.accent_light),

            badge_unread: tag(p.magenta_light, p.on_accent),

            // Status bar tags
            status_normal: tag(p.blue, p.chip_fg),
            status_insert: tag(p.green, p.on_accent),
            status_search: tag(p.magenta, p.chip_fg),
            status_confirm: tag(p.red, p.chip_fg),

            accent: fg(p.accent),
            success: fg(p.green),
            warning: fg(p.yellow),
            highlight: fg(p.magenta_light),
            highlight_alt: fg(p.magenta),
            chip: Style::default().bg(p.chip_bg).fg(p.chip_fg),
        }
    }
}
