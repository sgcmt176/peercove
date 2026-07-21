package app.peercove.android

// メモの Markdown プレビュー(M5 F-1/F-2)。
// 外部レンダラ(multiplatform-markdown-renderer)が実機でクラッシュしたため
// (2026-07-21 検証フィードバック)、依存なしの軽量実装に置き換えた。
// 対応: 見出し / 太字 / 斜体 / 取り消し線 / インラインコード / コードブロック /
// 箇条書き / 番号付き / チェックリスト / 引用 / 区切り線 / リンク / 表(等幅表示)。

import androidx.compose.foundation.background
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.LinkAnnotation
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.TextLinkStyles
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextDecoration
import androidx.compose.ui.unit.dp

@Composable
fun MarkdownPreview(body: String, modifier: Modifier = Modifier) {
    Column(modifier = modifier) {
        var inCode = false
        val codeLines = mutableListOf<String>()
        for (raw in body.lines()) {
            val line = raw.trimEnd()
            if (line.trimStart().startsWith("```")) {
                if (inCode) {
                    CodeBlock(codeLines.joinToString("\n"))
                    codeLines.clear()
                }
                inCode = !inCode
                continue
            }
            if (inCode) {
                codeLines.add(raw)
                continue
            }
            MarkdownLine(line)
        }
        // 閉じ忘れのコードブロックも表示する(編集途中のプレビュー)
        if (inCode && codeLines.isNotEmpty()) {
            CodeBlock(codeLines.joinToString("\n"))
        }
    }
}

@Composable
private fun MarkdownLine(line: String) {
    val trimmed = line.trimStart()
    val colors = MaterialTheme.colorScheme
    when {
        trimmed.isEmpty() -> Spacer(modifier = Modifier.height(8.dp))
        trimmed == "---" || trimmed == "***" || trimmed == "___" ->
            HorizontalDivider(modifier = Modifier.padding(vertical = 8.dp))
        trimmed.startsWith("### ") -> Text(
            inline(trimmed.removePrefix("### ")),
            style = MaterialTheme.typography.titleMedium,
            modifier = Modifier.padding(top = 8.dp, bottom = 2.dp),
        )
        trimmed.startsWith("## ") -> Text(
            inline(trimmed.removePrefix("## ")),
            style = MaterialTheme.typography.titleLarge,
            modifier = Modifier.padding(top = 10.dp, bottom = 2.dp),
        )
        trimmed.startsWith("# ") -> Text(
            inline(trimmed.removePrefix("# ")),
            style = MaterialTheme.typography.headlineSmall,
            modifier = Modifier.padding(top = 10.dp, bottom = 2.dp),
        )
        trimmed.startsWith("> ") || trimmed == ">" -> Row {
            Box(
                modifier = Modifier
                    .width(3.dp)
                    .height(20.dp)
                    .background(colors.primary),
            )
            Spacer(modifier = Modifier.width(8.dp))
            Text(
                inline(trimmed.removePrefix(">").trimStart()),
                style = MaterialTheme.typography.bodyMedium.copy(
                    fontStyle = FontStyle.Italic,
                ),
                color = colors.onSurfaceVariant,
            )
        }
        isChecklist(trimmed) -> {
            val done = trimmed.contains("[x]") || trimmed.contains("[X]")
            val content = inline(trimmed.substringAfter("]").trimStart())
            Row {
                Text(
                    if (done) "☑" else "☐",
                    style = MaterialTheme.typography.bodyMedium,
                    color = if (done) colors.primary else colors.onSurfaceVariant,
                )
                Spacer(modifier = Modifier.width(8.dp))
                Text(
                    if (done) {
                        buildAnnotatedString {
                            pushStyle(
                                SpanStyle(
                                    textDecoration = TextDecoration.LineThrough,
                                    color = colors.onSurfaceVariant,
                                ),
                            )
                            append(content)
                        }
                    } else {
                        content
                    },
                    style = MaterialTheme.typography.bodyMedium,
                )
            }
        }
        trimmed.startsWith("- ") || trimmed.startsWith("* ") || trimmed.startsWith("+ ") -> Row {
            Text("・", style = MaterialTheme.typography.bodyMedium)
            Text(inline(trimmed.drop(2)), style = MaterialTheme.typography.bodyMedium)
        }
        trimmed.startsWith("|") ->
            // 表は整形せず等幅でそのまま(初期版の割り切り)
            Text(
                trimmed,
                style = MaterialTheme.typography.bodySmall.copy(fontFamily = FontFamily.Monospace),
                modifier = Modifier.horizontalScroll(rememberScrollState()),
                maxLines = 1,
            )
        else -> Text(inline(trimmed), style = MaterialTheme.typography.bodyMedium)
    }
}

@Composable
private fun CodeBlock(code: String) {
    Surface(
        color = MaterialTheme.colorScheme.surfaceVariant,
        shape = RoundedCornerShape(8.dp),
        modifier = Modifier
            .fillMaxWidth()
            .padding(vertical = 4.dp),
    ) {
        Text(
            code,
            style = MaterialTheme.typography.bodySmall.copy(fontFamily = FontFamily.Monospace),
            modifier = Modifier
                .horizontalScroll(rememberScrollState())
                .padding(10.dp),
        )
    }
}

private fun isChecklist(trimmed: String): Boolean {
    val rest = when {
        trimmed.startsWith("- ") || trimmed.startsWith("* ") || trimmed.startsWith("+ ") ->
            trimmed.drop(2)
        else -> return false
    }
    return rest.startsWith("[ ] ") || rest.startsWith("[x] ") || rest.startsWith("[X] ") ||
        rest == "[ ]" || rest == "[x]" || rest == "[X]"
}

/** インライン記法(太字・斜体・取り消し線・コード・リンク)を再帰的に組む。 */
@Composable
private fun inline(text: String): AnnotatedString {
    val colors = MaterialTheme.colorScheme
    return buildAnnotatedString { appendInline(text, this, colors.primary.hashCode(), colors) }
}

private fun appendInline(
    text: String,
    builder: androidx.compose.ui.text.AnnotatedString.Builder,
    depthGuard: Int,
    colors: androidx.compose.material3.ColorScheme,
) {
    var rest = text
    var guard = 0
    while (rest.isNotEmpty() && guard < 200) {
        guard++
        // 各記法の「次の出現位置」を探し、最も手前のものを適用する
        data class Hit(val start: Int, val end: Int, val inner: String, val kind: Char)
        var best: Hit? = null
        fun consider(open: String, close: String, kind: Char) {
            val s = rest.indexOf(open)
            if (s < 0) return
            val e = rest.indexOf(close, s + open.length)
            if (e <= s) return
            val hit = Hit(s, e + close.length, rest.substring(s + open.length, e), kind)
            if (best == null || hit.start < best!!.start) best = hit
        }
        consider("**", "**", 'b')
        consider("~~", "~~", 's')
        consider("`", "`", 'c')
        // リンク [text](url)
        run {
            val s = rest.indexOf('[')
            if (s >= 0) {
                val mid = rest.indexOf("](", s + 1)
                val e = if (mid > 0) rest.indexOf(')', mid + 2) else -1
                if (mid > 0 && e > mid) {
                    val hit = Hit(s, e + 1, rest.substring(s + 1, mid), 'l')
                    if (best == null || hit.start < best!!.start) best = hit
                }
            }
        }
        // 斜体(* 単独)は ** と衝突しないよう最後に判定
        run {
            var s = rest.indexOf('*')
            while (s >= 0 && s + 1 < rest.length && rest[s + 1] == '*') {
                s = rest.indexOf('*', s + 2)
            }
            if (s >= 0) {
                var e = rest.indexOf('*', s + 1)
                while (e > 0 && e + 1 < rest.length && rest[e + 1] == '*' &&
                    rest.getOrNull(e - 1) == '*'
                ) {
                    e = rest.indexOf('*', e + 2)
                }
                if (e > s) {
                    val hit = Hit(s, e + 1, rest.substring(s + 1, e), 'i')
                    if (best == null || hit.start < best!!.start) best = hit
                }
            }
        }

        val hit = best
        if (hit == null) {
            builder.append(rest)
            return
        }
        builder.append(rest.substring(0, hit.start))
        when (hit.kind) {
            'b' -> {
                builder.pushStyle(SpanStyle(fontWeight = FontWeight.Bold))
                appendInline(hit.inner, builder, depthGuard, colors)
                builder.pop()
            }
            'i' -> {
                builder.pushStyle(SpanStyle(fontStyle = FontStyle.Italic))
                appendInline(hit.inner, builder, depthGuard, colors)
                builder.pop()
            }
            's' -> {
                builder.pushStyle(SpanStyle(textDecoration = TextDecoration.LineThrough))
                appendInline(hit.inner, builder, depthGuard, colors)
                builder.pop()
            }
            'c' -> {
                builder.pushStyle(
                    SpanStyle(
                        fontFamily = FontFamily.Monospace,
                        background = colors.surfaceVariant,
                    ),
                )
                builder.append(hit.inner)
                builder.pop()
            }
            'l' -> {
                val mid = rest.indexOf("](", hit.start + 1)
                val url = rest.substring(mid + 2, hit.end - 1)
                builder.pushLink(
                    LinkAnnotation.Url(
                        url,
                        TextLinkStyles(
                            SpanStyle(
                                color = colors.primary,
                                textDecoration = TextDecoration.Underline,
                            ),
                        ),
                    ),
                )
                builder.append(hit.inner)
                builder.pop()
            }
        }
        rest = rest.substring(hit.end)
    }
    if (rest.isNotEmpty()) builder.append(rest)
}
