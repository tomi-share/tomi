package eu.shaffe.tomi.core

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/** The "Relais" field: what is typed becomes a relay as the core reads it, or nothing; who decides which relay a call uses. */
class RelayAddressTest {
    private fun canonical(raw: String) = RelayAddress.canonical(raw, TestLinkReader)

    private fun choose(typed: String, configured: String?) = RelayAddress.choose(typed, configured, TestLinkReader)

    @Test
    fun aTypedRelayMayLeaveTheSchemeOut() {
        assertEquals("https://relay.example.org", canonical("relay.example.org"))
        assertEquals("https://relay.example.org", canonical("  https://Relay.Example.org/ "))
        assertEquals("https://relay.example.org:8443", canonical("relay.example.org:8443"))
    }

    @Test
    fun onlyHttpsHostsAreRelays() {
        assertNull(canonical(""))
        assertNull(canonical("   "))
        assertNull(canonical("http://relay.example.org"))
        assertNull(canonical("ftp://relay.example.org"))
        assertNull(canonical("user@relay.example.org"))
        assertNull(canonical("relay.example.org/a?b=1"))
        assertNull(canonical("relay.example.org#frag"))
        assertNull(canonical("relay example.org"))
        assertNull(canonical("rel\u0000ay.example.org"))
        assertNull(canonical("a".repeat(RelayAddress.MAX_INPUT + 1)))
    }

    @Test
    fun theAuthorityIsShownInFull() {
        assertEquals("relay.example.org", RelayAddress.authority("https://relay.example.org"))
        assertEquals("relay.example.org:8443", RelayAddress.authority("https://relay.example.org:8443"))
        assertEquals("relay.example.org", RelayAddress.authority("https://relay.example.org/"))
    }

    @Test
    fun whatIsTypedWinsOverTheRelayOfTheDevice() {
        assertEquals(RelayChoice.Typed("https://autre.example.net"), choose("autre.example.net", "https://relay.example.org"))
        assertEquals(RelayChoice.Typed("https://autre.example.net"), choose("autre.example.net", null))
    }

    @Test
    fun withNothingTypedTheRelayOfTheDeviceIsUsed() {
        assertEquals(RelayChoice.DeviceRelay, choose("", "https://relay.example.org"))
        assertEquals(RelayChoice.DeviceRelay, choose("   ", "https://relay.example.org"))
    }

    @Test
    fun withoutAnyRelayTheFieldIsRequired() {
        assertEquals(RelayChoice.Missing, choose("", null))
        assertEquals(RelayChoice.Missing, choose("  ", ""))
    }

    @Test
    fun aTypedRelayThatIsNotOneIsInvalidEvenWhenTheDeviceHasOne() {
        assertEquals(RelayChoice.Invalid, choose("http://relay.example.org", "https://relay.example.org"))
        assertEquals(RelayChoice.Invalid, choose("not a relay", null))
    }

    @Test
    fun theFieldIsOpenAndRequiredOnlyWhenNoRelayIsKnown() {
        assertTrue(RelayAddress.fieldRequired(null, null))
        assertTrue(RelayAddress.fieldRequired("", " "))
        assertFalse(RelayAddress.fieldRequired("https://relay.example.org", null))
        // A relay read from a link counts: the field is not required, the link says which relay.
        assertFalse(RelayAddress.fieldRequired(null, "https://relay.example.org"))
    }
}
