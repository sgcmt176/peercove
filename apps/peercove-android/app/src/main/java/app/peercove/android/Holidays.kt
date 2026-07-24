package app.peercove.android

// 日本の祝日(カレンダーの配色、M6 H-6、ADR-0055 決定 4)。holidays-jp API
// (https://holidays-jp.github.io/api/v1/date.json)から HttpURLConnection で
// バックグラウンド取得し、アプリ内部ストレージ(filesDir/holidays.json)へ
// 30 日キャッシュする。取得に失敗しても例外は投げない — 呼び出し側は常に
// Map を受け取り、キャッシュがあればそれを(古くても)、無ければ空
// (週末色のみで動作)を返す。デスクトップ版(holidays.ts)と同じ方針。
// **祝日名を Log に出さない**(通信データそのものが秘匿対象ではないが、
// 他の共有データと同じ扱いを徹底する)。

import android.content.Context
import java.io.File
import java.net.HttpURLConnection
import java.net.URL
import java.text.SimpleDateFormat
import java.util.Date
import java.util.Locale
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import org.json.JSONObject

object Holidays {
    private const val API_URL = "https://holidays-jp.github.io/api/v1/date.json"
    private const val TTL_MS = 30L * 24 * 60 * 60 * 1000
    private const val CONNECT_TIMEOUT_MS = 8000
    private const val READ_TIMEOUT_MS = 8000

    @Volatile
    private var memoryCache: Map<String, String>? = null

    private fun cacheFile(context: Context): File = File(context.filesDir, "holidays.json")

    private fun readCacheFile(context: Context): Pair<Long, Map<String, String>>? = try {
        val file = cacheFile(context)
        if (file.exists()) {
            val obj = JSONObject(file.readText())
            val fetchedAt = obj.optLong("fetchedAt", 0L)
            val data = obj.optJSONObject("holidays")
            if (data != null) {
                val map = LinkedHashMap<String, String>()
                data.keys().forEach { key -> map[key] = data.optString(key) }
                fetchedAt to map
            } else {
                null
            }
        } else {
            null
        }
    } catch (e: Exception) {
        null
    }

    private fun writeCacheFile(context: Context, holidays: Map<String, String>) {
        try {
            val obj = JSONObject()
            obj.put("fetchedAt", System.currentTimeMillis())
            obj.put("holidays", JSONObject(holidays as Map<*, *>))
            cacheFile(context).writeText(obj.toString())
        } catch (e: Exception) {
            // 保存できなくても今回取得した分はメモリキャッシュで使える
        }
    }

    private fun fetchFromNetwork(): Map<String, String> {
        val conn = URL(API_URL).openConnection() as HttpURLConnection
        conn.connectTimeout = CONNECT_TIMEOUT_MS
        conn.readTimeout = READ_TIMEOUT_MS
        conn.setRequestProperty("User-Agent", "PeerCove/Android")
        conn.inputStream.use { input ->
            val text = input.bufferedReader().readText()
            val obj = JSONObject(text)
            val map = LinkedHashMap<String, String>()
            obj.keys().forEach { key -> map[key] = obj.optString(key) }
            return map
        }
    }

    /**
     * 祝日マップ("yyyy-MM-dd" → 祝日名)。有効なキャッシュ(30 日以内)が
     * あればそれを即返し、無ければ 1 回だけ取得を試みる。失敗時は既存
     * キャッシュ(古くても)があればそれを、無ければ空を返す(呼び出し側は
     * 例外処理をしなくてよい)。
     */
    suspend fun get(context: Context): Map<String, String> {
        memoryCache?.let { return it }
        return withContext(Dispatchers.IO) {
            val cached = readCacheFile(context)
            if (cached != null && System.currentTimeMillis() - cached.first < TTL_MS) {
                memoryCache = cached.second
                return@withContext cached.second
            }
            val fetched = try {
                fetchFromNetwork()
            } catch (e: Exception) {
                null
            }
            val result = if (fetched != null) {
                writeCacheFile(context, fetched)
                fetched
            } else {
                cached?.second ?: emptyMap()
            }
            memoryCache = result
            result
        }
    }

    private val keyFormat = SimpleDateFormat("yyyy-MM-dd", Locale.US)

    /** "yyyy-MM-dd"(ローカル日付)。holidays-jp API のキー形式に合わせる。 */
    fun key(unixMs: Long): String = synchronized(keyFormat) { keyFormat.format(Date(unixMs)) }
}
