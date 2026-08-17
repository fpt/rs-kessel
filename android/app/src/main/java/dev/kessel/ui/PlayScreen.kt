package dev.kessel.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.TopAppBarDefaults
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.runtime.collectAsState
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.unit.dp
import dev.kessel.game.Game
import dev.kessel.game.GameEngine
import dev.kessel.vm.KesselVm

/**
 * One game, running.
 *
 * Portrait by design: the console's screen is square, which leaves the bottom
 * third of a phone free for a pad that no thumb has to reach across the picture
 * to use.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun PlayScreen(
    game: Game,
    source: String,
    libraries: Map<String, String>,
    onBack: () -> Unit,
) {
    // The console and its loop are owned by this screen. Leaving it joins the
    // thread *and* frees the native handle — the console is not on the Java
    // heap, so without the close() a player who browses the library leaks a
    // whole VmConsole per game they open.
    val engine = remember(game) { GameEngine(KesselVm()) }
    DisposableEffect(game) {
        engine.start(source, game.fileName, libraries)
        onDispose { engine.close() }
    }

    val state by engine.state.collectAsState()

    Scaffold(
        topBar = {
            TopAppBar(
                title = { Text(game.title) },
                navigationIcon = {
                    IconButton(onClick = onBack) {
                        Icon(Icons.AutoMirrored.Filled.ArrowBack, contentDescription = "Back")
                    }
                },
                actions = {
                    // The pause control lives here rather than on the pad,
                    // because the ROM decides *which* physical button pauses and
                    // that button often has a game meaning too.
                    if (state.error == null && state.controls.pauseBit != 0) {
                        TextButton(onClick = { engine.pulse(state.controls.pauseBit) }) {
                            Text(if (state.paused) "Resume" else "Pause")
                        }
                    }
                },
                colors = TopAppBarDefaults.topAppBarColors(
                    containerColor = MaterialTheme.colorScheme.surface,
                ),
            )
        },
    ) { padding ->
        Column(
            Modifier
                .fillMaxSize()
                .padding(padding),
            horizontalAlignment = Alignment.CenterHorizontally,
        ) {
            val error = state.error
            if (error != null) {
                Diagnostics(error, Modifier.weight(1f))
                return@Column
            }

            Box(
                Modifier
                    .fillMaxWidth()
                    .aspectRatio(1f)
                    .background(Color.Black),
                contentAlignment = Alignment.Center,
            ) {
                GameSurface(
                    engine = engine,
                    contentDescription = "${game.title} screen",
                    modifier = Modifier.fillMaxSize(),
                    // Only a ROM that says it reads touches gets a pointer
                    // handler over its screen. Otherwise every game would eat
                    // taps meant for whatever the system draws on top.
                    touchable = state.controls.touch != null,
                    screenDim = state.screenDim,
                )
                if (state.halted) {
                    Banner("Game over — tap back to return")
                } else if (state.paused) {
                    Banner("Paused")
                }
            }

            Gamepad(
                controls = state.controls,
                modifier = Modifier
                    .fillMaxWidth()
                    .weight(1f),
                onButtons = engine::setButtons,
                onStick = engine::setStick,
            )
        }
    }
}

@Composable
private fun Banner(text: String) {
    Box(
        Modifier
            .fillMaxWidth()
            .background(Color(0xCC000000))
            .padding(vertical = 12.dp),
        contentAlignment = Alignment.Center,
    ) {
        Text(text, color = Color.White, style = MaterialTheme.typography.titleMedium)
    }
}

/**
 * Compiler output, shown verbatim and monospaced.
 *
 * The same reasoning as `kessel run` refusing to open a blank window: a game
 * that did not compile should say why. Once the in-app editor lands this is the
 * screen the author reads.
 */
@Composable
private fun Diagnostics(error: String, modifier: Modifier = Modifier) {
    Column(
        modifier
            .fillMaxWidth()
            .verticalScroll(rememberScrollState())
            .padding(24.dp),
        verticalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        Text("This game didn't compile", style = MaterialTheme.typography.titleMedium)
        Text(
            error,
            style = MaterialTheme.typography.bodySmall.copy(fontFamily = FontFamily.Monospace),
            color = MaterialTheme.colorScheme.error,
        )
    }
}
