//! The QO-ALL mark: the dish of `assets/qo-all-logo.svg` with its uplink and downlink
//! arrows, drawn in software with distance fields (anti-aliased), so the window icon and
//! the header need no image file.

const WHITE: [u8; 3] = [0xff, 0xff, 0xff];
const UPLINK: [u8; 3] = [0xff, 0x7a, 0x7a];
const DOWNLINK: [u8; 3] = [0xf2, 0xb3, 0x3d];
const MAST: [u8; 3] = [0x8f, 0xa3, 0xbf];
const ICON_BACKGROUND: [u8; 3] = [0x0d, 0x14, 0x20];
/// Square the mark is centred in (SVG units) and its top-left corner.
const VIEW: f32 = 118.0;
const OFFSET: (f32, f32) = (10.0, 14.5);

type Point = (f32, f32);

/// The SVG group transform `rotate(30 60 90) translate(0 6)`.
fn tilt(p: Point) -> Point {
    let (s, c) = 30f32.to_radians().sin_cos();
    let (dx, dy) = (p.0 - 60.0, p.1 + 6.0 - 90.0);
    (60.0 + dx * c - dy * s, 90.0 + dx * s + dy * c)
}

/// Strokes of the mark: (from, to, width, colour), in SVG units.
fn strokes() -> Vec<(Point, Point, f32, [u8; 3])> {
    let mut out = Vec::new();
    // Dish: the quadratic curve M20 70 Q60 120 100 70, as short segments.
    let curve = |t: f32| {
        let u = 1.0 - t;
        (u * u * 20.0 + 2.0 * u * t * 60.0 + t * t * 100.0, u * u * 70.0 + 2.0 * u * t * 120.0 + t * t * 70.0)
    };
    const PIECES: usize = 16;
    for i in 0..PIECES {
        let (a, b) = (curve(i as f32 / PIECES as f32), curve((i + 1) as f32 / PIECES as f32));
        out.push((tilt(a), tilt(b), 7.0, WHITE));
    }
    out.push((tilt((60.0, 95.0)), tilt((60.0, 76.0)), 4.0, WHITE));
    // Feed horn (a 12 × 9 box), as a thick short stroke.
    out.push((tilt((57.5, 72.5)), tilt((62.5, 72.5)), 9.0, WHITE));
    for (a, b) in [((48.0, 60.0), (48.0, 14.0)), ((38.0, 26.0), (48.0, 13.0)), ((48.0, 13.0), (58.0, 26.0))] {
        out.push((tilt(a), tilt(b), 6.0, UPLINK));
    }
    for (a, b) in [((72.0, 13.0), (72.0, 58.0)), ((62.0, 46.0), (72.0, 59.0)), ((72.0, 59.0), (82.0, 46.0))] {
        out.push((tilt(a), tilt(b), 6.0, DOWNLINK));
    }
    out.push(((56.0, 104.0), (56.0, 124.0), 5.0, MAST));
    out.push(((40.0, 126.0), (72.0, 126.0), 5.0, MAST));
    out
}

/// Distance from (px, py) to the segment from `a` to `b`.
fn segment(px: f32, py: f32, a: Point, b: Point) -> f32 {
    let (bax, bay) = (b.0 - a.0, b.1 - a.1);
    let len = bax * bax + bay * bay;
    let t = if len > 0.0 { (((px - a.0) * bax + (py - a.1) * bay) / len).clamp(0.0, 1.0) } else { 0.0 };
    (px - a.0 - t * bax).hypot(py - a.1 - t * bay)
}

/// `size × size` RGBA (not premultiplied). With `background` the mark sits on a dark
/// rounded square (window icon); without it the rest is transparent (header).
pub fn rgba(size: usize, background: bool) -> Vec<u8> {
    let strokes = strokes();
    let scale = VIEW / size as f32;
    // Below ~32 px the thin strokes would vanish; they are kept at least a pixel wide.
    let min_width = 1.3 * scale;
    let mut out = Vec::with_capacity(size * size * 4);
    for py in 0..size {
        for px in 0..size {
            let (x, y) = ((px as f32 + 0.5) * scale + OFFSET.0, (py as f32 + 0.5) * scale + OFFSET.1);
            let cover = |d: f32| (0.5 - d / scale).clamp(0.0, 1.0);
            let mut color = [0.0f32; 3];
            let mut alpha = 0.0f32;
            if background {
                color = ICON_BACKGROUND.map(f32::from);
                let (cx, cy) = (px as f32 + 0.5 - size as f32 / 2.0, py as f32 + 0.5 - size as f32 / 2.0);
                let (half, r) = (size as f32 / 2.0, size as f32 * 0.22);
                let (qx, qy) = (cx.abs() - (half - r), cy.abs() - (half - r));
                let d = qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0) - r;
                alpha = (0.5 - d).clamp(0.0, 1.0);
            }
            for &(a, b, width, c) in &strokes {
                let cov = cover(segment(x, y, a, b) - width.max(min_width) / 2.0);
                if cov <= 0.0 {
                    continue;
                }
                for i in 0..3 {
                    color[i] += (c[i] as f32 - color[i]) * cov;
                }
                alpha += (1.0 - alpha) * cov;
            }
            out.extend(color.iter().map(|&c| c.round() as u8));
            out.push((alpha * 255.0).round() as u8);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mark_fits_the_square_and_corners_are_clear() {
        let size = 64;
        let px = rgba(size, false);
        assert_eq!(px.len(), size * size * 4);
        assert_eq!(px[3], 0, "corner must be transparent");
        assert!(px.chunks_exact(4).filter(|p| p[3] == 255).count() > 200, "mark too small");
        // Nothing may touch the outer rows and columns (the mark would be cut off).
        for i in 0..size {
            for (x, y) in [(i, 0), (i, size - 1), (0, i), (size - 1, i)] {
                assert_eq!(px[(y * size + x) * 4 + 3], 0, "mark touches the edge at {x},{y}");
            }
        }
        let icon = rgba(size, true);
        assert_eq!(icon[3], 0, "icon corner must be rounded off");
    }

    /// Preview: `QOALL_LOGO_OUT=path cargo test --lib logo -- --ignored` writes 256×256
    /// RGBA (convert with ffmpeg -f rawvideo -pix_fmt rgba -s 256x256 -i path out.png).
    #[test]
    #[ignore]
    fn export_preview() {
        if let Some(path) = std::env::var_os("QOALL_LOGO_OUT") {
            std::fs::write(path, rgba(256, true)).unwrap();
        }
    }
}
