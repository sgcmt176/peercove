package app.peercove.android

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
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
import androidx.compose.material3.Button
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Surface
import androidx.compose.material3.Tab
import androidx.compose.material3.TabRow
import androidx.compose.material3.Text
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalClipboardManager
import androidx.compose.ui.platform.LocalContext
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
import uniffi.peercove_mobile.chatFetch
import uniffi.peercove_mobile.chatGroups
import uniffi.peercove_mobile.dnsEntries
import uniffi.peercove_mobile.listNetworks
import uniffi.peercove_mobile.members
import uniffi.peercove_mobile.sessionState
import uniffi.peercove_mobile.setDisplayName
import uniffi.peercove_mobile.setDnsName
import uniffi.peercove_mobile.setRecvLimitMb

/**
 * ネットワーク詳細(接続中のトーク / メンバー / DNS)。
 * 状態はすべて Rust 側が正本で、2 秒(チャットは 1.5 秒)のポーリングで映す。
 */
@Composable
fun NetworkScreen(slug: String, networkName: String, onBack: () -> Unit, onNotice: (String) -> Unit) {
    var tab by remember { mutableStateOf(0) }
    var state by remember { mutableStateOf<SessionState?>(null) }
    var memberList by remember { mutableStateOf<List<MemberInfo>>(emptyList()) }
    var groupList by remember { mutableStateOf<List<GroupSummary>>(emptyList()) }
    var dnsList by remember { mutableStateOf<List<DnsEntry>>(emptyList()) }
    var messages by remember { mutableStateOf<List<ChatMessage>>(emptyList()) }
    var conv by remember { mutableStateOf<ConvKey?>(null) }
    val clipboard = LocalClipboardManager.current

    // 会話を開いているときのシステム戻る操作はトーク一覧へ(アプリを閉じない)
    BackHandler(enabled = conv != null) { conv = null }

    fun copy(text: String) {
        clipboard.setText(AnnotatedString(text))
        onNotice("コピーしました: $text")
    }

    // セッション情報のポーリング
    LaunchedEffect(slug) {
        while (true) {
            withContext(Dispatchers.IO) {
                state = sessionState(slug)
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

    Column(modifier = Modifier.fillMaxSize()) {
        // ヘッダ
        Row(verticalAlignment = Alignment.CenterVertically) {
            IconButton(onClick = { if (conv != null) conv = null else onBack() }) {
                Icon(Icons.AutoMirrored.Filled.ArrowBack, contentDescription = "戻る")
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
            listOf("トーク", "メンバー", "DNS", "設定").forEachIndexed { index, title ->
                Tab(selected = tab == index, onClick = { tab = index }, text = { Text(title) })
            }
        }
        when (tab) {
            0 -> TalkList(memberList, groupList, messages) { conv = it }
            1 -> MemberList(memberList, onCopy = ::copy)
            2 -> DnsList(memberList, dnsList, onCopy = ::copy)
            else -> SettingsTab(slug, onNotice)
        }
    }
}

private fun statusText(state: SessionState?): String = when {
    state == null -> "未接続(ホームから接続してください)"
    state.removed -> "ホストから削除されました"
    state.rejected != null -> "参加が拒否されました: ${state.rejected}"
    state.controlConnected ->
        "同期中" + (state.rttMs?.let { " ・RTT ${it}ms" } ?: "")
    else -> "ホストと同期待ち…"
}

@Composable
private fun statusColor(state: SessionState?): Color = when {
    state == null -> MaterialTheme.colorScheme.onSurfaceVariant
    state.removed || state.rejected != null -> MaterialTheme.colorScheme.error
    state.controlConnected -> MaterialTheme.colorScheme.primary
    else -> MaterialTheme.colorScheme.onSurfaceVariant
}

/** LINE 風のトーク一覧: 全体 / グループ / メンバー(1:1)。 */
@Composable
private fun TalkList(
    memberList: List<MemberInfo>,
    groupList: List<GroupSummary>,
    messages: List<ChatMessage>,
    onOpen: (ConvKey) -> Unit,
) {
    val conversations = buildList {
        add(ConvKey.Network)
        groupList.forEach { add(ConvKey.Group(it.id, it.name)) }
        memberList.filter { !it.isSelf }.forEach { add(ConvKey.Direct(it.ip, it.name)) }
    }
    LazyColumn {
        items(conversations, key = { it.title() + it.hashCode() }) { key ->
            val last = messages.lastOrNull { it.belongsTo(key) }
            val online = when (key) {
                is ConvKey.Direct -> memberList.firstOrNull { it.ip == key.ip }?.online == true
                else -> true
            }
            Row(
                modifier = Modifier
                    .fillMaxWidth()
                    .clickable { onOpen(key) }
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
                            is ConvKey.Network -> "全"
                            is ConvKey.Group -> "G"
                            is ConvKey.Direct -> key.name.take(1)
                        },
                        color = Color.White,
                    )
                }
                Spacer(modifier = Modifier.width(10.dp))
                Column(modifier = Modifier.weight(1f)) {
                    Row(verticalAlignment = Alignment.CenterVertically) {
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
                            last == null -> "メッセージはまだありません"
                            last.fileName != null -> "📎 ${last.fileName}"
                            else -> last.text
                        },
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                        maxLines = 1,
                    )
                }
                last?.let {
                    Text(
                        formatTime(it.sentAt),
                        style = MaterialTheme.typography.labelSmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
            }
            HorizontalDivider(color = MaterialTheme.colorScheme.surfaceVariant)
        }
    }
}

@Composable
private fun MemberList(memberList: List<MemberInfo>, onCopy: (String) -> Unit) {
    LazyColumn {
        items(memberList, key = { it.ip }) { member ->
            Row(
                modifier = Modifier
                    .fillMaxWidth()
                    .clickable { onCopy(member.ip) } // タップで仮想 IP をコピー
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
                            Badge("自分")
                        }
                        if (member.isHost) {
                            Badge("ホスト")
                        }
                        if (member.blocked) {
                            Badge("通信不可")
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
) {
    LazyColumn {
        items(
            memberList.filter { it.fqdn.isNotEmpty() },
            key = { "m-" + it.ip },
        ) { member ->
            DnsRow(member.fqdn, member.ip, null, onCopy)
        }
        items(dnsList, key = { "r-" + it.fqdn + it.value }) { entry ->
            DnsRow(entry.fqdn, entry.value, entry.url, onCopy)
        }
    }
}

@Composable
private fun DnsRow(fqdn: String, value: String, url: String?, onCopy: (String) -> Unit) {
    Column(
        modifier = Modifier
            .fillMaxWidth()
            // タップで接続 URL(あれば)か IP をコピー
            .clickable { onCopy(url ?: value) }
            .padding(horizontal = 12.dp, vertical = 8.dp),
    ) {
        Text(fqdn, style = MaterialTheme.typography.titleSmall)
        Text(
            value + (url?.let { " ・$it" } ?: "") + " ・タップでコピー",
            style = MaterialTheme.typography.bodySmall,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
    }
    HorizontalDivider(color = MaterialTheme.colorScheme.surfaceVariant)
}

/** 設定タブ: 受信サイズ上限(端末ローカル)と表示名・DNS 名(ホストへ依頼)。 */
@Composable
private fun SettingsTab(slug: String, onNotice: (String) -> Unit) {
    val context = LocalContext.current
    val baseDir = context.filesDir.absolutePath
    val scope = rememberCoroutineScope()
    var limitText by remember { mutableStateOf("") }
    var displayName by remember { mutableStateOf("") }
    var dnsName by remember { mutableStateOf("") }
    var busy by remember { mutableStateOf(false) }

    LaunchedEffect(slug) {
        val info = withContext(Dispatchers.IO) {
            listNetworks(baseDir).firstOrNull { it.slug == slug }
        }
        if (info != null) {
            limitText = info.maxRecvFileMb.toString()
            displayName = info.displayName
        }
    }

    fun run(action: suspend () -> String) {
        if (busy) return
        busy = true
        scope.launch {
            try {
                onNotice(withContext(Dispatchers.IO) { action() })
            } catch (e: MobileException) {
                onNotice(e.message ?: "失敗しました")
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
        Text("ファイル受信", style = MaterialTheme.typography.titleMedium)
        OutlinedTextField(
            value = limitText,
            onValueChange = { limitText = it.filter { c -> c.isDigit() } },
            modifier = Modifier.fillMaxWidth(),
            label = { Text("受信サイズ上限(MB、0 で無制限)") },
            keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Number),
            singleLine = true,
        )
        Spacer(modifier = Modifier.padding(2.dp))
        Button(
            enabled = !busy && limitText.isNotEmpty(),
            onClick = {
                run {
                    setRecvLimitMb(baseDir, slug, limitText.toULongOrNull() ?: 10uL)
                    "受信サイズ上限を保存しました(${limitText} MB)"
                }
            },
        ) { Text("保存") }

        HorizontalDivider(modifier = Modifier.padding(vertical = 12.dp))

        Text("プロフィール", style = MaterialTheme.typography.titleMedium)
        Text(
            "ホストへ依頼して変更します(「同期中」のときのみ)",
            style = MaterialTheme.typography.bodySmall,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
        Spacer(modifier = Modifier.padding(2.dp))
        OutlinedTextField(
            value = displayName,
            onValueChange = { displayName = it },
            modifier = Modifier.fillMaxWidth(),
            label = { Text("表示名") },
            singleLine = true,
        )
        Spacer(modifier = Modifier.padding(2.dp))
        Button(
            enabled = !busy && displayName.isNotBlank(),
            onClick = { run { setDisplayName(baseDir, slug, displayName) } },
        ) { Text("表示名を変更") }

        Spacer(modifier = Modifier.padding(6.dp))
        OutlinedTextField(
            value = dnsName,
            onValueChange = { dnsName = it },
            modifier = Modifier.fillMaxWidth(),
            label = { Text("DNS 名(例: my-phone)") },
            singleLine = true,
        )
        Spacer(modifier = Modifier.padding(2.dp))
        Button(
            enabled = !busy && dnsName.isNotBlank(),
            onClick = { run { setDnsName(slug, dnsName) } },
        ) { Text("DNS 名を変更") }
    }
}
