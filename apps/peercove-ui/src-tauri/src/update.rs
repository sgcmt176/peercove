//! GitHub Releases を使う更新通知(M3-12、ADR-0029)。
//!
//! 自動適用は行わず、公開リポジトリの latest release と現在版を比較して返す。

use std::time::Duration;

use serde::{Deserialize, Serialize};

const LATEST_RELEASE_URL: &str = "https://api.github.com/repos/sgcmt176/peercove/releases/latest";
const API_VERSION: &str = "2022-11-28";

#[derive(Debug, Deserialize)]
struct ReleaseResponse {
    tag_name: String,
    html_url: String,
    name: Option<String>,
    published_at: Option<String>,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInfo {
    pub current_version: String,
    pub latest_version: String,
    pub available: bool,
    pub release_url: String,
    pub release_name: Option<String>,
    pub published_at: Option<String>,
}

pub async fn check() -> anyhow::Result<UpdateInfo> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .redirect(reqwest::redirect::Policy::limited(3))
        .build()?;
    let response = client
        .get(LATEST_RELEASE_URL)
        .header(reqwest::header::ACCEPT, "application/vnd.github+json")
        .header(reqwest::header::USER_AGENT, "PeerCove-Update-Check")
        .header("X-GitHub-Api-Version", API_VERSION)
        .send()
        .await?
        .error_for_status()?;
    let release: ReleaseResponse = serde_json::from_str(&response.text().await?)?;
    from_release(env!("CARGO_PKG_VERSION"), release)
}

fn from_release(current: &str, release: ReleaseResponse) -> anyhow::Result<UpdateInfo> {
    let latest = release.tag_name.trim_start_matches(['v', 'V']);
    let current_parts = parse_stable_version(current)?;
    let latest_parts = parse_stable_version(latest)?;
    Ok(UpdateInfo {
        current_version: current.to_string(),
        latest_version: latest.to_string(),
        available: latest_parts > current_parts,
        release_url: release.html_url,
        release_name: release.name,
        published_at: release.published_at,
    })
}

fn parse_stable_version(value: &str) -> anyhow::Result<(u64, u64, u64)> {
    let core = value
        .split_once(['-', '+'])
        .map(|(core, _)| core)
        .unwrap_or(value);
    let mut parts = core.split('.');
    let major = parse_part(parts.next(), value)?;
    let minor = parse_part(parts.next(), value)?;
    let patch = parse_part(parts.next(), value)?;
    if parts.next().is_some() {
        anyhow::bail!("バージョン形式が不正です: {value}");
    }
    Ok((major, minor, patch))
}

fn parse_part(part: Option<&str>, original: &str) -> anyhow::Result<u64> {
    part.filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow::anyhow!("バージョン形式が不正です: {original}"))?
        .parse()
        .map_err(|_| anyhow::anyhow!("バージョン形式が不正です: {original}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release(tag: &str) -> ReleaseResponse {
        ReleaseResponse {
            tag_name: tag.to_string(),
            html_url: "https://github.com/sgcmt176/peercove/releases/tag/v1.2.3".to_string(),
            name: Some("PeerCove 1.2.3".to_string()),
            published_at: Some("2026-07-14T00:00:00Z".to_string()),
        }
    }

    #[test]
    fn compares_semver_without_lexicographic_mistakes() {
        assert!(from_release("1.9.0", release("v1.10.0")).unwrap().available);
        assert!(!from_release("1.10.0", release("v1.9.9")).unwrap().available);
        assert!(!from_release("1.2.3", release("1.2.3")).unwrap().available);
    }

    #[test]
    fn accepts_build_suffix_and_rejects_invalid_versions() {
        assert_eq!(parse_stable_version("1.2.3+dev").unwrap(), (1, 2, 3));
        assert!(parse_stable_version("1.2").is_err());
        assert!(parse_stable_version("one.2.3").is_err());
    }
}
