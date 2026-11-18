package eu.shaffe.tomi.core

import android.content.ContentProvider
import android.content.ContentValues
import android.content.Context
import android.database.Cursor
import android.database.MatrixCursor
import android.net.Uri
import android.os.ParcelFileDescriptor
import android.provider.OpenableColumns
import android.webkit.MimeTypeMap
import java.io.File
import java.io.FileNotFoundException

/** Where the Rust core keeps its files (`CoreConfig.filesDir`), under the no-backup directory. */
object CoreFiles {
    fun coreDir(context: Context): File = File(context.noBackupFilesDir, "tomi-core")

    fun filesDir(context: Context): File = File(coreDir(context), "files")

    /**
     * The decrypted-attachment cache of the core: `CACHE_DIR` of `crates/core/src/runtime/store.rs` ("files/cache"), relative to the
     * files root it is given ([filesDir]), hence `tomi-core/files/files/cache`.
     */
    fun plaintextCacheDir(context: Context): File = File(filesDir(context), "files/cache")
}

/**
 * Maps the files of the core's decrypted cache (`files/cache/{item}_{index}`, see `RustTomiCore.attachmentPath`) to the `content://`
 * URIs of [AttachmentProvider], and back. Pure: no Android call besides [Uri], so the rules are tested on the JVM.
 *
 * A URI is `content://{authority}/{cache file name}/{display name}`. The first segment names the file by its bare name only (no
 * separator, no `..`): it can never reach outside the cache directory. The second is only what the receiving app shows and saves
 * under: the real name of the attachment, cleaned by [cleanName], for an item that is not hidden; `fichier.{extension}` otherwise
 * (a hidden item never reveals its file names). The extension is read back from it by [extensionOf].
 */
object AttachmentUris {
    const val AUTHORITY_SUFFIX = ".attachments"
    private const val DISPLAY_BASE = "fichier"
    private const val MAX_NAME = 120
    private val FORBIDDEN = Regex("[\\\\/:*?\"<>|\\u0000-\\u001f]")

    fun authority(packageName: String): String = packageName + AUTHORITY_SUFFIX

    /** A cache file name of the core: `{item id}_{index}`, nothing that could be a path. */
    private val FILE_NAME = Regex("^[A-Za-z0-9-]{1,128}_[0-9]{1,6}$")
    private val EXTENSION = Regex("^[a-z0-9]{1,10}$")

    fun isCacheFileName(name: String): Boolean = FILE_NAME.matches(name)

    /** [name] as a plain file name (no separator or reserved character, no leading / trailing dot, bounded), or null if nothing is left. */
    fun cleanName(name: String?): String? =
        FORBIDDEN.replace(name.orEmpty(), "_").trim().trim('.').take(MAX_NAME).trim().ifEmpty { null }

    /**
     * What the receiving app shows: [name] cleaned when given (an item that is not hidden), else `fichier.{extension}`, or `fichier`
     * when the extension is unknown or not a plain one.
     */
    fun displayName(extension: String?, name: String? = null): String {
        cleanName(name)?.let { return it }
        val ext = extension?.lowercase(java.util.Locale.ROOT)
        return if (ext != null && EXTENSION.matches(ext)) "$DISPLAY_BASE.$ext" else DISPLAY_BASE
    }

    /** Percent-encoding of one path segment (a space is `%20`, never `+`). */
    private fun encodeSegment(s: String): String = java.net.URLEncoder.encode(s, "UTF-8").replace("+", "%20")

    /**
     * The URI of [path] (absolute, as the core returns it), or null if it is not a file directly inside [cacheDir]. Both are compared
     * canonically, so a symbolic link or a `..` cannot make a file elsewhere look like a cache file. [name] is the attachment's name,
     * to pass only for an item that is not hidden.
     */
    fun uriFor(path: String, cacheDir: File, authority: String, extension: String? = null, name: String? = null): String? {
        val file = File(path).canonicalFile
        if (file.parentFile != cacheDir.canonicalFile || !isCacheFileName(file.name)) return null
        return "content://$authority/${file.name}/${encodeSegment(displayName(extension, name))}"
    }

    /**
     * The cache file behind [segments] (the path segments of a URI), or null if they do not name one. The second segment is only
     * cosmetic: it must be a clean file name, and decides nothing about which file is served.
     */
    fun fileFor(segments: List<String>, cacheDir: File): File? {
        if (segments.size != 2 || cleanName(segments[1]) != segments[1]) return null
        val name = segments[0]
        if (!isCacheFileName(name)) return null
        return File(cacheDir, name)
    }

    /** The extension the URI carries (validated), or null. */
    fun extensionOf(segments: List<String>): String? =
        segments.getOrNull(1)?.substringAfterLast('.', "")?.lowercase(java.util.Locale.ROOT)?.takeIf { EXTENSION.matches(it) }
}

/**
 * Read-only access to the decrypted attachments, for the app itself (image viewer, code screen) and for the app the user picked
 * to open or share one (a temporary read grant, `FLAG_GRANT_READ_URI_PERMISSION`). Not exported: no other app can reach it without
 * such a grant.
 *
 * It only serves what is already in the core's cache: decrypting (after the biometric gate for a hidden item) is the core's job,
 * through `TomiCore.attachmentUri`. Nothing is copied: the file descriptor is the cache file itself. [query] returns the name the
 * URI carries: the attachment's own for an item that is not hidden, `fichier.{extension}` for a hidden one.
 */
class AttachmentProvider : ContentProvider() {
    private val cacheDir: File by lazy { CoreFiles.plaintextCacheDir(requireNotNull(context)) }

    override fun onCreate(): Boolean = true

    override fun openFile(uri: Uri, mode: String): ParcelFileDescriptor {
        if (mode != "r") throw SecurityException("read only")
        val file = AttachmentUris.fileFor(uri.pathSegments, cacheDir) ?: throw FileNotFoundException()
        if (!file.isFile) throw FileNotFoundException()
        return ParcelFileDescriptor.open(file, ParcelFileDescriptor.MODE_READ_ONLY)
    }

    override fun query(uri: Uri, projection: Array<out String>?, selection: String?, selectionArgs: Array<out String>?, sortOrder: String?): Cursor? {
        val file = AttachmentUris.fileFor(uri.pathSegments, cacheDir) ?: return null
        if (!file.isFile) return null
        val columns = projection?.filter { it == OpenableColumns.DISPLAY_NAME || it == OpenableColumns.SIZE }?.toTypedArray()
            ?: arrayOf(OpenableColumns.DISPLAY_NAME, OpenableColumns.SIZE)
        return MatrixCursor(columns, 1).apply {
            addRow(columns.map { if (it == OpenableColumns.SIZE) file.length() else uri.lastPathSegment })
        }
    }

    /** From the extension the URI carries (the intents that hand a URI out also carry the type of the attachment). */
    override fun getType(uri: Uri): String? =
        AttachmentUris.extensionOf(uri.pathSegments)?.let { MimeTypeMap.getSingleton().getMimeTypeFromExtension(it) }

    override fun insert(uri: Uri, values: ContentValues?): Uri? = throw UnsupportedOperationException()

    override fun delete(uri: Uri, selection: String?, selectionArgs: Array<out String>?): Int = throw UnsupportedOperationException()

    override fun update(uri: Uri, values: ContentValues?, selection: String?, selectionArgs: Array<out String>?): Int =
        throw UnsupportedOperationException()
}
