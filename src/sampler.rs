//! Gate 1 calibration sampler: a test design to sew on real fabric so the
//! stitch engine can be tuned from results instead of guesses.
//!
//! What to check after sewing (also printed on the operator sheet):
//! - Satin ladder: which widths sew clean, where satin starts to look loose.
//! - Angle squares: fill coverage and edges at 0/45/90/135 degrees.
//! - Circle + square: measure them. Both should be 25.0 mm each way;
//!   a short dimension shows pull, a long one push.
//! - Text: the smallest size that still reads clearly.
//! - Lines: run, triple run and a satin column on a curve.

pub const NOTES: &str = "CALIBRATION SAMPLER — sew on the target fabric with normal backing. \
Then check: (1) satin ladder 1–8 mm (1, 2, 3, 4, 5, 6, 8): note the narrowest that holds and the widest that stays tight; \
(2) the four angle squares: gaps or puckers at any angle; (3) measure the circle and square, both should be 25.0 mm wide and tall; \
(4) the smallest text size that reads cleanly; (5) the run, triple run and satin lines. Send photos and measurements back.";

pub fn svg() -> String {
    let mut s = String::from(
        "<svg xmlns='http://www.w3.org/2000/svg' width='120mm' height='140mm' viewBox='0 0 120 140'>",
    );
    let label = |x: f64, y: f64, t: &str, size: f64| {
        format!(
            "<text x='{x}' y='{y}' font-family='sans-serif' font-weight='bold' font-size='{size}' fill='#111111' text-anchor='middle'>{t}</text>"
        )
    };
    // Row 1: satin ladder (navy), forced satin, vertical columns 22 mm tall.
    let widths = [1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 8.0];
    let mut x = 6.0;
    for w in widths {
        s.push_str(&format!(
            "<rect id='sf-satin-ladder-{w}' x='{x:.2}' y='6' width='{w}' height='22' fill='#1b2d5c'/>"
        ));
        s.push_str(&label(x + w / 2.0, 35.0, &format!("{w}"), 5.0));
        x += w + 8.0;
    }
    // Row 2: fill squares at four forced angles (green).
    for (i, a) in [0, 45, 90, 135].iter().enumerate() {
        let x = 6.0 + i as f64 * 28.0;
        s.push_str(&format!(
            "<rect id='sf-fill-sf-angle-{a}-square' x='{x}' y='40' width='22' height='22' fill='#20843c'/>"
        ));
        s.push_str(&label(x + 11.0, 68.5, &format!("{a}°"), 5.0));
    }
    // Row 3: 25 mm circle and 25 mm square (red) for pull/push measurement,
    // the circle with a 1.5 mm satin outline (white).
    s.push_str("<circle id='measure-circle' cx='22.5' cy='86' r='12.5' fill='#c8102e' stroke='#f4f4f4' stroke-width='1.5'/>");
    s.push_str(&label(22.5, 105.5, "25 mm", 5.0));
    s.push_str(
        "<rect id='sf-fill-measure-square' x='45' y='73.5' width='25' height='25' fill='#c8102e'/>",
    );
    s.push_str(&label(57.5, 105.5, "25 mm", 5.0));
    // Row 3 right: lines — run, triple, satin column on a curve.
    s.push_str("<path id='sf-run-line' d='M80 76 C 90 70, 100 82, 114 76' fill='none' stroke='#111111' stroke-width='0.3'/>");
    s.push_str("<path id='sf-triple-line' d='M80 86 C 90 80, 100 92, 114 86' fill='none' stroke='#111111' stroke-width='0.3'/>");
    s.push_str("<path id='sf-satin-line' d='M80 96 C 90 90, 100 102, 114 96' fill='none' stroke='#111111' stroke-width='2'/>");
    s.push_str(&label(97.0, 105.5, "run·triple·satin", 5.0));
    // Row 4: text sizes (cap heights roughly 4, 5, 6, 8, 10 mm).
    let mut x = 8.0;
    for (h, fs) in [(4, 5.6), (5, 7.0), (6, 8.4), (8, 11.2), (10, 14.0)] {
        s.push_str(&format!(
            "<text x='{x}' y='128' font-family='sans-serif' font-weight='bold' font-size='{fs}' fill='#111111'>Aa{h}</text>"
        ));
        x += fs * 1.75 + 2.0;
    }
    s.push_str("</svg>");
    s
}

#[cfg(test)]
mod tests {
    #[test]
    fn sampler_builds_with_forced_settings() {
        let (opt, _) = crate::import::font_options(None).unwrap();
        let art = crate::import::load_svg_str(&super::svg(), &opt).unwrap();
        let d = crate::design::build(
            "sampler",
            &art,
            &crate::thread::generic_chart(),
            &crate::settings::recipe("pique").unwrap(),
            crate::settings::Profile::Flat,
        )
        .unwrap();
        // the 8 mm ladder rung is forced to satin, not fill
        let rung = d
            .objects
            .iter()
            .find(|o| o.label.contains("ladder-8"))
            .unwrap();
        assert_eq!(rung.stitch_type, "Satin");
        // the 135° square is filled at 135°
        let sq = d
            .objects
            .iter()
            .find(|o| o.label.contains("angle-135"))
            .unwrap();
        assert_eq!(sq.stitch_type, "Tatami fill");
        assert!((sq.angle_deg - 135.0).abs() < 0.5, "angle {}", sq.angle_deg);
    }
}
