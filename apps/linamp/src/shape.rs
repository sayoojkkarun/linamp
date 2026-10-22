//! Window shapes from `region.txt` via the X11 Shape extension.

use linamp_skin::textcfg::region_mask;
use x11rb::connection::Connection;
use x11rb::protocol::shape::{self, ConnectionExt as _, SK, SO};
use x11rb::protocol::xproto::{ClipOrdering, Rectangle};

/// Sets (or clears, for `None`) the bounding shape of `window`.
pub fn apply(conn: &impl Connection, window: u32, polys: Option<&[Vec<(i32, i32)>]>, size: (i32, i32), scale: i32) -> Result<(), String> {
    let err = |e: &dyn std::fmt::Display| e.to_string();
    if conn.extension_information(shape::X11_EXTENSION_NAME).map_err(|e| err(&e))?.is_none() {
        return Err("X server has no SHAPE extension".into());
    }
    let rects: Vec<Rectangle> = match polys {
        None => vec![Rectangle { x: 0, y: 0, width: (size.0 * scale) as u16, height: (size.1 * scale) as u16 }],
        Some(polys) => {
            let (w, h) = size;
            let mask = region_mask(polys, w, h);
            let mut out = Vec::new();
            for y in 0..h {
                let mut x = 0;
                while x < w {
                    if !mask[(y * w + x) as usize] {
                        x += 1;
                        continue;
                    }
                    let start = x;
                    while x < w && mask[(y * w + x) as usize] {
                        x += 1;
                    }
                    out.push(Rectangle {
                        x: (start * scale) as i16,
                        y: (y * scale) as i16,
                        width: ((x - start) * scale) as u16,
                        height: scale as u16,
                    });
                }
            }
            if out.is_empty() {
                // A region that hides everything is a broken skin: ignore it.
                vec![Rectangle { x: 0, y: 0, width: (w * scale) as u16, height: (h * scale) as u16 }]
            } else {
                out
            }
        }
    };
    conn.shape_rectangles(SO::SET, SK::BOUNDING, ClipOrdering::UNSORTED, window, 0, 0, &rects).map_err(|e| err(&e))?;
    conn.flush().map_err(|e| err(&e))?;
    Ok(())
}
