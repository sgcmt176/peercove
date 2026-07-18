package app.peercove.android

import android.content.Intent
import android.net.VpnService
import android.util.Log
import uniffi.peercove_mobile.SocketProtector
import uniffi.peercove_mobile.listNetworks
import uniffi.peercove_mobile.startTunnel
import uniffi.peercove_mobile.stopTunnel

/**
 * PeerCove の VPN サービス(M4 E-B)。
 *
 * OS から TUN の fd をもらい、所有権を Rust(peercove-mobile)へ渡すだけの
 * 薄い層。WG プロトコル処理はすべて Rust 側(ADR-0039/0040)。
 * デスクトップと違い root もデーモンも不要(VpnService が公式の仕組み)。
 */
class PeercoveVpnService : VpnService() {
    companion object {
        private const val TAG = "peercove"
        const val ACTION_CONNECT = "app.peercove.android.action.CONNECT"
        const val ACTION_DISCONNECT = "app.peercove.android.action.DISCONNECT"
        const val EXTRA_SLUG = "slug"
    }

    private var currentSlug: String? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        when (intent?.action) {
            ACTION_DISCONNECT -> {
                teardown()
                stopSelf()
                return START_NOT_STICKY
            }
            else -> {
                val slug = intent?.getStringExtra(EXTRA_SLUG)
                if (slug == null) {
                    stopSelf()
                    return START_NOT_STICKY
                }
                connect(slug)
            }
        }
        return START_STICKY
    }

    private fun connect(slug: String) {
        val info = listNetworks(filesDir.absolutePath).firstOrNull { it.slug == slug }
        if (info == null) {
            Log.e(TAG, "ネットワークが見つかりません: $slug")
            stopSelf()
            return
        }
        try {
            val builder = Builder()
                .setSession("PeerCove ${info.name}")
                .addAddress(info.memberIp, info.prefixLen.toInt())
                // ハブ&スポーク: VPN のサブネットだけをトンネルへ向ける
                // (全トラフィックは通さない。デスクトップ版と同じ方針)
                .addRoute(info.subnetAddr, info.prefixLen.toInt())
                .setMtu(info.mtu.toInt())
            val pfd = builder.establish()
                ?: throw IllegalStateException("establish() が null を返しました(VPN 権限が未許可?)")
            // detachFd で所有権を切り離して Rust へ渡す(以後 close は Rust 側)
            val fd = pfd.detachFd()
            teardown() // 同一サービスでの張り替え(別ネットワークへの切替)
            startTunnel(
                filesDir.absolutePath,
                slug,
                fd,
                object : SocketProtector {
                    // WG の UDP ソケットを VPN ルーティングから除外する
                    override fun protect(fd: Int): Boolean = this@PeercoveVpnService.protect(fd)
                },
            )
            currentSlug = slug
            Log.i(TAG, "トンネル開始: $slug")
        } catch (e: Exception) {
            Log.e(TAG, "接続に失敗: $slug", e)
            stopSelf()
        }
    }

    /** トンネル停止(冪等)。fd は Rust 側で close され、VPN も終了する */
    private fun teardown() {
        currentSlug?.let {
            stopTunnel(it)
            Log.i(TAG, "トンネル停止: $it")
        }
        currentSlug = null
    }

    override fun onDestroy() {
        teardown()
        super.onDestroy()
    }

    /** ユーザーが設定画面などから VPN を切った場合 */
    override fun onRevoke() {
        teardown()
        stopSelf()
    }
}
