// チャットのフロント側ストア(ADR-0016、M3-13b)。
//
// 履歴の正本はデーモン(networks/<net>.chat.jsonl)。UI は status の chatSeq が
// 進んだときだけ差分フェッチして、ここ(モジュールレベルの Map)に貯める。
// コンポーネントの再マウント(タブ切替・画面遷移)で取り直さないための置き場で、
// アプリを再起動すれば取り直す。
//
// 未読管理はこのマシンのローカル(会話ごとの最終既読 seq を localStorage)。

import { ChatMessage, Tunnel, api } from "./ipc";

/** 会話のキー。ネットワーク全体は "network"、1:1 は相手の仮想 IP。 */
export type ConversationKey = string;
export const NETWORK_CONVERSATION: ConversationKey = "network";

interface ChatState {
  messages: ChatMessage[];
  /** フェッチ済みの最終 seq。 */
  lastSeq: number;
}

const stores = new Map<string, ChatState>();

/** メッセージが属する会話(自分の仮想 IP を渡す)。 */
export function conversationOf(
  message: ChatMessage,
  selfAddress: string,
): ConversationKey {
  if (message.scope === "network") return NETWORK_CONVERSATION;
  return message.from === selfAddress ? (message.to ?? "?") : message.from;
}

/** このネットワークのフェッチ済み履歴(古い順)。 */
export function chatMessages(config: string): ChatMessage[] {
  return stores.get(config)?.messages ?? [];
}

/**
 * status の chatSeq に追いつくまで差分フェッチする。戻り値は新しく取れた分
 * (通知用。初回のまとめ読みでは空を返し、起動時に鳴らさない)。
 */
export async function syncChat(
  config: string,
  chatSeq: number,
): Promise<ChatMessage[]> {
  let state = stores.get(config);
  const isFirst = state === undefined;
  if (state === undefined) {
    state = { messages: [], lastSeq: 0 };
    stores.set(config, state);
  }
  // 履歴ファイルが消された等で seq が巻き戻ったら、取り直す
  if (chatSeq < state.lastSeq) {
    state.messages = [];
    state.lastSeq = 0;
  }
  if (chatSeq <= state.lastSeq) return [];

  const fresh: ChatMessage[] = [];
  // 1 応答には上限があるため、追いつくまで繰り返す(進まなければ打ち切り)
  while (state.lastSeq < chatSeq) {
    const page = await api.chatFetch(config, state.lastSeq);
    if (page.messages.length === 0) break;
    for (const message of page.messages) {
      if (message.seq > state.lastSeq) {
        state.messages.push(message);
        state.lastSeq = message.seq;
        fresh.push(message);
      }
    }
  }
  return isFirst ? [] : fresh;
}

/** 自分の送信直後に、フェッチを待たず履歴へ足す(デーモンの応答をそのまま)。 */
export function appendLocal(config: string, message: ChatMessage): void {
  const state = stores.get(config) ?? { messages: [], lastSeq: 0 };
  stores.set(config, state);
  if (message.seq > state.lastSeq) {
    state.messages.push(message);
    state.lastSeq = message.seq;
  }
}

/** 停止したネットワークのストアを消す(次の接続で取り直す)。 */
export function clearChat(config: string): void {
  stores.delete(config);
}

// ---- 未読管理(localStorage) ----

function readKey(config: string): string {
  return `peercove-chat-read:${config}`;
}

function loadRead(config: string): Record<string, number> {
  try {
    const raw = localStorage.getItem(readKey(config));
    return raw ? (JSON.parse(raw) as Record<string, number>) : {};
  } catch {
    return {};
  }
}

/** この会話をここまで読んだ、と記録する。 */
export function markRead(
  config: string,
  conversation: ConversationKey,
  seq: number,
): void {
  const read = loadRead(config);
  if ((read[conversation] ?? 0) >= seq) return;
  read[conversation] = seq;
  try {
    localStorage.setItem(readKey(config), JSON.stringify(read));
  } catch {
    // localStorage が使えなくても未読バッジが残るだけ
  }
}

/** 会話ごとの未読数(受信分のみ。自分の送信は未読にしない)。 */
export function unreadCounts(
  tunnel: Tunnel,
): Map<ConversationKey, number> {
  const read = loadRead(tunnel.config);
  const counts = new Map<ConversationKey, number>();
  for (const message of chatMessages(tunnel.config)) {
    if (message.from === tunnel.address) continue;
    const conversation = conversationOf(message, tunnel.address);
    if (message.seq > (read[conversation] ?? 0)) {
      counts.set(conversation, (counts.get(conversation) ?? 0) + 1);
    }
  }
  return counts;
}

/** チャットタブのバッジ用の合計未読数。 */
export function totalUnread(tunnel: Tunnel): number {
  let total = 0;
  for (const count of unreadCounts(tunnel).values()) total += count;
  return total;
}

// ---- 通知の抑制(いま見ている会話は鳴らさない) ----

let active: { config: string; conversation: ConversationKey } | null = null;

/** ChatPanel が「いまこの会話を表示している」と申告する(閉じたら null)。 */
export function setActiveConversation(
  value: { config: string; conversation: ConversationKey } | null,
): void {
  active = value;
}

/** この通はいま画面に見えているか(通知を出さなくてよいか)。 */
export function isViewing(config: string, conversation: ConversationKey): boolean {
  return (
    document.hasFocus() &&
    active !== null &&
    active.config === config &&
    active.conversation === conversation
  );
}
