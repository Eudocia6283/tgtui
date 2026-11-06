use crate::ui::theme::Theme;
use crate::ui::utils::centered_fixed;
use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
    Frame,
};

pub fn render(f: &mut Frame, area: Rect, theme: &Theme) {
    let heading =
        |text: &'static str, style: Style| Line::from(Span::styled(text, style.add_modifier(Modifier::BOLD)));
    let help_lines = vec![
        heading("NORMAL MODE (Navigation & Actions)", theme.accent),
        Line::from("  j / k       : Move down / up (messages or chat list)"),
        Line::from("  h / l       : Switch pane between Chats and Messages"),
        Line::from("  G / gg      : Jump to latest / oldest message (loads more)"),
        Line::from("  r           : Reply to selected message (mounts quote)"),
        Line::from("  y / Y       : Yank message (image / file / text) / text only"),
        Line::from("  o           : Open photo / file of selected message"),
        Line::from("  d           : Delete selected message (asks to confirm)"),
        Line::from("  i / a       : Enter INSERT mode to type a message"),
        Line::from("  e           : Compose in $EDITOR, sends on save & quit"),
        Line::from("  / or <Spc>f : Open Fuzzy Finder"),
        Line::from("  q / <C-c>   : Quit client"),
        Line::from(""),
        heading("INSERT MODE (Typing)", theme.success),
        Line::from("  <Enter>     : Send message"),
        Line::from("  <Esc>       : Return to NORMAL mode (drops reply/attachments)"),
        Line::from("  <C-e>       : Edit in $EDITOR and send upon :wq"),
        Line::from("  <C-v>       : Paste image / copied files / text (wl-paste)"),
        Line::from("  <C-w>       : Delete word backwards"),
        Line::from("  <C-u>       : Clear input"),
        Line::from(""),
        heading("FUZZY FINDER", theme.highlight_alt),
        Line::from("  <C-j>/<C-k> : Next / previous result (also ↓/↑)"),
        Line::from("  <Enter>     : Jump to chat    <Esc>: close"),
        Line::from(""),
        Line::from(Span::styled(
            "Press <Esc> or ? to close this help",
            theme.text_muted,
        )),
    ];

    let popup_area = centered_fixed(66, help_lines.len() as u16 + 2, area);
    f.render_widget(Clear, popup_area);

    let block = Block::default()
        .title(" Keyboard Cheat Sheet & Vim Keybindings ")
        .borders(Borders::ALL)
        .border_type(theme.border_type)
        .border_style(theme.warning);

    f.render_widget(
        Paragraph::new(help_lines).block(block).wrap(Wrap { trim: false }),
        popup_area,
    );
}
