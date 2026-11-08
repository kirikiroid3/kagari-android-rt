package club.suitsu31.kagari.simulator

import android.content.Context
import android.text.format.DateUtils
import android.text.format.Formatter
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.scaleIn
import androidx.compose.animation.scaleOut
import androidx.compose.foundation.clickable
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.RowScope
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.calculateEndPadding
import androidx.compose.foundation.layout.calculateStartPadding
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyListState
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.ExtendedFloatingActionButton
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.LargeTopAppBar
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.ListItem
import androidx.compose.material3.ListItemDefaults
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Scaffold
import androidx.compose.material3.SnackbarDuration
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.SnackbarResult
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.TopAppBarDefaults
import androidx.compose.material3.TopAppBarScrollBehavior
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.derivedStateOf
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.hapticfeedback.HapticFeedbackType
import androidx.compose.ui.input.nestedscroll.nestedScroll
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.LocalHapticFeedback
import androidx.compose.ui.platform.LocalLayoutDirection
import androidx.compose.ui.platform.LocalResources
import androidx.compose.ui.platform.LocalWindowInfo
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.style.LineBreak
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.LifecycleResumeEffect
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import java.text.NumberFormat
import kotlinx.coroutines.launch

private val ZIP_TYPES = arrayOf("application/zip", "application/x-zip-compressed", "application/octet-stream")

/** Cards stop growing here, on tablets and in landscape. */
private val MAX_CONTENT_WIDTH = 720.dp

/** The game list: import games from a ZIP file or a folder, play, delete. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun LauncherScreen(repository: GameRepository) {
    val library by repository.state.collectAsStateWithLifecycle()
    val import by repository.import.collectAsStateWithLifecycle()
    val context = LocalContext.current
    val resources = LocalResources.current
    val scope = rememberCoroutineScope()
    val snackbar = remember { SnackbarHostState() }

    LifecycleResumeEffect(repository) {
        // A game may have started or ended meanwhile.
        repository.refresh()
        onPauseOrDispose {}
    }

    val pickZip = rememberLauncherForActivityResult(ActivityResultContracts.OpenDocument()) { uri ->
        uri?.let(repository::importZip)
    }
    val pickFolder = rememberLauncherForActivityResult(ActivityResultContracts.OpenDocumentTree()) { uri ->
        uri?.let(repository::importFolder)
    }
    val importZip = { pickZip.launch(ZIP_TYPES) }
    val importFolder = { pickFolder.launch(null) }
    val play: (Game) -> Unit = { game ->
        scope.launch { context.startActivity(repository.launchIntent(game)) }
    }

    var showImportSheet by rememberSaveable { mutableStateOf(false) }
    var showAbout by rememberSaveable { mutableStateOf(false) }
    var deletingId by rememberSaveable { mutableStateOf<String?>(null) }

    val compact = with(LocalDensity.current) { LocalWindowInfo.current.containerSize.height.toDp() } < 480.dp
    val scrollBehavior = if (compact) {
        TopAppBarDefaults.pinnedScrollBehavior()
    } else {
        TopAppBarDefaults.exitUntilCollapsedScrollBehavior()
    }
    val listState = rememberLazyListState()
    val atTop by remember { derivedStateOf { listState.firstVisibleItemIndex == 0 } }
    val games = library?.games
    val importing = import as? ImportState.Running

    Scaffold(
        modifier = Modifier.nestedScroll(scrollBehavior.nestedScrollConnection),
        topBar = { LauncherTopBar(compact, scrollBehavior, onAbout = { showAbout = true }) },
        floatingActionButton = {
            AnimatedVisibility(
                visible = !games.isNullOrEmpty() && importing == null,
                enter = scaleIn() + fadeIn(),
                exit = scaleOut() + fadeOut(),
            ) {
                ExtendedFloatingActionButton(
                    text = { Text(stringResource(R.string.import_game)) },
                    icon = { Icon(AppIcons.Add, contentDescription = null) },
                    onClick = { showImportSheet = true },
                    expanded = atTop,
                )
            }
        },
        snackbarHost = { SnackbarHost(snackbar) },
    ) { padding ->
        when {
            // The first listing takes a few milliseconds: show nothing rather than a flash of "no games".
            games == null -> Unit
            games.isEmpty() && importing == null -> EmptyLibrary(compact, padding, importZip, importFolder)
            else -> GameList(
                games = games,
                runningId = library?.runningId,
                importing = importing,
                padding = padding,
                listState = listState,
                onPlay = play,
                onDelete = { deletingId = it.id },
            )
        }
    }

    if (showImportSheet) {
        val sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true)
        val choose = { pick: () -> Unit ->
            scope.launch { sheetState.hide() }.invokeOnCompletion {
                showImportSheet = false
                pick()
            }
        }
        ModalBottomSheet(onDismissRequest = { showImportSheet = false }, sheetState = sheetState) {
            Text(
                stringResource(R.string.import_title),
                style = MaterialTheme.typography.titleLarge,
                modifier = Modifier.padding(start = 24.dp, end = 24.dp, bottom = 8.dp),
            )
            ImportOption(
                AppIcons.Archive,
                stringResource(R.string.import_zip_title),
                stringResource(R.string.import_zip_summary),
            ) { choose(importZip) }
            ImportOption(
                AppIcons.Folder,
                stringResource(R.string.import_folder_title),
                stringResource(R.string.import_folder_summary),
            ) { choose(importFolder) }
            Spacer(Modifier.height(16.dp))
        }
    }

    val deleting = deletingId?.let { id -> games?.firstOrNull { it.id == id } }
    if (deleting != null) {
        val running = deleting.id == library?.runningId
        AlertDialog(
            onDismissRequest = { deletingId = null },
            icon = { Icon(AppIcons.Delete, contentDescription = null) },
            title = { Text(stringResource(R.string.delete_title, deleting.name)) },
            text = {
                val message = stringResource(R.string.delete_message)
                Text(if (running) message + "\n\n" + stringResource(R.string.delete_running) else message)
            },
            confirmButton = {
                TextButton(
                    onClick = {
                        deletingId = null
                        scope.launch {
                            repository.delete(deleting)
                            snackbar.showSnackbar(resources.getString(R.string.deleted, deleting.name))
                        }
                    },
                    colors = ButtonDefaults.textButtonColors(contentColor = MaterialTheme.colorScheme.error),
                ) { Text(stringResource(R.string.delete)) }
            },
            dismissButton = {
                TextButton(onClick = { deletingId = null }) { Text(stringResource(android.R.string.cancel)) }
            },
        )
    }

    when (val outcome = import) {
        is ImportState.Done -> LaunchedEffect(outcome) {
            val result = snackbar.showSnackbar(
                message = resources.getString(R.string.imported, outcome.game.name),
                actionLabel = resources.getString(R.string.play),
                withDismissAction = true,
                duration = SnackbarDuration.Long,
            )
            // Not before: a launcher recreated meanwhile shows it again.
            repository.acknowledgeImport()
            if (result == SnackbarResult.ActionPerformed) {
                play(outcome.game)
            }
        }
        is ImportState.Failed -> AlertDialog(
            onDismissRequest = repository::acknowledgeImport,
            icon = { Icon(AppIcons.Error, contentDescription = null) },
            title = { Text(stringResource(R.string.import_failed_title)) },
            text = { Text(stringResource(outcome.error.res, *outcome.error.args)) },
            confirmButton = {
                TextButton(onClick = repository::acknowledgeImport) { Text(stringResource(android.R.string.ok)) }
            },
        )
        else -> Unit
    }

    if (showAbout) {
        AboutDialog(onDismiss = { showAbout = false })
    }
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun LauncherTopBar(compact: Boolean, scrollBehavior: TopAppBarScrollBehavior, onAbout: () -> Unit) {
    val title = @Composable {
        Text(stringResource(R.string.app_name), maxLines = 1, overflow = TextOverflow.Ellipsis)
    }
    val actions: @Composable RowScope.() -> Unit = {
        IconButton(onClick = onAbout) {
            Icon(AppIcons.Info, contentDescription = stringResource(R.string.about))
        }
    }
    // A large bar would take a third of a phone in landscape.
    if (compact) {
        TopAppBar(title = title, actions = actions, scrollBehavior = scrollBehavior)
    } else {
        LargeTopAppBar(title = title, actions = actions, scrollBehavior = scrollBehavior)
    }
}

@Composable
private fun GameList(
    games: List<Game>,
    runningId: String?,
    importing: ImportState.Running?,
    padding: PaddingValues,
    listState: LazyListState,
    onPlay: (Game) -> Unit,
    onDelete: (Game) -> Unit,
) {
    val direction = LocalLayoutDirection.current
    LazyColumn(
        state = listState,
        modifier = Modifier.fillMaxSize(),
        contentPadding = PaddingValues(
            start = padding.calculateStartPadding(direction) + 16.dp,
            end = padding.calculateEndPadding(direction) + 16.dp,
            top = padding.calculateTopPadding() + 8.dp,
            // Clear of the floating action button.
            bottom = padding.calculateBottomPadding() + 96.dp,
        ),
        verticalArrangement = Arrangement.spacedBy(8.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
    ) {
        if (importing != null) {
            item(key = "import") {
                ImportCard(importing, Modifier.animateItem())
            }
        }
        items(games, key = { it.id }) { game ->
            GameCard(
                game = game,
                running = game.id == runningId,
                onPlay = { onPlay(game) },
                onDelete = { onDelete(game) },
                modifier = Modifier.animateItem(),
            )
        }
    }
}

@Composable
private fun GameCard(
    game: Game,
    running: Boolean,
    onPlay: () -> Unit,
    onDelete: () -> Unit,
    modifier: Modifier = Modifier,
) {
    val context = LocalContext.current
    val haptics = LocalHapticFeedback.current
    val colors = MaterialTheme.colorScheme
    var menu by remember { mutableStateOf(false) }
    val playLabel = stringResource(if (running) R.string.resume else R.string.play)
    val details = remember(game) { gameDetails(context, game) }
    Card(
        modifier = modifier.widthIn(max = MAX_CONTENT_WIDTH).fillMaxWidth(),
        shape = MaterialTheme.shapes.large,
        colors = CardDefaults.cardColors(
            containerColor = if (running) colors.secondaryContainer else colors.surfaceContainerHigh,
        ),
    ) {
        Row(
            modifier = Modifier
                .combinedClickable(
                    onClickLabel = playLabel,
                    onClick = onPlay,
                    onLongClickLabel = stringResource(R.string.more_options),
                    onLongClick = {
                        haptics.performHapticFeedback(HapticFeedbackType.LongPress)
                        menu = true
                    },
                )
                .padding(start = 16.dp, top = 14.dp, bottom = 14.dp, end = 4.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Monogram(game, running)
            Spacer(Modifier.width(16.dp))
            Column(Modifier.weight(1f)) {
                Text(
                    game.name,
                    style = MaterialTheme.typography.titleMedium,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
                Text(
                    details,
                    style = MaterialTheme.typography.bodyMedium,
                    color = if (running) colors.onSecondaryContainer else colors.onSurfaceVariant,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
                if (running) {
                    Spacer(Modifier.height(6.dp))
                    RunningBadge()
                }
            }
            Box {
                IconButton(onClick = { menu = true }) {
                    Icon(AppIcons.MoreVert, contentDescription = stringResource(R.string.more_options))
                }
                DropdownMenu(expanded = menu, onDismissRequest = { menu = false }) {
                    DropdownMenuItem(
                        text = { Text(playLabel) },
                        leadingIcon = { Icon(AppIcons.Play, contentDescription = null) },
                        onClick = {
                            menu = false
                            onPlay()
                        },
                    )
                    DropdownMenuItem(
                        text = { Text(stringResource(R.string.delete)) },
                        leadingIcon = { Icon(AppIcons.Delete, contentDescription = null) },
                        onClick = {
                            menu = false
                            onDelete()
                        },
                    )
                }
            }
        }
    }
}

/** "52 MB · Nov 8, 19:04": the game's size and import time, which tell two imports of a game apart. */
private fun gameDetails(context: Context, game: Game): String =
    context.getString(
        R.string.game_details,
        Formatter.formatShortFileSize(context, game.size),
        DateUtils.formatDateTime(
            context,
            game.importedAt,
            DateUtils.FORMAT_SHOW_DATE or DateUtils.FORMAT_SHOW_TIME or DateUtils.FORMAT_ABBREV_MONTH,
        ),
    )

/**
 * The game's initial on a tile; its color follows the game, not the list order, and stands out
 * on a running game's card.
 */
@Composable
private fun Monogram(game: Game, running: Boolean) {
    val colors = MaterialTheme.colorScheme
    val (container, content) = when {
        running -> colors.primary to colors.onPrimary
        Math.floorMod(game.id.hashCode(), 2) == 0 -> colors.primaryContainer to colors.onPrimaryContainer
        else -> colors.tertiaryContainer to colors.onTertiaryContainer
    }
    val initial = remember(game.name) {
        game.name.trim().takeIf { it.isNotEmpty() }
            ?.let { String(Character.toChars(it.codePointAt(0))).uppercase() } ?: "?"
    }
    Surface(color = container, contentColor = content, shape = RoundedCornerShape(16.dp)) {
        Box(Modifier.size(56.dp), contentAlignment = Alignment.Center) {
            Text(initial, style = MaterialTheme.typography.titleLarge)
        }
    }
}

@Composable
private fun RunningBadge() {
    Surface(color = MaterialTheme.colorScheme.primary, shape = CircleShape) {
        Text(
            stringResource(R.string.running),
            style = MaterialTheme.typography.labelSmall,
            modifier = Modifier.padding(horizontal = 8.dp, vertical = 2.dp),
        )
    }
}

@Composable
private fun ImportCard(state: ImportState.Running, modifier: Modifier = Modifier) {
    Card(
        modifier = modifier.widthIn(max = MAX_CONTENT_WIDTH).fillMaxWidth(),
        shape = MaterialTheme.shapes.large,
        colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.primaryContainer),
    ) {
        Column(Modifier.padding(16.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Icon(AppIcons.Archive, contentDescription = null)
                Spacer(Modifier.width(12.dp))
                Text(
                    if (state.name != null) {
                        stringResource(R.string.importing_name, state.name)
                    } else {
                        stringResource(R.string.importing)
                    },
                    style = MaterialTheme.typography.titleSmall,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                    modifier = Modifier.weight(1f),
                )
                if (state.fraction != null) {
                    Spacer(Modifier.width(12.dp))
                    Text(
                        remember(state.fraction) { NumberFormat.getPercentInstance().format(state.fraction) },
                        style = MaterialTheme.typography.labelLarge,
                    )
                }
            }
            Spacer(Modifier.height(14.dp))
            if (state.fraction == null) {
                LinearProgressIndicator(Modifier.fillMaxWidth())
            } else {
                val shown by animateFloatAsState(state.fraction, label = "import progress")
                LinearProgressIndicator(progress = { shown }, modifier = Modifier.fillMaxWidth())
            }
        }
    }
}

/** No games: what to import, and the two ways to. [compact] (a phone in landscape) drops the illustration. */
@Composable
private fun EmptyLibrary(
    compact: Boolean,
    padding: PaddingValues,
    onImportZip: () -> Unit,
    onImportFolder: () -> Unit,
) {
    Box(Modifier.fillMaxSize().padding(padding), contentAlignment = Alignment.Center) {
        Column(
            modifier = Modifier
                .verticalScroll(rememberScrollState())
                .padding(horizontal = 32.dp, vertical = if (compact) 8.dp else 24.dp),
            horizontalAlignment = Alignment.CenterHorizontally,
        ) {
            if (!compact) {
                Surface(
                    color = MaterialTheme.colorScheme.primaryContainer,
                    contentColor = MaterialTheme.colorScheme.onPrimaryContainer,
                    shape = CircleShape,
                ) {
                    Box(Modifier.size(112.dp), contentAlignment = Alignment.Center) {
                        Icon(AppIcons.Game, contentDescription = null, modifier = Modifier.size(56.dp))
                    }
                }
                Spacer(Modifier.height(24.dp))
            }
            Text(
                stringResource(R.string.empty_title),
                style = MaterialTheme.typography.headlineSmall,
                textAlign = TextAlign.Center,
            )
            Spacer(Modifier.height(8.dp))
            Text(
                stringResource(R.string.empty_body),
                // Even lines, and Japanese broken between phrases (Android 13+).
                style = MaterialTheme.typography.bodyLarge.copy(
                    lineBreak = LineBreak.Paragraph.copy(
                        strategy = LineBreak.Strategy.Balanced,
                        wordBreak = LineBreak.WordBreak.Phrase,
                    ),
                ),
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                textAlign = TextAlign.Center,
                modifier = Modifier.widthIn(max = if (compact) 560.dp else 360.dp),
            )
            Spacer(Modifier.height(if (compact) 20.dp else 32.dp))
            Button(onClick = onImportZip, modifier = Modifier.widthIn(min = 240.dp)) {
                Icon(AppIcons.Archive, contentDescription = null, modifier = Modifier.size(ButtonDefaults.IconSize))
                Spacer(Modifier.width(ButtonDefaults.IconSpacing))
                Text(stringResource(R.string.import_zip))
            }
            Spacer(Modifier.height(8.dp))
            OutlinedButton(onClick = onImportFolder, modifier = Modifier.widthIn(min = 240.dp)) {
                Icon(AppIcons.Folder, contentDescription = null, modifier = Modifier.size(ButtonDefaults.IconSize))
                Spacer(Modifier.width(ButtonDefaults.IconSpacing))
                Text(stringResource(R.string.import_folder))
            }
        }
    }
}

@Composable
private fun ImportOption(icon: ImageVector, title: String, summary: String, onClick: () -> Unit) {
    ListItem(
        headlineContent = { Text(title) },
        supportingContent = { Text(summary) },
        leadingContent = {
            Surface(
                color = MaterialTheme.colorScheme.secondaryContainer,
                contentColor = MaterialTheme.colorScheme.onSecondaryContainer,
                shape = CircleShape,
            ) {
                Box(Modifier.size(40.dp), contentAlignment = Alignment.Center) {
                    Icon(icon, contentDescription = null)
                }
            }
        },
        colors = ListItemDefaults.colors(containerColor = Color.Transparent),
        modifier = Modifier
            .clickable(onClick = onClick)
            .padding(horizontal = 8.dp),
    )
}

@Composable
private fun AboutDialog(onDismiss: () -> Unit) {
    val context = LocalContext.current
    val version = remember {
        context.packageManager.getPackageInfo(context.packageName, 0).versionName.orEmpty()
    }
    AlertDialog(
        onDismissRequest = onDismiss,
        icon = { Icon(AppIcons.Info, contentDescription = null) },
        title = { Text(stringResource(R.string.app_name)) },
        text = {
            Column {
                Text(stringResource(R.string.about_version, version))
                Spacer(Modifier.height(8.dp))
                Text(stringResource(R.string.about_bytecode))
            }
        },
        confirmButton = {
            TextButton(onClick = onDismiss) { Text(stringResource(android.R.string.ok)) }
        },
    )
}
