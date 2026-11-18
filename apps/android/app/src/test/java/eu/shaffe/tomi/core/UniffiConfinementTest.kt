package eu.shaffe.tomi.core

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import java.io.File

/**
 * The records of the Rust core must stay inside the adapter. A generated record is a data class whose `toString()` prints every field,
 * the verification code and the pairing code included (crates/core/src/ffi.rs): the rule that keeps them out of a state, a log or a message
 * is structural. Reads the sources of the module; the unit tests of Gradle run from the module directory.
 */
class UniffiConfinementTest {
    private val root = File("src/main/java")

    private fun sources(): List<File> {
        assertTrue("the test must run from the module directory (app/)", root.isDirectory)
        return root.walkTopDown().filter { it.isFile && it.extension == "kt" }.toList()
    }

    private fun File.rel(): String = relativeTo(root).invariantSeparatorsPath

    private fun File.mentions(vararg words: String): Boolean = readText().let { text -> words.any { text.contains(it) } }

    @Test
    fun onlyTheAdapterAndTheFactoryImportTheGeneratedBindings() {
        val importers = sources()
            .filter { file -> file.readLines().any { it.trimStart().startsWith("import uniffi.") } }
            .map { it.rel() }
            .filterNot { it.startsWith("eu/shaffe/tomi/core/") || it == "eu/shaffe/tomi/CoreFactory.kt" }
        assertEquals("these files use the generated records", emptyList<String>(), importers)
    }

    @Test
    fun theVerificationCodeOfTheCoreIsReadByTheTwoSessionAdaptersOnly() {
        val allowed = setOf("eu/shaffe/tomi/core/RealInviteSession.kt", "eu/shaffe/tomi/core/RealTomiCore.kt")
        val prompts = sources().filter { it.mentions("FfiVerifyPrompt", "FfiJoinPrompt") }.map { it.rel() }
        assertTrue("these files name the prompt records: $prompts", allowed.containsAll(prompts))

        // The `sas` field of a prompt: read where the record is converted, nowhere else.
        val sasReaders = sources().filter { Regex("""\.sas\b""").containsMatchIn(it.readText()) }.map { it.rel() }
        assertTrue("these files read the sas field: $sasReaders", sasReaders.isNotEmpty() && allowed.containsAll(sasReaders))
    }

    /** The secrets of the pairing, and the departure (an erasure, its verdicts and the clipboard of the space). */
    private val named = setOf(
        "eu/shaffe/tomi/core/RealTomiCore.kt",
        "eu/shaffe/tomi/core/RealInviteSession.kt",
        "eu/shaffe/tomi/core/RealCoreMapping.kt",
        "eu/shaffe/tomi/core/PairingErrors.kt",
        "eu/shaffe/tomi/core/Pairing.kt",
        "eu/shaffe/tomi/core/BlockingBridge.kt",
        "eu/shaffe/tomi/core/LeaveSpace.kt",
        "eu/shaffe/tomi/core/DepartureRetrier.kt",
        "eu/shaffe/tomi/ui/feed/LeaveSpaceDialog.kt",
        "eu/shaffe/tomi/ui/feed/DepartureBanner.kt",
        "eu/shaffe/tomi/ui/feed/EraseWithoutProofDialog.kt",
        "eu/shaffe/tomi/LeftSpaceCleanup.kt",
    )

    @Test
    fun nothingThatTouchesASecretOfThePairingOrTheDepartureLogsAnything() {
        val guarded = sources().filter { it.rel().startsWith("eu/shaffe/tomi/ui/pairing/") || it.rel() in named }
        assertTrue("the pairing screens are there", guarded.count { it.rel().startsWith("eu/shaffe/tomi/ui/pairing/") } >= 12)
        assertEquals("every named file exists", named, guarded.map { it.rel() }.filter { it in named }.toSet())
        val logging = Regex("""\bLog\.[a-z]\(|\bprintln\(|\bprint\(|\.printStackTrace\(|Timber\.""")
        val offenders = guarded.filter { logging.containsMatchIn(it.readText()) }.map { it.rel() }
        assertEquals("these files log", emptyList<String>(), offenders)
    }
}
