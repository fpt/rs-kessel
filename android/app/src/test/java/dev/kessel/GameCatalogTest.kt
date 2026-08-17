package dev.kessel

import dev.kessel.game.GameCatalog
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * What counts as a game in the asset root.
 *
 * Worth a test of its own because the failure is silent in both directions: a
 * rule that is too loose puts `lib` — the shared-source directory games
 * `#include` from — on the library screen as a playable title, and one that is
 * too tight drops a real game out of the app with no error anywhere.
 *
 * `GameCatalog.list` itself needs an `AssetManager`, which is a throwing stub on
 * the unit-test classpath; the rule it filters by is plain strings and tests
 * here.
 */
class GameCatalogTest {

    @Test
    fun `game sources are recognised by extension`() {
        assertTrue(GameCatalog.isSource("tetris.lua"))
        assertTrue(GameCatalog.isSource("swarm.lua"))
        assertTrue(GameCatalog.isSource("demo.ux"))
        assertTrue(GameCatalog.isSource("raw.asm"))
        assertTrue("the corpus is lowercase, but the rule should not care",
            GameCatalog.isSource("TETRIS.LUA"))
    }

    @Test
    fun `the shared-source directory is not a game`() {
        assertFalse("lib/ holds #include targets, not playable titles",
            GameCatalog.isSource(GameCatalog.LIB_DIR))
        assertFalse(GameCatalog.isSource("README"))
        assertFalse(GameCatalog.isSource("notes.txt"))
        assertFalse(GameCatalog.isSource("luaish"))
    }
}
