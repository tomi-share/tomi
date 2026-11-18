package eu.shaffe.tomi

import eu.shaffe.tomi.send.FileAttachmentStaging
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder
import java.io.File

/** What the reset and the start-up erase: the data of the core, and the clear copies of outgoing files */
class CoreFactoryWipeTest {
    @get:Rule
    val folder = TemporaryFolder()

    @Test
    fun `wipe removes the whole core directory and tolerates a missing one`() {
        val dir = folder.newFolder("tomi-core")
        File(dir, "tomi.db").writeText("x")
        File(dir, "files").mkdirs()
        File(dir, "files/cached.bin").writeText("clear")

        CoreFactory.wipe(dir)
        assertFalse(dir.exists())

        CoreFactory.wipe(dir) // nothing left: not an error
    }

    @Test
    fun `wipe deletes the Keystore key after the directory, so that a defective key does not make the reset a dead end (audit A-L3)`() {
        val dir = folder.newFolder("tomi-core")
        File(dir, "wrap-key.bin").writeText("sealed")
        val calls = mutableListOf<Boolean>()

        CoreFactory.wipe(dir) { calls += dir.exists() }

        assertEquals(listOf(false), calls) // called once, when the directory is already gone

        CoreFactory.wipe(dir) { calls += dir.exists() } // nothing to erase: the key is still deleted
        assertEquals(listOf(false, false), calls)
    }

    @Test
    fun `purgeAll removes every staged copy of outgoing files and keeps the rest of the cache`() {
        val cache = folder.newFolder("cache")
        val staged = File(cache, "${FileAttachmentStaging.ROOT}/draft-1").apply { mkdirs() }
        File(staged, "0-secret.txt").writeText("clear copy of a secret")
        val other = File(cache, "other").apply { mkdirs() }

        FileAttachmentStaging.purgeAll(cache)

        assertFalse(File(cache, FileAttachmentStaging.ROOT).exists())
        assertTrue(other.exists())
        FileAttachmentStaging.purgeAll(cache) // idempotent
    }
}
