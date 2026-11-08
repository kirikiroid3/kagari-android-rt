package club.suitsu31.kagari.simulator

import android.os.Build
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.dynamicDarkColorScheme
import androidx.compose.material3.dynamicLightColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext

// A warm, bonfire-orange scheme (篝, kagari) for devices without dynamic color (before Android 12).
private val LightColors = lightColorScheme(
    primary = Color(0xFF8F4C1E),
    onPrimary = Color(0xFFFFFFFF),
    primaryContainer = Color(0xFFFFDBC8),
    onPrimaryContainer = Color(0xFF713608),
    secondary = Color(0xFF765848),
    onSecondary = Color(0xFFFFFFFF),
    secondaryContainer = Color(0xFFFFDBC8),
    onSecondaryContainer = Color(0xFF5C4132),
    tertiary = Color(0xFF636032),
    onTertiary = Color(0xFFFFFFFF),
    tertiaryContainer = Color(0xFFEAE5AB),
    onTertiaryContainer = Color(0xFF4B481C),
    background = Color(0xFFFFF8F5),
    onBackground = Color(0xFF221A15),
    surface = Color(0xFFFFF8F5),
    onSurface = Color(0xFF221A15),
    surfaceVariant = Color(0xFFF4DED3),
    onSurfaceVariant = Color(0xFF52443C),
    surfaceContainerLowest = Color(0xFFFFFFFF),
    surfaceContainerLow = Color(0xFFFFF1EA),
    surfaceContainer = Color(0xFFFCEAE1),
    surfaceContainerHigh = Color(0xFFF6E5DB),
    surfaceContainerHighest = Color(0xFFF0DFD6),
    outline = Color(0xFF85746B),
    outlineVariant = Color(0xFFD7C3B8),
)

private val DarkColors = darkColorScheme(
    primary = Color(0xFFFFB68A),
    onPrimary = Color(0xFF532200),
    primaryContainer = Color(0xFF713608),
    onPrimaryContainer = Color(0xFFFFDBC8),
    secondary = Color(0xFFE6BEAB),
    onSecondary = Color(0xFF432B1D),
    secondaryContainer = Color(0xFF5C4132),
    onSecondaryContainer = Color(0xFFFFDBC8),
    tertiary = Color(0xFFCEC991),
    onTertiary = Color(0xFF343107),
    tertiaryContainer = Color(0xFF4B481C),
    onTertiaryContainer = Color(0xFFEAE5AB),
    background = Color(0xFF19120D),
    onBackground = Color(0xFFF0DFD6),
    surface = Color(0xFF19120D),
    onSurface = Color(0xFFF0DFD6),
    surfaceVariant = Color(0xFF52443C),
    onSurfaceVariant = Color(0xFFD7C3B8),
    surfaceContainerLowest = Color(0xFF140D08),
    surfaceContainerLow = Color(0xFF221A15),
    surfaceContainer = Color(0xFF261E19),
    surfaceContainerHigh = Color(0xFF312823),
    surfaceContainerHighest = Color(0xFF3C332D),
    outline = Color(0xFFA08D84),
    outlineVariant = Color(0xFF52443C),
)

/** Material 3, with the wallpaper's colors where the platform offers them (Android 12+). */
@Composable
fun KagariTheme(content: @Composable () -> Unit) {
    val dark = isSystemInDarkTheme()
    val colors = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
        val context = LocalContext.current
        if (dark) dynamicDarkColorScheme(context) else dynamicLightColorScheme(context)
    } else if (dark) {
        DarkColors
    } else {
        LightColors
    }
    MaterialTheme(colorScheme = colors, content = content)
}
