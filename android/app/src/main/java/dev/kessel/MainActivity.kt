package dev.kessel

import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.BackHandler
import androidx.activity.compose.setContent
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.darkColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.platform.LocalContext
import dev.kessel.game.Game
import dev.kessel.game.GameCatalog
import dev.kessel.ui.LibraryScreen
import dev.kessel.ui.PlayScreen

/**
 * The whole app: pick a game, play it, come back.
 *
 * Navigation is one nullable field rather than a nav library — there are two
 * screens. When the editor and cloud sync arrive and there are five, that is the
 * moment to add one, not now.
 */
class MainActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        setContent {
            // Dark only: this is a console, and the games are drawn on black.
            MaterialTheme(colorScheme = darkColorScheme()) {
                Surface { KesselApp() }
            }
        }
    }
}

@Composable
private fun KesselApp() {
    val context = LocalContext.current
    val games = remember { GameCatalog.list(context.assets) }
    var playing by remember { mutableStateOf<Game?>(null) }

    val game = playing
    if (game == null) {
        LibraryScreen(games = games) { playing = it }
        return
    }

    // Read on the main thread: these are tens of kilobytes out of the APK, and
    // the alternative is a loading state for something that takes under a
    // millisecond.
    val source = remember(game) { GameCatalog.source(context.assets, game) }
    // The shared sources a game may `#include`. Read once and reused across
    // games: the whole `lib/` directory is smaller than one game's sprite sheet.
    val libraries = remember { GameCatalog.libraries(context.assets) }

    // System back leaves the game, which stops the loop and frees the console —
    // see the DisposableEffect in PlayScreen.
    BackHandler { playing = null }
    PlayScreen(
        game = game,
        source = source,
        libraries = libraries,
        onBack = { playing = null },
    )
}
