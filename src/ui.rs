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
    pub fn RX() -> Color32 { pick([0xf2, 0xb3, 0x3d], [0x9a, 0x5b, 0x00]) }
    pub fn TX() -> Color32 { pick([0xff, 0x7a, 0x7a], [0xb0, 0x30, 0x2a]) }
    pub fn GOOD() -> Color32 { pick([0x6f, 0xd3, 0x9a], [0x2e, 0x6b, 0x3c]) }
    pub fn BLUE() -> Color32 { pick([0x8f, 0xb3, 0xe8], [0x36, 0x5f, 0x8c]) }
    /// Strongest text: white at night, near black by day.
    pub fn STRONG() -> Color32 { pick([0xff, 0xff, 0xff], [0x1a, 0x14, 0x0c]) }
    /// Backgrounds of transmitting and of good news.
    pub fn TX_BG() -> Color32 { pick([0x3a, 0x14, 0x18], [0xf6, 0xdc, 0xd5]) }
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
    let color = if enabled { TX() } else { DIM() };
    if on {
        p.circle_filled(c, 34.0, TX().gamma_multiply(0.85));
    } else if response.hovered() && enabled {
        p.circle_filled(c, 34.0, TX().gamma_multiply(0.15));
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
