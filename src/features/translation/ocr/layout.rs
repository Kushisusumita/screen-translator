//! Paragraphs out of bare lines of words.
//!
//! The online recogniser answers in paragraphs: it has already decided which
//! lines belong together and has thrown away what was not writing. A
//! recogniser that runs on the machine itself answers with lines of words and
//! nothing else — every line it thought it saw, in whatever order it came
//! across them, a logo read as `flt•lfrgtt` among them.
//!
//! Three things are done about that here, and all three are arithmetic on
//! words and rectangles, so that they can be tested without a recogniser:
//!
//! * [`merged`] takes several readings of one picture — the same recogniser
//!   shown it prepared in different ways — and keeps, for every place on it,
//!   the reading that makes the most sense;
//! * what makes no sense at all is dropped, because a translation of noise
//!   painted over a game is worse than no translation — and what is set
//!   large has to have been read the same twice, because the largest thing
//!   on a title screen is a logo, and a logo reads as a different word every
//!   time it is looked at ([`Kept::is_doubtful`]);
//! * [`blocks`] puts the lines that are left together into paragraphs, the
//!   shape the rest of the pipeline works in.

use super::{align_columns, alignment, median, Line, PxRect, TextBlock};
use crate::features::translation::providers::normalize_ocr_text;

/// A word as a recogniser boxed it, in capture pixels.
#[derive(Debug, Clone, PartialEq)]
pub struct Word {
    pub rect: PxRect,
    pub text: String,
}

/// Words a recogniser found standing on one line, left to right.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct WordLine {
    pub words: Vec<Word>,
}

impl WordLine {
    /// The rectangle round all of its words.
    pub fn rect(&self) -> PxRect {
        self.words
            .iter()
            .map(|word| word.rect)
            .reduce(|a, b| a.union(b))
            .unwrap_or_default()
    }

    /// What the line says. Words are a space apart, except in writing that
    /// has no spaces: a recogniser hands Japanese and Chinese over one
    /// character at a time, and a space between each would be a space that
    /// was never there.
    pub fn text(&self) -> String {
        let mut out = String::new();
        for word in &self.words {
            let text = word.text.trim();
            let Some(first) = text.chars().next() else {
                continue;
            };
            if let Some(last) = out.chars().next_back() {
                if !(unspaced(last) && unspaced(first)) {
                    out.push(' ');
                }
            }
            out.push_str(text);
        }
        out
    }
}

/// Characters of writing that puts no space between its words.
fn unspaced(c: char) -> bool {
    matches!(c as u32,
        0x3000..=0x30FF   // CJK punctuation, kana
        | 0x3400..=0x4DBF // CJK ext A
        | 0x4E00..=0x9FFF // CJK unified
        | 0xF900..=0xFAFF // compatibility
        | 0xFF00..=0xFFEF // full-width forms
    )
}

// ── What reads as writing ────────────────────────────────────────────────────

/// How much of a line reads as writing rather than as noise.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Sense {
    /// Letters standing in what look like words.
    letters: usize,
    /// Letters and digits in what look like words and numbers.
    good: usize,
    /// The share of the line those make up, 0–1.
    share: f32,
    /// One character was a word by itself, as it can be in writing that has
    /// no alphabet.
    ideograph: bool,
}

impl Sense {
    /// Worth translating: mostly words, and at least one of them. A line of
    /// figures alone — `2560 x 1440`, `0.65` — says the same in every
    /// language and is better left as it stands on the screen.
    fn is_writing(&self) -> bool {
        (self.letters >= 2 || self.ideograph) && self.share >= 0.6
    }

    /// Clean enough that its length, not its cleanliness, is what should
    /// decide between it and another reading of the same place.
    fn is_clean(&self) -> bool {
        self.share >= 0.85
    }
}

/// Marks that join the parts of one word or one number: `we're`, `v1.2`,
/// `1,200`, `12:30`, `and/or`.
const JOINERS: &[char] = &['\'', '’', '-', '‐', '‑', '.', ',', ':', '/', '&', '+', '%'];

/// Marks that no line of prose or interface has a use for. A recogniser
/// produces them where it was shown something that is not lettering.
fn is_odd(c: char) -> bool {
    matches!(
        c,
        '•' | '·' | '¦' | '|' | '\\' | '^' | '_' | '`' | '´' | '¨' | '¬' | '¯' | '¤' | '§' | '¶'
            | '¢' | '‚' | '†' | '‡' | '◦' | '▪' | '■' | '□' | '●' | '○' | '~' | '='
    )
}

enum Part {
    Word(usize),
    Number(usize),
    /// One letter on its own: `a`, `I`, the `E` of "Press E". Nothing against
    /// it and nothing for it.
    Single,
    Junk,
}

fn part(text: &str) -> Part {
    let chars: Vec<char> = text.chars().collect();
    let letters = chars.iter().filter(|c| c.is_alphabetic()).count();
    if letters == 0 {
        return Part::Number(chars.len());
    }
    if letters < chars.len() {
        // Letters and digits together: `3D`, `x2`, `F5`, `1440p` — or a word
        // with a digit read into the middle of it.
        return if chars.len() <= 6 {
            Part::Number(chars.len())
        } else {
            Part::Junk
        };
    }
    if chars.len() == 1 {
        let c = chars[0];
        return if c.is_uppercase() || c.is_lowercase() {
            Part::Single
        } else {
            // A character of a script without an alphabet is a word.
            Part::Word(1)
        };
    }
    if chars.len() > 28 || !well_cased(&chars) || !pronounceable(&chars) {
        return Part::Junk;
    }
    Part::Word(chars.len())
}

/// All small, all capitals, or a capital and then small — the ways a word is
/// written. One more capital inside is let through when a word carries on
/// after it: `YouTube`, `McDonald`.
fn well_cased(chars: &[char]) -> bool {
    let upper = chars.iter().filter(|c| c.is_uppercase()).count();
    let lower = chars.iter().filter(|c| c.is_lowercase()).count();
    if upper == 0 || lower == 0 {
        return true;
    }
    let inside: Vec<usize> = (1..chars.len()).filter(|i| chars[*i].is_uppercase()).collect();
    match inside.as_slice() {
        [] => true,
        [at] => chars.len() - at > 2 && chars[at + 1..].iter().all(|c| c.is_lowercase()),
        _ => false,
    }
}

/// Whether letters could be said aloud: a word of three letters or more has
/// a vowel in it. Short runs of capitals are let off — `HP`, `NPC`, `DLSS`.
fn pronounceable(chars: &[char]) -> bool {
    if chars.len() < 3 {
        return true;
    }
    if chars.len() <= 5 && chars.iter().all(|c| c.is_uppercase()) {
        return true;
    }
    let vowels: &[char] = match chars[0] as u32 {
        0x0041..=0x024F => &['a', 'e', 'i', 'o', 'u', 'y'],
        0x0370..=0x03FF => &['α', 'ε', 'η', 'ι', 'ο', 'υ', 'ω'],
        0x0400..=0x04FF => &[
            'а', 'е', 'ё', 'и', 'о', 'у', 'ы', 'э', 'ю', 'я', 'і', 'ї', 'є', 'ә', 'ө', 'ү', 'ұ',
        ],
        // No alphabet this could be checked against.
        _ => return true,
    };
    let is_vowel = |c: &char| {
        // An accented letter is nearly always a vowel, and not worth a table.
        !c.is_ascii() && (*c as u32) < 0x0250
            || c.to_lowercase().any(|lower| vowels.contains(&lower))
    };
    // And is not six consonants in a row: no language a game is written in
    // asks that of a reader, and a logo read letter by letter often does.
    let mut run = 0;
    let mut longest = 0;
    for c in chars {
        run = if is_vowel(c) { 0 } else { run + 1 };
        longest = longest.max(run);
    }
    longest < chars.len() && longest <= 5
}

fn sense(text: &str) -> Sense {
    let (mut letters, mut good, mut bad) = (0usize, 0usize, 0.0f32);
    let mut ideograph = false;

    for token in text.split_whitespace() {
        let core = token.trim_matches(|c: char| !c.is_alphanumeric());
        let odd = token.chars().filter(|c| is_odd(*c)).count() as f32;
        if core.is_empty() {
            // Standing by itself, such a mark is something that was read
            // where there was no letter at all.
            bad += 1.5 * odd;
            continue;
        }
        bad += 0.5 * odd;
        let length = core.chars().count();
        let mut found = (0usize, 0usize, 0.0f32);
        let mut broken = false;
        for piece in core.split(JOINERS) {
            if piece.is_empty() {
                continue;
            }
            if piece.chars().any(|c| !c.is_alphanumeric()) {
                // A mark in the middle of a word that joins nothing.
                broken = true;
                break;
            }
            match part(piece) {
                Part::Word(n) => {
                    ideograph |= n == 1;
                    found.0 += n;
                    found.1 += n;
                }
                Part::Number(n) => found.1 += n,
                Part::Single => found.2 += 0.5,
                Part::Junk => {
                    broken = true;
                    break;
                }
            }
        }
        if broken {
            bad += length as f32;
        } else {
            letters += found.0;
            good += found.1;
            bad += found.2;
        }
    }

    // A bracket that opens and never closes was not a bracket.
    for (open, close) in [('(', ')'), ('[', ']'), ('{', '}')] {
        let opened = text.chars().filter(|c| *c == open).count() as f32;
        let closed = text.chars().filter(|c| *c == close).count() as f32;
        bad += 0.5 * (opened - closed).abs();
    }

    let share = if good == 0 {
        0.0
    } else {
        good as f32 / (good as f32 + bad)
    };
    Sense {
        letters,
        good,
        share,
        ideograph: ideograph && share > 0.0,
    }
}

/// Puts right what a recogniser gets wrong in a way that can be told from
/// the text alone, and takes out marks that are certainly not writing.
fn tidy(line: &WordLine) -> WordLine {
    let mut words: Vec<Word> = line
        .words
        .iter()
        .filter_map(|word| {
            let mut text: String = word.text.chars().filter(|c| !is_odd(*c)).collect();
            // A capital K whose two halves were read apart.
            for split in ["I<", "1<", "l<"] {
                if let Some(rest) = text.strip_prefix(split) {
                    if rest.chars().next().is_some_and(char::is_alphabetic) {
                        text = format!("K{rest}");
                    }
                }
            }
            // A quotation mark with letters on both sides of it is a speck
            // between two words.
            let chars: Vec<char> = text.chars().collect();
            let text: String = chars
                .iter()
                .enumerate()
                .map(|(i, c)| {
                    let between = i > 0
                        && i + 1 < chars.len()
                        && chars[i - 1].is_alphanumeric()
                        && chars[i + 1].is_alphanumeric();
                    if between && matches!(c, '«' | '»' | '„' | '“' | '”' | '‹' | '›' | '"') {
                        ' '
                    } else {
                        *c
                    }
                })
                .collect();
            let text = text.trim().to_string();
            (!text.is_empty()).then(|| Word {
                rect: word.rect,
                text,
            })
        })
        .collect();

    // A bracket that opens the line and never closes, or closes it having
    // never opened: the rounded end of a button, read as one. It goes, and
    // the box gives up about as much room as a bracket takes — less rather
    // than more, since what is left over is trimmed against the picture
    // later and what is cut off is not put back.
    let said: String = words.iter().map(|word| word.text.as_str()).collect();
    let count = |mark: char| said.chars().filter(|c| *c == mark).count();
    for (open, close) in [('(', ')'), ('[', ']'), ('{', '}')] {
        if count(open) > count(close) {
            if let Some(first) = words.first_mut() {
                if let Some(rest) = first.text.strip_prefix(open) {
                    let share = first.rect.w / first.text.chars().count() as f32 * 0.5;
                    first.rect.x += share;
                    first.rect.w -= share;
                    first.text = rest.to_string();
                }
            }
        }
        if count(close) > count(open) {
            if let Some(last) = words.last_mut() {
                if let Some(rest) = last.text.strip_suffix(close) {
                    last.rect.w -= last.rect.w / last.text.chars().count() as f32 * 0.5;
                    last.text = rest.to_string();
                }
            }
        }
    }
    words.retain(|word| !word.text.is_empty() && word.rect.w > 0.0);
    WordLine { words }
}

// ── Several readings of one picture ──────────────────────────────────────────

/// Cuts a line where two of its words stand further apart than words of one
/// line do: a label and its value at opposite ends of a row are two pieces of
/// text, however level they are.
fn columns(line: WordLine) -> Vec<WordLine> {
    let reach = full_height(&line.text(), line.rect().h) * 3.0;
    let mut out = vec![WordLine::default()];
    for word in line.words {
        let far = out
            .last()
            .and_then(|piece| piece.words.last())
            .is_some_and(|before| word.rect.x - before.rect.right() > reach);
        if far {
            out.push(WordLine::default());
        }
        if let Some(piece) = out.last_mut() {
            piece.words.push(word);
        }
    }
    out.retain(|piece| !piece.words.is_empty());
    out
}

fn overlap(a: PxRect, b: PxRect) -> f32 {
    let w = a.right().min(b.right()) - a.x.max(b.x);
    let h = a.bottom().min(b.bottom()) - a.y.max(b.y);
    if w <= 0.0 || h <= 0.0 {
        return 0.0;
    }
    let smaller = (a.w * a.h).min(b.w * b.h);
    if smaller <= 0.0 {
        return 0.0;
    }
    w * h / smaller
}

/// How tall type has to be, ascender to descender in capture pixels, to
/// count as display type: a title, a logo.
const DISPLAY: f32 = 56.0;

/// A line that came through [`merged`].
#[derive(Debug, Clone, PartialEq)]
pub struct Kept {
    pub line: WordLine,
    /// Another reading of the picture said the same in the same place.
    pub confirmed: bool,
}

impl Kept {
    /// Large, and read only once.
    ///
    /// Type of an ordinary size that reads as words is words. Type set large
    /// is as often artwork: the name of the game drawn in dripping letters,
    /// which a recogniser turns into `Witt` one time and `furifrien` the
    /// next. Painted over with the translation of that, a title screen has
    /// lost its logo and gained a word nobody wrote. What tells the two
    /// apart is that real lettering reads the same however it is looked
    /// at, so display type is asked to do that.
    pub fn is_doubtful(&self) -> bool {
        !self.confirmed && full_height(&self.line.text(), self.line.rect().h) >= DISPLAY
    }

    /// Whether one of `others` says the same as this line, where it is.
    pub fn is_borne_out_by(&self, others: &[WordLine]) -> bool {
        let (rect, text) = (self.line.rect(), self.line.text());
        others
            .iter()
            .any(|other| overlap(rect, other.rect()) >= 0.4 && agree(&text, &other.text()))
    }
}

/// Whether two readings say the same thing, give or take what a recogniser
/// gets wrong between one look and the next: capitals, spacing, punctuation,
/// and a letter in five.
fn agree(a: &str, b: &str) -> bool {
    let bare = |text: &str| -> Vec<char> {
        text.chars()
            .filter(|c| c.is_alphanumeric())
            .flat_map(char::to_lowercase)
            .collect()
    };
    let (a, b) = (bare(a), bare(b));
    if a.is_empty() || b.is_empty() {
        return false;
    }
    // Edit distance, one row at a time.
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, x) in a.iter().enumerate() {
        let mut diagonal = row[0];
        row[0] = i + 1;
        for (j, y) in b.iter().enumerate() {
            let swap = diagonal + usize::from(x != y);
            diagonal = row[j + 1];
            row[j + 1] = swap.min(row[j] + 1).min(diagonal + 1);
        }
    }
    row[b.len()] * 5 <= a.len().max(b.len())
}

/// One reading out of several readings of the same picture.
///
/// `readings` are in order of trust: where two of them read a place equally
/// well, the earlier one is kept. A place is given to whichever reading of
/// it makes the most sense — the cleanest, and among clean ones the one that
/// read the most — and every other reading of that place is dropped, as is
/// anything that does not read as writing at all.
pub fn merged(readings: Vec<Vec<WordLine>>) -> Vec<Kept> {
    struct Candidate {
        line: WordLine,
        rect: PxRect,
        sense: Sense,
        reading: usize,
        confirmed: bool,
    }

    let mut candidates: Vec<Candidate> = Vec::new();
    for (reading, lines) in readings.into_iter().enumerate() {
        for line in lines.into_iter().flat_map(columns) {
            // Judged as it was read, so that a line which needed tidying
            // counts for less than one which did not.
            let sense = sense(&line.text());
            let line = tidy(&line);
            let rect = line.rect();
            if sense.is_writing() && rect.w > 0.0 && rect.h > 0.0 {
                candidates.push(Candidate {
                    line,
                    rect,
                    sense,
                    reading,
                    confirmed: false,
                });
            }
        }
    }

    candidates.sort_by(|a, b| {
        b.sense
            .is_clean()
            .cmp(&a.sense.is_clean())
            .then(b.sense.good.cmp(&a.sense.good))
            .then(b.sense.share.total_cmp(&a.sense.share))
            .then(a.reading.cmp(&b.reading))
    });

    let mut kept: Vec<Candidate> = Vec::new();
    for candidate in candidates {
        let mut taken = false;
        for other in kept.iter_mut() {
            if overlap(candidate.rect, other.rect) >= 0.4 {
                taken = true;
                // Passed over, but not for nothing if it said the same.
                if candidate.reading != other.reading
                    && agree(&candidate.line.text(), &other.line.text())
                {
                    other.confirmed = true;
                }
            }
        }
        if !taken {
            kept.push(candidate);
        }
    }
    kept.into_iter()
        .map(|candidate| Kept {
            line: candidate.line,
            confirmed: candidate.confirmed,
        })
        .collect()
}

// ── Lines into paragraphs ────────────────────────────────────────────────────

/// How tall the type of a line is from ascender to descender, given how tall
/// its ink is.
///
/// A box drawn tight round `Shadows` is as tall as a capital; round `High`
/// it reaches down to the tail of the `g` as well. They are the same type,
/// and what is measured off them must come out the same, so the part of the
/// height that a line's own letters happen not to use is put back.
fn full_height(text: &str, ink: f32) -> f32 {
    let (mut rises, mut hangs, mut cased) = (false, false, false);
    for c in text.chars() {
        if c.is_uppercase() || c.is_numeric() {
            rises = true;
            cased |= c.is_uppercase();
        } else if c.is_lowercase() {
            cased = true;
            rises |= matches!(c, 'b' | 'd' | 'f' | 'h' | 'i' | 'j' | 'k' | 'l' | 't' | 'б' | 'ё' | 'й');
            hangs |= matches!(c, 'g' | 'j' | 'p' | 'q' | 'y' | 'р' | 'у' | 'ф' | 'д' | 'ц' | 'щ');
        } else if matches!(c, '(' | ')' | '[' | ']' | '{' | '}' | '/' | '|') {
            rises = true;
            hangs = true;
        }
    }
    if !cased {
        // Writing without ascenders and descenders fills its line.
        return ink;
    }
    match (rises, hangs) {
        (true, true) => ink,
        (true, false) => ink * 1.28,
        (false, true) => ink * 1.4,
        (false, false) => ink * 1.9,
    }
}

/// A line, with what is asked about it more than once.
struct Row {
    rect: PxRect,
    text: String,
    glyph: f32,
    words: usize,
}

impl Row {
    fn of(line: &WordLine) -> Option<Row> {
        let rect = line.rect();
        let text = line.text();
        if text.is_empty() || rect.w <= 0.0 || rect.h <= 0.0 {
            return None;
        }
        Some(Row {
            glyph: full_height(&text, rect.h),
            words: text.split_whitespace().count(),
            rect,
            text,
        })
    }

    /// Set in capitals throughout.
    fn shouts(&self) -> bool {
        let mut cased = self.text.chars().filter(|c| c.is_uppercase() || c.is_lowercase());
        cased.clone().count() >= 2 && cased.all(char::is_uppercase)
    }

    /// Ends the way a sentence does, closing quotes and brackets aside.
    fn ends_a_sentence(&self) -> bool {
        self.text
            .trim_end_matches(['"', '\'', '”', '’', '»', ')', ']', '」', '』'])
            .ends_with(['.', '!', '?', '…', '。', '！', '？'])
    }

    /// Ends on a mark that promises more.
    fn ends_open(&self) -> bool {
        self.text.ends_with([',', ';', '-', '‐', '–', '—', '、'])
    }

    /// Ends on a word nothing ends on: an article, a preposition, an "and".
    fn ends_unfinished(&self) -> bool {
        let last = self
            .text
            .split_whitespace()
            .next_back()
            .unwrap_or_default()
            .trim_matches(|c: char| !c.is_alphanumeric())
            .to_lowercase();
        UNFINISHED.contains(&last.as_str())
    }

    /// Begins with a word nothing begins with: "on Patreon", "of the Ring",
    /// "and friends".
    fn begins_joined(&self) -> bool {
        let first = self
            .text
            .split_whitespace()
            .next()
            .unwrap_or_default()
            .trim_matches(|c: char| !c.is_alphanumeric())
            .to_lowercase();
        JOINING.contains(&first.as_str())
    }
}

/// Words that join what follows them to what came before: prepositions and
/// conjunctions. A caption's second line may begin with one; an entry of a
/// menu does not.
const JOINING: &[&str] = &[
    "of", "to", "and", "or", "with", "for", "from", "in", "on", "at", "by", "than", "und",
    "oder", "von", "mit", "für", "zu", "et", "ou", "de", "du", "des", "à", "avec", "pour", "y",
    "del", "con", "para", "por", "di", "per", "em", "com", "и", "или", "в", "на", "с", "к", "о",
    "для", "от", "по", "из",
];

/// Words a sentence does not stop on, in the languages a game is most often
/// written in. Far from complete, and it need not be: it is only asked when
/// nothing else has settled whether a line carries on.
const UNFINISHED: &[&str] = &[
    // English
    "a", "an", "the", "of", "to", "in", "on", "at", "by", "for", "with", "from", "and", "or",
    "but", "that", "which", "who", "is", "are", "was", "were", "be", "as", "if", "your", "my",
    "his", "her", "their", "our", "its", "this", "these", "those", "not", "will", "can",
    // German
    "der", "die", "das", "den", "dem", "des", "ein", "eine", "einen", "und", "oder", "zu", "im",
    "mit", "von", "für", "auf", "ist", "sind", "dass",
    // French
    "le", "la", "les", "un", "une", "du", "de", "et", "ou", "à", "en", "dans", "pour", "avec",
    "sur", "que", "qui", "est",
    // Spanish, Italian, Portuguese
    "el", "los", "las", "una", "del", "y", "o", "con", "por", "para", "es", "il", "lo", "gli",
    "di", "e", "per", "che", "è", "os", "as", "um", "uma", "do", "da", "em", "com", "é",
    // Polish, Russian, Ukrainian
    "i", "w", "z", "na", "że", "nie", "и", "в", "на", "с", "к", "у", "о", "за", "из", "по", "не",
    "что", "как", "а", "но", "для", "от", "і", "та", "що", "до", "й",
];

/// Whether `next` is the following line of the paragraph `so_far`, by where
/// it stands.
fn stands_under(so_far: &[Row], next: &Row) -> bool {
    let Some(last) = so_far.last() else {
        return false;
    };
    let glyph = last.glyph.max(next.glyph);
    // Type of one size.
    if last.glyph.min(next.glyph) < glyph * 0.8 {
        return false;
    }
    // The next line down: not the same line again, and not a line further
    // off than the lines of a paragraph are set. Measured from top to top,
    // where a tight box is steadiest — its bottom moves with every `g`.
    let pitch = next.rect.y - last.rect.y;
    if pitch < glyph * 0.6 || pitch > glyph * 1.9 {
        return false;
    }
    // In one column, by whichever edge the paragraph is set from.
    let near = (glyph * 0.5).max(4.0);
    (next.rect.x - last.rect.x).abs() <= near
        || (next.rect.center_x() - last.rect.center_x()).abs() <= near
        || (next.rect.right() - last.rect.right()).abs() <= near
}

/// Whether `next` carries on what the paragraph `so_far` was saying.
///
/// Where lines stand cannot settle this. A sentence wrapped over two lines
/// and two entries of a list look the same to a ruler: one under the other,
/// same type, same edge. Read together, the list becomes one run-on
/// sentence in translation and its words land in the wrong rows; read
/// apart, the sentence is translated in two halves. So the words are asked:
/// a line that starts small, or follows a comma, carries on. A capital after
/// neither is the hard case, and it is decided against joining unless there
/// is something to say otherwise — the line above stopping on a word nothing
/// stops on, or running to the edge of a paragraph whose width is already
/// known. Wrongly parted, a sentence is still translated, in two pieces,
/// where it was; wrongly joined, a menu is neither.
///
/// A line that ends a sentence is the other hard case. In a list every line
/// does; in a speech of two sentences the first does too, and translated
/// apart the two come back in different voices — one addressing a friend
/// and the next a stranger, in a language that tells them apart. What a
/// line of prose has that an entry of a list need not is length: it is
/// several words long, and as long as anything in its paragraph.
fn reads_on(so_far: &[Row], next: &Row) -> bool {
    let Some(last) = so_far.last() else {
        return false;
    };
    let Some(first) = next.text.chars().find(|c| c.is_alphanumeric()) else {
        return false;
    };
    if first.is_lowercase() {
        return true;
    }
    let widest = so_far.iter().map(|row| row.rect.w).fold(0.0, f32::max);
    if last.ends_a_sentence() {
        return last.words >= 4
            && last.rect.w >= widest.max(next.rect.w) * 0.85
            && last.shouts() == next.shouts();
    }
    if last.ends_open() {
        return true;
    }
    if !first.is_uppercase() && !first.is_numeric() {
        // Writing with no capitals to go by: a line that is long, and as
        // long as any line of its paragraph, ran out of room.
        return last.text.chars().count() >= 8
            && last.rect.w >= widest.max(next.rect.w) * 0.85;
    }
    if last.shouts() != next.shouts() {
        return false;
    }
    let ran_out_of_room = so_far.len() >= 2 && last.rect.w >= widest * 0.85;
    ran_out_of_room || (last.words >= 4 && last.ends_unfinished())
}

/// Whether `next` is the second half of a caption that `so_far` is the
/// first half of.
///
/// A caption is broken where its designer liked, and its halves are often
/// set in different sizes: `CLICK TO JOIN OUR` small over `DISCORD` large,
/// `SUPPORT ME` over `ON PATREON`. By where they stand and how big they are
/// those are two things. By what they say they are one, and translated as
/// two they come out wrong — a lone "discord" is a quarrel, not a place. The
/// first half says so by stopping on a word nothing stops on, or the second
/// by starting with a word nothing starts with. Then all that is asked of
/// the two lines is that one is directly under the other.
fn carries_over(so_far: &[Row], next: &Row) -> bool {
    let Some(last) = so_far.last() else {
        return false;
    };
    // More than a word each, or it is a column of values — `On`, `Off`.
    if last.words < 2 || last.shouts() != next.shouts() || last.ends_a_sentence() {
        return false;
    }
    // Two words that stop on a small one are an entry of a menu as often
    // as not: `Sign in`, `Turn on`.
    let left_open = last.words >= 3 && last.ends_unfinished();
    // And a small word with a capital begins a sentence: `To continue,
    // press A`. Only where there are no capitals to go by, or it is small
    // itself, does it join.
    let small = next
        .text
        .chars()
        .find(|c| c.is_alphabetic())
        .is_some_and(char::is_lowercase);
    let joined_on = next.words >= 2 && next.begins_joined() && (next.shouts() || small);
    if !(left_open || joined_on) {
        return false;
    }
    let smaller = last.glyph.min(next.glyph);
    let gap = next.rect.y - last.rect.bottom();
    if gap < -smaller * 0.3 || gap > smaller * 0.9 {
        return false;
    }
    let near = (last.glyph.max(next.glyph) * 0.5).max(4.0);
    (next.rect.x - last.rect.x).abs() <= near
        || (next.rect.center_x() - last.rect.center_x()).abs() <= near
        || (next.rect.right() - last.rect.right()).abs() <= near
}

/// Puts lines together into paragraphs, in reading order.
pub fn blocks(lines: &[WordLine], capture_width: f32) -> Vec<TextBlock> {
    let mut rows: Vec<Row> = lines.iter().filter_map(Row::of).collect();
    rows.sort_by(|a, b| a.rect.y.total_cmp(&b.rect.y).then(a.rect.x.total_cmp(&b.rect.x)));

    let mut paragraphs: Vec<Vec<Row>> = Vec::new();
    for row in rows {
        let home = paragraphs.iter().position(|so_far| {
            (stands_under(so_far, &row) && reads_on(so_far, &row)) || carries_over(so_far, &row)
        });
        match home {
            Some(index) => paragraphs[index].push(row),
            None => paragraphs.push(vec![row]),
        }
    }

    let mut blocks: Vec<TextBlock> = paragraphs
        .iter()
        .filter_map(|rows| {
            let rects: Vec<PxRect> = rows.iter().map(|row| row.rect).collect();
            let rect = rects.iter().copied().reduce(|a, b| a.union(b))?;
            let text = normalize_ocr_text(
                &rows
                    .iter()
                    .map(|row| row.text.as_str())
                    .collect::<Vec<_>>()
                    .join("\n"),
            );
            if text.is_empty() {
                return None;
            }
            let first = rows.first()?;
            let pitch = match (rects.first(), rects.last()) {
                (Some(top), Some(bottom)) if rects.len() >= 2 => {
                    (bottom.y - top.y) / (rects.len() - 1) as f32
                }
                _ => 0.0,
            };
            Some(TextBlock {
                rect,
                text,
                glyph_height: median(rows.iter().map(|row| row.glyph).collect())
                    .unwrap_or(first.glyph),
                lines: rows.len(),
                align: alignment(&rects, rect, capture_width),
                sample: Line {
                    rect: first.rect,
                    text: first.text.clone(),
                },
                pitch: pitch.max(0.0),
            })
        })
        .collect();

    align_columns(&mut blocks);
    in_reading_order(&mut blocks);
    blocks
}

/// Top to bottom, and left to right among paragraphs that start level.
fn in_reading_order(blocks: &mut [TextBlock]) {
    blocks.sort_by(|a, b| a.rect.y.total_cmp(&b.rect.y));
    let mut start = 0;
    while start < blocks.len() {
        // Everything whose first line is level with this one's.
        let level = blocks[start].sample.rect;
        let mut end = start + 1;
        while end < blocks.len() && blocks[end].sample.rect.center_y() < level.bottom() {
            end += 1;
        }
        blocks[start..end].sort_by(|a, b| a.rect.x.total_cmp(&b.rect.x));
        start = end;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::translation::ocr::Align;

    /// A line whose words are spaced the way type of that height is.
    fn line(x: f32, y: f32, h: f32, text: &str) -> WordLine {
        let mut at = x;
        let words = text
            .split(' ')
            .map(|word| {
                let w = word.chars().count() as f32 * h * 0.55;
                let rect = PxRect::new(at, y, w, h);
                at += w + h * 0.3;
                Word {
                    rect,
                    text: word.to_string(),
                }
            })
            .collect();
        WordLine { words }
    }

    /// A line of a given width, for when where it ends is what matters.
    fn boxed(x: f32, y: f32, w: f32, h: f32, text: &str) -> WordLine {
        WordLine {
            words: vec![Word {
                rect: PxRect::new(x, y, w, h),
                text: text.to_string(),
            }],
        }
    }

    fn texts(blocks: &[TextBlock]) -> Vec<&str> {
        blocks.iter().map(|block| block.text.as_str()).collect()
    }

    /// The lines [`super::merged`] keeps, without what it says about them.
    fn merged(readings: Vec<Vec<WordLine>>) -> Vec<WordLine> {
        super::merged(readings)
            .into_iter()
            .map(|kept| kept.line)
            .collect()
    }

    // What the recogniser built into Windows made of a logo, a figure and
    // Japanese it had no model for — every one of them returned as a line.
    const NOISE: &[&str] = &[
        "flt•lfrgtt",
        "r„jll",
        "iRS",
        "0)",
        "furlfr7en",
        "anurn-zn«n",
        "z-L\\ifiA",
        "- • ran •",
        "c",
        "Jqrlfrienct",
        "Welshoulälnötlbemutlhettelafteaanl«.",
    ];

    #[test]
    fn noise_does_not_read_as_writing() {
        for text in NOISE {
            assert!(!sense(text).is_writing(), "{text:?} was taken for writing");
        }
    }

    #[test]
    fn what_an_interface_says_reads_as_writing() {
        for text in [
            "CONTINUE",
            "NEW GAME",
            "Press E to interact",
            "VERSION 0.65",
            "HP: 100/250",
            "Lv. 5",
            "You need the silver key to open this door.",
            "We're out of time, aren't we?",
            "DLSS quality",
            "Привет, мир",
            "Сохранить и выйти",
            "設定",
            "力",
            "ゲームをはじめる",
            "YouTube",
            "[E] Open",
        ] {
            assert!(sense(text).is_writing(), "{text:?} was thrown away");
        }
    }

    #[test]
    fn figures_alone_are_left_as_they_stand() {
        for text in ["0.65", "2560 x 1440", "100", "12:30", "x3"] {
            assert!(!sense(text).is_writing(), "{text:?} was taken for writing");
        }
    }

    #[test]
    fn a_reading_with_something_stuck_to_it_counts_for_less_than_a_clean_one() {
        assert!(sense("SUPPORT ME").share > sense("O SUPPORT ME").share);
        assert!(sense("CREDITS").share > sense("(CREDITS").share);
        // But brackets that close are part of the writing.
        assert_eq!(sense("Save (recommended)").share, 1.0);
    }

    #[test]
    fn the_cleanest_reading_of_a_place_is_the_one_kept() {
        // The picture as it was, and with everything but light strokes
        // taken out of it: what each made of the same four buttons.
        let plain = vec![
            boxed(413.0, 634.0, 239.0, 34.0, "CONTINUE"),
            boxed(392.0, 1057.0, 223.0, 53.0, "(CREDITS"),
            boxed(2105.0, 98.0, 343.0, 67.0, "O SUPPORT ME"),
            boxed(359.0, 368.0, 426.0, 157.0, "flt•lfrgtt"),
        ];
        let strokes = vec![
            boxed(413.0, 633.0, 239.0, 35.0, "CONTINUE"),
            boxed(412.0, 847.0, 233.0, 35.0, "SETTINGS"),
            boxed(413.0, 1061.0, 202.0, 34.0, "CREDITS"),
            boxed(2184.0, 100.0, 264.0, 35.0, "SUPPORT ME"),
        ];
        let lines = merged(vec![plain, strokes]);
        let mut read: Vec<String> = lines.iter().map(WordLine::text).collect();
        read.sort();
        assert_eq!(read, ["CONTINUE", "CREDITS", "SETTINGS", "SUPPORT ME"]);
        // Read alike by both: the first reading's box is the one kept.
        let again = lines.iter().find(|l| l.text() == "CONTINUE").unwrap();
        assert_eq!(again.rect().y, 634.0);
    }

    #[test]
    fn a_whole_line_is_kept_over_the_pieces_another_reading_made_of_it() {
        let whole = vec![boxed(389.0, 200.0, 424.0, 26.0, "I told you not to come back here.")];
        let pieces = vec![
            boxed(404.0, 199.0, 32.0, 21.0, "tol"),
            boxed(606.0, 199.0, 207.0, 21.0, "come back here."),
        ];
        // Whichever is looked at first.
        for readings in [vec![whole.clone(), pieces.clone()], vec![pieces, whole]] {
            let lines = merged(readings);
            assert_eq!(lines.len(), 1);
            assert_eq!(lines[0].text(), "I told you not to come back here.");
        }
    }

    #[test]
    fn readings_agree_give_or_take_what_changes_between_two_looks() {
        assert!(agree("DISCORD", "Discord"));
        assert!(agree("VERSION 0.65", "VERSION0.65"));
        assert!(agree("The bridge is out.", "The bridge'is out"));
        assert!(agree("GAME OVER", "GAME 0VER"));
        assert!(!agree("Witt", "flt"));
        assert!(!agree("Jqrlfrienct", "furifrien"));
        assert!(!agree("CONTINUE", "CREDITS"));
        assert!(!agree("", "anything"));
    }

    #[test]
    fn large_type_read_only_once_is_in_doubt_and_read_twice_is_not() {
        // A logo, as two readings made of it; and a title both could read.
        let plain = vec![
            boxed(359.0, 368.0, 426.0, 157.0, "flt•lfrgtt"),
            boxed(2181.0, 336.0, 322.0, 68.0, "DISCORD"),
            boxed(413.0, 634.0, 239.0, 34.0, "CONTINUE"),
        ];
        let strokes = vec![
            boxed(474.0, 222.0, 141.0, 54.0, "Witt"),
            boxed(366.0, 352.0, 498.0, 118.0, "Furlfriend"),
            boxed(2181.0, 336.0, 323.0, 68.0, "DISCORD"),
        ];
        let kept = super::merged(vec![plain, strokes]);
        let doubtful: Vec<String> = kept
            .iter()
            .filter(|line| line.is_doubtful())
            .map(|line| line.line.text())
            .collect();
        assert_eq!(doubtful, ["Furlfriend", "Witt"]);
        // Type of an ordinary size is believed on one reading.
        assert!(kept.iter().any(|line| line.line.text() == "CONTINUE" && !line.confirmed));
    }

    #[test]
    fn a_line_in_doubt_is_borne_out_by_the_same_words_in_the_same_place() {
        let title = Kept {
            line: boxed(400.0, 300.0, 900.0, 150.0, "GAME OVER"),
            confirmed: false,
        };
        assert!(title.is_doubtful());
        assert!(title.is_borne_out_by(&[boxed(404.0, 296.0, 890.0, 156.0, "GAME OVER")]));
        // Other words there, or the same words elsewhere, bear nothing out.
        assert!(!title.is_borne_out_by(&[boxed(404.0, 296.0, 890.0, 156.0, "CAME QVFR")]));
        assert!(!title.is_borne_out_by(&[boxed(404.0, 900.0, 890.0, 156.0, "GAME OVER")]));
        assert!(!title.is_borne_out_by(&[]));
    }

    #[test]
    fn lines_one_above_the_other_are_not_readings_of_one_place() {
        let lines = merged(vec![vec![
            line(44.0, 118.0, 24.0, "You need the silver key"),
            line(44.0, 140.0, 24.0, "to open this door."),
        ]]);
        assert_eq!(lines.len(), 2);
    }

    #[test]
    fn a_letter_read_in_two_halves_is_put_back_together() {
        let lines = merged(vec![vec![line(484.0, 310.0, 33.0, "I<eep close and stay quiet.")]]);
        assert_eq!(lines[0].text(), "Keep close and stay quiet.");
    }

    #[test]
    fn the_end_of_a_button_read_as_a_bracket_is_taken_off() {
        let lines = merged(vec![vec![boxed(392.0, 1057.0, 224.0, 53.0, "(CREDITS")]]);
        assert_eq!(lines[0].text(), "CREDITS");
        // And the box starts nearer to where the word does.
        assert_eq!(lines[0].rect().x, 406.0);
        assert_eq!(lines[0].rect().right(), 616.0);
        // Brackets that pair up are writing.
        let paired = merged(vec![vec![line(0.0, 0.0, 20.0, "Save (recommended)")]]);
        assert_eq!(paired[0].text(), "Save (recommended)");
    }

    #[test]
    fn a_speck_read_as_a_quotation_mark_between_two_words_parts_them() {
        let lines = merged(vec![vec![line(458.0, 1753.0, 29.0, "We have to find another«way across.")]]);
        assert_eq!(lines[0].text(), "We have to find another way across.");
        // Around a word, it is what it looks like.
        let quoted = merged(vec![vec![line(0.0, 0.0, 20.0, "Он сказал «привет» и ушёл")]]);
        assert_eq!(quoted[0].text(), "Он сказал «привет» и ушёл");
    }

    #[test]
    fn a_label_and_a_value_at_the_ends_of_a_row_are_two_pieces() {
        let row = WordLine {
            words: vec![
                Word { rect: PxRect::new(84.0, 116.0, 87.0, 14.0), text: "Resolution".into() },
                Word { rect: PxRect::new(819.0, 117.0, 98.0, 13.0), text: "High".into() },
            ],
        };
        let lines = merged(vec![vec![row]]);
        let mut read: Vec<String> = lines.iter().map(WordLine::text).collect();
        read.sort();
        assert_eq!(read, ["High", "Resolution"]);
    }

    #[test]
    fn words_are_a_space_apart_except_where_writing_has_no_spaces() {
        let latin = line(0.0, 0.0, 20.0, "Press E to interact");
        assert_eq!(latin.text(), "Press E to interact");
        let japanese = line(0.0, 0.0, 20.0, "峠 は 春 ま で 。");
        assert_eq!(japanese.text(), "峠は春まで。");
        // Korean does put spaces between its words.
        let korean = line(0.0, 0.0, 20.0, "아무 키나 누르세요");
        assert_eq!(korean.text(), "아무 키나 누르세요");
    }

    #[test]
    fn type_of_one_size_measures_the_same_whatever_letters_it_holds() {
        // The tight boxes of one column of labels, as they were returned.
        let shadows = full_height("Shadows", 14.0);
        let quality = full_height("Texture quality", 18.0);
        let high = full_height("High", 18.0);
        for height in [shadows, quality, high] {
            assert!((height - 18.0).abs() < 0.5, "{height}");
        }
        // And two lines of one piece of dialogue.
        let first = full_height("The pass is closed until spring.", 22.0);
        let second = full_height("You will have to wait here.", 17.0);
        assert!((first - second).abs() < 0.5, "{first} against {second}");
    }

    #[test]
    fn a_sentence_wrapped_over_two_lines_is_one_paragraph() {
        let found = blocks(
            &[
                line(45.0, 158.0, 24.0, "Speak to the old merchant in the village"),
                line(46.0, 198.0, 24.0, "before nightfall."),
            ],
            900.0,
        );
        assert_eq!(texts(&found), ["Speak to the old merchant in the village before nightfall."]);
        assert_eq!(found[0].lines, 2);
        assert_eq!(found[0].pitch, 40.0);
        assert_eq!(found[0].sample.text, "Speak to the old merchant in the village");
    }

    #[test]
    fn two_sentences_of_one_speech_are_one_paragraph() {
        // Each on a line of its own, the way a dialogue box sets them.
        let found = blocks(
            &[
                line(44.0, 121.0, 22.0, "The pass is closed until spring."),
                // Nothing in it hangs below the line: a shorter box.
                line(44.0, 161.0, 17.0, "You will have to wait here."),
            ],
            900.0,
        );
        assert_eq!(
            texts(&found),
            ["The pass is closed until spring. You will have to wait here."]
        );
        assert_eq!(found[0].lines, 2);
    }

    #[test]
    fn a_short_sentence_above_a_longer_one_is_an_entry_of_a_list() {
        let found = blocks(
            &[
                line(44.0, 121.0, 22.0, "Save your progress now."),
                line(44.0, 161.0, 22.0, "Load a game you saved before from the disk."),
            ],
            900.0,
        );
        assert_eq!(
            texts(&found),
            ["Save your progress now.", "Load a game you saved before from the disk."]
        );
        // A word or two and a full stop is a button, not prose.
        let buttons = blocks(
            &[
                line(44.0, 121.0, 22.0, "Saved."),
                line(44.0, 161.0, 22.0, "Continue"),
            ],
            900.0,
        );
        assert_eq!(buttons.len(), 2);
    }

    #[test]
    fn the_entries_of_a_menu_are_not_read_as_one_sentence() {
        // Close set, one under another, one size, one edge — and three
        // separate things to choose from.
        let found = blocks(
            &[
                line(96.0, 280.0, 17.0, "Rent a room"),
                line(96.0, 306.0, 17.0, "Ask about the road north"),
                line(96.0, 332.0, 17.0, "Leave"),
            ],
            1100.0,
        );
        assert_eq!(texts(&found), ["Rent a room", "Ask about the road north", "Leave"]);

        let shouted = blocks(
            &[
                line(54.0, 54.0, 34.0, "NEW GAME"),
                line(54.0, 100.0, 34.0, "LOAD GAME"),
            ],
            600.0,
        );
        assert_eq!(texts(&shouted), ["NEW GAME", "LOAD GAME"]);

        // An entry that ends on a word sentences do not end on.
        let account = blocks(
            &[
                line(96.0, 280.0, 17.0, "Sign in"),
                line(96.0, 306.0, 17.0, "Create an account"),
            ],
            1100.0,
        );
        assert_eq!(texts(&account), ["Sign in", "Create an account"]);
    }

    #[test]
    fn a_line_that_ran_to_the_edge_of_its_paragraph_carries_on_into_a_capital() {
        let found = blocks(
            &[
                // No letter of the first line reaches below it, so its box
                // is that much shorter: the same type all the same.
                line(40.0, 100.0, 16.0, "The road north has been closed since"),
                line(40.0, 130.0, 20.0, "the first snow, and the innkeeper at"),
                line(40.0, 160.0, 20.0, "Ravenholm says it stays that way."),
            ],
            900.0,
        );
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].lines, 3);
    }

    #[test]
    fn a_long_line_followed_by_a_capital_is_still_one_sentence() {
        let found = blocks(
            &[
                line(40.0, 100.0, 22.0, "Yesterday the council decided that"),
                line(40.0, 134.0, 22.0, "Aloy must leave the valley."),
            ],
            900.0,
        );
        assert_eq!(texts(&found), ["Yesterday the council decided that Aloy must leave the valley."]);

        let shouted = blocks(
            &[
                line(40.0, 100.0, 22.0, "PRESS AND HOLD THE BUTTON TO"),
                line(40.0, 134.0, 22.0, "CHARGE YOUR ATTACK"),
            ],
            900.0,
        );
        assert_eq!(texts(&shouted), ["PRESS AND HOLD THE BUTTON TO CHARGE YOUR ATTACK"]);
    }

    #[test]
    fn a_caption_broken_over_two_lines_is_one_caption_whatever_their_sizes() {
        // As they stood on a title screen: small over large.
        let found = blocks(
            &[
                boxed(2178.0, 289.0, 322.0, 28.0, "CLICK TO JOIN OUR"),
                boxed(2181.0, 336.0, 322.0, 68.0, "DISCORD"),
            ],
            2560.0,
        );
        assert_eq!(texts(&found), ["CLICK TO JOIN OUR DISCORD"]);
        assert_eq!(found[0].lines, 2);
        // Measured off its first line, which is the one it starts in.
        assert_eq!(found[0].sample.text, "CLICK TO JOIN OUR");

        // And a second line that begins the way only a second line does.
        let found = blocks(
            &[
                boxed(2184.0, 100.0, 264.0, 35.0, "SUPPORT ME"),
                boxed(2185.0, 151.0, 261.0, 35.0, "ON PATREON"),
            ],
            2560.0,
        );
        assert_eq!(texts(&found), ["SUPPORT ME ON PATREON"]);
    }

    #[test]
    fn a_column_of_values_is_not_a_caption() {
        let found = blocks(
            &[
                boxed(877.0, 164.0, 39.0, 14.0, "High"),
                boxed(885.0, 186.0, 31.0, 14.0, "On"),
                boxed(880.0, 208.0, 36.0, 14.0, "Off"),
            ],
            1000.0,
        );
        assert_eq!(texts(&found), ["High", "On", "Off"]);
        // Nor are two entries because the second begins with a small word.
        let found = blocks(
            &[
                boxed(96.0, 280.0, 120.0, 17.0, "Sound effects"),
                boxed(96.0, 306.0, 20.0, 17.0, "On"),
            ],
            1000.0,
        );
        assert_eq!(found.len(), 2);
        // Nor lines that are far apart, whatever they end and begin with.
        let found = blocks(
            &[
                boxed(96.0, 280.0, 220.0, 17.0, "With thanks to"),
                boxed(96.0, 380.0, 200.0, 17.0, "everyone involved"),
            ],
            1000.0,
        );
        assert_eq!(found.len(), 2);
        // Nor an entry and the sentence under it, in another size.
        let found = blocks(
            &[
                boxed(96.0, 280.0, 90.0, 24.0, "Log in"),
                boxed(96.0, 310.0, 260.0, 14.0, "To continue, press A"),
            ],
            1000.0,
        );
        assert_eq!(found.len(), 2);
    }

    #[test]
    fn a_heading_is_not_run_into_the_text_under_it() {
        let found = blocks(
            &[
                line(46.0, 46.0, 30.0, "Old Merchant"),
                line(44.0, 90.0, 22.0, "the pass is closed"),
            ],
            900.0,
        );
        assert_eq!(found.len(), 2);
    }

    #[test]
    fn lines_too_far_apart_are_separate_however_they_read() {
        let found = blocks(
            &[
                line(84.0, 116.0, 18.0, "the first of them"),
                line(84.0, 164.0, 18.0, "the second of them"),
            ],
            1000.0,
        );
        assert_eq!(found.len(), 2);
    }

    #[test]
    fn two_columns_are_read_row_by_row() {
        // In the order the recogniser gave them: one column, then the other.
        let found = blocks(
            &[
                boxed(84.0, 116.0, 87.0, 14.0, "Resolution"),
                boxed(83.0, 164.0, 125.0, 18.0, "Texture quality"),
                boxed(877.0, 164.0, 39.0, 14.0, "Ultra"),
                boxed(844.0, 117.0, 71.0, 14.0, "Disabled"),
            ],
            1000.0,
        );
        assert_eq!(texts(&found), ["Resolution", "Disabled", "Texture quality", "Ultra"]);
        // And the values hang from their right edge.
        assert_eq!(found[1].align, Align::Right);
        assert_eq!(found[3].align, Align::Right);
        assert_eq!(found[0].align, Align::Left);
    }

    #[test]
    fn japanese_wrapped_at_the_edge_of_its_box_is_one_paragraph() {
        let found = blocks(
            &[
                boxed(40.0, 100.0, 520.0, 26.0, "峠は春まで閉ざされているので、ここで"),
                boxed(40.0, 140.0, 300.0, 26.0, "待つしかありません。"),
            ],
            900.0,
        );
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].text, "峠は春まで閉ざされているので、ここで待つしかありません。");
    }

    #[test]
    fn nothing_in_means_nothing_out() {
        assert!(blocks(&[], 900.0).is_empty());
        assert!(merged(Vec::new()).is_empty());
        assert!(merged(vec![vec![WordLine::default()]]).is_empty());
        assert!(!agree("", ""));
    }
}
