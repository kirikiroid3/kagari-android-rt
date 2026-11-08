package club.suitsu31.kagari.simulator

import java.io.File
import java.util.UUID

/** An imported game: [dir] holds `game.kagari` and its archives. */
data class Game(val id: String, val name: String, val dir: File)

/**
 * The imported games in `filesDir/games/<id>/`: `name.txt` (the display name) and `files/`
 * (`game.kagari` and every `*.kiarchive`). Saves live apart, in `filesDir/saves/`, and outlive
 * their game.
 */
class GameLibrary(private val filesDir: File, private val cacheDir: File) {
    val gamesDir = File(filesDir, "games")

    fun list(): List<Game> =
        (gamesDir.listFiles() ?: emptyArray())
            .filter { it.isDirectory && !it.name.startsWith(".") }
            .filter { File(it, "files/$PROGRAM").isFile }
            .map { dir ->
                val name = File(dir, NAME_FILE).takeIf { it.isFile }?.readText()?.trim()
                Game(dir.name, if (name.isNullOrEmpty()) dir.name else name, File(dir, "files"))
            }
            .sortedWith(compareBy(String.CASE_INSENSITIVE_ORDER) { it.name })

    /** Removes the game's files; its saves stay. */
    fun delete(game: Game) {
        File(gamesDir, game.id).deleteRecursively()
    }

    /** Removes what an interrupted import left behind. */
    fun cleanup() {
        gamesDir.listFiles()
            ?.filter { it.name.startsWith(STAGING_PREFIX) }
            ?.forEach { it.deleteRecursively() }
        cacheDir.listFiles()
            ?.filter { it.name.startsWith("import-") && it.name.endsWith(".zip") }
            ?.forEach { it.delete() }
    }

    /** A new, empty staging directory; the import fills its `files/`. */
    fun newStaging(): File {
        val staging = File(gamesDir, STAGING_PREFIX + UUID.randomUUID())
        File(staging, "files").mkdirs()
        return staging
    }

    /** Checks the staged `game.kagari` and moves the staging directory into the library. */
    fun commit(staging: File, name: String) {
        checkHeader(File(staging, "files/$PROGRAM"))
        File(staging, NAME_FILE).writeText(name)
        val target = File(gamesDir, UUID.randomUUID().toString())
        if (!staging.renameTo(target)) {
            throw ImportException(R.string.import_io, "cannot move ${staging.name}")
        }
    }

    /** A Kagari bytecode header this simulator runs: the magic and version `V000`. */
    private fun checkHeader(program: File) {
        val header = ByteArray(HEADER_SIZE)
        val read = program.inputStream().use { input ->
            var total = 0
            while (total < HEADER_SIZE) {
                val n = input.read(header, total, HEADER_SIZE - total)
                if (n < 0) break
                total += n
            }
            total
        }
        if (read < HEADER_SIZE || !header.copyOfRange(0, MAGIC.size).contentEquals(MAGIC)) {
            throw ImportException(R.string.import_not_kagari)
        }
        val version = header.copyOfRange(MAGIC.size, HEADER_SIZE)
        if (!version.contentEquals(VERSION)) {
            val shown = version.map { b ->
                if (b in 0x20..0x7e) b.toInt().toChar() else '?'
            }.joinToString("")
            throw ImportException(R.string.import_wrong_version, shown)
        }
    }

    companion object {
        const val PROGRAM = "game.kagari"
        const val ARCHIVE_SUFFIX = ".kiarchive"
        private const val NAME_FILE = "name.txt"
        private const val STAGING_PREFIX = ".staging-"
        private const val HEADER_SIZE = 16
        private val MAGIC = byteArrayOf(0, 0, 0, 0, 0x4B, 0x41, 0x47, 0x41, 0x52, 0x49, 0, 0)
        private val VERSION = "V000".toByteArray(Charsets.US_ASCII)
    }
}
