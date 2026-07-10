// メンバーの参加・切断を OS 通知で知らせる(M2-G6)。
//
// デーモンは状態を持つだけで通知は投げない(サービスとして Session 0 で動くと
// デスクトップへ通知できないため)。UI が 2 秒ごとの status ポーリングの差分を
// 見て通知する。UI を閉じてもトレイに常駐しているので通知は出続ける。
//
// 通知そのものは Rust 側の `notify` コマンドが出す。@tauri-apps/plugin-notification
// を frontend から使うと npm 依存が 1 つ増え、許可の問い合わせも要るため。

import { invoke } from "@tauri-apps/api/core";
import { ChatMessage, Group, Member, Transfer } from "./ipc";
import { conversationOf, isViewing } from "./chat";
import { t } from "./i18n";

export interface MemberEvent {
  kind: "joined" | "left";
  member: Member;
}

/**
 * 前回の台帳と比べて、オンライン状態が変わったメンバーを列挙する。
 *
 * - `selfAddress`: 自分の仮想 IP。自分自身の出入りは通知しない
 *   (メンバーとして参加中、自分が台帳に載る／削除されるのを鳴らさないため)
 */
export function diffMembers(
  previous: Member[] | null,
  current: Member[],
  selfAddress: string | null,
): MemberEvent[] {
  // 初回(baseline なし)は通知しない。起動時に全員分が鳴るのを避ける
  if (previous === null) return [];

  const relevant = (m: Member) => !m.isHost && m.ip !== selfAddress;
  const nowByKey = new Map(current.map((m) => [m.publicKey, m]));
  const wasOnline = new Map(previous.map((m) => [m.publicKey, m.online]));
  const events: MemberEvent[] = [];

  for (const member of current) {
    if (!relevant(member)) continue;
    const before = wasOnline.get(member.publicKey);
    // 新しく台帳に載ったメンバーは、オンラインになるまで通知しない
    if (before === undefined) {
      if (member.online) events.push({ kind: "joined", member });
      continue;
    }
    if (!before && member.online) events.push({ kind: "joined", member });
    if (before && !member.online) events.push({ kind: "left", member });
  }

  // 台帳から消えたメンバー(ホストが削除した)は「切断」として扱う。
  // online→offline を待たずに済むので、削除がすぐ通知される
  for (const member of previous) {
    if (!relevant(member)) continue;
    if (member.online && !nowByKey.has(member.publicKey)) {
      events.push({ kind: "left", member });
    }
  }
  return events;
}

export function describe(
  event: MemberEvent,
  network: string,
): { title: string; body: string } {
  const name = event.member.name ?? event.member.ip;
  const body = t.notify.body(name, event.member.ip, network);
  return event.kind === "joined"
    ? { title: t.notify.joinedTitle, body }
    : { title: t.notify.leftTitle, body };
}

export async function notifyMemberEvents(
  events: MemberEvent[],
  network: string,
): Promise<void> {
  for (const event of events) {
    try {
      await invoke("notify", describe(event, network));
    } catch {
      // 通知の失敗で UI を止めない(通知デーモンが無い環境など)
    }
  }
}

/**
 * 前回の転送一覧と比べて、新しく「受信完了」になった転送を列挙する(M3-9b)。
 * 初回(baseline なし)は通知しない(起動時にまとめて鳴るのを避ける。
 * 見逃した分は受信タブのバッジで分かる)。
 */
export function diffTransfers(
  previous: Transfer[] | null,
  current: Transfer[],
): Transfer[] {
  if (previous === null) return [];
  const wasDone = new Map(previous.map((tr) => [tr.id, tr.done]));
  return current.filter(
    (tr) =>
      tr.direction === "recv" &&
      tr.done &&
      tr.error === null &&
      wasDone.get(tr.id) !== true,
  );
}

/**
 * チャット新着の OS 通知(M3-13b)。自分の送信分は鳴らさない。
 * いま画面でその会話を見ている場合も鳴らさない(LINE と同じ)。
 */
export async function notifyChatEvents(
  fresh: ChatMessage[],
  tunnel: { config: string; address: string; network: string; groups: Group[] },
  members: Member[],
): Promise<void> {
  for (const message of fresh) {
    if (message.from === tunnel.address) continue;
    const conversation = conversationOf(message, tunnel.address);
    if (isViewing(tunnel.config, conversation)) continue;
    const from =
      members.find((m) => m.ip === message.from)?.name ?? message.from;
    // グループ宛はネットワーク名の代わりにグループ名を出す(LINE と同じ)
    const context =
      message.scope === "group"
        ? (tunnel.groups.find((g) => g.id === message.groupId)?.name ??
          t.chat.unknownGroup)
        : tunnel.network;
    try {
      await invoke("notify", {
        title: t.notify.chatTitle(from, context),
        body: t.notify.chatBody(message.text, message.scope === "network"),
      });
    } catch {
      // 通知の失敗で UI を止めない
    }
  }
}

/** 受信完了の OS 通知。送信者名は台帳(members)から引く。 */
export async function notifyFileEvents(
  events: Transfer[],
  members: Member[],
  network: string,
): Promise<void> {
  for (const transfer of events) {
    const from =
      members.find((m) => m.ip === transfer.peer)?.name ?? transfer.peer;
    try {
      await invoke("notify", {
        title: t.notify.fileTitle,
        body: t.notify.fileBody(transfer.name, from, network),
      });
    } catch {
      // 通知の失敗で UI を止めない
    }
  }
}
