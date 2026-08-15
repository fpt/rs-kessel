/*
 * kessel.h — the C ABI over the kessel console's play surface.
 *
 * Link against libkessel_ffi (staticlib for iOS, cdylib for anything that
 * dlopen's it). Android does not use this header; it goes through the JNI
 * entry points in src/android.rs, which call these same functions.
 *
 * Contract, in full:
 *   - Every function tolerates a NULL handle, returning a zero value.
 *   - char* returned here is owned by the caller and must go to
 *     kessel_string_free(). Do not free() it.
 *   - A handle may be used from several threads at once; it may not be freed
 *     while another thread is using it.
 */
#ifndef KESSEL_H
#define KESSEL_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* Opaque console handle. */
typedef struct KesselPlayer KesselPlayer;

/* Screen edge lengths, by video mode. The screen is square in both. */
#define KESSEL_CLASSIC_DIM  128
#define KESSEL_EXTENDED_DIM 240

/* Gamepad bits, as passed to kessel_player_tick. */
#define KESSEL_BTN_LEFT   0x01
#define KESSEL_BTN_RIGHT  0x02
#define KESSEL_BTN_UP     0x04
#define KESSEL_BTN_DOWN   0x08
#define KESSEL_BTN_A      0x10
#define KESSEL_BTN_B      0x20
#define KESSEL_BTN_START  0x40
#define KESSEL_BTN_SELECT 0x80

/* Create / destroy a console. */
KesselPlayer *kessel_player_new(void);
void kessel_player_free(KesselPlayer *p);

/*
 * Compile and load a game. `name`'s extension picks the dialect: .lua/.ux for
 * luax, .asm for assembly.
 *
 * Returns NULL on success, or an owned diagnostics string for the caller to
 * display and then pass to kessel_string_free(). A failed load leaves the
 * console with no ROM.
 */
char *kessel_player_load(KesselPlayer *p, const char *source, const char *name);

/* How many touch slots KesselInput carries. */
#define KESSEL_MAX_TOUCHES 4

/* Full analog deflection, in signed 8.8 fixed point (256 = 1.0). */
#define KESSEL_STICK_FULL 256

/* One touch point, in console pixels — the host has already undone its own
 * letterboxing and upscale, so these are the coordinates the game draws with. */
typedef struct {
  uint16_t x;
  uint16_t y;
  bool down;
} KesselTouch;

/*
 * Everything the console reads for one frame.
 *
 * `touches` is indexed by slot, and a slot is a finger's identity for its whole
 * life: the console derives press/release edges per slot, so a host that
 * renumbers its fingers between frames reports a release and a press the player
 * never made.
 */
typedef struct {
  uint8_t buttons;
  int16_t stick_x; /* -KESSEL_STICK_FULL .. KESSEL_STICK_FULL */
  int16_t stick_y;
  KesselTouch touches[KESSEL_MAX_TOUCHES];
} KesselInput;

/* Advance one frame with `buttons` held. No-op until a ROM is loaded. */
void kessel_player_tick(KesselPlayer *p, uint8_t buttons);

/*
 * Advance one frame with the full input. A NULL `input` means "nothing held",
 * the same as kessel_player_tick(p, 0) — the frame still runs.
 */
void kessel_player_tick_input(KesselPlayer *p, const KesselInput *input);

/*
 * Screen edge length in pixels; a frame is dim*dim*4 bytes.
 *
 * Read this AFTER kessel_player_load, not before: the resolution comes from
 * the ROM's `screen { ... }` block (128 by default, 240 for Extended240), so a
 * host that sizes its buffer at start-up will tear a 240x240 game across it.
 */
uint32_t kessel_player_screen_dim(KesselPlayer *p);

/*
 * Write the current frame into `dst` as packed RGBA.
 *
 * True if a frame was written. False — with `dst` untouched, so the caller can
 * keep presenting its last good frame — if there is no ROM, `dst` is NULL, or
 * `len` is under kessel_player_screen_dim(p)^2 * 4.
 */
bool kessel_player_framebuffer(KesselPlayer *p, uint8_t *dst, size_t len);

/*
 * The loaded ROM's control metadata as JSON: which buttons the game uses and
 * what they are called, so a host can draw only the controls that do
 * something. Always a parseable object. Owned by the caller.
 */
char *kessel_player_controls_json(KesselPlayer *p);

/*
 * Give this console a synth at `sample_rate`. Opt-in: a console that never gets
 * one costs nothing, and stays silent.
 *
 * Call once, before starting an audio thread; call
 * kessel_player_audio_render() from that thread and nowhere else.
 */
bool kessel_player_audio_enable(KesselPlayer *p, uint32_t sample_rate);

/*
 * Render `frames` stereo frames into `out`, which must hold `frames * 2`
 * floats. Returns the frames written, or 0 (with `out` zeroed) when there is no
 * synth.
 *
 * Safe to call while another thread is ticking the game: this path never takes
 * the console's lock, so a slow frame cannot delay a buffer. It also never
 * waits — a contended synth yields silence rather than a late buffer, because
 * an audio callback that blocks is a gap in everything, not just this sound.
 */
uint32_t kessel_player_audio_render(KesselPlayer *p, float *out, uint32_t frames);

/* Sounds dropped because the game got ahead of the audio thread. */
uint64_t kessel_player_audio_dropped(KesselPlayer *p);

bool kessel_player_has_rom(KesselPlayer *p);
bool kessel_player_is_paused(KesselPlayer *p);
bool kessel_player_is_halted(KesselPlayer *p);

/* Release a string returned by this API. NULL is a no-op. */
void kessel_string_free(char *s);

#ifdef __cplusplus
}
#endif

#endif /* KESSEL_H */
