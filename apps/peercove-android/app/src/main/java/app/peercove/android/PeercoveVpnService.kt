package app.peercove.android

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Intent
import android.content.pm.ServiceInfo
import android.net.ConnectivityManager
import android.net.Network
import android.net.NetworkCapabilities
import android.net.NetworkRequest
import android.net.VpnService
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.util.Log
import uniffi.peercove_mobile.SocketProtector
import uniffi.peercove_mobile.commitPendingKey
import uniffi.peercove_mobile.listNetworks
import uniffi.peercove_mobile.pendingKeyExists
import uniffi.peercove_mobile.pokeTunnel
import uniffi.peercove_mobile.rotateKey
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
    /// 更新待ちの鍵(member.key.new)で起動中か(鍵ローテーションの自己回復)
    private var usingPendingKey = false
    /// 監視スレッドの世代(接続し直しで旧スレッドを退役させる)
    @Volatile private var watchGeneration = 0
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

    private fun connect(slug: String, usePendingKey: Boolean = false) {
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
                usePendingKey,
            )
            currentSlug = slug
            currentName = info.name
            usingPendingKey = usePendingKey
            watchNetworkChanges(slug)
            startKeyWatchdog(slug)
            handler.removeCallbacks(notifyTick)
            handler.post(notifyTick)
            Log.i(TAG, "トンネル開始: $slug(pendingKey=$usePendingKey)")
        } catch (e: Exception) {
            Log.e(TAG, "接続に失敗: $slug", e)
            stopSelf()
        }
    }

    /**
     * 鍵ローテーションの監視(ADR-0020 のモバイル版 = ADR-0044)。
     * - 招待由来の鍵で同期できたら自動で鍵を更新して接続し直す
     * - 更新待ちの鍵(応答喪失の名残)があるのに 45 秒疎通しないときは、
     *   確定鍵と更新待ちの鍵を切り替えて試す(締め出しからの自己回復)
     * - 更新待ちの鍵で疎通できたら確定する
     */
    private fun startKeyWatchdog(slug: String) {
        val gen = ++watchGeneration
        Thread {
            val base = filesDir.absolutePath
            var lastHandshakeAt = System.currentTimeMillis()
            var rotateAttempted = false
            var deadSince = System.currentTimeMillis()
            var lastPokeAt = 0L
            while (gen == watchGeneration && currentSlug == slug) {
                Thread.sleep(5000)
                if (gen != watchGeneration || currentSlug != slug) break
                val status = tunnelStatus(slug) ?: continue
                val now = System.currentTimeMillis()
                // 自己回復(NetworkCallback が来ない機種・状況向け)。
                // 早い経路: UDP 送信自体が失敗し続けている = 回線消失の確実な
                // シグナル(keepalive が 25 秒ごとに送信されるので数十秒以内に
                // 立つ)→ 10 秒間隔で張り直す
                if (status.sendFailing && now - lastPokeAt > 10_000) {
                    lastPokeAt = now
                    Log.i(TAG, "UDP 送信が失敗しているため張り直します")
                    pokeTunnel(slug)
                }
                // 遅い経路(最後の保険): 健全なら keepalive によりハンドシェイクは
                // 2〜3 分ごとに更新されるので、「未確立 or 150 秒超」が 30 秒
                // 続いたら 30 秒間隔で張り直す。
                // ※受信バイト数は keepalive(ペイロード 0)で増えないため
                //   判定に使わない(誤検知して健全な接続を揺らした実機報告)
                val age = status.handshakeAgeSecs?.toLong()
                if (age != null && age <= 150) {
                    deadSince = now
                } else if (now - deadSince > 30_000 && now - lastPokeAt > 30_000) {
                    lastPokeAt = now
                    Log.i(TAG, "疎通が途絶えているため UDP を張り直します")
                    pokeTunnel(slug)
                }
                if (status.handshakeAgeSecs != null) {
                    lastHandshakeAt = now
                    if (usingPendingKey) {
                        // 更新待ちの鍵で疎通できた = ホストは新鍵を登録済み → 確定
                        try {
                            commitPendingKey(base, slug)
                            usingPendingKey = false
                            Log.i(TAG, "更新待ちの鍵で疎通できたため確定しました")
                        } catch (e: Exception) {
                            Log.w(TAG, "鍵の確定に失敗", e)
                        }
                    }
                    if (!rotateAttempted && sessionState(slug)?.controlConnected == true) {
                        rotateAttempted = true
                        val rotated = listNetworks(base)
                            .firstOrNull { it.slug == slug }?.keyRotated ?: true
                        if (!rotated) {
                            try {
                                rotateKey(base, slug)
                                Log.i(TAG, "鍵を自動更新しました(新しい鍵で接続し直します)")
                                handler.post { if (currentSlug == slug) connect(slug) }
                                break
                            } catch (e: Exception) {
                                Log.w(TAG, "鍵の自動更新に失敗(次回接続時に再試行)", e)
                            }
                        }
                    }
                } else if (now - lastHandshakeAt > 45_000) {
                    if (pendingKeyExists(base, slug)) {
                        // 鍵の不一致(更新応答の喪失)を疑い、もう一方の鍵で試す
                        val next = !usingPendingKey
                        Log.i(TAG, "疎通しないため鍵を切り替えて再接続します(pending=$next)")
                        handler.post { if (currentSlug == slug) connect(slug, next) }
                        break
                    }
                    lastHandshakeAt = now // 通常の未達はエンジンの自己回復に任せる
                }
            }
        }.start()
    }

    /** 回線切替(Wi-Fi ↔ モバイル)の検知 → Rust へ UDP の張り直しを依頼。
     *
     *  注意: VPN アプリ自身の「既定ネットワーク」は VPN そのものになるため、
     *  registerDefaultNetworkCallback では下回りの Wi-Fi ↔ モバイル切替が
     *  見えない(実機で観測)。INTERNET 能力を持つ実ネットワーク
     *  (NetworkRequest は既定で VPN を除外する)を監視する。 */
    private fun watchNetworkChanges(slug: String) {
        unwatchNetworkChanges()
        val cm = getSystemService(ConnectivityManager::class.java) ?: return
        val request = NetworkRequest.Builder()
            .addCapability(NetworkCapabilities.NET_CAPABILITY_INTERNET)
            .build()
        val callback = object : ConnectivityManager.NetworkCallback() {
            // Wi-Fi とモバイルが同時に居る端末ではイベントが連続するため間引く
            @Volatile private var lastPokeAt = 0L

            private fun poke(reason: String) {
                val now = System.currentTimeMillis()
                if (now - lastPokeAt < 3_000) return
                lastPokeAt = now
                Log.i(TAG, "$reason → 再バインド")
                // メインスレッドを塞がない(pokeTunnel はソケット操作を含む)
                Thread { pokeTunnel(slug) }.start()
            }

            override fun onAvailable(network: Network) {
                poke("実ネットワークが利用可能になりました($network)")
            }

            override fun onLost(network: Network) {
                // 使っていた回線が消えた → 残っている回線で張り直す
                poke("実ネットワークが失われました($network)")
            }
        }
        try {
            cm.registerNetworkCallback(request, callback)
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
        watchGeneration++ // 監視スレッドを退役させる
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
