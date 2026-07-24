// メモ間リンク `[[タイトル]]`(M5 F-5 Stage 2、ADR-0052 決定 2)。
// 個人メモ・共有メモのプレビュー両方から使う純関数 + 解決フック。
// 解決は同一ストア内(個人→個人、共有→共有)でタイトル完全一致。
// コードブロック内の `[[X]]` まで置換してしまう副作用は許容する
// (初版はシンプル優先)。
import { useEffect, useState } from "react";

const WIKILINK_RE = /\[\[([^[\]]+)\]\]/g;

/** 本文中の `[[タイトル]]` からタイトルの集合(前後空白除去・重複なし)を取り出す。 */
export function extractWikiTitles(body: string): string[] {
  const titles = new Set<string>();
  for (const match of body.matchAll(WIKILINK_RE)) {
    const title = match[1].trim();
    if (title) titles.add(title);
  }
  return [...titles];
}

/**
 * `[[タイトル]]` を Markdown リンク `[タイトル](#memolink=<encoded>)` へ
 * 前処理する。ReactMarkdown の `a` コンポーネントで `#memolink=` を横取り
 * して自ストア内のメモへ遷移させる(クリックハンドラ側の実装)。
 */
export function wikiLinkify(body: string): string {
  return body.replace(WIKILINK_RE, (all, rawTitle: string) => {
    const title = rawTitle.trim();
    if (!title) return all;
    return `[${rawTitle}](#memolink=${encodeURIComponent(title)})`;
  });
}

export const MEMOLINK_PREFIX = "#memolink=";

/** `a` コンポーネントの href が wikilink 由来なら遷移先タイトルを返す。 */
export function wikiLinkTitle(href: string | undefined): string | null {
  if (!href || !href.startsWith(MEMOLINK_PREFIX)) return null;
  try {
    return decodeURIComponent(href.slice(MEMOLINK_PREFIX.length));
  } catch {
    return null;
  }
}

/**
 * 本文中の `[[タイトル]]` を(デバウンスして)解決する。戻り値はタイトル →
 * memo_id のマップ(見つかったものだけ)。プレビュー側でリンク化・未解決の
 * グレー表示に使う。
 */
export function useResolvedWikiLinks(
  body: string,
  resolve: (titles: string[]) => Promise<Record<string, string>>,
): Record<string, string> {
  const [resolved, setResolved] = useState<Record<string, string>>({});

  useEffect(() => {
    const titles = extractWikiTitles(body);
    if (titles.length === 0) {
      setResolved({});
      return;
    }
    let cancelled = false;
    const timer = window.setTimeout(() => {
      void resolve(titles)
        .then((map) => {
          if (!cancelled) setResolved(map);
        })
        .catch(() => {
          if (!cancelled) setResolved({});
        });
    }, 400);
    return () => {
      cancelled = true;
      window.clearTimeout(timer);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [body]);

  return resolved;
}
