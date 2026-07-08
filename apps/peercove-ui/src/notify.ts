// メンバーの参加・切断を OS 通知で知らせる(M2-G6)。
//
// デーモンは状態を持つだけで通知は投げない(サービスとして Session 0 で動くと
// デスクトップへ通知できないため)。UI が 2 秒ごとの status ポーリングの差分を
// 見て通知する。UI を閉じてもトレイに常駐しているので通知は出続ける。
//
// 通知そのものは Rust 側の `notify` コマンドが出す。@tauri-apps/plugin-notification
// を frontend から使うと npm 依存が 1 つ増え、許可の問い合わせも要るため。

import { invoke } from "@tauri-apps/api/core";
import { Member } from "./ipc";

export interface MemberEvent {
  kind: "joined" | "left";
  member: Member;
}

/** 前回の台帳と比べて、オンライン状態が変わったメンバーを列挙する。 */
export function diffMembers(
  previous: Member[] | null,
  current: Member[],
): MemberEvent[] {
  // 初回(baseline なし)は通知しない。起動時に全員分が鳴るのを避ける
  if (previous === null) return [];

  const wasOnline = new Map(previous.map((m) => [m.publicKey, m.online]));
  const events: MemberEvent[] = [];
  for (const member of current) {
    if (member.isHost) continue; // ホスト自身は常にオンライン
    const before = wasOnline.get(member.publicKey);
    // 新しく台帳に載ったメンバーは、オンラインになるまで通知しない
    if (before === undefined) {
      if (member.online) events.push({ kind: "joined", member });
      continue;
    }
    if (!before && member.online) events.push({ kind: "joined", member });
    if (before && !member.online) events.push({ kind: "left", member });
  }
  return events;
}

export function describe(event: MemberEvent): { title: string; body: string } {
  const name = event.member.name ?? event.member.ip;
  return event.kind === "joined"
    ? { title: "メンバーが参加しました", body: `${name}（${event.member.ip}）` }
    : { title: "メンバーが切断しました", body: `${name}（${event.member.ip}）` };
}

export async function notifyMemberEvents(events: MemberEvent[]): Promise<void> {
  for (const event of events) {
    try {
      await invoke("notify", describe(event));
    } catch {
      // 通知の失敗で UI を止めない(通知デーモンが無い環境など)
    }
  }
}
