use ratatui::layout::{Constraint, Direction, Layout, Rect};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// A `width` x `height` rect centered in `r` (clamped to fit).
pub fn centered_fixed(width: u16, height: u16, r: Rect) -> Rect {
    let w = width.min(r.width);
    let h = height.min(r.height);
    Rect::new(r.x + (r.width - w) / 2, r.y + (r.height - h) / 2, w, h)
}

pub fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(r);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}

pub fn truncate_str(s: &str, max_width: usize) -> String {
    if UnicodeWidthStr::width(s) <= max_width {
        return s.to_string();
    }
    let mut current = String::new();
    let mut w = 0;
    for c in s.chars() {
        let cw = UnicodeWidthChar::width(c).unwrap_or(0);
        if w + cw + 1 > max_width {
            current.push('…');
            break;
        }
        current.push(c);
        w += cw;
    }
    current
}

/// Wrap `text` into lines of at most `width` display columns. Breaks at
/// spaces when possible, otherwise mid-word (needed for CJK text).
pub fn wrap_text(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut out = Vec::new();
    for raw_line in text.split('\n') {
        let mut line = String::new();
        let mut line_w = 0;
        // Byte index in `line` just after the last space, and the width up to it.
        let mut last_space: Option<(usize, usize)> = None;
        for c in raw_line.chars() {
            let cw = UnicodeWidthChar::width(c).unwrap_or(0);
            if line_w + cw > width && !line.is_empty() {
                if c == ' ' {
                    // Break exactly at this space and drop it.
                    out.push(std::mem::take(&mut line));
                    line_w = 0;
                    last_space = None;
                    continue;
                }
                match last_space {
                    Some((idx, w)) if idx < line.len() => {
                        let rest = line.split_off(idx);
                        out.push(line.trim_end().to_string());
                        line = rest;
                        line_w -= w;
                    }
                    _ => {
                        out.push(std::mem::take(&mut line));
                        line_w = 0;
                    }
                }
                last_space = None;
            }
            line.push(c);
            line_w += cw;
            if c == ' ' {
                last_space = Some((line.len(), line_w));
            }
        }
        out.push(line);
    }
    out
}

/// A visual line within an input buffer, expressed as byte offsets `[start, end)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InputLine {
    pub start: usize,
    pub end: usize,
}

/// Wrap `text` at `\n` and at `width` display columns for single-line/multi-line input layout.
pub fn layout_input_lines(text: &str, width: usize) -> Vec<InputLine> {
    let width = width.max(1);
    let mut lines = Vec::new();
    let mut line_start = 0;
    let mut line_w = 0;

    for (i, c) in text.char_indices() {
        if c == '\n' {
            lines.push(InputLine {
                start: line_start,
                end: i,
            });
            line_start = i + 1;
            line_w = 0;
            continue;
        }
        let cw = UnicodeWidthChar::width(c).unwrap_or(0);
        if line_w + cw > width && i > line_start {
            lines.push(InputLine {
                start: line_start,
                end: i,
            });
            line_start = i;
            line_w = cw;
        } else {
            line_w += cw;
        }
    }
    lines.push(InputLine {
        start: line_start,
        end: text.len(),
    });
    lines
}

/// Convert a byte offset `cursor` in `text` into `(visual_row, display_col)`.
pub fn cursor_line_col(text: &str, lines: &[InputLine], cursor: usize) -> (usize, usize) {
    let cursor = cursor.min(text.len());
    for (r, line) in lines.iter().enumerate() {
        let wrapped_into_next = lines.get(r + 1).is_some_and(|next| next.start == line.end);
        let in_line = if wrapped_into_next {
            cursor >= line.start && cursor < line.end
        } else {
            cursor >= line.start && cursor <= line.end
        };
        if in_line {
            let col = UnicodeWidthStr::width(&text[line.start..cursor]);
            return (r, col);
        }
    }
    let last_r = lines.len().saturating_sub(1);
    let last_col = lines
        .last()
        .map(|l| UnicodeWidthStr::width(&text[l.start..l.end]))
        .unwrap_or(0);
    (last_r, last_col)
}

/// Convert a `(line, target_col)` back to a byte offset in `text`.
/// If `wrapped_into_next` is true (no `\n` at `line.end`), the returned byte
/// offset stays strictly `< line.end` so it remains on `line`.
pub fn line_col_to_byte(text: &str, line: InputLine, target_col: usize, wrapped_into_next: bool) -> usize {
    let slice = &text[line.start..line.end];
    let mut w = 0;
    let mut last_char_start = None;
    for (rel, c) in slice.char_indices() {
        if w >= target_col {
            return line.start + rel;
        }
        last_char_start = Some(line.start + rel);
        w += UnicodeWidthChar::width(c).unwrap_or(0);
    }
    if wrapped_into_next {
        last_char_start.unwrap_or(line.start)
    } else {
        line.end
    }
}

/// The longest suffix of `s` that fits in `max_width` columns.
pub fn tail_fit(s: &str, max_width: usize) -> &str {
    let mut w = 0;
    for (i, c) in s.char_indices().rev() {
        w += UnicodeWidthChar::width(c).unwrap_or(0);
        if w > max_width {
            return &s[i + c.len_utf8()..];
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrap_breaks_at_spaces() {
        assert_eq!(wrap_text("hello world foo", 11), vec!["hello world", "foo"]);
        assert_eq!(wrap_text("hello world", 7), vec!["hello", "world"]);
    }

    #[test]
    fn wrap_handles_cjk_and_newlines() {
        // Each CJK char is 2 columns wide.
        assert_eq!(wrap_text("你好世界", 4), vec!["你好", "世界"]);
        assert_eq!(wrap_text("a\nb", 10), vec!["a", "b"]);
        assert_eq!(wrap_text("", 10), vec![""]);
    }

    #[test]
    fn wrap_splits_long_words() {
        assert_eq!(wrap_text("abcdefgh", 3), vec!["abc", "def", "gh"]);
    }

    #[test]
    fn tail_fit_keeps_suffix() {
        assert_eq!(tail_fit("abcdef", 3), "def");
        assert_eq!(tail_fit("你好世界", 5), "世界");
        assert_eq!(tail_fit("ab", 5), "ab");
    }

    #[test]
    fn truncate_adds_ellipsis() {
        assert_eq!(truncate_str("abcdef", 4), "abc…");
        assert_eq!(truncate_str("abc", 4), "abc");
    }

    #[test]
    fn input_layout_and_cursor_mapping() {
        let text = "abcd\nef";
        let lines = layout_input_lines(text, 3);
        // "abc", "d", "ef"
        assert_eq!(
            lines,
            vec![
                InputLine { start: 0, end: 3 },
                InputLine { start: 3, end: 4 },
                InputLine { start: 5, end: 7 },
            ]
        );
        // Byte 3 wraps to start of line 1
        assert_eq!(cursor_line_col(text, &lines, 3), (1, 0));
        // Byte 4 is before '\n', stays at end of line 1
        assert_eq!(cursor_line_col(text, &lines, 4), (1, 1));
        // Byte 5 is start of line 2
        assert_eq!(cursor_line_col(text, &lines, 5), (2, 0));
        // Moving up from (2, 1) to line 1 (which ends at \n) lands at byte 4
        assert_eq!(line_col_to_byte(text, lines[1], 1, false), 4);
        // Moving up from (1, 1) to wrapped line 0 clamps inside line 0 (byte 2, not byte 3)
        assert_eq!(line_col_to_byte(text, lines[0], 2, true), 2);
        assert_eq!(cursor_line_col(text, &lines, 2), (0, 2));
    }
}
