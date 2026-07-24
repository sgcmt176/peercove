// メンバーの参加・切断を OS 通知で知らせる(M2-G6)。
//
// デーモンは状態を持つだけで通知は投げない(サービスとして Session 0 で動くと
// デスクトップへ通知できないため)。UI が 2 秒ごとの status ポーリングの差分を
// 見て通知する。UI を閉じてもトレイに常駐しているので通知は出続ける。
//
// 通知そのものは Rust 側の `notify` コマンドが出す。@tauri-apps/plugin-notification
// を frontend から使うと npm 依存が 1 つ増え、許可の問い合わせも要るため。

import { invoke } from "@tauri-apps/api/core";
import { ChatMessage, Group, Member, MemoReminder, Transfer, Tunnel, api } from "./ipc";
import { conversationOf, isMuted, isViewing } from "./chat";
import { loadPrefs } from "./prefs";
import { t } from "./i18n";

export interface MemberEvent {
  kind: "joined" | "left" | "approval_requested";
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
  const wasStatus = new Map(previous.map((m) => [m.publicKey, m.inviteStatus]));
  const events: MemberEvent[] = [];

  for (const member of current) {
    if (!relevant(member)) continue;
    const before = wasOnline.get(member.publicKey);
    if (
      member.inviteStatus === "awaiting_approval" &&
      wasStatus.get(member.publicKey) !== "awaiting_approval"
    ) {
      events.push({ kind: "approval_requested", member });
      continue;
    }
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
  if (event.kind === "approval_requested") {
    return { title: t.notify.approvalTitle, body };
  }
  return event.kind === "joined"
    ? { title: t.notify.joinedTitle, body }
    : { title: t.notify.leftTitle, body };
}

export async function notifyMemberEvents(
  events: MemberEvent[],
  network: string,
): Promise<void> {
  // 通知はアプリ設定でまとめてオフにできる(M3-13e)。バッジは別(chat.ts)
  if (!loadPrefs().notifications) return;
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
  if (!loadPrefs().notifications) return;
  for (const message of fresh) {
    if (message.from === tunnel.address) continue;
    if (message.system) continue; // グループ操作のお知らせは鳴らさない
    const conversation = conversationOf(message, tunnel.address);
    if (isViewing(tunnel.config, conversation)) continue;
    if (isMuted(tunnel.config, conversation)) continue; // ミュート会話は鳴らさない
    const from =
      members.find((m) => m.ip === message.from)?.name ?? message.from;
    // グループ宛はネットワーク名の代わりにグループ名を出す(LINE と同じ)
    const context =
      message.scope === "group"
        ? (tunnel.groups.find((g) => g.id === message.groupId)?.name ??
          t.chat.unknownGroup)
        : tunnel.network;
    // ファイルは本文の代わりにファイル名(M3-13d)
    const text = message.file
      ? t.chat.filePreview(message.file.name)
      : message.text;
    try {
      await invoke("notify", {
        title: t.notify.chatTitle(from, context),
        body: t.notify.chatBody(text, message.scope === "network"),
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
  if (!loadPrefs().notifications) return;
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

const qualityCheckedAt = new Map<string, number>();
const qualityNotifiedWindow = new Map<string, number>();
const qualityRoute = new Map<string, string>();

/** 品質通知。status の高頻度ポーリングから呼ばれるため、内部で 1 分に抑える。 */
export async function notifyQualityEvents(tunnel: Tunnel): Promise<void> {
  const prefs = loadPrefs();
  if (!prefs.notifications || !prefs.qualityAlerts) return;
  const now = Date.now();
  if (now - (qualityCheckedAt.get(tunnel.config) ?? 0) < 60_000) return;
  qualityCheckedAt.set(tunnel.config, now);
  try {
    const report = await api.qualityHistory(tunnel.config, now - 5 * 60_000);
    const byPeer = new Map<string, typeof report.samples>();
    for (const sample of report.samples) {
      const list = byPeer.get(sample.publicKey) ?? [];
      list.push(sample);
      byPeer.set(sample.publicKey, list);
    }
    for (const [key, list] of byPeer) {
      list.sort((a, b) => a.windowStartUnixMs - b.windowStartUnixMs);
      const latest = list.at(-1);
      if (!latest) continue;
      const stateKey = `${tunnel.config}:${key}`;
      const label = latest.name || latest.ip;
      const lastThree = list.filter((sample) => sample.windowSecs >= 60).slice(-3);
      if (
        lastThree.length === 3 &&
        lastThree.every(
          (sample) =>
            sample.availability === "connected" &&
            sample.lossPercent !== null &&
            sample.lossPercent > prefs.qualityLossThreshold,
        ) &&
        qualityNotifiedWindow.get(stateKey) !== latest.windowStartUnixMs
      ) {
        qualityNotifiedWindow.set(stateKey, latest.windowStartUnixMs);
        await invoke("notify", {
          title: "通信品質が低下しています",
          body: `${label} との損失率が ${prefs.qualityLossThreshold}% を3分連続で超えました（${tunnel.network}）。`,
        });
      }
      const previousRoute = qualityRoute.get(stateKey);
      if (previousRoute === "direct" && latest.route === "relay") {
        await invoke("notify", {
          title: "通信経路が切り替わりました",
          body: `${label} との通信が直接経路からホスト経由へ切り替わりました（${tunnel.network}）。`,
        });
      }
      qualityRoute.set(stateKey, latest.route);
    }
  } catch {
    // 品質履歴がまだ無い／旧デーモンでも通常の状態更新を止めない。
  }
}

// ---- 共有メモのコメント・メンション通知(M5 F-5 Stage 3、ADR-0052 決定 4・5) ----
//
// プロトコルに専用イベントは無い(既存の Changed 配信 + comment_count に
// 相乗り — ADR-0052 決定 4)。UI 側は「メモ一覧の comment_count が増えた」
// ことを sharedMemoSeq の変化をきっかけに検知し、増えたメモだけコメントを
// 取り直して、新着コメント(前回のチェック以降に作成されたもの)ごとに
// メンション・自分のメモへのコメントかを判定して通知する。
// 自分のコメントは通知しない(送信直後に自分でも検知してしまうため)。

const commentLastSeq = new Map<string, number>();
const commentBaseline = new Map<
  string,
  Map<string, { count: number; latestAt: number }>
>();

/** 停止したネットワークの追跡状態を消す(次の接続で「初回」に戻す)。 */
export function clearCommentTracking(config: string): void {
  commentLastSeq.delete(config);
  commentBaseline.delete(config);
}

/**
 * 共有メモのコメント・メンション通知。status ポーリングのたびに呼んで
 * よい(sharedMemoSeq が変わっていなければ即返る = 追加のホスト問い合わせ
 * を増やさない)。
 */
export async function notifyCommentEvents(tunnel: Tunnel): Promise<void> {
  if (!loadPrefs().notifications) return;
  if (!tunnel.sharedMemo) return;
  if (commentLastSeq.get(tunnel.config) === tunnel.sharedMemoSeq) return;
  commentLastSeq.set(tunnel.config, tunnel.sharedMemoSeq);

  const self = tunnel.members.find((m) => m.isSelf);
  const myMemberId = self?.memberId ?? null;
  const myDisplayName = (self?.name ?? "").trim();

  let baseline = commentBaseline.get(tunnel.config);
  if (baseline === undefined) {
    baseline = new Map();
    commentBaseline.set(tunnel.config, baseline);
  }

  try {
    const reply = await api.sharedMemoOp(tunnel.config, {
      op: "list",
      query: {},
    });
    if (reply.kind !== "memos") return;
    for (const memo of reply.memos) {
      const count = memo.comment_count ?? 0;
      const previous = baseline.get(memo.id);
      if (previous === undefined) {
        // 初めて見るメモは基準点を作るだけ(起動直後にまとめて鳴らさない)
        baseline.set(memo.id, { count, latestAt: Date.now() });
        continue;
      }
      if (count <= previous.count) {
        baseline.set(memo.id, { count, latestAt: previous.latestAt });
        continue;
      }
      try {
        const commentReply = await api.sharedMemoOp(tunnel.config, {
          op: "comment_list",
          id: memo.id,
        });
        if (commentReply.kind !== "comments") continue;
        const fresh = commentReply.comments.filter(
          (c) => c.created_at_unix_ms > previous.latestAt,
        );
        let latestAt = previous.latestAt;
        for (const comment of fresh) {
          latestAt = Math.max(latestAt, comment.created_at_unix_ms);
          if (comment.author_id === myMemberId) continue; // 自分のコメントは通知しない
          const mentionsMe =
            myDisplayName !== "" && comment.body.includes(`@${myDisplayName}`);
          const ownsMemo = memo.owner_id === (myMemberId ?? "");
          if (!mentionsMe && !ownsMemo) continue;
          const title = mentionsMe
            ? t.notify.mentionTitle(comment.author_name, memo.title || t.memo.untitled)
            : t.notify.commentTitle(comment.author_name, memo.title || t.memo.untitled);
          try {
            await invoke("notify", { title, body: comment.body });
          } catch {
            // 通知の失敗で UI を止めない
          }
        }
        baseline.set(memo.id, { count, latestAt });
      } catch {
        // コメント取得に失敗しても次の周期で再試行する(基準点は更新しない)
      }
    }
  } catch {
    // 一覧取得に失敗しても次の周期で再試行する
  }
}

// ---- メモのリマインダー(端末ローカル、M5 F-5 Stage 5、ADR-0052 決定 6) ----
//
// 個人・共有どちらのリマインダーも個人メモ DB(memos.db)が正本。ネットワーク
// 非依存なので、他の通知(notifyCommentEvents 等)のようにネットワークごとに
// 呼ぶ必要はない — App.tsx の status ポーリング(2 秒)から毎回呼んでよいが、
// ここで 30 秒に抑える(reminder_take_due はサーバー側で fired へ遷移する
// 副作用があるため、呼びすぎても実害は無いが間隔を空けて負荷を抑える)。

const REMINDER_POLL_MS = 30_000;
let reminderCheckedAt = 0;

/** タイトルはローカル通知への表示であり、ログではない(ADR-0049)。 */
async function resolveReminderTitle(reminder: MemoReminder): Promise<string | null> {
  try {
    if (reminder.scope === "shared") {
      const reply = await api.sharedMemoOp(reminder.network ?? "", {
        op: "get",
        id: reminder.memo_id,
      });
      return reply.kind === "memo" ? reply.memo.title || t.memo.untitled : null;
    }
    const reply = await api.memoOp({ op: "get", id: reminder.memo_id });
    return reply.kind === "memo" ? reply.memo.title || t.memo.untitled : null;
  } catch {
    // 削除済み・ホスト未接続などで解決できない場合は黙って捨てる
    // (共有メモの削除はローカルで知れないため — ADR-0052 決定 6)
    return null;
  }
}

/**
 * 発火時刻を過ぎたリマインダーを取り出し、OS 通知を出す。呼ぶと該当分は
 * fired になり(以後の一覧・take_due には出ない)、タイトル解決に失敗しても
 * 再試行はされない(取り出し自体は一度きりのため)。
 */
export async function notifyReminderEvents(): Promise<void> {
  const now = Date.now();
  if (now - reminderCheckedAt < REMINDER_POLL_MS) return;
  reminderCheckedAt = now;
  if (!loadPrefs().notifications) return;
  try {
    const reply = await api.memoOp({ op: "reminder_take_due" });
    if (reply.kind !== "reminders" || reply.reminders.length === 0) return;
    for (const reminder of reply.reminders) {
      const title = await resolveReminderTitle(reminder);
      if (title === null) continue;
      try {
        await invoke("notify", { title: t.notify.reminderTitle(title), body: "" });
      } catch {
        // 通知の失敗で UI を止めない
      }
    }
  } catch {
    // reminder_take_due に失敗しても次の周期で再試行する(旧デーモン等)
  }
}
