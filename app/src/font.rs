//! Tiny PSF console-font loader + BGRA text drawing for the source picker.
//!
//! The TV ships gzipped PSF fonts (`/usr/share/consolefonts/*.psfu.gz`), so we
//! get a full ASCII font without embedding glyph data or shipping a font file.

use std::io::Read;

pub struct Font {
    pub width: usize,
    pub height: usize,
    glyph_bytes: usize,
    /// PSF2 only: bytes used per glyph row.
    row_bytes: usize,
    data: Vec<u8>,
}

fn gunzip(raw: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    flate2::read::GzDecoder::new(raw).read_to_end(&mut out).ok()?;
    Some(out)
}

impl Font {
    fn parse(d: &[u8]) -> Option<Font> {
        if d.len() > 4 && d[0] == 0x36 && d[1] == 0x04 {
            // PSF1
            let charsize = d[3] as usize;
            let count = if d[2] & 1 != 0 { 512 } else { 256 };
            let header = 4;
            let glyph_bytes = charsize;
            let row_bytes = charsize / 16;
            let data = d.get(header..header + count * charsize)?.to_vec();
            return Some(Font {
                width: 8,
                height: 16,
                glyph_bytes,
                row_bytes,
                data,
            });
        }
        if d.len() > 32 && d[0] == 0x72 && d[1] == 0xb5 && d[2] == 0x4a && d[3] == 0x86 {
            let headersize = u32::from_le_bytes([d[8], d[9], d[10], d[11]]) as usize;
            let count = u32::from_le_bytes([d[16], d[17], d[18], d[19]]) as usize;
            let charsize = u32::from_le_bytes([d[20], d[21], d[22], d[23]]) as usize;
            let height = u32::from_le_bytes([d[24], d[25], d[26], d[27]]) as usize;
            let width = u32::from_le_bytes([d[28], d[29], d[30], d[31]]) as usize;
            let row_bytes = (width + 7) / 8;
            let data = d.get(headersize..headersize + count * charsize)?.to_vec();
            return Some(Font {
                width,
                height,
                glyph_bytes: charsize,
                row_bytes,
                data,
            });
        }
        None
    }

    pub fn load() -> Option<Font> {
        for path in [
            "/usr/share/consolefonts/Agafari-16.psfu.gz",
            "/usr/share/consolefonts/Agafari-14.psfu.gz",
            "/usr/share/consolefonts/Agafari-12.psfu.gz",
        ] {
            if let Ok(raw) = std::fs::read(path) {
                if let Some(unz) = gunzip(&raw) {
                    if let Some(f) = Font::parse(&unz) {
                        return Some(f);
                    }
                }
            }
        }
        None
    }

    fn glyph(&self, ch: char) -> Option<&[u8]> {
        let idx = ch as usize;
        if idx >= 256 {
            return None;
        }
        let start = idx * self.glyph_bytes;
        self.data.get(start..start + self.glyph_bytes)
    }

    /// True when the pixel (x, y) of the glyph is set.
    fn bit(&self, g: &[u8], x: usize, y: usize) -> bool {
        if y >= self.height || x >= self.width {
            return false;
        }
        let row = y;
        let byte = g[row * self.row_bytes + x / 8];
        byte & (0x80 >> (x % 8)) != 0
    }
}

pub struct Canvas<'a> {
    pub buf: &'a mut [u8],
    pub width: usize,
    pub height: usize,
}

impl<'a> Canvas<'a> {
    pub fn new(buf: &'a mut [u8], width: usize, height: usize) -> Self {
        Canvas { buf, width, height }
    }

    pub fn clear(&mut self, b: u8, g: u8, r: u8) {
        for px in self.buf.chunks_exact_mut(4) {
            px[0] = b;
            px[1] = g;
            px[2] = r;
            px[3] = 255;
        }
    }

    pub fn fill(&mut self, x: usize, y: usize, w: usize, h: usize, b: u8, g: u8, r: u8) {
        let x2 = (x + w).min(self.width);
        let y2 = (y + h).min(self.height);
        for yy in y..y2 {
            for xx in x..x2 {
                let o = (yy * self.width + xx) * 4;
                self.buf[o] = b;
                self.buf[o + 1] = g;
                self.buf[o + 2] = r;
                self.buf[o + 3] = 255;
            }
        }
    }

    pub fn text(&mut self, font: &Font, x: usize, y: usize, scale: usize, s: &str, rgb: (u8, u8, u8)) {
        let (b, g, r) = rgb;
        let mut cx = x;
        for ch in s.chars() {
            if ch == '\n' {
                cx = x;
                continue;
            }
            if let Some(gl) = font.glyph(ch) {
                for gy in 0..font.height {
                    for gx in 0..font.width {
                        if font.bit(gl, gx, gy) {
                            self.fill(cx + gx * scale, y + gy * scale, scale, scale, b, g, r);
                        }
                    }
                }
            }
            cx += (font.width + 1) * scale;
            if cx + font.width * scale > self.width {
                break;
            }
        }
    }
}
