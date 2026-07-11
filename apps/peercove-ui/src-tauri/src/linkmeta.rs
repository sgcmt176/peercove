//! チャットのリンクプレビュー(M3-13e、ADR-0017)の HTML メタデータ抽出。
//!
//! 必要なのは OGP の meta タグ数個と `<title>` だけなので、HTML パーサの
//! crate は入れず最小限の手書きパーサで済ませる(壊れた HTML でも panic
//! せず、取れなかった項目が None になるだけ)。

/// ページから取れたメタデータ。`image` は URL のまま(取得は呼び出し元)。
#[derive(Debug, Default, PartialEq)]
pub struct PageMeta {
    pub title: Option<String>,
    pub description: Option<String>,
    pub site_name: Option<String>,
    pub image: Option<String>,
}

/// HTML から OGP(og:title 等)と `<title>` / `<meta name="description">` を
/// 取り出す。OGP を優先し、無い項目だけフォールバックで埋める。
pub fn extract(html: &str) -> PageMeta {
    let mut meta = PageMeta::default();
    let mut fallback_title: Option<String> = None;
    let mut fallback_description: Option<String> = None;

    // ASCII 小文字化ならバイト位置が元の HTML とずれない(タグ名の検索用)
    let lower = html.to_ascii_lowercase();
    let mut pos = 0;
    while let Some(found) = lower[pos..].find("<meta") {
        let start = pos + found;
        let Some(end_rel) = lower[start..].find('>') else {
            break;
        };
        let end = start + end_rel;
        let pairs = attributes(&html[start + "<meta".len()..end]);
        let key = pairs
            .iter()
            .find(|(name, _)| name == "property" || name == "name")
            .map(|(_, value)| value.to_ascii_lowercase());
        let content = pairs
            .iter()
            .find(|(name, _)| name == "content")
            .map(|(_, value)| decode_entities(value.trim()));
        if let (Some(key), Some(content)) = (key, content) {
            if !content.is_empty() {
                match key.as_str() {
                    "og:title" => meta.title = Some(content),
                    "og:description" => meta.description = Some(content),
                    "og:site_name" => meta.site_name = Some(content),
                    "og:image" | "og:image:url" => meta.image = Some(content),
                    "description" => fallback_description = Some(content),
                    _ => {}
                }
            }
        }
        pos = end + 1;
    }

    // <title>…</title>(og:title が無いページ向け)
    if let Some(open) = lower.find("<title") {
        if let Some(close_rel) = lower[open..].find('>') {
            let text_start = open + close_rel + 1;
            if let Some(end_rel) = lower[text_start..].find("</title") {
                let raw = html[text_start..text_start + end_rel].trim();
                if !raw.is_empty() {
                    fallback_title = Some(decode_entities(raw));
                }
            }
        }
    }

    meta.title = meta.title.or(fallback_title);
    meta.description = meta.description.or(fallback_description);
    meta
}

/// タグの中身(`<meta` と `>` の間)を属性の (名前, 値) に分解する。
/// 名前は ASCII 小文字化する。値の引用符は `"` / `'` / なしに対応。
fn attributes(tag: &str) -> Vec<(String, String)> {
    let bytes = tag.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        let name_start = i;
        while i < bytes.len()
            && !bytes[i].is_ascii_whitespace()
            && !matches!(bytes[i], b'=' | b'/' | b'>')
        {
            i += 1;
        }
        if i == name_start {
            i += 1; // '/' などの区切りだけ。読み飛ばす
            continue;
        }
        let name = tag[name_start..i].to_ascii_lowercase();
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i < bytes.len() && bytes[i] == b'=' {
            i += 1;
            while i < bytes.len() && bytes[i].is_ascii_whitespace() {
                i += 1;
            }
            if i < bytes.len() && (bytes[i] == b'"' || bytes[i] == b'\'') {
                let quote = bytes[i];
                i += 1;
                let value_start = i;
                while i < bytes.len() && bytes[i] != quote {
                    i += 1;
                }
                out.push((name, tag[value_start..i].to_string()));
                i += 1;
            } else {
                let value_start = i;
                while i < bytes.len() && !bytes[i].is_ascii_whitespace() {
                    i += 1;
                }
                out.push((name, tag[value_start..i].to_string()));
            }
        } else {
            out.push((name, String::new()));
        }
    }
    out
}

/// よく出る HTML エンティティだけ戻す(&amp; &lt; &gt; &quot; &#39; と数値参照)。
fn decode_entities(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(pos) = rest.find('&') {
        out.push_str(&rest[..pos]);
        rest = &rest[pos..];
        // エンティティ名は短い(長すぎる ; はただの文字列とみなす)
        let end = match rest.find(';') {
            Some(end) if end <= 12 => end,
            _ => {
                out.push('&');
                rest = &rest[1..];
                continue;
            }
        };
        let entity = &rest[1..end];
        let decoded = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            "nbsp" => Some(' '),
            _ => entity
                .strip_prefix('#')
                .and_then(|num| {
                    if let Some(hex) = num.strip_prefix('x').or(num.strip_prefix('X')) {
                        u32::from_str_radix(hex, 16).ok()
                    } else {
                        num.parse::<u32>().ok()
                    }
                })
                .and_then(char::from_u32),
        };
        match decoded {
            Some(c) => {
                out.push(c);
                rest = &rest[end + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_ogp_fields() {
        let html = r#"<html><head>
            <meta property="og:title" content="記事のタイトル" />
            <meta property="og:description" content="説明&amp;文" />
            <meta property="og:site_name" content='サイト名'>
            <meta property="og:image" content="https://example.com/a.png">
            <title>タイトルタグ</title>
        </head></html>"#;
        let meta = extract(html);
        assert_eq!(meta.title.as_deref(), Some("記事のタイトル"));
        assert_eq!(meta.description.as_deref(), Some("説明&文"));
        assert_eq!(meta.site_name.as_deref(), Some("サイト名"));
        assert_eq!(meta.image.as_deref(), Some("https://example.com/a.png"));
    }

    #[test]
    fn falls_back_to_title_and_description() {
        let html = r#"<head><TITLE>ページ &#x2764; 名</TITLE>
            <meta name="description" content="通常の説明"></head>"#;
        let meta = extract(html);
        assert_eq!(meta.title.as_deref(), Some("ページ ❤ 名"));
        assert_eq!(meta.description.as_deref(), Some("通常の説明"));
        assert_eq!(meta.image, None);
    }

    #[test]
    fn ogp_wins_over_fallbacks() {
        let html = r#"<title>素のタイトル</title>
            <meta name="description" content="素の説明">
            <meta property="og:title" content="OGP タイトル">
            <meta property="og:description" content="OGP 説明">"#;
        let meta = extract(html);
        assert_eq!(meta.title.as_deref(), Some("OGP タイトル"));
        assert_eq!(meta.description.as_deref(), Some("OGP 説明"));
    }

    #[test]
    fn survives_broken_html() {
        assert_eq!(extract(""), PageMeta::default());
        assert_eq!(extract("<meta property="), PageMeta::default());
        assert_eq!(extract("<meta content='x' property='og:title'").title, None);
        // 空 content は無視、閉じていないエンティティもそのまま
        let meta = extract("<meta property='og:title' content=''><title>a &b</title>");
        assert_eq!(meta.title.as_deref(), Some("a &b"));
    }
}
