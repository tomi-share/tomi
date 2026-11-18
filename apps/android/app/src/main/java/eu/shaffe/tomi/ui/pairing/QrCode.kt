package eu.shaffe.tomi.ui.pairing

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import com.google.zxing.BarcodeFormat
import com.google.zxing.EncodeHintType
import com.google.zxing.qrcode.QRCodeWriter
import com.google.zxing.qrcode.decoder.ErrorCorrectionLevel
import kotlin.math.floor

/** A QR code: [size] x [size] modules, row by row. No quiet zone, no scaling: the drawing adds the margin. */
class QrMatrix(val size: Int, private val dark: BooleanArray) {
    operator fun get(x: Int, y: Int): Boolean = dark[y * size + x]
}

object QrCode {
    /** Quiet zone around the modules, in modules (ISO 18004 asks for 4). */
    const val QUIET_ZONE = 4

    /**
     * Encodes [content] (byte mode, error correction level M as the spec advises) and returns the modules only. The content is the pairing
     * link, exactly as the core gives it: the QR code carries no more than the typed code (spec/pairing-v1.md 4.3).
     */
    fun encode(content: String): QrMatrix {
        val hints = mapOf(
            EncodeHintType.ERROR_CORRECTION to ErrorCorrectionLevel.M,
            EncodeHintType.CHARACTER_SET to "UTF-8",
            EncodeHintType.MARGIN to 0,
        )
        val bits = QRCodeWriter().encode(content, BarcodeFormat.QR_CODE, 0, 0, hints)
        val size = bits.width
        val dark = BooleanArray(size * size)
        for (y in 0 until size) for (x in 0 until size) dark[y * size + x] = bits.get(x, y)
        return QrMatrix(size, dark)
    }
}

/**
 * Draws [content] as a QR code: black modules on a white square with the quiet zone, whatever the theme (scanners expect dark on light).
 * [description] is what the screen reader says; the content itself is never exposed to it.
 */
@Composable
fun QrCodeView(content: String, description: String, modifier: Modifier = Modifier) {
    val matrix = remember(content) { QrCode.encode(content) }
    Canvas(modifier.aspectRatio(1f).semantics { contentDescription = description }) {
        drawRect(Color.White)
        val modules = matrix.size + 2 * QrCode.QUIET_ZONE
        // Whole pixels per module, centred: no hairline between two neighbouring modules.
        val cell = floor(size.minDimension / modules).coerceAtLeast(1f)
        val origin = (size.minDimension - cell * modules) / 2f + cell * QrCode.QUIET_ZONE
        for (y in 0 until matrix.size) {
            for (x in 0 until matrix.size) {
                if (matrix[x, y]) drawRect(Color.Black, Offset(origin + x * cell, origin + y * cell), Size(cell, cell))
            }
        }
    }
}
