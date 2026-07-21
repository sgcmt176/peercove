package app.peercove.android

// 共有メモ(M5 F-2、ADR-0049)。読み取りはキャッシュ(オフラインでも閲覧可)、
// 変更はホストへ届き、権限・単一編集者ロック・リビジョン(CAS)はホスト正本で
// 判定される。閲覧中は世代ポーリングでリアルタイムに追随する。

import androidx.activity.compose.BackHandler
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
import androidx.compose.material.icons.filled.MoreVert
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Card
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.FilterChip
import androidx.compose.material3.FloatingActionButton
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
import uniffi.peercove_mobile.MobileException
import uniffi.peercove_mobile.SharedMemoDetailInfo
import uniffi.peercove_mobile.SharedMemoListResult
import uniffi.peercove_mobile.sharedMemoAcquire
import uniffi.peercove_mobile.sharedMemoCreate
import uniffi.peercove_mobile.sharedMemoGeneration
import uniffi.peercove_mobile.sharedMemoGet
import uniffi.peercove_mobile.sharedMemoList
import uniffi.peercove_mobile.sharedMemoRelease
import uniffi.peercove_mobile.sharedMemoSave
import uniffi.peercove_mobile.sharedMemoTrash

private val sharedDateFmt = SimpleDateFormat("yyyy/MM/dd HH:mm", Locale.getDefault())

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
                MarkdownPreview(body)
            }
        }
        Spacer(modifier = Modifier.height(8.dp))
    }
}
