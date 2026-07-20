// チャットのフロント側ストア(ADR-0016、M3-13b)。
//
// 履歴の正本はデーモン(networks/<net>.chat.jsonl)。UI は status の chatSeq が
// 進んだときだけ差分フェッチして、ここ(モジュールレベルの Map)に貯める。
// コンポーネントの再マウント(タブ切替・画面遷移)で取り直さないための置き場で、
// アプリを再起動すれば取り直す。
//
// 未読管理はこのマシンのローカル(会話ごとの最終既読 seq を localStorage)。

import { ChatMessage, Tunnel, api } from "./ipc";

/**
 * 会話のキー。ネットワーク全体は "network"、グループは "g:<グループID>"、
 * 1:1 は相手の仮想 IP。
 */
export type ConversationKey = string;
export const NETWORK_CONVERSATION: ConversationKey = "network";

const GROUP_PREFIX = "g:";

/** グループの会話キー（M3-13c）。 */
export function groupConversation(groupId: string): ConversationKey {
  return `${GROUP_PREFIX}${groupId}`;
}

/** 会話キーがグループならそのグループ ID、それ以外は null。 */
export function groupIdOf(key: ConversationKey): string | null {
  return key.startsWith(GROUP_PREFIX)
    ? key.slice(GROUP_PREFIX.length)
    : null;
}

interface ChatState {
  messages: ChatMessage[];
  /** フェッチ済みの最終 seq。 */
  lastSeq: number;
  /** 直近に見たチャット消去世代。増えたら手元を捨てて取り直す。 */
  generation: number;
}

const stores = new Map<string, ChatState>();

/** メッセージが属する会話(自分の仮想 IP を渡す)。 */
export function conversationOf(
  message: ChatMessage,
  selfAddress: string,
): ConversationKey {
  if (message.scope === "network") return NETWORK_CONVERSATION;
  if (message.scope === "group") return groupConversation(message.groupId ?? "?");
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
  chatGeneration = 0,
): Promise<ChatMessage[]> {
  let state = stores.get(config);
  const isFirst = state === undefined;
  if (state === undefined) {
    state = { messages: [], lastSeq: 0, generation: chatGeneration };
    stores.set(config, state);
  }
  // 履歴が消された(メンバー再追加での 1:1 クリア等)= 消去世代が変わった、
  // または seq が巻き戻ったら、手元を捨てて取り直す
  if (chatGeneration !== state.generation || chatSeq < state.lastSeq) {
    state.messages = [];
    state.lastSeq = 0;
    state.generation = chatGeneration;
  }
  if (chatSeq <= state.lastSeq) {
    // 新着なしでも末尾を取り直す: 送信キュー(E-E 3)の failed フラグは
    // 揮発で、フェッチ済みの通のまま変化するため
    await refreshTail(config, state);
    return [];
  }

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

/** フェッチ済みの末尾(直近 30 通)を取り直して差し替える。 */
async function refreshTail(config: string, state: ChatState): Promise<void> {
  if (state.messages.length === 0) return;
  const from = Math.max(0, state.lastSeq - 30);
  const page = await api.chatFetch(config, from);
  if (page.messages.length === 0) return;
  const bySeq = new Map(page.messages.map((m) => [m.seq, m]));
  state.messages = state.messages.map((m) => bySeq.get(m.seq) ?? m);
}

/** 自分の送信直後に、フェッチを待たず履歴へ足す(デーモンの応答をそのまま)。 */
export function appendLocal(config: string, message: ChatMessage): void {
  const state = stores.get(config) ?? {
    messages: [],
    lastSeq: 0,
    generation: 0,
  };
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

// ---- ピン留め(localStorage。このマシンだけの表示設定) ----
//
// **順序付き**リストで保存する(配列の並びがそのままピン内の表示順)。
// 新しくピン留めした会話は末尾(下)に付き、送受信では並びが動かない。
// 並べ替えは movePin(上へ / 下へ)で行う。

function pinKey(config: string): string {
  return `peercove-chat-pins:${config}`;
}

/** ピン留め中の会話キー(表示順)。 */
export function loadPins(config: string): ConversationKey[] {
  try {
    const raw = localStorage.getItem(pinKey(config));
    const list = raw ? (JSON.parse(raw) as ConversationKey[]) : [];
    return Array.isArray(list) ? list : [];
  } catch {
    return [];
  }
}

function savePins(config: string, pins: ConversationKey[]): void {
  try {
    localStorage.setItem(pinKey(config), JSON.stringify(pins));
  } catch {
    // 保存できなくても並びが保たれないだけ
  }
}

/** ピン留めの付け外し(付けるときは末尾へ)。 */
export function togglePin(config: string, conversation: ConversationKey): void {
  const pins = loadPins(config);
  const at = pins.indexOf(conversation);
  if (at >= 0) {
    pins.splice(at, 1);
  } else {
    pins.push(conversation);
  }
  savePins(config, pins);
}

/** ピン内で 1 つ上(dir=-1)/下(dir=1)へ動かす。端なら何もしない。 */
export function movePin(
  config: string,
  conversation: ConversationKey,
  dir: -1 | 1,
): void {
  const pins = loadPins(config);
  const at = pins.indexOf(conversation);
  const to = at + dir;
  if (at < 0 || to < 0 || to >= pins.length) return;
  [pins[at], pins[to]] = [pins[to], pins[at]];
  savePins(config, pins);
}

// ---- 会話単位のミュート(localStorage) ----

function muteKey(config: string): string {
  return `peercove-chat-mutes:${config}`;
}

/** ミュート中の会話キーの集合(OS 通知を出さない)。 */
export function loadMutes(config: string): Set<ConversationKey> {
  try {
    const raw = localStorage.getItem(muteKey(config));
    return new Set(raw ? (JSON.parse(raw) as ConversationKey[]) : []);
  } catch {
    return new Set();
  }
}

/** ミュートの付け外し。 */
export function toggleMute(config: string, conversation: ConversationKey): void {
  const mutes = loadMutes(config);
  if (mutes.has(conversation)) {
    mutes.delete(conversation);
  } else {
    mutes.add(conversation);
  }
  try {
    localStorage.setItem(muteKey(config), JSON.stringify([...mutes]));
  } catch {
    // 保存できなくてもミュートが効かないだけ
  }
}

/** この会話はミュートされているか(通知抑止の判定用)。 */
export function isMuted(config: string, conversation: ConversationKey): boolean {
  return loadMutes(config).has(conversation);
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
    if (message.system) continue; // お知らせは未読に数えない
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
