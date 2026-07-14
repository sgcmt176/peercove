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

/// ドット区切りの相対名(`web.alice` 等、ADR-0022)として正規形かどうか。
/// 各ラベルが [`is_dns_label`] を満たせばよい(単一ラベルも真)。
pub fn is_dns_name(input: &str) -> bool {
    !input.is_empty() && input.split('.').all(is_dns_label)
}

/// カスタムレコードの相対名(`web`, `web.app`, `*.app`)として正規形かどうか
/// (ADR-0024)。先頭ラベルのみ `*`(ワイルドカード)を許し、他は
/// [`is_dns_label`]。ワイルドカードは 1 段(`*.app` が `x.app` に一致)。
pub fn is_custom_dns_name(input: &str) -> bool {
    if input.is_empty() {
        return false;
    }
    let mut labels = input.split('.');
    let first = labels.next().unwrap();
    (first == "*" || is_dns_label(first)) && labels.all(is_dns_label)
}

/// 自由入力の相対名を正規化する(ADR-0024)。各ラベルを [`dns_label`] で
/// 正規化し(小文字化・空白/記号 → ハイフン)、先頭の `*` はそのまま残す。
/// どれかのラベルが空になる(記号だけ・連続ドット・末尾ドット等)場合は `None`。
pub fn normalize_custom_dns_name(input: &str) -> Option<String> {
    let mut out: Vec<String> = Vec::new();
    for (index, label) in input.split('.').enumerate() {
        if index == 0 && label == "*" {
            out.push("*".to_string());
        } else {
            out.push(dns_label(label)?);
        }
    }
    Some(out.join("."))
}

/// DNS 名に使えない予約語(ADR-0021 §4)。将来の内部用途と
/// 紛らわしい名前を先取りで塞ぐ。
pub const RESERVED_DNS_LABELS: &[&str] = &[
    "localhost",
    "local",
    "internal",
    "arpa",
    "peercove",
    "dns",
    "gateway",
    "relay",
];

/// ホスト専用のラベル(`host.<ネットワーク>.peercove.internal`)。
/// メンバーの DNS 名としては予約語扱いで拒否する。
pub const HOST_DNS_LABEL: &str = "host";

/// 未参加メンバーの自動ラベル `member-<第4オクテット>` の形かどうか(ADR-0024)。
/// カスタムレコード名では予約(将来の登録と衝突しうる)。実在メンバー自身の
/// DNS 名としては許可する(重複はメンバー設定側の検証で弾く)。
pub fn is_reserved_member_label(label: &str) -> bool {
    label
        .strip_prefix("member-")
        .is_some_and(|rest| !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit()))
}

/// DNS 名の検証エラー(ADR-0021 §4)。
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum DnsNameError {
    #[error(
        "DNS 名には英数字(a-z、0-9)を 1 文字以上含めてください(使える文字は英数字とハイフンのみ)"
    )]
    Empty,
    #[error("「{0}」は予約されているため DNS 名に使えません")]
    Reserved(String),
}

/// 入力を DNS 名として正規化・検証する(ADR-0021 §4)。
///
/// 正規化は [`dns_label`] と同じ(小文字化・空白/記号はハイフンへ)。
/// `for_host` はホスト自身の名前かどうか([`HOST_DNS_LABEL`] はホスト専用)。
/// **重複チェックは含まない**(ホスト設定全体との突合は呼び出し側 =
/// peercove-ops が行う)。
pub fn normalize_dns_name(input: &str, for_host: bool) -> Result<String, DnsNameError> {
    let label = dns_label(input).ok_or(DnsNameError::Empty)?;
    if RESERVED_DNS_LABELS.contains(&label.as_str()) || (!for_host && label == HOST_DNS_LABEL) {
        return Err(DnsNameError::Reserved(label));
    }
    Ok(label)
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
    fn normalize_dns_name_validates() {
        // 正規化(小文字化・空白→ハイフン)して返す
        assert_eq!(
            normalize_dns_name("Yamada Dev", false),
            Ok("yamada-dev".into())
        );
        // 英数字が残らない入力はエラー
        assert_eq!(
            normalize_dns_name("開発機", false),
            Err(DnsNameError::Empty)
        );
        // 予約語は拒否(正規化後で判定)
        assert_eq!(
            normalize_dns_name("LocalHost", false),
            Err(DnsNameError::Reserved("localhost".into()))
        );
        // host はホスト専用
        assert_eq!(
            normalize_dns_name("host", false),
            Err(DnsNameError::Reserved("host".into()))
        );
        assert_eq!(normalize_dns_name("host", true), Ok("host".into()));
        // ホストでも一般予約語は不可
        assert_eq!(
            normalize_dns_name("dns", true),
            Err(DnsNameError::Reserved("dns".into()))
        );
    }

    #[test]
    fn is_dns_name_accepts_dotted_relative_names() {
        assert!(is_dns_name("web"));
        assert!(is_dns_name("web.alice"));
        assert!(is_dns_name("nas.member-2"));
        assert!(!is_dns_name("Web.alice"));
        assert!(!is_dns_name("web..alice"));
        assert!(!is_dns_name(".alice"));
        assert!(!is_dns_name("web.alice."));
        assert!(!is_dns_name(""));
    }

    #[test]
    fn custom_dns_name_allows_leading_wildcard_only() {
        assert!(is_custom_dns_name("web"));
        assert!(is_custom_dns_name("web.app"));
        assert!(is_custom_dns_name("*.app"));
        assert!(is_custom_dns_name("*"));
        assert!(!is_custom_dns_name("web.*"), "* は先頭ラベルのみ");
        assert!(!is_custom_dns_name("*app"), "* 単体のラベルのみ");
        assert!(!is_custom_dns_name("Web.app"));
        assert!(!is_custom_dns_name(""));
    }

    #[test]
    fn normalize_custom_dns_name_normalizes_each_label() {
        assert_eq!(normalize_custom_dns_name("My App"), Some("my-app".into()));
        assert_eq!(normalize_custom_dns_name("Web.App"), Some("web.app".into()));
        assert_eq!(normalize_custom_dns_name("*.App"), Some("*.app".into()));
        // 先頭以外の * はワイルドカードにならず、記号としてハイフン化 → 空 → None
        assert_eq!(normalize_custom_dns_name("web.*"), None);
        assert_eq!(normalize_custom_dns_name("web..app"), None);
        assert_eq!(normalize_custom_dns_name("たろう"), None);
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
