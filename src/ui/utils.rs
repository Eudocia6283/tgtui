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
}
