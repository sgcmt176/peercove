package app.peercove.android

// 共有オブジェクト参照 `@種別:id`(M5 F-5 Stage 4、ADR-0052 決定 1)。チャット
// 本文にそのまま書ける軽量トークンをカード表示するための汎用パーサ + 種別
// レジストリ。プロトコル変更なし(本文の一部。旧クライアントには文字列の
// まま見える)。種別を増やすときは SharedRefKind に 1 エントリ足すだけでよい。
// カードの内容は表示時に受信者自身の権限で解決する(メモはキャッシュ経由 =
// オフラインでも出る)。取得できなければ「アクセスできません」カードにし、
// タイトル等は一切出さない。**メモのタイトル・本文はログへ出さない**。

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import java.util.concurrent.ConcurrentHashMap
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import uniffi.peercove_mobile.MobileException
import uniffi.peercove_mobile.sharedMemoGet

/** 対応している種別のレジストリ。増やすときはここへ 1 エントリ足すだけでよい。 */
enum class SharedRefKind(val prefix: String, val icon: String) {
    MEMO("memo", "📝"),
    ;

    companion object {
        fun fromPrefix(value: String): SharedRefKind? =
            entries.firstOrNull { it.prefix.equals(value, ignoreCase = true) }
    }
}

data class SharedRefToken(val kind: SharedRefKind, val id: String)

sealed class SharedRefPart {
    data class PlainText(val value: String) : SharedRefPart()
    data class Ref(val token: SharedRefToken) : SharedRefPart()
}

// 種別:id(id は 16 進英数字)。id の後ろが英数字だとトークンの境界が
// 曖昧なので \b で区切る(例: @memo:abc123z のような不完全な id には反応しない)。
private val SHARED_REF_RE = Regex("""@([a-zA-Z][a-zA-Z0-9]*):([0-9a-fA-F]+)\b""")

/** 本文を `@種別:id` トークンと地の文へ分割する(未登録の種別はただの文字列のまま)。 */
fun splitSharedRefs(text: String): List<SharedRefPart> {
    val parts = mutableListOf<SharedRefPart>()
    var last = 0
    for (match in SHARED_REF_RE.findAll(text)) {
        val kind = SharedRefKind.fromPrefix(match.groupValues[1]) ?: continue
        if (match.range.first > last) {
            parts.add(SharedRefPart.PlainText(text.substring(last, match.range.first)))
        }
        parts.add(SharedRefPart.Ref(SharedRefToken(kind, match.groupValues[2])))
        last = match.range.last + 1
    }
    if (last < text.length) parts.add(SharedRefPart.PlainText(text.substring(last)))
    return parts
}

/** チャットへ貼る参照子の文字列(共有メモの「リンクをコピー」用)。 */
fun sharedRefToken(kind: SharedRefKind, id: String): String = "@${kind.prefix}:$id"

/** カードの表示内容(タイトル + 抜粋 1 行)。 */
private data class SharedRefResolved(val title: String, val excerpt: String)

private fun firstBodyLine(body: String): String =
    body.lineSequence().firstOrNull { it.isNotBlank() }?.trim()?.take(80) ?: ""

/** 表示時に受信者自身の権限で解決する。キャッシュ経由 = オフラインでも出る。 */
private suspend fun resolveSharedRef(
    baseDir: String,
    slug: String,
    token: SharedRefToken,
): SharedRefResolved? = when (token.kind) {
    SharedRefKind.MEMO -> try {
        val memo = withContext(Dispatchers.IO) { sharedMemoGet(baseDir, slug, token.id) }
        SharedRefResolved(memo.title, firstBodyLine(memo.body))
    } catch (e: MobileException) {
        null
    }
}

// 解決結果は slug::種別:id ごとに使い回す(表示のたびに引き直さない)。
private val sharedRefCache = ConcurrentHashMap<String, SharedRefResolved?>()

/** チャットの `@memo:id` カード(M5 F-5 Stage 4、ADR-0052 決定 1)。 */
@Composable
fun SharedRefCard(
    baseDir: String,
    slug: String,
    token: SharedRefToken,
    content: Color,
    onOpen: () -> Unit,
) {
    val key = "$slug::${token.kind.prefix}:${token.id}"
    var loading by remember(key) { mutableStateOf(!sharedRefCache.containsKey(key)) }
    var resolved by remember(key) { mutableStateOf(sharedRefCache[key]) }
    LaunchedEffect(key) {
        if (sharedRefCache.containsKey(key)) {
            resolved = sharedRefCache[key]
            loading = false
            return@LaunchedEffect
        }
        val value = resolveSharedRef(baseDir, slug, token)
        sharedRefCache[key] = value
        resolved = value
        loading = false
    }
    val current = resolved
    Column(
        modifier = Modifier
            .padding(top = 4.dp, bottom = 2.dp)
            .widthIn(max = 240.dp)
            .clip(RoundedCornerShape(10.dp))
            .background(content.copy(alpha = 0.08f))
            .let { base -> if (!loading && current != null) base.clickable(onClick = onOpen) else base }
            .padding(horizontal = 10.dp, vertical = 8.dp),
    ) {
        Row {
            Text(if (!loading && current == null) "🔒" else token.kind.icon)
            Text(
                text = " " + when {
                    loading -> stringResource(R.string.shared_ref_loading)
                    current == null -> stringResource(R.string.shared_ref_inaccessible)
                    else -> current.title.ifEmpty { stringResource(R.string.memo_untitled) }
                },
                color = content,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
        }
        if (!loading && current != null && current.excerpt.isNotEmpty()) {
            Text(
                current.excerpt,
                style = MaterialTheme.typography.labelSmall,
                color = content.copy(alpha = 0.75f),
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
        }
    }
}
