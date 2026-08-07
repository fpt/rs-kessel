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
     * Every game in the APK, alphabetically by title.
     *
     * Asset roots also hold whatever the toolchain put there, so this filters by
     * extension rather than assuming everything at the root is a game.
     */
    fun list(assets: AssetManager): List<Game> =
        (assets.list("") ?: emptyArray())
            .filter { it.substringAfterLast('.', "").lowercase() in SOURCE_EXTENSIONS }
            .map { Game(fileName = it, title = titleOf(it)) }
            .sortedBy { it.title }

    /** Read a game's source out of the APK. */
    fun source(assets: AssetManager, game: Game): String =
        assets.open(game.fileName).bufferedReader().use { it.readText() }

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
