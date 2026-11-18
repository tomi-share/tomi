package eu.shaffe.tomi.ui.onboarding

import android.content.Intent
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.requiredSize
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.wrapContentWidth
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.outlined.StickyNote2
import androidx.compose.material.icons.filled.PushPin
import androidx.compose.material.icons.outlined.Close
import androidx.compose.material.icons.outlined.Download
import androidx.compose.material.icons.outlined.Language
import androidx.compose.material.icons.outlined.PushPin
import androidx.compose.material.icons.outlined.Share
import androidx.compose.material3.Button
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.TopAppBarDefaults
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.luminance
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.colorResource
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.LiveRegionMode
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.liveRegion
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.compose.LifecycleEventEffect
import eu.shaffe.tomi.R
import eu.shaffe.tomi.ui.common.screenContentPadding
import eu.shaffe.tomi.ui.common.tomiContentInsets
import eu.shaffe.tomi.ui.theme.TomiTheme

/**
 * Onboarding "one gesture": shows how to pin Tomi at the top of the system share sheet (long press on Tomi, then "Pin"),
 * and opens a share sheet with a harmless sample so the user can do it right away.
 *
 * Android does not tell the app whether Tomi is pinned, so the confirmation is declarative: when the screen comes back to
 * the foreground after the test chooser was launched (ON_RESUME), it shows what the share sheet should look like and asks.
 *
 * @param fromHelp true when opened from the menu of the feed (the close button is enough, no "Later" button).
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun OnboardingScreen(onDone: () -> Unit, fromHelp: Boolean) {
    val context = LocalContext.current
    val sampleText = stringResource(R.string.onboarding_sample_text)
    var chooserLaunched by rememberSaveable { mutableStateOf(false) }
    var checking by rememberSaveable { mutableStateOf(false) }

    // Not at the first ON_RESUME of the screen: only when coming back from the chooser launched from here.
    LifecycleEventEffect(Lifecycle.Event.ON_RESUME) {
        if (chooserLaunched) checking = true
    }

    val openChooser = {
        chooserLaunched = true
        val sample = Intent(Intent.ACTION_SEND)
            .setType("text/plain")
            .putExtra(Intent.EXTRA_TEXT, sampleText)
        context.startActivity(Intent.createChooser(sample, null))
    }

    Scaffold(
        containerColor = MaterialTheme.colorScheme.background,
        contentWindowInsets = tomiContentInsets,
        topBar = {
            TopAppBar(
                title = {},
                navigationIcon = {
                    IconButton(onClick = onDone) {
                        Icon(Icons.Outlined.Close, contentDescription = stringResource(R.string.action_close))
                    }
                },
                colors = TopAppBarDefaults.topAppBarColors(containerColor = MaterialTheme.colorScheme.background),
            )
        },
    ) { padding ->
        Column(Modifier.screenContentPadding(padding).fillMaxSize()) {
            Column(
                Modifier.weight(1f).fillMaxWidth().verticalScroll(rememberScrollState()).padding(horizontal = 16.dp),
            ) {
                if (checking) {
                    Text(
                        stringResource(R.string.onboarding_check_caption),
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                        modifier = Modifier.padding(top = 4.dp, bottom = 8.dp),
                    )
                    ShareSceneIllustration(
                        height = 220.dp,
                        description = stringResource(R.string.onboarding_scene_check_description),
                        pinned = true,
                    )
                    Text(
                        stringResource(R.string.onboarding_check_title),
                        style = MaterialTheme.typography.titleLarge,
                        // Announced when the screen switches to the confirmation.
                        modifier = Modifier.padding(top = 24.dp).semantics {
                            heading()
                            liveRegion = LiveRegionMode.Polite
                        },
                    )
                    Text(
                        stringResource(R.string.onboarding_check_body),
                        style = MaterialTheme.typography.bodyLarge,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                        modifier = Modifier.padding(top = 8.dp),
                    )
                } else {
                    Spacer(Modifier.height(4.dp))
                    ShareSceneIllustration(
                        height = 300.dp,
                        description = stringResource(R.string.onboarding_scene_pin_description),
                        pinned = false,
                    )
                    Text(
                        stringResource(R.string.onboarding_pin_title),
                        style = MaterialTheme.typography.titleLarge,
                        modifier = Modifier.padding(top = 24.dp).semantics { heading() },
                    )
                    Text(
                        stringResource(R.string.onboarding_pin_body),
                        style = MaterialTheme.typography.bodyLarge,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                        modifier = Modifier.padding(top = 8.dp),
                    )
                }
            }
            Column(
                Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 16.dp),
                verticalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                if (checking) {
                    Button(
                        onClick = onDone,
                        shape = TomiTheme.shapes.button,
                        modifier = Modifier.fillMaxWidth().height(48.dp),
                    ) { Text(stringResource(R.string.onboarding_check_yes)) }
                    OutlinedButton(
                        onClick = openChooser,
                        shape = TomiTheme.shapes.button,
                        modifier = Modifier.fillMaxWidth().height(48.dp),
                    ) {
                        Icon(Icons.Outlined.Share, contentDescription = null, modifier = Modifier.size(20.dp))
                        Spacer(Modifier.width(8.dp))
                        Text(stringResource(R.string.onboarding_check_again))
                    }
                } else {
                    Button(
                        onClick = openChooser,
                        shape = TomiTheme.shapes.button,
                        modifier = Modifier.fillMaxWidth().height(48.dp),
                    ) {
                        Icon(Icons.Outlined.Share, contentDescription = null, modifier = Modifier.size(20.dp))
                        Spacer(Modifier.width(8.dp))
                        Text(stringResource(R.string.onboarding_open_share))
                    }
                    if (!fromHelp) {
                        TextButton(
                            onClick = onDone,
                            shape = TomiTheme.shapes.button,
                            modifier = Modifier.fillMaxWidth().height(48.dp),
                        ) { Text(stringResource(R.string.onboarding_later)) }
                    }
                }
            }
        }
    }
}

/**
 * Drawn illustration of the system share sheet (the `ob-scene` scene).
 * [pinned] false: Tomi third, with the press ring and the "Pin" chip under it. [pinned] true: Tomi first, with a pin badge.
 * Decorative inside: the whole scene is one element for accessibility, described by [description].
 */
@Composable
private fun ShareSceneIllustration(height: Dp, description: String, pinned: Boolean) {
    val colors = MaterialTheme.colorScheme
    val shape = TomiTheme.shapes.menu
    val dark = colors.background.luminance() < 0.5f
    Box(
        Modifier
            .fillMaxWidth()
            .height(height)
            .clip(shape)
            .background(colors.surfaceVariant)
            .clearAndSetSemantics { contentDescription = description },
    ) {
        FakePageLines(Modifier.align(Alignment.TopStart).padding(start = 20.dp, end = 20.dp, top = 18.dp))
        Box(Modifier.fillMaxSize().background(colors.scrim.copy(alpha = if (dark) 0.32f else 0.16f)))
        Surface(
            modifier = Modifier.align(Alignment.BottomCenter).fillMaxWidth().padding(12.dp),
            shape = shape,
            color = colors.surface,
            shadowElevation = 6.dp,
        ) {
            Column(Modifier.padding(start = 12.dp, top = 12.dp, end = 12.dp, bottom = if (pinned) 14.dp else 60.dp)) {
                Text(
                    stringResource(R.string.onboarding_scene_share_title),
                    style = MaterialTheme.typography.bodySmall,
                    color = colors.onSurfaceVariant,
                )
                Row(
                    Modifier.fillMaxWidth().padding(top = 12.dp),
                    horizontalArrangement = Arrangement.SpaceBetween,
                ) {
                    if (pinned) TomiTarget(pinned = true, pressed = false)
                    ShareTarget(Icons.AutoMirrored.Outlined.StickyNote2, stringResource(R.string.onboarding_scene_notes))
                    ShareTarget(Icons.Outlined.Language, stringResource(R.string.onboarding_scene_browser))
                    if (!pinned) TomiTarget(pinned = false, pressed = true)
                    ShareTarget(Icons.Outlined.Download, stringResource(R.string.onboarding_scene_files))
                }
            }
        }
    }
}

/** The lines of the page that is being shared, behind the sheet. */
@Composable
private fun FakePageLines(modifier: Modifier = Modifier) {
    val colors = MaterialTheme.colorScheme
    val line = RoundedCornerShape(5.dp)
    Column(modifier) {
        Box(
            Modifier
                .fillMaxWidth()
                .height(28.dp)
                .clip(RoundedCornerShape(14.dp))
                .background(colors.surface)
                .border(1.dp, colors.outlineVariant, RoundedCornerShape(14.dp)),
        )
        Spacer(Modifier.height(18.dp))
        Box(Modifier.fillMaxWidth(0.62f).height(16.dp).clip(line).background(colors.outline.copy(alpha = 0.55f)))
        Spacer(Modifier.height(10.dp))
        Box(Modifier.fillMaxWidth().height(10.dp).clip(line).background(colors.outlineVariant))
        Spacer(Modifier.height(10.dp))
        Box(Modifier.fillMaxWidth(0.8f).height(10.dp).clip(line).background(colors.outlineVariant))
    }
}

/** An app of the share sheet: round icon and label. */
@Composable
private fun ShareTarget(icon: ImageVector, label: String) {
    Column(
        Modifier.width(64.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.spacedBy(6.dp),
    ) {
        Box(
            Modifier.size(48.dp).clip(CircleShape).background(MaterialTheme.colorScheme.surfaceVariant),
            contentAlignment = Alignment.Center,
        ) {
            Icon(icon, contentDescription = null, tint = MaterialTheme.colorScheme.onSurfaceVariant)
        }
        Text(
            label,
            style = MaterialTheme.typography.bodySmall,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
            textAlign = TextAlign.Center,
            maxLines = 1,
            overflow = TextOverflow.Clip,
        )
    }
}

/**
 * Tomi in the share sheet. [pressed]: press ring and "Pin" chip under the icon (the gesture to make).
 * [pinned]: round pin badge on the icon corner (what the sheet shows once Tomi is pinned).
 */
@Composable
private fun TomiTarget(pinned: Boolean, pressed: Boolean) {
    val colors = MaterialTheme.colorScheme
    Box(Modifier.width(64.dp)) {
        Column(
            Modifier.fillMaxWidth(),
            horizontalAlignment = Alignment.CenterHorizontally,
            verticalArrangement = Arrangement.spacedBy(6.dp),
        ) {
            // The app icon of the project (night tile and sun symbol, the only place where those colours are used), scaled
            // so that the symbol fills about 55 % of the tile like on the mock-up.
            Box(
                Modifier
                    .size(48.dp)
                    .clip(RoundedCornerShape(11.dp))
                    .background(colorResource(R.color.ic_launcher_background)),
                contentAlignment = Alignment.Center,
            ) {
                Image(painterResource(R.drawable.ic_launcher_foreground), contentDescription = null, modifier = Modifier.requiredSize(72.dp))
            }
            Text(
                stringResource(R.string.app_name),
                style = MaterialTheme.typography.bodySmall.copy(fontWeight = FontWeight.Medium),
                color = colors.onSurface,
                maxLines = 1,
            )
        }
        if (pressed) {
            Box(
                Modifier
                    .size(64.dp)
                    .offset(y = (-8).dp)
                    .background(colors.primary.copy(alpha = 0.18f), CircleShape)
                    .border(2.dp, colors.primary, CircleShape),
            )
            PinChip(Modifier.align(Alignment.TopCenter).offset(y = 84.dp).wrapContentWidth(unbounded = true))
        }
        if (pinned) {
            Box(
                Modifier
                    .align(Alignment.TopEnd)
                    .offset(x = (-2).dp, y = (-6).dp)
                    .size(22.dp)
                    .background(colors.primary, CircleShape),
                contentAlignment = Alignment.Center,
            ) {
                Icon(Icons.Filled.PushPin, contentDescription = null, tint = colors.onPrimary, modifier = Modifier.size(16.dp))
            }
        }
    }
}

/** The "Pin" entry that Android shows after a long press on an app of the share sheet. */
@Composable
private fun PinChip(modifier: Modifier = Modifier) {
    val colors = MaterialTheme.colorScheme
    Surface(
        modifier = modifier,
        shape = TomiTheme.shapes.button,
        color = colors.surface,
        border = BorderStroke(1.dp, colors.outline),
        shadowElevation = 6.dp,
    ) {
        Row(
            Modifier.height(40.dp).padding(start = 12.dp, end = 16.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            Icon(Icons.Outlined.PushPin, contentDescription = null, tint = colors.onSurface, modifier = Modifier.size(18.dp))
            Text(
                stringResource(R.string.action_pin),
                style = MaterialTheme.typography.labelLarge.copy(fontWeight = FontWeight.Medium),
                color = colors.onSurface,
                maxLines = 1,
            )
        }
    }
}
