package app.peercove.android

import android.content.Intent
import android.net.Uri
import androidx.activity.compose.BackHandler
import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.Checkbox
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Surface
import androidx.compose.material3.Tab
import androidx.compose.material3.TabRow
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material.icons.filled.Add
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateMapOf
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
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import uniffi.peercove_mobile.ChatMessage
import uniffi.peercove_mobile.DnsEntry
import uniffi.peercove_mobile.GroupSummary
import uniffi.peercove_mobile.MemberInfo
import uniffi.peercove_mobile.MobileException
import uniffi.peercove_mobile.SessionState
import uniffi.peercove_mobile.TunnelStatus
import uniffi.peercove_mobile.chatFetch
import uniffi.peercove_mobile.chatGroups
import uniffi.peercove_mobile.createGroup
import uniffi.peercove_mobile.dnsEntries
import uniffi.peercove_mobile.listNetworks
import uniffi.peercove_mobile.members
import uniffi.peercove_mobile.rotateKey
import uniffi.peercove_mobile.sessionState
import uniffi.peercove_mobile.setDisplayName
import uniffi.peercove_mobile.setDnsName
import uniffi.peercove_mobile.tunnelStatus
import uniffi.peercove_mobile.updateNetworkSettings

/** 既読管理のキー(Prefs 保存用の安定 ID)。 */
private fun ConvKey.storageId(): String = when (this) {
    is ConvKey.Network -> "network"
    is ConvKey.Direct -> "direct/$ip"
    is ConvKey.Group -> "group/$id"
}

/**
 * ネットワーク詳細(接続中のトーク / メンバー / DNS / 設定)。
 * 状態はすべて Rust 側が正本で、2 秒(チャットは 1.5 秒)のポーリングで映す。
 */
@Composable
fun NetworkScreen(
    slug: String,
    networkName: String,
    initialConvId: String? = null,
    onBack: () -> Unit,
    onNotice: (String) -> Unit,
) {
    val context = LocalContext.current
    var tab by remember { mutableStateOf(0) }
    var state by remember { mutableStateOf<SessionState?>(null) }
    var tunnel by remember { mutableStateOf<TunnelStatus?>(null) }
    var memberList by remember { mutableStateOf<List<MemberInfo>>(emptyList()) }
    var groupList by remember { mutableStateOf<List<GroupSummary>>(emptyList()) }
    var dnsList by remember { mutableStateOf<List<DnsEntry>>(emptyList()) }
    var messages by remember { mutableStateOf<List<ChatMessage>>(emptyList()) }
    var conv by remember { mutableStateOf<ConvKey?>(null) }
    var showGroupDialog by remember { mutableStateOf(false) }
    val clipboard = LocalClipboardManager.current
    val copiedFmt = stringResource(R.string.notice_copied)

    // 既読位置(未読バッジ用)とピン留め。Prefs が正本、map は表示のための鏡
    val readMarks = remember { mutableStateMapOf<String, Long>() }
    val pinMarks = remember { mutableStateMapOf<String, Boolean>() }
    LaunchedEffect(slug) {
        Prefs.allReadSeqs(context, slug).forEach { (convId, seq) -> readMarks[convId] = seq }
        Prefs.allPins(context, slug).forEach { convId -> pinMarks[convId] = true }
    }
    // 会話を開いている間はその会話の最新までを既読にする
    LaunchedEffect(conv, messages.size) {
        val current = conv ?: return@LaunchedEffect
        val latest = messages.filter { it.belongsTo(current) }
            .maxOfOrNull { it.seq.toLong() } ?: return@LaunchedEffect
        Prefs.setReadSeq(context, slug, current.storageId(), latest)
        readMarks[current.storageId()] = maxOf(readMarks[current.storageId()] ?: 0L, latest)
    }
    fun unreadOf(key: ConvKey): Int {
        val read = readMarks[key.storageId()] ?: 0L
        return messages.count {
            it.belongsTo(key) && !it.outgoing && !it.system && it.seq.toLong() > read
        }
    }
    val pinnedNotice = stringResource(R.string.talk_pinned)
    val unpinnedNotice = stringResource(R.string.talk_unpinned)
    fun togglePin(key: ConvKey) {
        val id = key.storageId()
        val next = !(pinMarks[id] ?: false)
        pinMarks[id] = next
        Prefs.setPinned(context, slug, id, next)
        onNotice(if (next) pinnedNotice else unpinnedNotice)
    }

    // チャット通知のタップから来たとき、対象の会話を開く(メンバー・グループの
    // 情報が届いてから名前を解決する)
    var initialConsumed by remember { mutableStateOf(initialConvId == null) }
    LaunchedEffect(memberList, groupList) {
        if (initialConsumed || initialConvId == null) return@LaunchedEffect
        val key = when {
            initialConvId == "network" -> ConvKey.Network
            initialConvId.startsWith("direct/") -> {
                val ip = initialConvId.removePrefix("direct/")
                memberList.firstOrNull { it.ip == ip }?.let { ConvKey.Direct(ip, it.name) }
            }
            initialConvId.startsWith("group/") -> {
                val id = initialConvId.removePrefix("group/")
                groupList.firstOrNull { it.id == id }?.let { ConvKey.Group(id, it.name) }
            }
            else -> null
        }
        if (key != null) {
            conv = key
            initialConsumed = true
        }
    }

    // 会話を開いているときのシステム戻る操作はトーク一覧へ(アプリを閉じない)
    BackHandler(enabled = conv != null) { conv = null }

    fun copy(text: String) {
        clipboard.setText(AnnotatedString(text))
        onNotice(copiedFmt.format(text))
    }

    // セッション情報のポーリング
    LaunchedEffect(slug) {
        while (true) {
            withContext(Dispatchers.IO) {
                state = sessionState(slug)
                tunnel = tunnelStatus(slug)
                memberList = members(slug)
                groupList = chatGroups(slug)
                dnsList = dnsEntries(slug)
            }
            delay(2000)
        }
    }
    // チャット履歴の差分ポーリング
    LaunchedEffect(slug) {
        var after = 0uL
        while (true) {
            val batch = withContext(Dispatchers.IO) { chatFetch(slug, after, 500u) }
            if (batch.isNotEmpty()) {
                messages = messages + batch
                after = batch.last().seq
            } else {
                delay(1500)
            }
        }
    }

    if (showGroupDialog) {
        GroupCreateDialog(
            slug = slug,
            candidates = memberList.filter { !it.isSelf && !it.blocked },
            onNotice = onNotice,
            onDismiss = { showGroupDialog = false },
        )
    }

    Column(modifier = Modifier.fillMaxSize()) {
        // ヘッダ
        Row(verticalAlignment = Alignment.CenterVertically) {
            IconButton(onClick = { if (conv != null) conv = null else onBack() }) {
                Icon(
                    Icons.AutoMirrored.Filled.ArrowBack,
                    contentDescription = stringResource(R.string.action_back),
                )
            }
            Column {
                Text(
                    conv?.title() ?: networkName,
                    style = MaterialTheme.typography.titleMedium,
                )
                Text(
                    statusText(state),
                    style = MaterialTheme.typography.labelSmall,
                    color = statusColor(state),
                )
            }
        }

        val currentConv = conv
        if (currentConv != null) {
            ConversationScreen(slug, currentConv, messages, memberList, onNotice)
            return@Column
        }

        TabRow(selectedTabIndex = tab) {
            listOf(
                stringResource(R.string.tab_talk),
                stringResource(R.string.tab_members),
                stringResource(R.string.tab_dns),
                stringResource(R.string.tab_settings),
            ).forEachIndexed { index, title ->
                Tab(selected = tab == index, onClick = { tab = index }, text = { Text(title) })
            }
        }
        when (tab) {
            0 -> TalkList(
                memberList = memberList,
                groupList = groupList,
                messages = messages,
                unreadOf = ::unreadOf,
                pinnedOf = { pinMarks[it.storageId()] ?: false },
                onTogglePin = ::togglePin,
                onNewGroup = { showGroupDialog = true },
                onOpen = { conv = it },
            )
            1 -> MemberList(memberList, onCopy = ::copy)
            2 -> DnsList(memberList, dnsList, onCopy = ::copy, onNotice = onNotice)
            else -> SettingsTab(slug, state, tunnel, onNotice)
        }
    }
}

@Composable
private fun statusText(state: SessionState?): String = when {
    state == null -> stringResource(R.string.session_not_connected)
    state.removed -> stringResource(R.string.session_removed)
    state.rejected != null -> stringResource(R.string.session_rejected, state.rejected ?: "")
    state.controlConnected -> state.rttMs
        ?.let { stringResource(R.string.session_synced_rtt, it.toLong()) }
        ?: stringResource(R.string.session_synced)
    else -> stringResource(R.string.session_waiting)
}

@Composable
private fun statusColor(state: SessionState?): Color = when {
    state == null -> MaterialTheme.colorScheme.onSurfaceVariant
    state.removed || state.rejected != null -> MaterialTheme.colorScheme.error
    state.controlConnected -> MaterialTheme.colorScheme.primary
    else -> MaterialTheme.colorScheme.onSurfaceVariant
}

/** 新しいグループの作成ダイアログ(名前 + オンラインメンバーの選択)。 */
@Composable
private fun GroupCreateDialog(
    slug: String,
    candidates: List<MemberInfo>,
    onNotice: (String) -> Unit,
    onDismiss: () -> Unit,
) {
    val scope = rememberCoroutineScope()
    var name by remember { mutableStateOf("") }
    val checked = remember { mutableStateMapOf<String, Boolean>() }
    var busy by remember { mutableStateOf(false) }
    val createdFmt = stringResource(R.string.group_created)
    val failed = stringResource(R.string.failed_generic)

    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(stringResource(R.string.group_dialog_title)) },
        text = {
            Column {
                OutlinedTextField(
                    value = name,
                    onValueChange = { name = it },
                    modifier = Modifier.fillMaxWidth(),
                    label = { Text(stringResource(R.string.group_name_label)) },
                    singleLine = true,
                )
                Spacer(modifier = Modifier.padding(4.dp))
                Text(
                    stringResource(R.string.group_members_label),
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
                // モバイルは送達再送を持たないため、作成時に選べるのは
                // いま届けられる(オンラインの)相手だけ(Rust 側と同じ制約)
                Column(modifier = Modifier.verticalScroll(rememberScrollState())) {
                    candidates.forEach { member ->
                        Row(
                            modifier = Modifier
                                .fillMaxWidth()
                                .clickable(enabled = member.online) {
                                    checked[member.ip] = !(checked[member.ip] ?: false)
                                },
                            verticalAlignment = Alignment.CenterVertically,
                        ) {
                            Checkbox(
                                checked = checked[member.ip] ?: false,
                                onCheckedChange = { checked[member.ip] = it },
                                enabled = member.online,
                            )
                            Text(
                                member.name,
                                color = if (member.online) {
                                    MaterialTheme.colorScheme.onSurface
                                } else {
                                    MaterialTheme.colorScheme.onSurfaceVariant
                                },
                            )
                        }
                    }
                }
            }
        },
        confirmButton = {
            val selected = checked.filterValues { it }.keys.toList()
            Button(
                enabled = !busy && name.isNotBlank() && selected.isNotEmpty(),
                onClick = {
                    busy = true
                    scope.launch {
                        try {
                            val group = withContext(Dispatchers.IO) {
                                createGroup(slug, name.trim(), selected)
                            }
                            onNotice(createdFmt.format(group.name))
                            onDismiss()
                        } catch (e: MobileException) {
                            onNotice(e.message ?: failed)
                        } finally {
                            busy = false
                        }
                    }
                },
            ) { Text(stringResource(R.string.group_create)) }
        },
        dismissButton = {
            TextButton(onClick = onDismiss) { Text(stringResource(R.string.action_cancel)) }
        },
    )
}

/** LINE 風のトーク一覧: ピン留め → 直近のやり取り順。未読バッジ付き。
 *  行の長押しでピン留めの付け外し。 */
@OptIn(ExperimentalFoundationApi::class)
@Composable
private fun TalkList(
    memberList: List<MemberInfo>,
    groupList: List<GroupSummary>,
    messages: List<ChatMessage>,
    unreadOf: (ConvKey) -> Int,
    pinnedOf: (ConvKey) -> Boolean,
    onTogglePin: (ConvKey) -> Unit,
    onNewGroup: () -> Unit,
    onOpen: (ConvKey) -> Unit,
) {
    // 候補: 全体 → グループ → メンバー(この順は同順位時のフォールバック)
    val base = buildList {
        add(ConvKey.Network)
        groupList.forEach { add(ConvKey.Group(it.id, it.name)) }
        memberList.filter { !it.isSelf }.forEach { add(ConvKey.Direct(it.ip, it.name)) }
    }
    // 表示順: ピン留めが常に上、続いて最新メッセージの新しい順。
    // メッセージが無い会話は元の並びのまま後ろへ
    val lastSeq = HashMap<String, Long>()
    messages.forEach { m ->
        base.forEach { key -> if (m.belongsTo(key)) lastSeq[key.storageId()] = m.seq.toLong() }
    }
    val conversations = base.withIndex().sortedWith(
        compareByDescending<IndexedValue<ConvKey>> { pinnedOf(it.value) }
            .thenByDescending { lastSeq[it.value.storageId()] ?: 0L }
            .thenBy { it.index },
    ).map { it.value }
    LazyColumn {
        items(conversations, key = { it.storageId() }) { key ->
            val last = messages.lastOrNull { it.belongsTo(key) }
            val online = when (key) {
                is ConvKey.Direct -> memberList.firstOrNull { it.ip == key.ip }?.online == true
                else -> true
            }
            Row(
                modifier = Modifier
                    .fillMaxWidth()
                    .combinedClickable(
                        onClick = { onOpen(key) },
                        onLongClick = { onTogglePin(key) },
                    )
                    .padding(horizontal = 12.dp, vertical = 10.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Box(
                    modifier = Modifier
                        .size(44.dp)
                        .background(
                            when (key) {
                                is ConvKey.Network -> Color(0xFF42A5F5)
                                is ConvKey.Group -> Color(0xFF66BB6A)
                                is ConvKey.Direct -> avatarColor(key.ip)
                            },
                            CircleShape,
                        ),
                    contentAlignment = Alignment.Center,
                ) {
                    Text(
                        when (key) {
                            is ConvKey.Network -> stringResource(R.string.talk_all_avatar)
                            is ConvKey.Group -> "G"
                            is ConvKey.Direct -> key.name.take(1)
                        },
                        color = Color.White,
                    )
                }
                Spacer(modifier = Modifier.width(10.dp))
                Column(modifier = Modifier.weight(1f)) {
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        if (pinnedOf(key)) {
                            Text("📌", style = MaterialTheme.typography.labelSmall)
                            Spacer(modifier = Modifier.width(4.dp))
                        }
                        Text(key.title(), style = MaterialTheme.typography.titleSmall)
                        if (key is ConvKey.Direct && online) {
                            Spacer(modifier = Modifier.width(6.dp))
                            Box(
                                modifier = Modifier
                                    .size(8.dp)
                                    .background(Color(0xFF4CAF50), CircleShape),
                            )
                        }
                    }
                    Text(
                        when {
                            last == null -> stringResource(R.string.talk_no_message)
                            last.fileName != null -> stringResource(
                                R.string.chat_file_prefix,
                                last.fileName ?: "",
                            )
                            else -> last.text
                        },
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                        maxLines = 1,
                    )
                }
                Column(horizontalAlignment = Alignment.End) {
                    last?.let {
                        Text(
                            formatTime(it.sentAt),
                            style = MaterialTheme.typography.labelSmall,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                    }
                    val unread = unreadOf(key)
                    if (unread > 0) {
                        Surface(shape = CircleShape, color = Color(0xFFE53935)) {
                            Text(
                                if (unread > 99) "99+" else unread.toString(),
                                modifier = Modifier.padding(horizontal = 6.dp, vertical = 1.dp),
                                style = MaterialTheme.typography.labelSmall,
                                color = Color.White,
                            )
                        }
                    }
                }
            }
            HorizontalDivider(color = MaterialTheme.colorScheme.surfaceVariant)
        }
        // 末尾: グループの新規作成
        item(key = "new-group") {
            Row(
                modifier = Modifier
                    .fillMaxWidth()
                    .clickable { onNewGroup() }
                    .padding(horizontal = 12.dp, vertical = 14.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Icon(
                    Icons.Filled.Add,
                    contentDescription = null,
                    tint = MaterialTheme.colorScheme.primary,
                )
                Spacer(modifier = Modifier.width(8.dp))
                Text(
                    stringResource(R.string.talk_new_group),
                    color = MaterialTheme.colorScheme.primary,
                    style = MaterialTheme.typography.titleSmall,
                )
            }
        }
    }
}

/** コピー候補(ラベルと値)。行タップ時のボトムシートに並べる。 */
private data class CopyItem(val label: String, val value: String)

/** コピー候補を選ぶボトムシート。候補が 1 つでも値の確認を兼ねて表示する。
 *  `url` があれば「ブラウザで開く」も出す。 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun CopySheet(
    title: String,
    items: List<CopyItem>,
    url: String? = null,
    onCopy: (String) -> Unit,
    onOpenUrl: (String) -> Unit = {},
    onDismiss: () -> Unit,
) {
    ModalBottomSheet(onDismissRequest = onDismiss) {
        Text(
            title,
            style = MaterialTheme.typography.titleSmall,
            modifier = Modifier.padding(horizontal = 20.dp, vertical = 4.dp),
        )
        items.forEach { item ->
            Column(
                modifier = Modifier
                    .fillMaxWidth()
                    .clickable {
                        onCopy(item.value)
                        onDismiss()
                    }
                    .padding(horizontal = 20.dp, vertical = 10.dp),
            ) {
                Text(
                    item.label,
                    style = MaterialTheme.typography.labelSmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
                Text(item.value, style = MaterialTheme.typography.bodyMedium)
            }
        }
        url?.let { link ->
            Column(
                modifier = Modifier
                    .fillMaxWidth()
                    .clickable {
                        onOpenUrl(link)
                        onDismiss()
                    }
                    .padding(horizontal = 20.dp, vertical = 10.dp),
            ) {
                Text(
                    stringResource(R.string.open_in_browser),
                    style = MaterialTheme.typography.labelSmall,
                    color = MaterialTheme.colorScheme.primary,
                )
                Text(
                    link,
                    style = MaterialTheme.typography.bodyMedium,
                    color = MaterialTheme.colorScheme.primary,
                )
            }
        }
        Spacer(modifier = Modifier.size(24.dp))
    }
}

@Composable
private fun MemberList(memberList: List<MemberInfo>, onCopy: (String) -> Unit) {
    var sheetFor by remember { mutableStateOf<MemberInfo?>(null) }
    sheetFor?.let { member ->
        CopySheet(
            title = member.name,
            items = listOf(
                CopyItem(stringResource(R.string.copy_ip), member.ip),
                CopyItem(stringResource(R.string.copy_domain), member.fqdn),
            ),
            onCopy = onCopy,
            onDismiss = { sheetFor = null },
        )
    }
    LazyColumn {
        items(memberList, key = { it.ip }) { member ->
            Row(
                modifier = Modifier
                    .fillMaxWidth()
                    .clickable {
                        // ドメインが無いメンバーは IP 即コピー、あれば選択シート
                        if (member.fqdn.isEmpty()) onCopy(member.ip) else sheetFor = member
                    }
                    .padding(horizontal = 12.dp, vertical = 10.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Box(
                    modifier = Modifier
                        .size(36.dp)
                        .background(avatarColor(member.ip), CircleShape),
                    contentAlignment = Alignment.Center,
                ) {
                    Text(member.name.take(1), color = Color.White)
                }
                Spacer(modifier = Modifier.width(10.dp))
                Column(modifier = Modifier.weight(1f)) {
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        Text(member.name, style = MaterialTheme.typography.titleSmall)
                        if (member.isSelf) {
                            Badge(stringResource(R.string.badge_self))
                        }
                        if (member.isHost) {
                            Badge(stringResource(R.string.badge_host))
                        }
                        if (member.blocked) {
                            Badge(stringResource(R.string.badge_blocked))
                        }
                        member.appVersion?.let { Badge("v$it") }
                    }
                    Text(
                        member.ip + if (member.fqdn.isNotEmpty()) " ・${member.fqdn}" else "",
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
                Box(
                    modifier = Modifier
                        .size(10.dp)
                        .background(
                            if (member.online) Color(0xFF4CAF50) else Color(0xFFBDBDBD),
                            CircleShape,
                        ),
                )
            }
            HorizontalDivider(color = MaterialTheme.colorScheme.surfaceVariant)
        }
    }
}

@Composable
private fun Badge(text: String) {
    Spacer(modifier = Modifier.width(6.dp))
    Surface(
        shape = CircleShape,
        color = MaterialTheme.colorScheme.secondaryContainer,
    ) {
        Text(
            text,
            modifier = Modifier.padding(horizontal = 6.dp, vertical = 1.dp),
            style = MaterialTheme.typography.labelSmall,
        )
    }
}

/** トンネル内 DNS の一覧(メンバー名 + カスタムレコード)。
 *  スマホは OS の DNS をホストへ向けない(ADR-0040)ため、名前と IP の
 *  対応表 + IP 直接の URL を提供する。 */
@Composable
private fun DnsList(
    memberList: List<MemberInfo>,
    dnsList: List<DnsEntry>,
    onCopy: (String) -> Unit,
    onNotice: (String) -> Unit,
) {
    val context = LocalContext.current
    val copyDomain = stringResource(R.string.copy_domain)
    val copyValue = stringResource(R.string.copy_value)
    val openFailed = stringResource(R.string.open_failed)
    // (シートのタイトル, コピー候補, URL)。null なら非表示
    var sheetFor by remember {
        mutableStateOf<Triple<String, List<CopyItem>, String?>?>(null)
    }
    sheetFor?.let { (title, items, url) ->
        CopySheet(
            title = title,
            items = items,
            url = url,
            onCopy = onCopy,
            onOpenUrl = { link ->
                try {
                    context.startActivity(Intent(Intent.ACTION_VIEW, Uri.parse(link)))
                } catch (_: Exception) {
                    onNotice(openFailed)
                }
            },
            onDismiss = { sheetFor = null },
        )
    }
    fun open(fqdn: String, value: String, url: String?) {
        sheetFor = Triple(
            fqdn,
            listOf(CopyItem(copyDomain, fqdn), CopyItem(copyValue, value)),
            url,
        )
    }
    LazyColumn {
        items(
            memberList.filter { it.fqdn.isNotEmpty() },
            key = { "m-" + it.ip },
        ) { member ->
            DnsRow(member.fqdn, member.ip, null, onTap = ::open)
        }
        items(dnsList, key = { "r-" + it.fqdn + it.value }) { entry ->
            DnsRow(entry.fqdn, entry.value, entry.url, onTap = ::open)
        }
    }
}

@Composable
private fun DnsRow(
    fqdn: String,
    value: String,
    url: String?,
    onTap: (String, String, String?) -> Unit,
) {
    Column(
        modifier = Modifier
            .fillMaxWidth()
            // タップでコピー候補(ドメイン / IP / URL)の選択シートを開く
            .clickable { onTap(fqdn, value, url) }
            .padding(horizontal = 12.dp, vertical = 8.dp),
    ) {
        Text(fqdn, style = MaterialTheme.typography.titleSmall)
        Text(
            value + (url?.let { " ・$it" } ?: "") + stringResource(R.string.tap_to_copy),
            style = MaterialTheme.typography.bodySmall,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
    }
    HorizontalDivider(color = MaterialTheme.colorScheme.surfaceVariant)
}

/** 診断の 1 行(ラベル: 値)。 */
@Composable
private fun DiagRow(label: String, value: String, ok: Boolean? = null) {
    Row(modifier = Modifier.fillMaxWidth().padding(vertical = 2.dp)) {
        Text(
            label,
            modifier = Modifier.width(140.dp),
            style = MaterialTheme.typography.bodySmall,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
        Text(
            value,
            style = MaterialTheme.typography.bodySmall,
            color = when (ok) {
                true -> MaterialTheme.colorScheme.primary
                false -> MaterialTheme.colorScheme.error
                null -> MaterialTheme.colorScheme.onSurface
            },
        )
    }
}

/** 設定タブ: 接続診断 + デスクトップのメンバー設定と同等(接続先・MTU・
 *  受信上限 + 表示名・DNS 名はホストへ依頼)。 */
@Composable
private fun SettingsTab(
    slug: String,
    state: SessionState?,
    tunnel: TunnelStatus?,
    onNotice: (String) -> Unit,
) {
    val context = LocalContext.current
    val baseDir = context.filesDir.absolutePath
    val scope = rememberCoroutineScope()
    var endpointText by remember { mutableStateOf("") }
    var mtuText by remember { mutableStateOf("") }
    var limitText by remember { mutableStateOf("") }
    var displayName by remember { mutableStateOf("") }
    var dnsName by remember { mutableStateOf("") }
    var keyRotated by remember { mutableStateOf(true) }
    var busy by remember { mutableStateOf(false) }
    val failed = stringResource(R.string.failed_generic)
    val savedRestart = stringResource(R.string.settings_saved_restart)
    val saved = stringResource(R.string.settings_saved)
    val keyRotateDone = stringResource(R.string.settings_key_rotate_done)

    LaunchedEffect(slug) {
        val info = withContext(Dispatchers.IO) {
            listNetworks(baseDir).firstOrNull { it.slug == slug }
        }
        if (info != null) {
            endpointText = info.endpoint
            mtuText = info.mtu.toString()
            limitText = info.maxRecvFileMb.toString()
            displayName = info.displayName
            keyRotated = info.keyRotated
        }
    }

    fun run(action: suspend () -> String) {
        if (busy) return
        busy = true
        scope.launch {
            try {
                onNotice(withContext(Dispatchers.IO) { action() })
            } catch (e: MobileException) {
                onNotice(e.message ?: failed)
            } finally {
                busy = false
            }
        }
    }

    Column(
        modifier = Modifier
            .fillMaxSize()
            .verticalScroll(rememberScrollState())
            .padding(12.dp),
    ) {
        // 簡易接続診断: ポーリング済みの状態(トンネル + コントロール)を一望する
        Text(
            stringResource(R.string.settings_diag_title),
            style = MaterialTheme.typography.titleMedium,
        )
        Spacer(modifier = Modifier.padding(2.dp))
        val none = stringResource(R.string.diag_none)
        DiagRow(
            stringResource(R.string.diag_tunnel),
            when {
                tunnel == null -> stringResource(R.string.diag_tunnel_down)
                tunnel.handshakeAgeSecs == null -> stringResource(R.string.diag_tunnel_trying)
                else -> stringResource(
                    R.string.diag_tunnel_up,
                    tunnel.handshakeAgeSecs?.toLong() ?: 0L,
                )
            },
            ok = when {
                tunnel == null -> false
                tunnel.handshakeAgeSecs == null -> null
                else -> true
            },
        )
        DiagRow(
            stringResource(R.string.diag_endpoint),
            tunnel?.endpoint?.ifEmpty { none } ?: none,
        )
        DiagRow(
            stringResource(R.string.diag_traffic),
            tunnel?.let {
                stringResource(
                    R.string.diag_traffic_value,
                    formatBytesLong(it.txBytes),
                    formatBytesLong(it.rxBytes),
                )
            } ?: none,
        )
        DiagRow(
            stringResource(R.string.diag_control),
            if (state?.controlConnected == true) {
                stringResource(R.string.diag_control_ok)
            } else {
                stringResource(R.string.diag_control_waiting)
            },
            ok = state?.controlConnected == true,
        )
        DiagRow(
            stringResource(R.string.diag_rtt),
            state?.rttMs?.let { stringResource(R.string.diag_rtt_value, it.toLong()) } ?: none,
        )

        HorizontalDivider(modifier = Modifier.padding(vertical = 12.dp))

        Text(
            stringResource(R.string.settings_conn_title),
            style = MaterialTheme.typography.titleMedium,
        )
        OutlinedTextField(
            value = endpointText,
            onValueChange = { endpointText = it },
            modifier = Modifier.fillMaxWidth(),
            label = { Text(stringResource(R.string.settings_endpoint_label)) },
            singleLine = true,
        )
        OutlinedTextField(
            value = mtuText,
            onValueChange = { mtuText = it.filter { c -> c.isDigit() } },
            modifier = Modifier.fillMaxWidth(),
            label = { Text(stringResource(R.string.settings_mtu_label)) },
            keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Number),
            singleLine = true,
        )
        OutlinedTextField(
            value = limitText,
            onValueChange = { limitText = it.filter { c -> c.isDigit() } },
            modifier = Modifier.fillMaxWidth(),
            label = { Text(stringResource(R.string.settings_limit_label)) },
            keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Number),
            singleLine = true,
        )
        Spacer(modifier = Modifier.padding(2.dp))
        Button(
            enabled = !busy && endpointText.isNotBlank() && mtuText.isNotEmpty() && limitText.isNotEmpty(),
            onClick = {
                run {
                    val restart = updateNetworkSettings(
                        baseDir,
                        slug,
                        endpointText,
                        mtuText.toUShortOrNull() ?: 1420u,
                        limitText.toULongOrNull() ?: 10uL,
                    )
                    if (restart) savedRestart else saved
                }
            },
        ) { Text(stringResource(R.string.action_save)) }

        HorizontalDivider(modifier = Modifier.padding(vertical = 12.dp))

        Text(
            stringResource(R.string.settings_profile_title),
            style = MaterialTheme.typography.titleMedium,
        )
        Text(
            stringResource(R.string.settings_profile_hint),
            style = MaterialTheme.typography.bodySmall,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
        Spacer(modifier = Modifier.padding(2.dp))
        OutlinedTextField(
            value = displayName,
            onValueChange = { displayName = it },
            modifier = Modifier.fillMaxWidth(),
            label = { Text(stringResource(R.string.settings_display_name_label)) },
            singleLine = true,
        )
        Spacer(modifier = Modifier.padding(2.dp))
        Button(
            enabled = !busy && displayName.isNotBlank(),
            onClick = { run { setDisplayName(baseDir, slug, displayName) } },
        ) { Text(stringResource(R.string.settings_display_name_submit)) }

        Spacer(modifier = Modifier.padding(6.dp))
        OutlinedTextField(
            value = dnsName,
            onValueChange = { dnsName = it },
            modifier = Modifier.fillMaxWidth(),
            label = { Text(stringResource(R.string.settings_dns_name_label)) },
            singleLine = true,
        )
        Spacer(modifier = Modifier.padding(2.dp))
        Button(
            enabled = !busy && dnsName.isNotBlank(),
            onClick = { run { setDnsName(slug, dnsName) } },
        ) { Text(stringResource(R.string.settings_dns_name_submit)) }

        HorizontalDivider(modifier = Modifier.padding(vertical = 12.dp))

        // デバイス鍵(ADR-0044)。更新後は新しい鍵での接続し直しが必要なので
        // サービスへ再接続を依頼する
        Text(
            stringResource(R.string.settings_key_title),
            style = MaterialTheme.typography.titleMedium,
        )
        Text(
            if (keyRotated) {
                stringResource(R.string.settings_key_rotated)
            } else {
                stringResource(R.string.settings_key_from_invite)
            },
            style = MaterialTheme.typography.bodySmall,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
        Spacer(modifier = Modifier.padding(2.dp))
        Button(
            enabled = !busy,
            onClick = {
                run {
                    rotateKey(baseDir, slug)
                    keyRotated = true
                    startVpnService(context, slug) // 新しい鍵で入れ直し
                    keyRotateDone
                }
            },
        ) { Text(stringResource(R.string.settings_key_rotate)) }
    }
}
