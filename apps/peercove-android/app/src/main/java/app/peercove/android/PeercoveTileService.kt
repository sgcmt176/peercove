package app.peercove.android

import android.app.PendingIntent
import android.content.Intent
import android.net.VpnService
import android.os.Build
import android.service.quicksettings.Tile
import android.service.quicksettings.TileService
import uniffi.peercove_mobile.listNetworks
import uniffi.peercove_mobile.tunnelStatus

/**
 * クイック設定タイル: 通知シェードから 1 タップで接続 / 切断。
 *
 * - 接続先は「最後に接続したネットワーク」(Prefs.lastSlug)。無ければ一覧の先頭
 * - VPN の使用許可(VpnService.prepare)が未取得の間はアプリを開いて促す
 *   (許可ダイアログは Activity からしか出せない)
 */
class PeercoveTileService : TileService() {

    private fun runningSlug(): String? =
        listNetworks(filesDir.absolutePath).firstOrNull { tunnelStatus(it.slug) != null }?.slug

    override fun onStartListening() {
        updateTile()
    }

    override fun onClick() {
        if (runningSlug() != null) {
            startService(
                Intent(this, PeercoveVpnService::class.java)
                    .setAction(PeercoveVpnService.ACTION_DISCONNECT),
            )
        } else {
            val slug = Prefs.lastSlug(this)
                ?: listNetworks(filesDir.absolutePath).firstOrNull()?.slug
                ?: return
            if (VpnService.prepare(this) == null) {
                startService(
                    Intent(this, PeercoveVpnService::class.java)
                        .setAction(PeercoveVpnService.ACTION_CONNECT)
                        .putExtra(PeercoveVpnService.EXTRA_SLUG, slug),
                )
            } else {
                openApp()
            }
        }
        updateTile()
    }

    private fun openApp() {
        val intent = Intent(this, MainActivity::class.java)
            .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
        if (Build.VERSION.SDK_INT >= 34) {
            startActivityAndCollapse(
                PendingIntent.getActivity(this, 0, intent, PendingIntent.FLAG_IMMUTABLE),
            )
        } else {
            @Suppress("DEPRECATION", "StartActivityAndCollapseDeprecated")
            startActivityAndCollapse(intent)
        }
    }

    /** 接続状態をタイルへ反映(接続は非同期なので次の onStartListening でも追随) */
    private fun updateTile() {
        val tile = qsTile ?: return
        tile.state = if (runningSlug() != null) Tile.STATE_ACTIVE else Tile.STATE_INACTIVE
        tile.label = getString(R.string.app_name)
        tile.updateTile()
    }
}
