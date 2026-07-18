package app.peercove.android

import android.app.Activity
import android.content.Context
import android.content.Intent
import android.net.VpnService
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.compose.setContent
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.Button
import androidx.compose.material3.Card
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
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
 * M4 E-B の画面: 招待トークンで参加(貼り付け / QR 読み取り)、
 * ネットワークごとの接続・切断・状態表示。
 */
class MainActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        initLogging()
        setContent {
            MaterialTheme {
                Surface(modifier = Modifier.fillMaxSize()) { App() }
            }
        }
    }
}

private fun startVpnService(context: Context, slug: String) {
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
private fun App() {
    val context = LocalContext.current
    val baseDir = context.filesDir.absolutePath

    var networks by remember { mutableStateOf(listNetworks(baseDir)) }
    var statuses by remember { mutableStateOf(mapOf<String, TunnelStatus?>()) }
    var tokenInput by remember { mutableStateOf("") }
    var message by remember { mutableStateOf<String?>(null) }
    var pendingSlug by remember { mutableStateOf<String?>(null) }

    fun refresh() {
        networks = listNetworks(baseDir)
    }

    // 2 秒ごとに稼働状態を更新(正本は Rust 側のトンネル登録簿)
    LaunchedEffect(Unit) {
        while (true) {
            statuses = networks.associate { it.slug to tunnelStatus(it.slug) }
            delay(2000)
        }
    }

    // VPN 権限ダイアログ(初回のみ OS が表示)
    val vpnPermissionLauncher = rememberLauncherForActivityResult(
        ActivityResultContracts.StartActivityForResult(),
    ) { result ->
        val slug = pendingSlug
        pendingSlug = null
        if (result.resultCode == Activity.RESULT_OK && slug != null) {
            startVpnService(context, slug)
        } else {
            message = "VPN の使用が許可されませんでした"
        }
    }

    fun connect(slug: String) {
        val prepare = VpnService.prepare(context)
        if (prepare == null) {
            startVpnService(context, slug)
        } else {
            pendingSlug = slug
            vpnPermissionLauncher.launch(prepare)
        }
    }

    // 招待 QR の読み取り(zxing)。トークン文字列 or ディープリンクが入っている
    val scanLauncher = rememberLauncherForActivityResult(ScanContract()) { result ->
        result.contents?.let { tokenInput = it }
    }

    Column(modifier = Modifier.fillMaxSize().padding(16.dp)) {
        Text("PeerCove", style = MaterialTheme.typography.headlineMedium)
        Text(
            "mobile core v${remember { coreVersion() }}",
            style = MaterialTheme.typography.bodySmall,
        )
        Spacer(modifier = Modifier.padding(4.dp))

        message?.let {
            Text(it, color = MaterialTheme.colorScheme.error, style = MaterialTheme.typography.bodyMedium)
            Spacer(modifier = Modifier.padding(4.dp))
        }

        JoinCard(
            token = tokenInput,
            onTokenChange = { tokenInput = it },
            onScan = {
                scanLauncher.launch(
                    ScanOptions()
                        .setDesiredBarcodeFormats(ScanOptions.QR_CODE)
                        .setPrompt("招待 QR コードを読み取ってください")
                        .setBeepEnabled(false)
                        .setOrientationLocked(false),
                )
            },
            onJoin = joinAction@{
                val token = tokenInput.trim()
                if (token.isEmpty()) {
                    message = "招待コードを入力するか QR を読み取ってください"
                    return@joinAction
                }
                try {
                    val info = joinNetwork(baseDir, token)
                    message = "「${info.name}」に参加しました"
                    tokenInput = ""
                    refresh()
                } catch (e: MobileException) {
                    message = e.message
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
                    onRemove = {
                        try {
                            removeNetwork(baseDir, network.slug)
                            refresh()
                        } catch (e: MobileException) {
                            message = e.message
                        }
                    },
                )
            }
        }
    }
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
            Text("ネットワークに参加", style = MaterialTheme.typography.titleMedium)
            OutlinedTextField(
                value = token,
                onValueChange = onTokenChange,
                modifier = Modifier.fillMaxWidth(),
                label = { Text("招待コード(pcv1.…)") },
                minLines = 1,
                maxLines = 3,
            )
            Spacer(modifier = Modifier.padding(4.dp))
            Row {
                OutlinedButton(onClick = onScan) { Text("QR を読み取り") }
                Spacer(modifier = Modifier.width(8.dp))
                Button(onClick = onJoin) { Text("参加") }
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
    onRemove: () -> Unit,
) {
    Card(modifier = Modifier.fillMaxWidth()) {
        Column(modifier = Modifier.padding(12.dp)) {
            Text(network.name, style = MaterialTheme.typography.titleMedium)
            Text(
                "自分: ${network.memberIp}(${network.displayName})/ ホスト: ${network.hostIp}",
                style = MaterialTheme.typography.bodyMedium,
            )
            Text("接続先: ${network.endpoint}", style = MaterialTheme.typography.bodySmall)
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
                    Button(onClick = onConnect) { Text("接続") }
                    Spacer(modifier = Modifier.width(8.dp))
                    TextButton(onClick = onRemove) { Text("削除") }
                } else {
                    Button(onClick = onDisconnect) { Text("切断") }
                }
            }
        }
    }
}

private fun statusLine(status: TunnelStatus?): String {
    if (status == null) return "未接続"
    val age = status.handshakeAgeSecs ?: return "接続試行中…(ハンドシェイク待ち)"
    return "接続中(ハンドシェイク ${age} 秒前 / ↑${formatBytes(status.txBytes)} ↓${formatBytes(status.rxBytes)})"
}

private fun formatBytes(bytes: ULong): String {
    val b = bytes.toLong()
    return when {
        b >= 1_048_576 -> "%.1f MB".format(b / 1_048_576.0)
        b >= 1_024 -> "%.1f KB".format(b / 1_024.0)
        else -> "$b B"
    }
}
