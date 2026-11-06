use crate::backend::model::Chat;
use crate::ui::theme::Theme;
use crate::ui::utils::truncate_str;
use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState},
    Frame,
};
use std::cell::Cell;
use unicode_width::UnicodeWidthStr;

/// `left` and `right` spans with padding between so `right` is right-aligned.
fn justify<'a>(mut left: Vec<Span<'a>>, right: Vec<Span<'a>>, width: usize) -> Line<'a> {
    let used: usize = left.iter().chain(right.iter()).map(|s| s.width()).sum();
    left.push(Span::raw(" ".repeat(width.saturating_sub(used))));
    left.extend(right);
    Line::from(left)
}

pub fn render(
    f: &mut Frame,
    area: Rect,
    chats: &[Chat],
    selected_idx: usize,
    is_focused: bool,
    scroll: &Cell<usize>,
    theme: &Theme,
) {
    let border_style = if is_focused {
        theme.border_focus
    } else {
        theme.border_normal
    };

    let block = Block::default()
        .title(format!(" Chats ({}) ", chats.len()))
        .borders(Borders::ALL)
        .border_type(theme.border_type)
        .border_style(border_style);

    let inner_width = area.width.saturating_sub(2) as usize;

    let items: Vec<ListItem> = chats
        .iter()
        .enumerate()
        .map(|(i, chat)| {
            let is_selected = i == selected_idx;

            // Line 1: marker + pin + name ............ muted + time
            let (marker, marker_style, name_style) = if is_selected {
                ("❯ ", theme.accent.add_modifier(Modifier::BOLD), theme.text_bold)
            } else {
                ("  ", theme.text_muted, theme.text_normal)
            };
            let icons = &theme.icons;
            let pin_icon = if chat.is_pinned {
                format!("{} ", icons.pin)
            } else {
                String::new()
            };
            let mute_icon = if chat.is_muted {
                format!("{} ", icons.muted)
            } else {
                String::new()
            };
            let max_name = inner_width.saturating_sub(
                marker.width() + pin_icon.width() + mute_icon.width() + chat.last_message_time.width() + 1,
            );
            let line1 = justify(
                vec![
                    Span::styled(marker, marker_style),
                    Span::styled(pin_icon, theme.accent),
                    Span::styled(truncate_str(&chat.title, max_name), name_style),
                ],
                vec![
                    Span::styled(mute_icon, theme.text_muted),
                    Span::styled(chat.last_message_time.as_str(), theme.text_muted),
                ],
                inner_width,
            );

            // Line 2: last message preview ............ unread badge
            let badge = (chat.unread_count > 0).then(|| format!(" {} ", chat.unread_count));
            let badge_width = badge.as_ref().map(|b| b.width() + 1).unwrap_or(0);
            let max_preview = inner_width.saturating_sub(2 + badge_width);
            let right2 = badge
                .map(|b| {
                    let style = if chat.is_muted {
                        theme.text_muted.add_modifier(Modifier::REVERSED)
                    } else {
                        theme.badge_unread
                    };
                    vec![Span::styled(b, style)]
                })
                .unwrap_or_default();
            let line2 = justify(
                vec![
                    Span::raw("  "),
                    Span::styled(
                        truncate_str(&chat.last_message_preview.replace('\n', " "), max_preview),
                        theme.text_muted,
                    ),
                ],
                right2,
                inner_width,
            );

            let item_style = if is_selected {
                theme.selection_bg
            } else {
                Style::default()
            };
            // Highlight only the two text lines so the separator stays plain.
            let mut lines = vec![line1.style(item_style), line2.style(item_style)];
            if i + 1 < chats.len() {
                lines.push(Line::styled(
                    "┄".repeat(inner_width),
                    theme.border_normal.add_modifier(Modifier::DIM),
                ));
            }
            ListItem::new(lines)
        })
        .collect();

    let mut state = ListState::default()
        .with_offset(scroll.get())
        .with_selected((!chats.is_empty()).then_some(selected_idx));
    f.render_stateful_widget(List::new(items).block(block), area, &mut state);
    scroll.set(state.offset());
}
