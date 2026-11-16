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

    /// Half way between two colours.
    fn mean(self, other: Rgb) -> Rgb {
        let half = |a: u8, b: u8| ((a as u16 + b as u16) / 2) as u8;
        Rgb(half(self.0, other.0), half(self.1, other.1), half(self.2, other.2))
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

    /// The pixels themselves: RGBA, row by row.
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
        // Along the way, how often one sample is nothing like the one before:
        // the difference between a surface and a scene.
        let (mut pairs, mut jumps) = (0u32, 0u32);
        // One side of the ring, in the order it was walked.
        let mut walk = |points: &mut dyn Iterator<Item = (i32, i32)>| -> Vec<Rgb> {
            let mut side = Vec::new();
            let mut previous: Option<Rgb> = None;
            for (px, py) in points {
                let c = self.pixel(px as usize, py as usize);
                ring.add(c);
                let quarter = usize::from(px > mid_x) + 2 * usize::from(py > mid_y);
                quarters[quarter].add(c);
                side.push(c);
                if let Some(before) = previous {
                    pairs += 1;
                    if c.distance(before) > 22.0 {
                        jumps += 1;
                    }
                }
                previous = Some(c);
            }
            side
        };
        let step_x = (((rx1 - rx0) / 96).max(1)) as usize;
        let step_y = (((ry1 - ry0) / 96).max(1)) as usize;
        let top = walk(&mut (rx0..=rx1).step_by(step_x).map(|px| (px, ry0)));
        let bottom = walk(&mut (rx0..=rx1).step_by(step_x).map(|px| (px, ry1)));
        let left = walk(&mut (ry0..=ry1).step_by(step_y).map(|py| (rx0, py)));
        let right = walk(&mut (ry0..=ry1).step_by(step_y).map(|py| (rx1, py)));
        let background = ring.dominant().unwrap_or(Rgb(24, 24, 28));
        let typical = quarters.map(|quarter| quarter.dominant().unwrap_or(background));

        // What a quarter is mostly made of is the colour somewhere along it.
        // A plate shaded from that would run a flatter gradient than the one
        // it sits on, and show at both ends. So each corner is read where it
        // is — by following the surface along the two sides that meet there,
        // from the middle of each out to its end. A gradient is followed all
        // the way. The end of a button is not: the walk stops at its rim, and
        // the corner is the colour the button had just before it, which is
        // what a box that pokes out past a rounded end ought to report.
        let towards_start = |side: &[Rgb], known: [Rgb; 2]| {
            followed(side[..side.len().div_ceil(2)].iter().rev().copied(), known)
        };
        let towards_end = |side: &[Rgb], known: [Rgb; 2]| {
            followed(side[side.len() / 2..].iter().copied(), known)
        };
        let mut corners = typical;
        for (i, corner) in corners.iter_mut().enumerate() {
            let known = [typical[i], background];
            let (along, down) = match i {
                0 => (towards_start(&top, known), towards_start(&left, known)),
                1 => (towards_end(&top, known), towards_start(&right, known)),
                2 => (towards_start(&bottom, known), towards_end(&left, known)),
                _ => (towards_end(&bottom, known), towards_end(&right, known)),
            };
            *corner = match (along, down) {
                // Two answers that agree are one answer. Two that do not:
                // one of the walks ended on something else, and the quarter's
                // own colour says which.
                (Some(a), Some(b)) if a.distance(b) < 20.0 => a.mean(b),
                (Some(a), Some(b)) => {
                    if a.distance(typical[i]) <= b.distance(typical[i]) {
                        a
                    } else {
                        b
                    }
                }
                (Some(only), None) | (None, Some(only)) => only,
                (None, None) => typical[i],
            };
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

    /// How far in from the left and the right end of a rectangle the writing
    /// in it really starts, in pixels, neither more than `limit`.
    ///
    /// A recogniser's box is usually tight. Now and then it is not: the
    /// rounded end of a button reads as a bracket and the box takes it in.
    /// Everything that follows from the box is then off by that much — where
    /// the translation starts, what the plate covers, which colours are read
    /// around it — and the end of the button is painted over.
    ///
    /// So the box is held against the picture. Columns at either end with
    /// nothing of the text's colour in them are not writing. They are only
    /// taken off when what is in them is bare surface or the end of whatever
    /// the text stands on: something that stands on the surface by itself —
    /// a letter in another colour, an icon — has surface on both sides of it,
    /// and may well be part of the line. And only so far: writing that does
    /// not turn up within `limit` means the colours were misjudged, and the
    /// box is left as the recogniser drew it.
    pub fn ink_inset(
        &self,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        colours: &TextColours,
        limit: f32,
    ) -> (f32, f32) {
        /// How unlike the surface a pixel may be and still be it.
        const SURFACE: f32 = 24.0;

        let (fw, fh) = (self.width() as i32, self.height() as i32);
        if fw < 2 || fh < 2 || w < 4.0 || h < 3.0 || colours.busy {
            return (0.0, 0.0);
        }
        // Too alike to be separated by which one a pixel is nearer to.
        if colours.text.distance(colours.background) < 48.0 {
            return (0.0, 0.0);
        }
        let x0 = (x.round() as i32).clamp(0, fw - 1);
        let x1 = ((x + w).round() as i32 - 1).clamp(0, fw - 1);
        let y0 = (y.round() as i32).clamp(0, fh - 1);
        let y1 = ((y + h).round() as i32 - 1).clamp(0, fh - 1);
        let limit = (limit.round() as i32).min((x1 - x0) / 3);
        if y1 <= y0 || limit < 1 {
            return (0.0, 0.0);
        }

        let to_surface = |c: Rgb| {
            colours
                .corners
                .iter()
                .map(|corner| c.distance(*corner))
                .fold(c.distance(colours.background), f32::min)
        };
        let at = |px: i32, py: i32| self.pixel(px as usize, py as usize);
        let is_ink = |px: i32, py: i32| {
            let c = at(px, py);
            c.distance(colours.text) < to_surface(c)
        };
        let is_surface = |px: i32, py: i32| to_surface(at(px, py)) < SURFACE;

        // One end: `column(d)` is the column `d` in from it.
        let inset = |column: &dyn Fn(i32) -> i32| -> f32 {
            let first = (0..=limit).find(|d| (y0..=y1).any(|py| is_ink(column(*d), py)));
            let Some(blank) = first.filter(|blank| *blank > 0) else {
                return 0.0;
            };
            // Anything in the blank columns with surface on both sides of
            // it stands there by itself.
            let mut standing = 0;
            for py in y0..=y1 {
                let (mut on_surface, mut run) = (false, 0);
                for d in 0..blank {
                    if is_surface(column(d), py) {
                        if on_surface {
                            standing += run;
                        }
                        on_surface = true;
                        run = 0;
                    } else {
                        run += 1;
                    }
                }
            }
            let area = blank * (y1 - y0 + 1);
            if standing >= (area / 50).max(4) {
                0.0
            } else {
                blank as f32
            }
        };
        (inset(&|d| x0 + d), inset(&|d| x1 - d))
    }

    /// How far past each side of a rectangle the writing in it reaches, in
    /// pixels: left, top, right, bottom, none of them more than `reach`.
    ///
    /// A recogniser boxes the letters. Game type is rarely just letters: it
    /// has an outline, a drop shadow, a glow, and those run a few pixels past
    /// the box on every side. A plate the size of the box leaves them behind
    /// as a row of dark ticks around the translation.
    ///
    /// Each side is walked outwards for as long as it keeps finding something
    /// that is not surface. What matters is how the walk ends: if the surface
    /// comes back within reach, what was crossed belonged to the writing, and
    /// that is the answer. If it never does, this is the edge of the button
    /// or the next thing along — not something to paint over — and the answer
    /// for that side is nought.
    ///
    /// The rectangle covers whole pixels, from `x` up to but not including
    /// `x + w`: the first column looked at on the right is the first one it
    /// leaves showing.
    pub fn overhang(
        &self,
        rect: [f32; 4],
        colours: &TextColours,
        sampled: [f32; 4],
        reach: f32,
    ) -> [f32; 4] {
        const ALIKE: f32 = 40.0;

        let (fw, fh) = (self.width() as i32, self.height() as i32);
        let [x, y, w, h] = rect;
        // The first and the last column the rectangle covers, and row.
        let x0 = (x.round() as i32).clamp(0, fw.max(1) - 1);
        let y0 = (y.round() as i32).clamp(0, fh.max(1) - 1);
        let x1 = ((x + w).round() as i32 - 1).clamp(0, fw.max(1) - 1);
        let y1 = ((y + h).round() as i32 - 1).clamp(0, fh.max(1) - 1);
        let reach = reach.round() as i32;
        if fw < 4 || fh < 4 || x1 <= x0 || y1 <= y0 || reach < 1 {
            return [0.0; 4];
        }

        let [sx, sy, sw, sh] = sampled;
        let clean = |px: i32, py: i32| {
            let expected = blend(
                colours.corners,
                (px as f32 - sx) / sw.max(1.0),
                (py as f32 - sy) / sh.max(1.0),
            );
            self.pixel(px as usize, py as usize).distance(expected) < ALIKE
        };
        // One side: `line(d)` says whether the line `d` pixels out is all
        // surface, or `None` once it is off the frame.
        let walk = |line: &dyn Fn(i32) -> Option<bool>| -> f32 {
            let (mut last_dirty, mut clean_run) = (0, 0);
            for d in 1..=reach + 3 {
                match line(d) {
                    Some(true) => {
                        clean_run += 1;
                        // Three, not one: a drop shadow stands a pixel or
                        // two clear of the letter that casts it.
                        if clean_run == 3 {
                            return last_dirty as f32;
                        }
                    }
                    Some(false) if d <= reach => {
                        last_dirty = d;
                        clean_run = 0;
                    }
                    // Still not surface at the end of the reach, or the edge
                    // of the picture: not something that ends.
                    _ => return 0.0,
                }
            }
            0.0
        };

        // Looked for along the text's own rows and columns, not along the
        // whole of the rectangle: that may be taller than the button it is
        // on, and then every column would cross the rim and nothing would
        // ever look clean.
        let within = |from: f32, to: f32, limit: i32| {
            let a = (from.round() as i32).clamp(0, limit - 1);
            let b = (to.round() as i32).clamp(a, limit - 1);
            a..=b
        };
        let (rows, columns) = (within(sy, sy + sh, fh), within(sx, sx + sw, fw));
        let column = |px: i32| {
            (0..fw)
                .contains(&px)
                .then(|| rows.clone().all(|py| clean(px, py)))
        };
        let row = |py: i32| {
            (0..fh)
                .contains(&py)
                .then(|| columns.clone().all(|px| clean(px, py)))
        };
        [
            walk(&|d| column(x0 - d)),
            walk(&|d| row(y0 - d)),
            walk(&|d| column(x1 + d)),
            walk(&|d| row(y1 + d)),
        ]
    }

    /// The surface under a rectangle with whatever is written on it taken
    /// off: an image the size of the rectangle, to paint over it.
    ///
    /// A plate of one colour — or of four, blended — is right for a flat
    /// panel and a plain gradient and wrong for nearly everything a game
    /// draws: a button is lit from above, a dialogue box has a sheen, a panel
    /// has a texture. On those the plate shows as a patch. But the surface is
    /// right there on either side of the text, so each row of the image is
    /// simply the colour found just left of the rectangle run across to the
    /// colour found just right of it. Shading that runs down the surface is
    /// reproduced exactly; shading that runs across it, closely.
    ///
    /// `sampled` is the rectangle `colours` was read around; it is what says
    /// which colours on either side are surface at all. A row whose
    /// neighbours are something else — the end of a button, the next
    /// column's text — borrows from the nearest row that fared better.
    /// `None` when too few rows have surface beside them to go by.
    pub fn healed(
        &self,
        rect: [f32; 4],
        colours: &TextColours,
        sampled: [f32; 4],
    ) -> Option<egui::ColorImage> {
        /// How far outside the rectangle to look: past any antialiasing.
        const OUT: i32 = 2;
        /// How unlike the expected surface a pixel may be and still be taken
        /// for it outright.
        const ALIKE: f32 = 44.0;
        /// How much a surface may change from one row to the next. Shading
        /// drifts by a level or two a row; an edge jumps.
        const DRIFT: f32 = 12.0;
        /// How far off a row may be and still be a speck on the surface
        /// rather than the end of it, and how many such rows in a row.
        const SPECK: f32 = 30.0;
        const SPECKS: usize = 2;

        let (fw, fh) = (self.width() as i32, self.height() as i32);
        let [x, y, w, h] = rect;
        let x0 = (x.round() as i32).clamp(0, fw.max(1) - 1);
        let y0 = (y.round() as i32).clamp(0, fh.max(1) - 1);
        let x1 = ((x + w).round() as i32).clamp(0, fw.max(1) - 1);
        let y1 = ((y + h).round() as i32).clamp(0, fh.max(1) - 1);
        if fw < 4 || fh < 4 || x1 - x0 < 2 || y1 - y0 < 2 {
            return None;
        }
        let (width, height) = ((x1 - x0) as usize, (y1 - y0) as usize);

        let [sx, sy, sw, sh] = sampled;
        let expected = |px: i32, py: i32| {
            blend(
                colours.corners,
                (px as f32 - sx) / sw.max(1.0),
                (py as f32 - sy) / sh.max(1.0),
            )
        };

        // One side of the rectangle, top to bottom: the surface where there
        // is surface, nothing where there is something else.
        //
        // A button is darker towards its rim than in its middle, by more than
        // any one tolerance would allow without also letting in the rim. So
        // the surface is recognised once, in the row nearest the middle that
        // looks like what was sampled, and then *followed* up and down for as
        // long as each row is only a little different from the one before.
        // Shading is followed all the way to the edge; the edge itself is a
        // jump, and ends it.
        let side = |px: i32| -> Vec<Option<Rgb>> {
            let mut column = vec![None; height];
            if px < 0 || px >= fw {
                return column;
            }
            let at = |row: usize| self.pixel(px as usize, y0 as usize + row);
            let middle = height / 2;
            let seed = (0..=middle)
                .flat_map(|d| [middle.saturating_sub(d), (middle + d).min(height - 1)])
                .find(|row| at(*row).distance(expected(px, y0 + *row as i32)) < ALIKE);
            let Some(seed) = seed else {
                return column;
            };
            column[seed] = Some(at(seed));
            // One way from the seed, then the other. A row or two that is
            // off by a little — the ringing a compressed picture has around
            // its letters, a speck — is stepped over and given the colour
            // the surface had just before it. A row that is off by a lot is
            // an edge: the rim of the button, a border. Nothing is carried
            // across one of those, however thin it is.
            let mut follow = |rows: &mut dyn Iterator<Item = usize>| {
                let mut previous = at(seed);
                let mut stepped_over: Vec<usize> = Vec::new();
                for row in rows {
                    let c = at(row);
                    let off = c.distance(previous);
                    if off < DRIFT {
                        for speck in stepped_over.drain(..) {
                            column[speck] = Some(previous);
                        }
                        column[row] = Some(c);
                        previous = c;
                    } else if off < SPECK && stepped_over.len() < SPECKS {
                        stepped_over.push(row);
                    } else {
                        break;
                    }
                }
            };
            follow(&mut (0..seed).rev());
            follow(&mut (seed + 1..height));

            // Whatever grain is left would run across the plate as a faint
            // stripe, one for every row that sampled a little light or dark.
            // The middle one of each three rows takes it out and leaves
            // shading, which changes slowly, exactly as it was.
            let rough = column.clone();
            for row in 1..height.saturating_sub(1) {
                if let (Some(a), Some(b), Some(c)) = (rough[row - 1], rough[row], rough[row + 1]) {
                    let middle = |of: fn(Rgb) -> u8| {
                        let mut three = [of(a), of(b), of(c)];
                        three.sort_unstable();
                        three[1]
                    };
                    column[row] = Some(Rgb(middle(|c| c.0), middle(|c| c.1), middle(|c| c.2)));
                }
            }
            column
        };

        // `x1` is the first column past the rectangle, so both are the same
        // distance out.
        let (left_side, right_side) = (side(x0 - OUT), side(x1 - 1 + OUT));

        // What each row has beside it. One good side stands in for both.
        let mut rows: Vec<Option<(Rgb, Rgb)>> = left_side
            .iter()
            .zip(&right_side)
            .map(|sides| match sides {
                (Some(left), Some(right)) => Some((*left, *right)),
                (Some(only), None) | (None, Some(only)) => Some((*only, *only)),
                (None, None) => None,
            })
            .collect();
        // Less than this and it is not a surface with writing on it but
        // writing with a little surface showing: nothing to mend with.
        let known = rows.iter().filter(|row| row.is_some()).count();
        if known < 4 || known * 5 < rows.len() * 2 {
            return None;
        }
        // Rows above the first with surface beside it, and below the last,
        // are off the surface altogether: the plate is taller than the button
        // it sits on. They are left clear, so that what is there stays there.
        let first = rows.iter().position(Option::is_some)?;
        let last = rows.iter().rposition(Option::is_some)?;
        // The rest borrow from the nearest row above.
        let mut above = None;
        for row in rows[first..=last].iter_mut() {
            *row = row.or(above);
            above = *row;
        }

        /// How far in from an edge a row is searched for where its surface
        /// begins, when there is none beside it.
        const INSET: usize = 12;
        /// How like the fill a pixel has to be to count as that surface.
        const SAME: f32 = 26.0;

        let clear = egui::Color32::TRANSPARENT;
        let mut pixels = vec![clear; width * height];
        for (index, row) in rows.iter().enumerate().take(last + 1).skip(first) {
            let Some((left, right)) = *row else { continue };
            let fill = |i: usize| {
                let t = i as f32 / (width - 1).max(1) as f32;
                let mix = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * t).round() as u8;
                Rgb(mix(left.0, right.0), mix(left.1, right.1), mix(left.2, right.2))
            };
            let there = |i: usize| self.pixel(x0 as usize + i, y0 as usize + index);

            // A side with no surface beside it is the rounded end of a
            // button, or its rim: the row is painted only from where the
            // surface actually starts, and the corner keeps its shape.
            //
            // "No surface beside it" is asked of the pixel itself as well as
            // of the walk down the side. The walk gives up on a row for
            // reasons that have nothing to do with a button ending there —
            // and a row that does have its surface beside it has nothing at
            // its end to keep: what is unlike the fill there is the last
            // letter, which a box a pixel short has left sticking out.
            let beside = |column: i32, fill: Rgb| {
                (0..fw).contains(&column)
                    && self
                        .pixel(column as usize, y0 as usize + index)
                        .distance(fill)
                        < SAME
            };
            let reach = INSET.min(width / 3);
            let mut from = 0;
            if left_side[index].is_none() && !beside(x0 - OUT, fill(0)) {
                while from < reach && there(from).distance(fill(from)) >= SAME {
                    from += 1;
                }
            }
            let mut to = width;
            if right_side[index].is_none() && !beside(x1 - 1 + OUT, fill(width - 1)) {
                while to > width - reach && there(to - 1).distance(fill(to - 1)) >= SAME {
                    to -= 1;
                }
            }
            for i in from..to {
                let c = fill(i);
                pixels[index * width + i] = egui::Color32::from_rgb(c.0, c.1, c.2);
            }
        }
        Some(egui::ColorImage {
            size: [width, height],
            pixels,
        })
    }

    /// Where the letters are inside a rectangle of this frame: one bit for
    /// every other pixel, set where the pixel is the text's colour.
    ///
    /// For telling whether a piece of text is *still there*. A translation
    /// laid over a live picture is only right for as long as the words under
    /// it are the ones that were translated; the moment the game moves on to
    /// the next line, the old translation is covering something it does not
    /// say. Taking the print again from a later frame and asking
    /// [`InkPrint::still_there`] answers that without reading anything.
    ///
    /// Fine-grained on purpose. Two different lines of dialogue in the same
    /// font fill the same band of the screen, and anything coarser than a
    /// stroke cannot tell them apart.
    pub fn ink_print(&self, x: f32, y: f32, w: f32, h: f32, colours: &TextColours) -> InkPrint {
        /// Every other pixel each way: a quarter of the work, and still finer
        /// than the thinnest stroke worth reading.
        const STEP: usize = 2;

        let (fw, fh) = (self.width() as i32, self.height() as i32);
        let x0 = (x.round() as i32).clamp(0, fw.max(1) - 1) as usize;
        let y0 = (y.round() as i32).clamp(0, fh.max(1) - 1) as usize;
        let x1 = ((x + w).round() as i32).clamp(0, fw.max(1) - 1) as usize;
        let y1 = ((y + h).round() as i32).clamp(0, fh.max(1) - 1) as usize;
        if fw < 2 || fh < 2 || x1 <= x0 || y1 <= y0 {
            return InkPrint::default();
        }

        // Near enough to the text's colour to be a letter, and nearer to it
        // than to what the text is written on.
        let apart = colours.text.distance(colours.background);
        let near = (apart * 0.45).clamp(18.0, 70.0);

        let mut bits = Vec::with_capacity(((x1 - x0) / STEP + 1) * ((y1 - y0) / STEP + 1) / 64 + 1);
        let (mut word, mut filled, mut set) = (0u64, 0u32, 0u32);
        for py in (y0..=y1).step_by(STEP) {
            for px in (x0..=x1).step_by(STEP) {
                let c = self.pixel(px, py);
                let to_text = c.distance(colours.text);
                if to_text < near && to_text < c.distance(colours.background) {
                    word |= 1 << filled;
                    set += 1;
                }
                filled += 1;
                if filled == 64 {
                    bits.push(word);
                    (word, filled) = (0, 0);
                }
            }
        }
        if filled > 0 {
            bits.push(word);
        }
        InkPrint { bits, set }
    }

    /// Whether the letters `print` was taken from are still in this
    /// rectangle — in the colours they were in then, or in others.
    ///
    /// The colours they were in are tried first, and that is what keeps a
    /// subtitle standing while a scene goes by behind it. But text changes
    /// colour without changing: a button pulses, a line fades in, a
    /// highlight moves down a menu. Held to its old colours, such text reads
    /// as gone twice a second, and a translation over it blinks. So when the
    /// old colours fail, the colours are read off the picture again and the
    /// letters are looked for by their shape: the same strokes in the same
    /// places, whatever they are drawn in now.
    pub fn still_shows(
        &self,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        colours: &TextColours,
        print: &InkPrint,
    ) -> bool {
        if print.still_there(&self.ink_print(x, y, w, h, colours)) {
            return true;
        }
        let now = self.text_colours(x, y, w, h);
        print.still_there(&self.ink_print(x, y, w, h, &now))
    }

    /// A coarse fingerprint of the image, for telling "the dialogue changed"
    /// from "a particle drifted past".
    ///
    /// Coarse, but not coarser than a line of type. The picture is cut into
    /// cells about [`Signature::CELL`] pixels across — however many that
    /// takes, and never fewer than 32 by 16. A fixed 32 by 16 over a whole
    /// display made cells of eighty pixels by ninety, and a subtitle coming
    /// up in one of those moved its average brightness by less than the grain
    /// of the picture does.
    pub fn signature(&self) -> Signature {
        let (w, h) = (self.width(), self.height());
        let cols = (w / Signature::CELL).clamp(32, 160);
        let rows = (h / Signature::CELL).clamp(16, 100);
        let mut cells = vec![0u8; cols * rows];
        if w == 0 || h == 0 {
            return Signature { cells };
        }

        for cy in 0..rows {
            for cx in 0..cols {
                let x0 = cx * w / cols;
                let x1 = ((cx + 1) * w / cols).max(x0 + 1).min(w);
                let y0 = cy * h / rows;
                let y1 = ((cy + 1) * h / rows).max(y0 + 1).min(h);

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
                cells[cy * cols + cx] = if n == 0 { 0 } else { (sum / n as f32) as u8 };
            }
        }
        Signature { cells }
    }
}

/// See [`Frame::ink_print`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InkPrint {
    bits: Vec<u64>,
    /// How many of them are set.
    set: u32,
}

impl InkPrint {
    /// Whether the letters this print was taken from are still where they
    /// were, going by a print of the same rectangle taken later.
    ///
    /// Asked one way round only: how much of *this* ink is still ink. What
    /// has been added does not count — a bright cloud drifting behind a
    /// subtitle adds plenty and changes nothing about the subtitle. Text that
    /// has been replaced keeps about a third of its ink by coincidence, text
    /// that has gone keeps none, and text that is still there keeps nearly
    /// all of it.
    pub fn still_there(&self, later: &InkPrint) -> bool {
        // Too little to go by: a full stop, a hyphen. Say yes and let the
        // next read settle it.
        if self.set < 12 {
            return true;
        }
        if self.bits.len() != later.bits.len() {
            return false;
        }
        let kept: u32 = self
            .bits
            .iter()
            .zip(&later.bits)
            .map(|(then, now)| (then & now).count_ones())
            .sum();
        kept as f32 >= self.set as f32 * 0.72
    }
}

/// See [`Frame::signature`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Signature {
    cells: Vec<u8>,
}

impl Signature {
    /// How many pixels across a cell is, where the picture is large enough
    /// to be cut that finely: about the height of a line of interface type.
    const CELL: usize = 28;

    /// How many cells look different. Every one of them, when the two were
    /// not taken of the same area and cannot be compared at all.
    ///
    /// A count, not a share: what has to be noticed is a line of text, and a
    /// line of text is the same handful of cells on a whole display as it is
    /// in a rectangle drawn round it.
    pub fn cells_changed(&self, other: &Signature) -> usize {
        if self.cells.len() != other.cells.len() || self.cells.is_empty() {
            return self.cells.len().max(other.cells.len()).max(1);
        }
        // A cell counts as changed past a brightness step no compression
        // artefact or video grain produces on its own.
        const STEP: i16 = 14;
        self.cells
            .iter()
            .zip(&other.cells)
            .filter(|(a, b)| (**a as i16 - **b as i16).abs() > STEP)
            .count()
    }
}

/// The colour a surface has where a walk along it ends.
///
/// `known` are colours the surface is known to have somewhere. The walk is
/// not on the surface until it meets one of them; from then on each sample
/// that is only a little unlike the last one followed is taken for more of
/// the same surface, and anything else is stepped over — a letter of the line
/// above, a rule. Shading drifts a level or two a sample and is followed to
/// the end. The rim of a button is a jump, with nothing on the far side that
/// is like the near one, so the walk stays where the button stopped.
///
/// `None` when the walk never met the surface at all.
fn followed(walk: impl Iterator<Item = Rgb>, known: [Rgb; 2]) -> Option<Rgb> {
    /// How unlike a known colour a sample may be and still be taken for the
    /// surface outright.
    const NEAR: f32 = 16.0;
    /// How much a surface may change from one sample to the next.
    const STEP: f32 = 10.0;
    /// How many of the last samples the answer is taken from. The middle one
    /// of them, not the mean: where the walk ends on the soft edge of a
    /// letter's outline, the last sample or two are that edge, shading by
    /// small enough steps to have been followed.
    const LAST: usize = 5;

    let mut on: Option<Rgb> = None;
    let mut recent = [Rgb(0, 0, 0); LAST];
    let mut count = 0usize;
    for c in walk {
        let follows = match on {
            Some(last) => c.distance(last) < STEP,
            None => known.iter().any(|k| c.distance(*k) < NEAR),
        };
        if follows {
            on = Some(c);
            recent[count % LAST] = c;
            count += 1;
        }
    }
    let kept = &mut recent[..count.min(LAST)];
    if kept.is_empty() {
        return None;
    }
    let mut middle = |of: fn(&Rgb) -> u8| {
        kept.sort_by_key(of);
        of(&kept[kept.len() / 2])
    };
    Some(Rgb(middle(|c| c.0), middle(|c| c.1), middle(|c| c.2)))
}

/// The colour a surface has at a point, given its four corners — top-left,
/// top-right, bottom-left, bottom-right — and how far across and down the
/// point is, where 0 and 1 are the edges the corners were read at.
///
/// Carries on past those edges at the rate the surface was changing, but only
/// so far: beyond a rectangle's width again a guess is just a guess.
pub fn blend(corners: [Rgb; 4], across: f32, down: f32) -> Rgb {
    let (u, v) = (across.clamp(-1.0, 2.0), down.clamp(-1.0, 2.0));
    let channel = |of: fn(Rgb) -> u8| {
        let [tl, tr, bl, br] = corners.map(|c| of(c) as f32);
        let top = tl + (tr - tl) * u;
        let bottom = bl + (br - bl) * u;
        (top + (bottom - top) * v).round().clamp(0.0, 255.0) as u8
    };
    Rgb(channel(|c| c.0), channel(|c| c.1), channel(|c| c.2))
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

    /// A button lit from above: lighter at the top, darker at the bottom,
    /// with "text" across the middle of it.
    fn lit_button() -> Frame {
        let (w, h) = (240, 60);
        let mut rgba = Vec::with_capacity((w * h * 4) as usize);
        for y in 0..h {
            for _ in 0..w {
                let shade = (120 - y) as u8;
                rgba.extend_from_slice(&[shade, shade / 2, shade, 255]);
            }
        }
        paint(&mut rgba, w, 60, 20, 120, 20, Rgb(255, 255, 255));
        frame(w, h, rgba)
    }

    #[test]
    fn an_outline_past_the_box_is_measured() {
        // White lettering with a dark outline three pixels wide all round,
        // and a box that hugs the white.
        let (w, h) = (240, 80);
        let mut rgba = solid(w, h, Rgb(90, 60, 88));
        paint(&mut rgba, w, 57, 27, 126, 26, Rgb(10, 10, 10));
        paint(&mut rgba, w, 60, 30, 120, 20, Rgb(255, 255, 255));
        let f = frame(w, h, rgba);
        let colours = TextColours::flat(Rgb(90, 60, 88), Rgb(255, 255, 255));
        let over = f.overhang([60.0, 30.0, 120.0, 20.0], &colours, [57.0, 27.0, 126.0, 26.0], 12.0);
        assert_eq!(over, [3.0, 3.0, 3.0, 3.0]);
    }

    /// A dark page with a button on it whose left end is rounded off, and
    /// white "lettering" with a black outline starting at `text_from`.
    fn pill(text_from: i32) -> Frame {
        let (w, h) = (400, 100);
        let page = Rgb(15, 8, 7);
        let button = Rgb(78, 52, 71);
        let mut rgba = solid(w, h, page);
        // Rows 20..80, the end a half circle of radius 30 about (60, 50).
        for y in 20..80 {
            let dy = (y - 50) as f32 + 0.5;
            let starts = 60 - (900.0 - dy * dy).max(0.0).sqrt() as i32;
            paint(&mut rgba, w, starts, y, 380 - starts, 1, button);
        }
        paint(&mut rgba, w, text_from - 3, 33, 206, 34, Rgb(0, 0, 0));
        paint(&mut rgba, w, text_from, 36, 200, 28, Rgb(255, 255, 255));
        frame(w, h, rgba)
    }

    #[test]
    fn a_box_that_pokes_out_past_a_rounded_end_still_reads_the_button() {
        // The box starts twenty pixels left of the writing: its left edge is
        // off the button altogether and its corners are on the page.
        let f = pill(56);
        let c = f.text_colours(34.0, 30.0, 224.0, 40.0);
        assert_eq!(c.background, Rgb(78, 52, 71));
        for corner in c.corners {
            assert!(corner.distance(Rgb(78, 52, 71)) < 6.0, "{c:?}");
        }
        assert!(c.is_flat(), "{c:?}");
    }

    #[test]
    fn a_box_that_takes_in_the_end_of_a_button_is_brought_back_to_the_writing() {
        let f = pill(56);
        let (x, y, w, h) = (34.0, 30.0, 224.0, 40.0);
        let c = f.text_colours(x, y, w, h);
        // Twenty-two columns of page and button before the first white one;
        // the right end already sits on the last.
        assert_eq!(f.ink_inset(x, y, w, h, &c, 40.0), (22.0, 2.0));
    }

    #[test]
    fn a_box_that_hugs_its_writing_is_left_alone() {
        let f = pill(56);
        let c = f.text_colours(56.0, 36.0, 200.0, 28.0);
        assert_eq!(f.ink_inset(56.0, 36.0, 200.0, 28.0, &c, 40.0), (0.0, 0.0));
    }

    #[test]
    fn a_letter_in_another_colour_is_not_taken_for_the_end_of_a_button() {
        // White writing on a dark panel, with a red first letter: nearer the
        // panel than the white, so not "ink" — but it stands on the panel by
        // itself, with panel on both sides of it.
        let (w, h) = (300, 60);
        let mut rgba = solid(w, h, Rgb(20, 20, 24));
        paint(&mut rgba, w, 40, 18, 6, 24, Rgb(170, 30, 40));
        paint(&mut rgba, w, 60, 18, 180, 24, Rgb(255, 255, 255));
        let f = frame(w, h, rgba);
        let c = f.text_colours(36.0, 16.0, 208.0, 28.0);
        assert_eq!(c.text, Rgb(255, 255, 255));
        assert_eq!(f.ink_inset(36.0, 16.0, 208.0, 28.0, &c, 40.0).0, 0.0);
    }

    #[test]
    fn writing_that_does_not_turn_up_leaves_the_box_as_it_was() {
        // Nothing white for the first sixty columns of the box, and only
        // thirty are allowed to go.
        let (w, h) = (300, 60);
        let mut rgba = solid(w, h, Rgb(20, 20, 24));
        paint(&mut rgba, w, 100, 18, 150, 24, Rgb(255, 255, 255));
        let f = frame(w, h, rgba);
        let c = f.text_colours(40.0, 16.0, 212.0, 28.0);
        assert_eq!(f.ink_inset(40.0, 16.0, 212.0, 28.0, &c, 30.0).0, 0.0);
    }

    #[test]
    fn the_end_of_the_button_is_not_an_overhang() {
        // The surface stops six pixels right of the box and does not resume.
        let (w, h) = (240, 80);
        let mut rgba = solid(w, h, Rgb(12, 8, 12));
        paint(&mut rgba, w, 20, 20, 166, 40, Rgb(90, 60, 88));
        paint(&mut rgba, w, 60, 30, 120, 20, Rgb(255, 255, 255));
        let f = frame(w, h, rgba);
        let colours = TextColours::flat(Rgb(90, 60, 88), Rgb(255, 255, 255));
        let over = f.overhang([60.0, 30.0, 120.0, 20.0], &colours, [57.0, 27.0, 126.0, 26.0], 12.0);
        assert_eq!(over[2], 0.0, "{over:?}");
        // The other sides have surface all round and nothing sticking out.
        assert_eq!([over[0], over[1], over[3]], [0.0, 0.0, 0.0]);
    }

    #[test]
    fn healing_takes_the_writing_off_and_keeps_the_shading() {
        let f = lit_button();
        let colours = f.text_colours(60.0, 20.0, 120.0, 20.0);
        let healed = f
            .healed([56.0, 16.0, 128.0, 28.0], &colours, [57.0, 17.0, 126.0, 26.0])
            .expect("a clean surface either side");
        assert_eq!(healed.size, [128, 28]);
        for (row, y) in (16..44).enumerate() {
            let shade = (120 - y) as u8;
            let want = egui::Color32::from_rgb(shade, shade / 2, shade);
            // Every row its own shade, all the way across, and no white left.
            assert_eq!(healed.pixels[row * 128], want, "row {row}, left");
            assert_eq!(healed.pixels[row * 128 + 64], want, "row {row}, middle");
            assert_eq!(healed.pixels[row * 128 + 127], want, "row {row}, right");
        }
    }

    #[test]
    fn a_row_with_something_else_beside_it_borrows_from_a_neighbour() {
        // The same button, with another word hard against the right edge of
        // the rectangle on a few rows.
        let (w, h) = (240, 60);
        let mut rgba = f_rgba(&lit_button());
        paint(&mut rgba, w, 184, 24, 30, 6, Rgb(255, 255, 255));
        let f = frame(w, h, rgba);
        let colours = f.text_colours(60.0, 20.0, 120.0, 20.0);
        let healed = f
            .healed([56.0, 16.0, 128.0, 28.0], &colours, [57.0, 17.0, 126.0, 26.0])
            .expect("most rows are clean");
        // Row 10 is y = 26, where the other word is. It must not have been
        // smeared across the plate.
        let across = healed.pixels[10 * 128 + 120];
        assert!(across.r() < 130, "the neighbour's white ran in: {across:?}");
    }

    #[test]
    fn the_rounded_end_of_a_button_keeps_its_shape() {
        // A button whose left end is cut off at a slant, standing in for a
        // rounded one: the lower the row, the further right the button
        // starts. The rectangle to heal pokes out past that end.
        let (w, h) = (240, 60);
        let outside = Rgb(10, 8, 10);
        let button = Rgb(90, 60, 88);
        let mut rgba = solid(w, h, outside);
        for y in 10..50 {
            let starts = 40 + (y - 10) / 4;
            paint(&mut rgba, w, starts, y, 190 - starts, 1, button);
        }
        paint(&mut rgba, w, 70, 22, 80, 16, Rgb(255, 255, 255));
        let f = frame(w, h, rgba);
        let colours = TextColours::flat(button, Rgb(255, 255, 255));
        let healed = f
            .healed([44.0, 16.0, 120.0, 28.0], &colours, [67.0, 19.0, 86.0, 22.0])
            .expect("surface on the right of every row");

        let at = |x: i32, y: i32| healed.pixels[((y - 16) * 120 + (x - 44)) as usize];
        // Row 40: the button starts at x = 47 there. Left of that is not
        // button and must not be painted as one.
        assert_eq!(at(44, 40), egui::Color32::TRANSPARENT);
        assert_eq!(at(46, 40), egui::Color32::TRANSPARENT);
        assert_eq!(at(60, 40), egui::Color32::from_rgb(90, 60, 88));
        // And the writing is gone all the same.
        assert_eq!(at(100, 30), egui::Color32::from_rgb(90, 60, 88));
    }

    #[test]
    fn rows_off_the_surface_altogether_are_left_as_they_are() {
        // A button twenty rows tall and a rectangle thirty-six rows tall
        // around the text on it.
        let (w, h) = (240, 60);
        let mut rgba = solid(w, h, Rgb(10, 8, 10));
        paint(&mut rgba, w, 20, 20, 200, 20, Rgb(90, 60, 88));
        paint(&mut rgba, w, 70, 24, 80, 12, Rgb(255, 255, 255));
        let f = frame(w, h, rgba);
        let colours = TextColours::flat(Rgb(90, 60, 88), Rgb(255, 255, 255));
        let healed = f
            .healed([60.0, 12.0, 100.0, 36.0], &colours, [67.0, 21.0, 86.0, 18.0])
            .expect("the button's rows have surface beside them");
        assert_eq!(healed.pixels[2 * 100 + 50], egui::Color32::TRANSPARENT, "above the button");
        assert_eq!(healed.pixels[33 * 100 + 50], egui::Color32::TRANSPARENT, "below the button");
        assert_eq!(healed.pixels[18 * 100 + 50], egui::Color32::from_rgb(90, 60, 88));
    }

    #[test]
    fn a_letter_left_sticking_out_of_a_short_box_is_painted_over() {
        // The "lettering" runs right up to the last column of the rectangle,
        // and the surface just past it has a couple of rows of the ringing a
        // compressed picture carries around its letters.
        let (w, h) = (240, 60);
        let surface = Rgb(20, 24, 40);
        let mut rgba = solid(w, h, surface);
        paint(&mut rgba, w, 60, 24, 121, 12, Rgb(255, 255, 255));
        paint(&mut rgba, w, 181, 26, 4, 2, Rgb(44, 44, 58));
        let f = frame(w, h, rgba);
        let colours = TextColours::flat(surface, Rgb(255, 255, 255));
        let healed = f
            .healed([50.0, 16.0, 131.0, 28.0], &colours, [57.0, 21.0, 127.0, 18.0])
            .expect("surface on both sides");

        // Every row of the lettering is gone, the noisy ones included, all
        // the way to the edge.
        let painted = egui::Color32::from_rgb(surface.0, surface.1, surface.2);
        for row in 24..36 {
            for column in [60, 120, 176, 180] {
                let at = healed.pixels[((row - 16) * 131 + (column - 50)) as usize];
                assert_eq!(at, painted, "row {row}, column {column}");
            }
        }
    }

    #[test]
    fn a_border_is_not_a_speck_to_be_stepped_over() {
        // A button with a hairline round it, on a page of the button's own
        // colour. The rectangle is taller than the button. What is past the
        // hairline is not this surface, however alike it looks, and the
        // hairline itself is certainly not to be painted out.
        let (w, h) = (240, 60);
        let fill = Rgb(90, 60, 88);
        let mut rgba = solid(w, h, fill);
        paint(&mut rgba, w, 20, 19, 200, 1, Rgb(210, 210, 210));
        paint(&mut rgba, w, 20, 40, 200, 1, Rgb(210, 210, 210));
        paint(&mut rgba, w, 70, 24, 80, 12, Rgb(255, 255, 255));
        let f = frame(w, h, rgba);
        let colours = TextColours::flat(fill, Rgb(255, 255, 255));
        let healed = f
            .healed([60.0, 12.0, 100.0, 36.0], &colours, [67.0, 21.0, 86.0, 18.0])
            .expect("the button's rows have surface beside them");
        let at = |row: i32| healed.pixels[((row - 12) * 100 + 50) as usize];
        assert_eq!(at(19), egui::Color32::TRANSPARENT, "the hairline above");
        assert_eq!(at(40), egui::Color32::TRANSPARENT, "the hairline below");
        assert_eq!(at(14), egui::Color32::TRANSPARENT, "past the hairline");
        assert_eq!(at(30), egui::Color32::from_rgb(90, 60, 88));
    }

    #[test]
    fn nothing_to_go_by_means_no_healing() {
        // Scenery on every side: no row has surface beside it.
        let f = frame(300, 120, scenery(300, 120));
        let colours = f.text_colours(50.0, 40.0, 200.0, 26.0);
        assert!(f
            .healed([46.0, 36.0, 208.0, 34.0], &colours, [47.0, 37.0, 206.0, 32.0])
            .is_none());
    }

    #[test]
    fn a_surface_is_carried_on_past_its_corners_but_not_for_ever() {
        let corners = [Rgb(100, 100, 100), Rgb(200, 200, 200), Rgb(100, 100, 100), Rgb(200, 200, 200)];
        assert_eq!(blend(corners, 0.5, 0.5), Rgb(150, 150, 150));
        assert_eq!(blend(corners, -0.5, 0.0), Rgb(50, 50, 50));
        assert_eq!(blend(corners, 9.0, 0.0), blend(corners, 2.0, 0.0));
    }

    fn f_rgba(f: &Frame) -> Vec<u8> {
        f.rgba().to_vec()
    }

    /// A line of "letters": upright strokes at the given places.
    fn strokes(rgba: &mut [u8], w: i32, at: &[i32], ink: Rgb) {
        for x in at {
            paint(rgba, w, *x, 20, 3, 14, ink);
        }
    }

    const LINE_A: [i32; 10] = [20, 27, 41, 48, 60, 74, 81, 95, 110, 124];
    const LINE_B: [i32; 10] = [23, 34, 44, 55, 66, 70, 88, 101, 115, 131];

    fn print_of(rgba: Vec<u8>) -> InkPrint {
        frame(200, 60, rgba).ink_print(16.0, 16.0, 130.0, 24.0, &TextColours::flat(PAGE, INK))
    }

    #[test]
    fn text_that_has_not_changed_is_still_there() {
        let mut rgba = solid(200, 60, PAGE);
        strokes(&mut rgba, 200, &LINE_A, INK);
        let then = print_of(rgba.clone());
        assert!(then.still_there(&print_of(rgba)));
    }

    #[test]
    fn another_line_in_the_same_place_is_not_the_same_text() {
        let mut first = solid(200, 60, PAGE);
        strokes(&mut first, 200, &LINE_A, INK);
        let mut second = solid(200, 60, PAGE);
        strokes(&mut second, 200, &LINE_B, INK);
        assert!(!print_of(first).still_there(&print_of(second)));
    }

    #[test]
    fn text_that_has_gone_is_not_still_there() {
        let mut rgba = solid(200, 60, PAGE);
        strokes(&mut rgba, 200, &LINE_A, INK);
        assert!(!print_of(rgba).still_there(&print_of(solid(200, 60, PAGE))));
    }

    #[test]
    fn a_background_that_moved_behind_the_same_text_changes_nothing() {
        let mut then = solid(200, 60, PAGE);
        strokes(&mut then, 200, &LINE_A, INK);
        // The scene behind is now something else entirely, and something
        // bright has drifted in beside the words.
        let mut now = solid(200, 60, Rgb(70, 90, 60));
        paint(&mut now, 200, 100, 30, 40, 8, Rgb(250, 250, 250));
        strokes(&mut now, 200, &LINE_A, INK);
        assert!(print_of(then).still_there(&print_of(now)));
    }

    /// The same check as [`print_of`] makes, with the picture asked again
    /// what colours the text is in when the old ones do not answer.
    fn still_shows(then: Vec<u8>, now: Vec<u8>) -> bool {
        frame(200, 60, now).still_shows(
            16.0,
            16.0,
            130.0,
            24.0,
            &TextColours::flat(PAGE, INK),
            &print_of(then),
        )
    }

    #[test]
    fn text_that_has_only_changed_colour_is_still_there() {
        let mut then = solid(200, 60, PAGE);
        strokes(&mut then, 200, &LINE_A, INK);
        // A button at the dim end of its pulse: half way to its surface.
        let dim = INK.mean(PAGE);
        let mut now = solid(200, 60, PAGE);
        strokes(&mut now, 200, &LINE_A, dim);
        // In its old colours it reads as gone…
        assert!(!print_of(then.clone()).still_there(&print_of(now.clone())));
        // …and it is not.
        assert!(still_shows(then.clone(), now));

        // On a surface that changed with it.
        let lit = Rgb(120, 60, 70);
        let mut now = solid(200, 60, lit);
        strokes(&mut now, 200, &LINE_A, Rgb(255, 250, 240));
        assert!(still_shows(then, now));
    }

    #[test]
    fn other_text_in_other_colours_is_still_other_text() {
        let mut then = solid(200, 60, PAGE);
        strokes(&mut then, 200, &LINE_A, INK);
        let mut now = solid(200, 60, PAGE);
        strokes(&mut now, 200, &LINE_B, INK.mean(PAGE));
        assert!(!still_shows(then.clone(), now));
        // And no text is no text, whatever the surface has turned into.
        assert!(!still_shows(then.clone(), solid(200, 60, PAGE)));
        assert!(!still_shows(then, solid(200, 60, Rgb(120, 60, 70))));
    }

    #[test]
    fn a_speck_of_text_is_given_the_benefit_of_the_doubt() {
        let mut then = solid(200, 60, PAGE);
        paint(&mut then, 200, 40, 30, 3, 3, INK); // a full stop
        assert!(print_of(then).still_there(&print_of(solid(200, 60, PAGE))));
    }

    #[test]
    fn a_print_of_nothing_is_empty_and_does_not_panic() {
        let f = frame(200, 60, solid(200, 60, PAGE));
        let colours = TextColours::flat(PAGE, INK);
        assert_eq!(f.ink_print(500.0, 500.0, 40.0, 20.0, &colours), InkPrint::default());
        assert_eq!(f.ink_print(10.0, 10.0, 0.0, 0.0, &colours), InkPrint::default());
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
        assert_eq!(f.signature().cells_changed(&f.signature()), 0);
    }

    #[test]
    fn new_text_shows_up_as_a_difference() {
        let before = frame(320, 80, solid(320, 80, Rgb(20, 20, 30)));
        let mut rgba = solid(320, 80, Rgb(20, 20, 30));
        paint(&mut rgba, 320, 20, 20, 280, 40, Rgb(240, 240, 240));
        let after = frame(320, 80, rgba);

        let changed = before.signature().cells_changed(&after.signature());
        assert!(changed > 150, "a line of text only registered in {changed} cells of 512");
    }

    #[test]
    fn a_faint_flicker_does_not_register() {
        let a = frame(320, 80, solid(320, 80, Rgb(20, 20, 30)));
        let b = frame(320, 80, solid(320, 80, Rgb(24, 23, 33)));
        assert_eq!(a.signature().cells_changed(&b.signature()), 0);
    }

    #[test]
    fn a_line_of_small_type_registers_on_a_whole_display() {
        // 2560 by 1440 of dark picture, and one line of subtitle — twenty
        // pixels tall, six hundred long, a fifth of it ink — coming up.
        let (w, h) = (2560, 1440);
        let before = frame(w, h, solid(w, h, Rgb(30, 34, 40)));
        let mut rgba = solid(w, h, Rgb(30, 34, 40));
        for letter in 0..60 {
            paint(&mut rgba, w, 980 + letter * 10, 1200, 3, 20, Rgb(240, 240, 240));
        }
        let after = frame(w, h, rgba);

        let changed = before.signature().cells_changed(&after.signature());
        assert!(changed >= 8, "only {changed} cells noticed the subtitle");
        // And the picture it was compared with is itself, exactly.
        assert_eq!(before.signature().cells_changed(&before.signature()), 0);
    }

    #[test]
    fn a_small_area_is_still_cut_into_thirty_two_by_sixteen() {
        let f = frame(320, 80, solid(320, 80, Rgb(30, 34, 40)));
        assert_eq!(f.signature().cells.len(), 32 * 16);
    }

    #[test]
    fn signatures_of_different_sizes_count_as_fully_different() {
        let a = Signature { cells: vec![0; 4] };
        let b = Signature { cells: vec![0; 8] };
        assert_eq!(a.cells_changed(&b), 8);
    }
}
