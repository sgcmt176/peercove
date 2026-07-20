package app.peercove.android

import android.net.Uri
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
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
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
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import uniffi.peercove_mobile.MemberInfo
import uniffi.peercove_mobile.MobileException
import uniffi.peercove_mobile.NetworkInfo
import uniffi.peercove_mobile.listNetworks
import uniffi.peercove_mobile.members
import uniffi.peercove_mobile.sendFileTo
import uniffi.peercove_mobile.tunnelStatus

/**
 * 共有シート(ACTION_SEND)で受け取ったファイルの送信先を選ぶ画面。
 * 接続中のネットワーク → オンラインのメンバーの順に選び、タップで送る。
 */
@Composable
fun ShareSendScreen(uri: Uri, onNotice: (String) -> Unit, onClose: () -> Unit) {
    val context = LocalContext.current
    val baseDir = context.filesDir.absolutePath
    val scope = rememberCoroutineScope()

    var running by remember { mutableStateOf<List<NetworkInfo>>(emptyList()) }
    var selected by remember { mutableStateOf<NetworkInfo?>(null) }
    var memberList by remember { mutableStateOf<List<MemberInfo>>(emptyList()) }
    var loaded by remember { mutableStateOf(false) }
    var sending by remember { mutableStateOf(false) }
    val sendingMsg = stringResource(R.string.share_sending)
    val sentMsg = stringResource(R.string.share_sent)
    val readFailed = stringResource(R.string.share_read_failed)

    LaunchedEffect(Unit) {
        val list = withContext(Dispatchers.IO) {
            listNetworks(baseDir).filter { tunnelStatus(it.slug) != null }
        }
        running = list
        if (list.size == 1) selected = list.first()
        loaded = true
    }
    LaunchedEffect(selected) {
        val net = selected ?: return@LaunchedEffect
        memberList = withContext(Dispatchers.IO) {
            members(net.slug).filter { it.online && !it.isSelf && !it.blocked }
        }
    }

    fun send(target: MemberInfo) {
        val net = selected ?: return
        if (sending) return
        sending = true
        scope.launch {
            onNotice(sendingMsg)
            val error = withContext(Dispatchers.IO) {
                val cached = FileUtil.copyToCache(context, uri)
                    ?: return@withContext readFailed
                try {
                    sendFileTo(net.slug, target.ip, cached.absolutePath)
                    null
                } catch (e: MobileException) {
                    e.message
                } finally {
                    cached.delete()
                }
            }
            onNotice(error ?: sentMsg)
            sending = false
            if (error == null) onClose()
        }
    }

    Column(modifier = Modifier.fillMaxSize().padding(16.dp)) {
        Text(stringResource(R.string.share_title), style = MaterialTheme.typography.titleMedium)
        Spacer(modifier = Modifier.padding(4.dp))
        when {
            !loaded -> {}
            running.isEmpty() -> Text(
                stringResource(R.string.share_no_network),
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            selected == null -> {
                Text(
                    stringResource(R.string.share_pick_network),
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
                LazyColumn {
                    items(running, key = { it.slug }) { net ->
                        Text(
                            net.name,
                            modifier = Modifier
                                .fillMaxWidth()
                                .clickable { selected = net }
                                .padding(vertical = 12.dp),
                            style = MaterialTheme.typography.titleSmall,
                        )
                        HorizontalDivider(color = MaterialTheme.colorScheme.surfaceVariant)
                    }
                }
            }
            else -> {
                Text(
                    stringResource(R.string.share_pick_member),
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
                if (memberList.isEmpty()) {
                    Spacer(modifier = Modifier.padding(4.dp))
                    Text(
                        stringResource(R.string.share_no_member),
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
                LazyColumn(modifier = Modifier.weight(1f)) {
                    items(memberList, key = { it.ip }) { member ->
                        Row(
                            modifier = Modifier
                                .fillMaxWidth()
                                .clickable(enabled = !sending) { send(member) }
                                .padding(vertical = 10.dp),
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
                            Column {
                                Text(member.name, style = MaterialTheme.typography.titleSmall)
                                Text(
                                    member.ip,
                                    style = MaterialTheme.typography.bodySmall,
                                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                                )
                            }
                        }
                        HorizontalDivider(color = MaterialTheme.colorScheme.surfaceVariant)
                    }
                }
            }
        }
        TextButton(onClick = onClose) { Text(stringResource(R.string.action_cancel)) }
    }
}
