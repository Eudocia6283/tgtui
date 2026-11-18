use crate::backend::model::{Attachment, MediaKind};
use crate::mode::AppMode;
use crate::ui::theme::Theme;
use crate::ui::utils::{cursor_line_col, layout_input_lines, truncate_str};
use ratatui::{
    layout::{Position, Rect},
    style::Modifier,
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};
use std::cell::Cell;
use unicode_width::UnicodeWidthStr;

const MAX_INPUT_LINES: usize = 5;

/// Height of the input box including borders.
pub fn height(input_buffer: &str, area_width: u16, replying: bool, attachments: usize) -> u16 {
    let text_width = area_width.saturating_sub(4).max(1) as usize;
    let lines = if input_buffer.is_empty() {
        1
    } else {
        layout_input_lines(input_buffer, text_width)
            .len()
            .clamp(1, MAX_INPUT_LINES)
    };
    2 + lines as u16 + replying as u16 + (attachments > 0) as u16
}

#[allow(clippy::too_many_arguments)]
pub fn render(
    f: &mut Frame,
    area: Rect,
    input_buffer: &str,
    input_cursor: usize,
    replying_to: Option<&str>,
    attachments: &[Attachment],
    mode: AppMode,
    is_focused: bool,
    input_width: &Cell<usize>,
    input_scroll: &Cell<usize>,
    theme: &Theme,
) {
    let is_insert = mode == AppMode::Insert;
    let border_style = if is_insert {
        theme.success
    } else if is_focused {
        theme.border_focus
    } else {
        theme.border_normal
    };

    let title = if is_insert {
        " Input [INSERT - Markdown Enabled] "
    } else if is_focused {
        " Input [H/J/K/L: cursor | i/a: edit] "
    } else {
        " Input "
    };

    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_type(theme.border_type)
        .border_style(border_style);
    let inner = block.inner(area);
    let width = inner.width as usize;
    let text_width = width.saturating_sub(2).max(1);
    input_width.set(text_width);

    let mut lines = Vec::new();

    if let Some(quote) = replying_to {
        let label = "┌─ Replying: ";
        let hint = "  [Esc to cancel]";
        let max_quote = width.saturating_sub(label.width() + hint.width() + 2);
        lines.push(Line::from(vec![
            Span::styled(label, theme.accent),
            Span::styled(
                format!("\"{}\"", truncate_str(quote, max_quote)),
                theme.reply_quote,
            ),
            Span::styled(hint, theme.text_muted),
        ]));
    }

    if !attachments.is_empty() {
        let names: Vec<String> = attachments
            .iter()
            .map(|a| {
                let icon = if a.kind == MediaKind::Image {
                    theme.icons.image
                } else {
                    theme.icons.file
                };
                let name = a
                    .path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                format!("{icon} {name}")
            })
            .collect();
        let hint = "  [Enter: send · text = caption]";
        let clip = format!("{} ", theme.icons.attachment);
        let max_names = width.saturating_sub(clip.width() + hint.width());
        lines.push(Line::from(vec![
            Span::styled(clip, theme.accent),
            Span::styled(truncate_str(&names.join(", "), max_names), theme.highlight),
            Span::styled(hint, theme.text_muted),
        ]));
    }

    if is_insert || is_focused || !input_buffer.is_empty() {
        let visual_lines = layout_input_lines(input_buffer, text_width);
        let (cursor_row, cursor_col) = cursor_line_col(input_buffer, &visual_lines, input_cursor);
        let avail_rows = (inner.height as usize).saturating_sub(lines.len()).max(1);

        let max_scroll = visual_lines.len().saturating_sub(avail_rows);
        let mut scroll = input_scroll.get().min(max_scroll);
        if cursor_row < scroll {
            scroll = cursor_row;
        } else if cursor_row >= scroll + avail_rows {
            scroll = cursor_row + 1 - avail_rows;
        }
        input_scroll.set(scroll);

        let input_start_row = lines.len();
        let prompt_style = if is_insert {
            theme.success.add_modifier(Modifier::BOLD)
        } else if is_focused {
            theme.accent.add_modifier(Modifier::BOLD)
        } else {
            theme.text_muted
        };
        for (vis_idx, vline) in visual_lines.iter().enumerate().skip(scroll).take(avail_rows) {
            let prefix = if vis_idx == 0 { "❯ " } else { "  " };
            let slice = &input_buffer[vline.start..vline.end];
            lines.push(Line::from(vec![
                Span::styled(prefix, prompt_style),
                Span::styled(slice.to_string(), theme.text_bold),
            ]));
        }

        if (is_insert || (mode == AppMode::Normal && is_focused)) && inner.width > 0 && inner.height > 0 {
            let visible_row = cursor_row.saturating_sub(scroll);
            let cursor_y = inner.y + input_start_row as u16 + visible_row as u16;
            let cursor_x = inner.x + 2 + cursor_col as u16;
            if cursor_y < inner.bottom() {
                f.set_cursor_position(Position::new(
                    cursor_x.min(inner.right().saturating_sub(1)),
                    cursor_y,
                ));
            }
        }
    } else {
        lines.push(Line::from(vec![
            Span::styled("› ", theme.text_muted),
            Span::styled(
                "Type a message... [i: insert | r: reply | e: editor | /: search | ?: help]",
                theme.text_muted,
            ),
        ]));
    }

    f.render_widget(Paragraph::new(lines).block(block), area);
}
