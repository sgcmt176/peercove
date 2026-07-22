//! 読み取り専用の接続診断モデル(M3-21、ADR-0030)。

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::ipc::LogLine;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticStatus {
    Pass,
    Warning,
    Fail,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticOverall {
    Healthy,
    Attention,
    Problem,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticCategory {
    App,
    Tunnel,
    Internet,
    Dns,
    Permissions,
    /// 共有メモ(M5 F-3)。DB 容量・WAL サイズなどの健全性。
    Memo,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiagnosticScope {
    pub config: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiagnosticCheck {
    /// 翻訳・将来互換のため変更しない固定 ID。
    pub id: String,
    pub category: DiagnosticCategory,
    pub status: DiagnosticStatus,
    /// 秘密を含まない構造化された根拠。表示文そのものは UI が翻訳する。
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub evidence: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DiagnosticReport {
    pub generated_at_unix_ms: u64,
    pub scope: DiagnosticScope,
    pub overall: DiagnosticOverall,
    pub checks: Vec<DiagnosticCheck>,
    /// 直近ログ。デーモン側で deny-list による二重の redact を適用済み。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub logs: Vec<LogLine>,
}

impl DiagnosticReport {
    pub fn calculate_overall(checks: &[DiagnosticCheck]) -> DiagnosticOverall {
        if checks
            .iter()
            .any(|check| check.status == DiagnosticStatus::Fail)
        {
            DiagnosticOverall::Problem
        } else if checks.iter().any(|check| {
            matches!(
                check.status,
                DiagnosticStatus::Warning | DiagnosticStatus::Unknown
            )
        }) {
            DiagnosticOverall::Attention
        } else {
            DiagnosticOverall::Healthy
        }
    }
}

/// 診断エクスポート前の最後の防壁。秘密らしい行は一部だけ残さず行全体を隠す。
pub fn redact_log_line(line: &LogLine) -> LogLine {
    let lower = line.message.to_ascii_lowercase();
    const DENY: [&str; 6] = [
        "private_key",
        "private key",
        "preshared",
        "psk",
        "invite token",
        "peercove://join?token=",
    ];
    let mut redacted = line.clone();
    if DENY.iter().any(|needle| lower.contains(needle)) {
        redacted.message = "[REDACTED: sensitive log line]".to_string();
    }
    redacted
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overall_uses_worst_check() {
        let check = |status| DiagnosticCheck {
            id: "x".into(),
            category: DiagnosticCategory::App,
            status,
            evidence: BTreeMap::new(),
        };
        assert_eq!(
            DiagnosticReport::calculate_overall(&[check(DiagnosticStatus::Pass)]),
            DiagnosticOverall::Healthy
        );
        assert_eq!(
            DiagnosticReport::calculate_overall(&[check(DiagnosticStatus::Unknown)]),
            DiagnosticOverall::Attention
        );
        assert_eq!(
            DiagnosticReport::calculate_overall(&[check(DiagnosticStatus::Fail)]),
            DiagnosticOverall::Problem
        );
    }

    #[test]
    fn redaction_never_exports_secret_bearing_line() {
        let line = LogLine {
            seq: 1,
            unix_ms: 2,
            level: "INFO".into(),
            target: "test".into(),
            message: "invite token peercove://join?token=secret".into(),
        };
        let redacted = redact_log_line(&line);
        assert!(!redacted.message.contains("secret"));
        assert!(!redacted.message.contains("token="));
    }
}
