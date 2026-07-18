package app.peercove.android

import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import uniffi.peercove_mobile.coreVersion
import uniffi.peercove_mobile.probeCore

/**
 * M4 E-A(土台)の疎通確認画面。
 * Rust(crates/peercove-mobile)の関数を UniFFI 経由で呼び、
 * peercove-core の暗号が実機で動くことを表示する。
 * 本格的な画面は E-B 以降で作る。
 */
class MainActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        setContent {
            MaterialTheme {
                Surface(modifier = Modifier.fillMaxSize()) {
                    // remember: 再コンポーズのたびに鍵を作り直さない
                    val version = remember { coreVersion() }
                    val probe = remember { probeCore() }
                    Column(
                        modifier = Modifier.fillMaxSize().padding(24.dp),
                        verticalArrangement = Arrangement.spacedBy(12.dp, Alignment.CenterVertically),
                        horizontalAlignment = Alignment.CenterHorizontally,
                    ) {
                        Text("PeerCove", style = MaterialTheme.typography.headlineLarge)
                        Text("mobile core v$version", style = MaterialTheme.typography.bodyLarge)
                        Text(probe, style = MaterialTheme.typography.bodySmall)
                    }
                }
            }
        }
    }
}
