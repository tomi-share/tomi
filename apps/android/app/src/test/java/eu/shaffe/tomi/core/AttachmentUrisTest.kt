package eu.shaffe.tomi.core

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder
import java.io.File

class AttachmentUrisTest {
    @get:Rule
    val tmp = TemporaryFolder()

    private val authority = AttachmentUris.authority("eu.shaffe.tomi")
    private val id = "3f2b8c1e-9a4d-4e7f-8b21-0c5d6e7f8a9b"

    private fun cache(): File = File(tmp.root, "files/files/cache").apply { mkdirs() }

    @Test
    fun aCacheFileBecomesAUriOfTheProviderWithAGenericNameOrTheRealOne() {
        val dir = cache()
        val file = File(dir, "${id}_2").apply { writeText("x") }
        assertEquals("content://eu.shaffe.tomi.attachments/${id}_2/fichier.jpg", AttachmentUris.uriFor(file.path, dir, authority, "jpg"))
        assertEquals("content://eu.shaffe.tomi.attachments/${id}_2/fichier", AttachmentUris.uriFor(file.path, dir, authority, null))
        assertEquals("not a plain extension", "fichier", AttachmentUris.displayName("../x"))
        // The real name of a file that is not hidden: encoded in the URI, read back as it was.
        assertEquals(
            "content://eu.shaffe.tomi.attachments/${id}_2/Mon%20classement%20%C3%A9t%C3%A9.pdf",
            AttachmentUris.uriFor(file.path, dir, authority, "pdf", "Mon classement été.pdf"),
        )
        assertEquals("fichier.pdf", AttachmentUris.displayName("pdf", " .. "))
        assertEquals("separators and reserved characters are replaced", "x_y_z.pdf", AttachmentUris.cleanName("x/y:z.pdf"))
    }

    @Test
    fun aPathOutsideTheCacheHasNoUri() {
        val dir = cache()
        val outside = File(tmp.root, "${id}_0").apply { writeText("x") }
        assertNull(AttachmentUris.uriFor(outside.path, dir, authority))
        assertNull(AttachmentUris.uriFor(File(dir, "../${id}_0").path, dir, authority))
        assertNull(AttachmentUris.uriFor(File(File(dir, "sub"), "${id}_0").path, dir, authority))
        assertNull("not a cache file name", AttachmentUris.uriFor(File(dir, "tomi.db").path, dir, authority))
    }

    /** The demo space decrypts into `demo/this/files/cache`: its files have a URI too, the real cache does not take them. */
    @Test
    fun aFileOfTheDemoCacheHasAUriFromThatCacheOnly() {
        val real = cache()
        val demo = File(tmp.root, "demo/this/files/cache").apply { mkdirs() }
        val file = File(demo, "${id}_1").apply { writeText("x") }
        assertNull(AttachmentUris.uriFor(file.path, real, authority, "png"))
        assertEquals("content://eu.shaffe.tomi.attachments/${id}_1/fichier.png", AttachmentUris.uriFor(file.path, demo, authority, "png"))
        assertEquals(file, AttachmentUris.fileFor(listOf("${id}_1", "fichier.png"), demo))
    }

    @Test
    fun aUriOnlyNamesAFileDirectlyInTheCache() {
        val dir = cache()
        assertEquals(File(dir, "${id}_0"), AttachmentUris.fileFor(listOf("${id}_0", "fichier.png"), dir))
        assertEquals(File(dir, "${id}_0"), AttachmentUris.fileFor(listOf("${id}_0", "fichier"), dir))
        assertEquals("png", AttachmentUris.extensionOf(listOf("${id}_0", "fichier.png")))
        assertNull(AttachmentUris.extensionOf(listOf("${id}_0", "fichier")))
        assertNull("one segment", AttachmentUris.fileFor(listOf("${id}_0"), dir))
        assertEquals("the name is cosmetic", File(dir, "${id}_0"), AttachmentUris.fileFor(listOf("${id}_0", "classement.pdf"), dir))
        assertEquals("pdf", AttachmentUris.extensionOf(listOf("${id}_0", "Mon.classement.PDF")))
        assertNull("not a clean name", AttachmentUris.fileFor(listOf("${id}_0", "a:b.png"), dir))
        assertNull("not a clean name", AttachmentUris.fileFor(listOf("${id}_0", ".."), dir))
        assertNull(AttachmentUris.fileFor(listOf("..", "tomi.db"), dir))
        assertNull(AttachmentUris.fileFor(listOf("..", "fichier"), dir))
        assertNull(AttachmentUris.fileFor(listOf("sub", "${id}_0", "fichier"), dir))
        assertNull(AttachmentUris.fileFor(listOf("${id}_0.tmp", "fichier"), dir))
        assertNull(AttachmentUris.fileFor(emptyList(), dir))
    }
}
