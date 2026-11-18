use chrono::{DateTime, Datelike, Local};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChatType {
    Direct,
    Group,
    Channel,
    Bot,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Chat {
    pub id: i64,
    pub title: String,
    pub username: Option<String>,
    pub chat_type: ChatType,
    pub unread_count: u32,
    pub is_pinned: bool,
    pub is_muted: bool,
    pub last_message_preview: String,
    pub last_message_time: String,
    pub members_count: Option<u32>,
    /// Position in the main chat list (higher = closer to top). 0 = not in list.
    pub order: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplyInfo {
    pub sender_name: String,
    pub text_preview: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MediaKind {
    /// Opened with `image_viewer`.
    Image,
    /// Anything else, opened with `open_command`.
    File,
}

/// A small still image shown inline (photo size or thumbnail).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Preview {
    pub file_id: i32,
    /// Set when the file is already downloaded.
    pub local_path: Option<std::path::PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Media {
    /// Full-size file, opened with `o`.
    pub file_id: i32,
    pub kind: MediaKind,
    pub preview: Option<Preview>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum SendState {
    #[default]
    Sent,
    Pending,
    Failed(String),
}

/// A local file queued for sending.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Attachment {
    pub path: std::path::PathBuf,
    /// Image = send as photo, File = send as document.
    pub kind: MediaKind,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub id: i64,
    pub chat_id: i64,
    pub sender_name: String,
    pub text: String,
    /// Additional URLs attached via message entities (e.g. text links) or previews.
    #[serde(default)]
    pub urls: Vec<String>,
    /// Unix timestamp (seconds)
    pub date: i64,
    pub time_str: String,
    pub is_outgoing: bool,
    pub is_read: bool,
    pub reply_to: Option<ReplyInfo>,
    pub media: Option<Media>,
    pub send_state: SendState,
}

impl Message {
    /// All web links in this message (from text and rich-text entities), deduplicated.
    pub fn links(&self) -> Vec<String> {
        let mut out = extract_urls(&self.text);
        for url in &self.urls {
            if !url.is_empty() && !out.contains(url) {
                out.push(url.clone());
            }
        }
        out
    }

    /// Whether pressing `o` can open media or a web link for this message.
    pub fn has_openable(&self) -> bool {
        self.media.is_some() || !self.links().is_empty()
    }
}

/// Extract HTTP(S) and `www.` web links from plain message text.
pub fn extract_urls(text: &str) -> Vec<String> {
    let mut urls = Vec::new();
    let mut i = 0;
    while i < text.len() {
        let rest = &text[i..];
        let is_scheme = rest.starts_with("https://") || rest.starts_with("http://");
        let is_www = !is_scheme
            && rest.starts_with("www.")
            && text[..i].chars().next_back().is_none_or(|c| {
                c.is_whitespace()
                    || matches!(c, '(' | '[' | '{' | '<' | '"' | '\'' | '（' | '【' | '《' | '：')
            });

        if !is_scheme && !is_www {
            let Some(ch) = rest.chars().next() else { break };
            i += ch.len_utf8();
            continue;
        }

        let end = rest
            .char_indices()
            .find(|&(_, c)| {
                c.is_whitespace()
                    || c.is_control()
                    || matches!(
                        c,
                        '<' | '>'
                            | '"'
                            | '\''
                            | '`'
                            | '{'
                            | '}'
                            | '|'
                            | '\\'
                            | '^'
                            | '，'
                            | '。'
                            | '；'
                            | '：'
                            | '！'
                            | '？'
                            | '、'
                            | '（'
                            | '）'
                            | '【'
                            | '】'
                            | '《'
                            | '》'
                            | '「'
                            | '」'
                            | '『'
                            | '』'
                            | '“'
                            | '”'
                            | '‘'
                            | '’'
                    )
            })
            .map(|(idx, _)| idx)
            .unwrap_or(rest.len());
        let mut raw = &rest[..end];

        loop {
            let trimmed = raw.trim_end_matches(|c: char| {
                matches!(
                    c,
                    '.' | ','
                        | ';'
                        | ':'
                        | '!'
                        | '?'
                        | '}'
                        | '。'
                        | '，'
                        | '；'
                        | '：'
                        | '！'
                        | '？'
                        | '）'
                        | '】'
                        | '》'
                )
            });
            let after_paren = if let Some(stripped) = trimmed.strip_suffix(')') {
                let open = trimmed.matches('(').count();
                let close = trimmed.matches(')').count();
                if close > open {
                    stripped
                } else {
                    trimmed
                }
            } else {
                trimmed
            };
            let after_bracket = if let Some(stripped) = after_paren.strip_suffix(']') {
                let open = after_paren.matches('[').count();
                let close = after_paren.matches(']').count();
                if close > open {
                    stripped
                } else {
                    after_paren
                }
            } else {
                after_paren
            };
            if after_bracket.len() == raw.len() {
                break;
            }
            raw = after_bracket;
        }

        let valid = if is_scheme {
            let prefix_len = if raw.starts_with("https://") { 8 } else { 7 };
            raw.len() > prefix_len
        } else {
            // `www.<domain>.<tld>`
            raw.strip_prefix("www.")
                .is_some_and(|host| !host.is_empty() && host.contains('.'))
        };

        if valid {
            let normalized = if is_www {
                format!("https://{raw}")
            } else {
                raw.to_string()
            };
            if !urls.contains(&normalized) {
                urls.push(normalized);
            }
        }
        i += end.max(1);
    }
    urls
}

/// `HH:MM` for messages.
pub fn format_message_time(ts: i64) -> String {
    DateTime::from_timestamp(ts, 0)
        .map(|dt| dt.with_timezone(&Local).format("%H:%M").to_string())
        .unwrap_or_default()
}

/// Compact chat-list timestamp: `HH:MM` today, `MM-DD` this year, else `YYYY-MM-DD`.
pub fn format_chat_time(ts: i64) -> String {
    let Some(dt) = DateTime::from_timestamp(ts, 0).map(|d| d.with_timezone(&Local)) else {
        return String::new();
    };
    let now = Local::now();
    if dt.date_naive() == now.date_naive() {
        dt.format("%H:%M").to_string()
    } else if dt.year() == now.year() {
        dt.format("%m-%d").to_string()
    } else {
        dt.format("%Y-%m-%d").to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_urls_and_strips_trailing_punctuation() {
        assert_eq!(
            extract_urls("Check https://example.com/a?x=1&y=2. Also http://foo.bar, and www.rust-lang.org!"),
            vec![
                "https://example.com/a?x=1&y=2",
                "http://foo.bar",
                "https://www.rust-lang.org",
            ]
        );
        assert_eq!(
            extract_urls("(see https://en.wikipedia.org/wiki/Rust_(programming_language))"),
            vec!["https://en.wikipedia.org/wiki/Rust_(programming_language)"]
        );
        assert_eq!(
            extract_urls("看看这个https://example.com/path，还有[链接](https://test.org/abc)"),
            vec!["https://example.com/path", "https://test.org/abc"]
        );
        assert!(extract_urls("just https:// or www. or foo.www.bar").is_empty());
    }
}
