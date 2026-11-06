//! Kitty graphics protocol with Unicode placeholders.
//!
//! Images are transmitted once and given a *virtual* placement of
//! `cols x rows` cells. They are then shown by drawing placeholder
//! characters (U+10EEEE + row/column diacritics, foreground color = image
//! id) into the ratatui buffer, so they scroll and clip like normal text.
//! https://sw.kovidgoyal.net/kitty/graphics-protocol/#unicode-placeholders

use base64::Engine;
use ratatui::style::{Color, Style};
use ratatui::text::Span;
use std::io::{self, Write};

pub const PLACEHOLDER: char = '\u{10EEEE}';

/// Largest image that can be addressed (rows/cols are encoded by index).
pub const MAX_CELLS: usize = DIACRITICS.len();

/// Whether the terminal speaks the kitty graphics protocol with placeholders.
pub fn is_supported() -> bool {
    let env = |k: &str| std::env::var(k).unwrap_or_default();
    !env("KITTY_WINDOW_ID").is_empty()
        || env("TERM") == "xterm-kitty"
        || env("TERM_PROGRAM").eq_ignore_ascii_case("ghostty")
}

/// Size of one terminal cell in pixels (falls back to a typical 1:2 cell).
pub fn cell_size_px() -> (u16, u16) {
    match crossterm::terminal::window_size() {
        Ok(ws) if ws.width > 0 && ws.height > 0 && ws.columns > 0 && ws.rows > 0 => {
            ((ws.width / ws.columns).max(1), (ws.height / ws.rows).max(1))
        }
        _ => (10, 20),
    }
}

/// Fit an image of `w x h` pixels into at most `max_cols x max_rows` cells,
/// keeping its aspect ratio and never upscaling beyond its natural size.
pub fn fit_cells(w: u32, h: u32, cell: (u16, u16), max_cols: usize, max_rows: usize) -> (usize, usize) {
    let (cw, ch) = (cell.0.max(1) as f64, cell.1.max(1) as f64);
    let (w, h) = (w.max(1) as f64, h.max(1) as f64);
    let max_cols = max_cols.clamp(1, MAX_CELLS) as f64;
    let max_rows = max_rows.clamp(1, MAX_CELLS) as f64;

    let mut cols = (w / cw).ceil().min(max_cols);
    let mut rows = (cols * cw * h / w / ch).ceil();
    if rows > max_rows {
        rows = max_rows;
        cols = (rows * ch * w / h / cw).ceil().min(max_cols);
    }
    (cols.max(1.0) as usize, rows.max(1.0) as usize)
}

fn id_color(id: u32) -> Color {
    Color::Rgb((id >> 16) as u8, (id >> 8) as u8, id as u8)
}

/// One row of placeholder cells for image `id`.
pub fn placeholder_row(id: u32, row: usize, cols: usize) -> Span<'static> {
    let r = DIACRITICS[row.min(MAX_CELLS - 1)];
    let mut s = String::with_capacity(cols * 10);
    for &c in DIACRITICS.iter().take(cols) {
        s.push(PLACEHOLDER);
        s.push(r);
        s.push(c);
    }
    Span::styled(s, Style::default().fg(id_color(id)))
}

/// Transmit PNG data as image `id` (chunked, responses suppressed).
pub fn transmit_png(out: &mut impl Write, id: u32, png: &[u8]) -> io::Result<()> {
    let data = base64::engine::general_purpose::STANDARD.encode(png);
    let chunks: Vec<&[u8]> = data.as_bytes().chunks(4096).collect();
    for (i, chunk) in chunks.iter().enumerate() {
        let more = u8::from(i + 1 < chunks.len());
        if i == 0 {
            write!(out, "\x1b_Ga=t,f=100,i={id},q=2,m={more};")?;
        } else {
            write!(out, "\x1b_Gm={more};")?;
        }
        out.write_all(chunk)?;
        out.write_all(b"\x1b\\")?;
    }
    Ok(())
}

/// Create/replace the virtual placement used by the placeholders.
pub fn place(out: &mut impl Write, id: u32, cols: usize, rows: usize) -> io::Result<()> {
    write!(out, "\x1b_Ga=p,U=1,i={id},p=1,c={cols},r={rows},q=2\x1b\\")
}

/// Delete image `id` and free its data in the terminal.
pub fn delete(out: &mut impl Write, id: u32) -> io::Result<()> {
    write!(out, "\x1b_Ga=d,d=I,i={id},q=2\x1b\\")
}

/// Row/column diacritics from kitty's rowcolumn-diacritics.txt.
static DIACRITICS: [char; 297] = [
    '\u{305}',
    '\u{30D}',
    '\u{30E}',
    '\u{310}',
    '\u{312}',
    '\u{33D}',
    '\u{33E}',
    '\u{33F}',
    '\u{346}',
    '\u{34A}',
    '\u{34B}',
    '\u{34C}',
    '\u{350}',
    '\u{351}',
    '\u{352}',
    '\u{357}',
    '\u{35B}',
    '\u{363}',
    '\u{364}',
    '\u{365}',
    '\u{366}',
    '\u{367}',
    '\u{368}',
    '\u{369}',
    '\u{36A}',
    '\u{36B}',
    '\u{36C}',
    '\u{36D}',
    '\u{36E}',
    '\u{36F}',
    '\u{483}',
    '\u{484}',
    '\u{485}',
    '\u{486}',
    '\u{487}',
    '\u{592}',
    '\u{593}',
    '\u{594}',
    '\u{595}',
    '\u{597}',
    '\u{598}',
    '\u{599}',
    '\u{59C}',
    '\u{59D}',
    '\u{59E}',
    '\u{59F}',
    '\u{5A0}',
    '\u{5A1}',
    '\u{5A8}',
    '\u{5A9}',
    '\u{5AB}',
    '\u{5AC}',
    '\u{5AF}',
    '\u{5C4}',
    '\u{610}',
    '\u{611}',
    '\u{612}',
    '\u{613}',
    '\u{614}',
    '\u{615}',
    '\u{616}',
    '\u{617}',
    '\u{657}',
    '\u{658}',
    '\u{659}',
    '\u{65A}',
    '\u{65B}',
    '\u{65D}',
    '\u{65E}',
    '\u{6D6}',
    '\u{6D7}',
    '\u{6D8}',
    '\u{6D9}',
    '\u{6DA}',
    '\u{6DB}',
    '\u{6DC}',
    '\u{6DF}',
    '\u{6E0}',
    '\u{6E1}',
    '\u{6E2}',
    '\u{6E4}',
    '\u{6E7}',
    '\u{6E8}',
    '\u{6EB}',
    '\u{6EC}',
    '\u{730}',
    '\u{732}',
    '\u{733}',
    '\u{735}',
    '\u{736}',
    '\u{73A}',
    '\u{73D}',
    '\u{73F}',
    '\u{740}',
    '\u{741}',
    '\u{743}',
    '\u{745}',
    '\u{747}',
    '\u{749}',
    '\u{74A}',
    '\u{7EB}',
    '\u{7EC}',
    '\u{7ED}',
    '\u{7EE}',
    '\u{7EF}',
    '\u{7F0}',
    '\u{7F1}',
    '\u{7F3}',
    '\u{816}',
    '\u{817}',
    '\u{818}',
    '\u{819}',
    '\u{81B}',
    '\u{81C}',
    '\u{81D}',
    '\u{81E}',
    '\u{81F}',
    '\u{820}',
    '\u{821}',
    '\u{822}',
    '\u{823}',
    '\u{825}',
    '\u{826}',
    '\u{827}',
    '\u{829}',
    '\u{82A}',
    '\u{82B}',
    '\u{82C}',
    '\u{82D}',
    '\u{951}',
    '\u{953}',
    '\u{954}',
    '\u{F82}',
    '\u{F83}',
    '\u{F86}',
    '\u{F87}',
    '\u{135D}',
    '\u{135E}',
    '\u{135F}',
    '\u{17DD}',
    '\u{193A}',
    '\u{1A17}',
    '\u{1A75}',
    '\u{1A76}',
    '\u{1A77}',
    '\u{1A78}',
    '\u{1A79}',
    '\u{1A7A}',
    '\u{1A7B}',
    '\u{1A7C}',
    '\u{1B6B}',
    '\u{1B6D}',
    '\u{1B6E}',
    '\u{1B6F}',
    '\u{1B70}',
    '\u{1B71}',
    '\u{1B72}',
    '\u{1B73}',
    '\u{1CD0}',
    '\u{1CD1}',
    '\u{1CD2}',
    '\u{1CDA}',
    '\u{1CDB}',
    '\u{1CE0}',
    '\u{1DC0}',
    '\u{1DC1}',
    '\u{1DC3}',
    '\u{1DC4}',
    '\u{1DC5}',
    '\u{1DC6}',
    '\u{1DC7}',
    '\u{1DC8}',
    '\u{1DC9}',
    '\u{1DCB}',
    '\u{1DCC}',
    '\u{1DD1}',
    '\u{1DD2}',
    '\u{1DD3}',
    '\u{1DD4}',
    '\u{1DD5}',
    '\u{1DD6}',
    '\u{1DD7}',
    '\u{1DD8}',
    '\u{1DD9}',
    '\u{1DDA}',
    '\u{1DDB}',
    '\u{1DDC}',
    '\u{1DDD}',
    '\u{1DDE}',
    '\u{1DDF}',
    '\u{1DE0}',
    '\u{1DE1}',
    '\u{1DE2}',
    '\u{1DE3}',
    '\u{1DE4}',
    '\u{1DE5}',
    '\u{1DE6}',
    '\u{1DFE}',
    '\u{20D0}',
    '\u{20D1}',
    '\u{20D4}',
    '\u{20D5}',
    '\u{20D6}',
    '\u{20D7}',
    '\u{20DB}',
    '\u{20DC}',
    '\u{20E1}',
    '\u{20E7}',
    '\u{20E9}',
    '\u{20F0}',
    '\u{2CEF}',
    '\u{2CF0}',
    '\u{2CF1}',
    '\u{2DE0}',
    '\u{2DE1}',
    '\u{2DE2}',
    '\u{2DE3}',
    '\u{2DE4}',
    '\u{2DE5}',
    '\u{2DE6}',
    '\u{2DE7}',
    '\u{2DE8}',
    '\u{2DE9}',
    '\u{2DEA}',
    '\u{2DEB}',
    '\u{2DEC}',
    '\u{2DED}',
    '\u{2DEE}',
    '\u{2DEF}',
    '\u{2DF0}',
    '\u{2DF1}',
    '\u{2DF2}',
    '\u{2DF3}',
    '\u{2DF4}',
    '\u{2DF5}',
    '\u{2DF6}',
    '\u{2DF7}',
    '\u{2DF8}',
    '\u{2DF9}',
    '\u{2DFA}',
    '\u{2DFB}',
    '\u{2DFC}',
    '\u{2DFD}',
    '\u{2DFE}',
    '\u{2DFF}',
    '\u{A66F}',
    '\u{A67C}',
    '\u{A67D}',
    '\u{A6F0}',
    '\u{A6F1}',
    '\u{A8E0}',
    '\u{A8E1}',
    '\u{A8E2}',
    '\u{A8E3}',
    '\u{A8E4}',
    '\u{A8E5}',
    '\u{A8E6}',
    '\u{A8E7}',
    '\u{A8E8}',
    '\u{A8E9}',
    '\u{A8EA}',
    '\u{A8EB}',
    '\u{A8EC}',
    '\u{A8ED}',
    '\u{A8EE}',
    '\u{A8EF}',
    '\u{A8F0}',
    '\u{A8F1}',
    '\u{AAB0}',
    '\u{AAB2}',
    '\u{AAB3}',
    '\u{AAB7}',
    '\u{AAB8}',
    '\u{AABE}',
    '\u{AABF}',
    '\u{AAC1}',
    '\u{FE20}',
    '\u{FE21}',
    '\u{FE22}',
    '\u{FE23}',
    '\u{FE24}',
    '\u{FE25}',
    '\u{FE26}',
    '\u{10A0F}',
    '\u{10A38}',
    '\u{1D185}',
    '\u{1D186}',
    '\u{1D187}',
    '\u{1D188}',
    '\u{1D189}',
    '\u{1D1AA}',
    '\u{1D1AB}',
    '\u{1D1AC}',
    '\u{1D1AD}',
    '\u{1D242}',
    '\u{1D243}',
    '\u{1D244}',
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placeholder_row_is_one_cell_per_column() {
        let span = placeholder_row(0x123456, 3, 7);
        assert_eq!(span.width(), 7);
        assert_eq!(span.style.fg, Some(Color::Rgb(0x12, 0x34, 0x56)));
        let first: Vec<char> = span.content.chars().take(3).collect();
        assert_eq!(first, vec![PLACEHOLDER, DIACRITICS[3], DIACRITICS[0]]);
    }

    #[test]
    fn fit_keeps_aspect_and_limits() {
        // 10x20 px cells: a 200x200 image is 20 cols x 10 rows.
        assert_eq!(fit_cells(200, 200, (10, 20), 80, 30), (20, 10));
        // Width-limited.
        assert_eq!(fit_cells(2000, 1000, (10, 20), 40, 30), (40, 10));
        // Height-limited: tall image capped at 12 rows.
        assert_eq!(fit_cells(100, 2000, (10, 20), 40, 12), (2, 12));
        assert_eq!(fit_cells(0, 0, (0, 0), 0, 0), (1, 1));
    }

    #[test]
    fn transmit_chunks_and_terminates() {
        let mut out = Vec::new();
        transmit_png(&mut out, 7, &vec![0u8; 10_000]).unwrap();
        let s = String::from_utf8(out).unwrap();
        assert!(s.starts_with("\x1b_Ga=t,f=100,i=7,q=2,m=1;"));
        assert_eq!(s.matches("\x1b\\").count(), s.matches("\x1b_G").count());
        assert!(s.contains("\x1b_Gm=0;"));
    }
}
