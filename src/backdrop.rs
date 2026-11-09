//! The login screen's animated background: drifting colour light, a faint dot grid, and outline
//! check marks, notes and rings floating upward. Pure function of time and size, so a headless
//! `--at T` snapshot shows exactly the frame a window would at T.

use rune::{Color, El, LineCap, Path, Shape, canvas, el};
use rune_kit::Kit;

use crate::ui::{LOGO_FROM, LOGO_TO};

const TEAL: Color = Color { r: 0.20, g: 0.72, b: 0.84, a: 1.0 };
const GRID: f32 = 28.0;
const FLOATERS: usize = 16;

/// Full-window layers, pinned behind the content. `t` is seconds; `size` is the window in points.
pub fn backdrop(t: f32, [w, h]: [f32; 2], k: &Kit) -> El {
    let strength = if k.dark { 0.38 } else { 0.30 };
    let r = w.max(h) * 0.55;
    // Each blob wanders on its own slow Lissajous path (periods of 25–45 s), so the light never
    // visibly repeats.
    let blobs = [
        (LOGO_FROM, [0.25 + 0.18 * (t * 0.21).sin(), 0.30 + 0.20 * (t * 0.17 + 1.0).cos()]),
        (LOGO_TO, [0.75 + 0.16 * (t * 0.14 + 2.0).cos(), 0.70 + 0.18 * (t * 0.19).sin()]),
        (TEAL, [0.55 + 0.25 * (t * 0.11 + 4.0).sin(), 0.15 + 0.15 * (t * 0.23 + 3.0).sin()]),
    ];
    let mut layer = el().key("backdrop").pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0));
    for (i, (color, center)) in blobs.into_iter().enumerate() {
        let a = if i == 2 { strength * 0.6 } else { strength };
        layer = layer.child(
            el().key(format!("glow-{i}")).pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0)).radial_gradient(
                center,
                r,
                [(0.0, color.alpha(a)), (0.45, color.alpha(a * 0.35)), (1.0, color.alpha(0.0))],
            ),
        );
    }
    let ink = k.c.text_base;
    // Pinned like the glows, so the layers stack instead of flowing side by side.
    let full = |e: El| e.pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0));
    layer
        .child(full(canvas(dots(w, h, ink.alpha(if k.dark { 0.07 } else { 0.09 }))).key("dots")))
        .child(full(canvas(floaters(t, w, h, ink, k.dark)).key("floaters")))
}

fn dots(w: f32, h: f32, color: Color) -> Shape {
    let mut s = Shape::new();
    let mut y = GRID / 2.0;
    while y < h {
        let mut x = GRID / 2.0;
        while x < w {
            s = s.line([x, y], [x, y], 2.0, LineCap::Round, color);
            x += GRID;
        }
        y += GRID;
    }
    s
}

/// A cheap, stable pseudo-random number in 0..1 for floater `i`, attribute `salt`.
fn rand(i: usize, salt: u32) -> f32 {
    let mut x = (i as u32).wrapping_mul(0x9E37_79B9) ^ salt.wrapping_mul(0x85EB_CA6B);
    x ^= x >> 15;
    x = x.wrapping_mul(0x2C1B_3C6D);
    x ^= x >> 12;
    (x & 0xFFFF) as f32 / 65535.0
}

fn floaters(t: f32, w: f32, h: f32, ink: Color, dark: bool) -> Shape {
    let mut s = Shape::new();
    let travel = h + 120.0;
    for i in 0..FLOATERS {
        let size = 14.0 + rand(i, 1) * 18.0;
        let speed = 8.0 + rand(i, 2) * 10.0; // points per second: a slow rise
        let y = h + 60.0 - (rand(i, 3) * travel + t * speed) % travel;
        let x = rand(i, 4) * w + (t * 0.35 + i as f32).sin() * 14.0;
        let spin = (t * 0.2 + i as f32).sin() * 0.25;
        // Fade in from the bottom edge and out towards the top.
        let fade = smooth((h + 60.0 - y) / 160.0) * smooth(y / 200.0);
        let base = if dark { 0.22 } else { 0.20 };
        let color = match i % 3 {
            0 => LOGO_TO,
            1 => LOGO_FROM,
            _ => ink,
        }
        .alpha(base * fade);
        if fade <= 0.01 {
            continue;
        }
        s = glyph(s, i % 4, [x, y], size, spin, color);
    }
    s
}

fn smooth(x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}

/// One outline glyph centred at `c`, `size` across, rotated by `spin` radians.
fn glyph(s: Shape, kind: usize, c: [f32; 2], size: f32, spin: f32, color: Color) -> Shape {
    let (sin, cos) = spin.sin_cos();
    // Points are authored in a unit box (-0.5..0.5) and placed.
    let p = |x: f32, y: f32| [c[0] + (x * cos - y * sin) * size, c[1] + (x * sin + y * cos) * size];
    let stroke = (size * 0.09).max(1.5);
    match kind {
        // A check mark.
        0 => s.stroke_path(
            &Path::new().move_to(p(-0.3, 0.02)).line_to(p(-0.08, 0.24)).line_to(p(0.32, -0.22)),
            stroke,
            LineCap::Round,
            color,
        ),
        // A ticked box.
        1 => {
            let square = Path::new()
                .move_to(p(-0.45, -0.45))
                .line_to(p(0.45, -0.45))
                .line_to(p(0.45, 0.45))
                .line_to(p(-0.45, 0.45))
                .line_to(p(-0.45, -0.45));
            let tick = Path::new().move_to(p(-0.22, 0.0)).line_to(p(-0.05, 0.17)).line_to(p(0.24, -0.16));
            s.stroke_path(&square, stroke, LineCap::Round, color).stroke_path(&tick, stroke, LineCap::Round, color)
        }
        // A note: a page with three lines.
        2 => {
            let page = Path::new()
                .move_to(p(-0.36, -0.48))
                .line_to(p(0.36, -0.48))
                .line_to(p(0.36, 0.48))
                .line_to(p(-0.36, 0.48))
                .line_to(p(-0.36, -0.48));
            let mut s = s.stroke_path(&page, stroke, LineCap::Round, color);
            for (i, len) in [0.44, 0.44, 0.28].into_iter().enumerate() {
                let y = -0.22 + i as f32 * 0.2;
                s = s.line(p(-0.2, y), p(-0.2 + len, y), stroke * 0.8, LineCap::Round, color);
            }
            s
        }
        // A ring.
        _ => {
            let mut ring = Path::new().move_to(p(0.42, 0.0));
            for step in 1..=24 {
                let a = step as f32 / 24.0 * std::f32::consts::TAU;
                ring = ring.line_to(p(0.42 * a.cos(), 0.42 * a.sin()));
            }
            s.stroke_path(&ring, stroke, LineCap::Round, color)
        }
    }
}
