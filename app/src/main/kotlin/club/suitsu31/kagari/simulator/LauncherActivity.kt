package club.suitsu31.kagari.simulator

import android.app.Activity
import android.app.ActivityManager
import android.app.AlertDialog
import android.content.Intent
import android.net.Uri
import android.os.Bundle
import android.os.Process
import android.view.Gravity
import android.view.Menu
import android.view.MenuItem
import android.view.View
import android.view.WindowInsets
import android.widget.ArrayAdapter
import android.widget.LinearLayout
import android.widget.ListView
import android.widget.ProgressBar
import android.widget.TextView
import java.io.File
import java.util.concurrent.ExecutorService
import java.util.concurrent.Executors

/** The game list: import games from a ZIP file or a folder, play, delete. */
class LauncherActivity : Activity() {
    private lateinit var library: GameLibrary
    private lateinit var importer: GameImporter
    private lateinit var list: ListView
    private val io: ExecutorService = Executors.newSingleThreadExecutor()
    private var games: List<Game> = emptyList()

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        setContentView(R.layout.activity_launcher)
        library = GameLibrary(filesDir, cacheDir)
        importer = GameImporter(contentResolver, library, cacheDir)

        val root = findViewById<View>(R.id.root)
        list = findViewById(R.id.games)
        list.emptyView = findViewById(R.id.empty)
        // Edge to edge (enforced from targetSdk 35): keep the list clear of the system bars.
        root.setOnApplyWindowInsetsListener { _, insets ->
            val bars = insets.getInsets(WindowInsets.Type.systemBars())
            list.setPadding(bars.left, bars.top, bars.right, bars.bottom)
            insets
        }
        list.setOnItemClickListener { _, _, position, _ -> games.getOrNull(position)?.let(::launch) }
        list.setOnItemLongClickListener { _, _, position, _ ->
            games.getOrNull(position)?.let(::confirmDelete)
            true
        }

        io.execute { library.cleanup() }
        refresh()
        handleView(intent)
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        handleView(intent)
    }

    override fun onDestroy() {
        super.onDestroy()
        io.shutdown()
    }

    override fun onCreateOptionsMenu(menu: Menu): Boolean {
        menuInflater.inflate(R.menu.launcher, menu)
        return true
    }

    override fun onOptionsItemSelected(item: MenuItem): Boolean {
        when (item.itemId) {
            R.id.import_zip -> startActivityForResult(
                Intent(Intent.ACTION_OPEN_DOCUMENT)
                    .addCategory(Intent.CATEGORY_OPENABLE)
                    .setType("*/*")
                    .putExtra(
                        Intent.EXTRA_MIME_TYPES,
                        arrayOf("application/zip", "application/x-zip-compressed", "application/octet-stream"),
                    ),
                REQUEST_ZIP,
            )
            R.id.import_folder -> startActivityForResult(Intent(Intent.ACTION_OPEN_DOCUMENT_TREE), REQUEST_FOLDER)
            else -> return super.onOptionsItemSelected(item)
        }
        return true
    }

    @Deprecated("Activity results without AndroidX")
    @Suppress("DEPRECATION")
    override fun onActivityResult(requestCode: Int, resultCode: Int, data: Intent?) {
        super.onActivityResult(requestCode, resultCode, data)
        val uri = data?.data
        if (resultCode != RESULT_OK || uri == null) return
        when (requestCode) {
            REQUEST_ZIP -> runImport { importer.importZip(uri) }
            REQUEST_FOLDER -> runImport { importer.importFolder(uri) }
        }
    }

    /** A ZIP opened with this app from a file manager or a browser. */
    private fun handleView(intent: Intent?) {
        val uri: Uri = intent?.data ?: return
        if (intent.action != Intent.ACTION_VIEW) return
        setIntent(Intent(this, LauncherActivity::class.java))
        runImport { importer.importZip(uri) }
    }

    /** Runs [work] off the UI thread behind a progress dialog, then refreshes or explains. */
    private fun runImport(work: () -> Unit) {
        val progress = AlertDialog.Builder(this)
            .setView(progressView())
            .setCancelable(false)
            .show()
        io.execute {
            val failure = try {
                work()
                null
            } catch (e: ImportException) {
                e
            } catch (e: Exception) {
                ImportException(R.string.import_io, e.message ?: e.javaClass.simpleName)
            }
            runOnUiThread {
                progress.dismiss()
                refresh()
                if (failure != null && !isFinishing) {
                    AlertDialog.Builder(this)
                        .setTitle(R.string.import_failed_title)
                        .setMessage(getString(failure.res, *failure.args))
                        .setPositiveButton(android.R.string.ok, null)
                        .show()
                }
            }
        }
    }

    private fun progressView(): View {
        val pad = (24 * resources.displayMetrics.density).toInt()
        return LinearLayout(this).apply {
            orientation = LinearLayout.HORIZONTAL
            gravity = Gravity.CENTER_VERTICAL
            setPadding(pad, pad, pad, pad)
            addView(ProgressBar(context).apply { isIndeterminate = true })
            addView(TextView(context).apply {
                setText(R.string.importing)
                setPadding(pad, 0, 0, 0)
            })
        }
    }

    private fun refresh() {
        io.execute {
            val found = library.list()
            runOnUiThread {
                games = found
                list.adapter = ArrayAdapter(this, android.R.layout.simple_list_item_1, found.map { it.name })
            }
        }
    }

    private fun confirmDelete(game: Game) {
        AlertDialog.Builder(this)
            .setTitle(getString(R.string.delete_title, game.name))
            .setMessage(R.string.delete_message)
            .setPositiveButton(R.string.delete) { _, _ ->
                io.execute { library.delete(game) }
                refresh()
            }
            .setNegativeButton(android.R.string.cancel, null)
            .show()
    }

    /**
     * Starts [game] in the `:game` process. A different game running there is ended first (its
     * process holds one event loop); the same game is brought back as it was.
     */
    private fun launch(game: Game) {
        io.execute {
            val manager = getSystemService(ActivityManager::class.java)
            val gameProcess = "$packageName:game"
            val running = manager.runningAppProcesses?.firstOrNull { it.processName == gameProcess }
            val runningDir = File(filesDir, GameActivity.RUNNING_FILE)
                .takeIf { it.isFile }?.readText()
            val other = runningDir != game.dir.absolutePath
            if (running != null && other) {
                Process.killProcess(running.pid)
                val deadline = System.currentTimeMillis() + 1000
                while (System.currentTimeMillis() < deadline &&
                    manager.runningAppProcesses?.any { it.processName == gameProcess } == true
                ) {
                    Thread.sleep(50)
                }
            }
            // The game task outlives its process, and Android restores it with the intent it
            // had (or a stripped one after an update). Only a live process running this game
            // is resumed; anything else starts a fresh task with this game's intent.
            var flags = Intent.FLAG_ACTIVITY_NEW_TASK
            if (running == null || other) {
                flags = flags or Intent.FLAG_ACTIVITY_CLEAR_TASK
            }
            runOnUiThread {
                startActivity(
                    Intent(this, GameActivity::class.java)
                        .putExtra(GameActivity.EXTRA_GAME_DIR, game.dir.absolutePath)
                        .addFlags(flags),
                )
            }
        }
    }

    companion object {
        private const val REQUEST_ZIP = 1
        private const val REQUEST_FOLDER = 2
    }
}
