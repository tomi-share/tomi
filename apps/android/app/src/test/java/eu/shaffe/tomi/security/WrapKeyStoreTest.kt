package eu.shaffe.tomi.security

import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertNotSame
import org.junit.Assert.assertThrows
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder
import java.io.File
import java.security.GeneralSecurityException

/** The key that seals the secrets of the core's database: random, created once, stored only sealed. */
class WrapKeyStoreTest {
    @get:Rule
    val folder = TemporaryFolder()

    /** Stands for the Android Keystore: a reversible mask with a check byte, so that a changed file or a wrong binding is refused. */
    private class MaskWrapper : KeyWrapper {
        var seals = 0
        var lost = false

        override fun seal(plain: ByteArray, aad: ByteArray): ByteArray {
            seals++
            return byteArrayOf(1) + plain.map { (it.toInt() xor MASK).toByte() }.toByteArray() + byteArrayOf(aad.size.toByte())
        }

        override fun open(sealed: ByteArray, aad: ByteArray): ByteArray {
            if (lost || sealed.size < 2 || sealed.first() != 1.toByte() || sealed.last() != aad.size.toByte()) {
                throw GeneralSecurityException("cannot open")
            }
            return sealed.copyOfRange(1, sealed.size - 1).map { (it.toInt() xor MASK).toByte() }.toByteArray()
        }

        private companion object {
            const val MASK = 0x5A
        }
    }

    private fun store(wrapper: MaskWrapper = MaskWrapper(), name: String = "wrap-key.bin") = WrapKeyStore(File(folder.root, name), wrapper)

    @Test
    fun theFirstCallMakesAKeyOfTheRightSizeAndStoresItSealed() {
        val wrapper = MaskWrapper()
        val key = store(wrapper).loadOrCreate()

        assertEquals(32, key.size)
        val file = File(folder.root, "wrap-key.bin")
        assertTrue(file.isFile)
        assertFalse("the key is not stored in clear", file.readBytes().toList().windowed(key.size).any { it == key.toList() })
        assertEquals(1, wrapper.seals)
        assertEquals("nothing is left of the temporary file", listOf("wrap-key.bin"), folder.root.list()!!.toList())
    }

    @Test
    fun theNextCallsGiveTheSameKeyInAFreshArrayAndMakeNothingNew() {
        val wrapper = MaskWrapper()
        val first = store(wrapper).loadOrCreate()
        val second = store(wrapper).loadOrCreate()

        assertArrayEquals(first, second)
        assertNotSame(first, second)
        assertEquals("the key is made once", 1, wrapper.seals)
    }

    @Test
    fun wipingTheCopyOfTheCallerLeavesTheStoredKeyIntact() {
        val store = store()
        val first = store.loadOrCreate()
        val original = first.copyOf()
        first.fill(0)

        assertArrayEquals(original, store.loadOrCreate())
    }

    @Test
    fun twoDevicesDoNotShareAKey() {
        val a = store(name = "a.bin").loadOrCreate()
        val b = store(name = "b.bin").loadOrCreate()
        assertFalse(a.contentEquals(b))
        assertNotEquals(ByteArray(32).toList(), a.toList())
    }

    @Test
    fun anAlteredFileIsRefusedRatherThanReplaced() {
        val wrapper = MaskWrapper()
        store(wrapper).loadOrCreate()
        val file = File(folder.root, "wrap-key.bin")
        val altered = file.readBytes().also { it[0] = 9 }
        file.writeBytes(altered)

        assertThrows(WrapKeyUnavailableException::class.java) { store(wrapper).loadOrCreate() }
        assertArrayEquals("the file is left as it is", altered, file.readBytes())
        assertEquals("and no new key was made", 1, wrapper.seals)
    }

    @Test
    fun aLostProtectingKeyIsRefusedRatherThanReplacedBySilentlyMakingANewOne() {
        val wrapper = MaskWrapper()
        store(wrapper).loadOrCreate()
        wrapper.lost = true

        assertThrows(WrapKeyUnavailableException::class.java) { store(wrapper).loadOrCreate() }
        assertEquals(1, wrapper.seals)
    }

    @Test
    fun aKeyOfTheWrongSizeIsRefused() {
        val wrapper = MaskWrapper()
        File(folder.root, "wrap-key.bin").writeBytes(wrapper.seal(ByteArray(16), "tomi/android/core-wrap-key/v1".toByteArray()))
        assertThrows(WrapKeyUnavailableException::class.java) { store(wrapper).loadOrCreate() }
    }
}
