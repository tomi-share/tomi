package eu.shaffe.tomi

import android.app.Application
import android.os.Build
import android.provider.Settings
import android.webkit.MimeTypeMap
import eu.shaffe.tomi.core.AttachmentUris
import eu.shaffe.tomi.core.CoreFiles
import eu.shaffe.tomi.core.RealTomiCore
import eu.shaffe.tomi.core.TomiCore
import eu.shaffe.tomi.security.AndroidKeystoreWrapper
import eu.shaffe.tomi.security.WrapKeyStore
import eu.shaffe.tomi.send.DraftThumbnails
import kotlinx.coroutines.CoroutineScope
import uniffi.tomi_core.CoreConfig
import uniffi.tomi_core.RustTomiCore
import uniffi.tomi_core.isValidDeviceName
import java.io.File
import java.io.IOException
import java.security.KeyStore

/** Builds the Rust core of [AppContainer]. */
internal object CoreFactory {
    /**
     * Opens the Rust core (database, relay client; no network call).
     *
     * Everything the core stores lives under the no-backup directory (nothing of it is saved or transferred with the phone). The secrets of
     * its database are sealed under a key that only the Android Keystore can open ([WrapKeyStore]); that key is handed to the core for
     * the call that opens it and wiped from this side as soon as it returns.
     *
     * `autoImport = false`: the history the user said yes to at pairing is imported by the core without asking again (spec/pairing-v1.md
     * 9.5, test P27); a transfer nobody announced at pairing waits for an explicit acceptance, and the app has no screen for that yet, so
     * nothing arrives unasked after a "no, start from an empty feed".
     */
    fun real(app: Application, scope: CoroutineScope): TomiCore {
        val dir = coreDir(app).apply { mkdirs() }
        val filesDir = CoreFiles.filesDir(app).apply { mkdirs() }
        val wrapKey = WrapKeyStore(File(dir, "wrap-key.bin"), AndroidKeystoreWrapper()).loadOrCreate()
        val native = try {
            RustTomiCore.open(
                CoreConfig(
                    dbPath = File(dir, "tomi.db").path,
                    filesDir = filesDir.path,
                    wrapKey = wrapKey,
                    // Empty in a build without a relay (release by default): the device then waits for one (link, QR code, "Relais" field).
                    relayUrl = BuildConfig.RELAY_URL.ifBlank { null },
                    appVersion = BuildConfig.VERSION_NAME,
                    platform = "android",
                    autoImport = false,
                ),
            )
        } finally {
            wrapKey.fill(0)
        }
        return try {
            RealTomiCore(
                native = native,
                scope = scope,
                validDeviceName = ::isValidDeviceName,
                deviceNameCandidates = { deviceNameCandidates(app) },
                attachmentUriOf = { path, mime, name ->
                    val extension = mime?.let { MimeTypeMap.getSingleton().getExtensionFromMimeType(it) }
                    AttachmentUris.uriFor(path, CoreFiles.plaintextCacheDir(app), AttachmentUris.authority(app.packageName), extension, name)
                },
                makeThumbnail = DraftThumbnails::make,
            )
        } catch (e: Exception) {
            // The first read of the state failed: nothing observes the native core yet (no listener, no collector), so it is released
            // here, before "Réessayer" / "Réinitialiser" open or erase the same database.
            try {
                native.close()
            } catch (_: Exception) {
                // Nothing more to do: the original failure is the one to report.
            }
            throw e
        }
    }

    private fun coreDir(app: Application): File = CoreFiles.coreDir(app)

    /**
     * Erases everything the core stored (database, sealed wrap key, decrypted-file cache) and the Keystore key that wrapped the wrap key.
     * Used by the reset of the "unreadable data" screen: if it is the Keystore key that is defective, the next [real] creates a new one
     * instead of failing again. Without risk: the database it protected is erased with it.
     */
    fun wipeCoreData(app: Application) = wipe(coreDir(app), ::deleteKeystoreKey)

    /** Deletes the default Keystore entry of [AndroidKeystoreWrapper]. Best effort: a Keystore that refuses must not block the reset. */
    private fun deleteKeystoreKey() {
        try {
            val keyStore = KeyStore.getInstance("AndroidKeyStore")
            keyStore.load(null)
            keyStore.deleteEntry(AndroidKeystoreWrapper.DEFAULT_ALIAS)
        } catch (_: Exception) {
            // Nothing is logged (a message may carry details of the Keystore); loadOrCreate reports it if the key is still unusable.
        }
    }

    /** Throws [IOException] when something could not be deleted: a reset that leaves the damaged data behind must not look successful. */
    internal fun wipe(dir: File, deleteKeystoreKey: () -> Unit = {}) {
        if (dir.exists() && !dir.deleteRecursively()) throw IOException("the local data of Tomi could not be erased")
        // After the directory: if it could not be erased, the sealed wrap key is still there and needs its Keystore key.
        deleteKeystoreKey()
    }

    /**
     * The names this phone can announce, best first: the one the user gave it in the system settings ("Pixel 8 Phone"; readable
     * without any permission), then the model. [eu.shaffe.tomi.core.DeviceNames] picks the first the core accepts.
     */
    fun deviceNameCandidates(app: Application): List<String?> =
        listOf(Settings.Global.getString(app.contentResolver, Settings.Global.DEVICE_NAME), Build.MODEL)
}
