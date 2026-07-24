// メンションサジェストの共通部品(ADR-0055 決定 1a・1c)。
//
// もとは共有メモのコメント欄(SharedMemoView.tsx の CommentsPanel)だけに
// あった `@` 入力補助を、チャット(ChatPanel.tsx)でも使えるよう切り出した
// もの。「入力欄の値とカーソル位置から `@` 直後のクエリを取り出す」
// 「候補一覧(先頭に @All、続けて全メンバーを部分一致で絞り込み)」
// 「選んだ候補で `@クエリ` を置き換える」の 3 つを提供する。

import { useMemo } from "react";
import { Member } from "../ipc";
import { t } from "../i18n";
import { MENTION_ALL_TOKEN } from "../mentions";

/** メンションサジェストの候補行(通常メンバー、または先頭の @All)。 */
export interface MentionCandidate {
  key: string;
  /** 表示ラベル(通常はメンバー名、@All は専用ラベル)。 */
  label: string;
  /** insertMention 系へ渡す `@` の後ろの文字列。 */
  insertName: string;
  isAll: boolean;
}

/** テキストエリアの値とカーソル位置から、`@` 直後のクエリを取り出す
 *  (`@` の前は行頭または空白でなければならない)。無ければ null。 */
export function detectMentionQuery(value: string, caret: number): string | null {
  const before = value.slice(0, caret);
  const match = /(?:^|\s)@([^\s@]*)$/.exec(before);
  return match ? match[1] : null;
}

/** メンションサジェストの候補(先頭に @All、続けて全メンバーを部分一致で
 *  絞り込む。6 件制限は撤廃済み — ADR-0055 決定 1c。ポップアップ側でスクロール)。 */
export function useMentionCandidates(
  mentionQuery: string | null,
  members: Member[],
): MentionCandidate[] {
  return useMemo(() => {
    if (mentionQuery === null) return [];
    const query = mentionQuery.trim();
    const candidates: MentionCandidate[] = [];
    const allLabel = t.sharedMemo.mentionAllLabel;
    if (
      query === "" ||
      "all".startsWith(query.toLowerCase()) ||
      allLabel.toLowerCase().includes(query.toLowerCase())
    ) {
      candidates.push({
        key: "__all__",
        label: `@${allLabel}`,
        insertName: MENTION_ALL_TOKEN,
        isAll: true,
      });
    }
    for (const m of members) {
      if (m.isSelf || (m.name ?? "").length === 0) continue;
      if (query !== "" && !(m.name ?? "").includes(query)) continue;
      candidates.push({
        key: m.publicKey,
        label: m.name ?? "",
        insertName: m.name ?? "",
        isAll: false,
      });
    }
    return candidates;
  }, [mentionQuery, members]);
}

/** 入力欄の値のうち、カーソル位置の `@クエリ` を選んだ名前で置き換える。 */
export function applyMention(
  value: string,
  caret: number,
  name: string,
): string {
  const before = value.slice(0, caret);
  const after = value.slice(caret);
  const replaced = before.replace(/(?:^|\s)@([^\s@]*)$/, (whole) =>
    whole.startsWith(" ") ? ` @${name} ` : `@${name} `,
  );
  return replaced + after;
}

/** サジェストのポップアップ(呼び出し側で `position: relative` のコンテナに置く)。 */
export function MentionSuggestList({
  candidates,
  onPick,
}: {
  candidates: MentionCandidate[];
  onPick: (name: string) => void;
}) {
  if (candidates.length === 0) return null;
  return (
    <ul className="memo__mention-suggest">
      {candidates.map((candidate) => (
        <li key={candidate.key}>
          <button
            type="button"
            className={candidate.isAll ? "memo__mention-suggest-all" : undefined}
            onClick={() => onPick(candidate.insertName)}
          >
            {candidate.label}
          </button>
        </li>
      ))}
    </ul>
  );
}
