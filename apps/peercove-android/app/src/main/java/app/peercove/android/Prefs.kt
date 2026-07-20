package app.peercove.android

import android.content.Context
import android.content.SharedPreferences

/**
 * アプリ内の軽量な永続状態(SharedPreferences)。
 * - 最後に接続したネットワーク(クイック設定タイルの接続先)
 * - 会話ごとの既読位置(トーク一覧の未読バッジ用)
 */
object Prefs {
    private fun prefs(context: Context): SharedPreferences =
        context.getSharedPreferences("peercove", Context.MODE_PRIVATE)

    fun lastSlug(context: Context): String? = prefs(context).getString("last_slug", null)

    fun setLastSlug(context: Context, slug: String) {
        prefs(context).edit().putString("last_slug", slug).apply()
    }

    /** 会話の既読 seq(それ以下は既読)。 */
    fun readSeq(context: Context, slug: String, convId: String): Long =
        prefs(context).getLong("read/$slug/$convId", 0L)

    fun setReadSeq(context: Context, slug: String, convId: String, seq: Long) {
        val key = "read/$slug/$convId"
        if (prefs(context).getLong(key, 0L) < seq) {
            prefs(context).edit().putLong(key, seq).apply()
        }
    }

    /** このネットワークの既読位置をまとめて読む(convId → seq)。 */
    fun allReadSeqs(context: Context, slug: String): Map<String, Long> =
        prefs(context).all.mapNotNull { (key, value) ->
            val convId = key.removePrefix("read/$slug/")
            if (convId != key && value is Long) convId to value else null
        }.toMap()
}
