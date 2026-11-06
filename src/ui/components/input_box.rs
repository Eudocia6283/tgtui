use crate::backend::model::{Attachment, MediaKind};
use crate::mode::AppMode;
use crate::ui::theme::Theme;
use crate::ui::utils::{tail_fit, truncate_str};
use ratatui::{
    layout::{Position, Rect},
    style::Modifier,
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};
use unicode_width::UnicodeWidthStr;

/// Height of the input box including borders.
pub fn height(replying: bool, attachments: usize) -> u16 {
    3 + replying as u16 + (attachments > 0) as u16
}

pub fn render(
    f: &mut Frame,
    area: Rect,
    input_buffer: &str,
    replying_to: Option<&str>,
    attachments: &[Attachment],
    mode: AppMode,
    theme: &Theme,
) {
    let is_insert = mode == AppMode::Insert;
    let border_style = if is_insert {
        theme.success
    } else {
        theme.border_normal
    };

    let title = if is_insert {
        " Input [INSERT - Markdown Enabled] "
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

    let mut lines = Vec::new();

    if let Some(quote) = replying_to {
        let label = "┌─ Replying: ";
        let hint = "  [Esc to cancel]";
        let max_quote = width.saturating_sub(label.width() + hint.width() + 2);
        lines.push(Line::from(vec![
            Span::styled(label, theme.accent),
            Span::styled(format!("\"{}\"", truncate_str(quote, max_quote)), theme.reply_quote),
            Span::styled(hint, theme.text_muted),
        ]));
    }

    if !attachments.is_empty() {
        let names: Vec<String> = attachments
            .iter()
            .map(|a| {
                let icon = if a.kind == MediaKind::Image { "🖼 " } else { "📄 " };
                let name = a.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                format!("{icon}{name}")
            })
            .collect();
        let hint = "  [Enter: send · text = caption]";
        let max_names = width.saturating_sub("📎 ".width() + hint.width());
        lines.push(Line::from(vec![
            Span::styled("📎 ", theme.accent),
            Span::styled(truncate_str(&names.join(", "), max_names), theme.highlight),
            Span::styled(hint, theme.text_muted),
        ]));
    }

    if is_insert || !input_buffer.is_empty() {
        // Newlines (from paste / editor) are shown as ⏎ on the single input line.
        let flat = input_buffer.replace('\n', "⏎");
        let shown = tail_fit(&flat, width.saturating_sub(3));
        let cursor_x = inner.x + 2 + shown.width() as u16;
        lines.push(Line::from(vec![
            Span::styled(
                "❯ ",
                if is_insert {
                    theme.success.add_modifier(Modifier::BOLD)
                } else {
                    theme.text_muted
                },
            ),
            Span::styled(shown.to_string(), theme.text_bold),
        ]));
        if is_insert {
            let cursor_y = inner.y + lines.len() as u16 - 1;
            if cursor_y < inner.bottom() {
                f.set_cursor_position(Position::new(cursor_x.min(inner.right().saturating_sub(1)), cursor_y));
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
