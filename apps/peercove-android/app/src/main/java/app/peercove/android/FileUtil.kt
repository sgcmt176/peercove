package app.peercove.android

import android.content.ContentValues
import android.content.Context
import android.net.Uri
import android.os.Environment
import android.provider.MediaStore
import android.provider.OpenableColumns
import android.webkit.MimeTypeMap
import androidx.core.content.FileProvider
import java.io.File
import java.security.MessageDigest

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

    /** アプリ内部のファイルを他アプリへ渡すための content URI(FileProvider)。 */
    fun contentUri(context: Context, path: String): Uri? = try {
        FileProvider.getUriForFile(context, "${context.packageName}.fileprovider", File(path))
    } catch (e: IllegalArgumentException) {
        null // file_paths.xml の対象外
    }

    /** 拡張子から MIME タイプを引く(不明は octet-stream)。 */
    fun mimeOf(name: String): String {
        val ext = name.substringAfterLast('.', "").lowercase()
        return MimeTypeMap.getSingleton().getMimeTypeFromExtension(ext)
            ?: "application/octet-stream"
    }

    /** ファイルの SHA-256(16 進小文字)。大きくても逐次読みでメモリを食わない。 */
    fun sha256Of(path: String): String? = try {
        val digest = MessageDigest.getInstance("SHA-256")
        File(path).inputStream().use { input ->
            val buf = ByteArray(64 * 1024)
            while (true) {
                val n = input.read(buf)
                if (n < 0) break
                digest.update(buf, 0, n)
            }
        }
        digest.digest().joinToString("") { "%02x".format(it) }
    } catch (e: Exception) {
        null
    }

    /** 受信ファイルを SAF で選んだ保存先(content URI)へコピーする。 */
    fun copyToUri(context: Context, path: String, dest: Uri): Boolean = try {
        val src = File(path)
        context.contentResolver.openOutputStream(dest)?.use { output ->
            src.inputStream().use { input -> input.copyTo(output) }
        } != null
    } catch (e: Exception) {
        false
    }

    /** ディレクトリ配下の合計サイズ(バイト)。無ければ 0。 */
    fun dirSize(dir: File): Long = try {
        dir.walkBottomUp().filter { it.isFile }.sumOf { it.length() }
    } catch (e: Exception) {
        0L
    }

    /** ディレクトリの中身だけを消す(ディレクトリ自体は残す)。 */
    fun clearDir(dir: File): Boolean = try {
        dir.listFiles()?.forEach { it.deleteRecursively() }
        true
    } catch (e: Exception) {
        false
    }
}
