package eu.shaffe.tomi.ui.pairing

import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardCapitalization
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.unit.dp
import eu.shaffe.tomi.R
import eu.shaffe.tomi.core.RelayAddress
import eu.shaffe.tomi.ui.theme.TomiFonts
import eu.shaffe.tomi.ui.theme.TomiTheme

/**
 * The "Relais" field of the pairing and creation screens (spec/pairing-v1.md 4.3).
 *
 * - [required] (the device has no relay and no link named one): the field is open, there is nothing to fold it under.
 * - otherwise it is folded under "Autre relais" (open from the start when something is already typed or read from a link), and the relay
 *   of the device is named above it when there is one ([configuredRelay]).
 *
 * What is typed is only a candidate: the core checks it, shows it again on the verification screen and keeps it only once it succeeded.
 */
@Composable
internal fun RelayField(
    value: String,
    onValueChange: (String) -> Unit,
    required: Boolean,
    configuredRelay: String?,
    enabled: Boolean,
    imeAction: ImeAction,
    modifier: Modifier = Modifier,
) {
    var expanded by rememberSaveable { mutableStateOf(false) }
    Column(modifier, verticalArrangement = Arrangement.spacedBy(8.dp)) {
        if (!required && configuredRelay != null) {
            Text(
                stringResource(R.string.relay_current, RelayAddress.authority(configuredRelay)),
                style = MaterialTheme.typography.bodyMedium,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
        }
        if (required || expanded || value.isNotBlank()) {
            OutlinedTextField(
                value = value,
                onValueChange = {
                    expanded = true
                    onValueChange(it)
                },
                label = { Text(stringResource(R.string.relay_field_label)) },
                supportingText = { Text(stringResource(R.string.relay_field_hint)) },
                singleLine = true,
                enabled = enabled,
                shape = TomiTheme.shapes.field,
                textStyle = MaterialTheme.typography.bodyLarge.copy(fontFamily = TomiFonts.mono, fontFeatureSettings = TomiFonts.MONO_EXACT_FEATURES),
                keyboardOptions = KeyboardOptions(
                    capitalization = KeyboardCapitalization.None,
                    autoCorrectEnabled = false,
                    keyboardType = KeyboardType.Uri,
                    imeAction = imeAction,
                ),
                modifier = Modifier.fillMaxWidth(),
            )
        } else {
            TextButton(onClick = { expanded = true }, enabled = enabled) { Text(stringResource(R.string.relay_other_button)) }
        }
    }
}

/**
 * The relay in full, with the sentence that asks the user to check it. [label] is a format with the `host[:port]` (a relay that was read
 * from a QR code, a link or a field is not trusted: whoever showed it chose it), [hint] the sentence below.
 */
@Composable
internal fun RelayCheckCard(authority: String, label: Int, hint: Int, modifier: Modifier = Modifier) {
    Surface(
        shape = TomiTheme.shapes.card,
        color = MaterialTheme.colorScheme.surface,
        border = BorderStroke(1.dp, MaterialTheme.colorScheme.outlineVariant),
        modifier = modifier.fillMaxWidth(),
    ) {
        Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
            Text(
                stringResource(label, authority),
                style = MaterialTheme.typography.titleMedium.copy(fontFamily = TomiFonts.mono, fontFeatureSettings = TomiFonts.MONO_EXACT_FEATURES),
            )
            Text(
                stringResource(hint),
                style = MaterialTheme.typography.bodyMedium,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
        }
    }
}
