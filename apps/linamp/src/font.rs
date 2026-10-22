//! TrueType text for the playlist and menus (fontdue + fontconfig lookup).

use linamp_skin::bitmap::mix;
use linamp_skin::Bitmap;
use std::collections::HashMap;

pub struct Font {
    font: Option<fontdue::Font>,
    cache: HashMap<(char, u32), (fontdue::Metrics, Vec<u8>)>,
}

/// Finds a font file for `family` through `fc-match`.
fn find_font_file(family: &str) -> Option<std::path::PathBuf> {
    let out = std::process::Command::new("fc-match").args(["-f", "%{file}", family]).output().ok()?;
    let path = String::from_utf8(out.stdout).ok()?;
    let path = std::path::PathBuf::from(path.trim());
    path.is_file().then_some(path)
}

impl Font {
    pub fn load(family: &str) -> Font {
        let font = [family, "sans-serif", "DejaVu Sans"].iter().find_map(|f| {
            let path = find_font_file(f)?;
            let data = std::fs::read(&path).ok()?;
            match fontdue::Font::from_bytes(data, fontdue::FontSettings::default()) {
                Ok(font) => Some(font),
                Err(e) => {
                    log::warn!("cannot parse font {}: {e}", path.display());
                    None
                }
            }
        });
        if font.is_none() {
            log::warn!("no usable TrueType font found; playlist text will be blank");
        }
        Font { font, cache: HashMap::new() }
    }

    fn glyph(&mut self, c: char, px: u32) -> Option<&(fontdue::Metrics, Vec<u8>)> {
        let font = self.font.as_ref()?;
        if self.cache.len() > 4096 {
            self.cache.clear();
        }
        Some(self.cache.entry((c, px)).or_insert_with(|| font.rasterize(c, px as f32)))
    }

    pub fn ascent(&self, px: u32) -> i32 {
        self.font
            .as_ref()
            .and_then(|f| f.horizontal_line_metrics(px as f32))
            .map(|m| m.ascent.round() as i32)
            .unwrap_or(px as i32)
    }

    pub fn width(&mut self, text: &str, px: u32) -> i32 {
        text.chars().map(|c| self.glyph(c, px).map(|g| g.0.advance_width).unwrap_or(0.0)).sum::<f32>().round() as i32
    }

    /// Draws `text` with its top at `y`, clipped to `clip_x..clip_x+clip_w`.
    /// Returns the x after the last glyph.
    #[allow(clippy::too_many_arguments)]
    pub fn draw(&mut self, b: &mut Bitmap, x: i32, y: i32, text: &str, px: u32, color: u32, clip_x: i32, clip_w: i32) -> i32 {
        let baseline = y + self.ascent(px);
        let mut pen = x as f32;
        for c in text.chars() {
            let Some((m, coverage)) = self.glyph(c, px).map(|(m, c)| (*m, c.clone())) else { continue };
            let gx = pen.round() as i32 + m.xmin;
            let gy = baseline - m.height as i32 - m.ymin;
            for row in 0..m.height as i32 {
                for col in 0..m.width as i32 {
                    let a = coverage[(row * m.width as i32 + col) as usize];
                    let (px_x, px_y) = (gx + col, gy + row);
                    if a == 0 || px_x < clip_x || px_x >= clip_x + clip_w {
                        continue;
                    }
                    if let Some(bg) = b.get(px_x, px_y) {
                        b.set(px_x, px_y, mix(bg, color, a as f32 / 255.0));
                    }
                }
            }
            pen += m.advance_width;
            if pen as i32 > clip_x + clip_w {
                break;
            }
        }
        pen.round() as i32
    }

    /// Shortens `text` with an ellipsis so it fits in `max_w` pixels.
    pub fn fit(&mut self, text: &str, px: u32, max_w: i32) -> String {
        if self.width(text, px) <= max_w {
            return text.to_string();
        }
        let ell = self.width("…", px);
        let mut out = String::new();
        let mut w = 0;
        for c in text.chars() {
            let cw = self.width(&c.to_string(), px);
            if w + cw + ell > max_w {
                break;
            }
            out.push(c);
            w += cw;
        }
        out.push('…');
        out
    }
}
