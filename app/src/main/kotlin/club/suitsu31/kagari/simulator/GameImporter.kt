package club.suitsu31.kagari.simulator

import android.content.ContentResolver
import android.net.Uri
import android.provider.DocumentsContract
import android.provider.OpenableColumns
import java.io.File
import java.io.IOException
import java.io.InputStream
import java.io.OutputStream
import java.util.UUID
import java.util.zip.ZipFile

/** An import failure, shown as the string [res] formatted with [args]. */
class ImportException(val res: Int, vararg val args: Any) : Exception()

/** Hears how an import goes: the game's [name], and [fraction] done in 0..1 (null while unknown). */
fun interface ImportProgress {
    fun report(name: String, fraction: Float?)
}

/**
 * Copies a downloaded game (`game.kagari` and its `*.kiarchive` files) into the library, from a
 * ZIP file or a folder. Runs off the UI thread.
 */
class GameImporter(
    private val resolver: ContentResolver,
    private val library: GameLibrary,
    private val cacheDir: File,
) {
    /**
     * Imports the game in the ZIP at [uri]: the `game.kagari` nearest the root, with the archives
     * beside it. Returns the new game's id. The copy is the first half of [progress], the
     * extraction the second.
     */
    fun importZip(uri: Uri, progress: ImportProgress): String = staged { files ->
        val name = displayName(uri)?.replace(ZIP_SUFFIX, "")?.ifEmpty { null } ?: "Game"
        progress.report(name, null)
        val temp = File(cacheDir, "import-${UUID.randomUUID()}.zip")
        try {
            val copied = size(uri)?.let { total -> Meter(total) { progress.report(name, it / 2) } }
            open(uri).use { input -> temp.outputStream().use { copy(input, it, copied) } }
            ZipFile(temp).use { zip ->
                val entries = zip.entries().toList()
                    .filter { !it.isDirectory }
                    .associateBy { it.name.replace('\\', '/') }
                val program = entries.keys
                    .filter { it == GameLibrary.PROGRAM || it.endsWith("/" + GameLibrary.PROGRAM) }
                    .minWithOrNull(compareBy<String> { path -> path.count { it == '/' } }.thenBy { it })
                    ?: throw ImportException(R.string.import_no_game)
                val prefix = program.removeSuffix(GameLibrary.PROGRAM)
                val chosen = entries.mapNotNull { (path, entry) ->
                    val rest = path.removePrefix(prefix)
                    if (!path.startsWith(prefix) || rest.contains('/') || !wanted(rest)) null else rest to entry
                }
                val extracted = Meter(chosen.sumOf { (_, entry) -> entry.size.coerceAtLeast(0) }) {
                    progress.report(name, 0.5f + it / 2)
                }
                for ((rest, entry) in chosen) {
                    // Written by its bare file name: nothing escapes `files`.
                    zip.getInputStream(entry).use { input ->
                        File(files, rest).outputStream().use { copy(input, it, extracted) }
                    }
                }
            }
        } finally {
            temp.delete()
        }
        name
    }

    /** Imports the game directly in the folder [treeUri] (no subfolders). Returns the new game's id. */
    fun importFolder(treeUri: Uri, progress: ImportProgress): String = staged { files ->
        val docId = DocumentsContract.getTreeDocumentId(treeUri)
        val name = displayName(DocumentsContract.buildDocumentUriUsingTree(treeUri, docId)) ?: "Game"
        progress.report(name, null)
        val children = DocumentsContract.buildChildDocumentsUriUsingTree(treeUri, docId)
        val columns = arrayOf(
            DocumentsContract.Document.COLUMN_DOCUMENT_ID,
            DocumentsContract.Document.COLUMN_DISPLAY_NAME,
            DocumentsContract.Document.COLUMN_MIME_TYPE,
            DocumentsContract.Document.COLUMN_SIZE,
        )
        val chosen = mutableListOf<Pair<String, String>>()
        var total = 0L
        resolver.query(children, columns, null, null, null)?.use { cursor ->
            while (cursor.moveToNext()) {
                val childId = cursor.getString(0) ?: continue
                val childName = cursor.getString(1) ?: continue
                if (cursor.getString(2) == DocumentsContract.Document.MIME_TYPE_DIR) continue
                if (childName.contains('/') || !wanted(childName)) continue
                chosen += childId to childName
                if (!cursor.isNull(3)) total += cursor.getLong(3)
            }
        }
        if (chosen.none { (_, childName) -> childName == GameLibrary.PROGRAM }) {
            throw ImportException(R.string.import_no_game)
        }
        val copied = total.takeIf { it > 0 }?.let { Meter(it) { f -> progress.report(name, f) } }
        for ((childId, childName) in chosen) {
            open(DocumentsContract.buildDocumentUriUsingTree(treeUri, childId)).use { input ->
                File(files, childName).outputStream().use { copy(input, it, copied) }
            }
        }
        name
    }

    /** Runs [fill] on a new staging directory's `files/`, commits it under the returned name and returns its id. */
    private fun staged(fill: (File) -> String): String {
        val staging = library.newStaging()
        try {
            val name = fill(File(staging, "files"))
            return library.commit(staging, name)
        } catch (e: IOException) {
            staging.deleteRecursively()
            throw ImportException(R.string.import_io, e.message ?: "")
        } catch (e: Exception) {
            staging.deleteRecursively()
            throw e
        }
    }

    private fun wanted(name: String) =
        name == GameLibrary.PROGRAM || name.endsWith(GameLibrary.ARCHIVE_SUFFIX)

    private fun open(uri: Uri): InputStream =
        resolver.openInputStream(uri) ?: throw IOException("cannot open $uri")

    private fun copy(input: InputStream, output: OutputStream, meter: Meter?) {
        val buffer = ByteArray(BUFFER_SIZE)
        while (true) {
            val n = input.read(buffer)
            if (n < 0) break
            output.write(buffer, 0, n)
            meter?.add(n)
        }
    }

    private fun displayName(uri: Uri): String? =
        resolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)?.use {
            if (it.moveToFirst()) it.getString(0) else null
        }

    private fun size(uri: Uri): Long? =
        resolver.query(uri, arrayOf(OpenableColumns.SIZE), null, null, null)?.use {
            if (it.moveToFirst() && !it.isNull(0)) it.getLong(0).takeIf { size -> size > 0 } else null
        }

    /** Turns bytes done out of [total] into a fraction, told to [report] once per whole percent. */
    private class Meter(private val total: Long, private val report: (Float) -> Unit) {
        private var done = 0L
        private var shown = -1

        fun add(bytes: Int) {
            done += bytes
            val percent = if (total > 0) (done * 100 / total).toInt().coerceIn(0, 100) else 0
            if (percent != shown) {
                shown = percent
                report(percent / 100f)
            }
        }
    }

    companion object {
        private val ZIP_SUFFIX = Regex("\\.zip$", RegexOption.IGNORE_CASE)
        private const val BUFFER_SIZE = 64 * 1024
    }
}
