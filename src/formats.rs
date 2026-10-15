//! Machine formats besides DST: Melco EXP and Brother PES (with its PEC
//! section), plus readers for DST, EXP and PES so existing stitch files can
//! be viewed, re-coloured, documented and converted.
//!
//! Written from public descriptions of these formats. Machine units are
//! 0.1 mm. Inside StitchForge, y runs down (screen convention).

use crate::design::{Cmd, Design};
use crate::dst;
use crate::geom::Pt;

/// A stitch file read from disk.
pub struct MachineFile {
    /// Commands in mm, y down, with `Cmd::Color(i)` at the start of each stop.
    pub cmds: Vec<Cmd>,
    /// Colours stored in the file, one per stop, when the format has them.
    pub stop_colors: Vec<Option<[u8; 3]>>,
    pub label: String,
    pub format: &'static str,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Format {
    Dst,
    Exp,
    Pes,
}

impl Format {
    pub const ALL: [Format; 3] = [Format::Dst, Format::Pes, Format::Exp];
    pub fn ext(&self) -> &'static str {
        match self {
            Format::Dst => "dst",
            Format::Exp => "exp",
            Format::Pes => "pes",
        }
    }
    pub fn label(&self) -> &'static str {
        match self {
            Format::Dst => "DST (Tajima, most commercial machines)",
            Format::Exp => "EXP (Melco / Bernina)",
            Format::Pes => "PES (Brother / Babylock)",
        }
    }
    pub fn from_ext(ext: &str) -> Option<Format> {
        match ext.to_ascii_lowercase().as_str() {
            "dst" => Some(Format::Dst),
            "exp" => Some(Format::Exp),
            "pes" => Some(Format::Pes),
            _ => None,
        }
    }
}

pub fn write(d: &Design, f: Format) -> Vec<u8> {
    match f {
        Format::Dst => dst::write(d),
        Format::Exp => write_exp(d),
        Format::Pes => write_pes(d),
    }
}

/// Absolute machine positions (0.1 mm, centred on the design) for each
/// command, with y in the requested direction.
fn machine_moves(d: &Design, y_up: bool) -> Vec<(Cmd, i32, i32)> {
    let c = d.bbox.center();
    d.cmds
        .iter()
        .map(|cmd| {
            let p = match cmd {
                Cmd::Stitch(p) | Cmd::Jump(p) => *p,
                _ => Pt::new(c.x, c.y),
            };
            let x = ((p.x - c.x) * 10.0).round() as i32;
            let y = ((p.y - c.y) * 10.0).round() as i32;
            (*cmd, x, if y_up { -y } else { y })
        })
        .collect()
}

// ------------------------------------------------------------------ EXP

pub fn write_exp(d: &Design) -> Vec<u8> {
    let mut out = Vec::new();
    let (mut cx, mut cy) = (0i32, 0i32);
    let mut first_color = true;
    for (cmd, x, y) in machine_moves(d, true) {
        match cmd {
            Cmd::Stitch(_) | Cmd::Jump(_) => {
                let jump = matches!(cmd, Cmd::Jump(_));
                loop {
                    let dx = (x - cx).clamp(-127, 127);
                    let dy = (y - cy).clamp(-127, 127);
                    let done = cx + dx == x && cy + dy == y;
                    if jump || !done {
                        if dx == 0 && dy == 0 {
                            break;
                        }
                        out.extend_from_slice(&[0x80, 0x04, dx as i8 as u8, dy as i8 as u8]);
                    } else {
                        out.extend_from_slice(&[dx as i8 as u8, dy as i8 as u8]);
                    }
                    cx += dx;
                    cy += dy;
                    if done {
                        break;
                    }
                }
            }
            Cmd::Trim => out.extend_from_slice(&[0x80, 0x80, 0x07, 0x00]),
            Cmd::Color(_) => {
                if first_color {
                    first_color = false;
                } else {
                    out.extend_from_slice(&[0x80, 0x01, 0x00, 0x00]);
                }
            }
            Cmd::End => {}
        }
    }
    out
}

pub fn read_exp(bytes: &[u8]) -> Result<MachineFile, String> {
    let mut cmds = vec![Cmd::Color(0)];
    let mut stops = 1usize;
    let (mut x, mut y) = (0i32, 0i32);
    let mut i = 0;
    let at = |x: i32, y: i32| Pt::new(x as f64 / 10.0, -y as f64 / 10.0);
    while i + 1 < bytes.len() {
        let (b0, b1) = (bytes[i], bytes[i + 1]);
        i += 2;
        if b0 == 0x80 {
            if b1 == 0x04 {
                if i + 1 >= bytes.len() {
                    break;
                }
                x += bytes[i] as i8 as i32;
                y += bytes[i + 1] as i8 as i32;
                i += 2;
                cmds.push(Cmd::Jump(at(x, y)));
            } else if b1 & 0x01 != 0 {
                i += 2;
                cmds.push(Cmd::Trim);
                cmds.push(Cmd::Color(stops));
                stops += 1;
            } else if b1 == 0x80 {
                i += 2;
                cmds.push(Cmd::Trim);
            } else {
                i += 2;
            }
        } else {
            x += b0 as i8 as i32;
            y += b1 as i8 as i32;
            cmds.push(Cmd::Stitch(at(x, y)));
        }
    }
    cmds.push(Cmd::End);
    Ok(MachineFile {
        cmds,
        stop_colors: vec![None; stops],
        label: String::new(),
        format: "EXP",
    })
}

// ------------------------------------------------------------------ DST read

/// Full DST reader: stitches, jumps, colour changes, trims (runs of empty
/// jumps), all converted to mm with y down.
pub fn read_dst(bytes: &[u8]) -> Result<MachineFile, String> {
    let info = dst::read(bytes)?;
    let mut cmds = vec![Cmd::Color(0)];
    let mut stops = 1usize;
    let (mut x, mut y) = (0i32, 0i32);
    let at = |x: i32, y: i32| Pt::new(x as f64 / 10.0, -y as f64 / 10.0);
    // Machines trim on a run of consecutive jump records; read it the same way.
    let mut jump_run = 0;
    for r in bytes[512..].chunks_exact(3) {
        let b = [r[0], r[1], r[2]];
        if b[2] == 0xF3 {
            break;
        }
        if b[2] & 0x40 != 0 {
            cmds.push(Cmd::Trim);
            cmds.push(Cmd::Color(stops));
            stops += 1;
            jump_run = 0;
            continue;
        }
        let (dx, dy) = dst::decode(b);
        x += dx;
        y += dy;
        if b[2] & 0x80 != 0 {
            jump_run += 1;
            if jump_run == 2 && !matches!(cmds.last(), Some(Cmd::Trim)) {
                cmds.push(Cmd::Trim);
            }
            if dx != 0 || dy != 0 {
                cmds.push(Cmd::Jump(at(x, y)));
            }
        } else {
            jump_run = 0;
            cmds.push(Cmd::Stitch(at(x, y)));
        }
    }
    cmds.push(Cmd::End);
    Ok(MachineFile {
        cmds,
        stop_colors: vec![None; stops],
        label: info.label,
        format: "DST",
    })
}

// ------------------------------------------------------------------ PES / PEC

/// The Brother/PEC palette: 64 standard colours (index 1-64). Machines show
/// these in their colour lists; the thread actually sewn is up to the operator.
pub const PEC_PALETTE: [(&str, [u8; 3]); 64] = [
    ("Prussian Blue", [14, 31, 124]),
    ("Blue", [10, 85, 163]),
    ("Teal Green", [48, 135, 119]),
    ("Cornflower Blue", [75, 107, 175]),
    ("Red", [237, 23, 31]),
    ("Reddish Brown", [209, 92, 0]),
    ("Magenta", [145, 54, 151]),
    ("Light Lilac", [228, 154, 203]),
    ("Lilac", [145, 95, 172]),
    ("Mint Green", [158, 214, 125]),
    ("Deep Gold", [232, 169, 0]),
    ("Orange", [254, 186, 53]),
    ("Yellow", [255, 255, 0]),
    ("Lime Green", [112, 188, 31]),
    ("Brass", [186, 152, 0]),
    ("Silver", [168, 168, 168]),
    ("Russet Brown", [125, 111, 0]),
    ("Cream Brown", [255, 255, 179]),
    ("Pewter", [79, 85, 86]),
    ("Black", [0, 0, 0]),
    ("Ultramarine", [11, 61, 145]),
    ("Royal Purple", [119, 1, 118]),
    ("Dark Gray", [41, 49, 51]),
    ("Dark Brown", [42, 19, 1]),
    ("Deep Rose", [246, 74, 138]),
    ("Light Brown", [178, 118, 36]),
    ("Salmon Pink", [252, 187, 197]),
    ("Vermilion", [254, 55, 15]),
    ("White", [240, 240, 240]),
    ("Violet", [106, 28, 138]),
    ("Seacrest", [168, 221, 196]),
    ("Sky Blue", [37, 132, 187]),
    ("Pumpkin", [254, 179, 67]),
    ("Cream Yellow", [255, 243, 107]),
    ("Khaki", [208, 166, 96]),
    ("Clay Brown", [209, 84, 0]),
    ("Leaf Green", [102, 186, 73]),
    ("Peacock Blue", [19, 74, 70]),
    ("Gray", [135, 135, 135]),
    ("Warm Gray", [216, 204, 198]),
    ("Dark Olive", [67, 86, 7]),
    ("Linen", [253, 217, 222]),
    ("Pink", [249, 147, 188]),
    ("Deep Green", [0, 56, 34]),
    ("Lavender", [178, 175, 212]),
    ("Wisteria Violet", [104, 106, 176]),
    ("Beige", [239, 227, 185]),
    ("Carmine", [247, 56, 102]),
    ("Amber Red", [181, 75, 100]),
    ("Olive Green", [19, 43, 26]),
    ("Dark Fuchsia", [199, 1, 86]),
    ("Tangerine", [254, 158, 50]),
    ("Light Blue", [168, 222, 235]),
    ("Emerald Green", [0, 103, 62]),
    ("Purple", [78, 41, 144]),
    ("Moss Green", [47, 126, 32]),
    ("Flesh Pink", [255, 204, 204]),
    ("Harvest Gold", [255, 217, 17]),
    ("Electric Blue", [9, 91, 166]),
    ("Lemon Yellow", [240, 249, 112]),
    ("Fresh Green", [227, 243, 91]),
    ("Applique Material", [255, 153, 0]),
    ("Applique Position", [255, 240, 141]),
    ("Applique", [255, 200, 200]),
];

fn pec_index(rgb: [u8; 3]) -> u8 {
    // Skip the three appliqué pseudo-colours when matching.
    let mut best = (0usize, f64::MAX);
    for (i, (_, c)) in PEC_PALETTE.iter().enumerate().take(61) {
        let d = crate::thread::delta_e(*c, rgb);
        if d < best.1 {
            best = (i, d);
        }
    }
    best.0 as u8 + 1
}

const ICON_W: usize = 48;
const ICON_H: usize = 38;

/// A 48×38 one-bit preview, framed, for the machine's screen.
fn pec_icon(d: &Design, stop: Option<usize>) -> Vec<u8> {
    let mut px = vec![false; ICON_W * ICON_H];
    for x in 4..44 {
        px[2 * ICON_W + x] = true;
        px[35 * ICON_W + x] = true;
    }
    for y in 4..34 {
        px[y * ICON_W + 1] = true;
        px[y * ICON_W + 46] = true;
    }
    for (x, y) in [
        (2, 3),
        (3, 2),
        (44, 2),
        (45, 3),
        (2, 34),
        (3, 35),
        (44, 35),
        (45, 34),
    ] {
        px[y * ICON_W + x] = true;
    }
    let b = d.bbox;
    let k = (38.0 / b.width().max(1e-6)).min(28.0 / b.height().max(1e-6));
    let ox = 24.0 - b.width() * k / 2.0;
    let oy = 19.0 - b.height() * k / 2.0;
    let mut cur_stop = 0usize;
    for c in &d.cmds {
        match c {
            Cmd::Color(i) => cur_stop = *i,
            Cmd::Stitch(p) if stop.map(|s| s == cur_stop).unwrap_or(true) => {
                let x = (ox + (p.x - b.min.x) * k).round() as i64;
                let y = (oy + (p.y - b.min.y) * k).round() as i64;
                if (3..45).contains(&x) && (4..34).contains(&y) {
                    px[y as usize * ICON_W + x as usize] = true;
                }
            }
            _ => {}
        }
    }
    let mut out = vec![0u8; ICON_W / 8 * ICON_H];
    for y in 0..ICON_H {
        for x in 0..ICON_W {
            if px[y * ICON_W + x] {
                out[y * (ICON_W / 8) + x / 8] |= 1 << (x % 8);
            }
        }
    }
    out
}

fn put_long(out: &mut Vec<u8>, v: i32, flag: u8) {
    let v = (v & 0x0FFF) as u16;
    out.push(0x80 | flag | (v >> 8) as u8);
    out.push((v & 0xFF) as u8);
}

pub fn write_pes(d: &Design) -> Vec<u8> {
    // Short PES header pointing straight at the PEC section, which is what
    // Brother machines read.
    let mut out = b"#PES0001".to_vec();
    out.extend_from_slice(&22u32.to_le_bytes());
    out.extend_from_slice(&[0u8; 10]);

    // ---- PEC header (512 bytes)
    let label: String = d
        .name
        .chars()
        .filter(|c| c.is_ascii_graphic() || *c == ' ')
        .take(16)
        .collect();
    out.extend_from_slice(format!("LA:{:<16}\r", label).as_bytes());
    out.extend_from_slice(&[0x20; 12]);
    out.extend_from_slice(&[0xFF, 0x00, (ICON_W / 8) as u8, ICON_H as u8]);
    out.extend_from_slice(&[0x20; 12]);
    let colors: Vec<u8> = d.stops.iter().map(|s| pec_index(s.thread.rgb)).collect();
    let n = colors.len().max(1);
    out.push((n - 1) as u8);
    out.extend_from_slice(&colors);
    for _ in n..463 {
        out.push(0x20);
    }

    // ---- stitch block
    let moves = machine_moves(d, false);
    let (mut minx, mut miny, mut maxx, mut maxy) = (0i32, 0i32, 0i32, 0i32);
    for (c, x, y) in &moves {
        if matches!(c, Cmd::Stitch(_) | Cmd::Jump(_)) {
            minx = minx.min(*x);
            miny = miny.min(*y);
            maxx = maxx.max(*x);
            maxy = maxy.max(*y);
        }
    }
    let block_start = out.len();
    out.extend_from_slice(&[0x00, 0x00, 0, 0, 0]); // length patched below
    out.extend_from_slice(&[0x31, 0xFF, 0xF0]);
    out.extend_from_slice(&((maxx - minx) as u16).to_le_bytes());
    out.extend_from_slice(&((maxy - miny) as u16).to_le_bytes());
    out.extend_from_slice(&0x1E0u16.to_le_bytes());
    out.extend_from_slice(&0x1B0u16.to_le_bytes());
    out.extend_from_slice(&(0x9000u16 | ((-minx) as u16 & 0x0FFF)).to_be_bytes());
    out.extend_from_slice(&(0x9000u16 | ((-miny) as u16 & 0x0FFF)).to_be_bytes());

    let (mut cx, mut cy) = (0i32, 0i32);
    let mut color_two = true;
    let mut first_color = true;
    let mut trim_pending = false;
    for (cmd, x, y) in moves {
        match cmd {
            Cmd::Stitch(_) | Cmd::Jump(_) => {
                let jump = matches!(cmd, Cmd::Jump(_));
                loop {
                    let dx = (x - cx).clamp(-2047, 2047);
                    let dy = (y - cy).clamp(-2047, 2047);
                    let done = cx + dx == x && cy + dy == y;
                    if dx == 0 && dy == 0 && (jump || !done) {
                        break;
                    }
                    if jump || !done {
                        let flag = if trim_pending { 0x20 } else { 0x10 };
                        put_long(&mut out, dx, flag);
                        put_long(&mut out, dy, flag);
                        trim_pending = false;
                    } else if (-64..63).contains(&dx) && (-64..63).contains(&dy) {
                        out.push((dx & 0x7F) as u8);
                        out.push((dy & 0x7F) as u8);
                    } else {
                        put_long(&mut out, dx, 0);
                        put_long(&mut out, dy, 0);
                    }
                    cx += dx;
                    cy += dy;
                    if done {
                        break;
                    }
                }
            }
            Cmd::Trim => trim_pending = true,
            Cmd::Color(_) => {
                if first_color {
                    first_color = false;
                } else {
                    out.extend_from_slice(&[0xFE, 0xB0, if color_two { 2 } else { 1 }]);
                    color_two = !color_two;
                }
            }
            Cmd::End => {}
        }
    }
    out.push(0xFF);
    let block_len = (out.len() - block_start) as u32;
    out[block_start + 2] = (block_len & 0xFF) as u8;
    out[block_start + 3] = ((block_len >> 8) & 0xFF) as u8;
    out[block_start + 4] = ((block_len >> 16) & 0xFF) as u8;

    // ---- preview icons: whole design, then one per colour stop
    out.extend(pec_icon(d, None));
    for i in 0..d.stops.len() {
        out.extend(pec_icon(d, Some(i)));
    }
    out
}

pub fn read_pes(bytes: &[u8]) -> Result<MachineFile, String> {
    if bytes.len() < 12 || &bytes[0..4] != b"#PES" {
        return Err("not a PES file".into());
    }
    let pec = u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]) as usize;
    if pec + 512 + 20 > bytes.len() {
        return Err("PES file is truncated".into());
    }
    let label = String::from_utf8_lossy(&bytes[pec + 3..pec + 19])
        .trim()
        .to_string();
    let n = bytes[pec + 48] as usize + 1;
    let stop_colors: Vec<Option<[u8; 3]>> = (0..n)
        .map(|i| {
            let idx = bytes.get(pec + 49 + i).copied().unwrap_or(0) as usize;
            PEC_PALETTE.get(idx.wrapping_sub(1)).map(|(_, c)| *c)
        })
        .collect();
    let mut i = pec + 512 + 20;
    let mut cmds = vec![Cmd::Color(0)];
    let mut stops = 1usize;
    let (mut x, mut y) = (0i32, 0i32);
    let at = |x: i32, y: i32| Pt::new(x as f64 / 10.0, y as f64 / 10.0);
    let read_val = |i: &mut usize| -> Option<(i32, u8)> {
        let b = *bytes.get(*i)?;
        if b & 0x80 != 0 {
            let lo = *bytes.get(*i + 1)?;
            *i += 2;
            let mut v = (((b & 0x0F) as i32) << 8) | lo as i32;
            if v & 0x800 != 0 {
                v -= 0x1000;
            }
            Some((v, b & 0x70))
        } else {
            *i += 1;
            let v = if b & 0x40 != 0 {
                b as i32 - 0x80
            } else {
                b as i32
            };
            Some((v, 0))
        }
    };
    while i < bytes.len() {
        let b = bytes[i];
        if b == 0xFF {
            break;
        }
        if b == 0xFE && bytes.get(i + 1) == Some(&0xB0) {
            i += 3;
            cmds.push(Cmd::Trim);
            cmds.push(Cmd::Color(stops));
            stops += 1;
            continue;
        }
        let Some((dx, fx)) = read_val(&mut i) else {
            break;
        };
        let Some((dy, fy)) = read_val(&mut i) else {
            break;
        };
        x += dx;
        y += dy;
        let flags = fx | fy;
        if flags & 0x20 != 0 {
            cmds.push(Cmd::Trim);
            cmds.push(Cmd::Jump(at(x, y)));
        } else if flags & 0x10 != 0 {
            cmds.push(Cmd::Jump(at(x, y)));
        } else {
            cmds.push(Cmd::Stitch(at(x, y)));
        }
    }
    cmds.push(Cmd::End);
    let mut stop_colors = stop_colors;
    stop_colors.resize(stops, None);
    Ok(MachineFile {
        cmds,
        stop_colors,
        label,
        format: "PES",
    })
}

/// Read any supported stitch file by its extension.
pub fn read_file(path: &std::path::Path) -> Result<MachineFile, String> {
    let bytes = crate::vfs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_string())
        .unwrap_or_default();
    let mut f = match Format::from_ext(&ext) {
        Some(Format::Dst) => read_dst(&bytes)?,
        Some(Format::Exp) => read_exp(&bytes)?,
        Some(Format::Pes) => read_pes(&bytes)?,
        None => return Err(format!("{}: not a DST, EXP or PES file", path.display())),
    };
    if f.label.is_empty() {
        f.label = path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
    }
    Ok(f)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::design::build;
    use crate::import::{fit, load_svg_str};
    use crate::settings::{Profile, recipe};

    fn sample() -> Design {
        let svg = include_str!("../examples/left-chest-logo.svg");
        let (opt, _) = crate::import::font_options(None).unwrap();
        let mut art = load_svg_str(svg, &opt).unwrap();
        fit(&mut art, Some(80.0), None);
        build(
            "sample",
            &art,
            &crate::thread::generic_chart(),
            &recipe("pique").unwrap(),
            Profile::Flat,
        )
        .unwrap()
    }

    fn stitch_positions(cmds: &[Cmd]) -> Vec<(i32, i32)> {
        cmds.iter()
            .filter_map(|c| {
                if let Cmd::Stitch(p) = c {
                    Some(((p.x * 10.0).round() as i32, (p.y * 10.0).round() as i32))
                } else {
                    None
                }
            })
            .collect()
    }

    fn check_round_trip(f: Format) {
        let d = sample();
        let bytes = write(&d, f);
        let back = match f {
            Format::Dst => read_dst(&bytes).unwrap(),
            Format::Exp => read_exp(&bytes).unwrap(),
            Format::Pes => read_pes(&bytes).unwrap(),
        };
        let c = d.bbox.center();
        let want: Vec<(i32, i32)> = d
            .cmds
            .iter()
            .filter_map(|c2| {
                if let Cmd::Stitch(p) = c2 {
                    Some((
                        ((p.x - c.x) * 10.0).round() as i32,
                        ((p.y - c.y) * 10.0).round() as i32,
                    ))
                } else {
                    None
                }
            })
            .collect();
        let got = stitch_positions(&back.cmds);
        assert_eq!(got.len(), want.len(), "{f:?} stitch count");
        let worst = want
            .iter()
            .zip(&got)
            .map(|(a, b)| (a.0 - b.0).abs().max((a.1 - b.1).abs()))
            .max()
            .unwrap();
        assert!(worst <= 1, "{f:?} position error {worst} units");
        assert_eq!(back.stop_colors.len(), d.stops.len(), "{f:?} stops");
    }

    #[test]
    fn sample_keeps_its_colour_stops() {
        assert_eq!(sample().stops.len(), 6);
    }

    #[test]
    fn dst_round_trip() {
        check_round_trip(Format::Dst);
    }
    #[test]
    fn exp_round_trip() {
        check_round_trip(Format::Exp);
    }
    #[test]
    fn pes_round_trip() {
        check_round_trip(Format::Pes);
    }
}
