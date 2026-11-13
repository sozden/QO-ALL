//! Look of the QO-ALL window: colours, theme and small widgets.

use eframe::egui::{self, Color32, CornerRadius, FontId, Pos2, Rect, RichText, Sense, Stroke, Vec2, pos2, vec2};

/// Two palettes: night (deep navy, amber receiving, red transmitting) and paper (warm
/// cream, browns, the same roles in deeper tones). The one in use is chosen at run time.
#[allow(non_snake_case)]
pub mod palette {
    use std::sync::atomic::{AtomicBool, Ordering};

    use eframe::egui::Color32;

    static LIGHT: AtomicBool = AtomicBool::new(false);

    pub fn set_light(on: bool) {
        LIGHT.store(on, Ordering::Relaxed);
    }

    pub fn light() -> bool {
        LIGHT.load(Ordering::Relaxed)
    }

    fn pick(dark: [u8; 3], day: [u8; 3]) -> Color32 {
        let c = if light() { day } else { dark };
        Color32::from_rgb(c[0], c[1], c[2])
    }

    pub fn BG() -> Color32 { pick([0x0d, 0x14, 0x20], [0xf3, 0xef, 0xe6]) }
    pub fn CARD() -> Color32 { pick([0x12, 0x1c, 0x2c], [0xfb, 0xf8, 0xf1]) }
    pub fn DEEP() -> Color32 { pick([0x0a, 0x10, 0x19], [0xff, 0xfd, 0xf8]) }
    pub fn RAISED() -> Color32 { pick([0x16, 0x23, 0x3a], [0xe8, 0xe1, 0xd2]) }
    pub fn BORDER() -> Color32 { pick([0x24, 0x32, 0x4a], [0xd3, 0xc8, 0xb2]) }
    pub fn GRID() -> Color32 { pick([0x1b, 0x26, 0x38], [0xe9, 0xe2, 0xd4]) }
    pub fn TEXT() -> Color32 { pick([0xc9, 0xd4, 0xe3], [0x2b, 0x24, 0x1a]) }
    pub fn MUTED() -> Color32 { pick([0x8f, 0xa3, 0xbf], [0x6b, 0x5e, 0x49]) }
    pub fn DIM() -> Color32 { pick([0x5f, 0x73, 0x91], [0x9c, 0x8f, 0x78]) }
    /// Receiving: blue. Transmitting (and cautions): amber. Red is kept for being on
    /// the air, errors and what is forbidden.
    pub fn RX() -> Color32 { pick([0x5c, 0xc8, 0xff], [0x1f, 0x5f, 0xa8]) }
    pub fn TX() -> Color32 { pick([0xf2, 0xb3, 0x3d], [0x9a, 0x5b, 0x00]) }
    pub fn ALERT() -> Color32 { pick([0xff, 0x7a, 0x7a], [0xb0, 0x30, 0x2a]) }
    pub fn GOOD() -> Color32 { pick([0x6f, 0xd3, 0x9a], [0x2e, 0x6b, 0x3c]) }
    pub fn BLUE() -> Color32 { pick([0x8f, 0xb3, 0xe8], [0x36, 0x5f, 0x8c]) }
    /// Strongest text: white at night, near black by day.
    pub fn STRONG() -> Color32 { pick([0xff, 0xff, 0xff], [0x1a, 0x14, 0x0c]) }
    /// Backgrounds of being on the air (and errors) and of good news.
    pub fn ALERT_BG() -> Color32 { pick([0x3a, 0x14, 0x18], [0xf6, 0xdc, 0xd5]) }
    pub fn GOOD_BG() -> Color32 { pick([0x12, 0x30, 0x1f], [0xdf, 0xec, 0xcf]) }
}
use palette::*;

pub fn apply_theme(ctx: &egui::Context) {
    ctx.all_styles_mut(|style| {
        let mut v = if light() { egui::Visuals::light() } else { egui::Visuals::dark() };
        v.panel_fill = BG();
        v.window_fill = CARD();
        // Text boxes a shade apart from the cards they sit on.
        v.extreme_bg_color = RAISED();
        v.faint_bg_color = CARD();
        v.window_corner_radius = CornerRadius::same(10);
        v.window_stroke = Stroke::new(1.0, BORDER());
        v.selection.bg_fill = RX().gamma_multiply(0.35);
        v.selection.stroke = Stroke::new(1.0, RX());
        v.hyperlink_color = BLUE();
        let radius = CornerRadius::same(6);
        v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, BORDER());
        v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, TEXT());
        v.widgets.noninteractive.corner_radius = radius;
        // Buttons, value boxes and check boxes: navy at night, sand on paper.
        let (rest, hover, press) = if light() {
            (Color32::from_rgb(0xe8, 0xe1, 0xd2), Color32::from_rgb(0xdd, 0xd3, 0xbf), Color32::from_rgb(0xd0, 0xc4, 0xab))
        } else {
            (Color32::from_rgb(0x1a, 0x26, 0x3a), Color32::from_rgb(0x22, 0x31, 0x4a), Color32::from_rgb(0x2a, 0x3c, 0x5a))
        };
        for (w, fill) in [(&mut v.widgets.inactive, rest), (&mut v.widgets.hovered, hover), (&mut v.widgets.active, press), (&mut v.widgets.open, hover)] {
            w.bg_fill = fill;
            w.weak_bg_fill = fill;
            w.corner_radius = radius;
        }
        v.widgets.inactive.fg_stroke = Stroke::new(1.0, TEXT());
        v.widgets.hovered.bg_stroke = Stroke::new(1.0, RX().gamma_multiply(0.6));
        v.widgets.active.bg_stroke = Stroke::new(1.0, RX());
        style.visuals = v;
        style.spacing.item_spacing = vec2(8.0, 6.0);
        style.spacing.button_padding = vec2(10.0, 4.0);
        style.spacing.interact_size.y = 24.0;
        style.spacing.slider_width = 140.0;
    });
}

/// A rounded panel.
pub fn card<R>(ui: &mut egui::Ui, fill: Color32, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    egui::Frame::new()
        .fill(fill)
        .stroke(Stroke::new(1.0, BORDER()))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(egui::Margin::symmetric(12, 8))
        .show(ui, add)
        .inner
}

/// A card with a line of its role's colour along the top (receive, transmit).
pub fn accent_card<R>(ui: &mut egui::Ui, fill: Color32, accent: Color32, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let shown = egui::Frame::new()
        .fill(fill)
        .stroke(Stroke::new(1.0, BORDER()))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(egui::Margin::symmetric(12, 8))
        .show(ui, add);
    let r = shown.response.rect;
    ui.painter().line_segment([pos2(r.left() + 7.0, r.top() + 1.0), pos2(r.right() - 7.0, r.top() + 1.0)], Stroke::new(2.0, accent));
    shown.inner
}

/// One choice out of a few, as joined segments in one frame: the chosen one filled.
/// Returns the segment clicked.
pub fn segmented(ui: &mut egui::Ui, items: &[(&str, bool)], color: Color32) -> Option<usize> {
    let mut clicked = None;
    egui::Frame::new().fill(RAISED()).corner_radius(CornerRadius::same(8)).inner_margin(egui::Margin::same(2)).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            ui.spacing_mut().button_padding = vec2(12.0, 3.0);
            for (i, (text, on)) in items.iter().enumerate() {
                // On the filled segment: dark text at night, light on paper.
                let (fg, bg) = if *on { (if light() { DEEP() } else { BG() }, color) } else { (MUTED(), Color32::TRANSPARENT) };
                let button = egui::Button::new(RichText::new(*text).size(12.5).strong().color(fg)).fill(bg).corner_radius(6).frame_when_inactive(*on);
                if ui.add(button).clicked() {
                    clicked = Some(i);
                }
            }
        });
    });
    clicked
}

/// A small caption above a value.
pub fn caption(ui: &mut egui::Ui, text: impl Into<String>, color: Color32) {
    ui.label(RichText::new(text).size(11.0).color(color));
}

/// A rounded tag.
pub fn pill(ui: &mut egui::Ui, text: impl Into<String>, fg: Color32, bg: Color32) -> egui::Response {
    egui::Frame::new()
        .fill(bg)
        .corner_radius(CornerRadius::same(10))
        .inner_margin(egui::Margin::symmetric(9, 2))
        .show(ui, |ui| ui.label(RichText::new(text).size(12.0).color(fg)))
        .response
}

/// A toggle shown as a pill; returns true when clicked.
pub fn choice(ui: &mut egui::Ui, text: &str, on: bool, color: Color32) -> bool {
    choice_response(ui, text, on, color).clicked()
}

/// `choice`, with the whole response (for a hover text).
pub fn choice_response(ui: &mut egui::Ui, text: &str, on: bool, color: Color32) -> egui::Response {
    let (fg, bg) = if on { (color, color.gamma_multiply(0.22)) } else { (MUTED(), RAISED()) };
    let button = egui::Button::new(RichText::new(text).size(12.0).color(fg)).fill(bg).corner_radius(10);
    ui.add(button)
}

/// Display characters of a frequency at 10 Hz resolution, "10 489.725 00", with the
/// place value (Hz) of each digit, None for separators.
fn vfo_digits(hz: f64) -> Vec<(char, Option<f64>)> {
    let tens = (hz.max(0.0) / 10.0).round() as u64;
    let digits = format!("{tens:09}");
    let count = digits.len();
    let mut out = Vec::new();
    for (k, c) in digits.chars().enumerate() {
        // Place of this digit in Hz: the last one is 10 Hz.
        let place = (count - k) as i32;
        out.push((c, Some(10f64.powi(place))));
        match place {
            9 => out.push((' ', None)),
            6 => out.push(('.', None)),
            3 => out.push((' ', None)),
            _ => {}
        }
    }
    out
}

/// Big frequency display tunable digit by digit: the mouse wheel over a digit changes it
/// by that digit's place; clicking its upper half increases it, the lower half decreases
/// it. Returns the change (Hz).
pub fn vfo(ui: &mut egui::Ui, hz: f64, size: f32, color: Color32) -> f64 {
    let font = FontId::monospace(size);
    let mut delta = 0.0;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        for (c, step) in vfo_digits(hz) {
            let galley = ui.painter().layout_no_wrap(c.to_string(), font.clone(), color);
            let sense = if step.is_some() { Sense::click() } else { Sense::hover() };
            let (rect, response) = ui.allocate_exact_size(galley.size(), sense);
            if let (Some(step), true) = (step, response.hovered()) {
                ui.painter().rect_filled(rect.expand(1.0), CornerRadius::same(3), color.gamma_multiply(0.18));
                let steps = wheel_steps(ui).0;
                delta += steps as f64 * step;
                if let Some(pos) = response.clicked().then(|| response.interact_pointer_pos()).flatten() {
                    delta += if pos.y < rect.center().y { step } else { -step };
                }
            }
            ui.painter().galley(rect.min, galley, color);
        }
    });
    delta
}

/// Mouse wheel steps in this frame (up is positive) and whether Shift is held. The
/// scroll used here is consumed so it does not reach surrounding scroll areas.
pub fn wheel_steps(ui: &egui::Ui) -> (i32, bool) {
    let (steps, shift) = ui.input(|i| {
        let steps: i32 = i
            .events
            .iter()
            .filter_map(|e| match e {
                egui::Event::MouseWheel { delta, .. } => {
                    let d = delta.x + delta.y;
                    (d != 0.0).then_some(d.signum() as i32)
                }
                _ => None,
            })
            .sum();
        (steps, i.modifiers.shift)
    });
    if steps != 0 {
        ui.ctx().input_mut(|i| i.smooth_scroll_delta = Vec2::ZERO);
    }
    (steps, shift)
}

/// Horizontal level bar for `value` in `range`, with an optional marker (the VOX
/// threshold).
pub fn level_bar(ui: &mut egui::Ui, width: f32, value: f32, range: (f32, f32), color: Color32, marker: Option<f32>) {
    let (rect, _) = ui.allocate_exact_size(vec2(width, 8.0), Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, CornerRadius::same(4), RAISED());
    let t = |v: f32| ((v - range.0) / (range.1 - range.0)).clamp(0.0, 1.0);
    let fill = Rect::from_min_size(rect.min, vec2(rect.width() * t(value), rect.height()));
    p.rect_filled(fill, CornerRadius::same(4), color);
    if let Some(m) = marker {
        let x = rect.left() + rect.width() * t(m);
        p.line_segment([pos2(x, rect.top() - 3.0), pos2(x, rect.bottom() + 3.0)], Stroke::new(2.0, TX()));
    }
}

fn rgb(hex: u32) -> Color32 {
    Color32::from_rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let (a, b) = (a.to_array(), b.to_array());
    let c: Vec<u8> = (0..4).map(|i| (a[i] as f32 + (b[i] as f32 - a[i] as f32) * t).round() as u8).collect();
    Color32::from_rgba_premultiplied(c[0], c[1], c[2], c[3])
}

/// Colour at `t` (0..1) along gradient stops.
fn gradient(stops: &[(f32, Color32)], t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    for w in stops.windows(2) {
        if t <= w[1].0 {
            return mix(w[0].1, w[1].1, (t - w[0].0) / (w[1].0 - w[0].0).max(1e-6));
        }
    }
    stops.last().map_or(Color32::TRANSPARENT, |s| s.1)
}

/// Fills `rect` with a grid mesh coloured point by point: gradients egui has no shapes for.
fn shade(p: &egui::Painter, rect: Rect, nx: u32, ny: u32, color: impl Fn(Pos2) -> Color32) {
    let mut m = egui::Mesh::default();
    for j in 0..=ny {
        for i in 0..=nx {
            let pos = pos2(rect.left() + rect.width() * i as f32 / nx as f32, rect.top() + rect.height() * j as f32 / ny as f32);
            m.colored_vertex(pos, color(pos));
        }
    }
    let w = nx + 1;
    for j in 0..ny {
        for i in 0..nx {
            let a = j * w + i;
            m.add_triangle(a, a + 1, a + w);
            m.add_triangle(a + 1, a + w + 1, a + w);
        }
    }
    p.add(egui::Shape::mesh(m));
}

/// Where `db` (above the noise floor) sits on the S-meter scale (0..1): S1..S9 in 6 dB
/// steps over the first 60 %, then +10..+60 dB.
fn s_fraction(db: f32) -> f32 {
    if db <= 54.0 { db.max(0.0) / 54.0 * 0.6 } else { (0.6 + (db - 54.0) / 60.0 * 0.4).min(1.0) }
}

/// "S7", "S9+12".
pub fn s_units(db: f32) -> String {
    if db < 54.0 { format!("S{}", (db.max(0.0) / 6.0).floor() as i32) } else { format!("S9+{:.0}", db - 54.0) }
}

/// A classic S-meter behind glass in a chrome bezel: "SIGNAL LEVEL" in S units over an
/// S/N scale in dB, one needle for both. `db` is the signal over the noise floor, so S0
/// is the noise and S9 is 54 dB above it. Backlit warm on the paper theme, a glowing
/// night face on the night theme.
pub fn s_meter(ui: &mut egui::Ui, width: f32, db: f32) {
    let k = width / 300.0;
    let (rect, _) = ui.allocate_exact_size(vec2(width, 180.0 * k), Sense::hover());
    let p = ui.painter_at(rect);
    let at = |x: f32, y: f32| rect.min + vec2(x, y) * k;
    // Points on the scale: radius and fraction, around a pivot below the face.
    let on = |r: f32, f: f32| {
        let a = (228.0 + 84.0 * f).to_radians();
        at(150.0 + r * a.cos(), 230.0 + r * a.sin())
    };
    let warm = palette::light();
    let (face, ink, red, needle, glare, glow) = if warm {
        ([rgb(0xffe9b0), rgb(0xffb84a), rgb(0xe07a12)], rgb(0x2a1a08), rgb(0xb3121a), rgb(0xc8101a), 0.40, false)
    } else {
        ([rgb(0x3a4a66), rgb(0x162238), rgb(0x05080f)], rgb(0xf6c35a), rgb(0xff5a4a), rgb(0xff6a5a), 0.16, true)
    };
    let line = |a: Pos2, b: Pos2, w: f32, c: Color32| {
        if glow {
            p.line_segment([a, b], Stroke::new((w + 3.0) * k, c.gamma_multiply(0.22)));
        }
        p.line_segment([a, b], Stroke::new(w * k, c));
    };
    let arc = |r: f32, from: f32, to: f32, w: f32, c: Color32| {
        let pts: Vec<Pos2> = (0..=48).map(|i| on(r, from + (to - from) * i as f32 / 48.0)).collect();
        if glow {
            p.add(egui::Shape::line(pts.clone(), Stroke::new((w + 3.0) * k, c.gamma_multiply(0.22))));
        }
        p.add(egui::Shape::line(pts, Stroke::new(w * k, c)));
    };
    let text = |pos: Pos2, s: &str, size: f32, c: Color32| {
        p.text(pos, egui::Align2::CENTER_CENTER, s, FontId::proportional((size * k).max(9.0)), c);
    };

    // The lit face, without a frame: it sits on the card like the rest of the window.
    let face_rect = Rect::from_min_max(at(4.0, 4.0), at(296.0, 176.0));
    let centre = at(150.0, 162.0);
    let (rx, ry) = (294.0 * k, 168.0 * k);
    let stops = [(0.0, face[0]), (0.5, face[1]), (1.0, face[2])];
    shade(&p, face_rect, 28, 14, |q| gradient(&stops, (((q.x - centre.x) / rx).powi(2) + ((q.y - centre.y) / ry).powi(2)).sqrt()));

    // S scale: S1..S9, then +10..+60 in red on a thick band.
    arc(160.0, 0.0, 0.6, 1.6, ink);
    arc(160.0, 0.6, 1.0, 1.6, red);
    arc(163.0, 0.6, 1.0, 5.0, red);
    for s in 1..=9 {
        let f = s_fraction(6.0 * s as f32);
        let major = s % 2 == 1;
        line(on(160.0, f), on(if major { 172.0 } else { 167.0 }, f), if major { 2.0 } else { 1.2 }, ink);
        if major {
            text(on(186.0, f), &s.to_string(), 15.0, ink);
        }
    }
    for d in 1..=6 {
        let f = s_fraction(54.0 + 10.0 * d as f32);
        let major = d % 2 == 0;
        line(on(160.0, f), on(if major { 172.0 } else { 167.0 }, f), if major { 2.0 } else { 1.2 }, red);
        if major {
            text(on(186.0, f), &format!("+{}", 10 * d), 13.0, red);
        }
    }
    // The "S" badge in the corner.
    let badge = at(28.0, 28.0);
    p.rect_stroke(Rect::from_center_size(badge, vec2(15.0, 15.0) * k), CornerRadius::same((3.0 * k) as u8), Stroke::new(1.3 * k, ink), egui::StrokeKind::Middle);
    text(badge, "S", 11.0, ink);
    // S/N scale in dB under it, on the same needle.
    arc(126.0, 0.0, 1.0, 1.3, ink);
    for d in (0..=110).step_by(10) {
        let f = s_fraction(d as f32);
        let major = d % 20 == 0;
        line(on(126.0, f), on(if major { 117.0 } else { 121.0 }, f), if major { 1.6 } else { 1.0 }, ink);
        if major && d <= 100 {
            text(on(106.0, f), &d.to_string(), 10.0, ink);
        }
    }
    text(at(150.0, 88.0), "SIGNAL LEVEL", 12.0, ink);
    text(at(150.0, 146.0), "S/N dB", 10.0, ink);
    p.text(at(280.0, 158.0), egui::Align2::RIGHT_CENTER, s_units(db), FontId::monospace(13.0 * k), ink.gamma_multiply(0.85));

    // Needle with its shadow, the pivot cover and the glass, inside the face.
    let fp = p.with_clip_rect(face_rect);
    let tip = on(168.0, s_fraction(db));
    fp.line_segment([at(153.0, 234.0), tip + vec2(3.0, 4.0) * k], Stroke::new(2.5 * k, Color32::from_black_alpha(56)));
    if glow {
        fp.line_segment([at(150.0, 230.0), tip], Stroke::new(5.0 * k, needle.gamma_multiply(0.25)));
    }
    fp.line_segment([at(150.0, 230.0), tip], Stroke::new(1.8 * k, needle));
    let cap = at(150.0, 194.0);
    for i in 0..10 {
        let t = i as f32 / 9.0;
        let r = 24.0 * k * (1.0 - 0.85 * t);
        fp.circle_filled(cap + vec2(-4.0, -6.0) * k * t, r, mix(rgb(0x22262c), rgb(0xb9c0c8), t));
    }
    let (gc, grx, gry) = (at(150.0, -35.0), 240.0 * k, 100.0 * k);
    shade(&fp, face_rect, 28, 16, |q| {
        let inside = ((q.x - gc.x) / grx).powi(2) + ((q.y - gc.y) / gry).powi(2) < 1.0;
        let fade = 1.0 - (q.y - (gc.y - gry)) / (2.0 * gry);
        if inside { Color32::from_white_alpha((glare * fade.clamp(0.0, 1.0) * 255.0) as u8) } else { Color32::TRANSPARENT }
    });
    // A soft inner shadow and the window's own thin border; the face's square corners
    // are rounded off in the card's colour.
    let radius = 8.0 * k;
    let shadow = if warm { 30 } else { 70 };
    p.rect_stroke(face_rect, CornerRadius::same(radius as u8), Stroke::new(2.0 * k, Color32::from_black_alpha(shadow)), egui::StrokeKind::Inside);
    let w = radius;
    p.rect_stroke(face_rect.expand(w), CornerRadius::same((radius + w) as u8), Stroke::new(w, DEEP()), egui::StrokeKind::Inside);
    p.rect_stroke(face_rect, CornerRadius::same(radius as u8), Stroke::new(1.0, BORDER()), egui::StrokeKind::Outside);
}

/// The chain between the two frequency displays: closed when linked.
pub fn link_button(ui: &mut egui::Ui, linked: bool, tip: &str) -> bool {
    let (rect, response) = ui.allocate_exact_size(vec2(40.0, 40.0), Sense::click());
    let color = if response.hovered() { TEXT() } else if linked { MUTED() } else { DIM() };
    let p = ui.painter();
    let c = rect.center();
    let gap = if linked { 5.0 } else { 9.0 };
    for dx in [-gap, gap] {
        p.rect_stroke(
            Rect::from_center_size(c + vec2(dx, 0.0), vec2(16.0, 9.0)),
            CornerRadius::same(4),
            Stroke::new(2.0, color),
            egui::StrokeKind::Middle,
        );
    }
    if !linked {
        p.line_segment([c + vec2(-3.0, -9.0), c + vec2(3.0, 9.0)], Stroke::new(1.5, TX()));
    }
    response.on_hover_text(tip).clicked()
}

/// The round PTT button; returns true when clicked.
pub fn ptt_button(ui: &mut egui::Ui, on: bool, enabled: bool) -> bool {
    let (rect, response) = ui.allocate_exact_size(vec2(72.0, 72.0), Sense::click());
    let p = ui.painter();
    let c = rect.center();
    let color = if enabled { ALERT() } else { DIM() };
    if on {
        p.circle_filled(c, 34.0, ALERT().gamma_multiply(0.85));
    } else if response.hovered() && enabled {
        p.circle_filled(c, 34.0, ALERT().gamma_multiply(0.15));
    }
    p.circle_stroke(c, 34.0, Stroke::new(2.0, color));
    p.text(c, egui::Align2::CENTER_CENTER, "PTT", FontId::proportional(17.0), if on { BG() } else { color });
    response.clicked()
}

/// Waterfall colour of a value in 0..1: at night navy, blue, cyan, amber, white; on paper
/// cream, sand, ochre, brown, near black.
pub fn colormap(t: f32) -> Color32 {
    const NIGHT: [[f32; 3]; 5] = [[8.0, 12.0, 24.0], [18.0, 40.0, 90.0], [30.0, 110.0, 170.0], [240.0, 200.0, 80.0], [255.0, 250.0, 230.0]];
    const DAY: [[f32; 3]; 5] = [[255.0, 252.0, 244.0], [236.0, 214.0, 168.0], [200.0, 140.0, 60.0], [120.0, 60.0, 20.0], [40.0, 20.0, 8.0]];
    let stops = if light() { &DAY } else { &NIGHT };
    let t = t.clamp(0.0, 1.0) * (stops.len() - 1) as f32;
    let i = (t as usize).min(stops.len() - 2);
    let f = t - i as f32;
    let c = |k: usize| (stops[i][k] + (stops[i + 1][k] - stops[i][k]) * f) as u8;
    Color32::from_rgb(c(0), c(1), c(2))
}

/// A 1-2-5 step of at least `min`.
pub fn nice_step(min: f64) -> f64 {
    let mag = 10f64.powf(min.max(1e-9).log10().floor());
    [1.0, 2.0, 5.0, 10.0].iter().map(|m| m * mag).find(|&s| s >= min).unwrap_or(10.0 * mag)
}

/// A dashed vertical line.
pub fn dashed_vline(painter: &egui::Painter, x: f32, top: f32, bottom: f32, stroke: Stroke) {
    let points = [Pos2::new(x, top), Pos2::new(x, bottom)];
    painter.extend(egui::Shape::dashed_line(&points, stroke, 5.0, 4.0));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vfo_groups() {
        let text: String = vfo_digits(10_489_725_000.0).iter().map(|d| d.0).collect();
        assert_eq!(text, "10 489.725 00");
        let text: String = vfo_digits(2_400_225_000.0).iter().map(|d| d.0).collect();
        assert_eq!(text, "2 400.225 00");
        let places: Vec<f64> = vfo_digits(2_400_225_000.0).iter().filter_map(|d| d.1).collect();
        assert_eq!(places.first(), Some(&1e9));
        assert_eq!(places.last(), Some(&10.0));
    }

    #[test]
    fn steps() {
        assert_eq!(nice_step(37_000.0), 50_000.0);
        assert_eq!(nice_step(1_200.0), 2_000.0);
    }
}
