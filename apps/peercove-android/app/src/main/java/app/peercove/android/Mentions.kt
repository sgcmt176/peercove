package app.peercove.android

import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.withStyle

// メンション判定・強調表示の共通ロジック(ADR-0055 決定 1)。
//
// apps/peercove-ui/src/mentions.ts の移植。ロジックを変えるときは両方直すこと
// (語境界の扱い、長い名前を優先するソート、範囲の重複回避、`@All` の扱いを
// 一致させる)。「@<自分の表示名>」または「@All」を「自分宛」とみなす。
// チャット本文(ChatScreen.kt)と共有メモのコメント本文(SharedMemoScreen.kt)
// の色分け表示で使う。

/** `@All` の挿入トークン(英語固定)。 */
const val MENTION_ALL_TOKEN = "All"

/** 自分宛メンションの固定色(デスクトップと同じ、styles.css の
 *  .msg__mention--self: background #f5cf3d / color #3a2c00)。バブルの色や
 *  ダーク/ライトテーマに関わらず一定にする。 */
val MENTION_SELF_BACKGROUND = Color(0xFFF5CF3D)
val MENTION_SELF_FOREGROUND = Color(0xFF3A2C00)

/**
 * 本文に自分宛のメンション(`@<myName>` または `@All`)が含まれるか。
 * mentions.ts の isMentioned と同じ判定方式(単純な部分文字列一致。語境界は
 * 見ない)。`myName` が空文字なら名前メンションは判定しない(`@All` だけ見る)。
 */
fun isMentioned(text: String, myName: String): Boolean {
    if (text.contains("@$MENTION_ALL_TOKEN")) return true
    val trimmed = myName.trim()
    return trimmed.isNotEmpty() && text.contains("@$trimmed")
}

/** 本文の 1 区間(地の文 or メンショントークン)。 */
sealed class MentionSegment {
    data class Plain(val value: String) : MentionSegment()
    data class Mention(val value: String, val self: Boolean) : MentionSegment()
}

// JS の `\w`(Unicode フラグ無し)と同じ範囲(ASCII の英数字 + アンダースコア)。
private val mentionWordChar = Regex("[A-Za-z0-9_]")
private val mentionWhitespace = Regex("""\s""")

/**
 * 本文を「地の文」と「メンショントークン」に分割する(表示の強調用)。
 * `memberNames`(自分以外)または自分の名前、`All` に一致するものだけを
 * メンションと認識する(`@example.com` のような無関係な文字列を誤検知しない
 * ため)。同じ名前が他の名前の接頭辞になっている場合に備え、長い名前から
 * 先にマッチさせる。mentions.ts の splitMentions と同じアルゴリズム。
 */
fun splitMentions(text: String, myName: String, memberNames: List<String>): List<MentionSegment> {
    val trimmedMy = myName.trim()
    // name -> 自分宛か(挿入順を保持。TS 版の Map と同じく後勝ちにしない)
    val candidates = LinkedHashMap<String, Boolean>()
    candidates[MENTION_ALL_TOKEN] = true
    if (trimmedMy.isNotEmpty()) candidates[trimmedMy] = true
    for (name in memberNames) {
        val trimmed = name.trim()
        if (trimmed.isNotEmpty() && !candidates.containsKey(trimmed)) {
            candidates[trimmed] = false
        }
    }
    val names = candidates.keys.sortedByDescending { it.length }

    data class Range(val start: Int, val end: Int, val self: Boolean)
    val ranges = mutableListOf<Range>()
    for (name in names) {
        val token = "@$name"
        var from = 0
        while (true) {
            val at = text.indexOf(token, from)
            if (at < 0) break
            from = at + token.length
            val before = if (at > 0) text[at - 1].toString() else ""
            val after = if (at + token.length < text.length) {
                text[at + token.length].toString()
            } else {
                ""
            }
            // 直前が空白以外(行頭は可)、または直後が英数字なら
            // 「別の語の一部」とみなして除外
            if (before.isNotEmpty() && !mentionWhitespace.matches(before)) continue
            if (mentionWordChar.matches(after)) continue
            // 既により長い名前とマッチ済みの範囲とは重ねない
            if (ranges.any { at < it.end && at + token.length > it.start }) continue
            ranges.add(Range(at, at + token.length, candidates.getValue(name)))
        }
    }
    ranges.sortBy { it.start }

    val parts = mutableListOf<MentionSegment>()
    var cursor = 0
    for (r in ranges) {
        if (r.start > cursor) parts.add(MentionSegment.Plain(text.substring(cursor, r.start)))
        parts.add(MentionSegment.Mention(text.substring(r.start, r.end), r.self))
        cursor = r.end
    }
    if (cursor < text.length) parts.add(MentionSegment.Plain(text.substring(cursor)))
    return parts
}

/**
 * 語境界つきで「自分宛か」を判定する(splitMentions を土台にした便利関数)。
 * PeercoveVpnService の通知判定で使う: 素朴な `contains("@$myName")` だと、
 * 自分の名前が別の誰か宛の長い名前の接頭辞のとき(例: 自分が「Al」で本文が
 * 「@Allen ...」)誤って自分宛と判定してしまう。splitMentions は語境界を
 * 見るためこれを防げる(表示の色分けと同じロジックで統一されるという利点も
 * ある)。
 */
fun isMentionedIn(text: String, myName: String, memberNames: List<String>): Boolean =
    splitMentions(text, myName, memberNames).any { it is MentionSegment.Mention && it.self }

/**
 * メンション部分だけ色分けした AnnotatedString を作る(URL のリンク化はしない
 * — デスクトップの renderCommentBody と同じく範囲外。共有メモのコメント本文
 * で使う)。チャット本文のように URL リンク化も併用したい場合は
 * ChatScreen.kt の chatMessageText を使う。
 */
fun mentionAnnotatedString(
    text: String,
    myName: String,
    memberNames: List<String>,
    otherMentionBackground: Color,
    otherMentionForeground: Color,
): AnnotatedString = buildAnnotatedString {
    for (seg in splitMentions(text, myName, memberNames)) {
        when (seg) {
            is MentionSegment.Plain -> append(seg.value)
            is MentionSegment.Mention -> {
                val background = if (seg.self) MENTION_SELF_BACKGROUND else otherMentionBackground
                val foreground = if (seg.self) MENTION_SELF_FOREGROUND else otherMentionForeground
                withStyle(SpanStyle(background = background, color = foreground)) {
                    append(seg.value)
                }
            }
        }
    }
}
