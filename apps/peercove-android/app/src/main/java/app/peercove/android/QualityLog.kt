package app.peercove.android

import android.content.Context
import java.io.File
import org.json.JSONObject

/**
 * 通信品質履歴の簡易版(E-E 9。デスクトップ M3-23 の縮小移植)。
 * - 30 秒粒度の RTT サンプル(設定タブのスパークライン用)
 * - 接続/切断/回線切替/張り直し/品質低下のイベントタイムライン
 *
 * JSON Lines で端末ローカルに保存(filesDir/quality/<slug>.jsonl)。
 * 上限を超えたら古い行から落とす(おおむね直近 1 日ぶんが残る)。
 */
object QualityLog {
    private const val TRIM_THRESHOLD = 3200
    private const val TRIM_TO = 2600

    data class Entry(
        val t: Long,
        val kind: String,
        val rttMs: Long?,
        val label: String?,
    )

    private fun file(context: Context, slug: String): File =
        File(File(context.filesDir, "quality").apply { mkdirs() }, "$slug.jsonl")

    /** RTT などの定期サンプル(rtt が無い = 未同期の間も刻む)。 */
    @Synchronized
    fun sample(context: Context, slug: String, rttMs: Long?) {
        val obj = JSONObject().apply {
            put("t", System.currentTimeMillis())
            put("kind", "sample")
            if (rttMs != null) put("rtt", rttMs)
        }
        append(context, slug, obj)
    }

    /** 接続・切断・回線切替などのイベント(label は表示文そのまま)。 */
    @Synchronized
    fun event(context: Context, slug: String, label: String) {
        val obj = JSONObject().apply {
            put("t", System.currentTimeMillis())
            put("kind", "event")
            put("label", label)
        }
        append(context, slug, obj)
    }

    private fun append(context: Context, slug: String, obj: JSONObject) {
        try {
            val f = file(context, slug)
            f.appendText(obj.toString() + "\n")
            // 行数はサンプル 30 秒粒度で 1 日 ≈ 2900 行。超えたら詰め直す
            if (f.length() > 400 * 1024) {
                val lines = f.readLines()
                if (lines.size > TRIM_THRESHOLD) {
                    val tmp = File(f.parentFile, f.name + ".tmp")
                    tmp.writeText(lines.takeLast(TRIM_TO).joinToString("\n") + "\n")
                    tmp.renameTo(f)
                }
            }
        } catch (_: Exception) {
            // 品質履歴は落としてよい(本体機能に影響させない)
        }
    }

    /** sinceMs 以降のエントリ(古い順)。 */
    @Synchronized
    fun list(context: Context, slug: String, sinceMs: Long): List<Entry> = try {
        file(context, slug).readLines().mapNotNull { line ->
            try {
                val o = JSONObject(line)
                val t = o.getLong("t")
                if (t < sinceMs) {
                    null
                } else {
                    Entry(
                        t = t,
                        kind = o.getString("kind"),
                        rttMs = if (o.has("rtt")) o.getLong("rtt") else null,
                        label = if (o.has("label")) o.getString("label") else null,
                    )
                }
            } catch (_: Exception) {
                null
            }
        }
    } catch (_: Exception) {
        emptyList()
    }

    /** 履歴ファイルの削除(ストレージ管理から)。 */
    @Synchronized
    fun clear(context: Context, slug: String) {
        try {
            file(context, slug).delete()
        } catch (_: Exception) {
        }
    }
}
