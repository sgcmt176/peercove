// メンバーの参加・切断を OS 通知で知らせる(M2-G6)。
//
// デーモンは状態を持つだけで通知は投げない(サービスとして Session 0 で動くと
// デスクトップへ通知できないため)。UI が 2 秒ごとの status ポーリングの差分を
// 見て通知する。UI を閉じてもトレイに常駐しているので通知は出続ける。

import {
  isPermissionGranted,
  requestPermission,
  sendNotification,
} from "@tauri-apps/plugin-notification";
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
    ? { title: "メンバーが参加しました", body: `${name}(${event.member.ip})` }
    : { title: "メンバーが切断しました", body: `${name}(${event.member.ip})` };
}

/** 通知の許可状態。初回だけ OS に問い合わせる。 */
let granted: boolean | null = null;

async function ensurePermission(): Promise<boolean> {
  if (granted !== null) return granted;
  try {
    granted =
      (await isPermissionGranted()) ||
      (await requestPermission()) === "granted";
  } catch {
    granted = false; // 通知が使えない環境でも UI は動かす
  }
  return granted;
}

export async function notifyMemberEvents(events: MemberEvent[]): Promise<void> {
  if (events.length === 0) return;
  if (!(await ensurePermission())) return;
  for (const event of events) {
    try {
      sendNotification(describe(event));
    } catch {
      // 通知の失敗で UI を止めない
    }
  }
}
