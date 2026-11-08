package club.suitsu31.kagari.simulator

import android.annotation.SuppressLint
import android.app.ActivityManager
import android.content.Context
import android.content.Intent
import android.net.Uri
import android.os.Process
import java.io.File
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext

/** The imported games, and the game in the `:game` process if one runs ([runningId]). */
data class LibraryState(val games: List<Game>, val runningId: String?)

sealed interface ImportState {
    data object Idle : ImportState

    /** Copying [name] (null until known); [fraction] in 0..1, null while unknown. */
    data class Running(val name: String?, val fraction: Float?) : ImportState

    /** Imported [game]; stays until [GameRepository.acknowledgeImport]. */
    data class Done(val game: Game) : ImportState

    /** Stays until [GameRepository.acknowledgeImport]. */
    data class Failed(val error: ImportException) : ImportState
}

/**
 * The launcher's model, one per process: it outlives the activity, so an import survives a
 * rotation or a closed launcher, and its outcome waits for the next one.
 */
class GameRepository private constructor(private val app: Context) {
    private val library = GameLibrary(app.filesDir, app.cacheDir)
    private val importer = GameImporter(app.contentResolver, library, app.cacheDir)
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)

    /** One import at a time; also keeps [GameLibrary.cleanup] off a running import's staging. */
    private val importLock = Mutex()
    private val reloadLock = Mutex()

    private val _library = MutableStateFlow<LibraryState?>(null)
    private val _import = MutableStateFlow<ImportState>(ImportState.Idle)

    /** Null until first listed. */
    val state: StateFlow<LibraryState?> = _library.asStateFlow()
    val import: StateFlow<ImportState> = _import.asStateFlow()

    init {
        scope.launch { importLock.withLock { library.cleanup() } }
    }

    fun refresh() {
        scope.launch { reload() }
    }

    fun importZip(uri: Uri) = startImport { importer.importZip(uri, it) }

    fun importFolder(treeUri: Uri) = startImport { importer.importFolder(treeUri, it) }

    /** Forgets a [ImportState.Done] or [ImportState.Failed] outcome once shown. */
    fun acknowledgeImport() {
        _import.update { if (it is ImportState.Running) it else ImportState.Idle }
    }

    /** Removes [game]'s files (its saves stay), ending it first if it is running. */
    suspend fun delete(game: Game) = withContext(NonCancellable + Dispatchers.IO) {
        if (runningDir() == game.dir.absolutePath) {
            gameProcess()?.let { stopGame(it.pid) }
        }
        library.delete(game)
        reload()
    }

    /**
     * The intent that starts [game] in the `:game` process. A different game running there is
     * ended first (its process holds one event loop); the same game is brought back as it was.
     */
    suspend fun launchIntent(game: Game): Intent = withContext(Dispatchers.IO) {
        var running = gameProcess()
        if (running != null && runningDir() == null) {
            // A game that finished is still autosaving: let it end by itself.
            running = if (awaitGameExit(FINISH_TIMEOUT_MS)) null else gameProcess()
        }
        val other = runningDir() != game.dir.absolutePath
        if (running != null && other) {
            stopGame(running.pid)
        }
        // The game task outlives its process, and Android restores it with the intent it had (or
        // a stripped one after an update). Only a live process running this game is resumed;
        // anything else starts a fresh task with this game's intent.
        var flags = Intent.FLAG_ACTIVITY_NEW_TASK
        if (running == null || other) {
            flags = flags or Intent.FLAG_ACTIVITY_CLEAR_TASK
        }
        Intent(app, GameActivity::class.java)
            .putExtra(GameActivity.EXTRA_GAME_DIR, game.dir.absolutePath)
            .addFlags(flags)
    }

    private fun startImport(work: (ImportProgress) -> String) {
        scope.launch {
            importLock.withLock {
                _import.value = ImportState.Running(null, null)
                val outcome = try {
                    val id = work { name, fraction -> _import.value = ImportState.Running(name, fraction) }
                    reload()
                    _library.value?.games?.firstOrNull { it.id == id }?.let(ImportState::Done)
                        ?: ImportState.Idle
                } catch (e: ImportException) {
                    ImportState.Failed(e)
                } catch (e: Exception) {
                    ImportState.Failed(ImportException(R.string.import_io, e.message ?: e.javaClass.simpleName))
                }
                _import.value = outcome
            }
        }
    }

    private suspend fun reload() = reloadLock.withLock {
        val games = library.list()
        val runningDir = if (gameProcess() != null) runningDir() else null
        _library.value = LibraryState(games, games.firstOrNull { it.dir.absolutePath == runningDir }?.id)
    }

    private fun gameProcess(): ActivityManager.RunningAppProcessInfo? {
        val name = "${app.packageName}:game"
        return app.getSystemService(ActivityManager::class.java)
            .runningAppProcesses?.firstOrNull { it.processName == name }
    }

    private fun runningDir(): String? =
        File(app.filesDir, GameActivity.RUNNING_FILE).takeIf { it.isFile }?.readText()

    /** Kills the `:game` process [pid] and waits up to a second for it to go. */
    private suspend fun stopGame(pid: Int) {
        Process.killProcess(pid)
        awaitGameExit(KILL_TIMEOUT_MS)
    }

    /** Whether the `:game` process is gone within [timeoutMs]. */
    private suspend fun awaitGameExit(timeoutMs: Long): Boolean {
        repeat((timeoutMs / POLL_MS).toInt()) {
            if (gameProcess() == null) return true
            delay(POLL_MS)
        }
        return gameProcess() == null
    }

    companion object {
        private const val POLL_MS = 50L
        private const val KILL_TIMEOUT_MS = 1000L
        private const val FINISH_TIMEOUT_MS = 5000L

        // Holds the application context only, which lives as long as the process anyway.
        @SuppressLint("StaticFieldLeak")
        @Volatile
        private var instance: GameRepository? = null

        fun get(context: Context): GameRepository =
            instance ?: synchronized(this) {
                instance ?: GameRepository(context.applicationContext).also { instance = it }
            }
    }
}
