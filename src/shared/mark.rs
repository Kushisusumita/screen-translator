// The Sakura mark: five petals around a pale centre.
//
// This is the single source of the logo. The settings sidebar draws it with
// egui, the tray builds a Win32 icon from it, and `build.rs` rasterises it into
// the `.ico` embedded in the executable — all from the geometry below, so the
// icon in the taskbar can never drift from the mark inside the window.
//
// Deliberately free of dependencies, and commented with `//` rather than `//!`:
// `build.rs` pulls this file in with `include!`, where an inner doc comment is
// a syntax error and an import would have to become a build dependency.

/// A filled circle in a unit square, measured from the centre of the mark.
#[derive(Debug, Clone, Copy)]
pub struct Disc {
    /// Offset from the mark's centre, in units of the mark's radius.
    pub dx: f32,
    pub dy: f32,
    /// Radius, in units of the mark's radius.
    pub r: f32,
}

/// Petal radius and orbit, as fractions of the mark's radius. Chosen so the
/// five discs overlap into a flower rather than reading as separate dots.
const PETAL_RADIUS: f32 = 0.42;
const PETAL_ORBIT: f32 = 0.55;
const CORE_RADIUS: f32 = 0.28;

/// Petal colour — the brand pink.
pub const PETAL_RGB: [u8; 3] = [0xE8, 0x7C, 0x9E];
/// Core colour. Not pure white: a pink cast keeps it from reading as a hole
/// punched through the flower when the icon sits on a white background.
pub const CORE_RGB: [u8; 3] = [0xFF, 0xF2, 0xF6];

/// The five petals, starting at the top and going clockwise.
pub fn petals() -> [Disc; 5] {
    let mut out = [Disc {
        dx: 0.0,
        dy: 0.0,
        r: PETAL_RADIUS,
    }; 5];
    for (i, disc) in out.iter_mut().enumerate() {
        // -90° puts the first petal at twelve o'clock.
        let angle = -std::f32::consts::FRAC_PI_2 + std::f32::consts::TAU * (i as f32) / 5.0;
        disc.dx = angle.cos() * PETAL_ORBIT;
        disc.dy = angle.sin() * PETAL_ORBIT;
    }
    out
}

/// The petals turned by `turns` of a full revolution.
pub fn rotated_petals(turns: f32) -> [Disc; 5] {
    let mut out = petals();
    if turns == 0.0 {
        return out;
    }
    let a = turns * std::f32::consts::TAU;
    let (sin, cos) = a.sin_cos();
    for disc in out.iter_mut() {
        let (dx, dy) = (disc.dx, disc.dy);
        disc.dx = dx * cos - dy * sin;
        disc.dy = dx * sin + dy * cos;
    }
    out
}

/// A fifth of a turn: the flower maps onto itself, so an animation may stop on
/// any multiple of this and look exactly like rest.
pub const SYMMETRY_TURN: f32 = 0.2;

pub fn core() -> Disc {
    Disc {
        dx: 0.0,
        dy: 0.0,
        r: CORE_RADIUS,
    }
}

/// How much of the mark's bounding box the flower actually fills. Anything
/// outside this is margin.
pub fn extent() -> f32 {
    PETAL_ORBIT + PETAL_RADIUS
}

/// Renders the mark into `size × size` straight-alpha RGBA.
pub fn rasterise(size: u32) -> Vec<u8> {
    rasterise_with(size, 0.0, 1.0)
}

/// Renders the mark rotated by `turns` and scaled by `scale`.
///
/// Used for the "translating" animation: the flower turns while work is in
/// flight. Five-fold symmetry means a fifth of a turn is visually identical to
/// rest, which is what lets the spin settle without a jump.
///
/// Antialiased by supersampling: the shape is a union of overlapping discs, so
/// per-disc coverage cannot simply be added without darkening the seams.
pub fn rasterise_with(size: u32, turns: f32, scale: f32) -> Vec<u8> {
    const SUB: u32 = 4;

    let petals = rotated_petals(turns);
    let core = core();
    // Scale so the flower fills the canvas with a small margin, and keep the
    // proportions identical at every size.
    let radius = (size as f32) * 0.5 / extent() * 0.92 * scale.clamp(0.2, 1.0);
    let cx = (size as f32) * 0.5;
    let cy = (size as f32) * 0.5;

    let mut out = vec![0u8; (size as usize) * (size as usize) * 4];

    for py in 0..size {
        for px in 0..size {
            let mut petal_hits = 0u32;
            let mut core_hits = 0u32;

            for sy in 0..SUB {
                for sx in 0..SUB {
                    let x = px as f32 + (sx as f32 + 0.5) / SUB as f32;
                    let y = py as f32 + (sy as f32 + 0.5) / SUB as f32;

                    if inside(&core, x, y, cx, cy, radius) {
                        core_hits += 1;
                        petal_hits += 1;
                        continue;
                    }
                    if petals.iter().any(|d| inside(d, x, y, cx, cy, radius)) {
                        petal_hits += 1;
                    }
                }
            }

            let total = (SUB * SUB) as f32;
            let alpha = petal_hits as f32 / total;
            if alpha <= 0.0 {
                continue;
            }
            // Mix the core into the petal colour by how much of the pixel it
            // covers, so the boundary between them is antialiased too.
            let core_share = core_hits as f32 / petal_hits.max(1) as f32;
            let idx = ((py as usize) * (size as usize) + px as usize) * 4;
            for c in 0..3 {
                let petal = PETAL_RGB[c] as f32;
                let core_c = CORE_RGB[c] as f32;
                out[idx + c] = (petal + (core_c - petal) * core_share).round() as u8;
            }
            out[idx + 3] = (alpha * 255.0).round() as u8;
        }
    }

    out
}

fn inside(disc: &Disc, x: f32, y: f32, cx: f32, cy: f32, radius: f32) -> bool {
    let dx = x - (cx + disc.dx * radius);
    let dy = y - (cy + disc.dy * radius);
    dx * dx + dy * dy <= (disc.r * radius) * (disc.r * radius)
}

/// Sizes Windows asks for: the tray and title bar take the small ones, Explorer
/// the large. Supplying each one avoids the mushy downscale a single 256 px
/// image produces at 16 px.
///
/// Read by `build.rs`, which includes this file; the crate itself rasterises on
/// demand and never needs the list.
#[allow(dead_code)]
pub const ICON_SIZES: [u32; 9] = [16, 20, 24, 32, 40, 48, 64, 128, 256];

#[cfg(test)]
mod tests {
    use super::*;

    fn pixel(buf: &[u8], size: u32, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * size + x) * 4) as usize;
        [buf[i], buf[i + 1], buf[i + 2], buf[i + 3]]
    }

    #[test]
    fn the_centre_is_the_pale_core() {
        let size = 64;
        let buf = rasterise(size);
        let px = pixel(&buf, size, size / 2, size / 2);
        assert_eq!(px[3], 255, "the core must be opaque");
        assert_eq!([px[0], px[1], px[2]], CORE_RGB);
    }

    #[test]
    fn the_corners_are_transparent() {
        let size = 64;
        let buf = rasterise(size);
        for (x, y) in [(0, 0), (size - 1, 0), (0, size - 1), (size - 1, size - 1)] {
            assert_eq!(
                pixel(&buf, size, x, y)[3],
                0,
                "corner ({x},{y}) is not clear"
            );
        }
    }

    #[test]
    fn the_top_petal_is_brand_pink() {
        // Straight up from the centre, at the petal orbit.
        let size = 64;
        let buf = rasterise(size);
        let radius = (size as f32) * 0.5 / extent() * 0.92;
        let y = ((size as f32) * 0.5 - PETAL_ORBIT * radius).round() as u32;
        let px = pixel(&buf, size, size / 2, y);
        assert_eq!(px[3], 255);
        assert_eq!([px[0], px[1], px[2]], PETAL_RGB);
    }

    #[test]
    fn the_mark_is_left_right_symmetric() {
        // One petal points straight up, so the flower mirrors about the vertical
        // axis. Catches an orbit or phase mistake that still looks plausible.
        let size = 64;
        let buf = rasterise(size);
        for y in 0..size {
            for x in 0..size / 2 {
                let a = pixel(&buf, size, x, y);
                let b = pixel(&buf, size, size - 1 - x, y);
                assert_eq!(a, b, "asymmetry at ({x},{y})");
            }
        }
    }

    #[test]
    fn every_icon_size_renders_something() {
        for size in ICON_SIZES {
            let buf = rasterise(size);
            assert_eq!(buf.len(), (size * size * 4) as usize);
            let opaque = buf.chunks_exact(4).filter(|p| p[3] > 128).count();
            let total = (size * size) as usize;
            // A flower of five overlapping discs covers roughly half its box.
            assert!(
                opaque > total / 5,
                "{size}px is nearly empty: {opaque}/{total}"
            );
            assert!(opaque < total, "{size}px has no margin at all");
        }
    }

    #[test]
    fn a_fifth_of_a_turn_reproduces_the_mark_exactly() {
        // What makes the spin able to settle without a visible snap.
        let a = rasterise_with(48, 0.0, 1.0);
        let b = rasterise_with(48, SYMMETRY_TURN, 1.0);
        let differing = a
            .chunks_exact(4)
            .zip(b.chunks_exact(4))
            .filter(|(x, y)| x[3].abs_diff(y[3]) > 2)
            .count();
        assert!(
            differing < 16,
            "{differing} pixels differ across a symmetry step"
        );
    }

    #[test]
    fn a_quarter_turn_actually_moves_it() {
        // Guards against a rotation that silently does nothing.
        let a = rasterise_with(48, 0.0, 1.0);
        let b = rasterise_with(48, 0.1, 1.0);
        assert_ne!(a, b);
    }

    #[test]
    fn scaling_down_shrinks_the_covered_area() {
        let full = rasterise_with(48, 0.0, 1.0);
        let small = rasterise_with(48, 0.0, 0.7);
        let count = |b: &[u8]| b.chunks_exact(4).filter(|p| p[3] > 128).count();
        assert!(count(&small) < count(&full));
        assert!(count(&small) > 0);
    }

    #[test]
    fn the_flower_stays_inside_its_box() {
        assert!(
            extent() < 1.0,
            "the mark would be clipped by its own bounds"
        );
    }
}
