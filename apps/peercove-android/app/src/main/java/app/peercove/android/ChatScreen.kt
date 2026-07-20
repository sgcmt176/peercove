package app.peercove.android

import android.content.ActivityNotFoundException
import android.content.Intent
import android.graphics.BitmapFactory
import android.net.Uri
import android.widget.Toast
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.Image
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
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
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
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalClipboardManager
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.LinkAnnotation
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.TextLinkStyles
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextDecoration
import androidx.compose.ui.text.withLink
import androidx.compose.ui.unit.dp
import java.net.HttpURLConnection
import java.net.URL
import java.util.concurrent.ConcurrentHashMap
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import uniffi.peercove_mobile.ChatMessage
import uniffi.peercove_mobile.MemberInfo
import uniffi.peercove_mobile.MobileException
import uniffi.peercove_mobile.cancelChatSend
import uniffi.peercove_mobile.resendChat
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

@Composable
fun ConvKey.title(): String = when (this) {
    is ConvKey.Network -> stringResource(R.string.talk_all)
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
    val sendFailed = stringResource(R.string.chat_send_failed)
    val fileSending = stringResource(R.string.share_sending)
    val fileSent = stringResource(R.string.share_sent)
    val fileReadFailed = stringResource(R.string.share_read_failed)

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
                onNotice(e.message ?: sendFailed)
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
            onNotice(fileSending)
            val result = withContext(Dispatchers.IO) {
                val cached = FileUtil.copyToCache(context, uri)
                    ?: return@withContext fileReadFailed
                try {
                    sendFileTo(slug, target.ip, cached.absolutePath)
                    null // 成功時は残す(自分の画像サムネイルが参照する)
                } catch (e: MobileException) {
                    cached.delete()
                    e.message ?: sendFailed
                }
            }
            onNotice(result ?: fileSent)
        }
    }

    // キーボードの持ち上げはルートの safeDrawingPadding が担う(MainActivity)
    Column(modifier = Modifier.fillMaxSize()) {
        LazyColumn(
            state = listState,
            modifier = Modifier.weight(1f).fillMaxWidth().padding(horizontal = 8.dp),
            verticalArrangement = Arrangement.spacedBy(4.dp),
        ) {
            // key は Bundle 保存可能な型が必須(ULong のままだと実行時クラッシュ)
            items(convMessages, key = { it.seq.toLong() }) { message ->
                when {
                    message.system -> SystemLine(message.text)
                    message.outgoing -> OutgoingBubble(slug, message, onNotice)
                    else -> IncomingBubble(
                        message,
                        showName = conv !is ConvKey.Direct,
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
                    Icon(
                        Icons.Filled.Add,
                        contentDescription = stringResource(R.string.chat_attach),
                    )
                }
            }
            OutlinedTextField(
                value = input,
                onValueChange = { input = it },
                modifier = Modifier.weight(1f),
                placeholder = { Text(stringResource(R.string.chat_placeholder)) },
                maxLines = 4,
            )
            IconButton(onClick = { doSend() }, enabled = !sending && input.isNotBlank()) {
                Icon(
                    Icons.AutoMirrored.Filled.Send,
                    contentDescription = stringResource(R.string.chat_send),
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
    slug: String,
    message: ChatMessage,
    onNotice: (String) -> Unit,
) {
    val scope = rememberCoroutineScope()
    val resendFailed = stringResource(R.string.chat_send_failed)
    Row(
        modifier = Modifier.fillMaxWidth(),
        horizontalArrangement = Arrangement.End,
        verticalAlignment = Alignment.Bottom,
    ) {
        // 送信状態(E-E 3): 送信中 / 再送待ち(+取消) / 失敗(+再送)
        Column(horizontalAlignment = Alignment.End) {
            when {
                message.sending && !message.failed -> Text(
                    stringResource(R.string.chat_sending_state),
                    style = MaterialTheme.typography.labelSmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
                message.sending -> {
                    Text(
                        stringResource(R.string.chat_retrying),
                        style = MaterialTheme.typography.labelSmall,
                        color = MaterialTheme.colorScheme.error,
                    )
                    Text(
                        stringResource(R.string.chat_cancel_send),
                        modifier = Modifier
                            .clickable {
                                scope.launch {
                                    withContext(Dispatchers.IO) {
                                        cancelChatSend(slug, message.seq)
                                    }
                                }
                            }
                            .padding(2.dp),
                        style = MaterialTheme.typography.labelSmall,
                        color = MaterialTheme.colorScheme.primary,
                    )
                }
                message.failed -> {
                    Text(
                        stringResource(R.string.chat_failed_line),
                        style = MaterialTheme.typography.labelSmall,
                        color = MaterialTheme.colorScheme.error,
                    )
                    Text(
                        stringResource(R.string.chat_resend),
                        modifier = Modifier
                            .clickable {
                                scope.launch {
                                    try {
                                        withContext(Dispatchers.IO) {
                                            resendChat(slug, message.seq)
                                        }
                                    } catch (e: MobileException) {
                                        onNotice(e.message ?: resendFailed)
                                    }
                                }
                            }
                            .padding(2.dp),
                        style = MaterialTheme.typography.labelSmall,
                        color = MaterialTheme.colorScheme.primary,
                    )
                }
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
            onNotice = onNotice,
        )
    }
}

/** 画像として表示できる拡張子(BitmapFactory で読めるもの)。 */
private val imageExtensions = setOf("png", "jpg", "jpeg", "webp", "gif", "bmp")

private fun isImageFile(name: String?): Boolean =
    (name?.substringAfterLast('.', "")?.lowercase() ?: "") in imageExtensions

/** サムネイル用に縮小して読み込む(原寸ロードによるメモリ圧を避ける)。 */
private fun decodeSampled(path: String, maxDim: Int): android.graphics.Bitmap? {
    val bounds = BitmapFactory.Options().apply { inJustDecodeBounds = true }
    BitmapFactory.decodeFile(path, bounds)
    if (bounds.outWidth <= 0 || bounds.outHeight <= 0) return null
    var sample = 1
    while (bounds.outWidth / (sample * 2) >= maxDim || bounds.outHeight / (sample * 2) >= maxDim) {
        sample *= 2
    }
    return BitmapFactory.decodeFile(path, BitmapFactory.Options().apply { inSampleSize = sample })
}

@Composable
private fun Bubble(
    message: ChatMessage,
    container: Color,
    content: Color,
    shape: RoundedCornerShape,
    onNotice: (String) -> Unit,
) {
    // ファイルのタップ = 操作シート(開く / 共有 / 保存 / SHA-256)
    var showSheet by remember { mutableStateOf(false) }
    val fileTap: () -> Unit = { showSheet = true }
    if (showSheet && message.filePath != null) {
        FileActionSheet(message, onNotice) { showSheet = false }
    }
    Surface(shape = shape, color = container) {
        if (message.fileName != null) {
            // 画像はバブル内にサムネイル表示(受信済み or 手元にある場合)
            var thumbnail by remember(message.filePath) { mutableStateOf<ImageBitmap?>(null) }
            if (message.filePath != null && isImageFile(message.fileName)) {
                LaunchedEffect(message.filePath) {
                    thumbnail = withContext(Dispatchers.IO) {
                        message.filePath?.let { decodeSampled(it, 640)?.asImageBitmap() }
                    }
                }
            }
            val image = thumbnail
            if (image != null) {
                Column(
                    modifier = Modifier
                        .widthIn(max = 240.dp)
                        .clickable(enabled = message.filePath != null, onClick = fileTap)
                        .padding(4.dp),
                ) {
                    Image(
                        bitmap = image,
                        contentDescription = message.fileName,
                        modifier = Modifier.fillMaxWidth().clip(RoundedCornerShape(12.dp)),
                        contentScale = ContentScale.FillWidth,
                    )
                    Text(
                        (message.fileName ?: "") +
                            if (message.filePath != null) {
                                stringResource(R.string.chat_tap_to_save)
                            } else {
                                ""
                            },
                        modifier = Modifier.padding(horizontal = 6.dp, vertical = 2.dp),
                        style = MaterialTheme.typography.labelSmall,
                        color = content.copy(alpha = 0.7f),
                        maxLines = 1,
                    )
                }
            } else {
                // ファイルバブル: タップで操作シート(手元にファイルがある場合)
                Column(
                    modifier = Modifier
                        .widthIn(max = 260.dp)
                        .clickable(
                            enabled = message.filePath != null,
                            onClick = fileTap,
                        )
                        .padding(horizontal = 12.dp, vertical = 8.dp),
                ) {
                    Text(
                        stringResource(R.string.chat_file_prefix, message.fileName ?: ""),
                        color = content,
                    )
                    Text(
                        formatBytesLong(message.fileSize ?: 0u) +
                            if (message.filePath != null) {
                                stringResource(R.string.chat_tap_to_save)
                            } else {
                                ""
                            },
                        style = MaterialTheme.typography.labelSmall,
                        color = content.copy(alpha = 0.7f),
                    )
                }
            }
        } else {
            val urls = remember(message.text) {
                urlRegex.findAll(message.text).map { it.value }.toList()
            }
            Column(
                modifier = Modifier
                    .widthIn(max = 260.dp)
                    .padding(horizontal = 12.dp, vertical = 8.dp),
            ) {
                // URL はリンク化(タップでブラウザ)。無ければ素のテキスト
                if (urls.isEmpty()) {
                    Text(message.text, color = content)
                } else {
                    Text(linkifyText(message.text, content), color = content)
                    // 最初の URL のプレビューカード(タイトル / og:image)
                    LinkPreviewCard(urls.first(), content)
                }
            }
        }
    }
}

/** ファイルバブルのタップで出す操作シート: 開く / 共有 / 保存 / SHA-256。 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun FileActionSheet(
    message: ChatMessage,
    onNotice: (String) -> Unit,
    onDismiss: () -> Unit,
) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val clipboard = LocalClipboardManager.current
    val path = message.filePath ?: return
    val name = message.fileName ?: "file"
    val mime = remember(name) { FileUtil.mimeOf(name) }
    val savedMsg = stringResource(R.string.chat_saved)
    val saveFailedMsg = stringResource(R.string.chat_save_failed)
    val savedToast = stringResource(R.string.chat_saved_toast)
    val openFailed = stringResource(R.string.open_failed)
    val shaCopied = stringResource(R.string.chat_sha_copied)
    val shaFailed = stringResource(R.string.chat_sha_failed)

    // 名前を付けて保存(SAF)。保存先はユーザーが選ぶ
    val saveAs = rememberLauncherForActivityResult(
        ActivityResultContracts.CreateDocument(mime),
    ) { uri: Uri? ->
        if (uri != null) {
            scope.launch {
                val ok = withContext(Dispatchers.IO) { FileUtil.copyToUri(context, path, uri) }
                onNotice(if (ok) savedToast else saveFailedMsg)
            }
        }
        onDismiss()
    }

    fun contentUriOrNotice(): Uri? {
        val uri = FileUtil.contentUri(context, path)
        if (uri == null) onNotice(openFailed)
        return uri
    }

    ModalBottomSheet(onDismissRequest = onDismiss) {
        Column(modifier = Modifier.padding(bottom = 24.dp)) {
            Text(
                name,
                modifier = Modifier.padding(horizontal = 20.dp),
                style = MaterialTheme.typography.titleMedium,
                maxLines = 1,
            )
            Text(
                formatBytesLong(message.fileSize ?: 0u),
                modifier = Modifier.padding(horizontal = 20.dp),
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            HorizontalDivider(modifier = Modifier.padding(vertical = 8.dp))
            SheetAction(stringResource(R.string.action_open)) {
                val uri = contentUriOrNotice()
                if (uri != null) {
                    try {
                        context.startActivity(
                            Intent(Intent.ACTION_VIEW)
                                .setDataAndType(uri, mime)
                                .addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION),
                        )
                    } catch (e: ActivityNotFoundException) {
                        onNotice(openFailed)
                    }
                }
                onDismiss()
            }
            SheetAction(stringResource(R.string.chat_sheet_share)) {
                val uri = contentUriOrNotice()
                if (uri != null) {
                    context.startActivity(
                        Intent.createChooser(
                            Intent(Intent.ACTION_SEND)
                                .setType(mime)
                                .putExtra(Intent.EXTRA_STREAM, uri)
                                .addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION),
                            null,
                        ),
                    )
                }
                onDismiss()
            }
            SheetAction(stringResource(R.string.chat_sheet_save)) {
                scope.launch {
                    val ok = withContext(Dispatchers.IO) { FileUtil.copyToDownloads(context, path) }
                    onNotice(if (ok) savedMsg else saveFailedMsg)
                    if (ok) Toast.makeText(context, savedToast, Toast.LENGTH_SHORT).show()
                    onDismiss()
                }
            }
            SheetAction(stringResource(R.string.chat_sheet_save_as)) {
                saveAs.launch(name)
            }
            SheetAction(stringResource(R.string.chat_sheet_sha)) {
                scope.launch {
                    val sha = withContext(Dispatchers.IO) { FileUtil.sha256Of(path) }
                    if (sha != null) {
                        clipboard.setText(AnnotatedString(sha))
                        onNotice("$shaCopied: ${sha.take(16)}…")
                    } else {
                        onNotice(shaFailed)
                    }
                    onDismiss()
                }
            }
        }
    }
}

@Composable
private fun SheetAction(label: String, onClick: () -> Unit) {
    Text(
        label,
        modifier = Modifier
            .fillMaxWidth()
            .clickable(onClick = onClick)
            .padding(horizontal = 20.dp, vertical = 14.dp),
        style = MaterialTheme.typography.bodyLarge,
    )
}

/** チャット本文から URL を拾う(日本語の句読点や空白で区切る)。 */
private val urlRegex = Regex("""https?://[^\s　<>"「」]+""")

/** URL をタップ可能なリンクにした AnnotatedString を作る。 */
private fun linkifyText(text: String, content: Color) = buildAnnotatedString {
    var last = 0
    for (match in urlRegex.findAll(text)) {
        append(text.substring(last, match.range.first))
        withLink(
            LinkAnnotation.Url(
                match.value,
                TextLinkStyles(
                    style = SpanStyle(
                        color = content,
                        textDecoration = TextDecoration.Underline,
                    ),
                ),
            ),
        ) { append(match.value) }
        last = match.range.last + 1
    }
    append(text.substring(last))
}

/** リンクプレビューの取得結果(title / image どちらも無ければ表示しない)。 */
private data class LinkPreview(val title: String?, val image: android.graphics.Bitmap?)

/** URL → プレビューのキャッシュ(失敗も空として記録し再取得しない)。 */
private val previewCache = ConcurrentHashMap<String, LinkPreview>()

private fun htmlUnescape(text: String): String = text
    .replace("&amp;", "&")
    .replace("&lt;", "<")
    .replace("&gt;", ">")
    .replace("&quot;", "\"")
    .replace("&#39;", "'")

/** 先頭 128KB だけ読んで <title> と og:image を拾う(失敗は握りつぶす)。 */
private fun fetchPreview(url: String): LinkPreview {
    fun get(target: String, limit: Int): ByteArray? = try {
        val conn = URL(target).openConnection() as HttpURLConnection
        conn.connectTimeout = 5000
        conn.readTimeout = 5000
        conn.instanceFollowRedirects = true
        conn.setRequestProperty("User-Agent", "PeerCove/0.1")
        conn.inputStream.use { input ->
            val out = java.io.ByteArrayOutputStream()
            val buf = ByteArray(8192)
            while (out.size() < limit) {
                val n = input.read(buf)
                if (n < 0) break
                out.write(buf, 0, n.coerceAtMost(limit - out.size()))
            }
            out.toByteArray()
        }
    } catch (_: Exception) {
        null
    }

    val html = get(url, 128 * 1024)?.toString(Charsets.UTF_8) ?: return LinkPreview(null, null)
    val title = Regex("<title[^>]*>(.*?)</title>", setOf(RegexOption.IGNORE_CASE, RegexOption.DOT_MATCHES_ALL))
        .find(html)?.groupValues?.get(1)?.trim()?.take(120)?.let(::htmlUnescape)
    val ogImage =
        Regex("""<meta[^>]+property=["']og:image["'][^>]*content=["']([^"']+)""", RegexOption.IGNORE_CASE)
            .find(html)?.groupValues?.get(1)
            ?: Regex("""<meta[^>]+content=["']([^"']+)["'][^>]*property=["']og:image["']""", RegexOption.IGNORE_CASE)
                .find(html)?.groupValues?.get(1)
    val image = ogImage
        ?.let { if (it.startsWith("http")) it else null }
        ?.let { get(it, 1024 * 1024) }
        ?.let { BitmapFactory.decodeByteArray(it, 0, it.size) }
    return LinkPreview(title, image)
}

/** LINE 風のリンクプレビューカード(タイトル + og:image)。 */
@Composable
private fun LinkPreviewCard(url: String, content: Color) {
    var preview by remember(url) { mutableStateOf(previewCache[url]) }
    if (preview == null) {
        LaunchedEffect(url) {
            preview = withContext(Dispatchers.IO) {
                fetchPreview(url).also { previewCache[url] = it }
            }
        }
    }
    val current = preview ?: return
    if (current.title == null && current.image == null) return
    val context = LocalContext.current
    Column(
        modifier = Modifier
            .padding(top = 6.dp)
            .clip(RoundedCornerShape(10.dp))
            // バブルの文字色ベースの薄い膜(ライト/ダークどちらでも馴染む)
            .background(content.copy(alpha = 0.08f))
            .clickable {
                try {
                    context.startActivity(
                        android.content.Intent(android.content.Intent.ACTION_VIEW, Uri.parse(url)),
                    )
                } catch (_: Exception) {
                }
            },
    ) {
        current.image?.let {
            Image(
                bitmap = it.asImageBitmap(),
                contentDescription = current.title,
                modifier = Modifier.fillMaxWidth(),
                contentScale = ContentScale.FillWidth,
            )
        }
        current.title?.let {
            Text(
                it,
                modifier = Modifier.padding(horizontal = 8.dp, vertical = 6.dp),
                style = MaterialTheme.typography.labelSmall,
                color = content,
                maxLines = 2,
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
