//! A frame of a game made to look like a page.
//!
//! The recogniser built into Windows was made for pages: lettering on plain
//! paper. Shown a frame of a game as it is, it reads what stands on a quiet
//! panel and loses what does not — on one main menu it found two of six
//! buttons, because a figure stood beside them. What it needs is the same
//! frame with everything that is not a stroke of lettering taken out.
//!
//! Lettering is thin. Whatever else is in a picture — a panel, a face, the
//! sky — is wide. So the picture is compared with itself after everything
//! narrower than a letter's stroke has been pressed out of it (an *opening*,
//! for light strokes; a *closing*, for dark ones), and what differs is the
//! strokes: the top-hat transform, as old as image processing. The result is
//! drawn as dark ink on white, which is what a page looks like.
//!
//! Neither version replaces the picture as it was. Large type survives
//! neither, and each makes nonsense of text of the other polarity. They are
//! two more readings to choose from; the choosing is done in
//! [`super::layout::merged`].

/// A picture in shades of grey, one byte a pixel, row by row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Gray {
    pub w: usize,
    pub h: usize,
    pub px: Vec<u8>,
}

impl Gray {
    /// The brightness of an RGBA picture.
    pub fn of_rgba(rgba: &[u8], w: usize, h: usize) -> Gray {
        let px = rgba
            .chunks_exact(4)
            .take(w * h)
            .map(|p| ((77 * p[0] as u32 + 150 * p[1] as u32 + 29 * p[2] as u32) >> 8) as u8)
            .collect();
        Gray { w, h, px }
    }
}

/// How wide a thing may be and still count as a stroke, in pixels, for type
/// as it comes off a screen. Wider than the stems of any text a player is
/// meant to read at a glance; narrower than a button.
pub const STROKE: usize = 25;

/// Light strokes, as dark ink on white: everything lighter than what
/// surrounds it and narrower than `size`.
pub fn light(gray: &Gray, size: usize) -> Gray {
    let opened = spread(&spread(gray, size, Take::Darkest), size, Take::Lightest);
    Gray {
        w: gray.w,
        h: gray.h,
        px: gray
            .px
            .iter()
            .zip(&opened.px)
            .map(|(was, now)| 255 - was.saturating_sub(*now))
            .collect(),
    }
}

/// Dark strokes, as dark ink on white: everything darker than what
/// surrounds it and narrower than `size`.
pub fn dark(gray: &Gray, size: usize) -> Gray {
    let closed = spread(&spread(gray, size, Take::Lightest), size, Take::Darkest);
    Gray {
        w: gray.w,
        h: gray.h,
        px: gray
            .px
            .iter()
            .zip(&closed.px)
            .map(|(was, now)| 255 - now.saturating_sub(*was))
            .collect(),
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Take {
    Darkest,
    Lightest,
}

impl Take {
    /// What lies beyond the edge of the picture: whatever cannot win.
    fn outside(self) -> u8 {
        match self {
            Take::Darkest => 255,
            Take::Lightest => 0,
        }
    }

    /// The winner of each pair, written out so that both loops are nothing
    /// but one comparison a byte: a compiler turns either into a few wide
    /// instructions.
    fn of_each(self, out: &mut [u8], a: &[u8], b: &[u8]) {
        match self {
            Take::Darkest => {
                for ((out, a), b) in out.iter_mut().zip(a).zip(b) {
                    *out = (*a).min(*b);
                }
            }
            Take::Lightest => {
                for ((out, a), b) in out.iter_mut().zip(a).zip(b) {
                    *out = (*a).max(*b);
                }
            }
        }
    }
}

/// Gives every pixel the darkest (or lightest) value within a square of
/// `size` by `size` centred on it: first along the rows, then down the
/// columns of what that gave.
fn spread(gray: &Gray, size: usize, take: Take) -> Gray {
    let size = size.max(1) | 1;
    let radius = size / 2;
    let (w, h) = (gray.w, gray.h);
    if w == 0 || h == 0 {
        return gray.clone();
    }

    // Every row with nothing — whatever cannot win — on either side of it.
    let stride = w + size - 1;
    let mut wide = vec![take.outside(); stride * h];
    for y in 0..h {
        let at = y * stride + radius;
        wide[at..at + w].copy_from_slice(&gray.px[y * w..(y + 1) * w]);
    }
    let wide = runs(wide, 1, size, take);

    // And the rows of that with nothing above and below them.
    let mut tall = vec![take.outside(); w * (h + size - 1)];
    for y in 0..h {
        let at = (y + radius) * w;
        tall[at..at + w].copy_from_slice(&wide[y * stride..y * stride + w]);
    }
    let mut tall = runs(tall, w, size, take);
    tall.truncate(w * h);
    Gray { w, h, px: tall }
}

/// Leaves in every place the winner of `size` values: that one and those
/// `unit`, `2 * unit`, … further on. A `unit` of one works along a row; a
/// `unit` of a row's length works down a column.
///
/// By doubling: the winner of two, then of two pairs, then of two fours,
/// and at the end of two overlapping runs that together cover `size`. Each
/// step is one pass over the whole picture comparing it with itself a fixed
/// distance on, so a window of twenty-five costs five passes whatever is in
/// it — where the obvious way costs twenty-five.
///
/// Places within `size - 1` steps of the end have no full run ahead of them
/// and are left holding nothing meaningful; the caller pads for that.
fn runs(mut values: Vec<u8>, unit: usize, size: usize, take: Take) -> Vec<u8> {
    let mut spare = vec![0u8; values.len()];
    let mut step = |values: &mut Vec<u8>, distance: usize| {
        let offset = distance * unit;
        let reach = values.len().saturating_sub(offset);
        take.of_each(
            &mut spare[..reach],
            &values[..reach],
            &values[offset..offset + reach],
        );
        std::mem::swap(values, &mut spare);
    };

    let mut width = 1;
    while width * 2 <= size {
        step(&mut values, width);
        width *= 2;
    }
    if size > width {
        step(&mut values, size - width);
    }
    values
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat(w: usize, h: usize, value: u8) -> Gray {
        Gray {
            w,
            h,
            px: vec![value; w * h],
        }
    }

    fn fill(gray: &mut Gray, x: usize, y: usize, w: usize, h: usize, value: u8) {
        for row in y..y + h {
            for column in x..x + w {
                gray.px[row * gray.w + column] = value;
            }
        }
    }

    fn at(gray: &Gray, x: usize, y: usize) -> u8 {
        gray.px[y * gray.w + x]
    }

    /// The darkest value in a square, worked out the slow and obvious way.
    fn darkest_by_hand(gray: &Gray, size: usize) -> Gray {
        let radius = (size / 2) as i64;
        let mut out = gray.clone();
        for y in 0..gray.h as i64 {
            for x in 0..gray.w as i64 {
                let mut value = 255u8;
                for dy in -radius..=radius {
                    for dx in -radius..=radius {
                        let (px, py) = (x + dx, y + dy);
                        if px >= 0 && py >= 0 && px < gray.w as i64 && py < gray.h as i64 {
                            value = value.min(gray.px[py as usize * gray.w + px as usize]);
                        }
                    }
                }
                out.px[y as usize * gray.w + x as usize] = value;
            }
        }
        out
    }

    #[test]
    fn the_quick_way_gives_what_the_obvious_way_gives() {
        // Not a pattern with any regularity a wrong window could hide in.
        let (w, h) = (61, 47);
        let mut gray = flat(w, h, 0);
        let mut seed = 12345u32;
        for value in gray.px.iter_mut() {
            seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            *value = (seed >> 16) as u8;
        }
        for size in [1, 3, 5, 9, 16, 17, 25, 31] {
            let odd = size | 1;
            assert_eq!(
                spread(&gray, size, Take::Darkest),
                darkest_by_hand(&gray, odd),
                "window of {size}"
            );
        }
    }

    #[test]
    fn a_light_stroke_is_kept_and_the_panel_it_is_on_is_not() {
        // A wide light panel on a dark ground, and a thin lighter stroke on
        // the panel.
        let mut gray = flat(200, 120, 20);
        fill(&mut gray, 20, 20, 160, 80, 110);
        fill(&mut gray, 60, 50, 80, 4, 250);

        let page = light(&gray, STROKE);
        // The stroke is ink: as dark as it was lighter than the panel.
        assert_eq!(at(&page, 100, 51), 255 - (250 - 110));
        // The panel and the ground are both blank paper.
        assert_eq!(at(&page, 30, 30), 255);
        assert_eq!(at(&page, 5, 5), 255);
    }

    #[test]
    fn a_dark_stroke_is_kept_by_the_other_one() {
        let mut gray = flat(200, 120, 230);
        fill(&mut gray, 60, 50, 80, 4, 40);

        let page = dark(&gray, STROKE);
        assert_eq!(at(&page, 100, 51), 255 - (230 - 40));
        assert_eq!(at(&page, 30, 30), 255);
        // And means nothing to the one looking for light strokes.
        assert_eq!(at(&light(&gray, STROKE), 100, 51), 255);
    }

    #[test]
    fn something_wider_than_a_stroke_is_not_one() {
        let mut gray = flat(200, 120, 20);
        fill(&mut gray, 40, 30, 60, 60, 240);
        let page = light(&gray, STROKE);
        assert_eq!(at(&page, 70, 60), 255);
    }

    #[test]
    fn the_edge_of_the_picture_is_not_a_stroke() {
        // Light all the way to the border: nothing there is thin.
        let page = light(&flat(80, 60, 200), STROKE);
        assert!(page.px.iter().all(|value| *value == 255));
    }

    #[test]
    fn brightness_weighs_green_most() {
        let rgba = [255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255];
        let gray = Gray::of_rgba(&rgba, 4, 1);
        assert!(gray.px[1] > gray.px[0] && gray.px[0] > gray.px[2]);
        assert_eq!(gray.px[3], 255);
    }

    #[test]
    fn a_picture_with_no_pixels_is_left_alone() {
        let empty = flat(0, 0, 0);
        assert_eq!(light(&empty, STROKE), empty);
    }
}
