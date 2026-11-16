//! Fitting a translation into the space its original occupied.
//!
//! A translation is almost never the length of the text it replaces — Russian
//! runs a quarter longer than English, German longer still, Chinese far
//! shorter. Painting it "in place" therefore means deciding three things for
//! every paragraph: how far its plate may stretch before it runs into a
//! neighbour, how large the type can be and still fit, and what to do when it
//! cannot fit at any size worth reading.
//!
//! All of it is arithmetic on rectangles, kept here without a toolkit in sight
//! so that the answers can be pinned down by tests instead of by squinting at
//! screenshots.

use egui::{Pos2, Rect, Vec2};

/// The outcome of trying to fit text into a box.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Fit {
    /// The font size to use.
    pub size: f32,
    /// How much room the text takes at that size.
    pub text: Vec2,
    /// False when even the smallest allowed size overflows the box.
    pub fits: bool,
}

/// Finds the largest font size in `min..=max` at which the text, wrapped to
/// `width`, is no taller than `height`.
///
/// `measure(size, wrap_width)` lays the text out and returns its size. Layout
/// is the expensive part, so the search is a bisection over half-point steps —
/// half a dozen layouts instead of one per candidate size. Text only gets
/// taller as the type grows, which is what makes bisecting valid.
pub fn fit(
    min: f32,
    max: f32,
    width: f32,
    height: f32,
    measure: impl Fn(f32, f32) -> Vec2,
) -> Fit {
    const STEP: f32 = 0.5;
    let max = max.max(min);

    let at_max = measure(max, width);
    if at_max.y <= height {
        return Fit {
            size: max,
            text: at_max,
            fits: true,
        };
    }

    let at_min = measure(min, width);
    if at_min.y > height {
        return Fit {
            size: min,
            text: at_min,
            fits: false,
        };
    }

    // Invariant: `lo` fits, `hi` does not.
    let mut lo = 0u32;
    let mut hi = ((max - min) / STEP).ceil() as u32;
    let mut best = at_min;
    while hi - lo > 1 {
        let mid = (lo + hi) / 2;
        let size = min + mid as f32 * STEP;
        let measured = measure(size, width);
        if measured.y <= height {
            lo = mid;
            best = measured;
        } else {
            hi = mid;
        }
    }

    Fit {
        size: min + lo as f32 * STEP,
        text: best,
        fits: true,
    }
}

/// The narrowest width between `least` and `most` for which `holds` is true,
/// to within a few points — or `None` if not even `most` will do.
///
/// For finding the column a paragraph needs: wide enough to hold a longer
/// translation in the lines the original had, and no wider, so that the lines
/// come out about as full as each other instead of one long and one short.
/// What holds at one width holds at every greater one, which is what makes
/// halving the range valid.
pub fn narrowest(least: f32, most: f32, holds: impl Fn(f32) -> bool) -> Option<f32> {
    const CLOSE_ENOUGH: f32 = 4.0;
    let most = most.max(least);
    if holds(least) {
        return Some(least);
    }
    if !holds(most) {
        return None;
    }
    // Invariant: `narrow` does not hold, `wide` does.
    let (mut narrow, mut wide) = (least, most);
    while wide - narrow > CLOSE_ENOUGH {
        let middle = (narrow + wide) / 2.0;
        if holds(middle) {
            wide = middle;
        } else {
            narrow = middle;
        }
    }
    Some(wide)
}

/// Which way a paragraph may grow sideways — away from the edge it hangs from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Grow {
    /// Left-aligned: its first letter stays where the original's was.
    Right,
    /// Right-aligned: its last letter stays where the original's was.
    Left,
    /// Centred: evenly to both sides, so it stays centred.
    Both,
}

/// How far a paragraph's plate may reach without covering its neighbours.
///
/// `blocks` are the rectangles of every recognised paragraph, `index` picks the
/// one being placed, and `bounds` is the window. A plate may grow sideways
/// until the next block on the same lines, and downwards until the next block
/// underneath — a longer translation spills into the empty space a layout
/// usually has, and stops short of text that is still to be read.
pub fn room_for(index: usize, blocks: &[Rect], bounds: Rect, gap: f32, grow: Grow) -> Rect {
    let block = blocks[index];
    let others = || {
        blocks
            .iter()
            .enumerate()
            .filter(move |(i, _)| *i != index)
            .map(|(_, r)| *r)
    };
    let shares_rows = |other: &Rect| other.min.y < block.max.y && other.max.y > block.min.y;

    let mut left = bounds.min.x;
    let mut right = bounds.max.x;
    for other in others().filter(shares_rows) {
        if other.min.x >= block.max.x {
            right = right.min(other.min.x - gap);
        } else if other.max.x <= block.min.x {
            left = left.max(other.max.x + gap);
        }
    }

    let (left, right) = match grow {
        Grow::Both => {
            let centre = block.center().x;
            let half = (centre - left).min(right - centre).max(block.width() / 2.0);
            (centre - half, centre + half)
        }
        Grow::Right => (block.min.x, right.max(block.max.x)),
        Grow::Left => (left.min(block.min.x), block.max.x),
    };

    let mut bottom = bounds.max.y;
    for other in others() {
        let shares_columns = other.min.x < right && other.max.x > left;
        if shares_columns && other.min.y >= block.max.y - 1.0 {
            bottom = bottom.min(other.min.y - gap);
        }
    }

    Rect::from_min_max(
        Pos2::new(left, block.min.y),
        Pos2::new(right, bottom.max(block.max.y)),
    )
}

/// Gives paragraphs that were evidently set in one size, one size.
///
/// A font size worked out from the height of a recognised line is only good to
/// a fifth or so: a word with a "g" in it measures taller than one without.
/// Left alone, a column of labels that was perfectly uniform comes back in
/// three slightly different sizes — and nothing looks more like a mistake.
///
/// So sizes close enough to each other are treated as one size, and every
/// paragraph of the group gets the group's median — of two, the smaller: type
/// a little small still fits where it has to go. A heading stays a heading:
/// it is more than a rounding error away.
///
/// How close is close enough depends on where a size came from. One measured
/// off the pixels (`measured`) is good to a pixel or so and is grouped
/// tightly. One guessed from a recogniser's box is not, so it takes the size
/// of the nearest measured group it could plausibly belong to — on a screen
/// where most lines could be measured, the few that could not are almost
/// always more of the same.
///
/// The groups are found where the sizes fall apart, not by counting off
/// windows from the smallest. A window has an edge, and two lines of one
/// paragraph that measured 22.0 and 22.2 can stand either side of it: one was
/// then set with the 18s below and the other with the 23s above.
pub fn harmonise(sizes: &[f32], measured: &[bool]) -> Vec<f32> {
    /// How far apart two measured sizes can be and still be one size.
    const MEASURED: f32 = 1.12;
    /// The same for sizes guessed from a box.
    const GUESSED: f32 = 1.3;

    let ascending = |mut indices: Vec<usize>| {
        indices.sort_by(|a, b| {
            sizes[*a]
                .partial_cmp(&sizes[*b])
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        indices
    };
    let is_measured = |index: usize| measured.get(index).copied().unwrap_or(false);
    let apart = |a: f32, b: f32| a.max(b) / a.min(b).max(f32::EPSILON);

    let mut out = sizes.to_vec();
    let exact = ascending((0..sizes.len()).filter(|i| is_measured(*i)).collect());
    settle(&exact, sizes, MEASURED, &mut out);

    let mut loose = Vec::new();
    for index in (0..sizes.len()).filter(|i| !is_measured(*i)) {
        let nearest = exact.iter().map(|m| out[*m]).min_by(|a, b| {
            apart(*a, sizes[index])
                .partial_cmp(&apart(*b, sizes[index]))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        match nearest {
            Some(size) if apart(size, sizes[index]) <= GUESSED => out[index] = size,
            _ => loose.push(index),
        }
    }
    settle(&ascending(loose), sizes, GUESSED, &mut out);
    out
}

/// Gives every size in `run` — indices into `sizes`, ascending by size — the
/// median of its group. A run that spans more than `tolerance` is not one
/// group: it is parted at its widest gap, and each part settled by itself.
fn settle(run: &[usize], sizes: &[f32], tolerance: f32, out: &mut [f32]) {
    let (Some(&first), Some(&last)) = (run.first(), run.last()) else {
        return;
    };
    if sizes[last] <= sizes[first] * tolerance {
        let median = sizes[run[(run.len() - 1) / 2]];
        for &index in run {
            out[index] = median;
        }
        return;
    }
    let gap = |at: usize| sizes[run[at]] / sizes[run[at - 1]].max(f32::EPSILON);
    let cut = (1..run.len())
        .max_by(|a, b| {
            gap(*a)
                .partial_cmp(&gap(*b))
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .unwrap_or(1);
    settle(&run[..cut], sizes, tolerance, out);
    settle(&run[cut..], sizes, tolerance, out);
}

/// Moves plates that ended up on top of each other apart, top to bottom.
///
/// A plate only overlaps the one below it when its text would not fit even at
/// the smallest size — at that point something has to give, and what gives is
/// position: the lower plate slides down just far enough to clear. Between
/// them the two still cover both originals, so no untranslated text shows
/// through.
///
/// Returns the lowest edge any plate reaches, which is how tall the window has
/// to be.
pub fn stack(plates: &mut [Rect], gap: f32) -> f32 {
    let mut order: Vec<usize> = (0..plates.len()).collect();
    order.sort_by(|a, b| {
        plates[*a]
            .min
            .y
            .partial_cmp(&plates[*b].min.y)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    for position in 1..order.len() {
        let current = order[position];
        let mut push_to = plates[current].min.y;
        for &earlier in &order[..position] {
            let above = plates[earlier];
            let overlaps_columns =
                above.min.x < plates[current].max.x && above.max.x > plates[current].min.x;
            if overlaps_columns && above.max.y + gap > push_to {
                push_to = above.max.y + gap;
            }
        }
        let shift = push_to - plates[current].min.y;
        if shift > 0.0 {
            plates[current] = plates[current].translate(Vec2::new(0.0, shift));
        }
    }

    plates.iter().map(|p| p.max.y).fold(0.0, f32::max)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stand-in for real text layout: fixed-pitch glyphs half as wide as
    /// they are tall, breaking wherever the line is full.
    fn monospace(chars: usize) -> impl Fn(f32, f32) -> Vec2 {
        move |size: f32, width: f32| {
            let glyph = size * 0.5;
            let per_line = ((width / glyph).floor() as usize).max(1);
            let lines = chars.div_ceil(per_line).max(1);
            let widest = chars.min(per_line) as f32 * glyph;
            Vec2::new(widest, lines as f32 * size * 1.2)
        }
    }

    fn rect(x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect::from_min_size(Pos2::new(x, y), Vec2::new(w, h))
    }

    #[test]
    fn text_that_fits_at_the_preferred_size_keeps_it() {
        let f = fit(10.0, 20.0, 400.0, 30.0, monospace(20));
        assert!(f.fits);
        assert_eq!(f.size, 20.0);
    }

    #[test]
    fn longer_text_shrinks_until_it_fits() {
        // Forty glyphs at 20 pt are 400 wide: two lines in a 200-wide box,
        // which is taller than the 30 allowed. It has to come down.
        let f = fit(10.0, 20.0, 200.0, 30.0, monospace(40));
        assert!(f.fits);
        assert!(f.size < 20.0 && f.size >= 10.0, "size {}", f.size);
        assert!(f.text.y <= 30.0);

        // And it is the *largest* size that fits: half a point more does not.
        let bigger = monospace(40)(f.size + 0.5, 200.0);
        assert!(bigger.y > 30.0, "{} would also have fitted", f.size + 0.5);
    }

    #[test]
    fn text_that_cannot_fit_reports_so_at_the_smallest_size() {
        let f = fit(10.0, 20.0, 50.0, 12.0, monospace(500));
        assert!(!f.fits);
        assert_eq!(f.size, 10.0);
        assert!(f.text.y > 12.0);
    }

    #[test]
    fn a_minimum_above_the_maximum_does_not_invert_the_search() {
        let f = fit(14.0, 9.0, 400.0, 100.0, monospace(10));
        assert_eq!(f.size, 14.0);
        assert!(f.fits);
    }

    #[test]
    fn the_size_lands_on_a_half_point() {
        let f = fit(10.0, 20.0, 200.0, 30.0, monospace(40));
        assert_eq!((f.size * 2.0).fract(), 0.0, "size {}", f.size);
    }

    #[test]
    fn a_lone_block_may_use_the_rest_of_the_window() {
        let bounds = rect(0.0, 0.0, 600.0, 200.0);
        let room = room_for(0, &[rect(20.0, 30.0, 200.0, 20.0)], bounds, 4.0, Grow::Right);
        assert_eq!(room.min, Pos2::new(20.0, 30.0));
        assert_eq!(room.max, Pos2::new(600.0, 200.0));
    }

    #[test]
    fn a_block_stops_short_of_its_neighbour_on_the_same_line() {
        // A label and its value, side by side.
        let blocks = [rect(20.0, 30.0, 100.0, 20.0), rect(300.0, 30.0, 80.0, 20.0)];
        let room = room_for(0, &blocks, rect(0.0, 0.0, 600.0, 200.0), 4.0, Grow::Right);
        assert_eq!(room.max.x, 296.0);
    }

    #[test]
    fn a_block_stops_short_of_the_paragraph_below() {
        let blocks = [rect(20.0, 30.0, 300.0, 20.0), rect(20.0, 80.0, 300.0, 20.0)];
        let room = room_for(0, &blocks, rect(0.0, 0.0, 600.0, 200.0), 4.0, Grow::Right);
        assert_eq!(room.max.y, 76.0);
    }

    #[test]
    fn a_block_off_to_the_side_below_does_not_limit_the_height() {
        // Underneath, but entirely to the left of where this one can reach.
        let blocks = [rect(300.0, 30.0, 100.0, 20.0), rect(20.0, 80.0, 100.0, 20.0)];
        let room = room_for(0, &blocks, rect(0.0, 0.0, 600.0, 200.0), 4.0, Grow::Right);
        assert_eq!(room.max.y, 200.0);
    }

    #[test]
    fn a_centred_block_grows_evenly_and_stays_centred() {
        let block = rect(250.0, 30.0, 100.0, 20.0);
        let room = room_for(0, &[block], rect(0.0, 0.0, 500.0, 200.0), 4.0, Grow::Both);
        assert_eq!(room.center().x, block.center().x);
        // 300 from the centre to the left edge, 200 to the right: the smaller
        // one decides, on both sides.
        assert_eq!(room.width(), 400.0);
    }

    #[test]
    fn a_right_aligned_block_grows_to_the_left_and_keeps_its_right_edge() {
        // A value at the end of a row, with its label at the start of it.
        let blocks = [rect(790.0, 30.0, 130.0, 20.0), rect(80.0, 30.0, 110.0, 20.0)];
        let room = room_for(0, &blocks, rect(0.0, 0.0, 1000.0, 200.0), 4.0, Grow::Left);
        assert_eq!(room.max.x, 920.0, "the right edge moved");
        assert_eq!(room.min.x, 194.0, "it should stop short of the label");
    }

    #[test]
    fn a_right_aligned_block_alone_may_reach_the_left_edge_of_the_window() {
        let room = room_for(
            0,
            &[rect(790.0, 30.0, 130.0, 20.0)],
            rect(0.0, 0.0, 1000.0, 200.0),
            4.0,
            Grow::Left,
        );
        assert_eq!(room.min.x, 0.0);
        assert_eq!(room.max.x, 920.0);
    }

    #[test]
    fn a_column_is_as_narrow_as_still_holds_the_text() {
        // Holds from 317 upwards.
        let width = narrowest(200.0, 800.0, |w| w >= 317.0).expect("800 holds");
        assert!((317.0..=321.0).contains(&width), "{width}");
    }

    #[test]
    fn text_that_already_holds_keeps_its_own_width() {
        assert_eq!(narrowest(200.0, 800.0, |_| true), Some(200.0));
    }

    #[test]
    fn text_no_width_will_hold_has_no_column() {
        assert_eq!(narrowest(200.0, 800.0, |_| false), None);
    }

    #[test]
    fn a_range_that_is_no_range_is_just_asked_once() {
        assert_eq!(narrowest(500.0, 300.0, |w| w >= 500.0), Some(500.0));
        assert_eq!(narrowest(500.0, 300.0, |_| false), None);
    }

    #[test]
    fn of_two_sizes_taken_as_one_the_smaller_is_kept() {
        assert_eq!(harmonise(&[24.0, 26.0], &[false; 2]), vec![24.0, 24.0]);
    }

    #[test]
    fn sizes_a_rounding_error_apart_become_one_size() {
        // Three labels and one that measured taller because of a descender.
        let out = harmonise(&[20.0, 21.0, 25.5, 20.5], &[false; 4]);
        assert!(out.iter().all(|s| *s == out[0]), "{out:?}");
        assert!((20.0..=21.0).contains(&out[0]), "{out:?}");
    }

    #[test]
    fn a_heading_keeps_its_own_size() {
        let out = harmonise(&[34.0, 20.0, 21.0, 13.0], &[false; 4]);
        assert_eq!(out[0], 34.0);
        assert_eq!(out[1], out[2]);
        assert_eq!(out[3], 13.0);
        assert!(out[1] > out[3] && out[0] > out[1]);
    }

    #[test]
    fn harmonising_keeps_every_size_in_its_original_position() {
        let out = harmonise(&[13.0, 34.0, 13.5], &[false; 3]);
        assert_eq!(out.len(), 3);
        assert_eq!(out[1], 34.0);
        assert_eq!(out[0], out[2]);
    }

    #[test]
    fn nothing_to_harmonise_is_nothing() {
        assert!(harmonise(&[], &[]).is_empty());
        assert_eq!(harmonise(&[17.0], &[false]), vec![17.0]);
        assert_eq!(harmonise(&[17.0], &[true]), vec![17.0]);
    }

    #[test]
    fn two_lines_of_one_paragraph_never_end_up_in_different_sizes() {
        // A whole screen: a panel of 18s, a paragraph whose two lines
        // measured 22.0 and 22.2, a line above them at 22.6, one tiny line
        // that could not be measured at all. Counted off in windows from the
        // smallest, the 22.0 fell in with the 18s and the 22.2 did not.
        let sizes = [18.0, 22.0, 18.0, 22.2, 17.0, 18.0, 22.6, 30.0];
        let measured = [true, true, true, true, false, true, true, true];
        let out = harmonise(&sizes, &measured);
        assert_eq!(out[1], out[3], "{out:?}");
        assert_eq!(out[1], out[6], "{out:?}");
        assert!((22.0..=22.6).contains(&out[1]), "{out:?}");
        assert_eq!([out[0], out[2], out[5]], [18.0; 3]);
        assert_eq!(out[7], 30.0);
    }

    #[test]
    fn a_guess_takes_the_measured_size_it_is_nearest_to() {
        // 17 is a guess from a box; the 18s were measured. It is one of them.
        let out = harmonise(&[18.0, 18.0, 17.0, 30.0], &[true, true, false, true]);
        assert_eq!(out[2], 18.0);
        // A guess nowhere near anything measured keeps its own counsel.
        let out = harmonise(&[18.0, 44.0, 30.0], &[true, false, true]);
        assert_eq!(out[1], 44.0);
    }

    #[test]
    fn measured_sizes_are_held_to_a_tighter_standard_than_guesses() {
        // 20 and 25 as guesses are one size; measured, they are two.
        assert_eq!(harmonise(&[20.0, 25.0], &[false; 2]), vec![20.0, 20.0]);
        assert_eq!(harmonise(&[20.0, 25.0], &[true; 2]), vec![20.0, 25.0]);
    }

    #[test]
    fn a_long_run_of_sizes_parts_at_its_widest_gap() {
        let out = harmonise(&[13.0, 13.4, 18.0, 18.3, 18.9], &[true; 5]);
        assert_eq!(out[0], out[1]);
        assert_eq!(out[2], out[3]);
        assert_eq!(out[3], out[4]);
        assert!(out[1] < out[2]);
    }

    #[test]
    fn room_is_never_smaller_than_the_block_itself() {
        // Hemmed in on every side.
        let blocks = [
            rect(100.0, 100.0, 100.0, 20.0),
            rect(201.0, 100.0, 50.0, 20.0),
            rect(100.0, 120.0, 100.0, 20.0),
        ];
        let room = room_for(0, &blocks, rect(0.0, 0.0, 600.0, 400.0), 4.0, Grow::Right);
        assert!(room.width() >= 100.0);
        assert!(room.height() >= 20.0);
    }

    #[test]
    fn plates_that_do_not_touch_are_left_alone() {
        let mut plates = [rect(0.0, 0.0, 100.0, 20.0), rect(0.0, 40.0, 100.0, 20.0)];
        let before = plates;
        let bottom = stack(&mut plates, 2.0);
        assert_eq!(plates, before);
        assert_eq!(bottom, 60.0);
    }

    #[test]
    fn an_overflowing_plate_pushes_the_next_one_down() {
        // The first plate needed three lines where the original had one.
        let mut plates = [rect(0.0, 0.0, 100.0, 60.0), rect(0.0, 30.0, 100.0, 20.0)];
        let bottom = stack(&mut plates, 2.0);
        assert_eq!(plates[1].min.y, 62.0);
        assert_eq!(plates[1].height(), 20.0, "pushing must not resize");
        assert_eq!(bottom, 82.0);
    }

    #[test]
    fn a_push_carries_on_down_the_column() {
        let mut plates = [
            rect(0.0, 0.0, 100.0, 60.0),
            rect(0.0, 30.0, 100.0, 20.0),
            rect(0.0, 55.0, 100.0, 20.0),
        ];
        stack(&mut plates, 2.0);
        assert_eq!(plates[1].min.y, 62.0);
        assert_eq!(plates[2].min.y, 84.0);
    }

    #[test]
    fn plates_in_different_columns_do_not_push_each_other() {
        let mut plates = [rect(0.0, 0.0, 100.0, 60.0), rect(200.0, 30.0, 100.0, 20.0)];
        let before = plates;
        stack(&mut plates, 2.0);
        assert_eq!(plates, before);
    }

    #[test]
    fn stacking_does_not_depend_on_the_order_the_plates_arrive_in() {
        let mut plates = [rect(0.0, 30.0, 100.0, 20.0), rect(0.0, 0.0, 100.0, 60.0)];
        stack(&mut plates, 2.0);
        assert_eq!(plates[0].min.y, 62.0);
        assert_eq!(plates[1].min.y, 0.0);
    }
}
