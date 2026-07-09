//! DNS ラベル・ネットワーク名の正規化(ADR-0011 §2 / ADR-0012 §1)。
//!
//! ネットワーク名はディレクトリ名(`networks/<スラッグ>/`)と DNS の
//! サブドメイン階層(`<メンバー>.<スラッグ>.peercove.internal`)の両方に使う。
//! 全ノードが台帳から同じ結果を得られるよう、導出は純関数にする。

/// ネットワーク名の既定値(旧設定・旧トークンの移行先)。
pub const DEFAULT_NETWORK_NAME: &str = "home";

/// DNS ラベルの最大長(RFC 1035)。
pub const MAX_LABEL_LEN: usize = 63;

/// 表示名を DNS ラベルへ正規化する。
///
/// 1. 小文字化し、`a-z0-9-` 以外を `-` に置換(連続する `-` は 1 つに潰す)
/// 2. 先頭・末尾の `-` を除去
/// 3. 63 文字に切り詰め(切り詰め後も末尾 `-` は除去)
///
/// 結果が空になる場合(日本語名など)は `None`。呼び出し側でフォールバック
/// (`member-<IP第4オクテット>` 等)を選ぶ。
pub fn dns_label(input: &str) -> Option<String> {
    let mut out = String::with_capacity(input.len());
    for c in input.chars() {
        let mapped = match c {
            'A'..='Z' => c.to_ascii_lowercase(),
            'a'..='z' | '0'..='9' => c,
            _ => '-',
        };
        if mapped == '-' && out.ends_with('-') {
            continue; // 連続する区切りは 1 つに
        }
        out.push(mapped);
    }
    let trimmed = out.trim_matches('-');
    let truncated = &trimmed[..trimmed.len().min(MAX_LABEL_LEN)];
    let final_label = truncated.trim_end_matches('-');
    if final_label.is_empty() {
        None
    } else {
        Some(final_label.to_string())
    }
}

/// 文字列が正規化済みの DNS ラベルそのものかどうか。
/// トークンや設定に載ったネットワーク名の検証に使う。
pub fn is_dns_label(input: &str) -> bool {
    dns_label(input).as_deref() == Some(input)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lowercases_and_replaces_unsafe_chars() {
        assert_eq!(dns_label("Alice"), Some("alice".into()));
        assert_eq!(dns_label("My Game LAN"), Some("my-game-lan".into()));
        assert_eq!(dns_label("a_b.c"), Some("a-b-c".into()));
    }

    #[test]
    fn collapses_and_trims_dashes() {
        assert_eq!(dns_label("--a---b--"), Some("a-b".into()));
        assert_eq!(dns_label("あゲームa"), Some("a".into()));
    }

    #[test]
    fn non_ascii_only_becomes_none() {
        assert_eq!(dns_label("たろう"), None);
        assert_eq!(dns_label("---"), None);
        assert_eq!(dns_label(""), None);
    }

    #[test]
    fn truncates_to_63_chars() {
        let long = "a".repeat(100);
        assert_eq!(dns_label(&long).unwrap().len(), MAX_LABEL_LEN);
        // 63 文字目が区切りになるケースでも末尾 `-` は残らない
        let tricky = format!("{}-{}", "a".repeat(62), "b".repeat(10));
        let label = dns_label(&tricky).unwrap();
        assert!(!label.ends_with('-'));
    }

    #[test]
    fn is_dns_label_accepts_only_normalized() {
        assert!(is_dns_label("home"));
        assert!(is_dns_label("my-game-lan"));
        assert!(!is_dns_label("Home"));
        assert!(!is_dns_label("a--b"));
        assert!(!is_dns_label("-a"));
        assert!(!is_dns_label(""));
        assert!(is_dns_label(DEFAULT_NETWORK_NAME));
    }
}
