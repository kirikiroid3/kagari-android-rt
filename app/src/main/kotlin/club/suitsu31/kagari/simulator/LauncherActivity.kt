package club.suitsu31.kagari.simulator

import android.content.Intent
import android.content.pm.PackageManager
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge

/** Hosts [LauncherScreen]; the state and the work live in [GameRepository]. */
class LauncherActivity : ComponentActivity() {
    private lateinit var repository: GameRepository

    override fun onCreate(savedInstanceState: Bundle?) {
        enableEdgeToEdge()
        super.onCreate(savedInstanceState)
        repository = GameRepository.get(this)
        // A recreated activity gets its first intent again: that ZIP is already imported.
        if (savedInstanceState == null) {
            handleView(intent)
        }
        setContent {
            KagariTheme {
                LauncherScreen(repository)
            }
        }
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        handleView(intent)
    }

    /**
     * A ZIP opened with this app from a file manager or a browser. Not again from Recents, nor
     * when Android restores the task with its first intent after the process died (or after an
     * update): the read grant died with the process.
     */
    private fun handleView(intent: Intent) {
        if (intent.action != Intent.ACTION_VIEW) return
        if (intent.flags and Intent.FLAG_ACTIVITY_LAUNCHED_FROM_HISTORY != 0) return
        val uri = intent.data ?: return
        if (checkCallingOrSelfUriPermission(uri, Intent.FLAG_GRANT_READ_URI_PERMISSION) != PackageManager.PERMISSION_GRANTED) return
        repository.importZip(uri)
    }
}
