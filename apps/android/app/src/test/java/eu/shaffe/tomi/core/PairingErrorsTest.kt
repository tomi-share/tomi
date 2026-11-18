package eu.shaffe.tomi.core

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertThrows
import org.junit.Assert.assertTrue
import org.junit.Test
import uniffi.tomi_core.TomiException

/** What the Rust core throws, as the app understands it (crates/core/src/ffi.rs `TomiError`, spec/pairing-v1.md 11.5). */
class PairingErrorsTest {
    private fun failure(e: TomiException) = PairingErrors.failureOf(e)

    @Test
    fun thePairingCodesOfTheCoreHaveTheirOwnFailure() {
        assertEquals(PairingFailure.WrongCode, failure(pairingError("pairing_failed")))
        assertEquals(PairingFailure.Expired, failure(pairingError("pairing_expired")))
        assertEquals(PairingFailure.Taken, failure(pairingError("pairing_taken")))
        assertEquals(PairingFailure.Cancelled, failure(pairingError("pairing_cancelled")))
        assertEquals(PairingFailure.UpdateRequired, failure(pairingError("pairing_unsupported")))
    }

    @Test
    fun anotherCryptoCodeIsUnexpected() {
        assertEquals(PairingFailure.Unexpected, failure(pairingError("decryption_failed")))
    }

    @Test
    fun theRefusalsOfTheRelayKeepTheirMeaning() {
        assertEquals(PairingFailure.SpaceFull, failure(relayError(409, "too_many_devices")))
        assertEquals(PairingFailure.AnotherPairingOpen, failure(relayError(409, "channel_limit")))
        assertEquals(PairingFailure.InstallTokenInvalid, failure(relayError(403, "install_token_invalid")))
        assertEquals(PairingFailure.SpaceExists, failure(relayError(409, "space_exists")))
        assertEquals(PairingFailure.UpdateRequired, failure(relayError(409, "update_required")))
        assertEquals(PairingFailure.UpdateRequired, failure(relayError(409, "format_unsupported")))
        assertEquals(PairingFailure.Unexpected, failure(relayError(409, "epoch_conflict")))
        assertEquals(PairingFailure.Unexpected, failure(relayError(404, "not_found")))
    }

    @Test
    fun aBusyOrBrokenRelayIsAMatterOfTryingAgain() {
        assertEquals(PairingFailure.Network, failure(relayError(429, "rate_limited")))
        assertEquals(PairingFailure.Network, failure(relayError(500, "internal")))
        assertEquals(PairingFailure.Network, failure(relayError(503, "unavailable")))
        assertEquals(PairingFailure.Network, failure(TomiException.Network("offline")))
    }

    @Test
    fun aBlockedDeviceIsToldApartFromAnOutdatedApp() {
        assertEquals(PairingFailure.UpdateRequired, failure(TomiException.Blocked("UpdateRequired { update_url: Some(\"u\") }")))
        assertEquals(PairingFailure.Blocked, failure(TomiException.Blocked("Revoked")))
        assertEquals(PairingFailure.Blocked, failure(TomiException.Blocked("NotFound")))
    }

    @Test
    fun localFailuresAreUnexpected() {
        assertEquals(PairingFailure.Unexpected, failure(TomiException.Storage("disk")))
        assertEquals(PairingFailure.Unexpected, failure(TomiException.Internal("bug")))
        assertEquals(PairingFailure.Unexpected, failure(TomiException.InvalidInput("not the moment")))
    }

    @Test
    fun onlyTheErrorsThatLeaveASessionAliveAreTransient() {
        assertTrue(PairingErrors.isTransient(TomiException.Network("offline")))
        assertTrue(PairingErrors.isTransient(TomiException.Storage("busy")))
        assertTrue(PairingErrors.isTransient(relayError(429, "rate_limited")))
        assertTrue(PairingErrors.isTransient(relayError(502, "bad_gateway")))
        assertFalse(PairingErrors.isTransient(relayError(409, "channel_limit")))
        assertFalse(PairingErrors.isTransient(pairingError("pairing_failed")))
        assertFalse(PairingErrors.isTransient(TomiException.InvalidInput("not the moment")))
        assertFalse(PairingErrors.isTransient(TomiException.Blocked("Revoked")))
    }

    @Test
    fun aTransientCallIsRepeatedAfterTheBackOffThenGivesUp() {
        var calls = 0
        val slept = mutableListOf<Long>()
        val result = PairingErrors.retryingTransient(listOf(10L, 20L), { slept += it }) {
            if (++calls < 3) throw TomiException.Network("offline") else "ok"
        }
        assertEquals("ok", result)
        assertEquals(listOf(10L, 20L), slept)

        calls = 0
        assertThrows(TomiException.Network::class.java) {
            PairingErrors.retryingTransient(listOf(1L), { }) {
                calls++
                throw TomiException.Network("offline")
            }
        }
        assertEquals("the first try and one retry", 2, calls)
    }

    @Test
    fun aPermanentErrorIsNeverRepeated() {
        var calls = 0
        assertThrows(TomiException.Crypto::class.java) {
            PairingErrors.retryingTransient(listOf(1L, 1L), { }) {
                calls++
                throw pairingError("pairing_failed")
            }
        }
        assertEquals(1, calls)
    }

    @Test
    fun theErrorOfAnotherCallKeepsTheCoreExceptionAsItsCause() {
        val cause = relayError(409, "channel_limit")
        val e = PairingErrors.coreException(cause)
        assertEquals(cause, e.cause)
        assertEquals("core: relay refused (409 channel_limit)", e.message)
    }
}
