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

    /** 表示テーマ: "system"(既定)/ "light" / "dark"。 */
    fun theme(context: Context): String = prefs(context).getString("theme", "system") ?: "system"

    fun setTheme(context: Context, value: String) {
        prefs(context).edit().putString("theme", value).apply()
    }

    /** VPN を維持すべきか(明示的に切断するまで true)。プロセス再生成や
     *  OS 起動(Always-on)後の null Intent 起動時の復帰判断に使う。 */
    fun vpnShouldRun(context: Context): Boolean =
        prefs(context).getBoolean("vpn_should_run", false)

    fun setVpnShouldRun(context: Context, value: Boolean) {
        prefs(context).edit().putBoolean("vpn_should_run", value).apply()
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

    /** トーク一覧のピン留め(常に上へ表示する会話)。 */
    fun setPinned(context: Context, slug: String, convId: String, pinned: Boolean) {
        val key = "pin/$slug/$convId"
        prefs(context).edit().apply {
            if (pinned) putBoolean(key, true) else remove(key)
        }.apply()
    }

    fun allPins(context: Context, slug: String): Set<String> =
        prefs(context).all.mapNotNull { (key, value) ->
            val convId = key.removePrefix("pin/$slug/")
            if (convId != key && value == true) convId else null
        }.toSet()

    /** 会話単位の通知ミュート(バッジは出るが通知は出さない)。 */
    fun setMuted(context: Context, slug: String, convId: String, muted: Boolean) {
        val key = "mute/$slug/$convId"
        prefs(context).edit().apply {
            if (muted) putBoolean(key, true) else remove(key)
        }.apply()
    }

    fun isMuted(context: Context, slug: String, convId: String): Boolean =
        prefs(context).getBoolean("mute/$slug/$convId", false)

    fun allMutes(context: Context, slug: String): Set<String> =
        prefs(context).all.mapNotNull { (key, value) ->
            val convId = key.removePrefix("mute/$slug/")
            if (convId != key && value == true) convId else null
        }.toSet()

    /** チャット通知の本文をロック画面で隠す(全体設定)。 */
    fun hideNotifContent(context: Context): Boolean =
        prefs(context).getBoolean("notif_hide_content", false)

    fun setHideNotifContent(context: Context, value: Boolean) {
        prefs(context).edit().putBoolean("notif_hide_content", value).apply()
    }
}
