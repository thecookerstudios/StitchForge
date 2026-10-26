//! Thread texture for the 3D view: a seamless greyscale tile of twisted rope,
//! made from Tim's own rope photos (assets/rope_tile.png). It is greyscale on
//! purpose, so the thread colour is applied by multiplying with it.
//!
//! The tile runs along the strand vertically (v) and across it horizontally
//! (u). It repeats seamlessly along the strand; it never wraps across it.
//! If the picture can't be read, a generated twist pattern is used instead.

/// Side of the square tile in pixels.
pub const TILE: usize = 128;

/// The rope tile, made from a photo: greyscale PNG, TILE × TILE.
const ROPE_PNG: &[u8] = include_bytes!("../assets/rope_tile.png");

/// Greyscale RGBA8 tile, `TILE * TILE * 4` bytes, row-major. Values stay
/// light (about 0.7–1.0 of full brightness) so the thread colour shows.
pub fn rope_tile() -> Vec<u8> {
    photo_tile().unwrap_or_else(generated_tile)
}

fn photo_tile() -> Option<Vec<u8>> {
    let img = image::load_from_memory(ROPE_PNG).ok()?.to_luma8();
    if img.width() as usize != TILE || img.height() as usize != TILE {
        return None;
    }
    let mut out = Vec::with_capacity(TILE * TILE * 4);
    for p in img.pixels() {
        let g = p.0[0];
        out.extend_from_slice(&[g, g, g, 255]);
    }
    Some(out)
}

/// Fallback: diagonal twist ridges and fibre grain.
fn generated_tile() -> Vec<u8> {
    let n = TILE;
    let period = n / 4;
    let mut out = Vec::with_capacity(n * n * 4);
    for y in 0..n {
        for x in 0..n {
            let d = ((x + y) % period) as f32 / period as f32;
            let ridge = 0.5 + 0.5 * (2.0 * std::f32::consts::PI * d).cos();
            let strand = 0.80 + 0.16 * ridge;
            let grain = (hash(x as u32, y as u32) - 0.5) * 0.04;
            let g = ((strand + grain).clamp(0.0, 1.0) * 255.0).round() as u8;
            out.extend_from_slice(&[g, g, g, 255]);
        }
    }
    out
}

fn hash(x: u32, y: u32) -> f32 {
    let mut h = x.wrapping_mul(0x9E37_79B9) ^ y.wrapping_mul(0x85EB_CA6B);
    h ^= h >> 13;
    h = h.wrapping_mul(0xC2B2_AE35);
    h ^= h >> 16;
    (h & 0xFFFF) as f32 / 65535.0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(t: &[u8]) {
        assert_eq!(t.len(), TILE * TILE * 4);
        let (mut lo, mut hi) = (255u8, 0u8);
        for px in t.chunks_exact(4) {
            assert_eq!(px[0], px[1]);
            assert_eq!(px[1], px[2]);
            assert_eq!(px[3], 255);
            lo = lo.min(px[0]);
            hi = hi.max(px[0]);
        }
        assert!(hi - lo > 8 && hi - lo < 90, "range {lo}..{hi}");
        assert!(lo >= 170, "too dark for a tint: {lo}");
    }

    #[test]
    fn photo_tile_loads_and_is_light_greyscale() {
        let t = photo_tile().expect("embedded rope tile decodes at TILE size");
        check(&t);
    }

    #[test]
    fn generated_fallback_is_light_greyscale() {
        check(&generated_tile());
    }

    #[test]
    fn photo_tile_is_seamless_along_the_strand() {
        // the jump from the last row back to the first is no bigger than a
        // typical step between neighbouring rows
        let t = photo_tile().unwrap();
        let diff = |a: usize, b: usize| -> f64 {
            (0..TILE)
                .map(|x| (t[(a * TILE + x) * 4] as i32 - t[(b * TILE + x) * 4] as i32).abs())
                .sum::<i32>() as f64
                / TILE as f64
        };
        let seam = diff(TILE - 1, 0);
        let typical = (0..TILE - 1).map(|y| diff(y, y + 1)).sum::<f64>() / (TILE - 1) as f64;
        assert!(seam <= typical * 2.0, "seam {seam:.1} vs typical {typical:.1}");
    }
}
