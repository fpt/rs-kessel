package dev.kessel.game

import android.content.res.AssetManager

/** A game in the bundled library. */
data class Game(
    /** Asset filename, e.g. `tetris.lua` — also what the VM sees, so its
     *  extension picks the dialect (`.lua`/`.ux` luax, `.asm` assembly). */
    val fileName: String,
    /** Display name, e.g. `Tetris`. */
    val title: String,
)

/**
 * The bundled game library, read from `assets/`.
 *
 * The APK's assets include the repo's `games/` directory verbatim — see the
 * `assets.srcDir` in `app/build.gradle.kts`. Copying those files into the
 * Android project instead would fork the corpus that
 * `crates/vm/tests/games_compile.rs` guards, and the fork would quietly rot.
 */
object GameCatalog {

    /** Extensions the VM's assembler/compiler recognises. */
    private val SOURCE_EXTENSIONS = setOf("lua", "ux", "asm")

    /**
     * Where shared sources live, both in `games/` and in the APK. A game reaches
     * them by name — `#include "lib/motion.lua"` — so this string is half of a
     * path that appears in the corpus and cannot be renamed on its own.
     *
     * It is no longer the only such directory: a game whose art outgrew one file
     * splits it into `games/<game>/` and includes from there. [libraries] walks
     * every directory at the root, so this constant now names the *shared* one
     * rather than the only one.
     */
    const val LIB_DIR = "lib"

    /**
     * Every game in the APK, alphabetically by title.
     *
     * Asset roots also hold whatever the toolchain put there, so this filters by
     * extension rather than assuming everything at the root is a game. That is
     * also what keeps `lib/` out of the library screen: it is a directory, so it
     * has no extension to match.
     */
    fun list(assets: AssetManager): List<Game> =
        (assets.list("") ?: emptyArray())
            .filter { isSource(it) }
            .map { Game(fileName = it, title = titleOf(it)) }
            .sortedBy { it.title }

    /** Read a game's source out of the APK. */
    fun source(assets: AssetManager, game: Game): String =
        assets.open(game.fileName).bufferedReader().use { it.readText() }

    /**
     * Every includable source below the root, keyed by the path a game
     * `#include`s: the shared helpers in `lib/`, and the per-game directories a
     * game splits its art into (`outrun/car.lua`).
     *
     * Handed to the console before a game is loaded — the VM cannot open an
     * asset itself, so every file a game might include has to be pushed across
     * first. All of them, not the ones a given game names: finding that out
     * would mean parsing the source here, which is the compiler's job and would
     * be a second, worse implementation of it.
     *
     * Walking every directory rather than just `lib/` is what lets a game split
     * itself up at all. The alternative was to put one game's art in the shared
     * directory, and the failure mode of getting this wrong is bad: the game is
     * listed, the player taps it, and it fails to compile on the device only —
     * `kessel run` resolves the same include straight off the filesystem.
     *
     * One level deep, because that is the shape the corpus has. A nested tree
     * would want recursion and there is nothing to recurse into.
     */
    fun libraries(assets: AssetManager): Map<String, String> =
        includePaths { assets.list(it) }
            .associateWith { path ->
                assets.open(path).bufferedReader().use { it.readText() }
            }

    /**
     * The include paths one level below the root, given a directory lister.
     *
     * Separate from [libraries] so it can be tested at all: `AssetManager` is a
     * throwing stub on the unit-test classpath, and getting this rule wrong is
     * invisible everywhere except a real device.
     */
    internal fun includePaths(list: (String) -> Array<String>?): List<String> =
        (list("") ?: emptyArray())
            .filterNot { isSource(it) }
            .flatMap { dir ->
                (list(dir) ?: emptyArray()).filter { isSource(it) }.map { "$dir/$it" }
            }

    /** Is this asset something the VM can compile? */
    internal fun isSource(fileName: String): Boolean =
        fileName.substringAfterLast('.', "").lowercase() in SOURCE_EXTENSIONS

    /**
     * `outrun.lua` -> `Outrun`, `2048.lua` -> `2048`.
     *
     * Deliberately dumb: the corpus is filenames like `snake.lua`, and a game
     * that wants a prettier name should say so in the source one day rather
     * than have this guess harder.
     */
    private fun titleOf(fileName: String): String =
        fileName.substringBeforeLast('.')
            .replace('_', ' ')
            .replaceFirstChar { it.uppercase() }
}
