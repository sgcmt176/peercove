package app.peercove.android

// 共有メモ(M5 F-2、ADR-0049)。読み取りはキャッシュ(オフラインでも閲覧可)、
// 変更はホストへ届き、権限・単一編集者ロック・リビジョン(CAS)はホスト正本で
// 判定される。閲覧中は世代ポーリングでリアルタイムに追随する。

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.horizontalScroll
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
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material.icons.filled.Add
import androidx.compose.material.icons.filled.Close
import androidx.compose.material.icons.filled.History
import androidx.compose.material.icons.filled.MoreVert
import androidx.compose.material.icons.filled.Save
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
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import java.text.SimpleDateFormat
import java.util.Date
import java.util.Locale
import uniffi.peercove_mobile.DiffLineInfo
import uniffi.peercove_mobile.MobileException
import uniffi.peercove_mobile.SharedMemoCommentInfo
import uniffi.peercove_mobile.SharedMemoDetailInfo
import uniffi.peercove_mobile.SharedMemoHistoryDetailInfo
import uniffi.peercove_mobile.SharedMemoHistoryEntryInfo
import uniffi.peercove_mobile.SharedMemoListResult
import uniffi.peercove_mobile.SharedMemoSummaryInfo
import uniffi.peercove_mobile.memoCreate
import uniffi.peercove_mobile.members
import uniffi.peercove_mobile.sharedMemoAcquire
import uniffi.peercove_mobile.sharedMemoBacklinks
import uniffi.peercove_mobile.sharedMemoCommentAdd
import uniffi.peercove_mobile.sharedMemoCommentDelete
import uniffi.peercove_mobile.sharedMemoCommentList
import uniffi.peercove_mobile.sharedMemoCreate
import uniffi.peercove_mobile.sharedMemoGeneration
import uniffi.peercove_mobile.sharedMemoGet
import uniffi.peercove_mobile.sharedMemoHistoryDiff
import uniffi.peercove_mobile.sharedMemoHistoryGet
import uniffi.peercove_mobile.sharedMemoHistoryList
import uniffi.peercove_mobile.sharedMemoHistoryRestore
import uniffi.peercove_mobile.sharedMemoList
import uniffi.peercove_mobile.sharedMemoRelease
import uniffi.peercove_mobile.sharedMemoResolveTitles
import uniffi.peercove_mobile.sharedMemoSave
import uniffi.peercove_mobile.sharedMemoSaveVersion
import uniffi.peercove_mobile.sharedMemoTrash

private val sharedDateFmt = SimpleDateFormat("yyyy/MM/dd HH:mm", Locale.getDefault())

// 共有ハブ(M5 F-5 Stage 1、ADR-0052 決定 3)。共有系機能をタブで増やし続け
// ず「共有」1 か所にまとめる器。サブタブは現在「メモ」のみだが、今後
// スケジュール・表を足すときは SHARED_HUB_TABS に 1 要素足すだけでよい
private data class SharedHubTabSpec(
    val id: String,
    val labelRes: Int,
    val content: @Composable (slug: String, onNotice: (String) -> Unit) -> Unit,
)

private val SHARED_HUB_TABS = listOf(
    SharedHubTabSpec(
        id = "memos",
        labelRes = R.string.shared_hub_tab_memos,
        content = { slug, onNotice -> SharedMemoTab(slug, onNotice) },
    ),
)

@Composable
fun SharedHubTab(slug: String, onNotice: (String) -> Unit) {
    var tabId by remember { mutableStateOf(SHARED_HUB_TABS.first().id) }
    val active = SHARED_HUB_TABS.firstOrNull { it.id == tabId } ?: SHARED_HUB_TABS.first()

    Column(modifier = Modifier.fillMaxSize()) {
        Row(
            modifier = Modifier
                .fillMaxWidth()
                .horizontalScroll(rememberScrollState())
                .padding(horizontal = 12.dp, vertical = 6.dp),
            horizontalArrangement = Arrangement.spacedBy(6.dp),
        ) {
            SHARED_HUB_TABS.forEach { tab ->
                FilterChip(
                    selected = tab.id == active.id,
                    onClick = { tabId = tab.id },
                    label = { Text(stringResource(tab.labelRes)) },
                )
            }
        }
        active.content(slug, onNotice)
    }
}

@Composable
fun SharedMemoTab(slug: String, onNotice: (String) -> Unit) {
    val context = LocalContext.current
    val baseDir = context.filesDir.absolutePath
    val scope = rememberCoroutineScope()

    var search by remember { mutableStateOf("") }
    var result by remember { mutableStateOf<SharedMemoListResult?>(null) }
    var openId by remember { mutableStateOf<String?>(null) }
    var refreshTick by remember { mutableIntStateOf(0) }

    // 世代ポーリング: ホストからの配信でキャッシュが進んだら再取得(リアルタイム)
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
    LaunchedEffect(search, refreshTick) {
        try {
            result = withContext(Dispatchers.IO) {
                sharedMemoList(baseDir, slug, null, search.trim().ifEmpty { null })
            }
        } catch (e: MobileException) {
            onNotice(e.message ?: "")
        }
    }

    val opened = openId
    if (opened != null) {
        SharedMemoEditor(
            baseDir = baseDir,
            slug = slug,
            id = opened,
            onClose = {
                openId = null
                refreshTick++
            },
            // メモ間リンク(ADR-0052 決定 2)クリックで同じ画面内の別メモへ切替
            onOpenMemo = { openId = it },
            onNotice = onNotice,
        )
        return
    }

    val offline = result?.offline == true
    Scaffold(
        floatingActionButton = {
            if (!offline) {
                FloatingActionButton(onClick = {
                    scope.launch {
                        try {
                            val memo = withContext(Dispatchers.IO) {
                                sharedMemoCreate(slug, "", "")
                            }
                            openId = memo.id
                        } catch (e: MobileException) {
                            onNotice(e.message ?: "")
                        }
                    }
                }) {
                    Icon(
                        Icons.Filled.Add,
                        contentDescription = stringResource(R.string.memo_new),
                    )
                }
            }
        },
    ) { padding ->
        Column(modifier = Modifier.fillMaxSize().padding(padding)) {
            if (offline) {
                Text(
                    stringResource(R.string.shared_memo_offline),
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.primary,
                    modifier = Modifier.padding(vertical = 4.dp),
                )
            } else if (result?.supported == false) {
                Text(
                    stringResource(R.string.shared_memo_unsupported),
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    modifier = Modifier.padding(vertical = 4.dp),
                )
            }
            Text(
                stringResource(R.string.shared_memo_plaintext_note),
                style = MaterialTheme.typography.labelSmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                modifier = Modifier.padding(vertical = 2.dp),
            )
            OutlinedTextField(
                value = search,
                onValueChange = { search = it },
                modifier = Modifier.fillMaxWidth(),
                singleLine = true,
                label = { Text(stringResource(R.string.memo_search)) },
            )
            Spacer(modifier = Modifier.height(8.dp))
            val memos = result?.memos ?: emptyList()
            if (memos.isEmpty()) {
                Text(
                    stringResource(R.string.shared_memo_empty),
                    style = MaterialTheme.typography.bodyMedium,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    modifier = Modifier.padding(top = 16.dp),
                )
            }
            LazyColumn(
                verticalArrangement = Arrangement.spacedBy(8.dp),
                modifier = Modifier.weight(1f),
            ) {
                items(memos, key = { it.id }) { memo ->
                    Card(onClick = { openId = memo.id }, modifier = Modifier.fillMaxWidth()) {
                        Column(modifier = Modifier.padding(12.dp)) {
                            Row(verticalAlignment = Alignment.CenterVertically) {
                                Text(
                                    memo.title.ifEmpty { stringResource(R.string.memo_untitled) },
                                    style = MaterialTheme.typography.titleMedium,
                                    maxLines = 1,
                                    overflow = TextOverflow.Ellipsis,
                                    modifier = Modifier.weight(1f),
                                )
                                if (!memo.canEdit) {
                                    Text(
                                        stringResource(R.string.shared_memo_viewer),
                                        style = MaterialTheme.typography.labelSmall,
                                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                                    )
                                }
                            }
                            memo.lockedBy?.let {
                                Text(
                                    stringResource(R.string.shared_memo_editing_by, it),
                                    style = MaterialTheme.typography.labelSmall,
                                    color = MaterialTheme.colorScheme.primary,
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
                                Text(
                                    sharedDateFmt.format(Date(memo.updatedAt.toLong())),
                                    style = MaterialTheme.typography.labelSmall,
                                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                                )
                                memo.updatedBy?.let {
                                    Text(
                                        it,
                                        style = MaterialTheme.typography.labelSmall,
                                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                                    )
                                }
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
                                if (memo.commentCount > 0u) {
                                    Text(
                                        stringResource(
                                            R.string.shared_memo_comment_badge,
                                            memo.commentCount.toInt(),
                                        ),
                                        style = MaterialTheme.typography.labelSmall,
                                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                                    )
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

@Composable
private fun SharedMemoEditor(
    baseDir: String,
    slug: String,
    id: String,
    onClose: () -> Unit,
    /** メモ間リンク(ADR-0052 決定 2)クリックで同じ画面内の別メモへ切替。 */
    onOpenMemo: (String) -> Unit,
    onNotice: (String) -> Unit,
) {
    val scope = rememberCoroutineScope()
    var detail by remember { mutableStateOf<SharedMemoDetailInfo?>(null) }
    var editing by remember { mutableStateOf(false) }
    var title by remember { mutableStateOf("") }
    var body by remember { mutableStateOf("") }
    // 編集の土台(CAS 用リビジョンと保存済み内容)
    var baseRevision by remember { mutableStateOf(0UL) }
    var saved by remember { mutableStateOf<Pair<String, String>?>(null) }
    var saveFailed by remember { mutableStateOf<String?>(null) }
    var menuOpen by remember { mutableStateOf(false) }
    var confirmTrash by remember { mutableStateOf(false) }
    var historyOpen by remember { mutableStateOf(false) }
    val savedVersionMsg = stringResource(R.string.shared_memo_save_version_done)
    // メモ間リンク(ADR-0052 決定 2): タイトル → memo_id(見つかったものだけ)
    var wikiLinks by remember { mutableStateOf<Map<String, String>>(emptyMap()) }
    var backlinks by remember { mutableStateOf<List<SharedMemoSummaryInfo>>(emptyList()) }
    val wikilinkMissing = stringResource(R.string.memo_wikilink_missing)

    suspend fun refreshBacklinks() {
        backlinks = try {
            withContext(Dispatchers.IO) { sharedMemoBacklinks(baseDir, slug, id) }
        } catch (e: MobileException) {
            emptyList()
        }
    }

    // 閲覧中はリアルタイム追随(編集中は上書きしない)
    LaunchedEffect(id, editing) {
        if (editing) return@LaunchedEffect
        var lastGeneration = 0UL
        while (true) {
            try {
                val memo = withContext(Dispatchers.IO) { sharedMemoGet(baseDir, slug, id) }
                detail = memo
                title = memo.title
                body = memo.body
                refreshBacklinks()
            } catch (e: MobileException) {
                onNotice(e.message ?: "")
                onClose()
                return@LaunchedEffect
            }
            // 世代が進むまで待つ
            while (true) {
                delay(1500)
                val generation = withContext(Dispatchers.IO) { sharedMemoGeneration(slug) }
                if (generation != lastGeneration) {
                    lastGeneration = generation
                    break
                }
            }
        }
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
            withContext(Dispatchers.IO) { sharedMemoResolveTitles(baseDir, slug, titles) }
        } catch (e: MobileException) {
            emptyMap()
        }
    }

    // 自動保存(CAS)。編集中のみ
    LaunchedEffect(title, body) {
        val base = saved ?: return@LaunchedEffect
        if (!editing || (base.first == title && base.second == body)) return@LaunchedEffect
        delay(600)
        try {
            val memo = withContext(Dispatchers.IO) {
                sharedMemoSave(slug, id, baseRevision, title, body)
            }
            baseRevision = memo.revision
            saved = title to body
            saveFailed = null
            detail = memo
            // タイトル変更でバックリンクの対象が変わりうる
            refreshBacklinks()
        } catch (e: MobileException) {
            saveFailed = e.message
        }
    }

    fun stopEditing(onDone: () -> Unit = {}) {
        if (!editing) {
            onDone()
            return
        }
        val base = saved
        scope.launch {
            try {
                withContext(Dispatchers.IO) {
                    if (base != null && (base.first != title || base.second != body)) {
                        val memo = sharedMemoSave(slug, id, baseRevision, title, body)
                        baseRevision = memo.revision
                    }
                    sharedMemoRelease(slug, id)
                }
            } catch (e: MobileException) {
                onNotice(e.message ?: "")
            }
            editing = false
            saved = null
            onDone()
        }
    }

    if (historyOpen) {
        SharedMemoHistoryScreen(
            baseDir = baseDir,
            slug = slug,
            memoId = id,
            canEdit = detail?.canEdit == true,
            onNotice = onNotice,
            onClose = { historyOpen = false },
            onRestored = {
                historyOpen = false
                scope.launch {
                    try {
                        val memo = withContext(Dispatchers.IO) { sharedMemoGet(baseDir, slug, id) }
                        detail = memo
                        title = memo.title
                        body = memo.body
                    } catch (e: MobileException) {
                        onNotice(e.message ?: "")
                    }
                }
            },
        )
        return
    }

    BackHandler { stopEditing { onClose() } }

    if (confirmTrash) {
        AlertDialog(
            onDismissRequest = { confirmTrash = false },
            title = { Text(stringResource(R.string.memo_trash_action)) },
            text = { Text(stringResource(R.string.shared_memo_trash_confirm)) },
            confirmButton = {
                TextButton(onClick = {
                    confirmTrash = false
                    scope.launch {
                        try {
                            withContext(Dispatchers.IO) { sharedMemoTrash(slug, id) }
                            onClose()
                        } catch (e: MobileException) {
                            onNotice(e.message ?: "")
                        }
                    }
                }) { Text(stringResource(R.string.action_remove)) }
            },
            dismissButton = {
                TextButton(onClick = { confirmTrash = false }) {
                    Text(stringResource(R.string.action_cancel))
                }
            },
        )
    }

    Column(modifier = Modifier.fillMaxSize()) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            IconButton(onClick = { stopEditing { onClose() } }) {
                Icon(
                    Icons.AutoMirrored.Filled.ArrowBack,
                    contentDescription = stringResource(R.string.action_back),
                )
            }
            Column(modifier = Modifier.weight(1f)) {
                Text(
                    when {
                        editing && saveFailed != null ->
                            stringResource(R.string.memo_save_failed)
                        editing && saved == (title to body) ->
                            stringResource(R.string.memo_saved)
                        editing -> stringResource(R.string.memo_saving)
                        detail?.lockedBy != null -> stringResource(
                            R.string.shared_memo_editing_by,
                            detail?.lockedBy ?: "",
                        )
                        else -> stringResource(R.string.shared_memo_viewing)
                    },
                    style = MaterialTheme.typography.labelMedium,
                    color = if (editing && saveFailed != null) {
                        MaterialTheme.colorScheme.error
                    } else {
                        MaterialTheme.colorScheme.onSurfaceVariant
                    },
                )
                detail?.let {
                    Text(
                        stringResource(
                            R.string.shared_memo_owner,
                            it.ownerName.ifEmpty {
                                stringResource(R.string.shared_memo_host)
                            },
                        ) + " ・rev ${it.revision}",
                        style = MaterialTheme.typography.labelSmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
            }
            if (!editing) {
                IconButton(onClick = { historyOpen = true }) {
                    Icon(
                        Icons.Filled.History,
                        contentDescription = stringResource(R.string.shared_memo_history),
                    )
                }
                FilterChip(
                    selected = false,
                    enabled = detail?.canEdit == true && detail?.lockedBy == null,
                    onClick = {
                        scope.launch {
                            try {
                                val memo = withContext(Dispatchers.IO) {
                                    sharedMemoAcquire(slug, id)
                                }
                                detail = memo
                                title = memo.title
                                body = memo.body
                                baseRevision = memo.revision
                                saved = memo.title to memo.body
                                saveFailed = null
                                editing = true
                            } catch (e: MobileException) {
                                onNotice(e.message ?: "")
                            }
                        }
                    },
                    label = { Text(stringResource(R.string.shared_memo_edit)) },
                )
            } else {
                IconButton(onClick = {
                    scope.launch {
                        try {
                            withContext(Dispatchers.IO) { sharedMemoSaveVersion(slug, id) }
                            onNotice(savedVersionMsg)
                        } catch (e: MobileException) {
                            onNotice(e.message ?: "")
                        }
                    }
                }) {
                    Icon(
                        Icons.Filled.Save,
                        contentDescription = stringResource(R.string.shared_memo_save_version),
                    )
                }
                FilterChip(
                    selected = true,
                    onClick = { stopEditing() },
                    label = { Text(stringResource(R.string.shared_memo_finish)) },
                )
            }
            if (detail?.canManage == true) {
                IconButton(onClick = { menuOpen = true }) {
                    Icon(
                        Icons.Filled.MoreVert,
                        contentDescription = stringResource(R.string.memo_menu),
                    )
                }
                DropdownMenu(expanded = menuOpen, onDismissRequest = { menuOpen = false }) {
                    DropdownMenuItem(
                        text = {
                            Text(
                                stringResource(R.string.memo_trash_action),
                                color = MaterialTheme.colorScheme.error,
                            )
                        },
                        onClick = {
                            menuOpen = false
                            confirmTrash = true
                        },
                    )
                }
            }
        }
        saveFailed?.let {
            Text(
                it,
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.error,
            )
        }

        if (editing) {
            OutlinedTextField(
                value = title,
                onValueChange = { title = it },
                modifier = Modifier.fillMaxWidth(),
                singleLine = true,
                label = { Text(stringResource(R.string.memo_title_hint)) },
            )
            Spacer(modifier = Modifier.height(8.dp))
            OutlinedTextField(
                value = body,
                onValueChange = { body = it },
                modifier = Modifier.weight(1f).fillMaxWidth(),
                label = { Text(stringResource(R.string.memo_body_hint)) },
            )
        } else {
            Text(
                title.ifEmpty { stringResource(R.string.memo_untitled) },
                style = MaterialTheme.typography.titleLarge,
                modifier = Modifier.padding(vertical = 4.dp),
            )
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
                            stopEditing { onOpenMemo(targetId) }
                        } else {
                            onNotice(wikilinkMissing)
                        }
                    },
                )
                BacklinksSection(
                    backlinks = backlinks,
                    idOf = { it.id },
                    titleOf = { it.title },
                    onOpen = { targetId -> stopEditing { onOpenMemo(targetId) } },
                )
                CommentsSection(
                    slug = slug,
                    memoId = id,
                    commentCount = detail?.commentCount?.toInt() ?: 0,
                    canManage = detail?.canManage == true,
                    onNotice = onNotice,
                )
            }
        }
        Spacer(modifier = Modifier.height(8.dp))
    }
}

/**
 * コメント欄(M5 F-5 Stage 3、ADR-0052 決定 4・5)。閲覧・追加は閲覧権限が
 * あれば可、削除は本人・所有者・ホストだけ(サーバー側でも検査される。
 * ここでの削除ボタン表示は本人判定を表示名で近似する — Android は
 * 常にメンバー役割なので、所有者・ホストの判定は `canManage` で正確に行える。
 * ADR-0039 の「頭脳は Rust」に反しない範囲の UI 表示上の簡略化)。
 * `commentCount` が変わるたびに一覧を取り直す(親の世代ポーリングに相乗り)。
 */
@Composable
private fun CommentsSection(
    slug: String,
    memoId: String,
    commentCount: Int,
    canManage: Boolean,
    onNotice: (String) -> Unit,
) {
    val scope = rememberCoroutineScope()
    var comments by remember(memoId) { mutableStateOf<List<SharedMemoCommentInfo>>(emptyList()) }
    var draft by remember(memoId) { mutableStateOf("") }
    var sending by remember { mutableStateOf(false) }
    var myName by remember { mutableStateOf("") }
    var memberNames by remember { mutableStateOf<List<String>>(emptyList()) }
    var pendingDelete by remember { mutableStateOf<String?>(null) }
    val hostLabel = stringResource(R.string.shared_memo_host)

    LaunchedEffect(slug) {
        try {
            val list = withContext(Dispatchers.IO) { members(slug) }
            myName = list.firstOrNull { it.isSelf }?.name ?: ""
            memberNames = list.filter { !it.isSelf && it.name.isNotEmpty() }.map { it.name }
        } catch (e: MobileException) {
            // メンション候補が引けなくても入力自体は妨げない
        }
    }

    LaunchedEffect(memoId, commentCount) {
        comments = try {
            withContext(Dispatchers.IO) { sharedMemoCommentList(slug, memoId) }
        } catch (e: MobileException) {
            onNotice(e.message ?: "")
            emptyList()
        }
    }

    val toDelete = pendingDelete
    if (toDelete != null) {
        AlertDialog(
            onDismissRequest = { pendingDelete = null },
            title = { Text(stringResource(R.string.shared_memo_comment_delete_confirm)) },
            confirmButton = {
                TextButton(onClick = {
                    pendingDelete = null
                    scope.launch {
                        try {
                            withContext(Dispatchers.IO) {
                                sharedMemoCommentDelete(slug, memoId, toDelete)
                            }
                            comments = comments.filter { it.commentId != toDelete }
                        } catch (e: MobileException) {
                            onNotice(e.message ?: "")
                        }
                    }
                }) { Text(stringResource(R.string.action_remove)) }
            },
            dismissButton = {
                TextButton(onClick = { pendingDelete = null }) {
                    Text(stringResource(R.string.action_cancel))
                }
            },
        )
    }

    Spacer(modifier = Modifier.height(8.dp))
    HorizontalDivider()
    Spacer(modifier = Modifier.height(4.dp))
    Text(
        stringResource(R.string.shared_memo_comments_title, comments.size),
        style = MaterialTheme.typography.labelLarge,
        color = MaterialTheme.colorScheme.onSurfaceVariant,
    )
    if (comments.isEmpty()) {
        Text(
            stringResource(R.string.shared_memo_comments_empty),
            style = MaterialTheme.typography.bodySmall,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
            modifier = Modifier.padding(vertical = 4.dp),
        )
    }
    comments.forEach { comment ->
        Column(modifier = Modifier.padding(vertical = 4.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Text(
                    comment.authorName.ifEmpty { hostLabel },
                    style = MaterialTheme.typography.labelMedium,
                    modifier = Modifier.weight(1f),
                )
                Text(
                    sharedDateFmt.format(Date(comment.createdAtUnixMs.toLong())),
                    style = MaterialTheme.typography.labelSmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
                if (canManage || (myName.isNotEmpty() && comment.authorName == myName)) {
                    IconButton(
                        onClick = { pendingDelete = comment.commentId },
                        modifier = Modifier.height(24.dp),
                    ) {
                        Icon(
                            Icons.Filled.Close,
                            contentDescription = stringResource(R.string.action_remove),
                        )
                    }
                }
            }
            Text(comment.body, style = MaterialTheme.typography.bodyMedium)
        }
    }
    Spacer(modifier = Modifier.height(4.dp))
    val mentionMatches = remember(draft, memberNames) {
        val at = draft.lastIndexOf('@')
        if (at < 0 || (at > 0 && !draft[at - 1].isWhitespace())) {
            emptyList()
        } else {
            val query = draft.substring(at + 1)
            if (query.contains(' ') || query.contains('\n')) {
                emptyList()
            } else {
                memberNames.filter { query.isEmpty() || it.contains(query) }.take(5)
            }
        }
    }
    if (mentionMatches.isNotEmpty()) {
        Row(
            modifier = Modifier.fillMaxWidth().horizontalScroll(rememberScrollState()),
            horizontalArrangement = Arrangement.spacedBy(4.dp),
        ) {
            mentionMatches.forEach { name ->
                TextButton(onClick = {
                    val at = draft.lastIndexOf('@')
                    draft = draft.substring(0, at) + "@" + name + " "
                }) { Text(name) }
            }
        }
    }
    OutlinedTextField(
        value = draft,
        onValueChange = { draft = it },
        modifier = Modifier.fillMaxWidth(),
        label = { Text(stringResource(R.string.shared_memo_comment_hint)) },
    )
    Row(modifier = Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.End) {
        TextButton(
            enabled = draft.isNotBlank() && !sending,
            onClick = {
                val body = draft.trim()
                sending = true
                scope.launch {
                    try {
                        val comment = withContext(Dispatchers.IO) {
                            sharedMemoCommentAdd(slug, memoId, body)
                        }
                        comments = comments + comment
                        draft = ""
                    } catch (e: MobileException) {
                        onNotice(e.message ?: "")
                    }
                    sending = false
                }
            },
        ) { Text(stringResource(R.string.shared_memo_comment_send)) }
    }
}

/** 変更履歴の一覧(全画面。M5 F-3、ADR-0049)。項目タップで詳細/差分/復元へ。 */
@Composable
private fun SharedMemoHistoryScreen(
    baseDir: String,
    slug: String,
    memoId: String,
    canEdit: Boolean,
    onNotice: (String) -> Unit,
    onClose: () -> Unit,
    onRestored: () -> Unit,
) {
    var entries by remember { mutableStateOf<List<SharedMemoHistoryEntryInfo>>(emptyList()) }
    var loaded by remember { mutableStateOf(false) }
    var selected by remember { mutableStateOf<SharedMemoHistoryEntryInfo?>(null) }

    LaunchedEffect(memoId) {
        try {
            entries = withContext(Dispatchers.IO) { sharedMemoHistoryList(slug, memoId) }
        } catch (e: MobileException) {
            onNotice(e.message ?: "")
        }
        loaded = true
    }

    val sel = selected
    if (sel != null) {
        SharedMemoHistoryDetailScreen(
            baseDir = baseDir,
            slug = slug,
            memoId = memoId,
            entry = sel,
            canEdit = canEdit,
            onNotice = onNotice,
            onBack = { selected = null },
            onRestored = onRestored,
        )
        return
    }

    BackHandler { onClose() }

    Column(modifier = Modifier.fillMaxSize()) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            IconButton(onClick = onClose) {
                Icon(
                    Icons.AutoMirrored.Filled.ArrowBack,
                    contentDescription = stringResource(R.string.action_back),
                )
            }
            Text(
                stringResource(R.string.shared_memo_history),
                style = MaterialTheme.typography.titleMedium,
                modifier = Modifier.weight(1f),
            )
        }
        if (loaded && entries.isEmpty()) {
            Text(
                stringResource(R.string.shared_memo_history_empty),
                style = MaterialTheme.typography.bodyMedium,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                modifier = Modifier.padding(top = 16.dp),
            )
        }
        LazyColumn(
            verticalArrangement = Arrangement.spacedBy(8.dp),
            modifier = Modifier.weight(1f).padding(top = 8.dp),
        ) {
            items(entries, key = { it.hid }) { entry ->
                Card(onClick = { selected = entry }, modifier = Modifier.fillMaxWidth()) {
                    Column(modifier = Modifier.padding(12.dp)) {
                        Row(verticalAlignment = Alignment.CenterVertically) {
                            Text(
                                sharedDateFmt.format(Date(entry.createdAtUnixMs.toLong())),
                                style = MaterialTheme.typography.bodyMedium,
                                modifier = Modifier.weight(1f),
                            )
                            Text(
                                historyKindLabel(entry.kind),
                                style = MaterialTheme.typography.labelSmall,
                                color = MaterialTheme.colorScheme.primary,
                            )
                        }
                        Text(
                            entry.savedByName,
                            style = MaterialTheme.typography.labelSmall,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                    }
                }
            }
        }
    }
}

/** 変更履歴 1 版の詳細: 本文/差分の表示、復元、個人メモへコピー。 */
@Composable
private fun SharedMemoHistoryDetailScreen(
    baseDir: String,
    slug: String,
    memoId: String,
    entry: SharedMemoHistoryEntryInfo,
    canEdit: Boolean,
    onNotice: (String) -> Unit,
    onBack: () -> Unit,
    onRestored: () -> Unit,
) {
    val scope = rememberCoroutineScope()
    var detail by remember { mutableStateOf<SharedMemoHistoryDetailInfo?>(null) }
    var diffLines by remember { mutableStateOf<List<DiffLineInfo>?>(null) }
    var confirmRestore by remember { mutableStateOf(false) }
    var restoring by remember { mutableStateOf(false) }
    val restoredMsg = stringResource(R.string.shared_memo_history_restored)
    val copiedMsg = stringResource(R.string.shared_memo_copied_to_personal)

    LaunchedEffect(entry.hid) {
        diffLines = null
        try {
            detail = withContext(Dispatchers.IO) { sharedMemoHistoryGet(slug, memoId, entry.hid) }
        } catch (e: MobileException) {
            onNotice(e.message ?: "")
        }
    }

    fun toggleDiff() {
        if (diffLines != null) {
            diffLines = null
            return
        }
        scope.launch {
            try {
                diffLines = withContext(Dispatchers.IO) {
                    sharedMemoHistoryDiff(slug, memoId, entry.hid, null)
                }
            } catch (e: MobileException) {
                onNotice(e.message ?: "")
            }
        }
    }

    BackHandler { onBack() }

    if (confirmRestore) {
        AlertDialog(
            onDismissRequest = { confirmRestore = false },
            title = { Text(stringResource(R.string.shared_memo_history_restore_action)) },
            text = { Text(stringResource(R.string.shared_memo_history_restore_confirm)) },
            confirmButton = {
                TextButton(onClick = {
                    confirmRestore = false
                    restoring = true
                    scope.launch {
                        try {
                            withContext(Dispatchers.IO) {
                                sharedMemoHistoryRestore(slug, memoId, entry.hid)
                            }
                            onNotice(restoredMsg)
                            onRestored()
                        } catch (e: MobileException) {
                            onNotice(e.message ?: "")
                        }
                        restoring = false
                    }
                }) { Text(stringResource(R.string.shared_memo_history_restore_action)) }
            },
            dismissButton = {
                TextButton(onClick = { confirmRestore = false }) {
                    Text(stringResource(R.string.action_cancel))
                }
            },
        )
    }

    Column(modifier = Modifier.fillMaxSize()) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            IconButton(onClick = onBack) {
                Icon(
                    Icons.AutoMirrored.Filled.ArrowBack,
                    contentDescription = stringResource(R.string.action_back),
                )
            }
            Column(modifier = Modifier.weight(1f)) {
                Text(
                    sharedDateFmt.format(Date(entry.createdAtUnixMs.toLong())),
                    style = MaterialTheme.typography.labelMedium,
                )
                Text(
                    "${historyKindLabel(entry.kind)} ・ ${entry.savedByName}",
                    style = MaterialTheme.typography.labelSmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
        }
        Row(
            horizontalArrangement = Arrangement.spacedBy(4.dp),
            modifier = Modifier.padding(vertical = 4.dp),
        ) {
            TextButton(onClick = { toggleDiff() }) {
                Text(
                    if (diffLines != null) {
                        stringResource(R.string.shared_memo_history_show_body)
                    } else {
                        stringResource(R.string.shared_memo_history_compare)
                    },
                )
            }
            if (canEdit) {
                TextButton(enabled = !restoring, onClick = { confirmRestore = true }) {
                    Text(stringResource(R.string.shared_memo_history_restore_action))
                }
            }
            TextButton(onClick = {
                val d = detail ?: return@TextButton
                scope.launch {
                    try {
                        withContext(Dispatchers.IO) {
                            memoCreate(baseDir, d.entry.title, d.body, null)
                        }
                        onNotice(copiedMsg)
                    } catch (e: MobileException) {
                        onNotice(e.message ?: "")
                    }
                }
            }) { Text(stringResource(R.string.shared_memo_copy_to_personal)) }
        }
        val lines = diffLines
        if (lines != null) {
            Column(
                modifier = Modifier
                    .weight(1f)
                    .fillMaxWidth()
                    .verticalScroll(rememberScrollState())
                    .horizontalScroll(rememberScrollState())
                    .padding(vertical = 8.dp),
            ) {
                lines.forEach { line ->
                    val prefix = when (line.kind) {
                        "added" -> "+ "
                        "removed" -> "- "
                        else -> "  "
                    }
                    val bg = when (line.kind) {
                        "added" -> Color(0x1F4CAF50)
                        "removed" -> Color(0x1FF44336)
                        else -> Color.Transparent
                    }
                    Text(
                        prefix + line.text,
                        fontFamily = FontFamily.Monospace,
                        style = MaterialTheme.typography.bodySmall,
                        softWrap = false,
                        modifier = Modifier.fillMaxWidth().background(bg),
                    )
                }
            }
        } else {
            Column(
                modifier = Modifier
                    .weight(1f)
                    .fillMaxWidth()
                    .verticalScroll(rememberScrollState())
                    .padding(vertical = 8.dp),
            ) {
                MarkdownPreview(detail?.body ?: "")
            }
        }
    }
}

@Composable
private fun historyKindLabel(kind: String): String = when (kind) {
    "auto" -> stringResource(R.string.shared_memo_history_kind_auto)
    "close" -> stringResource(R.string.shared_memo_history_kind_close)
    "manual" -> stringResource(R.string.shared_memo_history_kind_manual)
    "restore" -> stringResource(R.string.shared_memo_history_kind_restore)
    else -> kind
}
