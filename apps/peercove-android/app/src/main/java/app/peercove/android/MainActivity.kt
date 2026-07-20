package app.peercove.android

import android.app.Activity
import android.content.Context
import android.content.Intent
import android.net.Uri
import android.net.VpnService
import android.os.Build
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.BackHandler
import androidx.activity.enableEdgeToEdge
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.compose.setContent
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.clickable
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.EditNote
import androidx.compose.material.icons.filled.Settings
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.Card
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.RadioButton
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.SideEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import androidx.core.view.WindowCompat
import com.journeyapps.barcodescanner.ScanContract
import com.journeyapps.barcodescanner.ScanOptions
import kotlinx.coroutines.delay
import uniffi.peercove_mobile.MobileException
import uniffi.peercove_mobile.NetworkInfo
import uniffi.peercove_mobile.TunnelStatus
import uniffi.peercove_mobile.coreVersion
import uniffi.peercove_mobile.initLogging
import uniffi.peercove_mobile.joinNetwork
import uniffi.peercove_mobile.listNetworks
import uniffi.peercove_mobile.removeNetwork
import uniffi.peercove_mobile.tunnelStatus

/**
 * M4 E-C の画面: 参加(貼り付け / QR)、接続・切断、ネットワーク詳細
 * (トーク / メンバー / DNS)への遷移。共有シート(ACTION_SEND)経由の
 * ファイル送信もここで受ける。
 */
class MainActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        // ウィンドウ自体の IME リサイズをやめて、余白の管理を Compose の
        // インセット(windowInsetsPadding / imePadding)へ一本化する。
        // これが無いと「ウィンドウ縮小 + imePadding」の二重でキーボード上の
        // 余白が大きくなりすぎる
        enableEdgeToEdge()
        initLogging()
        val shareUri = extractShareUri(intent)
        // チャット通知のタップ → 該当ネットワークの会話を開く
        val openSlug = intent?.getStringExtra(ChatNotifier.EXTRA_SLUG)
        val openConv = intent?.getStringExtra(ChatNotifier.EXTRA_CONV)
        val openTarget = openSlug?.let { slug ->
            listNetworks(filesDir.absolutePath).firstOrNull { it.slug == slug }
                ?.let { Triple(slug, it.name, openConv) }
        }
        setContent {
            val context = LocalContext.current
            var theme by remember { mutableStateOf(Prefs.theme(context)) }
            val dark = when (theme) {
                "light" -> false
                "dark" -> true
                else -> isSystemInDarkTheme()
            }
            MaterialTheme(colorScheme = if (dark) darkColorScheme() else lightColorScheme()) {
                // ステータスバー・ナビバーのシステム文字色を背景に合わせる
                // (ライト背景に白文字で時計・鍵アイコンが見えない実機報告)
                val view = LocalView.current
                SideEffect {
                    WindowCompat.getInsetsController(window, view).apply {
                        isAppearanceLightStatusBars = !dark
                        isAppearanceLightNavigationBars = !dark
                    }
                }
                Surface(modifier = Modifier.fillMaxSize()) {
                    App(
                        shareUri = shareUri,
                        openTarget = openTarget,
                        theme = theme,
                        onThemeChange = {
                            theme = it
                            Prefs.setTheme(context, it)
                        },
                    )
                }
            }
        }
    }

    override fun onResume() {
        super.onResume()
        AppState.visible = true // 表示中はチャット通知を出さない(バッジで見える)
    }

    override fun onPause() {
        super.onPause()
        AppState.visible = false
    }

    /** 共有シート(ACTION_SEND)で渡されたファイルの URI。 */
    private fun extractShareUri(intent: Intent?): Uri? {
        if (intent?.action != Intent.ACTION_SEND) return null
        return if (Build.VERSION.SDK_INT >= 33) {
            intent.getParcelableExtra(Intent.EXTRA_STREAM, Uri::class.java)
        } else {
            @Suppress("DEPRECATION")
            intent.getParcelableExtra(Intent.EXTRA_STREAM)
        }
    }
}

private sealed class Route {
    data object Home : Route()
    /** 個人メモ(M5 F-1、ADR-0049)。ネットワーク非依存なのでホーム直下 */
    data object Memos : Route()
    data class Net(val slug: String, val name: String) : Route()
}

fun startVpnService(context: Context, slug: String) {
    Prefs.setLastSlug(context, slug) // クイック設定タイルの接続先に使う
    val intent = Intent(context, PeercoveVpnService::class.java)
        .setAction(PeercoveVpnService.ACTION_CONNECT)
        .putExtra(PeercoveVpnService.EXTRA_SLUG, slug)
    context.startService(intent)
}

private fun stopVpnService(context: Context) {
    val intent = Intent(context, PeercoveVpnService::class.java)
        .setAction(PeercoveVpnService.ACTION_DISCONNECT)
    context.startService(intent)
}

@Composable
private fun App(
    shareUri: Uri?,
    openTarget: Triple<String, String, String?>?, // (slug, name, convId) 通知タップから
    theme: String,
    onThemeChange: (String) -> Unit,
) {
    var route by remember {
        mutableStateOf<Route>(
            if (openTarget != null) Route.Net(openTarget.first, openTarget.second) else Route.Home,
        )
    }
    var pendingConv by remember { mutableStateOf(openTarget?.third) }
    var notice by remember { mutableStateOf<String?>(null) }
    var noticeCount by remember { mutableStateOf(0) }
    var pendingShare by remember { mutableStateOf(shareUri) }

    // システムの戻る操作でひとつ前の画面へ(アプリを閉じない)
    BackHandler(enabled = route !is Route.Home) { route = Route.Home }

    val onNotice: (String) -> Unit = {
        notice = it
        noticeCount++ // 同文の連続通知でもタイマーを引き直す
    }
    // 通知は出しっぱなしにせず時間経過で消す
    LaunchedEffect(noticeCount) {
        if (notice != null) {
            delay(5000)
            notice = null
        }
    }

    // エッジツーエッジ対策の余白はここに一本化する。safeDrawing はステータス
    // バー・ナビバー・カメラ切欠き・キーボード(ime)の合成(union = 最大値)
    // なので、キーボード表示中は「キーボードのすぐ上」まで自動で持ち上がる。
    // ウィンドウ側のリサイズはマニフェストの adjustNothing で止めてある
    // (二重の持ち上がり防止)
    Column(modifier = Modifier.fillMaxSize().safeDrawingPadding()) {
        notice?.let {
            Text(
                it,
                modifier = Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 4.dp),
                color = MaterialTheme.colorScheme.primary,
                style = MaterialTheme.typography.bodySmall,
            )
        }
        val share = pendingShare
        if (share != null) {
            ShareSendScreen(
                uri = share,
                onNotice = onNotice,
                onClose = { pendingShare = null },
            )
            return@Column
        }
        when (val r = route) {
            is Route.Home -> HomeScreen(
                theme = theme,
                onThemeChange = onThemeChange,
                onNotice = onNotice,
                onOpen = { slug, name -> route = Route.Net(slug, name) },
                onOpenMemos = { route = Route.Memos },
            )
            is Route.Memos -> MemoScreen(
                onBack = { route = Route.Home },
                onNotice = onNotice,
            )
            is Route.Net -> NetworkScreen(
                slug = r.slug,
                networkName = r.name,
                initialConvId = pendingConv,
                onBack = {
                    pendingConv = null
                    route = Route.Home
                },
                onNotice = onNotice,
            )
        }
    }
}

@Composable
private fun HomeScreen(
    theme: String,
    onThemeChange: (String) -> Unit,
    onNotice: (String) -> Unit,
    onOpen: (String, String) -> Unit,
    onOpenMemos: () -> Unit,
) {
    val context = LocalContext.current
    val baseDir = context.filesDir.absolutePath
    var showThemeDialog by remember { mutableStateOf(false) }

    var networks by remember { mutableStateOf(listNetworks(baseDir)) }
    var statuses by remember { mutableStateOf(mapOf<String, TunnelStatus?>()) }
    var tokenInput by remember { mutableStateOf("") }
    var pendingSlug by remember { mutableStateOf<String?>(null) }
    // 削除確認中のネットワーク(slug, 表示名)。誤タップで即消えるのを防ぐ
    var confirmRemove by remember { mutableStateOf<Pair<String, String>?>(null) }
    val vpnDenied = stringResource(R.string.vpn_denied)
    val emptyToken = stringResource(R.string.join_empty_token)
    val joinFailed = stringResource(R.string.join_failed)
    val joinSuccess = stringResource(R.string.join_success)
    val removeFailed = stringResource(R.string.remove_failed)
    val scanPrompt = stringResource(R.string.scan_prompt)

    fun refresh() {
        networks = listNetworks(baseDir)
    }

    LaunchedEffect(Unit) {
        while (true) {
            statuses = networks.associate { it.slug to tunnelStatus(it.slug) }
            delay(2000)
        }
    }

    val vpnPermissionLauncher = rememberLauncherForActivityResult(
        ActivityResultContracts.StartActivityForResult(),
    ) { result ->
        val slug = pendingSlug
        pendingSlug = null
        if (result.resultCode == Activity.RESULT_OK && slug != null) {
            startVpnService(context, slug)
        } else {
            onNotice(vpnDenied)
        }
    }
    // 常駐通知(E-D)のための通知権限(Android 13+)。拒否されても接続はできる
    // (通知が出ないだけ)
    val notifPermissionLauncher = rememberLauncherForActivityResult(
        ActivityResultContracts.RequestPermission(),
    ) { }

    fun connect(slug: String) {
        if (Build.VERSION.SDK_INT >= 33 &&
            context.checkSelfPermission(android.Manifest.permission.POST_NOTIFICATIONS) !=
            android.content.pm.PackageManager.PERMISSION_GRANTED
        ) {
            notifPermissionLauncher.launch(android.Manifest.permission.POST_NOTIFICATIONS)
        }
        val prepare = VpnService.prepare(context)
        if (prepare == null) {
            startVpnService(context, slug)
        } else {
            pendingSlug = slug
            vpnPermissionLauncher.launch(prepare)
        }
    }

    val scanLauncher = rememberLauncherForActivityResult(ScanContract()) { result ->
        result.contents?.let { tokenInput = it }
    }

    confirmRemove?.let { (slug, name) ->
        AlertDialog(
            onDismissRequest = { confirmRemove = null },
            title = { Text(stringResource(R.string.remove_confirm_title)) },
            text = { Text(stringResource(R.string.remove_confirm_body, name)) },
            confirmButton = {
                TextButton(onClick = {
                    confirmRemove = null
                    try {
                        removeNetwork(baseDir, slug)
                        refresh()
                    } catch (e: MobileException) {
                        onNotice(e.message ?: removeFailed)
                    }
                }) { Text(stringResource(R.string.action_remove)) }
            },
            dismissButton = {
                TextButton(onClick = { confirmRemove = null }) {
                    Text(stringResource(R.string.action_cancel))
                }
            },
        )
    }

    if (showThemeDialog) {
        ThemeDialog(
            current = theme,
            onSelect = {
                onThemeChange(it)
                showThemeDialog = false
            },
            onDismiss = { showThemeDialog = false },
        )
    }

    Column(modifier = Modifier.fillMaxSize().padding(16.dp)) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            Column(modifier = Modifier.weight(1f)) {
                Text("PeerCove", style = MaterialTheme.typography.headlineMedium)
                Text(
                    stringResource(R.string.mobile_core_version, remember { coreVersion() }),
                    style = MaterialTheme.typography.bodySmall,
                )
            }
            IconButton(onClick = onOpenMemos) {
                Icon(
                    Icons.Filled.EditNote,
                    contentDescription = stringResource(R.string.memo_open),
                    tint = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
            IconButton(onClick = { showThemeDialog = true }) {
                Icon(
                    Icons.Filled.Settings,
                    contentDescription = stringResource(R.string.theme_open),
                    tint = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
        }
        Spacer(modifier = Modifier.padding(4.dp))

        JoinCard(
            token = tokenInput,
            onTokenChange = { tokenInput = it },
            onScan = {
                scanLauncher.launch(
                    ScanOptions()
                        .setDesiredBarcodeFormats(ScanOptions.QR_CODE)
                        .setPrompt(scanPrompt)
                        .setBeepEnabled(false)
                        // 既定の CaptureActivity は横向きなので縦固定版を使う
                        .setCaptureActivity(PortraitCaptureActivity::class.java)
                        .setOrientationLocked(true),
                )
            },
            onJoin = joinAction@{
                val token = tokenInput.trim()
                if (token.isEmpty()) {
                    onNotice(emptyToken)
                    return@joinAction
                }
                try {
                    val info = joinNetwork(baseDir, token)
                    onNotice(joinSuccess.format(info.name))
                    tokenInput = ""
                    refresh()
                } catch (e: MobileException) {
                    onNotice(e.message ?: joinFailed)
                }
            },
        )

        Spacer(modifier = Modifier.padding(8.dp))

        LazyColumn(verticalArrangement = Arrangement.spacedBy(8.dp)) {
            items(networks, key = { it.slug }) { network ->
                NetworkCard(
                    network = network,
                    status = statuses[network.slug],
                    onConnect = { connect(network.slug) },
                    onDisconnect = { stopVpnService(context) },
                    onOpen = { onOpen(network.slug, network.name) },
                    onRemove = { confirmRemove = network.slug to network.name },
                )
            }
        }
    }
}

/** 表示テーマの選択(システム / ライト / ダーク)。 */
@Composable
private fun ThemeDialog(
    current: String,
    onSelect: (String) -> Unit,
    onDismiss: () -> Unit,
) {
    val options = listOf(
        "system" to stringResource(R.string.theme_system),
        "light" to stringResource(R.string.theme_light),
        "dark" to stringResource(R.string.theme_dark),
    )
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(stringResource(R.string.theme_title)) },
        text = {
            Column {
                options.forEach { (value, label) ->
                    Row(
                        modifier = Modifier
                            .fillMaxWidth()
                            .clickable { onSelect(value) },
                        verticalAlignment = Alignment.CenterVertically,
                    ) {
                        RadioButton(selected = current == value, onClick = { onSelect(value) })
                        Text(label)
                    }
                }
            }
        },
        confirmButton = {
            TextButton(onClick = onDismiss) { Text(stringResource(R.string.action_close)) }
        },
    )
}

@Composable
private fun JoinCard(
    token: String,
    onTokenChange: (String) -> Unit,
    onScan: () -> Unit,
    onJoin: () -> Unit,
) {
    Card(modifier = Modifier.fillMaxWidth()) {
        Column(modifier = Modifier.padding(12.dp)) {
            Text(stringResource(R.string.join_title), style = MaterialTheme.typography.titleMedium)
            OutlinedTextField(
                value = token,
                onValueChange = onTokenChange,
                modifier = Modifier.fillMaxWidth(),
                label = { Text(stringResource(R.string.join_token_label)) },
                minLines = 1,
                maxLines = 3,
            )
            Spacer(modifier = Modifier.padding(4.dp))
            Row {
                OutlinedButton(onClick = onScan) { Text(stringResource(R.string.join_scan)) }
                Spacer(modifier = Modifier.width(8.dp))
                Button(onClick = onJoin) { Text(stringResource(R.string.join_submit)) }
            }
        }
    }
}

@Composable
private fun NetworkCard(
    network: NetworkInfo,
    status: TunnelStatus?,
    onConnect: () -> Unit,
    onDisconnect: () -> Unit,
    onOpen: () -> Unit,
    onRemove: () -> Unit,
) {
    Card(modifier = Modifier.fillMaxWidth()) {
        Column(modifier = Modifier.padding(12.dp)) {
            Text(network.name, style = MaterialTheme.typography.titleMedium)
            Text(
                stringResource(
                    R.string.net_self_line,
                    network.memberIp,
                    network.displayName,
                    network.hostIp,
                ),
                style = MaterialTheme.typography.bodyMedium,
            )
            Text(
                stringResource(R.string.net_endpoint_line, network.endpoint),
                style = MaterialTheme.typography.bodySmall,
            )
            Spacer(modifier = Modifier.padding(2.dp))
            Text(
                statusLine(status),
                style = MaterialTheme.typography.bodyMedium,
                color = if (status?.handshakeAgeSecs != null) {
                    MaterialTheme.colorScheme.primary
                } else {
                    MaterialTheme.colorScheme.onSurfaceVariant
                },
            )
            Spacer(modifier = Modifier.padding(4.dp))
            Row {
                if (status == null) {
                    Button(onClick = onConnect) { Text(stringResource(R.string.action_connect)) }
                    Spacer(modifier = Modifier.width(8.dp))
                    TextButton(onClick = onRemove) { Text(stringResource(R.string.action_remove)) }
                } else {
                    Button(onClick = onOpen) { Text(stringResource(R.string.action_open)) }
                    Spacer(modifier = Modifier.width(8.dp))
                    OutlinedButton(onClick = onDisconnect) {
                        Text(stringResource(R.string.action_disconnect))
                    }
                }
            }
        }
    }
}

@Composable
private fun statusLine(status: TunnelStatus?): String {
    if (status == null) return stringResource(R.string.status_disconnected)
    val age = status.handshakeAgeSecs
        ?: return stringResource(R.string.status_connecting, status.endpoint)
    return stringResource(
        R.string.status_connected,
        status.endpoint,
        age.toLong(),
        formatBytesLong(status.txBytes),
        formatBytesLong(status.rxBytes),
    )
}
