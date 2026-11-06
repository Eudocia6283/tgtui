use crate::mode::AppMode;
use crate::ui::theme::Theme;
use ratatui::{
    layout::Rect,
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

#[allow(clippy::too_many_arguments)]
pub fn render(
    f: &mut Frame,
    area: Rect,
    mode: AppMode,
    active_chat_title: Option<&str>,
    unread_total: u32,
    connection: &str,
    status_msg: Option<&str>,
    theme: &Theme,
) {
    let mode_style = match mode {
        AppMode::Normal | AppMode::Help => theme.status_normal,
        AppMode::Insert => theme.status_insert,
        AppMode::FuzzyFind => theme.status_search,
        AppMode::ConfirmDelete => theme.status_confirm,
    };

    let conn_style = if connection == "online" || connection == "demo" {
        theme.success
    } else {
        theme.warning
    };

    let line1 = Line::from(vec![
        Span::styled(format!(" {} ", mode), mode_style),
        Span::styled(
            format!(" {} ", active_chat_title.unwrap_or("No Selection")),
            theme.chip,
        ),
        Span::styled(format!(" {} unread • ", unread_total), theme.text_muted),
        Span::styled(format!("● {connection}"), conn_style),
    ]);

    let hints = match mode {
        AppMode::Normal => " j/k: scroll • h/l: switch pane • r: reply • y: copy • o: open • d: delete • i: insert • /: search • ?: help • q: quit (service keeps running)",
        AppMode::Insert => " <Esc>: normal mode • <Enter>: send • <C-e>: editor • <C-v>: paste • <C-w>: del word • <C-u>: clear",
        AppMode::FuzzyFind => " Type query to filter • <Enter>: jump • <C-j/k>: navigate • <Esc>: close",
        AppMode::ConfirmDelete => " Delete the selected message? y: confirm • any other key: cancel",
        AppMode::Help => " <Esc>: return to normal mode",
    };

    let line2 = match status_msg {
        Some(msg) => Line::from(Span::styled(format!(" {msg}"), theme.warning)),
        None => Line::from(Span::styled(hints, theme.text_muted)),
    };

    f.render_widget(Paragraph::new(vec![line1, line2]), area);
}
