//! Varvara-lite device layer, reached through the VM's `DEI`/`DEO` opcodes.
//!
//! A port is a single byte: the high nibble selects the device, the low nibble
//! selects a register. Devices are deliberately tiny and deterministic so a
//! whole run can be snapshotted and replayed:
//!
//! | dev | name    | registers |
//! |-----|---------|-----------|
//! | 0x0 | system  | 0 halt · 1 pal-index (0..255, commits) · 2 pal-r · 3 pal-g · 4 pal-b |
//! | 0x1 | screen  | 0 vector · 1 x · 2 y · 3 color · 4 pixel · 5 sprite · 6 cls · 7 cam-x · 8 cam-y · 9 flags · a blit-id · b tileset-base · c glyph(code) · d hspan(x2) · e sprite-bank |
//! | 0x2 | gamepad | 0 buttons · 1 pressed (edge) · 2 released (edge) · 3 stick-x · 4 stick-y — all read |
//! | 0x3 | rng     | 0 next (read) / set-seed (write) |
//! | 0x4 | storage | 0 addr · 1 read · 2 write |
//! | 0x5 | debug   | 0 ent-x · 1 ent-y · 2 ent-commit(tag) |
//! | 0x6 | console | 0 write-byte |
//! | 0x7 | tilemap | 0 base · 1 width · 2 tx · 3 ty · 4 sx · 5 sy · 6 tw · 7 th · 8 draw |
//! | 0x8 | time    | 0 frame-count (read) |
//! | 0x9 | sound   | 0 sfx(id) · 1 music(id) · 2 music-stop · 3 frames · 4 velocity · 5 note · 6 inst→play · 7 inst · 8 chan→note-on · 9 chan→note-off |
//! | 0xa | sprn    | 0 base-id · 1 w · 2 h · 3 draw (w×h block at screen x/y) |
//! | 0xb | scale   | 0 scale (8.8 fixed, 256 = 1.0) · 1 blit-id (scaled tile at screen x/y) |
//! | 0xc | trig    | 0 angle (write, 0..255 = full turn) → sin (read) · 1 cos (read); results are signed 8.8 fixed (-256..256) |
//! | 0xd | touch   | 0 slot (write) / count (read) · 1 x · 2 y · 3 state (bit0 down, bit1 pressed, bit2 released) · 4 swipe · 5 dx · 6 dy · 7 frames held |

/// Screen edge length for [`VideoMode::Classic128`].
pub const CLASSIC_DIM: usize = 128;
/// Screen edge length for [`VideoMode::Extended240`].
pub const EXTENDED_DIM: usize = 240;

/// The screen a ROM asks for.
///
/// Both modes are the *same machine*: an 8-bit palette-index framebuffer, one
/// 256-entry palette, the same drawing ports, the same 4bpp sprite sheet. The
/// only difference is how many pixels there are. Keeping it that way is the
/// whole point — a second mode that also changed the colour model would double
/// the blitter, the PNG path, and every host's upload code for no gain.
///
/// The screen stays square in both. That is what lets every host treat the
/// framebuffer as one number (`dim`) rather than a width and a height it has to
/// keep in agreement.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum VideoMode {
    /// 128×128 — the original console. What a ROM gets if it asks for nothing.
    #[default]
    Classic128,
    /// 240×240 — room for a HUD beside the play field.
    Extended240,
}

impl VideoMode {
    /// Screen edge length in pixels.
    pub fn dim(self) -> usize {
        match self {
            VideoMode::Classic128 => CLASSIC_DIM,
            VideoMode::Extended240 => EXTENDED_DIM,
        }
    }

    /// Total framebuffer cells (one palette index each).
    pub fn pixels(self) -> usize {
        self.dim() * self.dim()
    }

    /// Parse a mode name as written in a `screen { … }` block. Case-insensitive
    /// so `Extended240` and `extended240` both work.
    pub fn from_name(name: &str) -> Option<VideoMode> {
        match name.to_ascii_lowercase().as_str() {
            "classic128" | "classic" | "128" => Some(VideoMode::Classic128),
            "extended240" | "extended" | "240" => Some(VideoMode::Extended240),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            VideoMode::Classic128 => "Classic128",
            VideoMode::Extended240 => "Extended240",
        }
    }
}

/// Gamepad button bits, matching the values the host pushes.
pub const BTN_LEFT: u8 = 0x01;
pub const BTN_RIGHT: u8 = 0x02;
pub const BTN_UP: u8 = 0x04;
pub const BTN_DOWN: u8 = 0x08;
pub const BTN_A: u8 = 0x10;
pub const BTN_B: u8 = 0x20;
pub const BTN_START: u8 = 0x40;
pub const BTN_SELECT: u8 = 0x80;

/// Full analog deflection, in the same signed 8.8 fixed point the trig device
/// returns (256 = 1.0). A stick reads as `-STICK_FULL..=STICK_FULL` on each
/// axis, so `x = x + stick_x() * speed / 256` is the same arithmetic a game
/// already writes for `cos(a) * speed / 256`.
pub const STICK_FULL: i16 = 256;

/// How many simultaneous touch points the console reports.
///
/// Four, because that is what a pair of thumbs and a stray finger produce and
/// what fits in a snapshot without thought. It is a *console* limit, not a
/// hardware one: a host that sees more fingers drops the extras rather than
/// reshuffling the slots underneath a game.
pub const MAX_TOUCHES: usize = 4;

/// One touch point, in **console pixels** — the host has already undone its own
/// letterboxing and upscale, so a game compares these against the coordinates it
/// draws with and never learns the window size.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Touch {
    pub x: u16,
    pub y: u16,
    pub down: bool,
}

/// One touch slot's gesture, tracked by the console across frames.
///
/// The platforms this borrows from split gestures in two: a discrete flick
/// (`UISwipeGestureRecognizer`, `GestureDetector.onFling`) that reports only a
/// direction, and a continuous drag (`UIPanGestureRecognizer`, `onScroll`) that
/// reports displacement. This is both, because on a 60 Hz console they are the
/// same three numbers — where the press began, how far it has come, and how long
/// it has been going.
///
/// The console keeps the **origin** and hands out a *signed* delta rather than
/// the reverse. A game already knows the current position from `touch_x`, so a
/// delta gives it the origin back for free (`x - dx`) — while exposing the
/// origin instead would leave every swipe game subtracting two `u16`s and
/// wrapping on any leftward drag. That trap is the whole reason this lives in
/// the device.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Gesture {
    /// Where this press landed, in console pixels.
    origin: (u16, u16),
    /// Frames this press has been held, saturating rather than wrapping — a
    /// finger resting for eighteen minutes should not read as freshly landed.
    frames: u16,
    /// The direction already reported for this press, so one press is one
    /// swipe. Without it a finger held past the threshold would re-report the
    /// same direction every frame for as long as it stayed down.
    fired: u8,
    /// The direction to report *this frame only* — the discrete half.
    swipe: u8,
}

/// How far a finger must travel to count as a swipe, as a fraction of the
/// screen: `dim / SWIPE_DIVISOR`, so 16 px on Classic128 and 30 on
/// Extended240.
///
/// Screen-relative rather than a fixed pixel count because the two screens are
/// the same physical size — a fixed count would make the gesture feel shorter on
/// the denser one. Android reaches the same place from the other direction with
/// `ViewConfiguration.getScaledTouchSlop`, which is in dp precisely so it means
/// one physical distance.
const SWIPE_DIVISOR: usize = 8;

/// Everything a host hands the machine for one frame.
///
/// This is one struct rather than three arguments because the three are one
/// thing: the state of the player's hands at a frame boundary. A snapshot that
/// replays the buttons but not the stick would be a *plausibly* wrong replay,
/// which is the failure mode this machine exists to avoid.
///
/// `From<u8>` keeps the common case honest — `run_frame(BTN_A)` means "buttons
/// only, everything else at rest", which is exactly what a digital game wants.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Input {
    pub buttons: u8,
    /// Analog stick, signed 8.8 fixed in `-STICK_FULL..=STICK_FULL`.
    pub stick_x: i16,
    pub stick_y: i16,
    /// Touch points by **slot**. A host must keep one finger in one slot for
    /// that finger's whole life — the press/release edges are computed per slot,
    /// so a host that renumbers its fingers between frames reports a release and
    /// a press that never happened.
    pub touches: [Touch; MAX_TOUCHES],
}

impl From<u8> for Input {
    fn from(buttons: u8) -> Self {
        Input {
            buttons,
            ..Input::default()
        }
    }
}

impl Input {
    /// Buttons held, with everything analog at rest.
    pub fn buttons(buttons: u8) -> Input {
        Input::from(buttons)
    }

    /// The same input with `buttons` replaced — for the pause button, which the
    /// host masks out before the game ever sees the frame.
    pub fn with_buttons(self, buttons: u8) -> Input {
        Input { buttons, ..self }
    }

    /// True when nothing analog is being reported, so a caller can skip
    /// describing it. Buttons are *not* consulted: they have their own record.
    pub fn analog_is_at_rest(&self) -> bool {
        self.stick_x == 0 && self.stick_y == 0 && self.touches.iter().all(|t| !t.down)
    }
}

/// The 3×5 pixel rows for one glyph (ASCII `code`), top to bottom. Each row is
/// 3 bits — bit 2 is the leftmost column. Covers `A-Z` (lowercase folds up),
/// `0-9`, space, and `: ! . -`; anything else is blank. Small enough to inline
/// scores, titles and `GAME OVER` without a font ROM.
fn glyph_rows(code: u8) -> [u8; 5] {
    let c = code.to_ascii_uppercase();
    match c {
        b'0' => [7, 5, 5, 5, 7],
        b'1' => [2, 6, 2, 2, 7],
        b'2' => [7, 1, 7, 4, 7],
        b'3' => [7, 1, 7, 1, 7],
        b'4' => [5, 5, 7, 1, 1],
        b'5' => [7, 4, 7, 1, 7],
        b'6' => [7, 4, 7, 5, 7],
        b'7' => [7, 1, 2, 2, 2],
        b'8' => [7, 5, 7, 5, 7],
        b'9' => [7, 5, 7, 1, 7],
        b'A' => [7, 5, 7, 5, 5],
        b'B' => [6, 5, 6, 5, 6],
        b'C' => [7, 4, 4, 4, 7],
        b'D' => [6, 5, 5, 5, 6],
        b'E' => [7, 4, 7, 4, 7],
        b'F' => [7, 4, 7, 4, 4],
        b'G' => [7, 4, 5, 5, 7],
        b'H' => [5, 5, 7, 5, 5],
        b'I' => [7, 2, 2, 2, 7],
        b'J' => [1, 1, 1, 5, 7],
        b'K' => [5, 6, 4, 6, 5],
        b'L' => [4, 4, 4, 4, 7],
        b'M' => [5, 7, 7, 5, 5],
        b'N' => [5, 7, 5, 5, 5],
        b'O' => [7, 5, 5, 5, 7],
        b'P' => [7, 5, 7, 4, 4],
        b'Q' => [7, 5, 5, 7, 3],
        b'R' => [7, 5, 7, 6, 5],
        b'S' => [7, 4, 7, 1, 7],
        b'T' => [7, 2, 2, 2, 2],
        b'U' => [5, 5, 5, 5, 7],
        b'V' => [5, 5, 5, 5, 2],
        b'W' => [5, 5, 7, 7, 5],
        b'X' => [5, 5, 2, 5, 5],
        b'Y' => [5, 5, 2, 2, 2],
        b'Z' => [7, 1, 2, 4, 7],
        b':' => [0, 2, 0, 2, 0],
        b'!' => [2, 2, 2, 0, 2],
        b'.' => [0, 0, 0, 0, 2],
        b'-' => [0, 0, 7, 0, 0],
        _ => [0, 0, 0, 0, 0], // space + unknown
    }
}

/// Fixed-point sine/cosine of `angle`, where 0..256 spans a full turn (so 64 =
/// 90°, 128 = 180°, 192 = 270°). The result is signed 8.8 fixed in [-256, 256]
/// (256 = 1.0), returned as two's-complement `u16`. Games use it as
/// `vx = cos(a) * speed / 256`. Exact at the cardinal angles: sin(0)=0,
/// sin(64)=256, sin(128)=0, sin(192)=-256.
fn trig_fp(angle: u16, cosine: bool) -> u16 {
    let radians = (angle & 0xff) as f64 / 256.0 * std::f64::consts::TAU;
    let v = if cosine { radians.cos() } else { radians.sin() };
    let scaled = (v * 256.0).round() as i32;
    scaled.clamp(-256, 256) as i16 as u16
}

/// An entity record the running game reports to the debug port for observation.
/// These are authored by the game (not inferred), so the harness can expose or
/// hide internal state per experiment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Entity {
    pub tag: u16,
    pub x: u16,
    pub y: u16,
}

/// An out-of-range argument to a note port makes the whole note a **no-op**.
///
/// Neither truncating nor clamping works here, and the reason is that a channel
/// is an *identity*: `note_on(256, …)` truncated is channel 0 and clamped is
/// channel 255, and both are channels a game may legitimately be holding a note
/// on. Any mapping of an invalid value onto the valid range takes someone
/// else's note. There is no spare value to land on, so the only answer that
/// cannot corrupt state is to do nothing.
///
/// That is also what this device already does with an off-screen pixel — see
/// `pixel_out_of_bounds_ignored`. Silence is the cost; [`Devices::sound_dropped`]
/// is what keeps it from being a *silent* silence.
fn byte(val: u16) -> Option<u8> {
    (val <= 255).then_some(val as u8)
}

/// The same for a MIDI note number, whose range is `0..=127`.
fn midi(val: u16) -> Option<u8> {
    (val <= 127).then_some(val as u8)
}

/// Latched arguments for the note-level sound ports.
///
/// The multi-argument ports work like the palette's: each argument is written
/// to its own register and the *last* one commits. A stack machine hands its
/// arguments back in reverse, so the register that commits is the one holding
/// the call's **first** argument — `play(inst, …)` commits on `inst`.
#[derive(Debug, Clone, Copy, Default)]
struct NoteLatch {
    frames: u16,
    vel: u16,
    note: u16,
    inst: u16,
}

/// The 16 colours a ROM gets without touching the palette (PICO-8's).
///
/// These occupy indices 0–15, which is exactly the range a 4bpp sprite nibble
/// can name in bank 0 — so existing art keeps its colours, and a game that
/// never calls `pal` or `sprbank` sees the console it always saw.
pub const BASE_16: [(u8, u8, u8); 16] = [
    (0x00, 0x00, 0x00),
    (0x1D, 0x2B, 0x53),
    (0x7E, 0x25, 0x53),
    (0x00, 0x87, 0x51),
    (0xAB, 0x52, 0x36),
    (0x5F, 0x57, 0x4F),
    (0xC2, 0xC3, 0xC7),
    (0xFF, 0xF1, 0xE8),
    (0xFF, 0x00, 0x4D),
    (0xFF, 0xA3, 0x00),
    (0xFF, 0xEC, 0x27),
    (0x00, 0xE4, 0x36),
    (0x29, 0xAD, 0xFF),
    (0x83, 0x76, 0x9C),
    (0xFF, 0x77, 0xA8),
    (0xFF, 0xCC, 0xAA),
];

/// The default 256-entry palette.
///
/// Laid out the way an xterm-256 palette is, and for the same reason — an index
/// should mean something predictable before anyone calls `pal`:
///
/// - `0–15`   the [`BASE_16`] colours, so old art is unchanged
/// - `16–231` a 6×6×6 RGB cube, so `rgb6(r,g,b)` names a colour arithmetically
/// - `232–255` a 24-step grey ramp, for shadows, fades and dimming
///
/// A ROM may overwrite any of it; nothing here is reserved. The console draws
/// no UI of its own, so there is no system colour to protect — a host that
/// wants a pause menu draws it in native UI, outside the framebuffer.
pub const DEFAULT_PALETTE: [(u8, u8, u8); 256] = build_default_palette();

/// The 6 levels each channel takes in the colour cube, matching xterm's.
const CUBE_LEVELS: [u8; 6] = [0x00, 0x5F, 0x87, 0xAF, 0xD7, 0xFF];

/// Built at compile time so the default palette is a constant, not a lazily
/// initialised table every `Devices::new` would have to copy from.
const fn build_default_palette() -> [(u8, u8, u8); 256] {
    let mut p = [(0u8, 0u8, 0u8); 256];

    let mut i = 0;
    while i < 16 {
        p[i] = BASE_16[i];
        i += 1;
    }

    // 16..232 — the 6×6×6 cube, red-major so index = 16 + 36r + 6g + b.
    let mut r = 0;
    while r < 6 {
        let mut g = 0;
        while g < 6 {
            let mut b = 0;
            while b < 6 {
                p[16 + 36 * r + 6 * g + b] = (CUBE_LEVELS[r], CUBE_LEVELS[g], CUBE_LEVELS[b]);
                b += 1;
            }
            g += 1;
        }
        r += 1;
    }

    // 232..256 — grey ramp, 8..238 in steps of 10.
    let mut k = 0;
    while k < 24 {
        let v = (8 + k * 10) as u8;
        p[232 + k] = (v, v, v);
        k += 1;
    }

    p
}

/// The palette index of a colour in the default cube, for `r`/`g`/`b` in 0..6.
/// Out-of-range channels clamp, so this is total.
pub fn rgb6(r: u8, g: u8, b: u8) -> u8 {
    let c = |v: u8| v.min(5) as usize;
    (16 + 36 * c(r) + 6 * c(g) + c(b)) as u8
}

/// All device-side state. Cloned wholesale for snapshots.
#[derive(Clone)]
pub struct Devices {
    /// `dim * dim` palette indices, one byte each.
    pub framebuffer: Vec<u8>,
    /// Screen edge length. Fixed for the life of a loaded ROM — see
    /// [`set_mode`](Devices::set_mode).
    dim: usize,
    pub palette: [(u8, u8, u8); 256],
    /// Current gamepad button bitfield.
    pub gamepad: u8,
    /// Gamepad bitfield from the *previous* frame, for edge detection
    /// (`btnp`/`btnr`).
    pub prev_gamepad: u8,
    /// Analog stick this frame, signed 8.8 fixed (see [`STICK_FULL`]).
    pub stick_x: i16,
    pub stick_y: i16,
    /// Touch points this frame, by slot.
    pub touches: [Touch; MAX_TOUCHES],
    /// Which slots were down on the *previous* frame, for the touch equivalent
    /// of `btnp`/`btnr`.
    prev_touch_down: [bool; MAX_TOUCHES],
    /// Per-slot gesture state: where the current press began, how long it has
    /// been held, and which way it has been judged to have swiped.
    gestures: [Gesture; MAX_TOUCHES],
    /// Frames elapsed since power-on (wraps at 65536; drives blink/timers).
    pub frame_count: u16,
    /// The frame vector the game installed via `screen/vector`; 0 = none.
    pub frame_vector: u16,
    /// Set when the game writes a non-zero value to `system/halt`.
    pub halt_requested: bool,
    /// Entities reported this frame (cleared each frame by the console).
    pub entities: Vec<Entity>,
    /// Bytes written to the console this frame (cleared each frame).
    pub console: Vec<u8>,
    /// Sound the game asked for this frame (cleared each frame).
    ///
    /// Recorded, never rendered: the machine stays deterministic and a host
    /// turns this log into samples. A frame that is snapshotted and replayed
    /// therefore asks for exactly the same sound.
    pub sound: Vec<kessel_audio::AudioEvent>,
    /// Notes ignored because an argument was out of range. Cumulative, so a
    /// host can report the difference over a run rather than losing it with the
    /// per-frame log.
    pub sound_dropped: u64,
    /// Pending arguments for the note ports.
    note_latch: NoteLatch,
    /// Persistent storage (survives resets? no — power-on state, but survives frames).
    pub storage: [u8; 256],

    // --- transient device registers ---
    rng_state: u32,
    storage_addr: u8,
    // pending screen coords / colour
    sx: u16,
    sy: u16,
    scolor: u8,
    // pending palette entry being staged (committed by a write to pal-index)
    pr: u8,
    pg: u8,
    pb: u8,
    // pending entity coords
    ex: u16,
    ey: u16,
    // camera offset (world→screen translation), signed
    cam_x: i16,
    cam_y: i16,
    // flip flags for the next sprite blit: bit0 = flip-x, bit1 = flip-y
    sprite_flags: u8,
    // palette bank for sprite blits: a nibble `n` draws in colour `bank*16 + n`
    sprite_bank: u8,
    // base address of the sprite sheet (32-byte 4bpp tiles) for blit-by-id
    tileset_base: u16,
    // tilemap device (page 0x7): base/width of the tile-id grid + pending region
    map_base: u16,
    map_width: u16,
    map_tx: u16,
    map_ty: u16,
    map_sx: u16,
    map_sy: u16,
    map_tw: u16,
    map_th: u16,
    // composite-sprite device (page 0xa): base tile id + block size
    sprn_id: u16,
    sprn_w: u16,
    sprn_h: u16,
    // scaled-sprite device (page 0xb): pending scale (8.8 fixed, 256 = 1.0)
    scale_fp: u16,
    // trig device (page 0xc): latched angle (0..255 = a full turn)
    trig_angle: u16,
    // touch device (page 0xd): which slot the next x/y/state read describes
    touch_slot: u16,
}

impl Default for Devices {
    fn default() -> Self {
        Self::new()
    }
}

impl Devices {
    pub fn new() -> Self {
        Self::with_mode(VideoMode::default())
    }

    /// A fresh device set at `mode`. The resolution is fixed at construction
    /// because the reset vector draws — a ROM that sets up its first frame in
    /// `init()` must do it at the size it asked for, not at the default and
    /// then again after a switch.
    pub fn with_mode(mode: VideoMode) -> Self {
        Devices {
            framebuffer: vec![0u8; mode.pixels()],
            dim: mode.dim(),
            palette: DEFAULT_PALETTE,
            gamepad: 0,
            prev_gamepad: 0,
            stick_x: 0,
            stick_y: 0,
            touches: [Touch::default(); MAX_TOUCHES],
            prev_touch_down: [false; MAX_TOUCHES],
            gestures: [Gesture::default(); MAX_TOUCHES],
            frame_count: 0,
            frame_vector: 0,
            halt_requested: false,
            entities: Vec::new(),
            console: Vec::new(),
            sound: Vec::new(),
            sound_dropped: 0,
            note_latch: NoteLatch::default(),
            storage: [0u8; 256],
            rng_state: 0x1234_5678,
            storage_addr: 0,
            sx: 0,
            sy: 0,
            scolor: 0,
            pr: 0,
            pg: 0,
            pb: 0,
            ex: 0,
            ey: 0,
            cam_x: 0,
            cam_y: 0,
            sprite_flags: 0,
            sprite_bank: 0,
            tileset_base: 0,
            map_base: 0,
            map_width: 0,
            map_tx: 0,
            map_ty: 0,
            map_sx: 0,
            map_sy: 0,
            map_tw: 0,
            map_th: 0,
            sprn_id: 0,
            sprn_w: 0,
            sprn_h: 0,
            scale_fp: 256,
            trig_angle: 0,
            touch_slot: 0,
        }
    }

    /// Read a device register (`DEI`).
    pub fn read(&mut self, port: u8) -> u16 {
        let dev = port >> 4;
        let reg = port & 0x0f;
        match (dev, reg) {
            (0x2, 0x0) => self.gamepad as u16,
            // just-pressed this frame (rising edge): held now, not held before.
            (0x2, 0x1) => (self.gamepad & !self.prev_gamepad) as u16,
            // just-released this frame (falling edge): held before, not now.
            (0x2, 0x2) => (!self.gamepad & self.prev_gamepad) as u16,
            // Analog stick, signed 8.8 fixed as two's-complement — the same
            // shape the trig device returns, so one `* v / 256` idiom covers
            // both.
            (0x2, 0x3) => self.stick_x as u16,
            (0x2, 0x4) => self.stick_y as u16,
            (0x3, 0x0) => self.next_rand(),
            // time device: frames since power-on.
            (0x8, 0x0) => self.frame_count,
            (0x4, 0x1) => self.storage[self.storage_addr as usize] as u16,
            // trig device: sin/cos of the latched angle, signed 8.8 fixed.
            (0xc, 0x0) => trig_fp(self.trig_angle, false),
            (0xc, 0x1) => trig_fp(self.trig_angle, true),
            // Touch device: how many slots are down, then the latched slot's
            // position and edges. An out-of-range slot reads as an empty one —
            // the same do-nothing answer an off-screen `pset` gets, and the only
            // one that cannot make a game act on a finger nobody put down.
            (0xd, 0x0) => self.touches.iter().filter(|t| t.down).count() as u16,
            (0xd, 0x1) => self.touch(self.touch_slot).map_or(0, |t| t.x),
            (0xd, 0x2) => self.touch(self.touch_slot).map_or(0, |t| t.y),
            (0xd, 0x3) => self.touch_state(self.touch_slot),
            // The gesture half, on the *same* latched slot as the three above —
            // a swipe's direction and its distance have to describe one finger,
            // or a two-finger game reads one thumb's direction against the
            // other's travel.
            (0xd, 0x4) => self.gesture(self.touch_slot).map_or(0, |g| g.swipe as u16),
            (0xd, 0x5) => self.drag(self.touch_slot).0 as u16,
            (0xd, 0x6) => self.drag(self.touch_slot).1 as u16,
            (0xd, 0x7) => self.gesture(self.touch_slot).map_or(0, |g| g.frames),
            _ => 0,
        }
    }

    /// The latched slot, or `None` when the ROM named one that does not exist.
    fn touch(&self, slot: u16) -> Option<&Touch> {
        self.touches.get(slot as usize)
    }

    fn gesture(&self, slot: u16) -> Option<&Gesture> {
        self.gestures.get(slot as usize)
    }

    /// Displacement from where this press began, **signed**, in console pixels.
    ///
    /// Signed for the same reason the stick and `sin`/`cos` are: a leftward drag
    /// is a negative number, and a game computing it from two `u16` positions
    /// would wrap instead. A slot that is not down reads `(0, 0)` rather than
    /// the last drag it made — a finger that has lifted has no displacement,
    /// and a stale one would have a game keep steering after the player let go.
    fn drag(&self, slot: u16) -> (i16, i16) {
        let (Some(t), Some(g)) = (self.touch(slot), self.gesture(slot)) else {
            return (0, 0);
        };
        if !t.down {
            return (0, 0);
        }
        (
            t.x.wrapping_sub(g.origin.0) as i16,
            t.y.wrapping_sub(g.origin.1) as i16,
        )
    }

    /// Advance every slot's gesture for a new frame of touch input.
    ///
    /// Runs in `begin_frame`, over state the console already owns, which is what
    /// makes a swipe identical on every host and exact under snapshot/replay. A
    /// host-side recognizer would make one recorded frame mean different things
    /// depending on who replayed it.
    fn track_gestures(&mut self) {
        let threshold = (self.dim / SWIPE_DIVISOR) as i32;
        for (slot, g) in self.gestures.iter_mut().enumerate() {
            let now = self.touches[slot];
            let was = self.prev_touch_down[slot];

            // A swipe lasts exactly the frame it is recognized on, like the
            // gamepad's press edge.
            g.swipe = 0;

            if !now.down {
                *g = Gesture::default();
                continue;
            }
            if !was {
                // Press edge: a finger that has only just landed cannot have
                // travelled, so this frame starts the gesture and reports
                // nothing.
                *g = Gesture {
                    origin: (now.x, now.y),
                    ..Gesture::default()
                };
                continue;
            }

            g.frames = g.frames.saturating_add(1);
            if g.fired != 0 {
                continue;
            }

            let dx = now.x as i32 - g.origin.0 as i32;
            let dy = now.y as i32 - g.origin.1 as i32;
            if dx.abs() < threshold && dy.abs() < threshold {
                continue;
            }
            // Dominant axis only — never a diagonal. Every game that wants this
            // wants one of four answers, and a tie going to X keeps a perfectly
            // diagonal drag deterministic rather than dependent on rounding.
            let dir = if dx.abs() >= dy.abs() {
                if dx < 0 {
                    BTN_LEFT
                } else {
                    BTN_RIGHT
                }
            } else if dy < 0 {
                BTN_UP
            } else {
                BTN_DOWN
            };
            g.swipe = dir;
            g.fired = dir;
        }
    }

    /// `bit0` down · `bit1` pressed this frame · `bit2` released this frame.
    fn touch_state(&self, slot: u16) -> u16 {
        let Some(t) = self.touch(slot) else {
            return 0;
        };
        let was = self.prev_touch_down[slot as usize];
        (t.down as u16) | ((t.down && !was) as u16) << 1 | ((!t.down && was) as u16) << 2
    }

    /// Write a device register (`DEO`). `mem` is the VM's main memory, needed by
    /// the sprite blitter which reads tile data from it.
    pub fn write(&mut self, port: u8, val: u16, mem: &[u8]) {
        let dev = port >> 4;
        let reg = port & 0x0f;
        match dev {
            0x0 => match reg {
                0x0 => {
                    if val != 0 {
                        self.halt_requested = true;
                    }
                }
                // Palette: stage r/g/b, then strobe the index to commit.
                //
                // The index commits (rather than blue) because that is the
                // order a stack machine can produce for free: `pal(i,r,g,b)`
                // pushes i first, so b comes off the stack first and i last.
                // Committing on blue would need the arguments reversed.
                0x1 => self.palette[(val & 0xff) as usize] = (self.pr, self.pg, self.pb),
                0x2 => self.pr = val as u8,
                0x3 => self.pg = val as u8,
                0x4 => self.pb = val as u8,
                _ => {}
            },
            0x1 => match reg {
                0x0 => self.frame_vector = val,
                0x1 => self.sx = val,
                0x2 => self.sy = val,
                0x3 => self.scolor = val as u8,
                0x4 => self.put_pixel(self.sx, self.sy, self.scolor),
                0x5 => self.blit_sprite(val, mem),
                0x6 => {
                    // cls ignores the camera — it clears the whole screen.
                    let c = val as u8;
                    for px in self.framebuffer.iter_mut() {
                        *px = c;
                    }
                }
                0x7 => self.cam_x = val as i16,
                0x8 => self.cam_y = val as i16,
                0x9 => self.sprite_flags = val as u8,
                // Palette bank for subsequent sprite blits (0..15).
                0xe => self.sprite_bank = (val & 0x0f) as u8,
                // Blit sprite by id from the tileset (base + id*32).
                0xa => {
                    let addr = self
                        .tileset_base
                        .wrapping_add((val & 0xff).wrapping_mul(32));
                    self.blit_sprite(addr, mem);
                }
                0xb => self.tileset_base = val,
                // Draw one 3×5 font glyph (ascii code = val) at (sx,sy) in scolor.
                0xc => self.draw_glyph(val as u8),
                // Horizontal span from the pending sx to x2 (=val) at row sy in
                // scolor — the pseudo-3D road/scanline primitive.
                0xd => self.draw_hline(self.sx, val, self.sy, self.scolor),
                _ => {}
            },
            0x3 => {
                if reg == 0x0 && val != 0 {
                    self.rng_state = val as u32;
                }
            }
            0x4 => match reg {
                0x0 => self.storage_addr = val as u8,
                0x2 => self.storage[self.storage_addr as usize] = val as u8,
                _ => {}
            },
            0x5 => match reg {
                0x0 => self.ex = val,
                0x1 => self.ey = val,
                0x2 => self.entities.push(Entity {
                    tag: val,
                    x: self.ex,
                    y: self.ey,
                }),
                _ => {}
            },
            0x6 => {
                if reg == 0x0 {
                    self.console.push(val as u8);
                }
            }
            // Tilemap device: set base/width/region, then draw a tw×th block of
            // tiles from the map (ids in memory) via the sprite sheet.
            0x7 => match reg {
                0x0 => self.map_base = val,
                0x1 => self.map_width = val,
                0x2 => self.map_tx = val,
                0x3 => self.map_ty = val,
                0x4 => self.map_sx = val,
                0x5 => self.map_sy = val,
                0x6 => self.map_tw = val,
                0x7 => self.map_th = val,
                0x8 => self.draw_map(mem),
                _ => {}
            },
            // Sound device: record what the game asked for. The machine stays
            // silent and deterministic; a host renders the log.
            0x9 => match reg {
                0x0 => self
                    .sound
                    .push(kessel_audio::AudioEvent::PlaySfx { id: val }),
                0x1 => self
                    .sound
                    .push(kessel_audio::AudioEvent::PlayMusic { id: val }),
                0x2 => self.sound.push(kessel_audio::AudioEvent::StopMusic),
                // Latches, then a commit. See `NoteLatch`.
                0x3 => self.note_latch.frames = val,
                0x4 => self.note_latch.vel = val,
                0x5 => self.note_latch.note = val,
                0x6 => {
                    match (
                        byte(val),
                        midi(self.note_latch.note),
                        byte(self.note_latch.vel),
                    ) {
                        (Some(inst), Some(note), Some(vel)) => {
                            self.sound.push(kessel_audio::AudioEvent::Play {
                                inst,
                                note,
                                vel,
                                frames: self.note_latch.frames,
                            })
                        }
                        _ => self.sound_dropped += 1,
                    }
                }
                0x7 => self.note_latch.inst = val,
                0x8 => {
                    match (
                        byte(val),
                        byte(self.note_latch.inst),
                        midi(self.note_latch.note),
                        byte(self.note_latch.vel),
                    ) {
                        (Some(chan), Some(inst), Some(note), Some(vel)) => {
                            self.sound.push(kessel_audio::AudioEvent::NoteOn {
                                chan,
                                inst,
                                note,
                                vel,
                            })
                        }
                        _ => self.sound_dropped += 1,
                    }
                }
                0x9 => match byte(val) {
                    Some(chan) => self.sound.push(kessel_audio::AudioEvent::NoteOff { chan }),
                    None => self.sound_dropped += 1,
                },
                _ => {}
            },
            // Composite-sprite device: draw a w×h block of sheet tiles at the
            // pending screen (sx,sy) with the current sprite flags.
            0xa => match reg {
                0x0 => self.sprn_id = val,
                0x1 => self.sprn_w = val,
                0x2 => self.sprn_h = val,
                0x3 => self.draw_sprn(mem),
                _ => {}
            },
            // Scaled-sprite device: latch a scale, then blit a sheet tile scaled.
            0xb => match reg {
                0x0 => self.scale_fp = val,
                0x1 => self.draw_sprite_scaled(val, mem),
                _ => {}
            },
            // Trig device: latch the angle; sin/cos are read back via `read`.
            0xc => {
                if reg == 0x0 {
                    self.trig_angle = val;
                }
            }
            // Touch device: latch which slot the next x/y/state read describes.
            // Stored unclamped — the reads treat an impossible slot as an empty
            // one, which is a truthful answer, where clamping would hand back
            // some *other* finger's position.
            0xd if reg == 0x0 => self.touch_slot = val,
            _ => {}
        }
    }

    /// Draw a `sprn_w × sprn_h` block of sheet tiles anchored at the pending
    /// screen `(sx,sy)`. Tile ids are row-major and contiguous from `sprn_id`
    /// (id at col/row = `sprn_id + row*w + col`), each 8 px cell blitted from the
    /// tileset. The current `sprite_flags` (flip) apply to every tile; the block
    /// layout itself is not mirrored.
    fn draw_sprn(&mut self, mem: &[u8]) {
        let (base_x, base_y) = (self.sx, self.sy);
        for row in 0..self.sprn_h {
            for col in 0..self.sprn_w {
                let id = self
                    .sprn_id
                    .wrapping_add(row.wrapping_mul(self.sprn_w))
                    .wrapping_add(col);
                let addr = self.tileset_base.wrapping_add(id.wrapping_mul(32));
                self.sx = base_x.wrapping_add(col.wrapping_mul(8));
                self.sy = base_y.wrapping_add(row.wrapping_mul(8));
                self.blit_sprite(addr, mem);
            }
        }
        self.sx = base_x;
        self.sy = base_y;
    }

    /// Draw one 3×5 glyph (`code` = ASCII) at the pending `(sx,sy)` in `scolor`.
    /// The caller advances x between characters (4 px/char). Unknown codes draw
    /// nothing. Subject to the camera (via `put_pixel`) — reset `camera(0,0)`
    /// before HUD text.
    fn draw_glyph(&mut self, code: u8) {
        let (x0, y0, color) = (self.sx, self.sy, self.scolor);
        for (r, bits) in glyph_rows(code).iter().enumerate() {
            for col in 0..3u16 {
                if bits & (0x4 >> col) != 0 {
                    self.put_pixel(x0 + col, y0 + r as u16, color);
                }
            }
        }
    }

    /// Draw the pending map region: for each cell, read the tile id from
    /// `mem[map_base + (map_ty+row)*map_width + (map_tx+col)]` and blit that
    /// sheet tile at screen `(map_sx+col*8, map_sy+row*8)`. The camera applies
    /// (via `put_pixel`); sprite flip is forced off for map tiles.
    fn draw_map(&mut self, mem: &[u8]) {
        let saved_flags = self.sprite_flags;
        self.sprite_flags = 0;
        for row in 0..self.map_th {
            for col in 0..self.map_tw {
                let mx = self.map_tx.wrapping_add(col);
                let my = self.map_ty.wrapping_add(row);
                let cell = self
                    .map_base
                    .wrapping_add(my.wrapping_mul(self.map_width))
                    .wrapping_add(mx) as usize;
                let id = mem.get(cell).copied().unwrap_or(0) as u16;
                let addr = self.tileset_base.wrapping_add(id.wrapping_mul(32));
                self.sx = self.map_sx.wrapping_add(col.wrapping_mul(8));
                self.sy = self.map_sy.wrapping_add(row.wrapping_mul(8));
                self.blit_sprite(addr, mem);
            }
        }
        self.sprite_flags = saved_flags;
    }

    /// Clear the per-frame reported state (entities + console output) and
    /// advance per-frame input/timing: the previous gamepad and touch snapshots
    /// (for `btnp`/`btnr` and the touch edges) and the frame counter.
    pub fn begin_frame(&mut self, input: impl Into<Input>) {
        let input = input.into();
        self.prev_gamepad = self.gamepad;
        self.gamepad = input.buttons;
        for (was, now) in self.prev_touch_down.iter_mut().zip(self.touches.iter()) {
            *was = now.down;
        }
        self.touches = input.touches;
        // After the touches land and before anything reads them: the recognizer
        // needs this frame's positions against last frame's down flags.
        self.track_gestures();
        self.stick_x = input.stick_x.clamp(-STICK_FULL, STICK_FULL);
        self.stick_y = input.stick_y.clamp(-STICK_FULL, STICK_FULL);
        self.frame_count = self.frame_count.wrapping_add(1);
        self.entities.clear();
        self.console.clear();
        self.sound.clear();
        self.halt_requested = false;
    }

    /// Screen edge length in pixels.
    pub fn dim(&self) -> usize {
        self.dim
    }

    /// Total framebuffer cells.
    pub fn pixels(&self) -> usize {
        self.dim * self.dim
    }

    /// Switch resolution, resizing and clearing the framebuffer.
    ///
    /// Called once when a ROM is loaded, from its `screen { … }` metadata —
    /// never mid-run. A game's art and layout are authored for one size, so a
    /// resolution that could change under it would be a bug source, not a
    /// feature. The clear matters: a resized buffer would otherwise show the
    /// previous ROM's pixels reinterpreted at the new stride, which is exactly
    /// the plausible-but-wrong picture that is hard to diagnose.
    pub fn set_mode(&mut self, mode: VideoMode) {
        self.dim = mode.dim();
        self.framebuffer = vec![0u8; mode.pixels()];
    }

    /// Map a sprite's 4-bit pixel `nibble` onto a palette index through the
    /// current bank: bank `b` draws nibble `n` as colour `b * 16 + n`.
    ///
    /// This is how 4bpp art reaches a 256-colour palette without changing the
    /// sprite format. Bank 0 is the identity, so every existing sprite keeps
    /// exactly the colours it had, and one tile can be drawn in sixteen
    /// different colour schemes by changing a single register.
    ///
    /// Nibble 0 never gets here — it is transparent in every bank, so a bank
    /// switch can't accidentally make a sprite's holes opaque.
    fn banked(&self, nibble: u8) -> u8 {
        (self.sprite_bank << 4) | (nibble & 0x0f)
    }

    /// Draw a pixel at world coordinate (x, y). The camera offset translates
    /// world→screen; off-screen pixels are clipped.
    fn put_pixel(&mut self, x: u16, y: u16, color: u8) {
        let sx = x as i32 - self.cam_x as i32;
        let sy = y as i32 - self.cam_y as i32;
        let dim = self.dim as i32;
        if (0..dim).contains(&sx) && (0..dim).contains(&sy) {
            self.framebuffer[sy as usize * self.dim + sx as usize] = color;
        }
    }

    /// Blit an 8×8, 4-bits-per-pixel sprite from `mem[addr..addr+32]` at the
    /// current (sx, sy). Two pixels per byte (high nibble = left). Colour 0 is
    /// transparent. `sprite_flags` bit0/bit1 mirror the source horizontally /
    /// vertically. The destination position is subject to the camera (via
    /// `put_pixel`).
    fn blit_sprite(&mut self, addr: u16, mem: &[u8]) {
        let flip_x = self.sprite_flags & 0x01 != 0;
        let flip_y = self.sprite_flags & 0x02 != 0;
        for row in 0u16..8 {
            for col in 0u16..8 {
                let src_col = if flip_x { 7 - col } else { col };
                let src_row = if flip_y { 7 - row } else { row };
                let byte_addr = addr.wrapping_add(src_row * 4 + src_col / 2) as usize;
                let byte = mem.get(byte_addr).copied().unwrap_or(0);
                let ci = if src_col % 2 == 0 {
                    byte >> 4
                } else {
                    byte & 0x0f
                };
                if ci != 0 {
                    let c = self.banked(ci);
                    self.put_pixel(self.sx.wrapping_add(col), self.sy.wrapping_add(row), c);
                }
            }
        }
    }

    /// Fill a horizontal span from world x `xa`..`xb` (inclusive, order-free) at
    /// row `y` in `color`. The camera translates world→screen and the span is
    /// clipped to the framebuffer, so the loop runs at most one screen width —
    /// cheap enough to draw a full pseudo-3D road one row at a time.
    fn draw_hline(&mut self, xa: u16, xb: u16, y: u16, color: u8) {
        let sy = y as i32 - self.cam_y as i32;
        if !(0..self.dim as i32).contains(&sy) {
            return;
        }
        // Interpret the endpoints as signed, so a span whose left edge runs off
        // the screen (a road center that dips below 0) clips instead of wrapping
        // to a huge positive x and vanishing.
        let (xa, xb) = (xa as i16 as i32, xb as i16 as i32);
        let (lo, hi) = if xa <= xb { (xa, xb) } else { (xb, xa) };
        let sxa = (lo - self.cam_x as i32).max(0);
        let sxb = (hi - self.cam_x as i32).min(self.dim as i32 - 1);
        let c = color;
        let row = sy as usize * self.dim;
        let mut sx = sxa;
        while sx <= sxb {
            self.framebuffer[row + sx as usize] = c;
            sx += 1;
        }
    }

    /// Blit sheet tile `id` at the pending `(sx,sy)`, nearest-neighbour scaled by
    /// `scale_fp` (8.8 fixed: 256 = 1.0). Colour 0 stays transparent and the
    /// current `sprite_flags` (flip) apply. The destination side is clamped to a
    /// screen width so an absurd scale can't blow up the pixel loop — for
    /// distance-scaled cars, signs and trees in a racer.
    fn draw_sprite_scaled(&mut self, id: u16, mem: &[u8]) {
        let addr = self.tileset_base.wrapping_add(id.wrapping_mul(32));
        let flip_x = self.sprite_flags & 0x01 != 0;
        let flip_y = self.sprite_flags & 0x02 != 0;
        // Destination side length in px = 8 * scale / 256, at least 1.
        let dst = ((8u32 * self.scale_fp as u32 / 256).max(1)).min(self.dim as u32) as u16;
        for dy in 0..dst {
            let src_row0 = (dy as u32 * 8 / dst as u32) as u16; // 0..7
            let src_row = if flip_y { 7 - src_row0 } else { src_row0 };
            for dx in 0..dst {
                let src_col0 = (dx as u32 * 8 / dst as u32) as u16;
                let src_col = if flip_x { 7 - src_col0 } else { src_col0 };
                let byte_addr = addr.wrapping_add(src_row * 4 + src_col / 2) as usize;
                let byte = mem.get(byte_addr).copied().unwrap_or(0);
                let ci = if src_col % 2 == 0 {
                    byte >> 4
                } else {
                    byte & 0x0f
                };
                if ci != 0 {
                    let c = self.banked(ci);
                    self.put_pixel(self.sx.wrapping_add(dx), self.sy.wrapping_add(dy), c);
                }
            }
        }
    }

    /// xorshift32 — deterministic given the seed, returns the low 16 bits.
    fn next_rand(&mut self) -> u16 {
        let mut x = self.rng_state;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.rng_state = x;
        (x & 0xffff) as u16
    }

    /// Expand the palette-index framebuffer into packed RGBA (4 bytes/pixel),
    /// for the host window and the PNG encoder.
    pub fn framebuffer_rgba(&self) -> Vec<u8> {
        let mut out = vec![0u8; self.pixels() * 4];
        self.framebuffer_rgba_into(&mut out);
        out
    }

    /// The same expansion, written into a caller-owned buffer. Returns false if
    /// `dst` is smaller than `dim * dim * 4`.
    ///
    /// This exists for hosts that blit every frame at 60 Hz — a mobile app
    /// filling a direct `ByteBuffer`, say. Handing them the allocating variant
    /// would churn 64 KiB per frame through their allocator for nothing.
    pub fn framebuffer_rgba_into(&self, dst: &mut [u8]) -> bool {
        if dst.len() < self.pixels() * 4 {
            return false;
        }
        for (px, &idx) in dst.chunks_exact_mut(4).zip(self.framebuffer.iter()) {
            let (r, g, b) = self.palette[idx as usize];
            px.copy_from_slice(&[r, g, b, 0xff]);
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every test here runs the default screen.
    const CLASSIC_PIXELS: usize = CLASSIC_DIM * CLASSIC_DIM;

    #[test]
    fn pixel_and_cls() {
        let mut d = Devices::new();
        let mem = [0u8; 8];
        d.write(0x13, 5, &mem); // color = 5
        d.write(0x11, 10, &mem); // x = 10
        d.write(0x12, 20, &mem); // y = 20
        d.write(0x14, 0, &mem); // pixel
        assert_eq!(d.framebuffer[20 * CLASSIC_DIM + 10], 5);
        d.write(0x16, 3, &mem); // cls color 3
        assert!(d.framebuffer.iter().all(|&p| p == 3));
    }

    #[test]
    fn pixel_out_of_bounds_ignored() {
        let mut d = Devices::new();
        let mem = [0u8; 8];
        d.write(0x11, 999, &mem);
        d.write(0x12, 999, &mem);
        d.write(0x14, 0, &mem); // should be a no-op, not a panic
        assert!(d.framebuffer.iter().all(|&p| p == 0));
    }

    #[test]
    fn sprite_blit_4bpp() {
        let mut d = Devices::new();
        // Top row: pixels [1,2,3,4,0,0,0,0] -> bytes 0x12, 0x34, 0x00, 0x00
        let mut mem = [0u8; 64];
        mem[0] = 0x12;
        mem[1] = 0x34;
        d.write(0x11, 0, &mem); // x
        d.write(0x12, 0, &mem); // y
        d.write(0x15, 0, &mem); // sprite from addr 0
        assert_eq!(d.framebuffer[0], 1);
        assert_eq!(d.framebuffer[1], 2);
        assert_eq!(d.framebuffer[2], 3);
        assert_eq!(d.framebuffer[3], 4);
        assert_eq!(d.framebuffer[4], 0); // transparent stays background
    }

    #[test]
    fn camera_offset_translates_and_clips() {
        let mut d = Devices::new();
        let mem = [0u8; 8];
        d.write(0x17, 10, &mem); // cam_x = 10
        d.write(0x18, 5, &mem); // cam_y = 5
        d.write(0x13, 6, &mem); // colour 6
                                // World (12,7) -> screen (2,2).
        d.write(0x11, 12, &mem);
        d.write(0x12, 7, &mem);
        d.write(0x14, 0, &mem); // pixel
        assert_eq!(d.framebuffer[2 * CLASSIC_DIM + 2], 6);
        // World (0,0) -> screen (-10,-5) -> clipped.
        d.write(0x11, 0, &mem);
        d.write(0x12, 0, &mem);
        d.write(0x14, 0, &mem);
        assert_eq!(d.framebuffer[0], 0);
    }

    #[test]
    fn sprite_flip_x() {
        let mut d = Devices::new();
        // Top row pixels [1,2,3,4,0,0,0,0].
        let mut mem = [0u8; 64];
        mem[0] = 0x12;
        mem[1] = 0x34;
        d.write(0x19, 0x01, &mem); // flip-x
        d.write(0x11, 0, &mem);
        d.write(0x12, 0, &mem);
        d.write(0x15, 0, &mem); // sprite
                                // Mirrored: col 7 <- src 0 (=1), col 4 <- src 3 (=4).
        assert_eq!(d.framebuffer[7], 1);
        assert_eq!(d.framebuffer[6], 2);
        assert_eq!(d.framebuffer[5], 3);
        assert_eq!(d.framebuffer[4], 4);
        assert_eq!(d.framebuffer[0], 0); // src 7 was transparent
    }

    #[test]
    fn blit_sprite_by_id() {
        let mut d = Devices::new();
        let mut mem = [0u8; 128];
        // Tile id 1 lives at base(0) + 1*32 = 32; its top-left pixel is colour 5.
        mem[32] = 0x50;
        d.write(0x1b, 0, &mem); // tileset base = 0
        d.write(0x11, 3, &mem); // x = 3
        d.write(0x12, 4, &mem); // y = 4
        d.write(0x1a, 1, &mem); // blit id 1
        assert_eq!(d.framebuffer[4 * CLASSIC_DIM + 3], 5);
    }

    #[test]
    fn draw_map_blits_cells() {
        let mut d = Devices::new();
        let mut mem = vec![0u8; 256];
        // Sheet at 100: tile 1's top-left pixel is colour 5 (tile 0 stays blank).
        let sheet = 100usize;
        mem[sheet + 32] = 0x50;
        d.write(0x1b, sheet as u16, &mem); // tileset base
                                           // 2x2 map at 0, width 2: cells [0,1 / 1,0].
        mem[0] = 0;
        mem[1] = 1;
        mem[2] = 1;
        mem[3] = 0;
        d.write(0x70, 0, &mem); // map base
        d.write(0x71, 2, &mem); // map width
        d.write(0x72, 0, &mem); // tx
        d.write(0x73, 0, &mem); // ty
        d.write(0x74, 0, &mem); // sx
        d.write(0x75, 0, &mem); // sy
        d.write(0x76, 2, &mem); // tw
        d.write(0x77, 2, &mem); // th
        d.write(0x78, 0, &mem); // draw
        assert_eq!(d.framebuffer[8], 5); // cell (1,0) = tile 1 -> screen (8,0)
        assert_eq!(d.framebuffer[8 * CLASSIC_DIM], 5); // cell (0,1) -> screen (0,8)
        assert_eq!(d.framebuffer[0], 0); // cell (0,0) = tile 0 (blank)
    }

    #[test]
    fn gamepad_and_entities() {
        let mut d = Devices::new();
        let mem = [0u8; 8];
        d.begin_frame(BTN_LEFT | BTN_A);
        assert_eq!(d.read(0x20), (BTN_LEFT | BTN_A) as u16);
        d.write(0x50, 34, &mem); // ent x
        d.write(0x51, 110, &mem); // ent y
        d.write(0x52, 1, &mem); // commit tag 1
        assert_eq!(
            d.entities,
            vec![Entity {
                tag: 1,
                x: 34,
                y: 110
            }]
        );
    }

    /// The stick reads back as two's complement, so a game declaring the value
    /// `int` sees a negative number rather than a very large positive one.
    #[test]
    fn the_stick_reads_back_signed() {
        let mut d = Devices::new();
        d.begin_frame(Input {
            stick_x: -STICK_FULL,
            stick_y: 128,
            ..Input::default()
        });
        assert_eq!(d.read(0x23) as i16, -STICK_FULL);
        assert_eq!(d.read(0x24) as i16, 128);
    }

    /// A host that reports more than full deflection must not be able to make a
    /// game move faster than its own maths allows for.
    #[test]
    fn the_stick_is_clamped_to_full_deflection() {
        let mut d = Devices::new();
        d.begin_frame(Input {
            stick_x: 30_000,
            stick_y: -30_000,
            ..Input::default()
        });
        assert_eq!(d.read(0x23) as i16, STICK_FULL);
        assert_eq!(d.read(0x24) as i16, -STICK_FULL);
    }

    /// Select a slot, then read it — the trig device's shape, applied to touch.
    #[test]
    fn touch_slots_report_position_and_count() {
        let mut d = Devices::new();
        let mem = [0u8; 8];
        let mut input = Input::default();
        input.touches[0] = Touch {
            x: 10,
            y: 20,
            down: true,
        };
        input.touches[2] = Touch {
            x: 200,
            y: 5,
            down: true,
        };
        d.begin_frame(input);

        assert_eq!(d.read(0xd0), 2, "two fingers are down");
        d.write(0xd0, 2, &mem);
        assert_eq!((d.read(0xd1), d.read(0xd2)), (200, 5));
        d.write(0xd0, 1, &mem);
        assert_eq!(
            (d.read(0xd1), d.read(0xd2), d.read(0xd3)),
            (0, 0, 0),
            "an empty slot reads as empty"
        );
    }

    /// Press and release are per slot and last exactly one frame — the touch
    /// equivalent of `btnp`/`btnr`, and what makes a tap distinguishable from a
    /// finger that has been resting there for a second.
    #[test]
    fn touch_edges_last_one_frame() {
        let mut d = Devices::new();
        let mem = [0u8; 8];
        let down = |x, y| {
            let mut i = Input::default();
            i.touches[0] = Touch { x, y, down: true };
            i
        };

        d.write(0xd0, 0, &mem);
        d.begin_frame(down(4, 4));
        assert_eq!(d.read(0xd3), 0b011, "down + pressed");

        d.begin_frame(down(5, 5));
        assert_eq!(d.read(0xd3), 0b001, "still down, no longer a new press");

        d.begin_frame(Input::default());
        assert_eq!(d.read(0xd3), 0b100, "released");

        d.begin_frame(Input::default());
        assert_eq!(d.read(0xd3), 0, "and the release does not repeat");
    }

    /// Drag one finger through `path`, returning what slot 0's gesture
    /// registers read at each step: `(swipe, dx, dy, frames)`.
    fn drag_path(d: &mut Devices, path: &[(u16, u16)]) -> Vec<(u16, i16, i16, u16)> {
        let mem = [0u8; 8];
        d.write(0xd0, 0, &mem); // latch slot 0
        path.iter()
            .map(|&(x, y)| {
                let mut i = Input::default();
                i.touches[0] = Touch { x, y, down: true };
                d.begin_frame(i);
                (
                    d.read(0xd4),
                    d.read(0xd5) as i16,
                    d.read(0xd6) as i16,
                    d.read(0xd7),
                )
            })
            .collect()
    }

    /// The threshold is a fraction of the screen, so the *same* drag is a swipe
    /// on Classic and merely a drag on Extended. That is the point: the screens
    /// are the same physical size, so a gesture should be the same fraction of
    /// it rather than the same pixel count.
    #[test]
    fn the_swipe_threshold_scales_with_the_screen() {
        assert_eq!(CLASSIC_DIM / SWIPE_DIVISOR, 16);
        assert_eq!(EXTENDED_DIM / SWIPE_DIVISOR, 30);

        // 20 px right: past Classic's 16, short of Extended's 30.
        let steps = [(40, 40), (60, 40)];
        let mut classic = Devices::new();
        assert_eq!(drag_path(&mut classic, &steps)[1].0, BTN_RIGHT as u16);
        let mut extended = Devices::with_mode(VideoMode::Extended240);
        assert_eq!(drag_path(&mut extended, &steps)[1].0, 0);
    }

    /// A swipe is recognized *mid-gesture*, the frame the finger passes the
    /// threshold — iOS's `UISwipeGestureRecognizer` timing rather than Android's
    /// `onFling`, which waits for the lift. A console wants the board to move as
    /// you swipe, not after you let go.
    #[test]
    fn a_swipe_fires_once_mid_gesture_not_on_release() {
        let mut d = Devices::new();
        let seen = drag_path(
            &mut d,
            &[
                (60, 60), // press: no travel yet
                (70, 60), // 10 px — under the 16 px threshold
                (80, 60), // 20 px — recognized here
                (95, 60), // still dragging: one press is one swipe
            ],
        );
        assert_eq!(seen[0].0, 0, "a landing finger cannot have swiped");
        assert_eq!(seen[1].0, 0, "under the threshold");
        assert_eq!(seen[2].0, BTN_RIGHT as u16, "recognized while still down");
        assert_eq!(seen[3].0, 0, "one press is one swipe");

        // Lifting reports nothing extra — the swipe already happened.
        d.begin_frame(Input::default());
        assert_eq!(d.read(0xd4), 0);
    }

    /// Each of the four directions, by dominant axis. Never a diagonal: every
    /// game that wants a swipe wants one of four answers.
    #[test]
    fn a_swipe_reports_the_dominant_axis_as_a_button_bit() {
        for (to, expect) in [
            ((20, 60), BTN_LEFT),
            ((100, 60), BTN_RIGHT),
            ((60, 20), BTN_UP),
            ((60, 100), BTN_DOWN),
            // Mostly right, slightly up: the dominant axis wins outright.
            ((100, 50), BTN_RIGHT),
        ] {
            let mut d = Devices::new();
            let seen = drag_path(&mut d, &[(60, 60), to]);
            assert_eq!(
                seen[1].0, expect as u16,
                "dragging from (60,60) to {to:?} should report {expect:#04x}"
            );
        }
    }

    /// An exactly diagonal drag has to resolve *somewhere* deterministically,
    /// or the same replayed frame gives two answers.
    #[test]
    fn a_perfectly_diagonal_swipe_breaks_the_tie_toward_x() {
        let mut d = Devices::new();
        let seen = drag_path(&mut d, &[(60, 60), (90, 90)]);
        assert_eq!(seen[1].0, BTN_RIGHT as u16);
    }

    /// The continuous half: displacement from the origin, signed, plus how long
    /// the press has lasted. This is what a game needs that a bare direction
    /// cannot give it — and it is signed so a leftward drag does not wrap.
    #[test]
    fn a_drag_reports_signed_displacement_and_its_age() {
        let mut d = Devices::new();
        let seen = drag_path(&mut d, &[(60, 60), (50, 70), (30, 90)]);

        assert_eq!((seen[0].1, seen[0].2), (0, 0), "the origin is the origin");
        assert_eq!((seen[1].1, seen[1].2), (-10, 10), "left and down of it");
        assert_eq!((seen[2].1, seen[2].2), (-30, 30));

        assert_eq!(seen[0].3, 0, "a landing finger is zero frames old");
        assert_eq!((seen[1].3, seen[2].3), (1, 2));
    }

    /// A game already has the current position, so a signed delta hands back the
    /// origin for free — which is why the delta is what the device exposes.
    #[test]
    fn the_origin_is_recoverable_from_position_and_delta() {
        let mut d = Devices::new();
        drag_path(&mut d, &[(60, 60), (30, 90)]);
        let (x, y) = (d.read(0xd1) as i32, d.read(0xd2) as i32);
        let (dx, dy) = (d.read(0xd5) as i16 as i32, d.read(0xd6) as i16 as i32);
        assert_eq!((x - dx, y - dy), (60, 60));
    }

    /// Lifting ends the gesture outright: a released finger has no displacement,
    /// and a stale one would have a game keep steering after the player let go.
    #[test]
    fn releasing_clears_the_drag_and_starts_a_new_gesture() {
        let mut d = Devices::new();
        let mem = [0u8; 8];
        d.write(0xd0, 0, &mem);
        drag_path(&mut d, &[(60, 60), (30, 60)]);
        assert_eq!(d.read(0xd5) as i16, -30);

        d.begin_frame(Input::default());
        assert_eq!((d.read(0xd5) as i16, d.read(0xd6) as i16), (0, 0));
        assert_eq!(d.read(0xd7), 0, "frames reset with the gesture");

        // A fresh press swipes again — "one press is one swipe", not "one swipe
        // ever".
        let again = drag_path(&mut d, &[(60, 60), (100, 60)]);
        assert_eq!(again[1].0, BTN_RIGHT as u16);
    }

    /// Gestures are per slot, so two thumbs can swipe opposite ways at once.
    /// A screen-level swipe register would have made one of them disappear.
    #[test]
    fn two_fingers_swipe_independently() {
        let mut d = Devices::new();
        let mem = [0u8; 8];
        let frame = |d: &mut Devices, a: (u16, u16), b: (u16, u16)| {
            let mut i = Input::default();
            i.touches[0] = Touch {
                x: a.0,
                y: a.1,
                down: true,
            };
            i.touches[1] = Touch {
                x: b.0,
                y: b.1,
                down: true,
            };
            d.begin_frame(i);
        };
        frame(&mut d, (60, 60), (60, 60));
        frame(&mut d, (100, 60), (20, 60));

        d.write(0xd0, 0, &mem);
        assert_eq!((d.read(0xd4), d.read(0xd5) as i16), (BTN_RIGHT as u16, 40));
        d.write(0xd0, 1, &mem);
        assert_eq!((d.read(0xd4), d.read(0xd5) as i16), (BTN_LEFT as u16, -40));
    }

    /// A slot the console does not have reads as an empty one rather than
    /// wrapping onto a real finger. Same rule as an off-screen `pset`: the only
    /// answer that cannot make a game act on something the player never did.
    #[test]
    fn an_impossible_touch_slot_reads_empty() {
        let mut d = Devices::new();
        let mem = [0u8; 8];
        let mut input = Input::default();
        input.touches[0] = Touch {
            x: 77,
            y: 88,
            down: true,
        };
        d.begin_frame(input);

        d.write(0xd0, 9999, &mem);
        assert_eq!((d.read(0xd1), d.read(0xd2), d.read(0xd3)), (0, 0, 0));
        // The real finger is still reachable — the guard is a range, not a
        // blanket refusal.
        d.write(0xd0, 0, &mem);
        assert_eq!((d.read(0xd1), d.read(0xd2)), (77, 88));
    }

    #[test]
    fn rng_is_deterministic_and_seedable() {
        let mut a = Devices::new();
        let mut b = Devices::new();
        let mem = [0u8; 8];
        a.write(0x30, 42, &mem);
        b.write(0x30, 42, &mem);
        let sa: Vec<u16> = (0..5).map(|_| a.read(0x30)).collect();
        let sb: Vec<u16> = (0..5).map(|_| b.read(0x30)).collect();
        assert_eq!(sa, sb);
        // Different seed -> different stream (overwhelmingly likely).
        let mut c = Devices::new();
        c.write(0x30, 43, &mem);
        let sc: Vec<u16> = (0..5).map(|_| c.read(0x30)).collect();
        assert_ne!(sa, sc);
    }

    #[test]
    fn storage_persists() {
        let mut d = Devices::new();
        let mem = [0u8; 8];
        d.write(0x40, 7, &mem); // addr = 7
        d.write(0x42, 99, &mem); // write 99
        d.write(0x40, 7, &mem); // addr = 7
        assert_eq!(d.read(0x41), 99);
    }

    #[test]
    fn palette_write_commits_on_the_index_strobe() {
        let mut d = Devices::new();
        let mem = [0u8; 8];
        d.write(0x02, 0x11, &mem); // r
        d.write(0x03, 0x22, &mem); // g
        d.write(0x04, 0x33, &mem); // b
        assert_ne!(d.palette[2], (0x11, 0x22, 0x33), "must not commit early");
        d.write(0x01, 2, &mem); // index -> commit
        assert_eq!(d.palette[2], (0x11, 0x22, 0x33));
    }

    /// The whole 256-entry range is writable — the high indices are what a
    /// 240×240 game with a deep palette actually uses.
    #[test]
    fn palette_reaches_index_255() {
        let mut d = Devices::new();
        let mem = [0u8; 8];
        d.write(0x02, 1, &mem);
        d.write(0x03, 2, &mem);
        d.write(0x04, 3, &mem);
        d.write(0x01, 255, &mem);
        assert_eq!(d.palette[255], (1, 2, 3));
    }

    /// The default palette must be a *known* 256 colours, not 16 real ones and
    /// 240 blacks — a game drawing in index 200 should see a colour.
    #[test]
    fn default_palette_fills_all_256_entries() {
        let d = Devices::new();
        assert_eq!(d.palette[0..16], BASE_16, "base 16 must be unchanged");
        // 6x6x6 cube: index 16 is black, 231 is white, and rgb6 names them.
        assert_eq!(d.palette[rgb6(0, 0, 0) as usize], (0x00, 0x00, 0x00));
        assert_eq!(d.palette[rgb6(5, 5, 5) as usize], (0xFF, 0xFF, 0xFF));
        assert_eq!(d.palette[rgb6(5, 0, 0) as usize], (0xFF, 0x00, 0x00));
        // Grey ramp ascends and is neutral.
        let (r, g, b) = d.palette[240];
        assert!(
            r == g && g == b,
            "grey ramp must be neutral, got {r},{g},{b}"
        );
        assert!(d.palette[255].0 > d.palette[232].0, "ramp must ascend");
    }

    /// Colours above 15 must survive the trip to the framebuffer. They used to
    /// be masked to a nibble, which silently drew a different colour.
    #[test]
    fn colours_above_15_are_not_masked() {
        let mut d = Devices::new();
        let mem = [0u8; 8];
        d.write(0x13, 200, &mem); // color = 200
        d.write(0x11, 5, &mem);
        d.write(0x12, 6, &mem);
        d.write(0x14, 0, &mem); // pixel
        assert_eq!(d.framebuffer[6 * CLASSIC_DIM + 5], 200);

        d.write(0x16, 231, &mem); // cls to a cube colour
        assert!(d.framebuffer.iter().all(|&p| p == 231));
    }

    /// A sprite nibble is drawn as `bank * 16 + nibble`, so one 4bpp tile can
    /// wear sixteen different colour schemes.
    #[test]
    fn sprite_bank_offsets_the_nibble() {
        let mut mem = [0u8; 64];
        // One 8x8 tile at addr 0: top-left pixel is nibble 5, rest transparent.
        mem[0] = 0x50;
        let mut d = Devices::new();

        d.write(0x11, 0, &mem);
        d.write(0x12, 0, &mem);
        d.write(0x15, 0, &mem); // blit raw at (0,0), bank 0
        assert_eq!(d.framebuffer[0], 5, "bank 0 must be the identity");

        d.write(0x1e, 3, &mem); // bank 3
        d.write(0x11, 8, &mem);
        d.write(0x12, 0, &mem);
        d.write(0x15, 0, &mem);
        assert_eq!(d.framebuffer[8], 0x35, "bank 3 -> 3*16 + 5");
    }

    /// Nibble 0 is a hole in every bank. If a bank switch made it opaque,
    /// sprites would gain a solid background at bank 1+.
    #[test]
    fn nibble_zero_stays_transparent_in_every_bank() {
        let mut mem = [0u8; 64];
        mem[0] = 0x50; // pixel (0,0) = 5, pixel (1,0) = 0
        let mut d = Devices::new();
        d.write(0x16, 9, &mem); // fill with colour 9
        d.write(0x1e, 7, &mem); // bank 7
        d.write(0x11, 0, &mem);
        d.write(0x12, 0, &mem);
        d.write(0x15, 0, &mem);
        assert_eq!(d.framebuffer[0], 0x75, "opaque nibble is banked");
        assert_eq!(d.framebuffer[1], 9, "nibble 0 left the background alone");
    }

    #[test]
    fn extended_mode_resizes_and_clears_the_framebuffer() {
        let mut d = Devices::new();
        let mem = [0u8; 8];
        d.write(0x16, 7, &mem); // dirty the classic framebuffer
        assert_eq!(d.dim(), CLASSIC_DIM);

        d.set_mode(VideoMode::Extended240);
        assert_eq!(d.dim(), EXTENDED_DIM);
        assert_eq!(d.framebuffer.len(), EXTENDED_DIM * EXTENDED_DIM);
        assert!(
            d.framebuffer.iter().all(|&p| p == 0),
            "stale pixels would be re-read at the new stride"
        );
    }

    /// Clipping must follow the *current* screen, not a compile-time constant.
    /// x=200 is off-screen on Classic and on-screen on Extended.
    #[test]
    fn clipping_follows_the_active_mode() {
        let mem = [0u8; 8];
        let mut d = Devices::new();
        d.write(0x13, 5, &mem);
        d.write(0x11, 200, &mem);
        d.write(0x12, 10, &mem);
        d.write(0x14, 0, &mem);
        assert!(d.framebuffer.iter().all(|&p| p == 0), "clipped on Classic");

        let mut d = Devices::with_mode(VideoMode::Extended240);
        d.write(0x13, 5, &mem);
        d.write(0x11, 200, &mem);
        d.write(0x12, 10, &mem);
        d.write(0x14, 0, &mem);
        assert_eq!(
            d.framebuffer[10 * EXTENDED_DIM + 200],
            5,
            "drawn on Extended"
        );
    }

    #[test]
    fn framebuffer_rgba_uses_palette() {
        let mut d = Devices::new();
        let mem = [0u8; 8];
        d.write(0x16, 7, &mem); // cls to color 7 = (0xFF,0xF1,0xE8)
        let rgba = d.framebuffer_rgba();
        assert_eq!(&rgba[0..4], &[0xFF, 0xF1, 0xE8, 0xFF]);
        assert_eq!(rgba.len(), CLASSIC_PIXELS * 4);
    }

    #[test]
    fn trig_fp_cardinal_points() {
        // 0..256 = one turn; results are signed 8.8 fixed, exact at the axes.
        assert_eq!(trig_fp(0, false) as i16, 0); // sin 0
        assert_eq!(trig_fp(64, false) as i16, 256); // sin 90
        assert_eq!(trig_fp(128, false) as i16, 0); // sin 180
        assert_eq!(trig_fp(192, false) as i16, -256); // sin 270
        assert_eq!(trig_fp(0, true) as i16, 256); // cos 0
        assert_eq!(trig_fp(64, true) as i16, 0); // cos 90
        assert_eq!(trig_fp(128, true) as i16, -256); // cos 180
                                                     // Mid-angle magnitude is bounded and non-trivial.
        assert_eq!(trig_fp(32, false) as i16, 181); // sin 45 ~ 0.707*256
    }

    /// An out-of-range note argument emits nothing at all.
    ///
    /// Every channel `0..=255` is one a game may be holding a note on, so there
    /// is no value an invalid channel can be mapped onto without stealing
    /// someone else's — not 0 (truncation) and not 255 (clamping). Doing
    /// nothing is the only answer that cannot corrupt state.
    #[test]
    fn an_out_of_range_note_argument_emits_nothing() {
        let mut d = Devices::new();
        let mem = [0u8; 8];

        for (label, writes) in [
            ("channel", vec![(0x95u8, 60u16), (0x97, 0), (0x98, 256)]),
            ("note", vec![(0x95, 200), (0x97, 0), (0x98, 3)]),
            ("instrument", vec![(0x95, 60), (0x97, 256), (0x98, 3)]),
            (
                "velocity",
                vec![(0x94, 300), (0x95, 60), (0x97, 0), (0x98, 3)],
            ),
            ("play's instrument", vec![(0x95, 60), (0x96, 256)]),
            ("note_off's channel", vec![(0x99, 999)]),
        ] {
            d.sound.clear();
            d.sound_dropped = 0;
            d.write(0x94, 200, &mem); // a valid velocity, unless overwritten
            for (port, val) in writes {
                d.write(port, val, &mem);
            }
            assert!(
                d.sound.is_empty(),
                "an out-of-range {label} still emitted {:?}",
                d.sound
            );
            assert_eq!(
                d.sound_dropped, 1,
                "an out-of-range {label} was not counted"
            );
        }
    }

    /// ...and the valid extremes still work, so the check is a range and not a
    /// blanket refusal.
    #[test]
    fn the_ends_of_each_note_range_are_valid() {
        use kessel_audio::AudioEvent;
        let mut d = Devices::new();
        let mem = [0u8; 8];
        d.write(0x94, 255, &mem); // vel
        d.write(0x95, 127, &mem); // note
        d.write(0x97, 255, &mem); // inst
        d.write(0x98, 255, &mem); // chan -> commit
        assert_eq!(
            d.sound,
            [AudioEvent::NoteOn {
                chan: 255,
                inst: 255,
                note: 127,
                vel: 255,
            }]
        );
        assert_eq!(d.sound_dropped, 0);

        d.sound.clear();
        d.write(0x99, 0, &mem);
        assert_eq!(d.sound, [AudioEvent::NoteOff { chan: 0 }]);
    }

    #[test]
    fn hline_via_device_port() {
        let mut d = Devices::new();
        let mem = [0u8; 8];
        d.write(0x13, 5, &mem); // scolor = 5
        d.write(0x12, 3, &mem); // sy = 3
        d.write(0x11, 10, &mem); // sx = 10 (x1)
        d.write(0x1d, 14, &mem); // x2 = 14 -> draw span 10..=14 at row 3
        assert_eq!(d.framebuffer[3 * CLASSIC_DIM + 9], 0);
        assert_eq!(d.framebuffer[3 * CLASSIC_DIM + 10], 5);
        assert_eq!(d.framebuffer[3 * CLASSIC_DIM + 14], 5);
        assert_eq!(d.framebuffer[3 * CLASSIC_DIM + 15], 0);
    }

    #[test]
    fn hline_clips_negative_left_edge() {
        // x1 = -20 (0xFFEC) means "off the left edge": the span should still fill
        // 0..=30, not wrap to a huge positive x and draw nothing.
        let mut d = Devices::new();
        let mem = [0u8; 8];
        d.write(0x13, 9, &mem); // color 9
        d.write(0x12, 2, &mem); // row 2
        d.write(0x11, 0xFFEC, &mem); // x1 = -20 as u16
        d.write(0x1d, 30, &mem); // x2 = 30
        assert_eq!(d.framebuffer[2 * CLASSIC_DIM + 0], 9, "left edge drawn");
        assert_eq!(d.framebuffer[2 * CLASSIC_DIM + 30], 9, "right end drawn");
        assert_eq!(d.framebuffer[2 * CLASSIC_DIM + 31], 0);
    }
}
