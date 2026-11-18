package eu.shaffe.tomi.security

import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import java.io.File
import java.io.FileOutputStream
import java.io.IOException
import java.nio.file.Files
import java.nio.file.StandardCopyOption
import java.security.GeneralSecurityException
import java.security.KeyStore
import java.security.SecureRandom
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

/** The wrap key of the core cannot be recovered (its protecting key is gone or the file was altered). The device must be set up again. */
class WrapKeyUnavailableException(message: String, cause: Throwable? = null) : IllegalStateException(message, cause)

/** Seals a small secret under a key that never leaves the secure hardware of the phone. */
interface KeyWrapper {
    fun seal(plain: ByteArray, aad: ByteArray): ByteArray

    /** Throws [GeneralSecurityException] when [sealed] was altered or the key is not the one that sealed it. */
    fun open(sealed: ByteArray, aad: ByteArray): ByteArray
}

/**
 * The 32-byte key that seals the secrets of the core's database (`CoreConfig.wrap_key`, crates/core/src/ffi.rs): the device keys and the
 * space key are stored sealed under it, never in clear.
 *
 * The key is random (the platform CSPRNG), generated once, and kept on disk only sealed by a [KeyWrapper] (the Android Keystore in the app).
 * Nothing here is a cryptographic primitive: the sealing is the Keystore's AES-GCM, the randomness is [SecureRandom].
 *
 * Losing the Keystore key (it is deleted with the app data, or the phone is restored on another one) makes the sealed secrets of the
 * database unreadable by design: [loadOrCreate] then throws [WrapKeyUnavailableException] instead of silently making a new key, which would
 * hide the loss and leave the device unable to read its own keys.
 */
class WrapKeyStore(
    private val file: File,
    private val wrapper: KeyWrapper,
    private val random: SecureRandom = SecureRandom(),
) {
    /**
     * A fresh copy of the key each time; the caller wipes it (`fill(0)`) once the core has opened. Creates and stores it on the first call.
     */
    @Synchronized
    fun loadOrCreate(): ByteArray {
        if (file.exists()) return load()
        val key = ByteArray(KEY_BYTES).also(random::nextBytes)
        try {
            store(key)
        } catch (e: Exception) {
            key.fill(0)
            throw e
        }
        return key
    }

    private fun load(): ByteArray {
        val blob = try {
            file.readBytes()
        } catch (e: IOException) {
            throw WrapKeyUnavailableException("the wrap key file cannot be read", e)
        }
        val key = try {
            wrapper.open(blob, AAD)
        } catch (e: GeneralSecurityException) {
            throw WrapKeyUnavailableException("the wrap key cannot be opened: its protecting key is gone or the file was altered", e)
        }
        if (key.size != KEY_BYTES) {
            key.fill(0)
            throw WrapKeyUnavailableException("the wrap key has an unexpected size")
        }
        return key
    }

    /** Written next to its destination then moved over it: a crash leaves either no file or a complete one. */
    private fun store(key: ByteArray) {
        val parent = file.absoluteFile.parentFile ?: throw IOException("no parent directory for ${file.name}")
        parent.mkdirs()
        val temp = File(parent, file.name + ".tmp")
        FileOutputStream(temp).use { out ->
            out.write(wrapper.seal(key, AAD))
            out.flush()
            out.fd.sync()
        }
        Files.move(temp.toPath(), file.toPath(), StandardCopyOption.ATOMIC_MOVE, StandardCopyOption.REPLACE_EXISTING)
    }

    companion object {
        const val KEY_BYTES = 32

        /** Binds the sealed key to this use: the same Keystore key can never open it as something else. */
        private val AAD = "tomi/android/core-wrap-key/v1".toByteArray(Charsets.US_ASCII)
    }
}

/**
 * [KeyWrapper] over the Android Keystore: an AES-256-GCM key that never leaves the secure hardware. The key does not require the user to
 * be present: the core must open in the background (a push wakes the app while the phone is locked). The sealed form is
 * `version(1) || iv(12) || ciphertext || tag(16)`.
 */
class AndroidKeystoreWrapper(private val alias: String = DEFAULT_ALIAS) : KeyWrapper {
    override fun seal(plain: ByteArray, aad: ByteArray): ByteArray {
        val cipher = Cipher.getInstance(TRANSFORMATION)
        cipher.init(Cipher.ENCRYPT_MODE, key())
        cipher.updateAAD(aad)
        val ciphertext = cipher.doFinal(plain)
        return byteArrayOf(VERSION) + cipher.iv + ciphertext
    }

    override fun open(sealed: ByteArray, aad: ByteArray): ByteArray {
        if (sealed.size <= 1 + IV_BYTES || sealed[0] != VERSION) throw GeneralSecurityException("unknown sealed format")
        val cipher = Cipher.getInstance(TRANSFORMATION)
        cipher.init(Cipher.DECRYPT_MODE, key(), GCMParameterSpec(TAG_BITS, sealed, 1, IV_BYTES))
        cipher.updateAAD(aad)
        return cipher.doFinal(sealed, 1 + IV_BYTES, sealed.size - 1 - IV_BYTES)
    }

    private fun key(): SecretKey {
        val store = KeyStore.getInstance(PROVIDER).apply { load(null) }
        return (store.getKey(alias, null) as? SecretKey) ?: generate()
    }

    /**
     * Hardware-backed by the trusted environment (the Keystore picks it). StrongBox is not requested: it is much slower (about a
     * second to make the key, a noticeable delay at each start to open it) and has device quirks, for a key that only seals data
     * on a phone the user already unlocks. `setIsStrongBoxBacked(true)` is the switch if that trade-off changes.
     */
    private fun generate(): SecretKey {
        val generator = KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, PROVIDER)
        generator.init(
            KeyGenParameterSpec.Builder(alias, KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT)
                .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                .setKeySize(KEY_BITS)
                .build(),
        )
        return generator.generateKey()
    }

    companion object {
        const val DEFAULT_ALIAS = "eu.shaffe.tomi.core.wrap.v1"
        private const val PROVIDER = "AndroidKeyStore"
        private const val TRANSFORMATION = "AES/GCM/NoPadding"
        private const val VERSION: Byte = 1
        private const val KEY_BITS = 256
        private const val IV_BYTES = 12
        private const val TAG_BITS = 128
    }
}
