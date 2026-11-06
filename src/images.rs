//! Inline image previews: download → decode → transmit via kitty graphics.

use crate::ui::kitty;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::io::{self, Cursor, Write};
use std::path::Path;

/// Longest side of the PNG sent to the terminal; kitty scales it to the cells.
const MAX_SIDE_PX: u32 = 800;

#[derive(Debug, Clone)]
pub struct DecodedImage {
    pub png: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

/// Decode any supported image file and re-encode it as a PNG small enough
/// to transmit quickly (blocking; run on a worker thread).
pub fn decode(path: &Path) -> Result<DecodedImage, String> {
    let img = image::ImageReader::open(path)
        .and_then(|r| r.with_guessed_format())
        .map_err(|e| e.to_string())?
        .decode()
        .map_err(|e| e.to_string())?;
    let img = if img.width().max(img.height()) > MAX_SIDE_PX {
        img.thumbnail(MAX_SIDE_PX, MAX_SIDE_PX)
    } else {
        img
    };
    let mut png = Vec::new();
    img.write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png)
        .map_err(|e| e.to_string())?;
    Ok(DecodedImage { png, width: img.width(), height: img.height() })
}

struct Ready {
    kitty_id: u32,
    image: DecodedImage,
    transmitted: bool,
    placed: Option<(usize, usize)>,
}

enum Entry {
    Downloading,
    Decoding,
    Ready(Ready),
    Failed,
}

/// What `ImageStore::want` asks the caller to do next.
pub enum Need {
    Nothing,
    Download,
    Decode,
}

pub struct ImageStore {
    /// Inline images are drawn only when the terminal supports them.
    pub enabled: bool,
    /// Terminal cell size in pixels (refreshed before each frame).
    pub cell_px: Cell<(u16, u16)>,
    entries: HashMap<i32, Entry>,
    next_id: u32,
    /// Placements requested by the last rendered frame.
    wanted: RefCell<HashMap<u32, (usize, usize)>>,
}

impl ImageStore {
    pub fn new(enabled: bool) -> Self {
        Self {
            enabled,
            cell_px: Cell::new((10, 20)),
            entries: HashMap::new(),
            // Start from a per-process base so two instances in the same
            // kitty window don't clash; ids are 24-bit (encoded as RGB).
            next_id: 0x100000 + (std::process::id() % 0x400) * 0x1000,
            wanted: RefCell::new(HashMap::new()),
        }
    }

    /// Register interest in preview `file_id`. `downloaded` tells whether
    /// the file is already on disk.
    pub fn want(&mut self, file_id: i32, downloaded: bool) -> Need {
        if !self.enabled || self.entries.contains_key(&file_id) {
            return Need::Nothing;
        }
        if downloaded {
            self.entries.insert(file_id, Entry::Decoding);
            Need::Decode
        } else {
            self.entries.insert(file_id, Entry::Downloading);
            Need::Download
        }
    }

    /// A download finished; returns true if it should now be decoded.
    pub fn downloaded(&mut self, file_id: i32) -> bool {
        match self.entries.get_mut(&file_id) {
            Some(e @ Entry::Downloading) => {
                *e = Entry::Decoding;
                true
            }
            _ => false,
        }
    }

    pub fn decoded(&mut self, file_id: i32, result: Result<DecodedImage, String>) {
        let entry = match result {
            Ok(image) => {
                let kitty_id = self.next_id;
                self.next_id = (self.next_id + 1) & 0xFF_FFFF;
                Entry::Ready(Ready { kitty_id, image, transmitted: false, placed: None })
            }
            Err(e) => {
                log::warn!("failed to decode preview {file_id}: {e}");
                Entry::Failed
            }
        };
        self.entries.insert(file_id, entry);
    }

    /// (kitty id, width px, height px) of a decoded preview.
    pub fn ready(&self, file_id: i32) -> Option<(u32, u32, u32)> {
        match self.entries.get(&file_id) {
            Some(Entry::Ready(r)) if self.enabled => Some((r.kitty_id, r.image.width, r.image.height)),
            _ => None,
        }
    }

    /// Called while rendering: this frame shows `kitty_id` at `cols x rows`.
    pub fn request_placement(&self, kitty_id: u32, cols: usize, rows: usize) {
        self.wanted.borrow_mut().insert(kitty_id, (cols, rows));
    }

    /// Send pending transmissions/placements for the frame just drawn.
    pub fn flush(&mut self, out: &mut impl Write) -> io::Result<()> {
        let wanted = std::mem::take(&mut *self.wanted.borrow_mut());
        if wanted.is_empty() {
            return Ok(());
        }
        for entry in self.entries.values_mut() {
            let Entry::Ready(r) = entry else { continue };
            let Some(&(cols, rows)) = wanted.get(&r.kitty_id) else { continue };
            if !r.transmitted {
                kitty::transmit_png(out, r.kitty_id, &r.image.png)?;
                r.transmitted = true;
                r.placed = None;
            }
            if r.placed != Some((cols, rows)) {
                kitty::place(out, r.kitty_id, cols, rows)?;
                r.placed = Some((cols, rows));
            }
        }
        out.flush()
    }

    /// The terminal may have dropped our images (e.g. after leaving the
    /// alternate screen for $EDITOR): send them again when next shown.
    pub fn invalidate(&mut self) {
        for entry in self.entries.values_mut() {
            if let Entry::Ready(r) = entry {
                r.transmitted = false;
                r.placed = None;
            }
        }
    }

    /// Free all images in the terminal (on exit).
    pub fn delete_all(&mut self, out: &mut impl Write) -> io::Result<()> {
        for entry in self.entries.values_mut() {
            if let Entry::Ready(r) = entry {
                if r.transmitted {
                    kitty::delete(out, r.kitty_id)?;
                    r.transmitted = false;
                }
            }
        }
        out.flush()
    }
}
