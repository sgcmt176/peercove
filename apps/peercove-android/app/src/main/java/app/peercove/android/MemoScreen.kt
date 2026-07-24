package app.peercove.android

// 個人メモ(M5 F-1、ADR-0049)。ストレージはデスクトップと同じ Rust の
// peercove-memo(SQLite)。この画面は表示と SAF(txt の入出力)だけを担う。
// 一覧(検索・絞り込み・フォルダー・タグ)→ タップで編集(自動保存 +
// Markdown プレビュー)。ゴミ箱のメモは読み取り専用で復元・完全削除のみ。

import android.net.Uri
import android.provider.OpenableColumns
import androidx.activity.compose.BackHandler
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material.icons.filled.Add
import androidx.compose.material.icons.filled.Delete
import androidx.compose.material.icons.filled.Edit
import androidx.compose.material.icons.filled.Folder
import androidx.compose.material.icons.filled.MoreVert
import androidx.compose.material.icons.filled.PushPin
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Card
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.FilterChip
import androidx.compose.material3.FloatingActionButton
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.RadioButton
import androidx.compose.material3.Scaffold
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
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import java.text.SimpleDateFormat
import java.util.Date
import java.util.Locale
import uniffi.peercove_mobile.MemoDetailInfo
import uniffi.peercove_mobile.MemoFolderInfo
import uniffi.peercove_mobile.MemoListResult
import uniffi.peercove_mobile.MemoScopeArg
import uniffi.peercove_mobile.MemoSortArg
import uniffi.peercove_mobile.MemoSummaryInfo
import uniffi.peercove_mobile.MobileException
import uniffi.peercove_mobile.NetworkInfo
import uniffi.peercove_mobile.listNetworks
import uniffi.peercove_mobile.memoCreate
import uniffi.peercove_mobile.memoDeleteForever
import uniffi.peercove_mobile.memoDuplicate
import uniffi.peercove_mobile.memoEmptyTrash
import uniffi.peercove_mobile.memoExportName
import uniffi.peercove_mobile.memoFolderCreate
import uniffi.peercove_mobile.memoFolderDelete
import uniffi.peercove_mobile.memoFolderRename
import uniffi.peercove_mobile.memoBacklinks
import uniffi.peercove_mobile.memoGet
import uniffi.peercove_mobile.memoList
import uniffi.peercove_mobile.memoResolveTitles
import uniffi.peercove_mobile.memoRestore
import uniffi.peercove_mobile.memoSaveText
import uniffi.peercove_mobile.memoSetFlags
import uniffi.peercove_mobile.memoSetFolder
import uniffi.peercove_mobile.memoSetTags
import uniffi.peercove_mobile.memoTrash
import uniffi.peercove_mobile.sharedMemoCreate

private val dateFmt = SimpleDateFormat("yyyy/MM/dd HH:mm", Locale.getDefault())

@Composable
fun MemoScreen(
    onBack: () -> Unit,
    onNotice: (String) -> Unit,
    /** リマインダー通知タップから直接開くメモ(M5 F-5 Stage 5、ADR-0052 決定 6)。 */
    initialMemoId: String? = null,
) {
    val context = LocalContext.current
    val baseDir = context.filesDir.absolutePath
    val scope = rememberCoroutineScope()

    var filter by remember { mutableStateOf(MemoScopeArg.ACTIVE) }
    var sort by remember { mutableStateOf(MemoSortArg.UPDATED) }
    var search by remember { mutableStateOf("") }
    var folderId by remember { mutableStateOf<String?>(null) }
    var tag by remember { mutableStateOf<String?>(null) }
    var list by remember { mutableStateOf<MemoListResult?>(null) }
    var editingId by remember { mutableStateOf(initialMemoId) }
    var refreshTick by remember { mutableIntStateOf(0) }
    var showFolders by remember { mutableStateOf(false) }
    var confirmEmptyTrash by remember { mutableStateOf(false) }

    val importFailed = stringResource(R.string.memo_import_failed)
    val importedFmt = stringResource(R.string.memo_imported)

    fun refresh() {
        refreshTick++
    }

    LaunchedEffect(filter, sort, search, folderId, tag, refreshTick) {
        try {
            list = withContext(Dispatchers.IO) {
                memoList(
                    baseDir,
                    filter,
                    if (filter == MemoScopeArg.TRASH) null else folderId,
                    tag,
                    search.trim().ifEmpty { null },
                    sort,
                )
            }
        } catch (e: MobileException) {
            onNotice(e.message ?: "")
        }
    }

    // txt の取り込み(SAF、複数可)。ファイル名がタイトル・本文がメモ本文
    val importLauncher = rememberLauncherForActivityResult(
        ActivityResultContracts.OpenMultipleDocuments(),
    ) { uris ->
        if (uris.isEmpty()) return@rememberLauncherForActivityResult
        scope.launch {
            var imported = 0
            try {
                withContext(Dispatchers.IO) {
                    for (uri in uris) {
                        val body = context.contentResolver.openInputStream(uri)
                            ?.use { it.readBytes().toString(Charsets.UTF_8) } ?: continue
                        val title = displayName(context, uri)
                            ?.removeSuffix(".txt") ?: ""
                        memoCreate(baseDir, title, body, folderId)
                        imported++
                    }
                }
                onNotice(importedFmt.format(imported))
            } catch (e: Exception) {
                onNotice(e.message ?: importFailed)
            }
            refresh()
        }
    }

    val editing = editingId
    if (editing != null) {
        MemoEditor(
            baseDir = baseDir,
            id = editing,
            folders = list?.folders ?: emptyList(),
            onClose = {
                editingId = null
                refresh()
            },
            // メモ間リンク(ADR-0052 決定 2)クリックで同じ画面内の別メモへ切替
            onOpenMemo = { editingId = it },
            onNotice = onNotice,
        )
        return
    }

    BackHandler { onBack() }

    if (confirmEmptyTrash) {
        AlertDialog(
            onDismissRequest = { confirmEmptyTrash = false },
            title = { Text(stringResource(R.string.memo_empty_trash_action)) },
            text = { Text(stringResource(R.string.memo_empty_trash_confirm)) },
            confirmButton = {
                TextButton(onClick = {
                    confirmEmptyTrash = false
                    scope.launch {
                        try {
                            withContext(Dispatchers.IO) { memoEmptyTrash(baseDir) }
                        } catch (e: MobileException) {
                            onNotice(e.message ?: "")
                        }
                        refresh()
                    }
                }) { Text(stringResource(R.string.memo_delete_forever)) }
            },
            dismissButton = {
                TextButton(onClick = { confirmEmptyTrash = false }) {
                    Text(stringResource(R.string.action_cancel))
                }
            },
        )
    }

    if (showFolders) {
        FolderDialog(
            baseDir = baseDir,
            folders = list?.folders ?: emptyList(),
            selected = folderId,
            onSelect = {
                folderId = it
                showFolders = false
            },
            onChanged = { refresh() },
            onNotice = onNotice,
            onDismiss = { showFolders = false },
        )
    }

    Scaffold(
        floatingActionButton = {
            if (filter != MemoScopeArg.TRASH) {
                FloatingActionButton(onClick = {
                    scope.launch {
                        try {
                            val memo = withContext(Dispatchers.IO) {
                                memoCreate(baseDir, "", "", folderId)
                            }
                            editingId = memo.id
                        } catch (e: MobileException) {
                            onNotice(e.message ?: "")
                        }
                    }
                }) {
                    Icon(Icons.Filled.Add, contentDescription = stringResource(R.string.memo_new))
                }
            }
        },
    ) { padding ->
        Column(modifier = Modifier.fillMaxSize().padding(padding).padding(horizontal = 16.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                IconButton(onClick = onBack) {
                    Icon(
                        Icons.AutoMirrored.Filled.ArrowBack,
                        contentDescription = stringResource(R.string.action_back),
                    )
                }
                Text(
                    stringResource(R.string.memo_title),
                    style = MaterialTheme.typography.headlineSmall,
                    modifier = Modifier.weight(1f),
                )
                TextButton(onClick = { importLauncher.launch(arrayOf("text/plain")) }) {
                    Text(stringResource(R.string.memo_import))
                }
                SortMenu(sort = sort, onSelect = { sort = it })
            }

            OutlinedTextField(
                value = search,
                onValueChange = { search = it },
                modifier = Modifier.fillMaxWidth(),
                singleLine = true,
                label = { Text(stringResource(R.string.memo_search)) },
            )
            Spacer(modifier = Modifier.height(4.dp))

            Row(
                modifier = Modifier.horizontalScroll(rememberScrollState()),
                horizontalArrangement = Arrangement.spacedBy(6.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                FilterChip(
                    selected = filter == MemoScopeArg.ACTIVE,
                    onClick = { filter = MemoScopeArg.ACTIVE },
                    label = { Text(stringResource(R.string.memo_scope_active)) },
                )
                FilterChip(
                    selected = filter == MemoScopeArg.ARCHIVED,
                    onClick = { filter = MemoScopeArg.ARCHIVED },
                    label = { Text(stringResource(R.string.memo_scope_archived)) },
                )
                FilterChip(
                    selected = filter == MemoScopeArg.TRASH,
                    onClick = { filter = MemoScopeArg.TRASH },
                    label = { Text(stringResource(R.string.memo_scope_trash)) },
                )
                if (filter != MemoScopeArg.TRASH) {
                    FilterChip(
                        selected = folderId != null,
                        onClick = { showFolders = true },
                        label = {
                            val name = list?.folders?.firstOrNull { it.id == folderId }?.name
                            Text("📁 " + (name ?: stringResource(R.string.memo_folder_all)))
                        },
                    )
                }
            }

            val tags = list?.tags ?: emptyList()
            if (tags.isNotEmpty() && filter != MemoScopeArg.TRASH) {
                Row(
                    modifier = Modifier.horizontalScroll(rememberScrollState()),
                    horizontalArrangement = Arrangement.spacedBy(6.dp),
                ) {
                    tags.forEach { entry ->
                        FilterChip(
                            selected = tag == entry.tag,
                            onClick = { tag = if (tag == entry.tag) null else entry.tag },
                            label = { Text("#${entry.tag} ${entry.count}") },
                        )
                    }
                }
            }

            if (filter == MemoScopeArg.TRASH && (list?.memos?.isNotEmpty() == true)) {
                TextButton(onClick = { confirmEmptyTrash = true }) {
                    Text(
                        stringResource(R.string.memo_empty_trash_action),
                        color = MaterialTheme.colorScheme.error,
                    )
                }
            }

            val memos = list?.memos ?: emptyList()
            if (memos.isEmpty()) {
                Spacer(modifier = Modifier.height(24.dp))
                Text(
                    stringResource(
                        if (filter == MemoScopeArg.TRASH) R.string.memo_empty_trash
                        else R.string.memo_empty,
                    ),
                    style = MaterialTheme.typography.bodyMedium,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
            LazyColumn(
                verticalArrangement = Arrangement.spacedBy(8.dp),
                modifier = Modifier.weight(1f).padding(vertical = 8.dp),
            ) {
                items(memos, key = { it.id }) { memo ->
                    MemoCard(memo = memo, onClick = { editingId = memo.id })
                }
            }
        }
    }
}

@Composable
private fun SortMenu(sort: MemoSortArg, onSelect: (MemoSortArg) -> Unit) {
    var open by remember { mutableStateOf(false) }
    IconButton(onClick = { open = true }) {
        Icon(Icons.Filled.MoreVert, contentDescription = stringResource(R.string.memo_sort))
    }
    DropdownMenu(expanded = open, onDismissRequest = { open = false }) {
        listOf(
            MemoSortArg.UPDATED to R.string.memo_sort_updated,
            MemoSortArg.CREATED to R.string.memo_sort_created,
            MemoSortArg.TITLE to R.string.memo_sort_title,
        ).forEach { (value, label) ->
            DropdownMenuItem(
                text = { Text((if (sort == value) "✓ " else "") + stringResource(label)) },
                onClick = {
                    onSelect(value)
                    open = false
                },
            )
        }
    }
}

@Composable
private fun MemoCard(memo: MemoSummaryInfo, onClick: () -> Unit) {
    Card(onClick = onClick, modifier = Modifier.fillMaxWidth()) {
        Column(modifier = Modifier.padding(12.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                if (memo.pinned) {
                    Icon(
                        Icons.Filled.PushPin,
                        contentDescription = stringResource(R.string.memo_pin),
                        modifier = Modifier.width(16.dp),
                        tint = MaterialTheme.colorScheme.primary,
                    )
                    Spacer(modifier = Modifier.width(4.dp))
                }
                Text(
                    memo.title.ifEmpty { stringResource(R.string.memo_untitled) },
                    style = MaterialTheme.typography.titleMedium,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
            }
            if (memo.excerpt.isNotEmpty()) {
                Text(
                    memo.excerpt,
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    maxLines = 2,
                    overflow = TextOverflow.Ellipsis,
                )
            }
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                val at = memo.deletedAt ?: memo.updatedAt
                Text(
                    dateFmt.format(Date(at.toLong())),
                    style = MaterialTheme.typography.labelSmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
                if (memo.checklistTotal > 0u) {
                    Text(
                        stringResource(
                            R.string.memo_checklist,
                            memo.checklistDone.toInt(),
                            memo.checklistTotal.toInt(),
                        ),
                        style = MaterialTheme.typography.labelSmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
                memo.tags.take(3).forEach {
                    Text(
                        "#$it",
                        style = MaterialTheme.typography.labelSmall,
                        color = MaterialTheme.colorScheme.primary,
                    )
                }
            }
        }
    }
}

/** フォルダーの絞り込み + 管理(作成・改名・削除)。 */
@Composable
private fun FolderDialog(
    baseDir: String,
    folders: List<MemoFolderInfo>,
    selected: String?,
    onSelect: (String?) -> Unit,
    onChanged: () -> Unit,
    onNotice: (String) -> Unit,
    onDismiss: () -> Unit,
) {
    val scope = rememberCoroutineScope()
    var newName by remember { mutableStateOf("") }
    var renameTarget by remember { mutableStateOf<MemoFolderInfo?>(null) }
    var renameValue by remember { mutableStateOf("") }
    var deleteTarget by remember { mutableStateOf<MemoFolderInfo?>(null) }

    deleteTarget?.let { target ->
        AlertDialog(
            onDismissRequest = { deleteTarget = null },
            title = { Text(stringResource(R.string.action_remove)) },
            text = { Text(stringResource(R.string.memo_folder_delete_confirm, target.name)) },
            confirmButton = {
                TextButton(onClick = {
                    deleteTarget = null
                    scope.launch {
                        try {
                            withContext(Dispatchers.IO) { memoFolderDelete(baseDir, target.id) }
                            if (selected == target.id) onSelect(null)
                            onChanged()
                        } catch (e: MobileException) {
                            onNotice(e.message ?: "")
                        }
                    }
                }) { Text(stringResource(R.string.action_remove)) }
            },
            dismissButton = {
                TextButton(onClick = { deleteTarget = null }) {
                    Text(stringResource(R.string.action_cancel))
                }
            },
        )
        return
    }

    renameTarget?.let { target ->
        AlertDialog(
            onDismissRequest = { renameTarget = null },
            title = { Text(stringResource(R.string.memo_folder_rename)) },
            text = {
                OutlinedTextField(
                    value = renameValue,
                    onValueChange = { renameValue = it },
                    singleLine = true,
                    label = { Text(stringResource(R.string.memo_folder_name)) },
                )
            },
            confirmButton = {
                TextButton(
                    enabled = renameValue.isNotBlank(),
                    onClick = {
                        renameTarget = null
                        scope.launch {
                            try {
                                withContext(Dispatchers.IO) {
                                    memoFolderRename(baseDir, target.id, renameValue)
                                }
                                onChanged()
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
        return
    }

    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(stringResource(R.string.memo_folders)) },
        text = {
            Column {
                Row(
                    verticalAlignment = Alignment.CenterVertically,
                    modifier = Modifier.fillMaxWidth(),
                ) {
                    RadioButton(selected = selected == null, onClick = { onSelect(null) })
                    Text(stringResource(R.string.memo_folder_all))
                }
                folders.forEach { folder ->
                    Row(
                        verticalAlignment = Alignment.CenterVertically,
                        modifier = Modifier.fillMaxWidth(),
                    ) {
                        RadioButton(
                            selected = selected == folder.id,
                            onClick = { onSelect(folder.id) },
                        )
                        Text(
                            "${folder.name}(${folder.memoCount})",
                            modifier = Modifier.weight(1f),
                            maxLines = 1,
                            overflow = TextOverflow.Ellipsis,
                        )
                        IconButton(onClick = {
                            renameValue = folder.name
                            renameTarget = folder
                        }) {
                            Icon(
                                Icons.Filled.Edit,
                                contentDescription = stringResource(R.string.memo_folder_rename),
                            )
                        }
                        IconButton(onClick = { deleteTarget = folder }) {
                            Icon(
                                Icons.Filled.Delete,
                                contentDescription = stringResource(R.string.action_remove),
                            )
                        }
                    }
                }
                Row(verticalAlignment = Alignment.CenterVertically) {
                    OutlinedTextField(
                        value = newName,
                        onValueChange = { newName = it },
                        singleLine = true,
                        label = { Text(stringResource(R.string.memo_folder_new)) },
                        modifier = Modifier.weight(1f),
                    )
                    TextButton(
                        enabled = newName.isNotBlank(),
                        onClick = {
                            scope.launch {
                                try {
                                    withContext(Dispatchers.IO) {
                                        memoFolderCreate(baseDir, newName)
                                    }
                                    newName = ""
                                    onChanged()
                                } catch (e: MobileException) {
                                    onNotice(e.message ?: "")
                                }
                            }
                        },
                    ) { Text(stringResource(R.string.memo_folder_new)) }
                }
            }
        },
        confirmButton = {
            TextButton(onClick = onDismiss) { Text(stringResource(R.string.action_close)) }
        },
    )
}

@Composable
private fun MemoEditor(
    baseDir: String,
    id: String,
    folders: List<MemoFolderInfo>,
    onClose: () -> Unit,
    /** メモ間リンク(ADR-0052 決定 2)クリックで同じ画面内の別メモへ切替。 */
    onOpenMemo: (String) -> Unit,
    onNotice: (String) -> Unit,
) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    var detail by remember { mutableStateOf<MemoDetailInfo?>(null) }
    var title by remember { mutableStateOf("") }
    var body by remember { mutableStateOf("") }
    // 保存済みの内容(自動保存の判定用)。null = 読み込み前
    var saved by remember { mutableStateOf<Pair<String, String>?>(null) }
    var saveFailed by remember { mutableStateOf(false) }
    var preview by remember { mutableStateOf(false) }
    var menuOpen by remember { mutableStateOf(false) }
    var folderMenuOpen by remember { mutableStateOf(false) }
    var tagsDialog by remember { mutableStateOf(false) }
    var confirmDelete by remember { mutableStateOf(false) }
    var copyToSharedDialog by remember { mutableStateOf(false) }
    // 共有メモへコピーの先のネットワーク一覧(0 件ならメニュー自体を出さない)
    val networks: List<NetworkInfo> = remember { listNetworks(baseDir) }
    // メモ間リンク(ADR-0052 決定 2): タイトル → memo_id(見つかったものだけ)
    var wikiLinks by remember { mutableStateOf<Map<String, String>>(emptyMap()) }
    var backlinks by remember { mutableStateOf<List<MemoSummaryInfo>>(emptyList()) }
    // ⏰ リマインダーのメニュー・アイコンはここから撤去(ADR-0055 決定 3)。
    // Reminder.kt の基盤(fetchReminders/applyReminder/clearReminder/
    // pickReminderDateTime)自体はスケジュールの予定リマインダー(H-3)で
    // 流用するため残してある。既に設定済みのリマインダーは(このメニューを
    // 経由せず)引き続き発火する。

    val exportDone = stringResource(R.string.memo_export_done)
    val exportFailed = stringResource(R.string.memo_export_failed)
    val copiedToShared = stringResource(R.string.memo_copied_to_shared)
    val wikilinkMissing = stringResource(R.string.memo_wikilink_missing)
    val inTrash = detail?.deletedAt != null

    suspend fun refreshBacklinks() {
        try {
            backlinks = withContext(Dispatchers.IO) { memoBacklinks(baseDir, id) }
        } catch (e: MobileException) {
            backlinks = emptyList()
        }
    }

    LaunchedEffect(id) {
        try {
            val memo = withContext(Dispatchers.IO) { memoGet(baseDir, id) }
            detail = memo
            title = memo.title
            body = memo.body
            saved = memo.title to memo.body
            if (memo.deletedAt == null && memo.body.isNotEmpty()) {
                // 既存メモはプレビューから開く(閲覧が主目的のことが多い)
                preview = true
            }
        } catch (e: MobileException) {
            onNotice(e.message ?: "")
            onClose()
            return@LaunchedEffect
        }
        refreshBacklinks()
    }

    // メモ間リンクの解決(本文のデバウンス、ADR-0052 決定 2)
    LaunchedEffect(body) {
        val titles = extractWikiTitles(body)
        if (titles.isEmpty()) {
            wikiLinks = emptyMap()
            return@LaunchedEffect
        }
        delay(400)
        wikiLinks = try {
            withContext(Dispatchers.IO) { memoResolveTitles(baseDir, titles) }
        } catch (e: MobileException) {
            emptyMap()
        }
    }

    // 自動保存(600ms デバウンス)。ゴミ箱のメモは読み取り専用
    LaunchedEffect(title, body) {
        val base = saved ?: return@LaunchedEffect
        if (inTrash || (base.first == title && base.second == body)) return@LaunchedEffect
        delay(600)
        try {
            withContext(Dispatchers.IO) { memoSaveText(baseDir, id, title, body) }
            saved = title to body
            saveFailed = false
            // タイトル変更でバックリンクの対象が変わりうる
            refreshBacklinks()
        } catch (e: MobileException) {
            saveFailed = true
            onNotice(e.message ?: "")
        }
    }

    fun flushAndClose() {
        val base = saved
        if (base != null && !inTrash && (base.first != title || base.second != body)) {
            scope.launch {
                try {
                    withContext(Dispatchers.IO) { memoSaveText(baseDir, id, title, body) }
                } catch (e: MobileException) {
                    onNotice(e.message ?: "")
                }
                onClose()
            }
        } else {
            onClose()
        }
    }

    /** メモ間リンクのタップ: 未保存の変更を保存してから遷移する。 */
    fun openLinkedMemo(targetId: String) {
        val base = saved
        if (base != null && !inTrash && (base.first != title || base.second != body)) {
            scope.launch {
                try {
                    withContext(Dispatchers.IO) { memoSaveText(baseDir, id, title, body) }
                } catch (e: MobileException) {
                    onNotice(e.message ?: "")
                }
                onOpenMemo(targetId)
            }
        } else {
            onOpenMemo(targetId)
        }
    }

    BackHandler { flushAndClose() }

    val exportLauncher = rememberLauncherForActivityResult(
        ActivityResultContracts.CreateDocument("text/plain"),
    ) { uri ->
        if (uri == null) return@rememberLauncherForActivityResult
        scope.launch {
            try {
                withContext(Dispatchers.IO) {
                    context.contentResolver.openOutputStream(uri)?.use {
                        it.write(body.toByteArray(Charsets.UTF_8))
                    }
                }
                onNotice(exportDone)
            } catch (e: Exception) {
                onNotice(e.message ?: exportFailed)
            }
        }
    }

    if (confirmDelete) {
        AlertDialog(
            onDismissRequest = { confirmDelete = false },
            title = { Text(stringResource(R.string.memo_delete_forever)) },
            text = { Text(stringResource(R.string.memo_delete_forever_confirm)) },
            confirmButton = {
                TextButton(onClick = {
                    confirmDelete = false
                    scope.launch {
                        try {
                            withContext(Dispatchers.IO) { memoDeleteForever(baseDir, id) }
                            onClose()
                        } catch (e: MobileException) {
                            onNotice(e.message ?: "")
                        }
                    }
                }) { Text(stringResource(R.string.memo_delete_forever)) }
            },
            dismissButton = {
                TextButton(onClick = { confirmDelete = false }) {
                    Text(stringResource(R.string.action_cancel))
                }
            },
        )
    }

    if (tagsDialog) {
        var tagsValue by remember {
            mutableStateOf(detail?.tags?.joinToString(", ") ?: "")
        }
        AlertDialog(
            onDismissRequest = { tagsDialog = false },
            title = { Text(stringResource(R.string.memo_tags)) },
            text = {
                OutlinedTextField(
                    value = tagsValue,
                    onValueChange = { tagsValue = it },
                    singleLine = true,
                    label = { Text(stringResource(R.string.memo_tags_hint)) },
                )
            },
            confirmButton = {
                TextButton(onClick = {
                    tagsDialog = false
                    scope.launch {
                        try {
                            detail = withContext(Dispatchers.IO) {
                                memoSetTags(
                                    baseDir,
                                    id,
                                    tagsValue.split(',', '、')
                                        .map { it.trim() }
                                        .filter { it.isNotEmpty() },
                                )
                            }
                        } catch (e: MobileException) {
                            onNotice(e.message ?: "")
                        }
                    }
                }) { Text(stringResource(R.string.action_save)) }
            },
            dismissButton = {
                TextButton(onClick = { tagsDialog = false }) {
                    Text(stringResource(R.string.action_cancel))
                }
            },
        )
    }

    if (copyToSharedDialog) {
        AlertDialog(
            onDismissRequest = { copyToSharedDialog = false },
            title = { Text(stringResource(R.string.memo_copy_to_shared_choose)) },
            text = {
                Column(modifier = Modifier.verticalScroll(rememberScrollState())) {
                    networks.forEach { net ->
                        Text(
                            net.name,
                            style = MaterialTheme.typography.bodyLarge,
                            modifier = Modifier
                                .fillMaxWidth()
                                .clickable {
                                    copyToSharedDialog = false
                                    scope.launch {
                                        try {
                                            withContext(Dispatchers.IO) {
                                                sharedMemoCreate(net.slug, title, body)
                                            }
                                            onNotice(copiedToShared)
                                        } catch (e: MobileException) {
                                            onNotice(e.message ?: "")
                                        }
                                    }
                                }
                                .padding(vertical = 12.dp),
                        )
                    }
                }
            },
            confirmButton = {
                TextButton(onClick = { copyToSharedDialog = false }) {
                    Text(stringResource(R.string.action_cancel))
                }
            },
        )
    }

    Column(modifier = Modifier.fillMaxSize().padding(horizontal = 16.dp)) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            IconButton(onClick = { flushAndClose() }) {
                Icon(
                    Icons.AutoMirrored.Filled.ArrowBack,
                    contentDescription = stringResource(R.string.action_back),
                )
            }
            Text(
                when {
                    inTrash -> stringResource(R.string.memo_in_trash)
                    saveFailed -> stringResource(R.string.memo_save_failed)
                    saved != null && saved == (title to body) ->
                        stringResource(R.string.memo_saved)
                    else -> stringResource(R.string.memo_saving)
                },
                style = MaterialTheme.typography.labelMedium,
                color = if (saveFailed) {
                    MaterialTheme.colorScheme.error
                } else {
                    MaterialTheme.colorScheme.onSurfaceVariant
                },
                modifier = Modifier.weight(1f),
            )
            if (!inTrash) {
                FilterChip(
                    selected = !preview,
                    onClick = { preview = false },
                    label = { Text(stringResource(R.string.memo_edit)) },
                )
                Spacer(modifier = Modifier.width(6.dp))
                FilterChip(
                    selected = preview,
                    onClick = { preview = true },
                    label = { Text(stringResource(R.string.memo_preview)) },
                )
                IconButton(onClick = { menuOpen = true }) {
                    Icon(
                        Icons.Filled.MoreVert,
                        contentDescription = stringResource(R.string.memo_menu),
                    )
                }
                DropdownMenu(expanded = menuOpen, onDismissRequest = { menuOpen = false }) {
                    val memo = detail
                    DropdownMenuItem(
                        text = {
                            Text(
                                stringResource(
                                    if (memo?.pinned == true) R.string.memo_unpin
                                    else R.string.memo_pin,
                                ),
                            )
                        },
                        onClick = {
                            menuOpen = false
                            scope.launch {
                                try {
                                    detail = withContext(Dispatchers.IO) {
                                        memoSetFlags(baseDir, id, memo?.pinned != true, null)
                                    }
                                } catch (e: MobileException) {
                                    onNotice(e.message ?: "")
                                }
                            }
                        },
                    )
                    DropdownMenuItem(
                        text = {
                            Text(
                                stringResource(
                                    if (memo?.archived == true) R.string.memo_unarchive
                                    else R.string.memo_archive,
                                ),
                            )
                        },
                        onClick = {
                            menuOpen = false
                            scope.launch {
                                try {
                                    detail = withContext(Dispatchers.IO) {
                                        memoSetFlags(baseDir, id, null, memo?.archived != true)
                                    }
                                } catch (e: MobileException) {
                                    onNotice(e.message ?: "")
                                }
                            }
                        },
                    )
                    DropdownMenuItem(
                        text = { Text(stringResource(R.string.memo_move_folder)) },
                        leadingIcon = { Icon(Icons.Filled.Folder, contentDescription = null) },
                        onClick = {
                            menuOpen = false
                            folderMenuOpen = true
                        },
                    )
                    DropdownMenuItem(
                        text = { Text(stringResource(R.string.memo_tags)) },
                        onClick = {
                            menuOpen = false
                            tagsDialog = true
                        },
                    )
                    DropdownMenuItem(
                        text = { Text(stringResource(R.string.memo_duplicate)) },
                        onClick = {
                            menuOpen = false
                            scope.launch {
                                try {
                                    withContext(Dispatchers.IO) { memoDuplicate(baseDir, id) }
                                    onClose()
                                } catch (e: MobileException) {
                                    onNotice(e.message ?: "")
                                }
                            }
                        },
                    )
                    // ⏰ リマインダーの設定・解除メニューはここから撤去(ADR-0055 決定 3)。
                    DropdownMenuItem(
                        text = { Text(stringResource(R.string.memo_export)) },
                        onClick = {
                            menuOpen = false
                            exportLauncher.launch(memoExportName(title) + ".txt")
                        },
                    )
                    if (networks.isNotEmpty()) {
                        DropdownMenuItem(
                            text = { Text(stringResource(R.string.memo_copy_to_shared)) },
                            onClick = {
                                menuOpen = false
                                copyToSharedDialog = true
                            },
                        )
                    }
                    DropdownMenuItem(
                        text = {
                            Text(
                                stringResource(R.string.memo_trash_action),
                                color = MaterialTheme.colorScheme.error,
                            )
                        },
                        onClick = {
                            menuOpen = false
                            scope.launch {
                                try {
                                    withContext(Dispatchers.IO) { memoTrash(baseDir, id) }
                                    onClose()
                                } catch (e: MobileException) {
                                    onNotice(e.message ?: "")
                                }
                            }
                        },
                    )
                }
                DropdownMenu(
                    expanded = folderMenuOpen,
                    onDismissRequest = { folderMenuOpen = false },
                ) {
                    DropdownMenuItem(
                        text = { Text(stringResource(R.string.memo_folder_none)) },
                        onClick = {
                            folderMenuOpen = false
                            scope.launch {
                                try {
                                    detail = withContext(Dispatchers.IO) {
                                        memoSetFolder(baseDir, id, null)
                                    }
                                } catch (e: MobileException) {
                                    onNotice(e.message ?: "")
                                }
                            }
                        },
                    )
                    folders.forEach { folder ->
                        DropdownMenuItem(
                            text = {
                                Text(
                                    (if (detail?.folderId == folder.id) "✓ " else "") +
                                        folder.name,
                                )
                            },
                            onClick = {
                                folderMenuOpen = false
                                scope.launch {
                                    try {
                                        detail = withContext(Dispatchers.IO) {
                                            memoSetFolder(baseDir, id, folder.id)
                                        }
                                    } catch (e: MobileException) {
                                        onNotice(e.message ?: "")
                                    }
                                }
                            },
                        )
                    }
                }
            }
            if (inTrash) {
                TextButton(onClick = {
                    scope.launch {
                        try {
                            withContext(Dispatchers.IO) { memoRestore(baseDir, id) }
                            onClose()
                        } catch (e: MobileException) {
                            onNotice(e.message ?: "")
                        }
                    }
                }) { Text(stringResource(R.string.memo_restore)) }
                TextButton(onClick = { confirmDelete = true }) {
                    Text(
                        stringResource(R.string.memo_delete_forever),
                        color = MaterialTheme.colorScheme.error,
                    )
                }
            }
        }

        OutlinedTextField(
            value = title,
            onValueChange = { title = it },
            modifier = Modifier.fillMaxWidth(),
            singleLine = true,
            readOnly = inTrash,
            label = { Text(stringResource(R.string.memo_title_hint)) },
        )
        Spacer(modifier = Modifier.height(8.dp))

        if (preview || inTrash) {
            Column(
                modifier = Modifier
                    .weight(1f)
                    .fillMaxWidth()
                    .verticalScroll(rememberScrollState())
                    .padding(vertical = 8.dp),
            ) {
                MarkdownPreview(
                    body,
                    resolvedTitles = wikiLinks.keys,
                    onWikiLink = { linkedTitle ->
                        val targetId = wikiLinks[linkedTitle]
                        if (targetId != null) {
                            openLinkedMemo(targetId)
                        } else {
                            onNotice(wikilinkMissing)
                        }
                    },
                )
                BacklinksSection(
                    backlinks = backlinks,
                    idOf = { it.id },
                    titleOf = { it.title },
                    onOpen = { openLinkedMemo(it) },
                )
            }
        } else {
            OutlinedTextField(
                value = body,
                onValueChange = { body = it },
                modifier = Modifier.weight(1f).fillMaxWidth(),
                label = { Text(stringResource(R.string.memo_body_hint)) },
            )
        }
        Spacer(modifier = Modifier.height(8.dp))
    }
}

/**
 * メモ間リンクの解決対象タイトルの抽出(前後空白除去、重複なし)。
 * `SharedMemoScreen.kt` からも使う(共有メモも同じ記法、ADR-0052 決定 2)。
 */
fun extractWikiTitles(body: String): List<String> {
    val re = Regex("\\[\\[([^\\[\\]]+)\\]\\]")
    return re.findAll(body)
        .mapNotNull { it.groupValues.getOrNull(1)?.trim()?.ifEmpty { null } }
        .distinct()
        .toList()
}

/**
 * バックリンク欄(ADR-0052 決定 2)。0 件なら何も表示しない。個人メモ
 * (`MemoSummaryInfo`)・共有メモ(`SharedMemoSummaryInfo`)の両方の要約型に
 * 共通の親が無いため、id/title の取り出し方をラムダで渡す形にしてある
 * (`SharedMemoScreen.kt` からも使う)。
 */
@Composable
fun <T> BacklinksSection(
    backlinks: List<T>,
    idOf: (T) -> String,
    titleOf: (T) -> String,
    onOpen: (String) -> Unit,
) {
    if (backlinks.isEmpty()) return
    Spacer(modifier = Modifier.height(8.dp))
    HorizontalDivider()
    Spacer(modifier = Modifier.height(4.dp))
    Text(
        stringResource(R.string.memo_backlinks_title, backlinks.size),
        style = MaterialTheme.typography.labelLarge,
        color = MaterialTheme.colorScheme.onSurfaceVariant,
    )
    Spacer(modifier = Modifier.height(4.dp))
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .horizontalScroll(rememberScrollState()),
        horizontalArrangement = Arrangement.spacedBy(6.dp),
    ) {
        backlinks.forEach { memo ->
            FilterChip(
                selected = false,
                onClick = { onOpen(idOf(memo)) },
                label = {
                    Text(titleOf(memo).ifEmpty { stringResource(R.string.memo_untitled) })
                },
            )
        }
    }
}

/** SAF の URI から表示名(ファイル名)を引く。 */
private fun displayName(context: android.content.Context, uri: Uri): String? {
    context.contentResolver.query(uri, null, null, null, null)?.use { cursor ->
        val index = cursor.getColumnIndex(OpenableColumns.DISPLAY_NAME)
        if (index >= 0 && cursor.moveToFirst()) {
            return cursor.getString(index)
        }
    }
    return uri.lastPathSegment
}
