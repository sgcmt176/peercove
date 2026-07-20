package app.peercove.android

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Intent
import android.content.pm.ServiceInfo
import android.net.ConnectivityManager
import android.net.Network
import android.net.VpnService
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.util.Log
import uniffi.peercove_mobile.SocketProtector
import uniffi.peercove_mobile.listNetworks
import uniffi.peercove_mobile.pokeTunnel
import uniffi.peercove_mobile.sessionState
import uniffi.peercove_mobile.startTunnel
import uniffi.peercove_mobile.stopTunnel
import uniffi.peercove_mobile.tunnelStatus

/**
 * PeerCove の VPN サービス(M4 E-B/E-D)。
 *
 * OS から TUN の fd をもらい、所有権を Rust(peercove-mobile)へ渡す薄い層。
 * WG プロトコル処理はすべて Rust 側(ADR-0039/0040)。
 *
 * E-D で常駐化:
 * - Foreground Service + 継続通知(状態・RTT・転送量、切断アクション)
 * - 回線切替(Wi-Fi ↔ モバイル)を NetworkCallback で検知して UDP を張り直す
 * - プロセス再生成(START_STICKY の null Intent)や Always-on 起動
 *   (SERVICE_INTERFACE)では最後に接続していたネットワークへ復帰する
 */
class PeercoveVpnService : VpnService() {
    companion object {
        private const val TAG = "peercove"
        const val ACTION_CONNECT = "app.peercove.android.action.CONNECT"
        const val ACTION_DISCONNECT = "app.peercove.android.action.DISCONNECT"
        const val EXTRA_SLUG = "slug"
        private const val CHANNEL_ID = "peercove_vpn"
        private const val NOTIFICATION_ID = 1
        private const val NOTIFY_INTERVAL_MS = 10_000L
    }

    private var currentSlug: String? = null
    private var currentName: String? = null
    private var networkCallback: ConnectivityManager.NetworkCallback? = null
    private val handler = Handler(Looper.getMainLooper())
    private val notifyTick = object : Runnable {
        override fun run() {
            updateNotification()
            handler.postDelayed(this, NOTIFY_INTERVAL_MS)
        }
    }

    override fun onCreate() {
        super.onCreate()
        val channel = NotificationChannel(
            CHANNEL_ID,
            getString(R.string.notif_channel_name),
            NotificationManager.IMPORTANCE_LOW, // 音を鳴らさない常駐通知
        )
        getSystemService(NotificationManager::class.java).createNotificationChannel(channel)
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        when (intent?.action) {
            ACTION_DISCONNECT -> {
                Prefs.setVpnShouldRun(this, false)
                teardown()
                stopForeground(STOP_FOREGROUND_REMOVE)
                stopSelf()
                return START_NOT_STICKY
            }
            ACTION_CONNECT -> {
                val slug = intent.getStringExtra(EXTRA_SLUG)
                if (slug == null) {
                    stopSelf()
                    return START_NOT_STICKY
                }
                Prefs.setVpnShouldRun(this, true)
                Prefs.setLastSlug(this, slug)
                startAsForeground()
                connect(slug)
            }
            else -> {
                // null Intent(プロセス再生成)や Always-on(SERVICE_INTERFACE)。
                // 維持フラグが立っている(または OS が Always-on で起動した)
                // 場合だけ、最後のネットワークへ復帰する
                val alwaysOn = intent?.action == SERVICE_INTERFACE
                val slug = if (Prefs.vpnShouldRun(this) || alwaysOn) Prefs.lastSlug(this) else null
                if (slug == null) {
                    stopSelf()
                    return START_NOT_STICKY
                }
                Log.i(TAG, "サービス再起動から復帰します: $slug(always-on=$alwaysOn)")
                startAsForeground()
                connect(slug)
            }
        }
        return START_STICKY
    }

    private fun startAsForeground() {
        val notification = buildNotification(getString(R.string.notif_preparing))
        if (Build.VERSION.SDK_INT >= 34) {
            startForeground(
                NOTIFICATION_ID,
                notification,
                ServiceInfo.FOREGROUND_SERVICE_TYPE_SYSTEM_EXEMPTED,
            )
        } else {
            startForeground(NOTIFICATION_ID, notification)
        }
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
            currentName = info.name
            watchNetworkChanges(slug)
            handler.removeCallbacks(notifyTick)
            handler.post(notifyTick)
            Log.i(TAG, "トンネル開始: $slug")
        } catch (e: Exception) {
            Log.e(TAG, "接続に失敗: $slug", e)
            stopSelf()
        }
    }

    /** 回線切替(Wi-Fi ↔ モバイル)の検知 → Rust へ UDP の張り直しを依頼。 */
    private fun watchNetworkChanges(slug: String) {
        unwatchNetworkChanges()
        val cm = getSystemService(ConnectivityManager::class.java) ?: return
        val callback = object : ConnectivityManager.NetworkCallback() {
            override fun onAvailable(network: Network) {
                Log.i(TAG, "既定ネットワークが変わりました → 再バインド")
                // メインスレッドを塞がない(pokeTunnel はソケット操作を含む)
                Thread { pokeTunnel(slug) }.start()
            }
        }
        try {
            cm.registerDefaultNetworkCallback(callback)
            networkCallback = callback
        } catch (e: Exception) {
            Log.w(TAG, "ネットワーク監視の登録に失敗", e)
        }
    }

    private fun unwatchNetworkChanges() {
        networkCallback?.let {
            try {
                getSystemService(ConnectivityManager::class.java)?.unregisterNetworkCallback(it)
            } catch (_: Exception) {
            }
        }
        networkCallback = null
    }

    private fun buildNotification(text: String): Notification {
        val open = PendingIntent.getActivity(
            this,
            0,
            Intent(this, MainActivity::class.java),
            PendingIntent.FLAG_IMMUTABLE,
        )
        val disconnect = PendingIntent.getService(
            this,
            1,
            Intent(this, PeercoveVpnService::class.java).setAction(ACTION_DISCONNECT),
            PendingIntent.FLAG_IMMUTABLE,
        )
        return Notification.Builder(this, CHANNEL_ID)
            .setSmallIcon(R.drawable.ic_tile)
            .setContentTitle(currentName?.let { "PeerCove ・$it" } ?: "PeerCove")
            .setContentText(text)
            .setContentIntent(open)
            .setOngoing(true)
            .setOnlyAlertOnce(true)
            .addAction(
                Notification.Action.Builder(
                    null,
                    getString(R.string.notif_action_disconnect),
                    disconnect,
                ).build(),
            )
            .build()
    }

    /** 常駐通知へ状態(接続・RTT・転送量)を反映する(10 秒ごと)。 */
    private fun updateNotification() {
        val slug = currentSlug ?: return
        val status = tunnelStatus(slug)
        val rtt = sessionState(slug)?.rttMs
        val text = when {
            status == null -> getString(R.string.notif_preparing)
            status.handshakeAgeSecs == null ->
                getString(R.string.notif_connecting, status.endpoint)
            rtt != null -> getString(
                R.string.notif_connected_rtt,
                rtt.toLong(),
                formatBytesLong(status.txBytes),
                formatBytesLong(status.rxBytes),
            )
            else -> getString(
                R.string.notif_connected,
                formatBytesLong(status.txBytes),
                formatBytesLong(status.rxBytes),
            )
        }
        getSystemService(NotificationManager::class.java)
            .notify(NOTIFICATION_ID, buildNotification(text))
    }

    /** トンネル停止(冪等)。fd は Rust 側で close され、VPN も終了する */
    private fun teardown() {
        handler.removeCallbacks(notifyTick)
        unwatchNetworkChanges()
        currentSlug?.let {
            stopTunnel(it)
            Log.i(TAG, "トンネル停止: $it")
        }
        currentSlug = null
        currentName = null
    }

    override fun onDestroy() {
        teardown()
        super.onDestroy()
    }

    /** ユーザーが設定画面などから VPN を切った場合 */
    override fun onRevoke() {
        Prefs.setVpnShouldRun(this, false)
        teardown()
        stopForeground(STOP_FOREGROUND_REMOVE)
        stopSelf()
    }
}
