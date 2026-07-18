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
 * M4 E-C の画面: 参加(貼り付け / QR)、接続・切断、ネットワーク詳細
 * (トーク / メンバー / DNS)への遷移。
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

private sealed class Route {
    data object Home : Route()
    data class Net(val slug: String, val name: String) : Route()
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
    var route by remember { mutableStateOf<Route>(Route.Home) }
    var notice by remember { mutableStateOf<String?>(null) }

    Column(modifier = Modifier.fillMaxSize()) {
        notice?.let {
            Text(
                it,
                modifier = Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 4.dp),
                color = MaterialTheme.colorScheme.primary,
                style = MaterialTheme.typography.bodySmall,
            )
        }
        when (val r = route) {
            is Route.Home -> HomeScreen(
                onNotice = { notice = it },
                onOpen = { slug, name -> route = Route.Net(slug, name) },
            )
            is Route.Net -> NetworkScreen(
                slug = r.slug,
                networkName = r.name,
                onBack = { route = Route.Home },
                onNotice = { notice = it },
            )
        }
    }
}

@Composable
private fun HomeScreen(onNotice: (String) -> Unit, onOpen: (String, String) -> Unit) {
    val context = LocalContext.current
    val baseDir = context.filesDir.absolutePath

    var networks by remember { mutableStateOf(listNetworks(baseDir)) }
    var statuses by remember { mutableStateOf(mapOf<String, TunnelStatus?>()) }
    var tokenInput by remember { mutableStateOf("") }
    var pendingSlug by remember { mutableStateOf<String?>(null) }

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
            onNotice("VPN の使用が許可されませんでした")
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
                    onNotice("招待コードを入力するか QR を読み取ってください")
                    return@joinAction
                }
                try {
                    val info = joinNetwork(baseDir, token)
                    onNotice("「${info.name}」に参加しました")
                    tokenInput = ""
                    refresh()
                } catch (e: MobileException) {
                    onNotice(e.message ?: "参加に失敗しました")
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
                    onRemove = {
                        try {
                            removeNetwork(baseDir, network.slug)
                            refresh()
                        } catch (e: MobileException) {
                            onNotice(e.message ?: "削除に失敗しました")
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
    onOpen: () -> Unit,
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
                    Button(onClick = onOpen) { Text("開く") }
                    Spacer(modifier = Modifier.width(8.dp))
                    OutlinedButton(onClick = onDisconnect) { Text("切断") }
                }
            }
        }
    }
}

private fun statusLine(status: TunnelStatus?): String {
    if (status == null) return "未接続"
    val age = status.handshakeAgeSecs ?: return "接続試行中…(ハンドシェイク待ち)"
    return "接続中(ハンドシェイク ${age} 秒前 / ↑${formatBytesLong(status.txBytes)} ↓${formatBytesLong(status.rxBytes)})"
}
