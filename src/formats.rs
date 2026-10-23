//! Machine formats besides DST: Melco EXP, Brother PES and PEC, Janome JEF
//! and Happy TAP, plus readers for all of them so existing stitch files can
//! be viewed, re-coloured, documented, converted, and turned back into
//! editable shapes.
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
    Pec,
    Jef,
    Tap,
    Vp3,
    Xxx,
    U01,
}

/// File extensions StitchForge can open as stitch files.
pub const READ_EXTS: [&str; 9] = ["dst", "exp", "pes", "pec", "jef", "tap", "vp3", "xxx", "u01"];

impl Format {
    pub const ALL: [Format; 9] = [
        Format::Dst,
        Format::Pes,
        Format::Exp,
        Format::Jef,
        Format::Vp3,
        Format::Xxx,
        Format::U01,
        Format::Tap,
        Format::Pec,
    ];
    pub fn ext(&self) -> &'static str {
        match self {
            Format::Dst => "dst",
            Format::Exp => "exp",
            Format::Pes => "pes",
            Format::Pec => "pec",
            Format::Jef => "jef",
            Format::Tap => "tap",
            Format::Vp3 => "vp3",
            Format::Xxx => "xxx",
            Format::U01 => "u01",
        }
    }
    pub fn label(&self) -> &'static str {
        match self {
            Format::Dst => "DST (Tajima, most commercial machines)",
            Format::Exp => "EXP (Melco / Bernina)",
            Format::Pes => "PES (Brother / Babylock)",
            Format::Pec => "PEC (older Brother / Babylock)",
            Format::Jef => "JEF (Janome / Elna / Kenmore)",
            Format::Tap => "TAP (Happy)",
            Format::Vp3 => "VP3 (Husqvarna Viking / Pfaff)",
            Format::Xxx => "XXX (Singer)",
            Format::U01 => "U01 (Barudan)",
        }
    }
    pub fn from_ext(ext: &str) -> Option<Format> {
        match ext.to_ascii_lowercase().as_str() {
            "dst" => Some(Format::Dst),
            "exp" => Some(Format::Exp),
            "pes" => Some(Format::Pes),
            "pec" => Some(Format::Pec),
            "jef" => Some(Format::Jef),
            "tap" => Some(Format::Tap),
            "vp3" => Some(Format::Vp3),
            "xxx" => Some(Format::Xxx),
            "u01" => Some(Format::U01),
            _ => None,
        }
    }
}

pub fn write(d: &Design, f: Format) -> Vec<u8> {
    match f {
        Format::Dst => dst::write(d),
        Format::Exp => write_exp(d),
        Format::Pes => write_pes(d),
        Format::Pec => write_pec(d),
        Format::Jef => write_jef(d),
        Format::Tap => write_tap(d),
        Format::Vp3 => write_vp3(d),
        Format::Xxx => write_xxx(d),
        Format::U01 => write_u01(d),
    }
}

/// Read stitch-file bytes in the given format.
pub fn read_bytes(bytes: &[u8], f: Format) -> Result<MachineFile, String> {
    match f {
        Format::Dst => read_dst(bytes),
        Format::Exp => read_exp(bytes),
        Format::Pes => read_pes(bytes),
        Format::Pec => read_pec(bytes),
        Format::Jef => read_jef(bytes),
        Format::Tap => read_tap(bytes),
        Format::Vp3 => read_vp3(bytes),
        Format::Xxx => read_xxx(bytes),
        Format::U01 => read_u01(bytes),
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
    Ok(dst_records(&bytes[512..], info.label, "DST"))
}

// ------------------------------------------------------------------ TAP

/// Happy TAP: the same 3-byte stitch records as DST, with no header.
pub fn write_tap(d: &Design) -> Vec<u8> {
    dst::write(d)[512..].to_vec()
}

pub fn read_tap(bytes: &[u8]) -> Result<MachineFile, String> {
    if bytes.len() < 3 {
        return Err("file too short to be TAP".into());
    }
    Ok(dst_records(bytes, String::new(), "TAP"))
}

fn dst_records(recs: &[u8], label: String, format: &'static str) -> MachineFile {
    let mut cmds = vec![Cmd::Color(0)];
    let mut stops = 1usize;
    let (mut x, mut y) = (0i32, 0i32);
    let at = |x: i32, y: i32| Pt::new(x as f64 / 10.0, -y as f64 / 10.0);
    // Machines trim on a run of consecutive jump records; read it the same way.
    let mut jump_run = 0;
    for r in recs.chunks_exact(3) {
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
    MachineFile {
        cmds,
        stop_colors: vec![None; stops],
        label,
        format,
    }
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

// ------------------------------------------------------------------ PEC

/// A bare PEC file: the PEC section of a PES file behind its own signature.
pub fn write_pec(d: &Design) -> Vec<u8> {
    let pes = write_pes(d);
    let mut out = b"#PEC0001".to_vec();
    out.extend_from_slice(&pes[22..]);
    out
}

pub fn read_pec(bytes: &[u8]) -> Result<MachineFile, String> {
    if bytes.len() < 8 || &bytes[0..4] != b"#PEC" {
        return Err("not a PEC file".into());
    }
    // wrap it as a PES file whose PEC section starts right after the header
    let mut pes = b"#PES0001".to_vec();
    pes.extend_from_slice(&12u32.to_le_bytes());
    pes.extend_from_slice(&bytes[8..]);
    let mut f = read_pes(&pes)?;
    f.format = "PEC";
    Ok(f)
}

// ------------------------------------------------------------------ JEF

/// Janome's basic colour numbers (1-based). Like PEC, the number only picks
/// the colour the machine shows; the operator threads what the sheet says.
pub const JEF_PALETTE: [(&str, [u8; 3]); 30] = [
    ("Black", [0, 0, 0]),
    ("White", [255, 255, 255]),
    ("Yellow", [255, 255, 23]),
    ("Orange", [255, 102, 0]),
    ("Olive Green", [47, 89, 51]),
    ("Green", [35, 115, 54]),
    ("Sky", [101, 194, 200]),
    ("Purple", [171, 90, 150]),
    ("Pink", [246, 105, 160]),
    ("Red", [255, 0, 0]),
    ("Brown", [177, 112, 78]),
    ("Blue", [11, 47, 132]),
    ("Gold", [228, 195, 93]),
    ("Dark Brown", [72, 26, 5]),
    ("Pale Violet", [172, 156, 199]),
    ("Pale Yellow", [252, 242, 148]),
    ("Pale Pink", [249, 153, 183]),
    ("Peach", [250, 179, 129]),
    ("Beige", [201, 164, 128]),
    ("Wine Red", [151, 5, 51]),
    ("Pale Sky", [160, 184, 204]),
    ("Yellow Green", [127, 194, 28]),
    ("Silver Grey", [229, 229, 229]),
    ("Grey", [136, 155, 155]),
    ("Pale Aqua", [152, 214, 189]),
    ("Baby Blue", [178, 225, 227]),
    ("Powder Blue", [54, 139, 160]),
    ("Bright Blue", [79, 131, 171]),
    ("Slate Blue", [56, 106, 145]),
    ("Navy Blue", [7, 22, 80]),
];

fn jef_index(rgb: [u8; 3]) -> u32 {
    let d = |c: [u8; 3]| -> i32 { (0..3).map(|k| (c[k] as i32 - rgb[k] as i32).pow(2)).sum() };
    JEF_PALETTE
        .iter()
        .enumerate()
        .min_by_key(|(_, (_, c))| d(*c))
        .map(|(i, _)| i as u32 + 1)
        .unwrap_or(1)
}

/// "YYYYMMDDHHMMSS" for the file header (a fixed date on the web, where
/// there's no clock to ask).
fn jef_date() -> String {
    #[cfg(not(target_arch = "wasm32"))]
    {
        if let Ok(t) = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
            let secs = t.as_secs() as i64;
            let (days, rem) = (secs.div_euclid(86400), secs.rem_euclid(86400));
            // civil date from days since 1970-01-01
            let z = days + 719468;
            let era = z.div_euclid(146097);
            let doe = z - era * 146097;
            let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
            let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
            let mp = (5 * doy + 2) / 153;
            let day = doy - (153 * mp + 2) / 5 + 1;
            let month = if mp < 10 { mp + 3 } else { mp - 9 };
            let year = yoe + era * 400 + if month <= 2 { 1 } else { 0 };
            return format!(
                "{year:04}{month:02}{day:02}{:02}{:02}{:02}",
                rem / 3600,
                rem / 60 % 60,
                rem % 60
            );
        }
    }
    "20260101120000".into()
}

pub fn write_jef(d: &Design) -> Vec<u8> {
    let moves = machine_moves(d, true);
    // ---- stitch block first, so the header can count it
    let mut body = Vec::new();
    let (mut cx, mut cy) = (0i32, 0i32);
    let (mut minx, mut miny, mut maxx, mut maxy) = (0i32, 0i32, 0i32, 0i32);
    let mut first_color = true;
    for (cmd, x, y) in moves {
        match cmd {
            Cmd::Stitch(_) | Cmd::Jump(_) => {
                minx = minx.min(x);
                maxx = maxx.max(x);
                miny = miny.min(y);
                maxy = maxy.max(y);
                let jump = matches!(cmd, Cmd::Jump(_));
                loop {
                    let dx = (x - cx).clamp(-127, 127);
                    let dy = (y - cy).clamp(-127, 127);
                    let done = cx + dx == x && cy + dy == y;
                    if jump || !done {
                        if dx == 0 && dy == 0 {
                            break;
                        }
                        body.extend_from_slice(&[0x80, 0x02, dx as i8 as u8, dy as i8 as u8]);
                    } else {
                        body.extend_from_slice(&[dx as i8 as u8, dy as i8 as u8]);
                    }
                    cx += dx;
                    cy += dy;
                    if done {
                        break;
                    }
                }
            }
            Cmd::Trim => body.extend_from_slice(&[0x80, 0x02, 0, 0]),
            Cmd::Color(_) => {
                if first_color {
                    first_color = false;
                } else {
                    body.extend_from_slice(&[0x80, 0x01, 0, 0]);
                }
            }
            Cmd::End => {}
        }
    }
    body.extend_from_slice(&[0x80, 0x10]);

    // ---- header
    let colors: Vec<u32> = d.stops.iter().map(|s| jef_index(s.thread.rgb)).collect();
    let n = colors.len().max(1);
    let mut out = Vec::new();
    let put = |out: &mut Vec<u8>, v: i32| out.extend_from_slice(&v.to_le_bytes());
    put(&mut out, (116 + 8 * n) as i32);
    put(&mut out, 0x14);
    out.extend_from_slice(jef_date().as_bytes());
    out.extend_from_slice(&[0, 0]);
    put(&mut out, n as i32);
    put(&mut out, (body.len() / 2) as i32);
    // design extents from the centre: left, top, right, bottom
    let ext = [-minx, maxy, maxx, -miny];
    let (w, h) = (ext[0].max(ext[2]), ext[1].max(ext[3]));
    // smallest standard hoop it fits (codes: 0 = 126×110, 1 = 110×110,
    // 2 = 50×50, 3 = 140×200, 4 = 230×200)
    let hoop = [(2, 250, 250), (1, 550, 550), (0, 630, 550), (3, 700, 1000), (4, 1150, 1000)]
        .iter()
        .find(|(_, hx, hy)| w <= *hx && h <= *hy)
        .map(|(c, _, _)| *c)
        .unwrap_or(4);
    put(&mut out, hoop);
    for v in ext {
        put(&mut out, v);
    }
    for (hx, hy) in [(550, 550), (250, 250), (700, 1000), (1150, 1000)] {
        if w <= hx && h <= hy {
            for v in [hx - ext[0], hy - ext[1], hx - ext[2], hy - ext[3]] {
                put(&mut out, v);
            }
        } else {
            for _ in 0..4 {
                put(&mut out, -1);
            }
        }
    }
    debug_assert_eq!(out.len(), 116);
    for i in 0..n {
        put(&mut out, colors.get(i).copied().unwrap_or(1) as i32);
    }
    for _ in 0..n {
        put(&mut out, 0x0D);
    }
    out.extend(body);
    out
}

pub fn read_jef(bytes: &[u8]) -> Result<MachineFile, String> {
    if bytes.len() < 116 {
        return Err("file too short to be JEF".into());
    }
    let u = |o: usize| u32::from_le_bytes([bytes[o], bytes[o + 1], bytes[o + 2], bytes[o + 3]]) as usize;
    let start = u(0);
    let n = u(24);
    if start > bytes.len() || n > 255 || 116 + 4 * n > bytes.len() {
        return Err("not a JEF file (header doesn't add up)".into());
    }
    let table: Vec<Option<[u8; 3]>> = (0..n)
        .map(|i| {
            let idx = u(116 + 4 * i);
            JEF_PALETTE.get(idx.wrapping_sub(1)).map(|(_, c)| *c)
        })
        .collect();
    let mut cmds = vec![Cmd::Color(0)];
    let mut stops = 1usize;
    let (mut x, mut y) = (0i32, 0i32);
    let at = |x: i32, y: i32| Pt::new(x as f64 / 10.0, -y as f64 / 10.0);
    let mut i = start;
    let mut jump_run = 0;
    while i + 1 < bytes.len() {
        if bytes[i] == 0x80 {
            let ctrl = bytes[i + 1];
            if ctrl == 0x10 || i + 3 >= bytes.len() {
                break;
            }
            let (dx, dy) = (bytes[i + 2] as i8 as i32, bytes[i + 3] as i8 as i32);
            i += 4;
            match ctrl {
                0x01 => {
                    x += dx;
                    y += dy;
                    if !matches!(cmds.last(), Some(Cmd::Trim)) {
                        cmds.push(Cmd::Trim);
                    }
                    cmds.push(Cmd::Color(stops));
                    stops += 1;
                    jump_run = 0;
                }
                0x02 => {
                    x += dx;
                    y += dy;
                    jump_run += 1;
                    // a move of nothing, or a run of moves, is a trim
                    if (dx == 0 && dy == 0 || jump_run == 2) && !matches!(cmds.last(), Some(Cmd::Trim)) {
                        cmds.push(Cmd::Trim);
                    }
                    if dx != 0 || dy != 0 {
                        cmds.push(Cmd::Jump(at(x, y)));
                    }
                }
                _ => {}
            }
        } else {
            x += bytes[i] as i8 as i32;
            y += bytes[i + 1] as i8 as i32;
            i += 2;
            jump_run = 0;
            cmds.push(Cmd::Stitch(at(x, y)));
        }
    }
    cmds.push(Cmd::End);
    let mut stop_colors = table;
    stop_colors.resize(stops, None);
    Ok(MachineFile {
        cmds,
        stop_colors,
        label: String::new(),
        format: "JEF",
    })
}

// ------------------------------------------------------------------ shared

/// Split a move into equal steps no longer than `max` units on either axis.
fn steps(dx: i32, dy: i32, max: i32) -> Vec<(i32, i32)> {
    let n = ((dx.abs().max(dy.abs()) + max - 1) / max).max(1);
    let mut out = Vec::with_capacity(n as usize);
    let (mut px, mut py) = (0, 0);
    for k in 1..=n {
        let x = (dx as f64 * k as f64 / n as f64).round() as i32;
        let y = (dy as f64 * k as f64 / n as f64).round() as i32;
        out.push((x - px, y - py));
        px = x;
        py = y;
    }
    out
}

/// Extents of the stitched and moved-to points (machine units, as given).
fn extents(moves: &[(Cmd, i32, i32)]) -> (i32, i32, i32, i32) {
    let (mut minx, mut miny, mut maxx, mut maxy) = (0, 0, 0, 0);
    for (c, x, y) in moves {
        if matches!(c, Cmd::Stitch(_) | Cmd::Jump(_)) {
            minx = minx.min(*x);
            miny = miny.min(*y);
            maxx = maxx.max(*x);
            maxy = maxy.max(*y);
        }
    }
    (minx, miny, maxx, maxy)
}

// ------------------------------------------------------------------ U01

/// Barudan U01: 3-byte records (control, |dy|, |dx|) after a 256-byte header.
pub fn write_u01(d: &Design) -> Vec<u8> {
    let moves = machine_moves(d, true);
    let mut recs: Vec<[u8; 3]> = vec![[0xE9, 0, 0]]; // needle 1
    let (mut cx, mut cy) = (0i32, 0i32);
    let mut stop = 0usize;
    let rec = |dx: i32, dy: i32, jump: bool| -> [u8; 3] {
        let mut c = 0x80u8;
        if dy <= 0 {
            c |= 0x40;
        }
        if dx < 0 || (dx == 0 && dy == 0) {
            c |= 0x20;
        }
        if jump {
            c |= 0x01;
        }
        [c, dy.unsigned_abs() as u8, dx.unsigned_abs() as u8]
    };
    for (cmd, x, y) in &moves {
        match cmd {
            Cmd::Stitch(_) | Cmd::Jump(_) => {
                let jump = matches!(cmd, Cmd::Jump(_));
                if jump && *x == cx && *y == cy {
                    continue;
                }
                let st = steps(x - cx, y - cy, 127);
                let n = st.len();
                for (i, (dx, dy)) in st.into_iter().enumerate() {
                    recs.push(rec(dx, dy, jump || i + 1 < n));
                }
                cx = *x;
                cy = *y;
            }
            Cmd::Trim => recs.push([0xE7, 0, 0]),
            Cmd::Color(i) => {
                if *i > 0 && stop != *i {
                    stop = *i;
                    recs.push([0xE8 + (stop % 7) as u8 + 1, 0, 0]);
                }
            }
            Cmd::End => {}
        }
    }
    recs.push([0xF8, 0, 0]);
    let (minx, miny, maxx, maxy) = extents(&moves);
    let mut out = vec![0x30u8; 0x80];
    for v in [minx, miny, maxx, maxy] {
        out.extend_from_slice(&(v as i16).to_le_bytes());
    }
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&(recs.len() as u32 + 1).to_le_bytes());
    out.extend_from_slice(&(cx as i16).to_le_bytes());
    out.extend_from_slice(&(cy as i16).to_le_bytes());
    out.resize(0x100, 0);
    for r in recs {
        out.extend_from_slice(&r);
    }
    out
}

pub fn read_u01(bytes: &[u8]) -> Result<MachineFile, String> {
    if bytes.len() < 0x100 {
        return Err("file too short to be U01".into());
    }
    let mut cmds = vec![Cmd::Color(0)];
    let mut stops = 1usize;
    let (mut x, mut y) = (0i32, 0i32);
    let at = |x: i32, y: i32| Pt::new(x as f64 / 10.0, -y as f64 / 10.0);
    let mut seen_needle = false;
    let mut jump_run = 0;
    for r in bytes[0x100..].chunks_exact(3) {
        let c = r[0];
        if c == 0xF8 {
            break;
        }
        let code = c & 0x0F;
        if code >= 0x08 {
            // needle change: the first one just picks the starting needle
            if seen_needle || cmds.iter().any(|c| matches!(c, Cmd::Stitch(_))) {
                if !matches!(cmds.last(), Some(Cmd::Trim)) {
                    cmds.push(Cmd::Trim);
                }
                cmds.push(Cmd::Color(stops));
                stops += 1;
            }
            seen_needle = true;
            jump_run = 0;
            continue;
        }
        if code == 0x07 {
            if !matches!(cmds.last(), Some(Cmd::Trim)) {
                cmds.push(Cmd::Trim);
            }
            continue;
        }
        let mut dy = r[1] as i32;
        let mut dx = r[2] as i32;
        if c & 0x40 != 0 {
            dy = -dy;
        }
        if c & 0x20 != 0 {
            dx = -dx;
        }
        x += dx;
        y += dy;
        if code == 0x01 {
            jump_run += 1;
            if jump_run == 3 && !matches!(cmds.last(), Some(Cmd::Trim)) {
                cmds.push(Cmd::Trim);
            }
            cmds.push(Cmd::Jump(at(x, y)));
        } else {
            jump_run = 0;
            cmds.push(Cmd::Stitch(at(x, y)));
        }
    }
    cmds.push(Cmd::End);
    Ok(MachineFile {
        cmds,
        stop_colors: vec![None; stops],
        label: String::new(),
        format: "U01",
    })
}

// ------------------------------------------------------------------ XXX

/// Singer XXX: byte-pair stitches after a 256-byte header, 0x7F escapes,
/// with a colour table at the end.
pub fn write_xxx(d: &Design) -> Vec<u8> {
    let moves = machine_moves(d, true);
    let mut body = Vec::new();
    let mut count = 0u32;
    let (mut cx, mut cy) = (0i32, 0i32);
    let mut first_color = true;
    for (cmd, x, y) in &moves {
        match cmd {
            Cmd::Stitch(_) | Cmd::Jump(_) => {
                let jump = matches!(cmd, Cmd::Jump(_));
                if jump && *x == cx && *y == cy {
                    continue;
                }
                let st = steps(x - cx, y - cy, 126);
                let n = st.len();
                for (i, (dx, dy)) in st.into_iter().enumerate() {
                    if jump || i + 1 < n {
                        body.extend_from_slice(&[0x7F, 0x01, dx as i8 as u8, dy as i8 as u8]);
                    } else {
                        body.extend_from_slice(&[dx as i8 as u8, dy as i8 as u8]);
                    }
                    count += 1;
                }
                cx = *x;
                cy = *y;
            }
            Cmd::Trim => {
                body.extend_from_slice(&[0x7F, 0x03, 0, 0]);
                count += 1;
            }
            Cmd::Color(_) => {
                if first_color {
                    first_color = false;
                } else {
                    body.extend_from_slice(&[0x7F, 0x08, 0, 0]);
                    count += 1;
                }
            }
            Cmd::End => {}
        }
    }
    let (minx, miny, maxx, maxy) = extents(&moves);
    let n = d.stops.len().max(1);
    let mut out = vec![0u8; 0x17];
    out.extend_from_slice(&count.to_le_bytes());
    out.resize(0x27, 0);
    out.extend_from_slice(&(n as u16).to_le_bytes());
    out.resize(0x2D, 0);
    // width, height, last position, distance left of and below the start
    for v in [maxx - minx, maxy - miny, cx, cy, -minx, -miny] {
        out.extend_from_slice(&(v as i16).to_le_bytes());
    }
    out.resize(0xF2, 0);
    out.extend_from_slice(&0x20u16.to_le_bytes());
    out.resize(0xFC, 0);
    let table = 0x100 + body.len();
    out.extend_from_slice(&(table as u32).to_le_bytes());
    out.extend(body);
    out.extend_from_slice(&[0x7F, 0x7F, 0x02, 0x14, 0, 0]);
    for i in 0..n.max(21) {
        match d.stops.get(i) {
            Some(s) => out.extend_from_slice(&[0, s.thread.rgb[0], s.thread.rgb[1], s.thread.rgb[2]]),
            None => out.extend_from_slice(&[0, 0, 0, 0]),
        }
    }
    out.extend_from_slice(&[0, 0xFF, 0xFF, 0xFF, 0, 1]);
    out
}

pub fn read_xxx(bytes: &[u8]) -> Result<MachineFile, String> {
    if bytes.len() < 0x100 {
        return Err("file too short to be XXX".into());
    }
    let n = u16::from_le_bytes([bytes[0x27], bytes[0x28]]) as usize;
    let table = u32::from_le_bytes([bytes[0xFC], bytes[0xFD], bytes[0xFE], bytes[0xFF]]) as usize;
    let end = if table > 0x100 && table <= bytes.len() { table } else { bytes.len() };
    let mut cmds = vec![Cmd::Color(0)];
    let mut stops = 1usize;
    let (mut x, mut y) = (0i32, 0i32);
    let at = |x: i32, y: i32| Pt::new(x as f64 / 10.0, -y as f64 / 10.0);
    let mut i = 0x100;
    let mut jump_run = 0;
    while i + 1 < end {
        if bytes[i] == 0x7F {
            let ctrl = bytes[i + 1];
            if ctrl == 0x7F || i + 3 >= bytes.len() {
                break;
            }
            let (dx, dy) = (bytes[i + 2] as i8 as i32, bytes[i + 3] as i8 as i32);
            i += 4;
            match ctrl {
                0x01 => {
                    x += dx;
                    y += dy;
                    jump_run += 1;
                    if jump_run == 3 && !matches!(cmds.last(), Some(Cmd::Trim)) {
                        cmds.push(Cmd::Trim);
                    }
                    cmds.push(Cmd::Jump(at(x, y)));
                }
                0x03 => {
                    if !matches!(cmds.last(), Some(Cmd::Trim)) {
                        cmds.push(Cmd::Trim);
                    }
                }
                0x08 | 0x0A..=0x17 => {
                    if !matches!(cmds.last(), Some(Cmd::Trim)) {
                        cmds.push(Cmd::Trim);
                    }
                    cmds.push(Cmd::Color(stops));
                    stops += 1;
                    jump_run = 0;
                }
                _ => {}
            }
        } else {
            x += bytes[i] as i8 as i32;
            y += bytes[i + 1] as i8 as i32;
            i += 2;
            jump_run = 0;
            cmds.push(Cmd::Stitch(at(x, y)));
        }
    }
    cmds.push(Cmd::End);
    // colour table: end code, two bytes, then 00 rr gg bb per colour
    let mut stop_colors: Vec<Option<[u8; 3]>> = Vec::new();
    if table + 6 + 4 * n <= bytes.len() && bytes[table] == 0x7F {
        for k in 0..n {
            let o = table + 6 + 4 * k;
            stop_colors.push(Some([bytes[o + 1], bytes[o + 2], bytes[o + 3]]));
        }
    }
    stop_colors.resize(stops, None);
    Ok(MachineFile {
        cmds,
        stop_colors,
        label: String::new(),
        format: "XXX",
    })
}

// ------------------------------------------------------------------ VP3

const VP3_PRODUCER: &str = "Produced by     Software Ltd";

fn vp3_str16(out: &mut Vec<u8>, s: &str) {
    let units: Vec<u16> = s.encode_utf16().collect();
    out.extend_from_slice(&((units.len() * 2) as u16).to_be_bytes());
    for u in units {
        out.extend_from_slice(&u.to_be_bytes());
    }
}

fn vp3_str8(out: &mut Vec<u8>, s: &str) {
    let b: Vec<u8> = s.chars().filter(|c| c.is_ascii()).map(|c| c as u8).take(255).collect();
    out.extend_from_slice(&(b.len() as u16).to_be_bytes());
    out.extend(b);
}

fn be32(out: &mut Vec<u8>, v: i32) {
    out.extend_from_slice(&v.to_be_bytes());
}

/// Husqvarna Viking / Pfaff VP3: big-endian nested blocks, positions in
/// 1/1000 mm (y up), stitches as byte pairs in 0.1 mm (y down), one block
/// per colour. There is no jump code: a move is a long stitch, and after a
/// trim the machine travels to the next stitch.
pub fn write_vp3(d: &Design) -> Vec<u8> {
    let moves = machine_moves(d, false); // y down, 0.1 mm
    // ---- split into colour blocks: (start, stitch bytes, end, colour)
    struct Block {
        start: (i32, i32),
        end: (i32, i32),
        data: Vec<u8>,
        stop: usize,
    }
    let mut blocks: Vec<Block> = Vec::new();
    let mut count = 0i32;
    let (mut cx, mut cy) = (0i32, 0i32);
    let mut cur: Option<Block> = None;
    let mut pending_move = false;
    for (cmd, x, y) in &moves {
        match cmd {
            Cmd::Color(i) => {
                if let Some(b) = cur.take() {
                    blocks.push(b);
                }
                count += if *i > 0 { 1 } else { 0 };
                cur = Some(Block {
                    start: (cx, cy),
                    end: (cx, cy),
                    data: vec![0x0A, 0xF6, 0x00],
                    stop: *i,
                });
            }
            Cmd::Jump(_) => pending_move = true,
            Cmd::Stitch(_) => {
                let Some(b) = cur.as_mut() else { continue };
                if b.data.len() == 3 && !pending_move {
                    b.start = (cx, cy);
                }
                if b.data.len() == 3 && pending_move {
                    // a block starts where its first stitch is
                    b.start = (*x, *y);
                    cx = *x;
                    cy = *y;
                }
                pending_move = false;
                let (dx, dy) = (x - cx, y - cy);
                if (-127..=127).contains(&dx) && (-127..=127).contains(&dy) {
                    b.data.extend_from_slice(&[dx as i8 as u8, dy as i8 as u8]);
                } else {
                    b.data.extend_from_slice(&[0x80, 0x01]);
                    b.data.extend_from_slice(&(dx as i16).to_be_bytes());
                    b.data.extend_from_slice(&(dy as i16).to_be_bytes());
                    b.data.extend_from_slice(&[0x80, 0x02]);
                }
                count += 1;
                cx = *x;
                cy = *y;
                b.end = (cx, cy);
            }
            Cmd::Trim => {
                if let Some(b) = cur.as_mut() {
                    if b.data.len() > 3 && !b.data.ends_with(&[0x80, 0x03]) {
                        b.data.extend_from_slice(&[0x80, 0x03]);
                        count += 1;
                    }
                }
            }
            Cmd::End => {}
        }
    }
    if let Some(b) = cur.take() {
        blocks.push(b);
    }
    blocks.retain(|b| b.data.len() > 3);
    if let Some(b) = blocks.last_mut()
        && !b.data.ends_with(&[0x80, 0x03])
    {
        b.data.extend_from_slice(&[0x80, 0x03]);
    }
    let (minx, miny, maxx, maxy) = extents(&moves);
    // µm, y up
    let (right, top, left, bottom) = (maxx * 100, -miny * 100, minx * 100, -maxy * 100);
    let (ccx, ccy) = ((left + right) / 2, (top + bottom) / 2);
    let (w, h) = (right - left, top - bottom);

    // ---- innermost: design block
    let mut design = Vec::new();
    be32(&mut design, ccx);
    be32(&mut design, ccy);
    design.extend_from_slice(&[0, 0, 0]);
    for v in [-w / 2, w / 2, -h / 2, h / 2, w, h] {
        be32(&mut design, v);
    }
    design.extend_from_slice(&[
        0x00, 0x00, 0x64, 0x64, 0x00, 0x00, 0x10, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x10, 0x00, 0x78, 0x78, 0x50, 0x50, 0x01, 0x00,
    ]);
    vp3_str16(&mut design, VP3_PRODUCER);
    design.extend_from_slice(&(blocks.len() as u16).to_be_bytes());
    for b in &blocks {
        let mut blk = Vec::new();
        let (sx, sy) = (b.start.0 * 100, -b.start.1 * 100);
        be32(&mut blk, sx - ccx);
        be32(&mut blk, sy - ccy);
        let thread = d.stops.get(b.stop).map(|s| s.thread.clone());
        let rgb = thread.as_ref().map(|t| t.rgb).unwrap_or([0, 0, 0]);
        blk.extend_from_slice(&[1, 0, rgb[0], rgb[1], rgb[2], 0, 0, 0, 5, 0x28]);
        vp3_str8(&mut blk, thread.as_ref().map(|t| t.code.as_str()).unwrap_or(""));
        vp3_str8(&mut blk, thread.as_ref().map(|t| t.name.as_str()).unwrap_or(""));
        vp3_str8(&mut blk, thread.as_ref().map(|t| t.brand.as_str()).unwrap_or(""));
        be32(&mut blk, (b.end.0 - b.start.0) * 100);
        be32(&mut blk, -(b.end.1 - b.start.1) * 100);
        blk.extend_from_slice(&[0, 1, 0]);
        be32(&mut blk, b.data.len() as i32);
        blk.extend_from_slice(&b.data);
        blk.push(0);
        design.extend_from_slice(&[0, 5, 0]);
        be32(&mut design, blk.len() as i32);
        design.extend(blk);
    }

    // ---- middle: file block
    let mut file = Vec::new();
    vp3_str16(&mut file, ""); // settings
    for v in [right, top, left, bottom, count] {
        be32(&mut file, v);
    }
    file.extend_from_slice(&(blocks.len() as u16).to_be_bytes());
    file.extend_from_slice(&[0x0C, 0x00, 0x01, 0x00, 0x03, 0x00]);
    be32(&mut file, design.len() as i32);
    file.extend(design);

    let mut out = b"%vsm%\0".to_vec();
    vp3_str16(&mut out, VP3_PRODUCER);
    out.extend_from_slice(&[0, 2, 0]);
    be32(&mut out, file.len() as i32);
    out.extend(file);
    out
}

pub fn read_vp3(bytes: &[u8]) -> Result<MachineFile, String> {
    if bytes.len() < 0x60 || &bytes[0..5] != b"%vsm%" {
        return Err("not a VP3 file".into());
    }
    let bad = || "VP3 file is damaged or a kind StitchForge doesn't know".to_string();
    let u16be = |o: usize| -> Option<usize> { Some(u16::from_be_bytes([*bytes.get(o)?, *bytes.get(o + 1)?]) as usize) };
    let i32be = |o: usize| -> Option<i32> {
        Some(i32::from_be_bytes([*bytes.get(o)?, *bytes.get(o + 1)?, *bytes.get(o + 2)?, *bytes.get(o + 3)?]))
    };
    let mut p = 6;
    p += 2 + u16be(p).ok_or_else(bad)?; // producer
    p += 3 + 4; // file block tag and length
    p += 2 + u16be(p).ok_or_else(bad)?; // settings
    p += 16 + 4; // extents, count
    p += 2 + 6 + 4; // colours, fixed bytes, design block length
    let ccx = i32be(p).ok_or_else(bad)?;
    let ccy = i32be(p + 4).ok_or_else(bad)?;
    p += 8 + 3 + 24 + 26;
    p += 2 + u16be(p).ok_or_else(bad)?; // producer again
    let n = u16be(p).ok_or_else(bad)?;
    p += 2;
    let mut cmds = Vec::new();
    let mut stop_colors = Vec::new();
    let at = |x: i32, y: i32| Pt::new(x as f64 / 10.0, y as f64 / 10.0);
    for k in 0..n {
        if bytes.get(p..p + 3) != Some(&[0, 5, 0]) {
            if k == 0 {
                return Err(bad());
            }
            break;
        }
        let blen = i32be(p + 3).ok_or_else(bad)? as usize;
        let b0 = p + 7;
        let bend = (b0 + blen).min(bytes.len());
        // start, in µm from the design centre, y up → 0.1 mm, y down
        let sx = ((i32be(b0).ok_or_else(bad)? + ccx) as f64 / 100.0).round() as i32;
        let sy = (-(i32be(b0 + 4).ok_or_else(bad)? + ccy) as f64 / 100.0).round() as i32;
        let rgb = bytes.get(b0 + 10..b0 + 13).map(|c| [c[0], c[1], c[2]]);
        stop_colors.push(rgb);
        // the stitch data runs up to the block's last byte
        let mut data = None;
        for q in b0 + 8..bend.saturating_sub(8) {
            if bytes[q..q + 3] == [0, 1, 0]
                && let Some(len) = i32be(q + 3)
                && q + 7 + len as usize + 1 == bend
            {
                data = Some((q + 7, q + 7 + len as usize));
                break;
            }
        }
        let Some((d0, d1)) = data else { return Err(bad()) };
        cmds.push(Cmd::Color(k));
        let (mut x, mut y) = (sx, sy);
        cmds.push(Cmd::Jump(at(x, y)));
        let mut i = d0 + 3; // skip 0A F6 00
        let mut after_trim = false;
        while i + 1 < d1 {
            let b = bytes[i];
            if b == 0x80 {
                match bytes[i + 1] {
                    0x01 if i + 5 < d1 => {
                        let dx = i16::from_be_bytes([bytes[i + 2], bytes[i + 3]]) as i32;
                        let dy = i16::from_be_bytes([bytes[i + 4], bytes[i + 5]]) as i32;
                        i += 6;
                        x += dx;
                        y += dy;
                        // a long move: travel there, then the stitch lands
                        if after_trim || dx.abs().max(dy.abs()) > 127 {
                            cmds.push(Cmd::Jump(at(x, y)));
                        }
                        cmds.push(Cmd::Stitch(at(x, y)));
                        after_trim = false;
                    }
                    0x03 => {
                        i += 2;
                        if !matches!(cmds.last(), Some(Cmd::Trim)) {
                            cmds.push(Cmd::Trim);
                        }
                        after_trim = true;
                    }
                    _ => i += 2,
                }
                continue;
            }
            x += b as i8 as i32;
            y += bytes[i + 1] as i8 as i32;
            i += 2;
            if after_trim {
                // the machine travels to the first stitch after a trim
                cmds.push(Cmd::Jump(at(x, y)));
                after_trim = false;
            }
            cmds.push(Cmd::Stitch(at(x, y)));
        }
        if !matches!(cmds.last(), Some(Cmd::Trim)) {
            cmds.push(Cmd::Trim);
        }
        p = bend;
    }
    if cmds.is_empty() {
        return Err(bad());
    }
    cmds.push(Cmd::End);
    Ok(MachineFile {
        cmds,
        stop_colors,
        label: String::new(),
        format: "VP3",
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
        Some(fmt) => read_bytes(&bytes, fmt)?,
        None => {
            return Err(format!(
                "{}: not a stitch file StitchForge reads (DST, PES, PEC, EXP, JEF, VP3, XXX, U01, TAP)",
                path.display()
            ))
        }
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
        let back = read_bytes(&bytes, f).unwrap();
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
    #[test]
    fn pec_round_trip() {
        check_round_trip(Format::Pec);
    }
    #[test]
    fn jef_round_trip() {
        check_round_trip(Format::Jef);
    }
    #[test]
    fn vp3_round_trip() {
        check_round_trip(Format::Vp3);
    }
    #[test]
    fn xxx_round_trip() {
        check_round_trip(Format::Xxx);
    }
    #[test]
    fn u01_round_trip() {
        check_round_trip(Format::U01);
    }
    #[test]
    fn tap_round_trip() {
        check_round_trip(Format::Tap);
    }

    #[test]
    fn jef_header_adds_up() {
        let d = sample();
        let b = write_jef(&d);
        let u = |o: usize| u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]) as usize;
        let colors = u(24);
        assert_eq!(colors, d.stops.len());
        assert_eq!(u(0), 116 + 8 * colors, "stitch data offset");
        // the stitch block ends with the end code
        assert_eq!(&b[b.len() - 2..], &[0x80, 0x10]);
        // point count matches the 2-byte units after the header
        assert_eq!(u(28), (b.len() - u(0)) / 2);
    }
}
