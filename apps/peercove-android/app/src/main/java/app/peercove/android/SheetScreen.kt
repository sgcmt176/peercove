package app.peercove.android

// 共有シート(Excel ライク表、M6 G-2、ADR-0054)。共有メモ・共有スケジュール表の
// 基盤(ホスト正本 DB・コントロールチャネル配信・読み取りキャッシュ・世代
// ポーリング)を転用する。V1 は閲覧 + セルタップ編集(矢印キー移動や TSV
// 貼り付けはデスクトップのみ、ADR-0054 決定 8)。セルの競合はセル単位の
// revision CAS(ADR-0054 決定 4)。**シート名・セル値は Log に出さないこと。**

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Link
import androidx.compose.material.icons.filled.MoreVert
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.FilterChip
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalClipboardManager
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import uniffi.peercove_mobile.MobileException
import uniffi.peercove_mobile.SheetCellInfo
import uniffi.peercove_mobile.SheetCellWriteArg
import uniffi.peercove_mobile.SheetMetaInfo
import uniffi.peercove_mobile.sharedMemoGeneration
import uniffi.peercove_mobile.sheetCells
import uniffi.peercove_mobile.sheetCreate
import uniffi.peercove_mobile.sheetDelete
import uniffi.peercove_mobile.sheetList
import uniffi.peercove_mobile.sheetRename
import uniffi.peercove_mobile.sheetWrite

// crates/peercove-core/src/sheet.rs の上限と同期(ADR-0054 決定 7)。
private const val MAX_SHEET_ROWS = 1000
private const val MAX_SHEET_COLS = 200

// スマホの幅ではデスクトップより控えめな最小表示範囲(仕様どおり最低 10 行 × 5 列)。
private const val MIN_DISPLAY_ROWS = 10
private const val MIN_DISPLAY_COLS = 5
private const val DISPLAY_MARGIN = 2

private val HEADER_COL_WIDTH = 40.dp
private val CELL_WIDTH = 96.dp
private val CELL_HEIGHT = 40.dp

private val SHEET_NUMERIC_RE = Regex("""^-?\d+(\.\d+)?$""")

/** 0-indexed 列番号 → A, B, ..., Z, AA, AB, ...(デスクトップの colLabel と同じ規則)。 */
private fun sheetColLabel(index: Int): String {
    var n = index + 1
    val sb = StringBuilder()
    while (n > 0) {
        val rem = (n - 1) % 26
        sb.insert(0, 'A' + rem)
        n = (n - 1) / 26
    }
    return sb.toString()
}

private data class SheetCellDialogState(
    val row: UInt,
    val col: UInt,
    val revision: ULong,
    val value: String,
    /** 直前の保存で競合したときの通知文(再表示用)。 */
    val conflictNotice: String? = null,
)

/** 共有ハブの「表」サブタブ(SharedHubTabSpec から呼ばれる)。 */
@Composable
fun SheetTab(
    slug: String,
    onNotice: (String) -> Unit,
    /** チャットの `@sheet:id` カード(ADR-0054)から開くシート。 */
    focusSheetId: String? = null,
    onFocusConsumed: () -> Unit = {},
) {
    val context = LocalContext.current
    val baseDir = context.filesDir.absolutePath
    val scope = rememberCoroutineScope()
    val clipboard = LocalClipboardManager.current

    var sheets by remember { mutableStateOf<List<SheetMetaInfo>>(emptyList()) }
    var sheetsLoaded by remember { mutableStateOf(false) }
    var activeSheetId by remember { mutableStateOf<String?>(null) }
    var cells by remember { mutableStateOf<List<SheetCellInfo>>(emptyList()) }
    var offline by remember { mutableStateOf(false) }
    var supported by remember { mutableStateOf(true) }
    var refreshTick by remember { mutableIntStateOf(0) }
    var menuOpen by remember { mutableStateOf(false) }
    var createDialog by remember { mutableStateOf(false) }
    var createName by remember { mutableStateOf("") }
    var renameTarget by remember { mutableStateOf<SheetMetaInfo?>(null) }
    var renameName by remember { mutableStateOf("") }
    var confirmDelete by remember { mutableStateOf<SheetMetaInfo?>(null) }
    var cellDialog by remember { mutableStateOf<SheetCellDialogState?>(null) }
    var saving by remember { mutableStateOf(false) }

    val offlineMsg = stringResource(R.string.sheet_offline)
    val unsupportedMsg = stringResource(R.string.sheet_unsupported)
    val conflictMsg = stringResource(R.string.sheet_conflict_notice)
    val copiedFmt = stringResource(R.string.notice_copied)

    // チャットの `@sheet:id` カードから開く(一覧が届いてから 1 回だけ)
    LaunchedEffect(focusSheetId, sheetsLoaded) {
        val id = focusSheetId ?: return@LaunchedEffect
        if (!sheetsLoaded) return@LaunchedEffect
        if (sheets.any { it.id == id }) activeSheetId = id
        onFocusConsumed()
    }

    // 世代ポーリング(ScheduleTab と同じ 2 秒流儀)。進んだら再取得する
    LaunchedEffect(Unit) {
        var lastGeneration = 0UL
        while (true) {
            val generation = withContext(Dispatchers.IO) { sharedMemoGeneration(slug) }
            if (generation != lastGeneration) {
                lastGeneration = generation
                refreshTick++
            }
            delay(2000)
        }
    }

    LaunchedEffect(refreshTick) {
        try {
            val result = withContext(Dispatchers.IO) { sheetList(baseDir, slug) }
            sheets = result.sheets
            offline = result.offline
            supported = result.supported
            if (activeSheetId == null || sheets.none { it.id == activeSheetId }) {
                activeSheetId = sheets.firstOrNull()?.id
            }
        } catch (e: MobileException) {
            onNotice(e.message ?: "")
        }
        sheetsLoaded = true
    }

    LaunchedEffect(activeSheetId, refreshTick) {
        val id = activeSheetId
        if (id == null) {
            cells = emptyList()
            return@LaunchedEffect
        }
        try {
            val result = withContext(Dispatchers.IO) { sheetCells(baseDir, slug, id) }
            cells = result.cells
            offline = result.offline
        } catch (e: MobileException) {
            onNotice(e.message ?: "")
        }
    }

    val readOnly = offline || !supported
    val activeSheet = sheets.firstOrNull { it.id == activeSheetId }
    val cellsByKey = remember(cells) { cells.associateBy { it.row to it.col } }
    val usedRows = cells.maxOfOrNull { it.row.toInt() + 1 } ?: 0
    val usedCols = cells.maxOfOrNull { it.col.toInt() + 1 } ?: 0
    val displayRows = (usedRows + DISPLAY_MARGIN).coerceIn(MIN_DISPLAY_ROWS, MAX_SHEET_ROWS)
    val displayCols = (usedCols + DISPLAY_MARGIN).coerceIn(MIN_DISPLAY_COLS, MAX_SHEET_COLS)

    fun copyLink(sheet: SheetMetaInfo) {
        val token = sharedRefToken(SharedRefKind.SHEET, sheet.id)
        clipboard.setText(AnnotatedString(token))
        onNotice(copiedFmt.format(token))
    }

    fun submitCell(state: SheetCellDialogState, newValue: String) {
        val id = activeSheetId ?: return
        saving = true
        scope.launch {
            try {
                val result = withContext(Dispatchers.IO) {
                    sheetWrite(
                        slug,
                        id,
                        // format = null(書式変更なし。表示・編集対応は M6 H-6、ADR-0055 決定 6)
                        listOf(SheetCellWriteArg(state.row, state.col, newValue, state.revision, null)),
                    )
                }
                if (result.conflicts.isNotEmpty()) {
                    // 他の端末が先に更新済み。最新値を提示して再編集させる(ADR-0054 決定 4)
                    val conflict = result.conflicts.first()
                    cellDialog = state.copy(
                        revision = conflict.revision,
                        value = conflict.value,
                        conflictNotice = conflictMsg,
                    )
                } else {
                    cellDialog = null
                }
                refreshTick++
            } catch (e: MobileException) {
                onNotice(e.message ?: "")
            } finally {
                saving = false
            }
        }
    }

    confirmDelete?.let { target ->
        AlertDialog(
            onDismissRequest = { confirmDelete = null },
            title = { Text(stringResource(R.string.sheet_delete)) },
            text = { Text(stringResource(R.string.sheet_delete_confirm, target.name)) },
            confirmButton = {
                TextButton(onClick = {
                    confirmDelete = null
                    scope.launch {
                        try {
                            withContext(Dispatchers.IO) { sheetDelete(slug, target.id) }
                            if (activeSheetId == target.id) activeSheetId = null
                            refreshTick++
                        } catch (e: MobileException) {
                            onNotice(e.message ?: "")
                        }
                    }
                }) { Text(stringResource(R.string.action_remove)) }
            },
            dismissButton = {
                TextButton(onClick = { confirmDelete = null }) {
                    Text(stringResource(R.string.action_cancel))
                }
            },
        )
    }

    renameTarget?.let { target ->
        AlertDialog(
            onDismissRequest = { renameTarget = null },
            title = { Text(stringResource(R.string.sheet_rename_title)) },
            text = {
                OutlinedTextField(
                    value = renameName,
                    onValueChange = { renameName = it },
                    singleLine = true,
                    label = { Text(stringResource(R.string.sheet_name_label)) },
                )
            },
            confirmButton = {
                TextButton(
                    enabled = renameName.isNotBlank(),
                    onClick = {
                        val newName = renameName.trim()
                        renameTarget = null
                        scope.launch {
                            try {
                                withContext(Dispatchers.IO) { sheetRename(slug, target.id, newName) }
                                refreshTick++
                            } catch (e: MobileException) {
                                onNotice(e.message ?: "")
                            }
                        }
                    },
                ) { Text(stringResource(R.string.action_save)) }
            },
            dismissButton = {
                TextButton(onClick = { renameTarget = null }) {
                    Text(stringResource(R.string.action_cancel))
                }
            },
        )
    }

    if (createDialog) {
        AlertDialog(
            onDismissRequest = { createDialog = false },
            title = { Text(stringResource(R.string.sheet_create_title)) },
            text = {
                OutlinedTextField(
                    value = createName,
                    onValueChange = { createName = it },
                    singleLine = true,
                    label = { Text(stringResource(R.string.sheet_name_label)) },
                    placeholder = { Text(stringResource(R.string.sheet_name_placeholder)) },
                )
            },
            confirmButton = {
                TextButton(
                    enabled = createName.isNotBlank(),
                    onClick = {
                        val newName = createName.trim()
                        createDialog = false
                        scope.launch {
                            try {
                                val sheet = withContext(Dispatchers.IO) { sheetCreate(slug, newName) }
                                activeSheetId = sheet.id
                                refreshTick++
                            } catch (e: MobileException) {
                                onNotice(e.message ?: "")
                            }
                        }
                    },
                ) { Text(stringResource(R.string.action_save)) }
            },
            dismissButton = {
                TextButton(onClick = { createDialog = false }) {
                    Text(stringResource(R.string.action_cancel))
                }
            },
        )
    }

    cellDialog?.let { state ->
        SheetCellEditDialog(
            addressLabel = sheetColLabel(state.col.toInt()) + (state.row.toInt() + 1),
            initialValue = state.value,
            readOnly = readOnly,
            conflictNotice = state.conflictNotice,
            saving = saving,
            onDismiss = { cellDialog = null },
            onSave = { newValue -> submitCell(state, newValue) },
            onClear = { submitCell(state, "") },
        )
    }

    Column(modifier = Modifier.fillMaxSize()) {
        Row(
            modifier = Modifier
                .fillMaxWidth()
                .horizontalScroll(rememberScrollState())
                .padding(horizontal = 12.dp, vertical = 6.dp),
            horizontalArrangement = Arrangement.spacedBy(6.dp),
        ) {
            sheets.forEach { sheet ->
                FilterChip(
                    selected = sheet.id == activeSheetId,
                    onClick = { activeSheetId = sheet.id },
                    label = {
                        Text(
                            sheet.name.ifEmpty { stringResource(R.string.memo_untitled) },
                            maxLines = 1,
                            overflow = TextOverflow.Ellipsis,
                        )
                    },
                )
            }
            if (!readOnly) {
                FilterChip(
                    selected = false,
                    onClick = {
                        createName = ""
                        createDialog = true
                    },
                    label = { Text("+") },
                )
            }
        }
        if (offline) {
            Text(
                offlineMsg,
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.primary,
                modifier = Modifier.padding(horizontal = 12.dp, vertical = 4.dp),
            )
        } else if (!supported) {
            Text(
                unsupportedMsg,
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                modifier = Modifier.padding(horizontal = 12.dp, vertical = 4.dp),
            )
        }
        activeSheet?.let { sheet ->
            Row(
                verticalAlignment = Alignment.CenterVertically,
                modifier = Modifier.fillMaxWidth().padding(horizontal = 12.dp),
            ) {
                Text(
                    sheet.name.ifEmpty { stringResource(R.string.memo_untitled) },
                    style = MaterialTheme.typography.titleSmall,
                    modifier = Modifier.weight(1f),
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
                Box {
                    IconButton(onClick = { menuOpen = true }) {
                        Icon(
                            Icons.Filled.MoreVert,
                            contentDescription = stringResource(R.string.sheet_menu),
                        )
                    }
                    DropdownMenu(expanded = menuOpen, onDismissRequest = { menuOpen = false }) {
                        DropdownMenuItem(
                            text = { Text(stringResource(R.string.sheet_copy_link)) },
                            leadingIcon = { Icon(Icons.Filled.Link, contentDescription = null) },
                            onClick = {
                                menuOpen = false
                                copyLink(sheet)
                            },
                        )
                        if (sheet.canManage) {
                            DropdownMenuItem(
                                text = { Text(stringResource(R.string.sheet_rename)) },
                                onClick = {
                                    menuOpen = false
                                    renameName = sheet.name
                                    renameTarget = sheet
                                },
                            )
                            DropdownMenuItem(
                                text = {
                                    Text(
                                        stringResource(R.string.sheet_delete),
                                        color = MaterialTheme.colorScheme.error,
                                    )
                                },
                                onClick = {
                                    menuOpen = false
                                    confirmDelete = sheet
                                },
                            )
                        }
                    }
                }
            }
        }

        if (sheets.isEmpty() && sheetsLoaded) {
            Column(
                modifier = Modifier.fillMaxWidth().padding(24.dp),
                horizontalAlignment = Alignment.CenterHorizontally,
            ) {
                Text(
                    stringResource(R.string.sheet_empty),
                    style = MaterialTheme.typography.bodyMedium,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
                if (!readOnly) {
                    Spacer(modifier = Modifier.height(8.dp))
                    TextButton(onClick = {
                        createName = ""
                        createDialog = true
                    }) {
                        Text(stringResource(R.string.sheet_add))
                    }
                }
            }
        } else if (activeSheet != null) {
            Column(
                modifier = Modifier
                    .weight(1f)
                    .fillMaxWidth()
                    .padding(top = 4.dp)
                    .verticalScroll(rememberScrollState())
                    .horizontalScroll(rememberScrollState()),
            ) {
                Row {
                    SheetGridCell(text = "", width = HEADER_COL_WIDTH, isHeader = true)
                    for (c in 0 until displayCols) {
                        SheetGridCell(text = sheetColLabel(c), width = CELL_WIDTH, isHeader = true)
                    }
                }
                for (r in 0 until displayRows) {
                    Row {
                        SheetGridCell(text = (r + 1).toString(), width = HEADER_COL_WIDTH, isHeader = true)
                        for (c in 0 until displayCols) {
                            val cell = cellsByKey[r.toUInt() to c.toUInt()]
                            val value = cell?.value ?: ""
                            SheetGridCell(
                                text = value,
                                width = CELL_WIDTH,
                                numeric = value.isNotEmpty() && SHEET_NUMERIC_RE.matches(value),
                                onClick = {
                                    cellDialog = SheetCellDialogState(
                                        row = r.toUInt(),
                                        col = c.toUInt(),
                                        revision = cell?.revision ?: 0uL,
                                        value = value,
                                    )
                                },
                            )
                        }
                    }
                }
                // 横スクロール表の下端の余白
                Spacer(modifier = Modifier.height(24.dp))
            }
        }
    }
}

/** グリッドのセル 1 つ(見出し・データ兼用。等幅・固定幅・枠線)。 */
@Composable
private fun SheetGridCell(
    text: String,
    width: Dp,
    isHeader: Boolean = false,
    numeric: Boolean = false,
    onClick: (() -> Unit)? = null,
) {
    Box(
        modifier = Modifier
            .width(width)
            .height(CELL_HEIGHT)
            .border(0.5.dp, MaterialTheme.colorScheme.outlineVariant)
            .background(if (isHeader) MaterialTheme.colorScheme.surfaceVariant else Color.Transparent)
            .let { m -> if (onClick != null) m.clickable(onClick = onClick) else m }
            .padding(horizontal = 6.dp),
        contentAlignment = if (numeric) Alignment.CenterEnd else Alignment.CenterStart,
    ) {
        Text(
            text,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
            style = if (isHeader) MaterialTheme.typography.labelSmall else MaterialTheme.typography.bodySmall,
            color = if (isHeader) {
                MaterialTheme.colorScheme.onSurfaceVariant
            } else {
                MaterialTheme.colorScheme.onSurface
            },
        )
    }
}

/** セル編集ダイアログ。オフライン/未対応時は値を表示するだけの読み取り専用になる。 */
@Composable
private fun SheetCellEditDialog(
    addressLabel: String,
    initialValue: String,
    readOnly: Boolean,
    conflictNotice: String?,
    saving: Boolean,
    onDismiss: () -> Unit,
    onSave: (String) -> Unit,
    onClear: () -> Unit,
) {
    var text by remember(initialValue) { mutableStateOf(initialValue) }
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(addressLabel) },
        text = {
            Column {
                if (conflictNotice != null) {
                    Text(
                        conflictNotice,
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.error,
                    )
                    Spacer(modifier = Modifier.height(8.dp))
                }
                OutlinedTextField(
                    value = text,
                    onValueChange = { text = it },
                    enabled = !readOnly && !saving,
                    modifier = Modifier.fillMaxWidth(),
                    label = { Text(stringResource(R.string.sheet_cell_value_label)) },
                )
            }
        },
        confirmButton = {
            if (!readOnly) {
                TextButton(enabled = !saving, onClick = { onSave(text) }) {
                    Text(stringResource(R.string.action_save))
                }
            }
        },
        dismissButton = {
            Row {
                if (!readOnly) {
                    TextButton(enabled = !saving, onClick = onClear) {
                        Text(stringResource(R.string.sheet_clear_cell))
                    }
                    Spacer(modifier = Modifier.width(4.dp))
                }
                TextButton(onClick = onDismiss) { Text(stringResource(R.string.action_cancel)) }
            }
        },
    )
}
