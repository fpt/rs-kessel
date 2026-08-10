//! The sound bank: instruments and sound effects, and the grammar that
//! describes them.
//!
//! ```text
//! instrument kick {
//!   wave = sine
//!   attack = 0  decay = 90  sustain = 0
//!   pitch_env = 36  pitch_decay = 60
//! }
//!
//! sfx boom {
//!   inst = kick
//!   speed = 2                    -- frames per row
//!   notes = "48 - 43 40 . 36"    -- note number, `-` hold, `.` rest
//! }
//! ```
//!
//! ## Why the grammar lives here and not in the compiler
//!
//! A standalone instrument has to read a patch file, and if that grammar lived
//! in `luax.rs` the synth app would have to link the game compiler to do it.
//! So the *meaning* of every key — its name, its accepted spellings, its range
//! — is defined once, here, in [`set_instrument_field`] and [`set_sfx_field`].
//!
//! What is **not** shared is tokenization. `luax` lexes these blocks with its
//! own lexer and calls the setters above; [`parse`] has a small tokenizer of
//! its own for standalone patch files. The alternative — handing `luax`'s block
//! text to [`parse`] — needs byte spans that the luax lexer does not carry, and
//! adding spans to it touches every rule in the compiler. Two tokenizers over
//! one definition of meaning is the cheaper half of that trade.

use crate::filter::FilterMode;
use crate::patch::{Patch, Waveform};

/// What one row of an [`SfxDef`] does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Row {
    /// Start this note.
    Note(u8),
    /// Keep the previous note sounding for another row.
    Hold,
    /// Silence — end whatever was sounding.
    Rest,
}

/// A sound effect: a short line of notes on one instrument.
///
/// Not a tracker pattern. A pattern has channels and effects and belongs with
/// the sequencer; this is the thing `sfx(boom)` plays, and it is one voice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SfxDef {
    pub inst: u8,
    /// Frames per row. At 60 fps, `speed = 2` is a 30 Hz arpeggio.
    pub speed: u8,
    pub vel: u8,
    pub rows: Vec<Row>,
}

impl Default for SfxDef {
    fn default() -> Self {
        SfxDef {
            inst: 0,
            speed: 4,
            vel: 255,
            rows: Vec::new(),
        }
    }
}

impl SfxDef {
    /// How long this effect lasts, in console frames.
    pub fn frames(&self) -> u32 {
        self.rows.len() as u32 * self.speed.max(1) as u32
    }

    /// Walk the effect as `(frame offset, note, length in frames)`.
    ///
    /// A note followed by holds becomes **one** longer note rather than a
    /// retrigger per row — that is what makes `"48 - - -"` a sustained hit and
    /// `"48 48 48 48"` a machine gun, and both are things a game wants.
    ///
    /// Allocation-free: the engine calls this while scheduling.
    pub fn notes(&self) -> impl Iterator<Item = (u32, u8, u16)> + '_ {
        let speed = self.speed.max(1) as u32;
        // Index each run by the row it starts on.
        (0..self.rows.len()).filter_map(move |i| {
            let Row::Note(note) = self.rows[i] else {
                return None;
            };
            let mut len = 1u32;
            while self.rows.get(i + len as usize) == Some(&Row::Hold) {
                len += 1;
            }
            Some((
                i as u32 * speed,
                note,
                (len * speed).min(u16::MAX as u32) as u16,
            ))
        })
    }
}

/// Everything a ROM (or a patch file) says about sound.
///
/// Carried as metadata beside the ROM, the way `controls` and `screen` are —
/// not assembled into the 64 KiB space. A host reads it once at load time and
/// hands it to the engine.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SoundBank {
    pub instruments: Vec<Patch>,
    pub sfx: Vec<SfxDef>,
    /// Declaration-order names, for `bank.instrument_id("kick")` and for a
    /// host UI that wants to list what a ROM can play.
    pub instrument_names: Vec<String>,
    pub sfx_names: Vec<String>,
}

impl SoundBank {
    pub fn is_empty(&self) -> bool {
        self.instruments.is_empty() && self.sfx.is_empty()
    }

    pub fn instrument_id(&self, name: &str) -> Option<u8> {
        self.instrument_names
            .iter()
            .position(|n| n == name)?
            .try_into()
            .ok()
    }

    pub fn sfx_id(&self, name: &str) -> Option<u16> {
        self.sfx_names
            .iter()
            .position(|n| n == name)?
            .try_into()
            .ok()
    }

    /// Append an instrument, returning its id.
    pub fn add_instrument(&mut self, name: impl Into<String>, patch: Patch) -> u8 {
        let id = self.instruments.len() as u8;
        self.instruments.push(patch);
        self.instrument_names.push(name.into());
        id
    }

    /// Append a sound effect, returning its id.
    pub fn add_sfx(&mut self, name: impl Into<String>, def: SfxDef) -> u16 {
        let id = self.sfx.len() as u16;
        self.sfx.push(def);
        self.sfx_names.push(name.into());
        id
    }
}

/// The most instruments and effects a bank can hold.
///
/// Instrument ids are a byte on the wire (`AudioEvent::Play`), so 256 is the
/// hard ceiling rather than a policy.
pub const MAX_INSTRUMENTS: usize = 256;
/// Effects are addressed by `u16`, but a limit this side of absurd keeps a
/// runaway generator from producing a bank nothing can load.
pub const MAX_SFX: usize = 1024;
/// Rows in one effect. Long enough for a two-second arpeggio at speed 1.
pub const MAX_SFX_ROWS: usize = 128;

/// A value, as either tokenizer sees it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FieldValue<'a> {
    Int(i64),
    /// A bare word: `sine`, `lpf`, or another declaration's name.
    Word(&'a str),
    /// A `"..."` literal.
    Text(&'a str),
}

impl FieldValue<'_> {
    fn int(&self, key: &str) -> Result<i64, String> {
        match self {
            FieldValue::Int(n) => Ok(*n),
            _ => Err(format!("'{key}' expects a number")),
        }
    }

    /// An integer that has to fit a range, reported with the range in the
    /// message — a model that gets this wrong can fix it from the text alone.
    fn ranged(&self, key: &str, lo: i64, hi: i64) -> Result<i64, String> {
        let n = self.int(key)?;
        if (lo..=hi).contains(&n) {
            Ok(n)
        } else {
            Err(format!("'{key}' is {n}, outside {lo}..={hi}"))
        }
    }

    fn word(&self, key: &str) -> Result<&str, String> {
        match self {
            FieldValue::Word(w) => Ok(w),
            FieldValue::Text(t) => Ok(t),
            FieldValue::Int(_) => Err(format!("'{key}' expects a name")),
        }
    }
}

impl Waveform {
    pub fn from_name(name: &str) -> Option<Waveform> {
        Some(match name.to_ascii_lowercase().as_str() {
            "sine" => Waveform::Sine,
            "triangle" | "tri" => Waveform::Triangle,
            "saw" => Waveform::Saw,
            "square" | "pulse" => Waveform::Square,
            "noise" => Waveform::Noise,
            _ => return None,
        })
    }
}

impl FilterMode {
    pub fn from_name(name: &str) -> Option<FilterMode> {
        Some(match name.to_ascii_lowercase().as_str() {
            "off" | "none" => FilterMode::Off,
            "lpf" | "lowpass" => FilterMode::Lpf,
            "hpf" | "highpass" => FilterMode::Hpf,
            _ => return None,
        })
    }
}

/// Every key an `instrument` block accepts, in the order a patch is usually
/// written. Used for the "unknown key" message, so the suggestion never drifts
/// from what the setter actually handles.
pub const INSTRUMENT_KEYS: &[&str] = &[
    "wave",
    "attack",
    "decay",
    "sustain",
    "release",
    "pitch_env",
    "pitch_decay",
    "filter",
    "cutoff",
    "resonance",
    "distortion",
    "volume",
    "pan",
];

/// Apply one `key = value` pair to an instrument.
///
/// The single definition of what an instrument key means. Both the standalone
/// parser and the luax front-end call this.
pub fn set_instrument_field(p: &mut Patch, key: &str, v: FieldValue) -> Result<(), String> {
    match key {
        "wave" => {
            let name = v.word(key)?;
            p.wave = Waveform::from_name(name).ok_or_else(|| {
                format!("unknown wave '{name}' (sine, triangle, saw, square, noise)")
            })?;
        }
        "attack" => p.attack_ms = v.ranged(key, 0, 60_000)? as u16,
        "decay" => p.decay_ms = v.ranged(key, 0, 60_000)? as u16,
        "sustain" => p.sustain = v.ranged(key, 0, 255)? as u8,
        "release" => p.release_ms = v.ranged(key, 0, 60_000)? as u16,
        "pitch_env" => p.pitch_env = v.ranged(key, -127, 127)? as i8,
        "pitch_decay" => p.pitch_decay_ms = v.ranged(key, 0, 60_000)? as u16,
        "filter" => {
            let name = v.word(key)?;
            p.filter = FilterMode::from_name(name)
                .ok_or_else(|| format!("unknown filter '{name}' (off, lpf, hpf)"))?;
        }
        "cutoff" => p.cutoff = v.ranged(key, 0, 255)? as u8,
        "resonance" => p.resonance = v.ranged(key, 0, 255)? as u8,
        "distortion" => p.distortion = v.ranged(key, 0, 255)? as u8,
        "volume" => p.volume = v.ranged(key, 0, 255)? as u8,
        "pan" => p.pan = v.ranged(key, -127, 127)? as i8,
        other => {
            return Err(format!(
                "unknown instrument key '{other}' (expected {})",
                INSTRUMENT_KEYS.join(", ")
            ))
        }
    }
    Ok(())
}

/// Every key an `sfx` block accepts.
pub const SFX_KEYS: &[&str] = &["inst", "speed", "vel", "notes"];

/// Apply one `key = value` pair to a sound effect.
///
/// `instrument` resolves an instrument name to its id; it is a callback because
/// the two front-ends track declaration order differently.
pub fn set_sfx_field(
    s: &mut SfxDef,
    key: &str,
    v: FieldValue,
    instrument: &dyn Fn(&str) -> Option<u8>,
) -> Result<(), String> {
    match key {
        "inst" => match v {
            // A bare id is allowed but a name is the point: `inst = kick`
            // survives someone inserting an instrument above it.
            FieldValue::Int(n) => {
                s.inst = (0..=255)
                    .contains(&n)
                    .then_some(n as u8)
                    .ok_or(format!("'inst' is {n}, outside 0..=255"))?
            }
            _ => {
                let name = v.word(key)?;
                s.inst = instrument(name).ok_or_else(|| format!("no instrument named '{name}'"))?;
            }
        },
        "speed" => s.speed = v.ranged(key, 1, 255)? as u8,
        "vel" => s.vel = v.ranged(key, 0, 255)? as u8,
        "notes" => s.rows = parse_rows(v.word(key)?)?,
        other => {
            return Err(format!(
                "unknown sfx key '{other}' (expected {})",
                SFX_KEYS.join(", ")
            ))
        }
    }
    Ok(())
}

/// `"48 - 43 . 36"` → rows.
pub fn parse_rows(src: &str) -> Result<Vec<Row>, String> {
    let mut rows = Vec::new();
    for word in src.split_whitespace() {
        if rows.len() == MAX_SFX_ROWS {
            return Err(format!("too many notes (limit {MAX_SFX_ROWS})"));
        }
        rows.push(match word {
            "-" => Row::Hold,
            "." => Row::Rest,
            n => {
                let n: i64 = n
                    .parse()
                    .map_err(|_| format!("'{n}' is not a note, '-' (hold), or '.' (rest)"))?;
                if !(0..=127).contains(&n) {
                    return Err(format!("note {n} is outside 0..=127"));
                }
                Row::Note(n as u8)
            }
        });
    }
    Ok(rows)
}

/// A problem with a bank, with the line it was on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BankError {
    pub line: usize,
    pub message: String,
}

/// Parse a standalone patch file into a bank.
///
/// This is what a synth app loads. The luax front-end does *not* call it — it
/// lexes with its own lexer and calls the field setters — but both end up
/// applying the same rules to the same `Patch`.
///
/// Errors accumulate: a file with three mistakes reports three.
pub fn parse(src: &str) -> (SoundBank, Vec<BankError>) {
    let mut bank = SoundBank::default();
    let mut errors = Vec::new();
    let mut lex = Lexer::new(src);

    loop {
        let (word, line) = match lex.next_top() {
            Some(Ok(w)) => w,
            Some(Err((c, line))) => {
                errors.push(BankError {
                    line,
                    message: format!("unexpected '{c}'"),
                });
                continue;
            }
            None => break,
        };
        match word.as_str() {
            "instrument" | "sfx" => {
                let is_inst = word == "instrument";
                let Some(Ok((name, _))) = lex.next_top() else {
                    errors.push(BankError {
                        line,
                        message: format!("'{word}' needs a name"),
                    });
                    break;
                };
                let fields = match lex.block(line, &mut errors) {
                    Some(f) => f,
                    None => break,
                };
                if is_inst {
                    let mut patch = Patch::default();
                    for (key, value, kline) in &fields {
                        let v = value.as_field();
                        if let Err(message) = set_instrument_field(&mut patch, key, v) {
                            errors.push(BankError {
                                line: *kline,
                                message,
                            });
                        }
                    }
                    if bank.instruments.len() >= MAX_INSTRUMENTS {
                        errors.push(BankError {
                            line,
                            message: format!("too many instruments (limit {MAX_INSTRUMENTS})"),
                        });
                    } else if bank.instrument_id(&name).is_some() {
                        errors.push(BankError {
                            line,
                            message: format!("duplicate instrument '{name}'"),
                        });
                    } else {
                        bank.add_instrument(name, patch);
                    }
                } else {
                    let mut def = SfxDef::default();
                    let names = bank.instrument_names.clone();
                    let resolve = |n: &str| names.iter().position(|x| x == n).map(|i| i as u8);
                    for (key, value, kline) in &fields {
                        let v = value.as_field();
                        if let Err(message) = set_sfx_field(&mut def, key, v, &resolve) {
                            errors.push(BankError {
                                line: *kline,
                                message,
                            });
                        }
                    }
                    if bank.sfx.len() >= MAX_SFX {
                        errors.push(BankError {
                            line,
                            message: format!("too many sound effects (limit {MAX_SFX})"),
                        });
                    } else if bank.sfx_id(&name).is_some() {
                        errors.push(BankError {
                            line,
                            message: format!("duplicate sfx '{name}'"),
                        });
                    } else {
                        bank.add_sfx(name, def);
                    }
                }
            }
            other => {
                errors.push(BankError {
                    line,
                    message: format!("expected 'instrument' or 'sfx', found '{other}'"),
                });
                // Skip to the end of whatever this was, so one stray word does
                // not turn into an error per remaining token.
                lex.skip_block();
            }
        }
    }
    (bank, errors)
}

/// A [`FieldValue`] that owns its text.
///
/// Public because the luax front-end parks parsed fields in its declaration
/// list before the compiler pass applies them, and it should not have to
/// invent a second copy of this enum to do it.
#[derive(Debug, Clone, PartialEq)]
pub enum OwnedValue {
    Int(i64),
    Word(String),
    Text(String),
}

impl OwnedValue {
    pub fn as_field(&self) -> FieldValue<'_> {
        match self {
            OwnedValue::Int(n) => FieldValue::Int(*n),
            OwnedValue::Word(w) => FieldValue::Word(w),
            OwnedValue::Text(t) => FieldValue::Text(t),
        }
    }
}

/// A tokenizer for standalone patch files only. Deliberately tiny: words,
/// integers, `"..."`, braces, `=`, `,`, and `--` comments.
struct Lexer<'a> {
    src: &'a [u8],
    i: usize,
    line: usize,
}

impl<'a> Lexer<'a> {
    fn new(src: &'a str) -> Self {
        Lexer {
            src: src.as_bytes(),
            i: 0,
            line: 1,
        }
    }

    fn skip_trivia(&mut self) {
        while self.i < self.src.len() {
            let c = self.src[self.i];
            if c == b'\n' {
                self.line += 1;
                self.i += 1;
            } else if c.is_ascii_whitespace() || c == b',' {
                self.i += 1;
            } else if c == b'-' && self.src.get(self.i + 1) == Some(&b'-') {
                while self.i < self.src.len() && self.src[self.i] != b'\n' {
                    self.i += 1;
                }
            } else {
                return;
            }
        }
    }

    fn starts_word(c: u8) -> bool {
        c.is_ascii_alphabetic() || c == b'_'
    }

    /// Read the word at the cursor. The caller has checked that one starts here.
    fn word_here(&mut self) -> String {
        let start = self.i;
        while self
            .src
            .get(self.i)
            .is_some_and(|c| c.is_ascii_alphanumeric() || *c == b'_')
        {
            self.i += 1;
        }
        String::from_utf8_lossy(&self.src[start..self.i]).into_owned()
    }

    /// The next thing at the top level: a word, or a character that has no
    /// business being there.
    ///
    /// Skipping stray punctuation silently would be worse than it sounds — a
    /// skipper looking for the next word walks straight through a `}`, so one
    /// misplaced character inside a block would swallow the declaration after
    /// it and report nothing.
    fn next_top(&mut self) -> Option<Result<(String, usize), (char, usize)>> {
        self.skip_trivia();
        let &c = self.src.get(self.i)?;
        let line = self.line;
        if Self::starts_word(c) {
            Some(Ok((self.word_here(), line)))
        } else {
            self.i += 1;
            Some(Err((c as char, line)))
        }
    }

    /// `{ key = value ... }`, returning the pairs and the line each was on.
    fn block(
        &mut self,
        open_line: usize,
        errors: &mut Vec<BankError>,
    ) -> Option<Vec<(String, OwnedValue, usize)>> {
        self.skip_trivia();
        if self.src.get(self.i) != Some(&b'{') {
            errors.push(BankError {
                line: self.line,
                message: "expected '{'".to_string(),
            });
            return None;
        }
        self.i += 1;

        let mut out = Vec::new();
        loop {
            self.skip_trivia();
            match self.src.get(self.i) {
                None => {
                    errors.push(BankError {
                        line: open_line,
                        message: "unclosed '{'".to_string(),
                    });
                    return Some(out);
                }
                Some(b'}') => {
                    self.i += 1;
                    return Some(out);
                }
                _ => {}
            }
            let line = self.line;
            let Some(&c) = self.src.get(self.i) else {
                errors.push(BankError {
                    line: open_line,
                    message: "unclosed '{'".to_string(),
                });
                return Some(out);
            };
            if !Self::starts_word(c) {
                errors.push(BankError {
                    line,
                    message: format!("expected a key, found '{}'", c as char),
                });
                self.i += 1;
                continue;
            }
            let key = self.word_here();
            self.skip_trivia();
            if self.src.get(self.i) == Some(&b'=') {
                self.i += 1;
            } else {
                errors.push(BankError {
                    line,
                    message: format!("expected '=' after '{key}'"),
                });
            }
            match self.value() {
                Some(v) => out.push((key, v, line)),
                None => errors.push(BankError {
                    line,
                    message: format!("'{key}' has no value"),
                }),
            }
        }
    }

    fn value(&mut self) -> Option<OwnedValue> {
        self.skip_trivia();
        let c = *self.src.get(self.i)?;
        if c == b'"' {
            self.i += 1;
            let start = self.i;
            while self.i < self.src.len() && self.src[self.i] != b'"' {
                if self.src[self.i] == b'\n' {
                    self.line += 1;
                }
                self.i += 1;
            }
            let text = String::from_utf8_lossy(&self.src[start..self.i]).into_owned();
            self.i += 1; // closing quote (or EOF, which the caller reports)
            return Some(OwnedValue::Text(text));
        }
        if c.is_ascii_digit()
            || (c == b'-' && self.src.get(self.i + 1).is_some_and(u8::is_ascii_digit))
        {
            let start = self.i;
            self.i += 1;
            while self.src.get(self.i).is_some_and(u8::is_ascii_digit) {
                self.i += 1;
            }
            let text = String::from_utf8_lossy(&self.src[start..self.i]);
            return text.parse().ok().map(OwnedValue::Int);
        }
        if c.is_ascii_alphabetic() || c == b'_' {
            let start = self.i;
            while self
                .src
                .get(self.i)
                .is_some_and(|c| c.is_ascii_alphanumeric() || *c == b'_')
            {
                self.i += 1;
            }
            return Some(OwnedValue::Word(
                String::from_utf8_lossy(&self.src[start..self.i]).into_owned(),
            ));
        }
        None
    }

    /// Recovery: swallow a `{ ... }` if one starts here.
    fn skip_block(&mut self) {
        self.skip_trivia();
        if self.src.get(self.i) != Some(&b'{') {
            return;
        }
        let mut depth = 0;
        while self.i < self.src.len() {
            match self.src[self.i] {
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        self.i += 1;
                        return;
                    }
                }
                b'\n' => self.line += 1,
                _ => {}
            }
            self.i += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRC: &str = r#"
-- a drum and a hit
instrument kick {
  wave = sine
  attack = 0  decay = 90  sustain = 0
  pitch_env = 36  pitch_decay = 60
  volume = 255
}

instrument lead {
  wave = square
  filter = lpf  cutoff = 160  resonance = 40
  pan = -40
}

sfx boom {
  inst = kick
  speed = 2
  notes = "48 - - 43 . 36"
}
"#;

    #[test]
    fn parses_a_bank() {
        let (bank, errors) = parse(SRC);
        assert_eq!(errors, vec![]);
        assert_eq!(bank.instrument_names, ["kick", "lead"]);
        assert_eq!(bank.sfx_names, ["boom"]);

        let kick = bank.instruments[0];
        assert_eq!(kick.wave, Waveform::Sine);
        assert_eq!(kick.decay_ms, 90);
        assert_eq!(kick.sustain, 0);
        assert_eq!(kick.pitch_env, 36);
        assert_eq!(kick.volume, 255);

        let lead = bank.instruments[1];
        assert_eq!(lead.filter, FilterMode::Lpf);
        assert_eq!(lead.cutoff, 160);
        assert_eq!(lead.pan, -40);
        // Unmentioned keys keep the default, not zero.
        assert_eq!(lead.volume, Patch::default().volume);

        let boom = &bank.sfx[0];
        assert_eq!(boom.inst, 0); // resolved from the name `kick`
        assert_eq!(boom.speed, 2);
        assert_eq!(
            boom.rows,
            [
                Row::Note(48),
                Row::Hold,
                Row::Hold,
                Row::Note(43),
                Row::Rest,
                Row::Note(36)
            ]
        );
    }

    #[test]
    fn ids_follow_declaration_order() {
        let (bank, _) = parse(SRC);
        assert_eq!(bank.instrument_id("kick"), Some(0));
        assert_eq!(bank.instrument_id("lead"), Some(1));
        assert_eq!(bank.sfx_id("boom"), Some(0));
        assert_eq!(bank.instrument_id("nope"), None);
    }

    #[test]
    fn holds_become_one_long_note() {
        let (bank, _) = parse(SRC);
        let notes: Vec<_> = bank.sfx[0].notes().collect();
        // "48 - - 43 . 36" at speed 2: a 6-frame 48, a 2-frame 43, a 2-frame 36.
        assert_eq!(notes, [(0, 48, 6), (6, 43, 2), (10, 36, 2)]);
        assert_eq!(bank.sfx[0].frames(), 12);
    }

    #[test]
    fn a_repeat_is_not_a_hold() {
        let rows = parse_rows("48 48").unwrap();
        let def = SfxDef {
            speed: 3,
            rows,
            ..SfxDef::default()
        };
        // Two separate hits, not one long one — that is the difference between
        // a machine gun and a drone.
        assert_eq!(def.notes().collect::<Vec<_>>(), [(0, 48, 3), (3, 48, 3)]);
    }

    #[test]
    fn comments_and_commas_are_ignored() {
        let (bank, errors) = parse(
            r#"
            instrument a { wave = saw, decay = 10, } -- trailing comma
            -- instrument b { wave = nonsense }
            "#,
        );
        assert_eq!(errors, vec![]);
        assert_eq!(bank.instruments.len(), 1);
        assert_eq!(bank.instruments[0].wave, Waveform::Saw);
    }

    #[test]
    fn every_error_is_reported_with_its_line() {
        let (_, errors) = parse(
            r#"instrument bad {
  wave = trumpet
  cutoff = 900
  wobble = 3
}"#,
        );
        let lines: Vec<usize> = errors.iter().map(|e| e.line).collect();
        assert_eq!(lines, [2, 3, 4], "{errors:#?}");
        assert!(errors[0].message.contains("unknown wave 'trumpet'"));
        assert!(
            errors[1].message.contains("0..=255"),
            "{}",
            errors[1].message
        );
        assert!(errors[2].message.contains("unknown instrument key"));
    }

    #[test]
    fn an_unknown_instrument_name_is_an_error_not_a_silent_zero() {
        let (_, errors) = parse("sfx s { inst = ghost }");
        assert_eq!(errors.len(), 1);
        assert!(errors[0].message.contains("no instrument named 'ghost'"));
    }

    #[test]
    fn duplicate_names_are_rejected() {
        let (bank, errors) = parse("instrument a {} instrument a {}");
        assert_eq!(bank.instruments.len(), 1);
        assert!(errors[0].message.contains("duplicate instrument 'a'"));
    }

    #[test]
    fn bad_notes_name_the_offending_word() {
        let (_, errors) = parse(r#"instrument i {} sfx s { inst = i  notes = "48 x 50" }"#);
        assert_eq!(errors.len(), 1);
        assert!(
            errors[0].message.contains("'x' is not a note"),
            "{errors:#?}"
        );
    }

    #[test]
    fn an_empty_source_is_an_empty_bank() {
        let (bank, errors) = parse("   \n -- nothing here \n ");
        assert!(bank.is_empty());
        assert_eq!(errors, vec![]);
    }

    #[test]
    fn malformed_input_terminates() {
        // Recovery must not loop: every one of these has to return.
        for src in [
            "instrument",
            "instrument a",
            "instrument a {",
            "instrument a { wave",
            "instrument a { wave = ",
            "}}}}",
            "instrument a { = 5 } instrument b { wave = saw }",
            "garbage { nested { } }",
            "sfx s { notes = \"unterminated",
        ] {
            let (_, errors) = parse(src);
            assert!(!errors.is_empty(), "{src:?} should have complained");
        }
    }
}
