//! The docs must not promise a field the parser refuses.
//!
//! `docs/VM_AUDIO.md` and `docs/SYNTH.md` both showed an `instrument` block
//! containing `lfo`, `lfo_rate`, `lfo_depth` and `lfo_target`. None of the four
//! ever existed: `set_instrument_field`'s fallthrough rejects an unknown key, so
//! a game that copied the documented example — the obvious thing to do with a
//! documented example — failed to compile on a line it had every reason to
//! trust.
//!
//! Nothing caught it because the two halves are checked by different things.
//! The parser has tests and the corpus has guards, but no game had reason to
//! write `lfo`, so the corpus proved only that the keys games *do* use work.
//! This closes the other direction: every key the docs show is a key the parser
//! takes.
//!
//! Names rather than a full parse, deliberately. Feeding the blocks to
//! [`kessel_audio::bank::parse`] would check the values too, but the docs'
//! blocks are illustrative — `track drive` names channels after instruments no
//! example declares — so it would fail on prose decisions rather than on drift.
//! A key name is the part that is a claim about the parser.

use kessel_audio::bank::{FX_KEYS, INSTRUMENT_KEYS, SFX_KEYS};

const DOCS: &[(&str, &str)] = &[
    (
        "docs/VM_AUDIO.md",
        include_str!("../../../docs/VM_AUDIO.md"),
    ),
    ("docs/SYNTH.md", include_str!("../../../docs/SYNTH.md")),
];

/// One `key = value` the docs wrote, and where.
struct Key {
    name: String,
    line: usize,
}

/// A declaration block found inside a fenced code sample.
struct Block {
    kind: String,
    line: usize,
    keys: Vec<Key>,
}

/// Everything on `line` before a `--` comment that is not inside a string.
///
/// Quote-aware because `notes = "48 - 43 . 36"` is full of dashes, and a naive
/// cut at the first `-` would silently drop the rest of a line — losing keys
/// rather than reporting them, which is the failure mode a guard must not have.
fn code_of(line: &str) -> &str {
    let b = line.as_bytes();
    let mut quoted = false;
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'"' => quoted = !quoted,
            b'-' if !quoted && i + 1 < b.len() && b[i + 1] == b'-' => return &line[..i],
            _ => {}
        }
        i += 1;
    }
    line
}

/// Every identifier that `code` assigns to: the word before each `=`.
fn keys_in(code: &str, line: usize) -> Vec<Key> {
    let b = code.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] != b'=' {
            i += 1;
            continue;
        }
        // Skip comparisons: the docs have none inside a block today, but a
        // future `if x == 1` in a sample must not read as a key called `x`.
        if b.get(i + 1) == Some(&b'=')
            || matches!(
                b.get(i.wrapping_sub(1)),
                Some(b'=' | b'~' | b'<' | b'>' | b'!')
            )
        {
            i += 1;
            continue;
        }
        let mut end = i;
        while end > 0 && b[end - 1].is_ascii_whitespace() {
            end -= 1;
        }
        let mut start = end;
        while start > 0 && (b[start - 1].is_ascii_alphanumeric() || b[start - 1] == b'_') {
            start -= 1;
        }
        if start < end {
            out.push(Key {
                name: code[start..end].to_string(),
                line,
            });
        }
        i += 1;
    }
    out
}

/// Does `code` open a declaration block, and of what kind?
fn opens(code: &str) -> Option<&'static str> {
    let t = code.trim_start();
    ["instrument", "sfx", "track", "fx"]
        .into_iter()
        .find(|kind| {
            t.strip_prefix(kind)
                .is_some_and(|rest| rest.starts_with(char::is_whitespace) || rest.starts_with('{'))
        })
}

/// Pull every `instrument` / `sfx` / `track` / `fx` block out of a markdown
/// file's fenced samples.
///
/// Fenced only: the same words appear in prose all over both documents, and a
/// sentence is not a claim about the grammar.
fn blocks(src: &str) -> Vec<Block> {
    let mut out: Vec<Block> = Vec::new();
    let mut fenced = false;
    let mut depth: i32 = 0;

    for (i, raw) in src.lines().enumerate() {
        let line = i + 1;
        if raw.trim_start().starts_with("```") {
            fenced = !fenced;
            // A block cannot span a fence. If one appears to, the sample is
            // malformed and its keys are not worth guessing at.
            depth = 0;
            continue;
        }
        if !fenced {
            continue;
        }

        let code = code_of(raw);
        if depth == 0 {
            match opens(code) {
                Some(kind) if code.contains('{') => out.push(Block {
                    kind: kind.to_string(),
                    line,
                    keys: Vec::new(),
                }),
                _ => continue,
            }
        }

        if let Some(block) = out.last_mut() {
            block.keys.extend(keys_in(code, line));
        }
        depth += code.matches('{').count() as i32;
        depth -= code.matches('}').count() as i32;
        if depth < 0 {
            depth = 0;
        }
    }

    out
}

/// Which key list a block's keys must come from, or `None` when the block's
/// keys are open-ended.
///
/// `track` is the one that cannot be checked: every key there that is not
/// `tempo`/`vel`/`loop` is *by design* an instrument name, so a typo and a
/// channel are the same thing to the parser and there is nothing for a guard to
/// compare against. That is a property of the grammar, not a gap in this test.
fn allowed(kind: &str) -> Option<&'static [&'static str]> {
    match kind {
        "instrument" => Some(INSTRUMENT_KEYS),
        "fx" => Some(FX_KEYS),
        "sfx" => Some(SFX_KEYS),
        _ => None,
    }
}

#[test]
fn every_documented_field_is_a_field_the_parser_takes() {
    let mut checked = 0usize;
    let mut instrument_blocks = 0usize;

    for (doc, src) in DOCS {
        let found = blocks(src);
        assert!(
            !found.is_empty(),
            "{doc}: no declaration blocks found — the fence format changed and \
             this guard is now checking nothing"
        );

        for block in &found {
            if block.kind == "instrument" {
                instrument_blocks += 1;
            }
            let Some(keys) = allowed(&block.kind) else {
                continue;
            };
            for key in &block.keys {
                assert!(
                    keys.contains(&key.name.as_str()),
                    "{doc}:{} documents '{}' in the '{}' block opened at line {}, \
                     but the parser rejects it. Either implement it in \
                     kessel_audio::bank or take it out of the docs — a reader \
                     who copies this block cannot compile it.\nAccepted: {}",
                    key.line,
                    key.name,
                    block.kind,
                    block.line,
                    keys.join(", ")
                );
                checked += 1;
            }
        }
    }

    // Both documents carry a full `instrument` block, and between them they
    // spell out most of the patch. Without a floor, a change that stopped the
    // scanner finding keys would leave this test green over nothing — which is
    // exactly how the `lfo` line survived in the first place.
    assert!(
        instrument_blocks >= 2,
        "expected an instrument block in each document, found {instrument_blocks}"
    );
    assert!(
        checked >= 30,
        "only {checked} documented keys checked — the scanner stopped finding them"
    );
}

/// The `lfo` line specifically, by name.
///
/// The test above would catch it coming back, but only as "unknown key". This
/// one says what it was and why it is not returning, so the next person to
/// reach for a modulator reads the reason instead of re-deriving it.
#[test]
fn no_document_offers_an_lfo() {
    for (doc, src) in DOCS {
        for block in blocks(src) {
            for key in &block.keys {
                assert!(
                    !key.name.starts_with("lfo"),
                    "{doc}:{} offers '{}'. There is no LFO in this synth and \
                     there was never meant to be one: `pitch_env` was chosen \
                     over it because a console's sweeps are one-shot, and a \
                     patch is fixed when the ROM loads, so a sweep the player \
                     drives is a choice between patches declared up front and \
                     retriggered on a channel.",
                    key.line,
                    key.name
                );
            }
        }
    }
}
