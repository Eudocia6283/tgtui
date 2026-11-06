//! Monochrome UI icons: Nerd Font glyphs, or plain Unicode for other fonts.
//! Every icon is one cell wide; callers put a space after it so Nerd Font
//! glyphs may overflow into that space.

#[derive(Clone, Copy)]
pub struct Icons {
    pub pin: &'static str,
    pub muted: &'static str,
    pub attachment: &'static str,
    pub image: &'static str,
    pub file: &'static str,
    pub user: &'static str,
    pub group: &'static str,
    pub channel: &'static str,
    pub bot: &'static str,
    pub warning: &'static str,
}

impl Icons {
    pub fn new(nerd_font: bool) -> Self {
        if nerd_font {
            Self {
                pin: "\u{f435}",        // nf-oct-pin
                muted: "\u{f1f6}",      // nf-fa-bell_slash
                attachment: "\u{f0c6}", // nf-fa-paperclip
                image: "\u{f03e}",      // nf-fa-image
                file: "\u{f15b}",       // nf-fa-file
                user: "\u{f007}",       // nf-fa-user
                group: "\u{f0c0}",      // nf-fa-users
                channel: "\u{f0a1}",    // nf-fa-bullhorn
                bot: "\u{f06a9}",       // nf-md-robot
                warning: "\u{f071}",    // nf-fa-warning
            }
        } else {
            Self {
                pin: "↑",
                muted: "∅",
                attachment: "+",
                image: "▣",
                file: "▤",
                user: "@",
                group: "#",
                channel: "»",
                bot: "¤",
                warning: "!",
            }
        }
    }
}
