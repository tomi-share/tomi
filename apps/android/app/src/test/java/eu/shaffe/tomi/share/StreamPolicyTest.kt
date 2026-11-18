package eu.shaffe.tomi.share

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class StreamPolicyTest {
    private val ownPackage = "eu.shaffe.tomi"

    /** What PackageManager.resolveContentProvider answers: exact, case-sensitive authorities. */
    private val providers = mapOf(
        "eu.shaffe.tomi.fileprovider" to ownPackage,
        "com.android.providers.media.documents" to "com.android.providers.media",
        // Another app whose authority merely starts with our package name: not ours.
        "eu.shaffe.tomi.imposter" to "com.evil.app",
    )
    private val lookups = mutableListOf<String>()
    private val own = PackageManagerOwnAuthorityCheck(ownPackage) { authority ->
        lookups += authority
        providers[authority]
    }

    private fun accepted(uri: String) = StreamPolicy.isAcceptableUri(uri, own)

    @Test
    fun foreignContentUrisAreAccepted() {
        assertTrue(accepted("content://media/external/images/1"))
        assertTrue(accepted("content://com.android.providers.media.documents/document/image%3A5"))
        assertTrue(accepted("CONTENT://media/external/images/1"))
    }

    @Test
    fun ownProviderIsRefusedWhateverTheCaseOfTheAuthority() {
        assertFalse(accepted("content://eu.shaffe.tomi.fileprovider/x"))
        // The sender controls the case; the system lookup is exact, so the lower-cased form is tried too.
        assertFalse(accepted("content://EU.Shaffe.Tomi.FileProvider/x"))
        assertFalse(accepted("content://eu.shaffe.tomi.FILEPROVIDER/x"))
    }

    @Test
    fun userIdPrefixAndPercentEscapesCannotHideTheOwnProvider() {
        // Android's "userId@authority" form.
        assertFalse(accepted("content://0@eu.shaffe.tomi.fileprovider/x"))
        assertFalse(accepted("content://anything@EU.shaffe.tomi.fileprovider/x"))
        // Android percent-decodes the authority before resolving it.
        assertFalse(accepted("content://eu.shaffe%2Etomi.fileprovider/x"))
    }

    @Test
    fun ownershipComesFromTheSystemNotFromThePrefixOfTheName() {
        // Same prefix as our package but served by someone else: accepted.
        assertTrue(accepted("content://eu.shaffe.tomi.imposter/x"))
        // Our provider under an unrelated authority name would still be refused: the package decides.
        val renamed = PackageManagerOwnAuthorityCheck(ownPackage) { if (it == "files.example") ownPackage else null }
        assertFalse(StreamPolicy.isAcceptableUri("content://files.example/x", renamed))
    }

    @Test
    fun nonContentSchemesAndMalformedUrisAreRefused() {
        assertFalse(accepted("file:///data/data/eu.shaffe.tomi/databases/x"))
        assertFalse(accepted("http://example.org/a.png"))
        assertFalse(accepted("content:///no-authority"))
        assertFalse(accepted("content://@/x"))
        assertFalse(accepted("content://has space/x"))
        assertFalse(accepted(""))
    }

    @Test
    fun aFailingLookupFailsClosed() {
        val broken = PackageManagerOwnAuthorityCheck(ownPackage) { throw IllegalStateException("package manager died") }
        assertFalse(StreamPolicy.isAcceptableUri("content://media/external/images/1", broken))
    }

    @Test
    fun theSystemIsAskedWithTheBareAuthorityBothAsGivenAndLowerCased() {
        lookups.clear()
        accepted("content://7@Some.Authority/x")
        assertEquals(listOf("Some.Authority", "some.authority"), lookups)
    }
}
