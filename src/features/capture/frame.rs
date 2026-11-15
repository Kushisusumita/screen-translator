//! One capture of the screen, shared by everything that needs its pixels.
//!
//! The overlay shows it frozen, the recogniser reads a crop of it, and the
//! in-place view paints the translation back over that same crop. All three
//! used to take their own screenshot, which cost three grabs and — worse —
//! meant the text that was recognised was not necessarily the text the user had
//! selected: by the second grab the overlay itself was on screen, and a game had
//! moved on to the next line.
//!
//! So the desktop is grabbed once, and everyone works from that.

use std::sync::Arc;

use super::screenshot::Bounds;

/// Pixels of a rectangle of the desktop.
#[derive(Clone)]
pub struct Frame {
    /// Where these pixels sit on the desktop, in physical pixels.
    pub bounds: Bounds,
    /// RGBA, row-major, exactly `bounds.w × bounds.h × 4` bytes. Behind an
    /// `Arc` because a 4K desktop is 33 MB and is handed to several owners.
    pixels: Arc<Vec<u8>>,
}

impl std::fmt::Debug for Frame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Frame").field("bounds", &self.bounds).finish()
    }
}

/// A plain colour, without egui in the signature so this stays testable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgb(pub u8, pub u8, pub u8);

impl Rgb {
    /// Perceived brightness, 0–255.
    pub fn luma(self) -> f32 {
        0.299 * self.0 as f32 + 0.587 * self.1 as f32 + 0.114 * self.2 as f32
    }

    fn distance(self, other: Rgb) -> f32 {
        let dr = self.0 as f32 - other.0 as f32;
        let dg = self.1 as f32 - other.1 as f32;
        let db = self.2 as f32 - other.2 as f32;
        // Weighted the way the eye is: a change in green reads as more of a
        // change than the same step in blue.
        (0.30 * dr * dr + 0.59 * dg * dg + 0.11 * db * db).sqrt()
    }

    /// WCAG contrast ratio against another colour, 1–21.
    pub fn contrast(self, other: Rgb) -> f32 {
        fn channel(v: u8) -> f32 {
            let v = v as f32 / 255.0;
            if v <= 0.03928 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        }
        fn luminance(c: Rgb) -> f32 {
            0.2126 * channel(c.0) + 0.7152 * channel(c.1) + 0.0722 * channel(c.2)
        }
        let (a, b) = (luminance(self), luminance(other));
        let (hi, lo) = if a > b { (a, b) } else { (b, a) };
        (hi + 0.05) / (lo + 0.05)
    }
}

/// The colours a piece of text is made of.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextColours {
    /// What the text is written on, taken as one colour.
    pub background: Rgb,
    /// The same surface at its four corners — top-left, top-right,
    /// bottom-left, bottom-right. All equal on a flat panel; on a gradient
    /// they are what lets a plate continue the gradient instead of cutting a
    /// flat rectangle out of it.
    pub corners: [Rgb; 4],
    pub text: Rgb,
    /// True when there is no surface to speak of: the text lies over a scene
    /// or a photograph, and what surrounds it is neither one colour nor a
    /// gradient between four.
    pub busy: bool,
}

impl TextColours {
    /// One colour for the surface and one for the text.
    pub fn flat(background: Rgb, text: Rgb) -> Self {
        Self {
            background,
            corners: [background; 4],
            text,
            busy: false,
        }
    }

    /// Whether the surface is one colour all over, near enough.
    pub fn is_flat(&self) -> bool {
        self.corners
            .iter()
            .all(|corner| corner.distance(self.background) < 9.0)
    }
}

/// How far a rectangle could grow before it runs into something that is not
/// its own background, in pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Clearance {
    pub left: f32,
    pub right: f32,
    pub below: f32,
}

impl Frame {
    /// Takes ownership of BGRA rows straight from the platform capture.
    pub fn from_bgra(bounds: Bounds, mut bgra: Vec<u8>) -> Self {
        for px in bgra.chunks_exact_mut(4) {
            px.swap(0, 2);
            // GDI leaves the alpha byte at zero; an image that is fully
            // transparent everywhere is not what anyone downstream expects.
            px[3] = 255;
        }
        Self {
            bounds,
            pixels: Arc::new(bgra),
        }
    }

    #[cfg(test)]
    pub fn from_rgba(bounds: Bounds, rgba: Vec<u8>) -> Self {
        Self {
            bounds,
            pixels: Arc::new(rgba),
        }
    }

    pub fn width(&self) -> usize {
        self.bounds.w.max(0) as usize
    }

    pub fn height(&self) -> usize {
        self.bounds.h.max(0) as usize
    }

    #[cfg(test)]
    pub fn rgba(&self) -> &[u8] {
        &self.pixels
    }

    /// The part of this frame that covers `area` of the desktop, or `None`
    /// when they do not overlap.
    pub fn crop(&self, area: Bounds) -> Option<Frame> {
        let hit = area.clamp_to(self.bounds)?;
        if hit == self.bounds {
            return Some(self.clone());
        }

        let (w, h) = (hit.w as usize, hit.h as usize);
        let stride = self.width() * 4;
        let x0 = (hit.x - self.bounds.x) as usize * 4;
        let y0 = (hit.y - self.bounds.y) as usize;

        let mut out = Vec::with_capacity(w * h * 4);
        for row in 0..h {
            let start = (y0 + row) * stride + x0;
            out.extend_from_slice(&self.pixels[start..start + w * 4]);
        }
        Some(Frame {
            bounds: hit,
            pixels: Arc::new(out),
        })
    }

    pub fn to_color_image(&self) -> egui::ColorImage {
        // Every pixel is opaque, so premultiplied and straight alpha are the
        // same bytes and the cheaper constructor applies.
        egui::ColorImage::from_rgba_premultiplied([self.width(), self.height()], &self.pixels)
    }

    pub fn to_rgb_image(&self) -> Option<image::RgbImage> {
        let mut rgb = Vec::with_capacity(self.width() * self.height() * 3);
        for px in self.pixels.chunks_exact(4) {
            rgb.extend_from_slice(&px[..3]);
        }
        image::RgbImage::from_raw(self.width() as u32, self.height() as u32, rgb)
    }

    fn pixel(&self, x: usize, y: usize) -> Rgb {
        let i = (y * self.width() + x) * 4;
        Rgb(self.pixels[i], self.pixels[i + 1], self.pixels[i + 2])
    }

    /// Works out what colour a block of text is, and what it is written on.
    ///
    /// `x, y, w, h` are relative to this frame. The background is the colour
    /// that dominates a thin ring just outside the block — glyphs do not reach
    /// it, so whatever is most common there is the surface the text sits on.
    /// The text colour is then whatever inside the block differs from that the
    /// most, which is the glyph cores rather than their antialiased edges.
    ///
    /// When the block turns out to have no usable contrast — a photograph, a
    /// gradient — the text falls back to black or white, whichever reads.
    pub fn text_colours(&self, x: f32, y: f32, w: f32, h: f32) -> TextColours {
        let (fw, fh) = (self.width() as i32, self.height() as i32);
        if fw == 0 || fh == 0 {
            return TextColours::flat(Rgb(24, 24, 28), Rgb(245, 245, 245));
        }

        let clamp_x = |v: f32| (v.round() as i32).clamp(0, fw - 1);
        let clamp_y = |v: f32| (v.round() as i32).clamp(0, fh - 1);
        let (x0, y0) = (clamp_x(x), clamp_y(y));
        let (x1, y1) = (clamp_x(x + w), clamp_y(y + h));

        // The ring: one row above and below, one column either side, pushed
        // out a few pixels so antialiasing from the glyphs has faded.
        const RING: i32 = 3;
        let (rx0, ry0) = ((x0 - RING).max(0), (y0 - RING).max(0));
        let (rx1, ry1) = ((x1 + RING).min(fw - 1), (y1 + RING).min(fh - 1));
        let (mid_x, mid_y) = ((rx0 + rx1) / 2, (ry0 + ry1) / 2);

        // One histogram for the whole ring and one per quarter of it, so a
        // gradient can be told from a flat panel.
        let mut ring = Histogram::default();
        let mut quarters: [Histogram; 4] = Default::default();
        let mut ring_pixels: Vec<(i32, i32, Rgb)> = Vec::new();
        // Along the way, how often one sample is nothing like the one before:
        // the difference between a surface and a scene.
        let (mut pairs, mut jumps) = (0u32, 0u32);
        let mut walk = |points: &mut dyn Iterator<Item = (i32, i32)>| {
            let mut previous: Option<Rgb> = None;
            for (px, py) in points {
                let c = self.pixel(px as usize, py as usize);
                ring.add(c);
                let quarter = usize::from(px > mid_x) + 2 * usize::from(py > mid_y);
                quarters[quarter].add(c);
                ring_pixels.push((px, py, c));
                if let Some(before) = previous {
                    pairs += 1;
                    if c.distance(before) > 22.0 {
                        jumps += 1;
                    }
                }
                previous = Some(c);
            }
        };
        let step_x = (((rx1 - rx0) / 96).max(1)) as usize;
        let step_y = (((ry1 - ry0) / 96).max(1)) as usize;
        walk(&mut (rx0..=rx1).step_by(step_x).map(|px| (px, ry0)));
        walk(&mut (rx0..=rx1).step_by(step_x).map(|px| (px, ry1)));
        walk(&mut (ry0..=ry1).step_by(step_y).map(|py| (rx0, py)));
        walk(&mut (ry0..=ry1).step_by(step_y).map(|py| (rx1, py)));
        let background = ring.dominant().unwrap_or(Rgb(24, 24, 28));
        let typical = quarters.map(|quarter| quarter.dominant().unwrap_or(background));

        // What a quarter is mostly made of is the colour somewhere along it.
        // A plate shaded from that would run a flatter gradient than the one
        // it sits on, and show at both ends. So each corner is read from the
        // pixels right next to it — those of them that are surface at all,
        // and not a neighbouring letter or the edge of something.
        let near_x = ((rx1 - rx0) / 10).max(2);
        let near_y = ((ry1 - ry0) / 10).max(2);
        let mut corners = typical;
        for (i, (cx, cy)) in [(rx0, ry0), (rx1, ry0), (rx0, ry1), (rx1, ry1)]
            .into_iter()
            .enumerate()
        {
            let (mut sum, mut count) = ([0u32; 3], 0u32);
            for (px, py, c) in &ring_pixels {
                let close = (px - cx).abs() <= near_x && (py - cy).abs() <= near_y;
                if close && c.distance(typical[i]) < 48.0 {
                    sum[0] += c.0 as u32;
                    sum[1] += c.1 as u32;
                    sum[2] += c.2 as u32;
                    count += 1;
                }
            }
            let mean = |total: u32| total.checked_div(count).map(|value| value as u8);
            if let (Some(r), Some(g), Some(b)) = (mean(sum[0]), mean(sum[1]), mean(sum[2])) {
                corners[i] = Rgb(r, g, b);
            }
        }

        // Is there a surface at all? A gradient changes all the way across
        // and never once abruptly. A panel may have another line of text run
        // through the ring, abrupt as anything, and is still mostly panel.
        // Only a scene is both: restless, and no one colour's.
        let restless = pairs >= 12 && jumps * 5 > pairs;
        let busy = restless && ring.share_of_dominant() < 0.4;

        // Inside: how far does anything get from the background?
        let step_x = (((x1 - x0) / 160).max(1)) as usize;
        let step_y = (((y1 - y0) / 48).max(1)) as usize;
        let mut far = 0.0_f32;
        let mut samples = Vec::new();
        for py in (y0..=y1).step_by(step_y) {
            for px in (x0..=x1).step_by(step_x) {
                let c = self.pixel(px as usize, py as usize);
                // Measured against the nearest corner, so that on a gradient
                // the far side of the surface is not mistaken for text.
                let d = corners
                    .iter()
                    .map(|corner| c.distance(*corner))
                    .fold(c.distance(background), f32::min);
                far = far.max(d);
                samples.push((c, d));
            }
        }

        // Glyph cores are the samples near the far end; their edges, blended
        // with the background, fall short of it and are left out.
        let mut glyphs = Histogram::default();
        for (c, d) in &samples {
            if *d >= far * 0.72 {
                glyphs.add(*c);
            }
        }
        let text = glyphs
            .dominant()
            .filter(|c| far >= 28.0 && c.contrast(background) >= 2.2)
            .unwrap_or_else(|| readable_on(background));

        TextColours {
            background,
            corners,
            text,
            busy,
        }
    }

    /// Measures how far a rectangle could be stretched to the left, to the
    /// right and downwards over its own background.
    ///
    /// A translation is usually longer than what it replaces, and the plate
    /// under it has to grow somewhere. Growing over more of the same surface
    /// is invisible; growing off the end of a button, or over the next
    /// column's text, is a dark sticker on the page. So each direction is
    /// walked pixel by pixel until the picture stops looking like the surface
    /// the text was on.
    ///
    /// "Looks like" is judged against the pixel just before, not against one
    /// fixed colour: a gradient drifts a long way across a panel without ever
    /// taking a step, and an edge is a step.
    pub fn clearance(&self, x: f32, y: f32, w: f32, h: f32) -> Clearance {
        /// How different two neighbouring pixels may be and still be surface.
        const STEP: f32 = 22.0;
        /// Pixels skipped next to the rectangle: the last letter's antialiasing.
        const SKIP: i32 = 2;

        let (fw, fh) = (self.width() as i32, self.height() as i32);
        let none = Clearance {
            left: 0.0,
            right: 0.0,
            below: 0.0,
        };
        if fw < 4 || fh < 4 || w < 1.0 || h < 1.0 {
            return none;
        }

        let x0 = (x.round() as i32).clamp(0, fw - 1);
        let y0 = (y.round() as i32).clamp(0, fh - 1);
        let x1 = ((x + w).round() as i32).clamp(0, fw - 1);
        let y1 = ((y + h).round() as i32).clamp(0, fh - 1);
        if x1 <= x0 || y1 <= y0 {
            return none;
        }

        // A few probes spread across the side being walked from.
        let probes = |from: i32, to: i32, count: i32| -> Vec<i32> {
            let span = (to - from).max(1);
            (0..count)
                .map(|i| from + (span * (2 * i + 1)) / (2 * count))
                .collect()
        };

        // Walks away from the rectangle one line of probes at a time and
        // returns how many lines were clear. A line is clear when all but one
        // of its probes continue the surface — one speck is not an edge.
        let walk = |start: i32, end: i32, step: i32, pixel: &dyn Fn(i32, i32) -> Rgb, along: &[i32]| {
            let mut previous: Vec<Rgb> = along.iter().map(|a| pixel(start, *a)).collect();
            let mut clear = 0;
            let mut at = start + step;
            while at != end + step {
                let mut steps = 0;
                let mut line = Vec::with_capacity(along.len());
                for (probe, before) in along.iter().zip(&previous) {
                    let c = pixel(at, *probe);
                    if c.distance(*before) > STEP {
                        steps += 1;
                    }
                    line.push(c);
                }
                if steps > 1 {
                    break;
                }
                previous = line;
                clear += 1;
                at += step;
            }
            clear as f32
        };

        let rows = probes(y0, y1, 5);
        let columns = probes(x0, x1, 9);
        let by_column = |cx: i32, cy: i32| self.pixel(cx as usize, cy as usize);
        let by_row = |cy: i32, cx: i32| self.pixel(cx as usize, cy as usize);

        let right = if x1 + SKIP < fw - 1 {
            walk(x1 + SKIP, fw - 1, 1, &by_column, &rows) + SKIP as f32
        } else {
            (fw - 1 - x1) as f32
        };
        let left = if x0 - SKIP > 0 {
            walk(x0 - SKIP, 0, -1, &by_column, &rows) + SKIP as f32
        } else {
            x0 as f32
        };
        let below = if y1 + SKIP < fh - 1 {
            walk(y1 + SKIP, fh - 1, 1, &by_row, &columns) + SKIP as f32
        } else {
            (fh - 1 - y1) as f32
        };

        Clearance { left, right, below }
    }

    /// The rows of a line of text that carry ink: from the top of its tallest
    /// letter to the bottom of its deepest, in frame pixels.
    ///
    /// A recogniser draws its boxes with a loose hand — the same type comes
    /// back fifteen pixels tall around one word and twenty-two around the
    /// next — and sizes worked out from them turn a uniform column of labels
    /// into a ransom note. The pixels themselves are exact: a row belongs to
    /// the text when something in it is nearer the text's colour than the
    /// surface's.
    ///
    /// `None` when the letters cannot be told from what they are written on,
    /// or when what was found is not plausibly one line of text.
    pub fn ink_rows(
        &self,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        colours: &TextColours,
    ) -> Option<(f32, f32)> {
        let (fw, fh) = (self.width() as i32, self.height() as i32);
        if fw < 2 || fh < 2 || w < 2.0 || h < 3.0 {
            return None;
        }
        // Too alike to be separated by which one a pixel is nearer to.
        if colours.text.distance(colours.background) < 48.0 {
            return None;
        }

        let x0 = (x.round() as i32).clamp(0, fw - 1);
        let x1 = ((x + w).round() as i32).clamp(0, fw - 1);
        // A box can clip a descender; look a little past it either way.
        let reach = (h * 0.2).ceil() as i32;
        let top = ((y.round() as i32) - reach).clamp(0, fh - 1);
        let bottom = (((y + h).round() as i32) + reach).clamp(0, fh - 1);
        if x1 <= x0 || bottom <= top {
            return None;
        }

        let is_ink = |px: i32, py: i32| {
            let c = self.pixel(px as usize, py as usize);
            let to_surface = colours
                .corners
                .iter()
                .map(|corner| c.distance(*corner))
                .fold(c.distance(colours.background), f32::min);
            c.distance(colours.text) < to_surface
        };
        let width = (x1 - x0 + 1) as f32;
        let rows: Vec<bool> = (top..=bottom)
            .map(|py| {
                let ink = (x0..=x1).filter(|px| is_ink(*px, py)).count() as f32;
                // Ink nearly all the way across is a rule or the edge of a
                // panel, not lettering.
                ink >= 1.0 && ink <= width * 0.85
            })
            .collect();

        // From the middle of the box, where a line of text always has ink,
        // outwards for as long as the rows keep carrying some. Small gaps are
        // bridged: the dot of an "i" floats clear of its stem.
        let gap = ((h * 0.15).round() as usize).max(2);
        let middle = rows.len() / 2;
        let start = (0..=rows.len() / 3)
            .flat_map(|d| [middle.saturating_sub(d), (middle + d).min(rows.len() - 1)])
            .find(|i| rows[*i])?;

        let mut first = start;
        let mut missed = 0;
        for i in (0..start).rev() {
            if rows[i] {
                first = i;
                missed = 0;
            } else {
                missed += 1;
                if missed > gap {
                    break;
                }
            }
        }
        let mut last = start;
        let mut missed = 0;
        for (i, ink) in rows.iter().enumerate().skip(start + 1) {
            if *ink {
                last = i;
                missed = 0;
            } else {
                missed += 1;
                if missed > gap {
                    break;
                }
            }
        }

        // One line of text is neither a sliver nor much taller than its box.
        let height = (last - first + 1) as f32;
        if height < 3.0 || height < h * 0.4 || height > h * 1.3 {
            return None;
        }
        Some((
            (top as usize + first) as f32,
            (top as usize + last + 1) as f32,
        ))
    }

    /// A coarse fingerprint of the image, for telling "the dialogue changed"
    /// from "a particle drifted past".
    pub fn signature(&self) -> Signature {
        const COLS: usize = 32;
        const ROWS: usize = 16;

        let (w, h) = (self.width(), self.height());
        let mut cells = vec![0u8; COLS * ROWS];
        if w == 0 || h == 0 {
            return Signature { cells };
        }

        for cy in 0..ROWS {
            for cx in 0..COLS {
                let x0 = cx * w / COLS;
                let x1 = ((cx + 1) * w / COLS).max(x0 + 1).min(w);
                let y0 = cy * h / ROWS;
                let y1 = ((cy + 1) * h / ROWS).max(y0 + 1).min(h);

                // A handful of samples per cell is plenty: this only has to
                // notice text appearing, not reproduce the image.
                let step_x = ((x1 - x0) / 6).max(1);
                let step_y = ((y1 - y0) / 6).max(1);
                let (mut sum, mut n) = (0.0_f32, 0u32);
                for y in (y0..y1).step_by(step_y) {
                    for x in (x0..x1).step_by(step_x) {
                        sum += self.pixel(x, y).luma();
                        n += 1;
                    }
                }
                cells[cy * COLS + cx] = if n == 0 { 0 } else { (sum / n as f32) as u8 };
            }
        }
        Signature { cells }
    }
}

/// See [`Frame::signature`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Signature {
    cells: Vec<u8>,
}

impl Signature {
    /// The share of the image that looks different, 0–1.
    pub fn difference(&self, other: &Signature) -> f32 {
        if self.cells.len() != other.cells.len() || self.cells.is_empty() {
            return 1.0;
        }
        // A cell counts as changed past a brightness step no compression
        // artefact or video grain produces on its own.
        const STEP: i16 = 14;
        let changed = self
            .cells
            .iter()
            .zip(&other.cells)
            .filter(|(a, b)| (**a as i16 - **b as i16).abs() > STEP)
            .count();
        changed as f32 / self.cells.len() as f32
    }
}

/// Near-black or near-white, whichever stands out more on `background`.
///
/// Decided by the contrast each would actually have, not by a brightness
/// threshold: the two cross over at a mid grey well below "half way", and a
/// threshold in the wrong place picks the weaker of the two exactly where
/// neither is strong.
pub fn readable_on(background: Rgb) -> Rgb {
    const LIGHT: Rgb = Rgb(250, 250, 250);
    const DARK: Rgb = Rgb(10, 10, 12);
    if LIGHT.contrast(background) >= DARK.contrast(background) {
        LIGHT
    } else {
        DARK
    }
}

/// Colour buckets, five bits a channel: coarse enough that antialiasing and
/// compression noise fall into one bucket, fine enough that white text on a
/// pale panel does not.
#[derive(Default)]
struct Histogram {
    buckets: std::collections::HashMap<u16, (u32, [u32; 3])>,
}

impl Histogram {
    fn add(&mut self, c: Rgb) {
        let key = ((c.0 as u16 >> 3) << 10) | ((c.1 as u16 >> 3) << 5) | (c.2 as u16 >> 3);
        let entry = self.buckets.entry(key).or_insert((0, [0; 3]));
        entry.0 += 1;
        entry.1[0] += c.0 as u32;
        entry.1[1] += c.1 as u32;
        entry.1[2] += c.2 as u32;
    }

    /// How much of everything added the fullest bucket holds, 0–1.
    fn share_of_dominant(&self) -> f32 {
        let total: u32 = self.buckets.values().map(|(count, _)| *count).sum();
        let fullest = self.buckets.values().map(|(count, _)| *count).max();
        match fullest {
            Some(fullest) if total > 0 => fullest as f32 / total as f32,
            _ => 0.0,
        }
    }

    /// The average colour of the fullest bucket.
    fn dominant(&self) -> Option<Rgb> {
        self.buckets
            .values()
            .max_by_key(|(count, _)| *count)
            .map(|(count, sum)| {
                Rgb(
                    (sum[0] / count) as u8,
                    (sum[1] / count) as u8,
                    (sum[2] / count) as u8,
                )
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(w: i32, h: i32, c: Rgb) -> Vec<u8> {
        let mut v = Vec::with_capacity((w * h * 4) as usize);
        for _ in 0..(w * h) {
            v.extend_from_slice(&[c.0, c.1, c.2, 255]);
        }
        v
    }

    fn paint(rgba: &mut [u8], w: i32, x: i32, y: i32, pw: i32, ph: i32, c: Rgb) {
        for py in y..y + ph {
            for px in x..x + pw {
                let i = ((py * w + px) * 4) as usize;
                rgba[i..i + 3].copy_from_slice(&[c.0, c.1, c.2]);
            }
        }
    }

    fn frame(w: i32, h: i32, rgba: Vec<u8>) -> Frame {
        Frame::from_rgba(Bounds { x: 0, y: 0, w, h }, rgba)
    }

    #[test]
    fn bgra_from_the_platform_becomes_opaque_rgba() {
        let f = Frame::from_bgra(
            Bounds {
                x: 0,
                y: 0,
                w: 1,
                h: 1,
            },
            vec![10, 20, 30, 0],
        );
        assert_eq!(f.rgba(), &[30, 20, 10, 255]);
    }

    #[test]
    fn a_crop_keeps_the_pixels_and_remembers_where_it_came_from() {
        let mut rgba = solid(8, 6, Rgb(0, 0, 0));
        paint(&mut rgba, 8, 2, 1, 3, 2, Rgb(200, 100, 50));
        let f = Frame::from_rgba(
            Bounds {
                x: 100,
                y: 50,
                w: 8,
                h: 6,
            },
            rgba,
        );

        let crop = f
            .crop(Bounds {
                x: 102,
                y: 51,
                w: 3,
                h: 2,
            })
            .expect("overlaps");
        assert_eq!(
            crop.bounds,
            Bounds {
                x: 102,
                y: 51,
                w: 3,
                h: 2
            }
        );
        assert_eq!(crop.rgba().len(), 3 * 2 * 4);
        assert!(crop
            .rgba()
            .chunks_exact(4)
            .all(|p| p == [200, 100, 50, 255]));
    }

    #[test]
    fn a_crop_running_off_the_edge_is_trimmed() {
        let f = frame(10, 10, solid(10, 10, Rgb(1, 2, 3)));
        let crop = f
            .crop(Bounds {
                x: 6,
                y: 6,
                w: 20,
                h: 20,
            })
            .expect("overlaps");
        assert_eq!((crop.bounds.w, crop.bounds.h), (4, 4));
    }

    #[test]
    fn a_crop_that_misses_the_frame_is_none() {
        let f = frame(10, 10, solid(10, 10, Rgb(1, 2, 3)));
        assert!(f
            .crop(Bounds {
                x: 50,
                y: 50,
                w: 5,
                h: 5
            })
            .is_none());
    }

    #[test]
    fn light_text_on_a_dark_panel_is_read_as_such() {
        // A navy panel with two strokes of gold "text" inside the block.
        let navy = Rgb(24, 28, 44);
        let gold = Rgb(255, 200, 80);
        let mut rgba = solid(200, 60, navy);
        paint(&mut rgba, 200, 30, 22, 60, 4, gold);
        paint(&mut rgba, 200, 100, 22, 50, 4, gold);
        let f = frame(200, 60, rgba);

        let c = f.text_colours(20.0, 15.0, 150.0, 20.0);
        assert_eq!(c.background, navy);
        assert_eq!(c.text, gold);
        assert!(c.is_flat(), "a flat panel was read as a gradient: {c:?}");
    }

    /// Left to right, dark blue to orange.
    fn gradient(w: i32, h: i32) -> Vec<u8> {
        let mut rgba = Vec::with_capacity((w * h * 4) as usize);
        for _ in 0..h {
            for x in 0..w {
                let t = x as f32 / (w - 1) as f32;
                rgba.extend_from_slice(&[
                    (20.0 + 170.0 * t) as u8,
                    (30.0 + 80.0 * t) as u8,
                    (70.0 - 10.0 * t) as u8,
                    255,
                ]);
            }
        }
        rgba
    }

    #[test]
    fn a_gradient_is_reported_corner_by_corner() {
        let mut rgba = gradient(400, 80);
        paint(&mut rgba, 400, 60, 34, 280, 6, Rgb(255, 255, 255));
        let f = frame(400, 80, rgba);

        let c = f.text_colours(50.0, 25.0, 300.0, 24.0);
        assert!(!c.is_flat(), "a gradient was read as flat: {c:?}");
        // Darker on the left than on the right, top and bottom alike.
        assert!(c.corners[0].luma() + 20.0 < c.corners[1].luma(), "{c:?}");
        assert!(c.corners[2].luma() + 20.0 < c.corners[3].luma(), "{c:?}");
        // And the white stroke is still what gets called the text.
        assert_eq!(c.text, Rgb(255, 255, 255));
    }

    #[test]
    fn a_label_on_a_button_may_only_grow_as_far_as_the_button() {
        // A light page with a dark button from x = 80 to 300, and a short
        // stroke of "text" in the middle of it.
        let mut rgba = solid(600, 100, Rgb(244, 244, 246));
        paint(&mut rgba, 600, 80, 30, 220, 40, Rgb(36, 40, 52));
        paint(&mut rgba, 600, 150, 46, 80, 8, Rgb(255, 255, 255));
        let f = frame(600, 100, rgba);

        let room = f.clearance(150.0, 44.0, 80.0, 12.0);
        // 70 pixels of button either side of the text, and not a pixel of
        // page beyond its edge.
        assert!((room.left - 70.0).abs() <= 3.0, "{room:?}");
        assert!((room.right - 70.0).abs() <= 3.0, "{room:?}");
        assert!(room.below <= 16.0, "{room:?}");
    }

    #[test]
    fn text_on_an_open_surface_has_the_whole_surface_to_grow_into() {
        let mut rgba = solid(600, 100, Rgb(24, 28, 44));
        paint(&mut rgba, 600, 40, 20, 100, 8, Rgb(240, 240, 240));
        let f = frame(600, 100, rgba);

        let room = f.clearance(40.0, 18.0, 100.0, 12.0);
        assert!(room.right >= 440.0, "{room:?}");
        assert!(room.left >= 38.0, "{room:?}");
        assert!(room.below >= 60.0, "{room:?}");
    }

    #[test]
    fn a_neighbouring_column_of_text_stops_the_growth() {
        let mut rgba = solid(600, 60, Rgb(250, 250, 250));
        paint(&mut rgba, 600, 40, 24, 100, 10, Rgb(20, 20, 20));
        // The value column, starting at x = 400.
        paint(&mut rgba, 600, 400, 22, 120, 14, Rgb(20, 90, 190));
        let f = frame(600, 60, rgba);

        let room = f.clearance(40.0, 22.0, 100.0, 14.0);
        assert!(room.right <= 262.0 && room.right >= 250.0, "{room:?}");
    }

    #[test]
    fn a_gradient_is_one_surface_however_far_it_drifts() {
        let mut rgba = gradient(600, 60);
        paint(&mut rgba, 600, 200, 26, 100, 8, Rgb(255, 255, 255));
        let f = frame(600, 60, rgba);

        // From dark blue at one end to orange at the other, and no step
        // anywhere along the way.
        let room = f.clearance(200.0, 24.0, 100.0, 12.0);
        assert!(room.left >= 195.0, "{room:?}");
        assert!(room.right >= 295.0, "{room:?}");
    }

    #[test]
    fn clearance_of_a_rectangle_off_the_frame_is_nothing_not_a_panic() {
        let f = frame(50, 50, solid(50, 50, Rgb(9, 9, 9)));
        let _ = f.clearance(-100.0, -100.0, 20.0, 20.0);
        let _ = f.clearance(500.0, 500.0, 20.0, 20.0);
        let _ = f.clearance(10.0, 10.0, 0.0, 0.0);
        let tiny = frame(2, 2, solid(2, 2, Rgb(9, 9, 9)));
        let _ = tiny.clearance(0.0, 0.0, 2.0, 2.0);
    }

    /// Something shaped like a line of lettering: a tall stem, a run of
    /// x-height letters and one that hangs below the baseline.
    fn lettering(rgba: &mut [u8], w: i32, ink: Rgb) {
        paint(rgba, w, 20, 20, 6, 14, ink); // ascender: rows 20..34
        paint(rgba, w, 40, 26, 30, 8, ink); // x-height: rows 26..34
        paint(rgba, w, 80, 26, 4, 12, ink); // descender: rows 26..38
    }

    const PAGE: Rgb = Rgb(30, 30, 40);
    const INK: Rgb = Rgb(240, 240, 240);

    #[test]
    fn the_ink_of_a_line_is_found_inside_a_loose_box() {
        let mut rgba = solid(200, 60, PAGE);
        lettering(&mut rgba, 200, INK);
        let f = frame(200, 60, rgba);
        // The box a recogniser might give: a few pixels of air all round.
        let rows = f.ink_rows(16.0, 16.0, 72.0, 26.0, &TextColours::flat(PAGE, INK));
        assert_eq!(rows, Some((20.0, 38.0)));
    }

    #[test]
    fn a_box_that_clips_the_descender_still_finds_it() {
        let mut rgba = solid(200, 60, PAGE);
        lettering(&mut rgba, 200, INK);
        let f = frame(200, 60, rgba);
        let rows = f.ink_rows(16.0, 19.0, 72.0, 16.0, &TextColours::flat(PAGE, INK));
        assert_eq!(rows, Some((20.0, 38.0)));
    }

    #[test]
    fn a_rule_under_the_text_is_not_lettering() {
        let mut rgba = solid(200, 60, PAGE);
        lettering(&mut rgba, 200, INK);
        // Right under the descender, the full width of the panel.
        paint(&mut rgba, 200, 0, 40, 200, 2, INK);
        let f = frame(200, 60, rgba);
        let rows = f.ink_rows(16.0, 16.0, 72.0, 26.0, &TextColours::flat(PAGE, INK));
        assert_eq!(rows, Some((20.0, 38.0)));
    }

    #[test]
    fn a_dot_floating_over_a_letter_belongs_to_the_line() {
        let mut rgba = solid(200, 60, PAGE);
        paint(&mut rgba, 200, 40, 26, 30, 8, INK); // x-height letters
        paint(&mut rgba, 200, 50, 22, 3, 2, INK); // the dot of an "i", two rows clear
        let f = frame(200, 60, rgba);
        let rows = f.ink_rows(36.0, 20.0, 40.0, 16.0, &TextColours::flat(PAGE, INK));
        assert_eq!(rows, Some((22.0, 34.0)));
    }

    #[test]
    fn the_line_above_is_not_mistaken_for_part_of_this_one() {
        let mut rgba = solid(200, 80, PAGE);
        // The line above ends six rows before this one starts, close enough
        // to be inside the area searched.
        paint(&mut rgba, 200, 20, 14, 20, 10, INK); // rows 14..24
        paint(&mut rgba, 200, 20, 30, 20, 12, INK); // rows 30..42
        paint(&mut rgba, 200, 60, 30, 10, 12, INK);
        let f = frame(200, 80, rgba);
        let rows = f.ink_rows(18.0, 27.0, 64.0, 18.0, &TextColours::flat(PAGE, INK));
        assert_eq!(rows, Some((30.0, 42.0)));
    }

    #[test]
    fn letters_that_cannot_be_told_from_their_surface_are_not_measured() {
        let mut rgba = solid(200, 60, PAGE);
        lettering(&mut rgba, 200, Rgb(44, 44, 52));
        let f = frame(200, 60, rgba);
        let colours = TextColours::flat(PAGE, Rgb(44, 44, 52));
        assert_eq!(f.ink_rows(16.0, 16.0, 72.0, 26.0, &colours), None);
    }

    #[test]
    fn an_empty_box_has_no_ink() {
        let f = frame(200, 60, solid(200, 60, PAGE));
        let colours = TextColours::flat(PAGE, INK);
        assert_eq!(f.ink_rows(16.0, 16.0, 72.0, 26.0, &colours), None);
        // Nor does one hanging off the frame, and it does not panic either.
        assert_eq!(f.ink_rows(500.0, 500.0, 72.0, 26.0, &colours), None);
    }

    /// A scene: blocks of unrelated colours, the way foliage or rubble is.
    fn scenery(w: i32, h: i32) -> Vec<u8> {
        let mut v = Vec::with_capacity((w * h * 4) as usize);
        for y in 0..h {
            for x in 0..w {
                // A cheap hash of the 6-pixel cell the pixel is in.
                let cell = ((x / 6) as u32 * 73_856_093) ^ ((y / 6) as u32 * 19_349_663);
                let c = cell.wrapping_mul(2_654_435_761);
                v.extend_from_slice(&[(c >> 8) as u8, (c >> 16) as u8, (c >> 24) as u8, 255]);
            }
        }
        v
    }

    #[test]
    fn text_over_a_scene_has_no_surface() {
        let mut rgba = scenery(300, 120);
        paint(&mut rgba, 300, 60, 50, 180, 6, Rgb(255, 255, 255));
        let c = frame(300, 120, rgba).text_colours(50.0, 40.0, 200.0, 26.0);
        assert!(c.busy, "scenery was taken for a surface: {c:?}");
    }

    #[test]
    fn a_panel_with_other_text_close_by_is_still_a_surface() {
        // A dense list: the line above runs right through the ring.
        let mut rgba = solid(300, 120, PAGE);
        for x in (40..260).step_by(9) {
            paint(&mut rgba, 300, x, 30, 3, 9, INK); // strokes of the line above
        }
        paint(&mut rgba, 300, 60, 46, 180, 10, INK);
        let c = frame(300, 120, rgba).text_colours(50.0, 41.0, 200.0, 20.0);
        assert!(!c.busy, "a panel was taken for scenery: {c:?}");
        assert!(c.is_flat());
    }

    #[test]
    fn a_gradient_is_a_surface() {
        let rgba = gradient(300, 120);
        let c = frame(300, 120, rgba).text_colours(50.0, 40.0, 200.0, 26.0);
        assert!(!c.busy, "a gradient was taken for scenery: {c:?}");
    }

    #[test]
    fn dark_text_on_a_light_page_is_read_as_such() {
        let paper = Rgb(250, 250, 248);
        let ink = Rgb(20, 20, 20);
        let mut rgba = solid(200, 60, paper);
        paint(&mut rgba, 200, 30, 22, 120, 3, ink);
        let f = frame(200, 60, rgba);

        let c = f.text_colours(20.0, 15.0, 150.0, 20.0);
        assert_eq!(c.background, paper);
        assert_eq!(c.text, ink);
    }

    #[test]
    fn a_block_with_no_contrast_falls_back_to_something_legible() {
        let grey = Rgb(120, 120, 120);
        let f = frame(100, 40, solid(100, 40, grey));
        let c = f.text_colours(10.0, 10.0, 60.0, 15.0);
        assert_eq!(c.background, grey);
        assert!(
            c.text.contrast(c.background) >= 4.2,
            "fallback text is not readable: {:?}",
            c
        );
    }

    #[test]
    fn a_block_outside_the_frame_does_not_panic() {
        let f = frame(20, 20, solid(20, 20, Rgb(9, 9, 9)));
        let _ = f.text_colours(-50.0, -50.0, 500.0, 500.0);
        let _ = f.text_colours(100.0, 100.0, 5.0, 5.0);
    }

    #[test]
    fn the_fallback_text_is_readable_on_any_background() {
        // Every grey there is. The worst case is the mid grey where black and
        // white are equally far away, and even there the better of the two
        // clears four to one.
        for v in 0..=255u8 {
            let bg = Rgb(v, v, v);
            let ratio = readable_on(bg).contrast(bg);
            assert!(ratio >= 4.2, "only {ratio:.2}:1 on grey {v}");
        }
    }

    #[test]
    fn the_fallback_is_light_on_dark_and_dark_on_light() {
        assert!(readable_on(Rgb(20, 24, 40)).luma() > 200.0);
        assert!(readable_on(Rgb(245, 240, 230)).luma() < 40.0);
    }

    #[test]
    fn the_same_image_has_no_difference() {
        let f = frame(64, 32, solid(64, 32, Rgb(30, 30, 30)));
        assert_eq!(f.signature().difference(&f.signature()), 0.0);
    }

    #[test]
    fn new_text_shows_up_as_a_difference() {
        let before = frame(320, 80, solid(320, 80, Rgb(20, 20, 30)));
        let mut rgba = solid(320, 80, Rgb(20, 20, 30));
        paint(&mut rgba, 320, 20, 20, 280, 40, Rgb(240, 240, 240));
        let after = frame(320, 80, rgba);

        let diff = before.signature().difference(&after.signature());
        assert!(diff > 0.3, "a line of text only registered as {diff}");
    }

    #[test]
    fn a_faint_flicker_does_not_register() {
        let a = frame(320, 80, solid(320, 80, Rgb(20, 20, 30)));
        let b = frame(320, 80, solid(320, 80, Rgb(24, 23, 33)));
        assert_eq!(a.signature().difference(&b.signature()), 0.0);
    }

    #[test]
    fn signatures_of_different_sizes_count_as_fully_different() {
        let a = Signature { cells: vec![0; 4] };
        let b = Signature { cells: vec![0; 8] };
        assert_eq!(a.difference(&b), 1.0);
    }
}
