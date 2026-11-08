package club.suitsu31.kagari.simulator

import android.content.ContentResolver
import android.net.Uri
import android.provider.DocumentsContract
import android.provider.OpenableColumns
import java.io.File
import java.io.IOException
import java.io.InputStream
import java.util.UUID
import java.util.zip.ZipFile

/** An import failure, shown as the string [res] formatted with [args]. */
class ImportException(val res: Int, vararg val args: Any) : Exception()

/**
 * Copies a downloaded game (`game.kagari` and its `*.kiarchive` files) into the library, from a
 * ZIP file or a folder. Runs off the UI thread.
 */
class GameImporter(
    private val resolver: ContentResolver,
    private val library: GameLibrary,
    private val cacheDir: File,
) {
    /** Imports the game in the ZIP at [uri]: the `game.kagari` nearest the root, with the archives beside it. */
    fun importZip(uri: Uri) = staged { files ->
        val name = displayName(uri)?.replace(ZIP_SUFFIX, "")?.ifEmpty { null } ?: "Game"
        val temp = File(cacheDir, "import-${UUID.randomUUID()}.zip")
        try {
            open(uri).use { input -> temp.outputStream().use { input.copyTo(it) } }
            ZipFile(temp).use { zip ->
                val entries = zip.entries().toList()
                    .filter { !it.isDirectory }
                    .associateBy { it.name.replace('\\', '/') }
                val program = entries.keys
                    .filter { it == GameLibrary.PROGRAM || it.endsWith("/" + GameLibrary.PROGRAM) }
                    .minWithOrNull(compareBy<String> { path -> path.count { it == '/' } }.thenBy { it })
                    ?: throw ImportException(R.string.import_no_game)
                val prefix = program.removeSuffix(GameLibrary.PROGRAM)
                for ((path, entry) in entries) {
                    val rest = path.removePrefix(prefix)
                    if (!path.startsWith(prefix) || rest.contains('/') || !wanted(rest)) continue
                    // Written by its bare file name: nothing escapes `files`.
                    zip.getInputStream(entry).use { input -> copy(input, File(files, rest)) }
                }
            }
        } finally {
            temp.delete()
        }
        name
    }

    /** Imports the game directly in the folder [treeUri] (no subfolders). */
    fun importFolder(treeUri: Uri) = staged { files ->
        val docId = DocumentsContract.getTreeDocumentId(treeUri)
        val name = displayName(DocumentsContract.buildDocumentUriUsingTree(treeUri, docId)) ?: "Game"
        val children = DocumentsContract.buildChildDocumentsUriUsingTree(treeUri, docId)
        val columns = arrayOf(
            DocumentsContract.Document.COLUMN_DOCUMENT_ID,
            DocumentsContract.Document.COLUMN_DISPLAY_NAME,
            DocumentsContract.Document.COLUMN_MIME_TYPE,
        )
        var found = false
        resolver.query(children, columns, null, null, null)?.use { cursor ->
            while (cursor.moveToNext()) {
                val childId = cursor.getString(0) ?: continue
                val childName = cursor.getString(1) ?: continue
                if (cursor.getString(2) == DocumentsContract.Document.MIME_TYPE_DIR) continue
                if (childName.contains('/') || !wanted(childName)) continue
                open(DocumentsContract.buildDocumentUriUsingTree(treeUri, childId)).use { input ->
                    copy(input, File(files, childName))
                }
                found = found || childName == GameLibrary.PROGRAM
            }
        }
        if (!found) throw ImportException(R.string.import_no_game)
        name
    }

    /** Runs [fill] on a new staging directory's `files/` and commits it under the returned name. */
    private fun staged(fill: (File) -> String) {
        val staging = library.newStaging()
        try {
            val name = fill(File(staging, "files"))
            library.commit(staging, name)
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

    private fun copy(input: InputStream, to: File) {
        to.outputStream().use { input.copyTo(it) }
    }

    private fun displayName(uri: Uri): String? =
        resolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)?.use {
            if (it.moveToFirst()) it.getString(0) else null
        }

    companion object {
        private val ZIP_SUFFIX = Regex("\\.zip$", RegexOption.IGNORE_CASE)
    }
}
