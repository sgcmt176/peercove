package app.peercove.android

import android.content.ContentValues
import android.content.Context
import android.net.Uri
import android.os.Environment
import android.provider.MediaStore
import android.provider.OpenableColumns
import java.io.File

/**
 * ファイル送受信の OS 連携(M4 E-C)。
 * - 送信: SAF の content URI をアプリのキャッシュへ実ファイル化して Rust へ渡す
 * - 受信: 受信ボックス(アプリ内部)のファイルを共有の Download/ へコピーする
 */
object FileUtil {

    /** content URI の表示名(無ければ "file")。 */
    fun displayName(context: Context, uri: Uri): String {
        context.contentResolver.query(uri, null, null, null, null)?.use { cursor ->
            val index = cursor.getColumnIndex(OpenableColumns.DISPLAY_NAME)
            if (index >= 0 && cursor.moveToFirst()) {
                cursor.getString(index)?.let { return it }
            }
        }
        return uri.lastPathSegment?.substringAfterLast('/') ?: "file"
    }

    /**
     * content URI をキャッシュディレクトリへコピーして File を返す。
     * (Rust 側はパスでファイルを読むため実ファイルが必要)
     *
     * 送信成功後も削除しない: チャット履歴の自分の画像サムネイルが
     * このパスを参照する。キャッシュ領域なのでストレージ逼迫時は OS が
     * 消してよい(消えたらサムネイルがファイル表示に戻るだけ)。
     * サブディレクトリを時刻で分けるのは同名ファイルの上書き防止。
     */
    fun copyToCache(context: Context, uri: Uri): File? {
        val name = displayName(context, uri)
        val dir = File(context.cacheDir, "send/${System.currentTimeMillis()}").apply { mkdirs() }
        val dest = File(dir, name)
        return try {
            context.contentResolver.openInputStream(uri)?.use { input ->
                dest.outputStream().use { output -> input.copyTo(output) }
            } ?: return null
            dest
        } catch (e: Exception) {
            null
        }
    }

    /**
     * 受信ボックスのファイルを共有の Download/PeerCove/ へコピーする。
     * MediaStore 経由なのでストレージ権限は不要(API 29+)。
     */
    fun copyToDownloads(context: Context, path: String): Boolean {
        val src = File(path)
        if (!src.isFile) return false
        return try {
            val values = ContentValues().apply {
                put(MediaStore.Downloads.DISPLAY_NAME, src.name)
                put(
                    MediaStore.Downloads.RELATIVE_PATH,
                    Environment.DIRECTORY_DOWNLOADS + "/PeerCove",
                )
            }
            val resolver = context.contentResolver
            val uri = resolver.insert(MediaStore.Downloads.EXTERNAL_CONTENT_URI, values)
                ?: return false
            resolver.openOutputStream(uri)?.use { output ->
                src.inputStream().use { input -> input.copyTo(output) }
            } ?: return false
            true
        } catch (e: Exception) {
            false
        }
    }
}
