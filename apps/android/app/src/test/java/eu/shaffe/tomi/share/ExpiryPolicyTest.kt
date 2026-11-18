package eu.shaffe.tomi.share

import eu.shaffe.tomi.core.DraftAttachment
import eu.shaffe.tomi.core.ReadPolicy
import eu.shaffe.tomi.core.RichText
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import java.time.Duration
import java.time.Instant

/** Product decision: nothing expires unless the user chose it, secrets included. */
class ExpiryPolicyTest {
    private val now = Instant.parse("2026-11-04T10:00:00Z")
    private val parsed = ParsedShare(title = null, text = RichText("Hx7!pQ2-zR9"), url = null, streams = emptyList(), looksSecret = true)

    @Test
    fun theDefaultIsNeverForEveryItemSecretsIncluded() {
        assertEquals(ExpiryChoice.Never, ExpiryPolicy.DEFAULT)
        for (secret in listOf(false, true)) {
            val resolved = ExpiryPolicy.resolve(ExpiryPolicy.DEFAULT, secret, hasAttachment = false, now = now)
            assertNull("secret=$secret", resolved.expiresAt)
            assertEquals(ReadPolicy.Normal, resolved.readPolicy)
            // "Never" is the first chip, so it is what the selector shows first.
            assertEquals(ExpiryChoice.Never, ExpiryPolicy.optionsFor(secret, hasAttachment = false).first())
            assertEquals(ExpiryChoice.Never, ExpiryPolicy.optionsFor(secret, hasAttachment = true).first())
        }
    }

    @Test
    fun optionsAreNeverFirstThenAfterReadForHiddenContentThenDurationsWithoutFiveMinutes() {
        assertEquals(
            listOf(ExpiryChoice.Never, ExpiryChoice.Hour1, ExpiryChoice.Day1, ExpiryChoice.Day7, ExpiryChoice.Day30),
            ExpiryPolicy.optionsFor(secret = false, hasAttachment = false),
        )
        assertEquals(
            listOf(ExpiryChoice.Never, ExpiryChoice.AfterFirstRead, ExpiryChoice.Hour1, ExpiryChoice.Day1, ExpiryChoice.Day7, ExpiryChoice.Day30),
            ExpiryPolicy.optionsFor(secret = true, hasAttachment = false),
        )
        // No 5-minute choice exists any more.
        assertTrue(ExpiryChoice.entries.none { it.duration == Duration.ofMinutes(5) })
    }

    @Test
    fun aSecretDraftBuiltWithTheDefaultsHasNoExpiryAtAll() {
        val draft = OutgoingDraftFactory.create("id", parsed, emptyList(), secret = true, expiry = ExpiryPolicy.DEFAULT, now = now)
        assertTrue(draft.secret)
        assertNull(draft.expiresAt)
        assertEquals(ReadPolicy.Normal, draft.readPolicy)
    }

    @Test
    fun explicitDurationsAreHonouredForBothKinds() {
        for (secret in listOf(false, true)) {
            assertEquals(now.plus(Duration.ofHours(1)), ExpiryPolicy.resolve(ExpiryChoice.Hour1, secret, hasAttachment = false, now = now).expiresAt)
            assertEquals(now.plus(Duration.ofDays(30)), ExpiryPolicy.resolve(ExpiryChoice.Day30, secret, hasAttachment = false, now = now).expiresAt)
        }
        assertEquals(
            listOf(ExpiryChoice.Hour1, ExpiryChoice.Day1, ExpiryChoice.Day7, ExpiryChoice.Day30),
            ExpiryPolicy.optionsFor(secret = true, hasAttachment = false).filter { it.duration != null },
        )
    }

    @Test
    fun afterFirstReadIsAnOptInForSecretsWithoutAHiddenTimeLimit() {
        assertTrue(ExpiryPolicy.isAllowed(ExpiryChoice.AfterFirstRead, secret = true, hasAttachment = false))
        assertFalse(ExpiryPolicy.isAllowed(ExpiryChoice.AfterFirstRead, secret = false, hasAttachment = false))
        val resolved = ExpiryPolicy.resolve(ExpiryChoice.AfterFirstRead, secret = true, hasAttachment = false, now = now)
        assertEquals(ReadPolicy.AfterFirstRead, resolved.readPolicy)
        assertNull("no expiry is added on top of the user's choice", resolved.expiresAt)
    }

    @Test
    fun aChoiceNotAllowedForTheContentFallsBackToNoExpiry() {
        val resolved = ExpiryPolicy.resolve(ExpiryChoice.AfterFirstRead, secret = false, hasAttachment = false, now = now)
        assertNull(resolved.expiresAt)
        assertEquals(ReadPolicy.Normal, resolved.readPolicy)
    }

    // --- product decision: no "after first read" when the item has an attachment (the file would be lost after "Afficher") ---

    @Test
    fun afterFirstReadIsNotOfferedWhenTheItemHasAnAttachment() {
        for (secret in listOf(false, true)) {
            assertFalse("secret=$secret", ExpiryChoice.AfterFirstRead in ExpiryPolicy.optionsFor(secret, hasAttachment = true))
            assertFalse("secret=$secret", ExpiryPolicy.isAllowed(ExpiryChoice.AfterFirstRead, secret, hasAttachment = true))
        }
        // The other choices are untouched by an attachment.
        assertEquals(
            listOf(ExpiryChoice.Never, ExpiryChoice.Hour1, ExpiryChoice.Day1, ExpiryChoice.Day7, ExpiryChoice.Day30),
            ExpiryPolicy.optionsFor(secret = true, hasAttachment = true),
        )
        assertEquals(ExpiryPolicy.optionsFor(secret = false, hasAttachment = false), ExpiryPolicy.optionsFor(secret = false, hasAttachment = true))
    }

    @Test
    fun afterFirstReadIsKeptForASecretWithoutAttachment() {
        assertEquals(
            listOf(ExpiryChoice.Never, ExpiryChoice.AfterFirstRead, ExpiryChoice.Hour1, ExpiryChoice.Day1, ExpiryChoice.Day7, ExpiryChoice.Day30),
            ExpiryPolicy.optionsFor(secret = true, hasAttachment = false),
        )
    }

    @Test
    fun aForcedAfterFirstReadOnAnItemWithAnAttachmentResolvesToNoExpiryAtAll() {
        val resolved = ExpiryPolicy.resolve(ExpiryChoice.AfterFirstRead, secret = true, hasAttachment = true, now = now)
        assertNull(resolved.expiresAt)
        assertEquals(ReadPolicy.Normal, resolved.readPolicy)
        // Same through the draft factory, which is what reaches the core.
        val file = DraftAttachment("/tmp/a.pdf", "a.pdf", "application/pdf", 10)
        val draft = OutgoingDraftFactory.create("id", parsed, listOf(file), secret = true, expiry = ExpiryChoice.AfterFirstRead, now = now)
        assertNull(draft.expiresAt)
        assertEquals(ReadPolicy.Normal, draft.readPolicy)
        // Without a file the choice is honoured.
        val noFile = OutgoingDraftFactory.create("id", parsed, emptyList(), secret = true, expiry = ExpiryChoice.AfterFirstRead, now = now)
        assertEquals(ReadPolicy.AfterFirstRead, noFile.readPolicy)
    }

    @Test
    fun attachingAFileDropsAnAfterFirstReadChoiceBackToTheDefault() {
        assertEquals(ExpiryChoice.Never, ExpiryPolicy.onContentChanged(ExpiryChoice.AfterFirstRead, secret = true, hasAttachment = true))
        assertEquals(ExpiryChoice.Day7, ExpiryPolicy.onContentChanged(ExpiryChoice.Day7, secret = true, hasAttachment = true))
        assertEquals(ExpiryChoice.AfterFirstRead, ExpiryPolicy.onContentChanged(ExpiryChoice.AfterFirstRead, secret = true, hasAttachment = false))
    }

    @Test
    fun togglingTheSecretSwitchKeepsTheUsersChoiceAndNeverAddsAnExpiry() {
        // Turning a message into a secret does not silently add a time limit.
        assertEquals(ExpiryChoice.Never, ExpiryPolicy.onContentChanged(ExpiryChoice.Never, secret = true, hasAttachment = false))
        assertEquals(ExpiryChoice.Day7, ExpiryPolicy.onContentChanged(ExpiryChoice.Day7, secret = true, hasAttachment = false))
        assertEquals(ExpiryChoice.Hour1, ExpiryPolicy.onContentChanged(ExpiryChoice.Hour1, secret = false, hasAttachment = false))
        // "After first read" does not exist for plain content: back to the default.
        assertEquals(ExpiryChoice.Never, ExpiryPolicy.onContentChanged(ExpiryChoice.AfterFirstRead, secret = false, hasAttachment = false))
    }
}
