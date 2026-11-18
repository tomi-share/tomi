package eu.shaffe.tomi.ui.pairing

import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.outlined.KeyboardArrowRight
import androidx.compose.material.icons.outlined.Add
import androidx.compose.material.icons.outlined.Devices
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.role
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import eu.shaffe.tomi.R
import eu.shaffe.tomi.ui.common.screenContentPadding
import eu.shaffe.tomi.ui.common.tomiContentInsets
import eu.shaffe.tomi.ui.theme.TomiLogo
import eu.shaffe.tomi.ui.theme.TomiTheme

/**
 * First screen of a device that is not in a space yet: says what Tomi does and opens one of two doors, create the space
 * ([onCreateSpace]) or join it ([onJoinSpace]). The doors sit at the bottom, under the thumb; on a screen too small for that, the page
 * scrolls. Nothing secret is shown or typed here, so the window is not secure.
 */
@Composable
fun WelcomeScreen(onCreateSpace: () -> Unit, onJoinSpace: () -> Unit) {
    Scaffold(
        containerColor = MaterialTheme.colorScheme.background,
        // Status bar, gesture bar and cutout: see Insets.kt. No top bar here, the Scaffold padding clears the status bar.
        contentWindowInsets = tomiContentInsets,
    ) { padding ->
        BoxWithConstraints(Modifier.screenContentPadding(padding).fillMaxSize()) {
            Column(
                // At least the height of the screen, so that the weighted spacer has room to push the doors down.
                Modifier.fillMaxWidth().verticalScroll(rememberScrollState()).heightIn(min = maxHeight).padding(16.dp),
                verticalArrangement = Arrangement.spacedBy(16.dp),
            ) {
                TomiLogo(height = 32.dp, tint = MaterialTheme.colorScheme.onBackground)
                Text(
                    stringResource(R.string.welcome_title),
                    style = MaterialTheme.typography.titleLarge,
                    modifier = Modifier.padding(top = 8.dp),
                )
                Text(
                    stringResource(R.string.welcome_intro),
                    style = MaterialTheme.typography.bodyLarge,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
                Spacer(Modifier.weight(1f))
                Text(
                    stringResource(R.string.welcome_question),
                    style = MaterialTheme.typography.titleMedium,
                    modifier = Modifier.padding(top = 8.dp),
                )
                DoorCard(
                    icon = Icons.Outlined.Add,
                    title = stringResource(R.string.welcome_create_title),
                    hint = stringResource(R.string.welcome_create_hint),
                    onClick = onCreateSpace,
                )
                DoorCard(
                    icon = Icons.Outlined.Devices,
                    title = stringResource(R.string.welcome_join_title),
                    hint = stringResource(R.string.welcome_join_hint),
                    onClick = onJoinSpace,
                )
            }
        }
    }
}

/** One door: round icon, title, what it takes and a chevron. Read by the screen reader as one button. */
@Composable
private fun DoorCard(icon: ImageVector, title: String, hint: String, onClick: () -> Unit) {
    val description = stringResource(R.string.welcome_door_description, title, hint)
    Surface(
        onClick = onClick,
        shape = TomiTheme.shapes.card,
        color = MaterialTheme.colorScheme.surface,
        border = BorderStroke(1.dp, MaterialTheme.colorScheme.outlineVariant),
        modifier = Modifier.fillMaxWidth().semantics { role = Role.Button },
    ) {
        Row(
            Modifier
                .fillMaxWidth()
                .heightIn(min = 72.dp)
                .padding(horizontal = 16.dp, vertical = 12.dp)
                .clearAndSetSemantics { contentDescription = description },
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            Surface(shape = CircleShape, color = MaterialTheme.colorScheme.primaryContainer, modifier = Modifier.size(40.dp)) {
                Box(contentAlignment = Alignment.Center) {
                    Icon(icon, contentDescription = null, tint = MaterialTheme.colorScheme.onPrimaryContainer)
                }
            }
            Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(4.dp)) {
                Text(title, style = MaterialTheme.typography.titleMedium)
                Text(hint, style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
            }
            Icon(
                Icons.AutoMirrored.Outlined.KeyboardArrowRight,
                contentDescription = null,
                tint = MaterialTheme.colorScheme.onSurfaceVariant,
            )
        }
    }
}
