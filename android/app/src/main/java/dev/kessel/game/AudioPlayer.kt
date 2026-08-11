package dev.kessel.game

import android.media.AudioAttributes
import android.media.AudioFormat
import android.media.AudioManager
import android.media.AudioTrack
import dev.kessel.vm.KesselVm
import java.nio.ByteBuffer
import java.nio.ByteOrder

/**
 * Plays the console's sound on its own thread.
 *
 * The synth lives in native code and is driven from here, not from the game
 * loop: [KesselVm.renderAudio] is the one call that does not take the console's
 * lock, precisely so a slow frame of game code cannot delay a buffer. Rendering
 * sound inside [GameEngine]'s tick would be simpler and would produce a click
 * every time a frame ran long.
 *
 * Pacing comes from `AudioTrack.write`, which blocks until the device has room.
 * That is what makes this loop self-clocking — there is no timer here, and no
 * relationship to the game's 60 Hz beyond the events the game queues.
 *
 * **[stop] must run before the console is closed.** The render path reads the
 * native handle without the lock that protects `close`, so only a joined thread
 * makes that safe; [GameEngine] joins this one first.
 */
class AudioPlayer(private val vm: KesselVm) : AutoCloseable {

    private var track: AudioTrack? = null
    private var thread: Thread? = null

    @Volatile
    private var running = false

    /**
     * The staging buffer, direct and in native byte order.
     *
     * Direct because the native side writes into it without a copy, and native
     * order because it writes host-endian `f32` — a big-endian view here would
     * turn every sample into noise, which is the kind of mistake that sounds
     * like a broken synth rather than a broken buffer.
     */
    private val staging: ByteBuffer =
        ByteBuffer.allocateDirect(CHUNK_FRAMES * 2 * 4).order(ByteOrder.nativeOrder())

    /**
     * Give the console a synth and start playing. Returns false if there is no
     * audio device to be had, in which case the game simply runs silently —
     * sound is never a reason to refuse to play.
     */
    fun start(): Boolean {
        check(thread == null) { "audio already started" }

        val rate = nativeSampleRate()
        if (!vm.enableAudio(rate)) return false

        // Ask for a few chunks of headroom: the minimum is a floor, not a
        // recommendation, and a buffer that small underruns whenever the system
        // looks away.
        val minBytes = AudioTrack.getMinBufferSize(
            rate,
            AudioFormat.CHANNEL_OUT_STEREO,
            AudioFormat.ENCODING_PCM_FLOAT,
        )
        if (minBytes <= 0) return false
        val bytes = maxOf(minBytes, CHUNK_FRAMES * 2 * 4 * 3)

        val t = try {
            AudioTrack.Builder()
                .setAudioAttributes(
                    AudioAttributes.Builder()
                        .setUsage(AudioAttributes.USAGE_GAME)
                        .setContentType(AudioAttributes.CONTENT_TYPE_MUSIC)
                        .build()
                )
                .setAudioFormat(
                    AudioFormat.Builder()
                        .setEncoding(AudioFormat.ENCODING_PCM_FLOAT)
                        .setSampleRate(rate)
                        .setChannelMask(AudioFormat.CHANNEL_OUT_STEREO)
                        .build()
                )
                .setBufferSizeInBytes(bytes)
                .setTransferMode(AudioTrack.MODE_STREAM)
                .build()
        } catch (_: UnsupportedOperationException) {
            return false
        } catch (_: IllegalArgumentException) {
            return false
        }

        track = t
        t.play()
        running = true
        thread = Thread({ loop(t) }, "kessel-audio").apply {
            // Above the game thread: a late frame drops a frame, but a late
            // buffer is a click in everything the player can hear.
            priority = Thread.MAX_PRIORITY
            start()
        }
        return true
    }

    /** Stop playing and wait for the thread. Safe to call twice. */
    fun stop() {
        running = false
        // Pause and flush *before* joining. The loop spends most of its life
        // blocked inside `write`, waiting for the device to take more audio;
        // joining first would wait out the timeout and then release the track
        // from under a thread still inside it.
        track?.let {
            try {
                it.pause()
                it.flush()
            } catch (_: IllegalStateException) {
                // Already torn down.
            }
        }
        thread?.let {
            it.interrupt() // in case it is idling rather than writing
            it.join(JOIN_TIMEOUT_MS)
        }
        thread = null
        track?.let {
            try {
                it.stop()
            } catch (_: IllegalStateException) {
            }
            it.release()
        }
        track = null
    }

    override fun close() = stop()

    private fun loop(t: AudioTrack) {
        val bytes = CHUNK_FRAMES * 2 * 4
        while (running) {
            staging.clear()
            val frames = vm.renderAudio(staging, CHUNK_FRAMES)
            if (frames <= 0) {
                // No synth, or the console is gone. Don't spin on it.
                try {
                    Thread.sleep(IDLE_MS)
                } catch (_: InterruptedException) {
                    return // stop() is waiting for us
                }
                continue
            }
            staging.position(0)
            staging.limit(frames * 2 * 4)
            // Blocking: this is the loop's clock. A short write means the track
            // is being torn down, so leave rather than spin.
            if (t.write(staging, minOf(bytes, frames * 2 * 4), AudioTrack.WRITE_BLOCKING) < 0) {
                return
            }
        }
    }

    private fun nativeSampleRate(): Int {
        val rate = AudioTrack.getNativeOutputSampleRate(AudioManager.STREAM_MUSIC)
        // The synth runs at whatever the device wants, so there is never a
        // resampler in the path; the fallback is only for a device that will
        // not say.
        return if (rate > 0) rate else 48_000
    }

    private companion object {
        /**
         * Frames per render. About 10 ms at 48 kHz — comfortably under one
         * 60 Hz frame, so a sound queued by the game is heard on time, and long
         * enough that the thread is not woken constantly.
         */
        const val CHUNK_FRAMES = 512

        /** How long to wait for a sleeping render thread on the way out. */
        const val JOIN_TIMEOUT_MS = 200L

        /** Idle poll while there is nothing to render. */
        const val IDLE_MS = 10L
    }
}
