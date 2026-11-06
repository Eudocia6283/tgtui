use crate::backend::model::{Chat, Message, SendState};
use crate::images::ImageStore;
use crate::ui::kitty;
use crate::ui::theme::Theme;
use crate::ui::utils::{truncate_str, wrap_text};
use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Paragraph},
    Frame,
};
use std::cell::Cell;
use unicode_width::UnicodeWidthStr;

/// Bubbles take at most this share of the pane width.
const BUBBLE_MAX_PERCENT: usize = 75;
const BUBBLE_MIN_WIDTH: usize = 16;
/// Tallest inline image, in rows.
const IMAGE_MAX_ROWS: usize = 15;

/// "[Photo] caption" -> "caption" (the image itself replaces the label).
fn strip_media_label(text: &str) -> &str {
    match text.strip_prefix('[').and_then(|t| t.split_once(']')) {
        Some((_, rest)) => rest.trim_start(),
        None => text,
    }
}

struct Corners {
    tl: &'static str,
    tr: &'static str,
    bl: &'static str,
    br: &'static str,
}

fn corners(border_type: BorderType) -> Corners {
    match border_type {
        BorderType::Rounded => Corners { tl: "╭", tr: "╮", bl: "╰", br: "╯" },
        _ => Corners { tl: "┌", tr: "┐", bl: "└", br: "┘" },
    }
}

/// Render one message as a bordered bubble: own messages right-aligned,
/// others left-aligned. Sender/time sit in the top border.
#[allow(clippy::too_many_arguments)]
fn bubble<'a>(
    msg: &'a Message,
    is_selected: bool,
    pane_width: usize,
    max_image_rows: usize,
    images: &ImageStore,
    theme: &Theme,
    out: &mut Vec<Line<'a>>,
) {
    let c = corners(theme.border_type);
    let max_bubble = (pane_width * BUBBLE_MAX_PERCENT / 100)
        .max(BUBBLE_MIN_WIDTH)
        .min(pane_width)
        .max(5);
    // "│ " + content + " │"
    let max_content = max_bubble.saturating_sub(4).max(1);

    let border = if is_selected {
        theme.border_focus.add_modifier(Modifier::BOLD)
    } else if msg.is_outgoing {
        theme.text_sender_me.remove_modifier(Modifier::BOLD)
    } else {
        theme.border_normal
    };
    let body_style = if is_selected { theme.text_bold } else { theme.text_normal };

    // Header shown in the top border.
    let media_icon = if msg.media.is_some() { "📎 " } else { "" };
    let mut header: Vec<Span> = Vec::new();
    if msg.is_outgoing {
        header.push(Span::styled(format!("{media_icon}{}", msg.time_str), theme.text_muted));
        header.push(match msg.send_state {
            SendState::Pending => Span::styled(" ⏳", theme.text_muted),
            SendState::Failed(_) => Span::styled(" ✗", theme.error),
            SendState::Sent if msg.is_read => Span::styled(" ✓✓", theme.accent),
            SendState::Sent => Span::styled(" ✓", theme.accent),
        });
    } else {
        let name = truncate_str(&msg.sender_name, max_bubble.saturating_sub(13).max(1));
        header.push(Span::styled(name, theme.text_sender_other));
        header.push(Span::styled(format!("  {media_icon}{}", msg.time_str), theme.text_muted));
    }
    let header_w: usize = header.iter().map(|s| s.width()).sum();

    let reply = msg.reply_to.as_ref().map(|r| {
        let text = format!("┃ {}: {}", r.sender_name, r.text_preview.replace('\n', " "));
        truncate_str(&text, max_content)
    });
    // Inline image (kitty), once its preview is downloaded and decoded.
    let image = msg
        .media
        .as_ref()
        .and_then(|m| m.preview.as_ref())
        .and_then(|p| images.ready(p.file_id))
        .map(|(id, w, h)| {
            let (cols, rows) = kitty::fit_cells(w, h, images.cell_px.get(), max_content, max_image_rows);
            images.request_placement(id, cols, rows);
            (id, cols, rows)
        });
    let text = if image.is_some() { strip_media_label(&msg.text) } else { msg.text.as_str() };
    let mut body = if text.is_empty() { Vec::new() } else { wrap_text(text, max_content) };
    let failure = match &msg.send_state {
        SendState::Failed(e) => wrap_text(&format!("⚠ {e}"), max_content),
        _ => Vec::new(),
    };
    if body.is_empty() && image.is_none() {
        body.push(String::new());
    }
    let hint = (is_selected && msg.media.is_some()).then_some(" o: open ");

    let content_w = body
        .iter()
        .map(|l| l.width())
        .chain(failure.iter().map(|l| l.width()))
        .chain(reply.iter().map(|r| r.width()))
        .chain(image.map(|(_, cols, _)| cols))
        .max()
        .unwrap_or(0);
    // Top border is "╭─ " + header + " " + fill + "╮".
    let bubble_w = (content_w + 4)
        .max(header_w + 5)
        .max(hint.map(|h| h.width() + 3).unwrap_or(0))
        .min(max_bubble);
    let inner_w = bubble_w - 4;
    let indent = if msg.is_outgoing { pane_width.saturating_sub(bubble_w) } else { 0 };
    let pad = || Span::raw(" ".repeat(indent));

    // Top border
    let fill = bubble_w.saturating_sub(header_w + 5);
    let mut top = vec![pad(), Span::styled(format!("{}─ ", c.tl), border)];
    top.extend(header);
    top.push(Span::styled(format!(" {}{}", "─".repeat(fill), c.tr), border));
    out.push(Line::from(top));

    let row = |text: String, style: Style| {
        let gap = inner_w.saturating_sub(text.width());
        Line::from(vec![
            pad(),
            Span::styled("│ ", border),
            Span::styled(text, style),
            Span::raw(" ".repeat(gap)),
            Span::styled(" │", border),
        ])
    };

    if let Some(r) = reply {
        out.push(row(truncate_str(&r, inner_w), theme.reply_quote));
    }
    if let Some((id, cols, rows)) = image {
        for r in 0..rows {
            out.push(Line::from(vec![
                pad(),
                Span::styled("│ ", border),
                kitty::placeholder_row(id, r, cols),
                Span::raw(" ".repeat(inner_w.saturating_sub(cols))),
                Span::styled(" │", border),
            ]));
        }
    }
    for line in body {
        out.push(row(truncate_str(&line, inner_w), body_style));
    }
    for line in failure {
        out.push(row(truncate_str(&line, inner_w), theme.error));
    }

    // Bottom border (with an "open media" hint on the selected bubble)
    let bottom = match hint {
        Some(h) => format!("{}─{}{}{}", c.bl, h, "─".repeat(bubble_w.saturating_sub(h.width() + 3)), c.br),
        None => format!("{}{}{}", c.bl, "─".repeat(bubble_w - 2), c.br),
    };
    out.push(Line::from(vec![pad(), Span::styled(bottom, border)]));
}

#[allow(clippy::too_many_arguments)]
pub fn render(
    f: &mut Frame,
    area: Rect,
    active_chat: Option<&Chat>,
    messages: &[Message],
    selected_msg_idx: usize,
    is_focused: bool,
    scroll: &Cell<usize>,
    images: &ImageStore,
    theme: &Theme,
) {
    let title = match active_chat {
        Some(chat) => match chat.members_count {
            Some(members) => format!(" {} ({} members) ", chat.title, members),
            None => format!(" {} ", chat.title),
        },
        None => " No Chat Selected ".to_string(),
    };

    let border_style = if is_focused {
        theme.border_focus
    } else {
        theme.border_normal
    };

    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_type(theme.border_type)
        .border_style(border_style);

    let inner = block.inner(area);
    // One column of margin on each side of the bubbles.
    let pane_width = (inner.width as usize).saturating_sub(2);

    let max_image_rows = IMAGE_MAX_ROWS.min((inner.height as usize).saturating_sub(3)).max(1);

    let mut lines: Vec<Line> = Vec::new();
    // Line range [start, end) of the selected message, to keep it in view.
    let mut selected_range = (0, 0);

    if messages.is_empty() && active_chat.is_some() {
        lines.push(Line::from(Span::styled("No messages loaded yet…", theme.text_muted)));
    }

    for (i, msg) in messages.iter().enumerate() {
        let is_selected = i == selected_msg_idx;
        let start = lines.len();
        bubble(msg, is_selected, pane_width, max_image_rows, images, theme, &mut lines);
        if is_selected {
            selected_range = (start, lines.len());
        }
    }

    // Keep the selected message visible, preserving the previous offset when possible.
    let height = inner.height as usize;
    let max_offset = lines.len().saturating_sub(height);
    let mut offset = scroll.get().min(max_offset);
    let (sel_start, sel_end) = selected_range;
    if sel_end > offset + height {
        offset = sel_end.saturating_sub(height);
    }
    if sel_start < offset {
        offset = sel_start;
    }
    let offset = offset.min(max_offset);
    scroll.set(offset);

    let visible: Vec<Line> = lines
        .into_iter()
        .skip(offset)
        .take(height)
        .map(|mut l| {
            l.spans.insert(0, Span::raw(" "));
            l
        })
        .collect();
    f.render_widget(Paragraph::new(visible).block(block), area);
}
