package app.peercove.android

import android.net.Uri
import android.widget.Toast
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.Send
import androidx.compose.material.icons.filled.Add
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
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import uniffi.peercove_mobile.ChatMessage
import uniffi.peercove_mobile.MemberInfo
import uniffi.peercove_mobile.MobileException
import uniffi.peercove_mobile.sendChatMessage
import uniffi.peercove_mobile.sendFileTo
import java.text.SimpleDateFormat
import java.util.Date
import java.util.Locale

/** トーク(会話)の種別。 */
sealed class ConvKey {
    data object Network : ConvKey()
    data class Direct(val ip: String, val name: String) : ConvKey()
    data class Group(val id: String, val name: String) : ConvKey()
}

fun ConvKey.title(): String = when (this) {
    is ConvKey.Network -> "全体"
    is ConvKey.Direct -> name
    is ConvKey.Group -> name
}

/** この会話に属するメッセージか。 */
fun ChatMessage.belongsTo(key: ConvKey): Boolean = when (key) {
    is ConvKey.Network -> scope == "network"
    is ConvKey.Direct -> scope == "direct" &&
        (if (outgoing) toIp == key.ip else fromIp == key.ip)
    is ConvKey.Group -> scope == "group" && groupId == key.id
}

private val timeFormat = SimpleDateFormat("HH:mm", Locale.JAPAN)

fun formatTime(sentAtMs: ULong): String = timeFormat.format(Date(sentAtMs.toLong()))

/** IP から決定的にアバター色を選ぶ(デスクトップの色分けと同じ発想)。 */
fun avatarColor(ip: String): Color {
    val palette = listOf(
        Color(0xFF7E57C2), Color(0xFF26A69A), Color(0xFFEF5350), Color(0xFF42A5F5),
        Color(0xFFFFA726), Color(0xFF66BB6A), Color(0xFFEC407A), Color(0xFF8D6E63),
    )
    return palette[Math.floorMod(ip.hashCode(), palette.size)]
}

/** LINE 風の会話画面。 */
@Composable
fun ConversationScreen(
    slug: String,
    conv: ConvKey,
    messages: List<ChatMessage>,
    members: List<MemberInfo>,
    onNotice: (String) -> Unit,
) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    var input by remember { mutableStateOf("") }
    var sending by remember { mutableStateOf(false) }
    val convMessages = remember(messages, conv) { messages.filter { it.belongsTo(conv) } }
    val listState = rememberLazyListState()

    // 新着で最下部へ
    LaunchedEffect(convMessages.size) {
        if (convMessages.isNotEmpty()) {
            listState.scrollToItem(convMessages.size - 1)
        }
    }

    fun doSend() {
        val text = input.trim()
        if (text.isEmpty() || sending) return
        sending = true
        scope.launch {
            try {
                withContext(Dispatchers.IO) {
                    when (conv) {
                        is ConvKey.Network ->
                            sendChatMessage(slug, "network", null, null, text)
                        is ConvKey.Direct ->
                            sendChatMessage(slug, "direct", conv.ip, null, text)
                        is ConvKey.Group ->
                            sendChatMessage(slug, "group", null, conv.id, text)
                    }
                }
                input = ""
            } catch (e: MobileException) {
                onNotice(e.message ?: "送信に失敗しました")
            } finally {
                sending = false
            }
        }
    }

    // 添付(1:1 のみ。ファイルは相手 1 人へ送る)
    val pickFile = rememberLauncherForActivityResult(
        ActivityResultContracts.GetContent(),
    ) { uri: Uri? ->
        val target = conv as? ConvKey.Direct ?: return@rememberLauncherForActivityResult
        if (uri == null) return@rememberLauncherForActivityResult
        scope.launch {
            onNotice("ファイルを送信中…")
            val result = withContext(Dispatchers.IO) {
                val cached = FileUtil.copyToCache(context, uri)
                    ?: return@withContext "ファイルを読み込めませんでした"
                try {
                    sendFileTo(slug, target.ip, cached.absolutePath)
                    null
                } catch (e: MobileException) {
                    e.message ?: "送信に失敗しました"
                } finally {
                    cached.delete()
                }
            }
            onNotice(result ?: "ファイルを送信しました")
        }
    }

    Column(modifier = Modifier.fillMaxSize()) {
        LazyColumn(
            state = listState,
            modifier = Modifier.weight(1f).fillMaxWidth().padding(horizontal = 8.dp),
            verticalArrangement = Arrangement.spacedBy(4.dp),
        ) {
            items(convMessages, key = { it.seq }) { message ->
                when {
                    message.system -> SystemLine(message.text)
                    message.outgoing -> OutgoingBubble(message, context, onNotice)
                    else -> IncomingBubble(
                        message,
                        showName = conv !is ConvKey.Direct,
                        context = context,
                        onNotice = onNotice,
                    )
                }
            }
        }
        // 入力バー
        Row(
            modifier = Modifier.fillMaxWidth().padding(8.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            if (conv is ConvKey.Direct) {
                IconButton(onClick = { pickFile.launch("*/*") }) {
                    Icon(Icons.Filled.Add, contentDescription = "ファイルを送る")
                }
            }
            OutlinedTextField(
                value = input,
                onValueChange = { input = it },
                modifier = Modifier.weight(1f),
                placeholder = { Text("メッセージを入力") },
                maxLines = 4,
            )
            IconButton(onClick = { doSend() }, enabled = !sending && input.isNotBlank()) {
                Icon(
                    Icons.AutoMirrored.Filled.Send,
                    contentDescription = "送信",
                    tint = if (input.isNotBlank()) MaterialTheme.colorScheme.primary
                    else MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
        }
    }
}

@Composable
private fun SystemLine(text: String) {
    Box(modifier = Modifier.fillMaxWidth().padding(vertical = 4.dp)) {
        Surface(
            modifier = Modifier.align(Alignment.Center),
            shape = RoundedCornerShape(12.dp),
            color = MaterialTheme.colorScheme.surfaceVariant,
        ) {
            Text(
                text,
                modifier = Modifier.padding(horizontal = 10.dp, vertical = 4.dp),
                style = MaterialTheme.typography.labelSmall,
                textAlign = TextAlign.Center,
            )
        }
    }
}

@Composable
private fun IncomingBubble(
    message: ChatMessage,
    showName: Boolean,
    context: android.content.Context,
    onNotice: (String) -> Unit,
) {
    Row(modifier = Modifier.fillMaxWidth(), verticalAlignment = Alignment.Top) {
        // アバター(名前の頭文字)
        Box(
            modifier = Modifier
                .padding(top = if (showName) 18.dp else 2.dp)
                .size(32.dp)
                .background(avatarColor(message.fromIp), CircleShape),
            contentAlignment = Alignment.Center,
        ) {
            Text(
                message.fromName.take(1),
                color = Color.White,
                style = MaterialTheme.typography.labelMedium,
            )
        }
        Spacer(modifier = Modifier.width(6.dp))
        Column {
            if (showName) {
                Text(
                    message.fromName,
                    style = MaterialTheme.typography.labelSmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
            Row(verticalAlignment = Alignment.Bottom) {
                Bubble(
                    message = message,
                    container = MaterialTheme.colorScheme.surfaceVariant,
                    content = MaterialTheme.colorScheme.onSurface,
                    shape = RoundedCornerShape(4.dp, 16.dp, 16.dp, 16.dp),
                    context = context,
                    onNotice = onNotice,
                )
                Spacer(modifier = Modifier.width(4.dp))
                Text(
                    formatTime(message.sentAt),
                    style = MaterialTheme.typography.labelSmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
        }
    }
}

@Composable
private fun OutgoingBubble(
    message: ChatMessage,
    context: android.content.Context,
    onNotice: (String) -> Unit,
) {
    Row(
        modifier = Modifier.fillMaxWidth(),
        horizontalArrangement = Arrangement.End,
        verticalAlignment = Alignment.Bottom,
    ) {
        Column(horizontalAlignment = Alignment.End) {
            if (message.failed) {
                Text(
                    "送信できませんでした",
                    style = MaterialTheme.typography.labelSmall,
                    color = MaterialTheme.colorScheme.error,
                )
            }
        }
        Text(
            formatTime(message.sentAt),
            style = MaterialTheme.typography.labelSmall,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
        Spacer(modifier = Modifier.width(4.dp))
        // LINE 風: 自分の吹き出しは緑
        Bubble(
            message = message,
            container = Color(0xFF8DE055),
            content = Color(0xFF102A00),
            shape = RoundedCornerShape(16.dp, 4.dp, 16.dp, 16.dp),
            context = context,
            onNotice = onNotice,
        )
    }
}

@Composable
private fun Bubble(
    message: ChatMessage,
    container: Color,
    content: Color,
    shape: RoundedCornerShape,
    context: android.content.Context,
    onNotice: (String) -> Unit,
) {
    Surface(shape = shape, color = container) {
        if (message.fileName != null) {
            // ファイルバブル: タップでダウンロードへコピー(受信側)
            Column(
                modifier = Modifier
                    .widthIn(max = 260.dp)
                    .clickable(enabled = !message.outgoing && message.filePath != null) {
                        val path = message.filePath ?: return@clickable
                        val ok = FileUtil.copyToDownloads(context, path)
                        onNotice(
                            if (ok) "ダウンロード(PeerCove)に保存しました"
                            else "保存に失敗しました(受信ボックスから消えている可能性)",
                        )
                        if (ok) {
                            Toast.makeText(context, "保存しました", Toast.LENGTH_SHORT).show()
                        }
                    }
                    .padding(horizontal = 12.dp, vertical = 8.dp),
            ) {
                Text("📎 ${message.fileName}", color = content)
                Text(
                    formatBytesLong(message.fileSize ?: 0u) +
                        if (!message.outgoing) " ・タップで保存" else "",
                    style = MaterialTheme.typography.labelSmall,
                    color = content.copy(alpha = 0.7f),
                )
            }
        } else {
            Text(
                message.text,
                modifier = Modifier
                    .widthIn(max = 260.dp)
                    .padding(horizontal = 12.dp, vertical = 8.dp),
                color = content,
            )
        }
    }
}

fun formatBytesLong(bytes: ULong): String {
    val b = bytes.toLong()
    return when {
        b >= 1_048_576 -> "%.1f MB".format(b / 1_048_576.0)
        b >= 1_024 -> "%.1f KB".format(b / 1_024.0)
        else -> "$b B"
    }
}
