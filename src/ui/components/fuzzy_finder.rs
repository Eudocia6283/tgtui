use crate::backend::model::{Chat, ChatType};
use crate::ui::theme::Theme;
use crate::ui::utils::{centered_rect, tail_fit};
use ratatui::{
    layout::{Constraint, Layout, Position, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph},
    Frame,
};
use std::cell::Cell;
use unicode_width::UnicodeWidthStr;

pub fn render(
    f: &mut Frame,
    area: Rect,
    query: &str,
    matches: &[&Chat],
    selected_idx: usize,
    scroll: &Cell<usize>,
    theme: &Theme,
) {
    let popup_area = centered_rect(65, 45, area);
    f.render_widget(Clear, popup_area);

    let block = Block::default()
        .title(format!(" Jump to Chat / Contact ({}) ", matches.len()))
        .borders(Borders::ALL)
        .border_type(theme.border_type)
        .border_style(theme.highlight);
    let inner = block.inner(popup_area);
    f.render_widget(block, popup_area);

    let [prompt_area, sep_area, list_area] =
        Layout::vertical([Constraint::Length(1), Constraint::Length(1), Constraint::Min(0)]).areas(inner);

    // Query prompt line with the real terminal cursor at the end.
    let shown_query = tail_fit(query, (prompt_area.width as usize).saturating_sub(3));
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("❯ ", theme.accent.add_modifier(Modifier::BOLD)),
            Span::styled(shown_query, theme.text_bold),
        ])),
        prompt_area,
    );
    f.set_cursor_position(Position::new(
        prompt_area.x + 2 + shown_query.width() as u16,
        prompt_area.y,
    ));
    f.render_widget(
        Paragraph::new("─".repeat(sep_area.width as usize)).style(theme.border_normal),
        sep_area,
    );

    let items: Vec<ListItem> = matches
        .iter()
        .enumerate()
        .map(|(i, chat)| {
            let is_selected = i == selected_idx;
            let (prefix, prefix_style, title_style, style) = if is_selected {
                (
                    "❯ ",
                    theme.accent.add_modifier(Modifier::BOLD),
                    theme.text_bold,
                    theme.selection_bg,
                )
            } else {
                ("  ", theme.text_muted, theme.text_normal, Style::default())
            };
            let icon = match chat.chat_type {
                ChatType::Group => theme.icons.group,
                ChatType::Channel => theme.icons.channel,
                ChatType::Bot => theme.icons.bot,
                ChatType::Direct => theme.icons.user,
            };
            let mut spans = vec![
                Span::styled(prefix, prefix_style),
                Span::styled(format!("{icon} "), theme.accent),
                Span::styled(chat.title.as_str(), title_style),
            ];
            if let Some(u) = &chat.username {
                spans.push(Span::styled(format!("  @{u}"), theme.text_muted));
            }
            spans.push(Span::styled(
                format!("  [{:?}]", chat.chat_type),
                theme.highlight_alt,
            ));
            ListItem::new(Line::from(spans)).style(style)
        })
        .collect();

    let mut state = ListState::default()
        .with_offset(scroll.get())
        .with_selected((!matches.is_empty()).then_some(selected_idx));
    f.render_stateful_widget(List::new(items), list_area, &mut state);
    scroll.set(state.offset());
}
