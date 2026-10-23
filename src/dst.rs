//! Tajima DST writer and reader, from the publicly documented format:
//! a 512-byte text header, then 3-byte records holding x/y moves of up to
//! ±121 units (0.1 mm) in balanced-ternary bit flags. Origin = design centre,
//! +y is up (so y is flipped from SVG coordinates).

use crate::design::{Cmd, Design};
use crate::geom::Pt;

const MAX: i32 = 121;

fn encode(dx: i32, dy: i32, flags: u8) -> [u8; 3] {
    let mut b = [0u8, 0u8, 0x03 | flags];
    let mut x = dx;
    let mut y = dy;
    // (value, byte, plus-bit, minus-bit)
    let xs = [
        (81, 2, 0x04, 0x08),
        (27, 1, 0x04, 0x08),
        (9, 0, 0x04, 0x08),
        (3, 1, 0x01, 0x02),
        (1, 0, 0x01, 0x02),
    ];
    let ys = [
        (81, 2, 0x20, 0x10),
        (27, 1, 0x20, 0x10),
        (9, 0, 0x20, 0x10),
        (3, 1, 0x80, 0x40),
        (1, 0, 0x80, 0x40),
    ];
    for &(v, i, plus, minus) in &xs {
        if x > v / 2 {
            b[i] |= plus;
            x -= v;
        } else if x < -(v / 2) {
            b[i] |= minus;
            x += v;
        }
    }
    for &(v, i, plus, minus) in &ys {
        if y > v / 2 {
            b[i] |= plus;
            y -= v;
        } else if y < -(v / 2) {
            b[i] |= minus;
            y += v;
        }
    }
    debug_assert!(x == 0 && y == 0, "residual {x},{y}");
    b
}

pub fn decode(b: [u8; 3]) -> (i32, i32) {
    let mut x = 0;
    let mut y = 0;
    let bit = |byte: u8, m: u8| byte & m != 0;
    if bit(b[0], 0x01) {
        x += 1
    }
    if bit(b[0], 0x02) {
        x -= 1
    }
    if bit(b[0], 0x04) {
        x += 9
    }
    if bit(b[0], 0x08) {
        x -= 9
    }
    if bit(b[0], 0x80) {
        y += 1
    }
    if bit(b[0], 0x40) {
        y -= 1
    }
    if bit(b[0], 0x20) {
        y += 9
    }
    if bit(b[0], 0x10) {
        y -= 9
    }
    if bit(b[1], 0x01) {
        x += 3
    }
    if bit(b[1], 0x02) {
        x -= 3
    }
    if bit(b[1], 0x04) {
        x += 27
    }
    if bit(b[1], 0x08) {
        x -= 27
    }
    if bit(b[1], 0x80) {
        y += 3
    }
    if bit(b[1], 0x40) {
        y -= 3
    }
    if bit(b[1], 0x20) {
        y += 27
    }
    if bit(b[1], 0x10) {
        y -= 27
    }
    if bit(b[2], 0x04) {
        x += 81
    }
    if bit(b[2], 0x08) {
        x -= 81
    }
    if bit(b[2], 0x20) {
        y += 81
    }
    if bit(b[2], 0x10) {
        y -= 81
    }
    (x, y)
}

/// Machine units (0.1 mm), centred, y up.
pub fn to_units(d: &Design, p: Pt) -> (i32, i32) {
    let c = d.bbox.center();
    (
        ((p.x - c.x) * 10.0).round() as i32,
        (-(p.y - c.y) * 10.0).round() as i32,
    )
}

pub fn write(d: &Design) -> Vec<u8> {
    let mut recs: Vec<[u8; 3]> = Vec::new();
    let (mut cx, mut cy) = (0i32, 0i32);
    let (mut minx, mut maxx, mut miny, mut maxy) = (0, 0, 0, 0);
    let mut stitch_recs = 0usize;
    let mut colors = 0usize;
    let mut first_color = true;

    let move_to = |recs: &mut Vec<[u8; 3]>,
                   tx: i32,
                   ty: i32,
                   jump: bool,
                   cx: &mut i32,
                   cy: &mut i32|
     -> usize {
        let mut n = 0;
        loop {
            let dx = (tx - *cx).clamp(-MAX, MAX);
            let dy = (ty - *cy).clamp(-MAX, MAX);
            let done = *cx + dx == tx && *cy + dy == ty;
            // Long stitches are split into jumps followed by a final stitch.
            let as_jump = jump || !done;
            if dx == 0 && dy == 0 && !jump {
                // zero-length stitch: still a needle penetration
                recs.push(encode(0, 0, 0));
                return n + 1;
            }
            if dx == 0 && dy == 0 {
                return n;
            }
            recs.push(encode(dx, dy, if as_jump { 0x80 } else { 0 }));
            *cx += dx;
            *cy += dy;
            n += 1;
            if done {
                return n;
            }
        }
    };

    for c in &d.cmds {
        match *c {
            Cmd::Stitch(p) => {
                let (x, y) = to_units(d, p);
                stitch_recs += move_to(&mut recs, x, y, false, &mut cx, &mut cy);
            }
            Cmd::Jump(p) => {
                let (x, y) = to_units(d, p);
                move_to(&mut recs, x, y, true, &mut cx, &mut cy);
            }
            Cmd::Trim => {
                // Trim convention: three consecutive empty jumps.
                for _ in 0..3 {
                    recs.push(encode(0, 0, 0x80));
                }
            }
            Cmd::Color(_) => {
                if first_color {
                    first_color = false;
                } else {
                    recs.push([0x00, 0x00, 0xC3]);
                    colors += 1;
                }
            }
            Cmd::End => recs.push([0x00, 0x00, 0xF3]),
        }
        minx = minx.min(cx);
        maxx = maxx.max(cx);
        miny = miny.min(cy);
        maxy = maxy.max(cy);
    }

    let label: String = d
        .name
        .chars()
        .filter(|c| c.is_ascii_graphic() || *c == ' ')
        .take(16)
        .collect();
    let mut h = String::new();
    h.push_str(&format!("LA:{:<16}\r", label));
    h.push_str(&format!("ST:{:>7}\r", stitch_recs));
    h.push_str(&format!("CO:{:>3}\r", colors));
    h.push_str(&format!("+X:{:>5}\r", maxx));
    h.push_str(&format!("-X:{:>5}\r", -minx));
    h.push_str(&format!("+Y:{:>5}\r", maxy));
    h.push_str(&format!("-Y:{:>5}\r", -miny));
    h.push_str(&format!("AX:{:+6}\r", cx));
    h.push_str(&format!("AY:{:+6}\r", cy));
    h.push_str("MX:+    0\rMY:+    0\rPD:******\r");
    let mut out = h.into_bytes();
    out.push(0x1A);
    out.resize(512, b' ');
    for r in recs {
        out.extend_from_slice(&r);
    }
    out
}

#[derive(Debug, Default)]
pub struct Decoded {
    pub label: String,
    pub stitches: Vec<(i32, i32)>,
    pub jumps: usize,
    pub color_changes: usize,
    pub ended: bool,
    pub header_stitches: usize,
    pub header_colors: usize,
}

pub fn read(bytes: &[u8]) -> Result<Decoded, String> {
    if bytes.len() < 512 {
        return Err("file too short to be DST".into());
    }
    let header = String::from_utf8_lossy(&bytes[..512]);
    let field = |k: &str| -> Option<String> {
        header
            .split('\r')
            .find(|l| l.starts_with(k))
            .map(|l| l[k.len()..].trim().to_string())
    };
    let mut d = Decoded {
        label: field("LA:").unwrap_or_default(),
        header_stitches: field("ST:").and_then(|v| v.parse().ok()).unwrap_or(0),
        header_colors: field("CO:").and_then(|v| v.parse().ok()).unwrap_or(0),
        ..Default::default()
    };
    let (mut x, mut y) = (0, 0);
    for r in bytes[512..].chunks_exact(3) {
        let b = [r[0], r[1], r[2]];
        if b[2] == 0xF3 {
            d.ended = true;
            break;
        }
        if b[2] & 0x40 != 0 {
            d.color_changes += 1;
            continue;
        }
        let (dx, dy) = decode(b);
        x += dx;
        y += dy;
        if b[2] & 0x80 != 0 {
            d.jumps += 1;
        } else {
            d.stitches.push((x, y));
        }
    }
    Ok(d)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn every_move_round_trips() {
        for dx in -MAX..=MAX {
            for dy in [-MAX, -40, -1, 0, 1, 13, 80, MAX] {
                let b = encode(dx, dy, 0);
                assert_eq!(decode(b), (dx, dy), "move {dx},{dy}");
                let b = encode(dy, dx, 0x80);
                assert_eq!(decode(b), (dy, dx));
            }
        }
    }
}
