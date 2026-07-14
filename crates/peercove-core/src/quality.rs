//! 通信品質履歴の OS 非依存モデル(M3-23)。

use std::net::Ipv4Addr;

use serde::{Deserialize, Serialize};

/// 1 分窓で観測した制御接続の状態。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QualityAvailability {
    Connected,
    Disconnected,
    /// この端末からは制御 Ping の対象でないピア(メンバー同士など)。
    Unmeasured,
}

/// 1 分窓で最も長く使われた経路。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QualityRoute {
    Direct,
    Relay,
    Trying,
}

/// ピア 1 台・1 分分の品質集計。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QualitySample {
    pub window_start_unix_ms: u64,
    pub window_secs: u32,
    pub public_key: String,
    pub ip: Ipv4Addr,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub availability: QualityAvailability,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rtt_latest_ms: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rtt_min_ms: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rtt_avg_ms: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rtt_p95_ms: Option<f64>,
    /// 連続する RTT 標本の差の絶対値の平均。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jitter_ms: Option<f64>,
    pub probes_sent: u32,
    pub probes_received: u32,
    /// 接続中に送った Ping がある窓だけ値を持つ。切断中は `None`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loss_percent: Option<f64>,
    pub route: QualityRoute,
    pub route_switches: u32,
    pub rx_bytes: u64,
    pub tx_bytes: u64,
}

/// IPC で返す品質履歴。壊れた JSONL 行は読み飛ばした件数を通知する。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QualityReport {
    pub generated_at_unix_ms: u64,
    pub retention_days: u32,
    #[serde(default)]
    pub skipped_corrupt_lines: u32,
    #[serde(default)]
    pub samples: Vec<QualitySample>,
}
