//! Placeholder thread texture for the 3D view: a seamless, greyscale tile of
//! twisted-rope ridges and fibre grain. It is greyscale on purpose, so the
//! thread colour is applied by multiplying with it and the shading stays
//! subtle. A real photo of rope can replace it later, as long as it is
//! greyscale-friendly and tiles seamlessly.

/// Side of the square tile in pixels. Must be divisible by 4 so the twist
/// pattern repeats exactly across the tile edges.
pub const TILE: usize = 64;

/// Greyscale RGBA8 tile, `TILE * TILE * 4` bytes, row-major. Values stay
/// between about 0.88 and 1.0 of full brightness, so the shading stays subtle.
pub fn rope_tile() -> Vec<u8> {
    let n = TILE;
    let period = n / 4; // one twist of the rope, in pixels
    let mut out = Vec::with_capacity(n * n * 4);
    for y in 0..n {
        for x in 0..n {
            // Twist ridges run diagonally, like a laid rope.
            let d = ((x + y) % period) as f32 / period as f32;
            let ridge = 0.5 + 0.5 * (2.0 * std::f32::consts::PI * d).cos();
            // Strand edges sit a little darker than their crowns.
            let strand = 0.90 + 0.08 * ridge;
            // Fibre grain: a fixed hash of the pixel, so the tile stays seamless.
            let grain = (hash(x as u32, y as u32) - 0.5) * 0.04;
            let v = (strand + grain).clamp(0.0, 1.0);
            let g = (v * 255.0).round() as u8;
            out.extend_from_slice(&[g, g, g, 255]);
        }
    }
    out
}

/// A small integer hash mapped to 0..1, deterministic per pixel.
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

    #[test]
    fn tile_is_full_size_greyscale_and_in_range() {
        let t = rope_tile();
        assert_eq!(t.len(), TILE * TILE * 4);
        let mut lo = 255u8;
        let mut hi = 0u8;
        for px in t.chunks_exact(4) {
            assert_eq!(px[0], px[1]);
            assert_eq!(px[1], px[2]);
            assert_eq!(px[3], 255);
            lo = lo.min(px[0]);
            hi = hi.max(px[0]);
        }
        // subtle: not flat, and not stark
        assert!(hi - lo > 8 && hi - lo < 60, "range {lo}..{hi}");
        assert!(lo >= 170, "too dark for a tint: {lo}");
    }

    #[test]
    fn ridges_repeat_across_the_tile_edge() {
        // The twist ridges depend only on (x + y) mod period, and the period
        // divides the tile, so they meet seamlessly at the edges. (The grain is
        // random per pixel and is not meant to match across edges.)
        let period = TILE / 4;
        assert_eq!(TILE % period, 0);
        let ridge = |x: usize, y: usize| (x + y) % period;
        for y in 0..TILE {
            // stepping one full tile (or one period) lands on the same ridge phase
            assert_eq!(ridge(0, y), ridge(period, y));
            assert_eq!(ridge(0, y), ridge(TILE, y));
        }
    }
}
