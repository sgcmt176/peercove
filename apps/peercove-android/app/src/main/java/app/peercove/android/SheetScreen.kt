package app.peercove.android

// 共有シート(Excel ライク表、M6 G-2/H-6、ADR-0054/ADR-0055)。共有メモ・共有
// スケジュール表の基盤(ホスト正本 DB・コントロールチャネル配信・読み取り
// キャッシュ・世代ポーリング)を転用する。V1 は閲覧 + セルタップ編集(矢印
// キー移動や TSV 貼り付け・書式編集・プレゼンス・検索・Undo・右クリック
// メニューはデスクトップのみ = ADR-0055 の範囲確認済み)。セルの競合は
// セル単位の revision CAS(ADR-0054 決定 4)。
//
// M6 H-6 で追加: セル書式の表示(太字/斜体/下線/取り消し線/文字色/背景色/
// 配置/フォントサイズ/罫線)、列幅・行高の反映、セル結合の表示(左上セルに
// スパン分の幅・高さを与え、内包セルはスキップする簡易実装。タップ編集は
// 左上セルのみ)、目盛線 OFF の反映。固定枠(freeze)は表示しない
// (スマホ幅では実用性が薄いため無視 = 判断は作業報告を参照)。
//
// 罫線・グリッドは Excel の「紙面」という扱いで、アプリのテーマ(ダーク
// モード等)によらず白背景・黒文字の固定色にする(デスクトップ版 ADR-0055
// 決定 6 の「外観はライト固定」と同じ考え方をスマホにも適用)。
//
// **シート名・セル値は Log に出さないこと。**

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
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalClipboardManager
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.font.FontStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextDecoration
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import org.json.JSONObject
import uniffi.peercove_mobile.MobileException
import uniffi.peercove_mobile.SheetCellInfo
import uniffi.peercove_mobile.SheetCellWriteArg
import uniffi.peercove_mobile.SheetLayoutEntry
import uniffi.peercove_mobile.SheetMergeInfo
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

// サーバー側は列幅・行高を px(デスクトップ CSS px)で持つ。Android は
// dp 表示のため、指示どおり固定係数で概算する(px ≒ dp * 1/0.75)。
private const val PX_TO_DP = 0.75f

private val SHEET_NUMERIC_RE = Regex("""^-?\d+(\.\d+)?$""")

// 「紙面」の固定配色(ADR-0055 決定 6 のデスクトップ方針をスマホにも適用。
// M6 H-6 判断: ダークモードでも白背景・黒文字を保つ)。
private val SHEET_PAPER_BG = Color(0xFFFFFFFF)
private val SHEET_PAPER_TEXT = Color(0xFF1A1A1A)
private val SHEET_HEADER_BG = Color(0xFFEDEDED)
private val SHEET_HEADER_TEXT = Color(0xFF555555)
private val SHEET_GRIDLINE_COLOR = Color(0xFFD0D0D0)
private val SHEET_BORDER_STRONG = Color(0xFF333333)

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

/** セル書式(M6 H-6)。[SheetCellInfo.format] の JSON(空文字 = 既定)を解析した結果。 */
private data class CellFormatData(
    val bold: Boolean = false,
    val italic: Boolean = false,
    val underline: Boolean = false,
    val strike: Boolean = false,
    val fontSize: Int? = null,
    val color: String? = null,
    val bg: String? = null,
    val align: String? = null,
    val borderTop: Boolean = false,
    val borderBottom: Boolean = false,
    val borderLeft: Boolean = false,
    val borderRight: Boolean = false,
) {
    val hasBorder: Boolean get() = borderTop || borderBottom || borderLeft || borderRight
}

private val DEFAULT_CELL_FORMAT = CellFormatData()

/** [SheetCellInfo.format] の JSON 文字列(org.json で解析。既定・解析失敗は
 * 既定書式)。 */
private fun parseCellFormat(json: String): CellFormatData {
    if (json.isEmpty()) return DEFAULT_CELL_FORMAT
    return try {
        val obj = JSONObject(json)
        CellFormatData(
            bold = obj.optBoolean("bold", false),
            italic = obj.optBoolean("italic", false),
            underline = obj.optBoolean("underline", false),
            strike = obj.optBoolean("strike", false),
            fontSize = if (obj.has("font_size")) obj.optInt("font_size") else null,
            color = if (obj.has("color")) obj.optString("color") else null,
            bg = if (obj.has("bg")) obj.optString("bg") else null,
            align = if (obj.has("align")) obj.optString("align") else null,
            borderTop = obj.optBoolean("border_top", false),
            borderBottom = obj.optBoolean("border_bottom", false),
            borderLeft = obj.optBoolean("border_left", false),
            borderRight = obj.optBoolean("border_right", false),
        )
    } catch (e: Exception) {
        DEFAULT_CELL_FORMAT
    }
}

/** "#rrggbb" → Compose Color(解析失敗は null)。 */
private fun parseHexColor(hex: String?): Color? {
    if (hex.isNullOrEmpty()) return null
    return try {
        Color(android.graphics.Color.parseColor(hex))
    } catch (e: IllegalArgumentException) {
        null
    }
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
    var colWidths by remember { mutableStateOf<List<SheetLayoutEntry>>(emptyList()) }
    var rowHeights by remember { mutableStateOf<List<SheetLayoutEntry>>(emptyList()) }
    var merges by remember { mutableStateOf<List<SheetMergeInfo>>(emptyList()) }
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
            colWidths = emptyList()
            rowHeights = emptyList()
            merges = emptyList()
            return@LaunchedEffect
        }
        try {
            val result = withContext(Dispatchers.IO) { sheetCells(baseDir, slug, id) }
            cells = result.cells
            colWidths = result.colWidths
            rowHeights = result.rowHeights
            merges = result.merges
            offline = result.offline
        } catch (e: MobileException) {
            onNotice(e.message ?: "")
        }
    }

    val readOnly = offline || !supported
    val activeSheet = sheets.firstOrNull { it.id == activeSheetId }
    val cellsByKey = remember(cells) { cells.associateBy { it.row to it.col } }
    val colWidthPx = remember(colWidths) { colWidths.associate { it.idx.toInt() to it.size.toInt() } }
    val rowHeightPx = remember(rowHeights) { rowHeights.associate { it.idx.toInt() to it.size.toInt() } }
    fun colWidthFor(col: Int): Dp = colWidthPx[col]?.let { (it * PX_TO_DP).dp } ?: CELL_WIDTH
    fun rowHeightFor(row: Int): Dp = rowHeightPx[row]?.let { (it * PX_TO_DP).dp } ?: CELL_HEIGHT
    val usedRows = cells.maxOfOrNull { it.row.toInt() + 1 } ?: 0
    val usedCols = cells.maxOfOrNull { it.col.toInt() + 1 } ?: 0
    val mergeUsedRows = merges.maxOfOrNull { (it.row + it.rowSpan).toInt() } ?: 0
    val mergeUsedCols = merges.maxOfOrNull { (it.col + it.colSpan).toInt() } ?: 0
    val displayRows = (maxOf(usedRows, mergeUsedRows) + DISPLAY_MARGIN).coerceIn(MIN_DISPLAY_ROWS, MAX_SHEET_ROWS)
    val displayCols = (maxOf(usedCols, mergeUsedCols) + DISPLAY_MARGIN).coerceIn(MIN_DISPLAY_COLS, MAX_SHEET_COLS)

    // セル結合(ADR-0055 決定 6、M6 H-6)。mergeCovered は結合範囲内の全セル
    // (左上含む)→ その結合。左上セルにスパン分の幅・高さを与え、内包セル
    // はスキップする簡易実装(タップ編集は左上のみ)。
    val mergeCovered = remember(merges) {
        val map = HashMap<Pair<Int, Int>, SheetMergeInfo>()
        merges.forEach { m ->
            for (r in m.row.toInt() until (m.row + m.rowSpan).toInt()) {
                for (c in m.col.toInt() until (m.col + m.colSpan).toInt()) {
                    map[r to c] = m
                }
            }
        }
        map
    }

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
                        // format = null(書式変更なし。値の編集のみ = ADR-0055 の
                        // 範囲確認済み、書式の編集は Android では実装しない)
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
            val gridlines = activeSheet.gridlines
            Column(
                modifier = Modifier
                    .weight(1f)
                    .fillMaxWidth()
                    .padding(top = 4.dp)
                    .background(SHEET_PAPER_BG)
                    .verticalScroll(rememberScrollState())
                    .horizontalScroll(rememberScrollState()),
            ) {
                Row {
                    SheetGridCell(text = "", width = HEADER_COL_WIDTH, height = CELL_HEIGHT, isHeader = true, gridlines = gridlines)
                    for (c in 0 until displayCols) {
                        SheetGridCell(
                            text = sheetColLabel(c),
                            width = colWidthFor(c),
                            height = CELL_HEIGHT,
                            isHeader = true,
                            gridlines = gridlines,
                        )
                    }
                }
                for (r in 0 until displayRows) {
                    val rowHeight = rowHeightFor(r)
                    Row {
                        SheetGridCell(
                            text = (r + 1).toString(),
                            width = HEADER_COL_WIDTH,
                            height = rowHeight,
                            isHeader = true,
                            gridlines = gridlines,
                        )
                        var c = 0
                        while (c < displayCols) {
                            val merge = mergeCovered[r to c]
                            when {
                                merge != null && merge.row.toInt() == r && merge.col.toInt() == c -> {
                                    // 結合の左上セル: スパン分の幅・高さを与える(簡易実装)
                                    val spanEndCol = (merge.col + merge.colSpan).toInt().coerceAtMost(displayCols)
                                    val spanWidth = (merge.col.toInt() until spanEndCol).fold(0.dp) { acc, col ->
                                        acc + colWidthFor(col)
                                    }
                                    val spanEndRow = (merge.row + merge.rowSpan).toInt()
                                    val spanHeight = (merge.row.toInt() until spanEndRow).fold(0.dp) { acc, row ->
                                        acc + rowHeightFor(row)
                                    }
                                    val cell = cellsByKey[r.toUInt() to c.toUInt()]
                                    val value = cell?.value ?: ""
                                    val format = parseCellFormat(cell?.format ?: "")
                                    SheetGridCell(
                                        text = value,
                                        width = spanWidth,
                                        height = spanHeight,
                                        numeric = value.isNotEmpty() && SHEET_NUMERIC_RE.matches(value),
                                        format = format,
                                        gridlines = gridlines,
                                        onClick = {
                                            cellDialog = SheetCellDialogState(
                                                row = r.toUInt(),
                                                col = c.toUInt(),
                                                revision = cell?.revision ?: 0uL,
                                                value = value,
                                            )
                                        },
                                    )
                                    c = spanEndCol
                                }
                                merge != null && merge.row.toInt() == r -> {
                                    // 同じ行の内包セル(左上セルの幅に含まれるためスキップ)
                                    c += 1
                                }
                                merge != null -> {
                                    // 結合範囲内・別の行(縦方向の内包セル): 空欄で埋める(簡易実装)
                                    SheetGridCell(
                                        text = "",
                                        width = colWidthFor(c),
                                        height = rowHeight,
                                        gridlines = gridlines,
                                    )
                                    c += 1
                                }
                                else -> {
                                    val cell = cellsByKey[r.toUInt() to c.toUInt()]
                                    val value = cell?.value ?: ""
                                    val format = parseCellFormat(cell?.format ?: "")
                                    SheetGridCell(
                                        text = value,
                                        width = colWidthFor(c),
                                        height = rowHeight,
                                        numeric = value.isNotEmpty() && SHEET_NUMERIC_RE.matches(value),
                                        format = format,
                                        gridlines = gridlines,
                                        onClick = {
                                            cellDialog = SheetCellDialogState(
                                                row = r.toUInt(),
                                                col = c.toUInt(),
                                                revision = cell?.revision ?: 0uL,
                                                value = value,
                                            )
                                        },
                                    )
                                    c += 1
                                }
                            }
                        }
                    }
                }
                // 横スクロール表の下端の余白
                Spacer(modifier = Modifier.height(24.dp))
            }
        }
    }
}

/** グリッドのセル 1 つ(見出し・データ兼用)。M6 H-6: 書式(太字・斜体・
 * 下線・取り消し線・文字色・背景色・配置・フォントサイズ・罫線)と可変の
 * 幅・高さ(列幅・行高・セル結合)に対応する。目盛線(gridlines)が false
 * なら既定罫線をごく薄く(実質非表示)にする。 */
@Composable
private fun SheetGridCell(
    text: String,
    width: Dp,
    height: Dp,
    isHeader: Boolean = false,
    numeric: Boolean = false,
    format: CellFormatData = DEFAULT_CELL_FORMAT,
    gridlines: Boolean = true,
    onClick: (() -> Unit)? = null,
) {
    val gridColor = if (gridlines) SHEET_GRIDLINE_COLOR else SHEET_GRIDLINE_COLOR.copy(alpha = 0.06f)
    val bg = when {
        isHeader -> SHEET_HEADER_BG
        format.bg != null -> parseHexColor(format.bg) ?: SHEET_PAPER_BG
        else -> SHEET_PAPER_BG
    }
    val textColor = when {
        isHeader -> SHEET_HEADER_TEXT
        format.color != null -> parseHexColor(format.color) ?: SHEET_PAPER_TEXT
        else -> SHEET_PAPER_TEXT
    }
    val decorations = buildList {
        if (format.underline) add(TextDecoration.Underline)
        if (format.strike) add(TextDecoration.LineThrough)
    }
    val alignment = when (format.align) {
        "center" -> Alignment.Center
        "right" -> Alignment.CenterEnd
        "left" -> Alignment.CenterStart
        else -> if (numeric) Alignment.CenterEnd else Alignment.CenterStart
    }
    Box(
        modifier = Modifier
            .width(width)
            .height(height)
            .border(0.5.dp, gridColor)
            .let { m -> if (format.hasBorder) m.drawBehind { drawCellBorders(format) } else m }
            .background(bg)
            .let { m -> if (onClick != null) m.clickable(onClick = onClick) else m }
            .padding(horizontal = 6.dp),
        contentAlignment = alignment,
    ) {
        Text(
            text,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
            fontWeight = if (format.bold) FontWeight.Bold else if (isHeader) FontWeight.Normal else null,
            fontStyle = if (format.italic) FontStyle.Italic else null,
            textDecoration = if (decorations.isNotEmpty()) TextDecoration.combine(decorations) else null,
            fontSize = format.fontSize?.sp ?: if (isHeader) MaterialTheme.typography.labelSmall.fontSize else MaterialTheme.typography.bodySmall.fontSize,
            color = textColor,
        )
    }
}

/** 書式の罫線(上下左右、太め = デスクトップの "2px solid #333333" に合わせる)。 */
private fun androidx.compose.ui.graphics.drawscope.DrawScope.drawCellBorders(format: CellFormatData) {
    val strokeWidth = 2.dp.toPx()
    if (format.borderTop) {
        drawLine(SHEET_BORDER_STRONG, Offset(0f, 0f), Offset(size.width, 0f), strokeWidth)
    }
    if (format.borderBottom) {
        drawLine(SHEET_BORDER_STRONG, Offset(0f, size.height), Offset(size.width, size.height), strokeWidth)
    }
    if (format.borderLeft) {
        drawLine(SHEET_BORDER_STRONG, Offset(0f, 0f), Offset(0f, size.height), strokeWidth)
    }
    if (format.borderRight) {
        drawLine(SHEET_BORDER_STRONG, Offset(size.width, 0f), Offset(size.width, size.height), strokeWidth)
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
