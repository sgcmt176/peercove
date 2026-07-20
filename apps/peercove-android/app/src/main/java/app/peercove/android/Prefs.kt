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

    /** 既読位置を現在の履歴の最新 seq まで切り詰める(自己修復)。
     *  ネットワークの削除→再参加などで履歴の seq が 1 から振り直されると、
     *  残った既読位置がすべての新着より大きくなり、通知が永久に抑止される
     *  (2026-07-20 の「通知が全く来ない」障害の原因)。setReadSeq は単調増加
     *  なので、接続時にここで一度だけ強制的に下げる。 */
    fun clampReadSeqs(context: Context, slug: String, latestSeq: Long) {
        val editor = prefs(context).edit()
        var changed = false
        for ((key, value) in prefs(context).all) {
            if (key.startsWith("read/$slug/") && value is Long && value > latestSeq) {
                editor.putLong(key, latestSeq)
                changed = true
            }
        }
        if (changed) editor.apply()
    }

    /** トーク一覧のピン留め(常に上へ表示する会話)。**順序付き**で保存する
     *  (リストの並び = ピン内の表示順。convId は改行を含まないので改行区切り)。 */
    fun pinOrder(context: Context, slug: String): List<String> =
        prefs(context).getString("pinorder/$slug", "")
            ?.split("\n")
            ?.filter { it.isNotEmpty() }
            ?: emptyList()

    fun setPinOrder(context: Context, slug: String, order: List<String>) {
        prefs(context).edit().putString("pinorder/$slug", order.joinToString("\n")).apply()
    }

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
