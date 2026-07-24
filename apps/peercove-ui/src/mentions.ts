// メンション判定・強調表示の共通ロジック(ADR-0055 決定 1)。
//
// 「@<自分の表示名>」または「@All」(本人の発言は除く — 呼び出し側で判定)を
// 「自分宛」とみなす。メモコメントの通知判定(notify.ts)と、チャット本文の
// 強調表示(ChatPanel.tsx)・サジェスト(MentionSuggest.tsx)で共有する。

/** `@All` の挿入トークン(英語固定)。表示ラベルは i18n 側(mentionAllLabel)。 */
export const MENTION_ALL_TOKEN = "All";

/**
 * 本文に自分宛のメンション(`@<myName>` または `@All`)が含まれるか。
 * 既存のメモコメント通知(`@${myDisplayName}` の部分一致)と同じ判定方式
 * (単純な部分文字列一致。語境界は見ない)に `@All` を足しただけ。
 * `myName` が空文字なら名前メンションは判定しない(`@All` だけ見る)。
 */
export function isMentioned(text: string, myName: string): boolean {
  if (text.includes(`@${MENTION_ALL_TOKEN}`)) return true;
  const trimmed = myName.trim();
  return trimmed !== "" && text.includes(`@${trimmed}`);
}

export type MentionSegment =
  | { mention: false; value: string }
  | { mention: true; value: string; self: boolean };

/**
 * 本文を「地の文」と「メンショントークン」に分割する(表示の強調用)。
 * `memberNames`(自分以外)または自分の名前、`All` に一致するものだけを
 * メンションと認識する(`@example.com` のような無関係な文字列を誤検知
 * しないため)。同じ名前が他の名前の接頭辞になっている場合に備え、長い
 * 名前から先にマッチさせる。
 */
export function splitMentions(
  text: string,
  myName: string,
  memberNames: string[],
): MentionSegment[] {
  const trimmedMy = myName.trim();
  const candidates = new Map<string, boolean>(); // name -> 自分宛か
  candidates.set(MENTION_ALL_TOKEN, true);
  if (trimmedMy !== "") candidates.set(trimmedMy, true);
  for (const name of memberNames) {
    const trimmed = name.trim();
    if (trimmed !== "" && !candidates.has(trimmed)) candidates.set(trimmed, false);
  }
  const names = [...candidates.keys()].sort((a, b) => b.length - a.length);

  interface Range {
    start: number;
    end: number;
    self: boolean;
  }
  const ranges: Range[] = [];
  for (const name of names) {
    const token = `@${name}`;
    let from = 0;
    for (;;) {
      const at = text.indexOf(token, from);
      if (at < 0) break;
      from = at + token.length;
      const before = at > 0 ? text[at - 1] : "";
      const after = text[at + token.length] ?? "";
      // 直前が空白以外(行頭は可)、または直後が英数字なら「別の語の一部」とみなして除外
      if (before !== "" && !/\s/.test(before)) continue;
      if (/[\w]/.test(after)) continue;
      // 既により長い名前とマッチ済みの範囲とは重ねない
      if (ranges.some((r) => at < r.end && at + token.length > r.start)) continue;
      ranges.push({ start: at, end: at + token.length, self: candidates.get(name)! });
    }
  }
  ranges.sort((a, b) => a.start - b.start);

  const parts: MentionSegment[] = [];
  let cursor = 0;
  for (const r of ranges) {
    if (r.start > cursor) parts.push({ mention: false, value: text.slice(cursor, r.start) });
    parts.push({ mention: true, value: text.slice(r.start, r.end), self: r.self });
    cursor = r.end;
  }
  if (cursor < text.length) parts.push({ mention: false, value: text.slice(cursor) });
  return parts;
}
