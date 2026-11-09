//! The app icon: an original keyboard glyph (white rounded body with a
//! grid of keys and a space bar on the hub's red), drawn in code so no
//! image file or vendor artwork is needed. `assets/icons/keyboard-hub.png`
//! is the same drawing at 256 px, written by the render test.

/// Straight RGBA pixels, `size`×`size`.
pub fn rgba(size: u32) -> Vec<u8> {
    let n = size as f32;
    let mut px = vec![0u8; (size * size * 4) as usize];
    // Shapes in unit coordinates (0..1), anti-aliased by 4x4 supersampling.
    let bg = |x: f32, y: f32| rounded(x, y, 0.0, 0.0, 1.0, 1.0, 0.22);
    let body = |x: f32, y: f32| rounded(x, y, 0.12, 0.27, 0.88, 0.75, 0.07);
    let key = |x: f32, y: f32| -> bool {
        // Three rows: 6 keys, 6 keys, space bar between two keys.
        let rows = [0.33, 0.45];
        for ry in rows {
            for k in 0..6 {
                let kx = 0.17 + k as f32 * 0.113;
                if rounded(x, y, kx, ry, kx + 0.08, ry + 0.08, 0.02) {
                    return true;
                }
            }
        }
        let ry = 0.57;
        rounded(x, y, 0.17, ry, 0.25, ry + 0.08, 0.02)
            || rounded(x, y, 0.283, ry, 0.704, ry + 0.08, 0.02)
            || rounded(x, y, 0.735, ry, 0.815, ry + 0.08, 0.02)
    };
    for py in 0..size {
        for pxi in 0..size {
            let mut acc = [0f32; 4];
            for sy in 0..4 {
                for sx in 0..4 {
                    let x = (pxi as f32 + (sx as f32 + 0.5) / 4.0) / n;
                    let y = (py as f32 + (sy as f32 + 0.5) / 4.0) / n;
                    let c: [f32; 4] = if !bg(x, y) {
                        [0.0, 0.0, 0.0, 0.0]
                    } else if body(x, y) && !key(x, y) {
                        [247.0, 247.0, 247.0, 255.0]
                    } else {
                        [0xE5 as f32, 0x2B as f32, 0x36 as f32, 255.0]
                    };
                    for i in 0..4 {
                        acc[i] += c[i] / 16.0;
                    }
                }
            }
            let o = ((py * size + pxi) * 4) as usize;
            let a = acc[3].max(1e-3);
            for i in 0..3 {
                // Un-premultiply the averaged colour.
                px[o + i] = (acc[i] * 255.0 / a).round().min(255.0) as u8;
            }
            px[o + 3] = acc[3].round() as u8;
        }
    }
    px
}

fn rounded(x: f32, y: f32, x0: f32, y0: f32, x1: f32, y1: f32, r: f32) -> bool {
    if x < x0 || x > x1 || y < y0 || y > y1 {
        return false;
    }
    let cx = x.clamp(x0 + r, x1 - r);
    let cy = y.clamp(y0 + r, y1 - r);
    (x - cx).powi(2) + (y - cy).powi(2) <= r * r
}

/// ARGB32 in network byte order, as StatusNotifierItem wants it.
pub fn argb(size: u32) -> Vec<u8> {
    rgba(size)
        .chunks_exact(4)
        .flat_map(|p| [p[3], p[0], p[1], p[2]])
        .collect()
}

/// The window icon.
pub fn window_icon() -> egui::IconData {
    egui::IconData {
        rgba: rgba(64),
        width: 64,
        height: 64,
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn icon_has_body_keys_and_transparent_corners() {
        let s = 64;
        let p = super::rgba(s);
        let at = |x: u32, y: u32| &p[((y * s + x) * 4) as usize..((y * s + x) * 4 + 4) as usize];
        assert_eq!(at(0, 0)[3], 0, "corner transparent");
        assert_eq!(at(32, 4), &[0xE5, 0x2B, 0x36, 255], "red background");
        assert_eq!(at(10, 18)[0], 247, "white body");
        let a = super::argb(s);
        assert_eq!(
            &a[(4 * (4 * s + 32)) as usize..][..4],
            &[255, 0xE5, 0x2B, 0x36]
        );
    }
}
