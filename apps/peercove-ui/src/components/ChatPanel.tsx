import { ReactNode, useEffect, useMemo, useRef, useState } from "react";
import { convertFileSrc } from "@tauri-apps/api/core";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import {
  ChatContext,
  ChatMessage,
  Group,
  LinkPreview,
  Member,
  TextPreview,
  Transfer,
  Tunnel,
  api,
  baseName,
  errorMessage,
  formatBytes,
} from "../ipc";
import { loadPrefs } from "../prefs";
import {
  SharedRefKind,
  SharedRefTokenValue,
  sharedRefIcon,
  sharedRefNoun,
  splitSharedRefs,
  useSharedRefResolve,
} from "../sharedRefs";
import {
  ConversationKey,
  NETWORK_CONVERSATION,
  appendLocal,
  chatMessages,
  conversationOf,
  groupConversation,
  groupIdOf,
  loadMutes,
  loadPins,
  markRead,
  movePin,
  setActiveConversation,
  toggleMute,
  togglePin,
  unreadCounts,
  unreadMentionConversations,
} from "../chat";
import { Avatar } from "./Avatar";
import { ConfirmModal, Modal } from "./Modal";
import {
  applyMention,
  detectMentionQuery,
  MentionSuggestList,
  useMentionCandidates,
} from "./MentionSuggest";
import { splitMentions } from "../mentions";
import { t } from "../i18n";

/** ArrayBuffer を base64 文字列へ(貼り付け画像を Rust へ渡す用)。
 *  大きい画像でもスタックを溢れさせないよう分割して変換する。 */
function bytesToBase64(buf: ArrayBuffer): string {
  const bytes = new Uint8Array(buf);
  let binary = "";
  const chunk = 0x8000;
  for (let i = 0; i < bytes.length; i += chunk) {
    binary += String.fromCharCode(...bytes.subarray(i, i + chunk));
  }
  return btoa(binary);
}

/** 会話リストの 1 行(全体・グループ・メンバー 1:1)。 */
interface ConversationItem {
  key: ConversationKey;
  name: string;
  online: boolean;
  member: Member | null;
  group: Group | null;
  /** 履歴にだけ残っている会話(退出済みメンバー・退出済みグループ)。 */
  left: boolean;
}

/**
 * チャットタブ(M3-13b/c/d、ADR-0016)。LINE 風の 2 ペイン:
 * 左 = 会話リスト(全体 + グループ + メンバー 1:1、未読バッジ、グループ作成)、
 * 右 = 吹き出しの会話(テキスト + ファイルバブル)。ファイルは 📎 か
 * ドラッグ&ドロップで、いま開いている会話の宛先へ送る。
 * 履歴は chat.ts のストア(App の 2 秒ポーリングが差分フェッチ済み)を読む。
 */
export function ChatPanel({
  tunnel,
  initialConversation,
  onOpenRef,
}: {
  tunnel: Tunnel;
  /** メンバー行の 💬 から開くとき、その相手(仮想 IP)の 1:1 会話を選ぶ。 */
  initialConversation?: { peer: string } | null;
  /** 本文中の `@memo:id` / `@schedule:id` カード(ADR-0052 決定 1、ADR-0053)を
   * クリックしたときの遷移先。 */
  onOpenRef: (kind: SharedRefKind, id: string) => void;
}) {
  const [conversation, setConversation] = useState<ConversationKey>(
    initialConversation?.peer ?? NETWORK_CONVERSATION,
  );

  // 💬 で開かれた相手の会話へ切り替える(同じ相手を続けてクリックしても
  // 参照が変わるので毎回ここを通る)
  useEffect(() => {
    if (initialConversation) setConversation(initialConversation.peer);
  }, [initialConversation]);
  const [draft, setDraft] = useState("");
  const [sending, setSending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  /** メンションサジェスト(ADR-0055 決定 1a)。SharedMemoView と共通の部品。 */
  const [mentionQuery, setMentionQuery] = useState<string | null>(null);
  const mentionCandidates = useMentionCandidates(mentionQuery, tunnel.members);
  /** グループ作成ダイアログ(M3-13c)。 */
  const [creating, setCreating] = useState(false);
  /** グループ管理ダイアログの対象。 */
  const [managing, setManaging] = useState<Group | null>(null);
  /** ドロップされたファイル(送信確認待ち — M3-13d)。 */
  const [dropPaths, setDropPaths] = useState<string[] | null>(null);
  const [dragOver, setDragOver] = useState(false);
  /** 拡大表示(クリックで開く)。画像(検証 FB)とテキスト(M3-13e)。 */
  const [viewer, setViewer] = useState<
    | { kind: "image"; src: string; name: string }
    | { kind: "text"; name: string; preview: TextPreview }
    | null
  >(null);
  // 送信直後・既読直後にポーリングを待たず再描画するためのカウンタ
  const [, setBump] = useState(0);
  const rerender = () => setBump((n) => n + 1);

  const selfIp = tunnel.address;
  const messages = chatMessages(tunnel.config);
  const unread = unreadCounts(tunnel);
  const memberByIp = new Map(tunnel.members.map((m) => [m.ip, m]));
  // 未読メンションバッジ(会話一覧、ADR-0055 決定 1、M6 H-7b)
  const myName = memberByIp.get(selfIp)?.name ?? "";
  const unreadMentions = unreadMentionConversations(tunnel, myName);
  const groupById = new Map(tunnel.groups.map((g) => [g.id, g]));

  // ピン留めは順序付きリスト(手動順・新規は末尾)。ミュートは集合。
  const pinOrder = loadPins(tunnel.config);
  const pinIndex = new Map(pinOrder.map((key, i) => [key, i]));
  const mutes = loadMutes(tunnel.config);

  // チャット横断検索(本文・ファイル名)。空なら通常の会話リスト。
  const [search, setSearch] = useState("");
  // 会話行の右クリックメニュー(ピン/ミュート/並べ替え)。
  const [menu, setMenu] = useState<{
    key: ConversationKey;
    x: number;
    y: number;
  } | null>(null);

  // 会話リストの候補: 全体 → 参加中のグループ → メンバー(台帳順)
  // → 履歴にだけ残っている会話(退出済みグループ・居なくなった相手)。
  // 表示順はこの後「ピン留め → 直近のやり取り順」に並べ替える
  const conversations = useMemo(() => {
    const items: ConversationItem[] = [
      {
        key: NETWORK_CONVERSATION,
        name: t.chat.all,
        online: true,
        member: null,
        group: null,
        left: false,
      },
    ];
    for (const group of tunnel.groups) {
      if (!group.members.includes(selfIp)) continue;
      items.push({
        key: groupConversation(group.id),
        name: group.name,
        online: true,
        member: null,
        group,
        left: false,
      });
    }
    for (const member of tunnel.members) {
      if (member.isSelf) continue;
      items.push({
        key: member.ip,
        name: member.name ?? member.ip,
        online: member.online,
        member,
        group: null,
        left: false,
      });
    }
    const known = new Set(items.map((item) => item.key));
    for (const message of messages) {
      const key = conversationOf(message, selfIp);
      if (known.has(key)) continue;
      known.add(key);
      const groupId = groupIdOf(key);
      const group = groupId ? (groupById.get(groupId) ?? null) : null;
      items.push({
        key,
        // グループ情報がまだ届いていない間は「同期中」の表示名にする
        name: groupId ? (group?.name ?? t.chat.unknownGroup) : key,
        online: false,
        member: null,
        group,
        left: true,
      });
    }
    return items;
    // groupById は tunnel.groups から導出されるので依存はそちらで足りる
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [tunnel.members, tunnel.groups, messages, selfIp]);

  // 表示順: ピン留めが常に上(手動順のまま。送受信で入れ替えない)、
  // 続いて直近のやり取り(最新メッセージ)順。メッセージが無い会話は
  // 元の並び(全体 → グループ → メンバー)のまま後ろへ
  const orderedConversations = useMemo(() => {
    const lastSeq = new Map<ConversationKey, number>();
    for (const message of messages) {
      lastSeq.set(conversationOf(message, selfIp), message.seq);
    }
    return conversations
      .map((item, index) => ({ item, index }))
      .sort((a, b) => {
        const aPin = pinIndex.get(a.item.key);
        const bPin = pinIndex.get(b.item.key);
        // ピン留めは常に上。ピン内は手動の並び順(pinIndex 昇順)で固定
        if (aPin !== undefined || bPin !== undefined) {
          if (aPin === undefined) return 1;
          if (bPin === undefined) return -1;
          return aPin - bPin;
        }
        const seqDiff =
          (lastSeq.get(b.item.key) ?? 0) - (lastSeq.get(a.item.key) ?? 0);
        if (seqDiff !== 0) return seqDiff;
        return a.index - b.index;
      })
      .map(({ item }) => item);
    // pinIndex/mutes は localStorage 由来(操作後は rerender で再計算)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [conversations, messages, selfIp, pinOrder]);

  // 検索ヒット(全会話横断、新しい順、最大 50 件)。会話名は conversations から引く
  const searchHits = useMemo(() => {
    const q = search.trim().toLowerCase();
    if (!q) return [];
    const nameOf = (key: ConversationKey) =>
      conversations.find((c) => c.key === key)?.name ?? key;
    return messages
      .filter(
        (m) =>
          !m.system &&
          (m.text.toLowerCase().includes(q) ||
            (m.file?.name.toLowerCase().includes(q) ?? false)),
      )
      .slice()
      .sort((a, b) => b.seq - a.seq)
      .slice(0, 50)
      .map((m) => ({
        message: m,
        key: conversationOf(m, selfIp),
        name: nameOf(conversationOf(m, selfIp)),
      }));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [search, messages, selfIp, conversations]);

  const current = messages.filter(
    (message) => conversationOf(message, selfIp) === conversation,
  );
  const selected = conversations.find((item) => item.key === conversation);
  /** 相手がホストの ACL で遮断されている(M3-10)。送っても届かない。 */
  const selectedBlocked = selected?.member?.blocked ?? false;
  // テキストは送信キュー(E-E 3)がオフライン宛も面倒を見るので、
  // 遮断・退出以外は送信可。ファイルは即時転送のみなのでオンライン必須
  const canSend =
    conversation === NETWORK_CONVERSATION ||
    (selected !== undefined &&
      !selected.left &&
      (selected.group !== null || !selectedBlocked));
  const canSendFiles =
    canSend &&
    (conversation === NETWORK_CONVERSATION ||
      selected?.group != null ||
      selected?.online === true);
  const sendingSeqs = new Set(tunnel.chatSending ?? []);

  // いま見ている会話を申告する(新着通知を鳴らさないため — notify.ts)
  useEffect(() => {
    setActiveConversation({ config: tunnel.config, conversation });
    return () => setActiveConversation(null);
  }, [tunnel.config, conversation]);

  // 表示中の会話は既読にする(未読バッジの解消)
  const lastSeq = current.at(-1)?.seq ?? 0;
  useEffect(() => {
    if (lastSeq > 0) {
      markRead(tunnel.config, conversation, lastSeq);
      rerender();
    }
  }, [tunnel.config, conversation, lastSeq]);

  // メッセージ入力欄の自動高さ調整(複数行でも全体が見えるように — M3-16)。
  // 内容に合わせて伸ばし、画面の 4 割で頭打ち(それ以上は入力欄内スクロール)
  const inputRef = useRef<HTMLTextAreaElement>(null);
  useEffect(() => {
    const el = inputRef.current;
    if (!el) return;
    el.style.height = "auto";
    el.style.height = `${Math.min(el.scrollHeight, window.innerHeight * 0.4)}px`;
  }, [draft]);

  /** メンションサジェスト(ADR-0055 決定 1a)。SharedMemoView のコメント欄と
   *  同じ挙動(MentionSuggest.tsx 共通部品)。 */
  const handleDraftChange = (target: HTMLTextAreaElement) => {
    setDraft(target.value);
    const caret = target.selectionStart ?? target.value.length;
    setMentionQuery(detectMentionQuery(target.value, caret));
  };
  const insertMention = (name: string) => {
    const caret = inputRef.current?.selectionStart ?? draft.length;
    setDraft(applyMention(draft, caret, name));
    setMentionQuery(null);
    inputRef.current?.focus();
  };

  // 自動スクロール: 最下部付近にいるときだけ追従する(遡り閲覧を邪魔しない)
  const listRef = useRef<HTMLDivElement>(null);
  const stickBottom = useRef(true);
  useEffect(() => {
    stickBottom.current = true;
    setMentionQuery(null);
  }, [conversation]);
  useEffect(() => {
    const el = listRef.current;
    if (el && stickBottom.current) el.scrollTop = el.scrollHeight;
  }, [conversation, current.length]);

  // ネイティブのファイルドロップ(M3-13d)。webview 全体のイベントだが、
  // このリスナーはチャットタブが開いている間だけ生きている
  useEffect(() => {
    let alive = true;
    let unlisten: (() => void) | undefined;
    void getCurrentWebview()
      .onDragDropEvent((event) => {
        if (event.payload.type === "enter" || event.payload.type === "over") {
          setDragOver(true);
        } else if (event.payload.type === "drop") {
          setDragOver(false);
          if (event.payload.paths.length > 0) setDropPaths(event.payload.paths);
        } else {
          setDragOver(false);
        }
      })
      .then((fn) => {
        if (alive) {
          unlisten = fn;
        } else {
          fn();
        }
      });
    return () => {
      alive = false;
      unlisten?.();
    };
  }, []);

  /** いま開いている会話の宛先(ファイル送信のチャット文脈)。 */
  const destination = (): { peer: string | null; chat: ChatContext } => {
    const groupId = groupIdOf(conversation);
    if (conversation === NETWORK_CONVERSATION) {
      return { peer: null, chat: { scope: "network" } };
    }
    if (groupId !== null) {
      return { peer: null, chat: { scope: "group", groupId } };
    }
    return { peer: conversation, chat: { scope: "direct" } };
  };

  const send = async () => {
    const text = draft.trim();
    if (!text || sending || !canSend) return;
    setSending(true);
    setError(null);
    try {
      const groupId = groupIdOf(conversation);
      const peer =
        conversation === NETWORK_CONVERSATION || groupId !== null
          ? null
          : conversation;
      const message = await api.chatSend(tunnel.config, peer, groupId, text);
      appendLocal(tunnel.config, message);
      markRead(tunnel.config, conversation, message.seq);
      setDraft("");
      setMentionQuery(null);
      stickBottom.current = true;
      rerender();
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setSending(false);
    }
  };

  /** ファイルを送る(📎 / ドロップ共通)。バブルは次のポーリングで出る。 */
  const sendFiles = async (paths: string[]) => {
    if (!canSendFiles || paths.length === 0) return;
    setError(null);
    try {
      const { peer, chat } = destination();
      for (const path of paths) {
        await api.sendFile(tunnel.config, peer, path, chat);
      }
      stickBottom.current = true;
      setNotice(t.chat.fileStarted(paths.length));
      setTimeout(() => setNotice(null), 8000);
    } catch (e) {
      setError(errorMessage(e));
    }
  };

  const attach = async () => {
    if (!canSend) return;
    try {
      const path = await api.pickFile();
      if (path) await sendFiles([path]);
    } catch (e) {
      setError(errorMessage(e));
    }
  };

  /** クリップボードのファイルを貼り付けて送る(スクリーンショット等の画像に
   *  加え、エクスプローラーで Ctrl+C したファイルも対象 — 2026-07-20 検証 FB)。
   *  一時ファイル化してから、ドロップと同じ送信確認モーダルへ渡す。 */
  const handlePaste = async (event: React.ClipboardEvent) => {
    if (!canSendFiles) return;
    const files = Array.from(event.clipboardData?.files ?? []);
    if (files.length === 0) return; // ファイルでなければ通常のテキスト貼り付け
    event.preventDefault();
    const paths: string[] = [];
    for (const file of files) {
      try {
        const b64 = bytesToBase64(await file.arrayBuffer());
        // コピー元がファイルなら元の名前を保つ。スクリーンショット等の
        // 無名画像には拡張子付きの名前を振る
        const ext = (file.type.split("/")[1] || "png").replace("jpeg", "jpg");
        const name = file.name || `貼り付け-${Date.now()}.${ext}`;
        paths.push(await api.savePastedFile(name, b64));
      } catch (e) {
        setError(errorMessage(e));
      }
    }
    if (paths.length > 0) setDropPaths(paths); // 送信確認モーダルを開く
  };

  /** 受信ファイルの保存(バブルの「保存」— 実体は受信ボックス)。 */
  const saveFile = async (name: string) => {
    setError(null);
    try {
      const saved = await api.saveInboxFile(tunnel.config, name);
      if (saved) {
        setNotice(t.inbox.savedTo(saved));
        setTimeout(() => setNotice(null), 8000);
      }
    } catch (e) {
      setError(errorMessage(e));
    }
  };

  const showNames =
    conversation === NETWORK_CONVERSATION || groupIdOf(conversation) !== null;

  // 右クリックメニューはどこかをクリック/Esc で閉じる
  useEffect(() => {
    if (!menu) return;
    const close = () => setMenu(null);
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") setMenu(null);
    };
    window.addEventListener("click", close);
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("click", close);
      window.removeEventListener("keydown", onKey);
    };
  }, [menu]);

  return (
    <div className="chat">
      <div className="chat__list">
        <input
          type="search"
          className="chat__search"
          placeholder={t.chat.searchPlaceholder}
          value={search}
          onChange={(event) => setSearch(event.target.value)}
        />
        {search.trim() !== "" ? (
          searchHits.length === 0 ? (
            <p className="muted small chat__search-empty">
              {t.chat.searchEmpty}
            </p>
          ) : (
            searchHits.map(({ message, key, name }) => (
              <button
                key={message.seq}
                type="button"
                className="chat__conv chat__hit"
                onClick={() => {
                  setConversation(key);
                  setSearch("");
                }}
              >
                <span className="chat__conv-text">
                  <span className="chat__conv-name ellipsis">
                    <span className="muted small">{name}</span>
                  </span>
                  <span className="chat__conv-preview ellipsis">
                    {message.file
                      ? t.chat.filePreview(message.file.name)
                      : message.text}
                  </span>
                </span>
              </button>
            ))
          )
        ) : (
          <>
            <button
              type="button"
              className="chat__new-group"
              onClick={() => setCreating(true)}
            >
              ＋ {t.chat.groupCreate}
            </button>
            {orderedConversations.map((item) => {
              const count = unread.get(item.key) ?? 0;
              const last = lastMessageOf(messages, selfIp, item.key);
              const pinned = pinIndex.has(item.key);
              const muted = mutes.has(item.key);
              return (
                <button
                  key={item.key}
                  type="button"
                  className={
                    item.key === conversation
                      ? "chat__conv chat__conv--active"
                      : "chat__conv"
                  }
                  onClick={() => setConversation(item.key)}
                  onContextMenu={(event) => {
                    event.preventDefault();
                    setMenu({ key: item.key, x: event.clientX, y: event.clientY });
                  }}
                >
                  {item.member ? (
                <Avatar
                  publicKey={item.member.publicKey}
                  name={item.member.name}
                  online={item.member.online}
                  onlineLabel={
                    item.online
                      ? t.tunnel.member.online
                      : t.tunnel.member.offline
                  }
                />
              ) : item.group !== null || groupIdOf(item.key) !== null ? (
                <span className="avatar chat__group-icon" aria-hidden>
                  👥
                </span>
              ) : (
                <span className="avatar chat__all-icon" aria-hidden>
                  {item.key === NETWORK_CONVERSATION ? "@" : "?"}
                </span>
              )}
              <span className="chat__conv-text">
                <span className="chat__conv-name ellipsis">
                  {pinned && (
                    <span className="chat__pin-mark" aria-hidden>
                      📌
                    </span>
                  )}
                  {muted && (
                    <span className="chat__pin-mark" aria-hidden>
                      🔕
                    </span>
                  )}
                  {item.name}
                </span>
                <span className="chat__conv-preview ellipsis">
                  {last ? previewOf(last, selfIp) : t.chat.noMessages}
                </span>
              </span>
              {count > 0 && <span className="chat__badge">{count}</span>}
              {unreadMentions.has(item.key) && (
                <span
                  className="chat__badge chat__badge--mention"
                  title={t.chat.mentionBadgeTitle}
                >
                  @
                </span>
              )}
              <span
                role="button"
                tabIndex={0}
                className={pinned ? "chat__pin chat__pin--on" : "chat__pin"}
                title={pinned ? t.chat.unpin : t.chat.pin}
                onClick={(event) => {
                  event.stopPropagation();
                  togglePin(tunnel.config, item.key);
                  rerender();
                }}
                onKeyDown={(event) => {
                  if (event.key === "Enter" || event.key === " ") {
                    event.stopPropagation();
                    togglePin(tunnel.config, item.key);
                    rerender();
                  }
                }}
              >
                📌
              </span>
                </button>
              );
            })}
          </>
        )}
      </div>

      <div className="chat__main">
        <div className="chat__head">
          <strong>{selected?.name ?? conversation}</strong>
          {conversation === NETWORK_CONVERSATION ? (
            <span className="muted small">{t.chat.allNote}</span>
          ) : selected?.group && !selected.left ? (
            <>
              <span className="muted small">
                {t.chat.groupCount(selected.group.members.length)}
              </span>
              <button
                type="button"
                className="button--ghost chat__manage"
                onClick={() => setManaging(selected.group)}
              >
                {t.chat.groupManage}
              </button>
            </>
          ) : groupIdOf(conversation) !== null ? (
            <span className="muted small">
              {selected?.group ? t.chat.leftGroup : t.chat.groupPending}
            </span>
          ) : selected?.member ? (
            <span className="muted small">
              {selectedBlocked
                ? `🚫 ${t.tunnel.member.blocked}`
                : selected.online
                  ? t.tunnel.member.online
                  : t.tunnel.member.offline}
            </span>
          ) : (
            <span className="muted small">{t.chat.left}</span>
          )}
        </div>

        <div className="chat__messages" ref={listRef} onScroll={() => {
          const el = listRef.current;
          if (el) {
            stickBottom.current =
              el.scrollHeight - el.scrollTop - el.clientHeight < 48;
          }
        }}>
          {current.length === 0 ? (
            <p className="muted small chat__empty">{t.chat.empty}</p>
          ) : (
            <Bubbles
              messages={current}
              selfIp={selfIp}
              memberByIp={memberByIp}
              showNames={showNames}
              transfers={tunnel.transfers}
              sendingSeqs={sendingSeqs}
              configPath={tunnel.config}
              onOpenRef={onOpenRef}
              onResend={(seq) =>
                void api
                  .chatResend(tunnel.config, seq)
                  .catch((e) => setError(errorMessage(e)))
              }
              onCancelSend={(seq) =>
                void api
                  .chatCancelSend(tunnel.config, seq)
                  .catch((e) => setError(errorMessage(e)))
              }
              onSaveFile={(name) => void saveFile(name)}
              onEnlarge={(src, name) => setViewer({ kind: "image", src, name })}
              onOpenText={(name, preview) =>
                setViewer({ kind: "text", name, preview })
              }
            />
          )}
          {dragOver && canSendFiles && (
            <div className="chat__drop" aria-hidden>
              {t.chat.dropHint(selected?.name ?? conversation)}
            </div>
          )}
        </div>

        {error && <p className="error-text small chat__error">{error}</p>}
        {notice && <p className="notice small chat__error">{notice}</p>}
        <div className="chat__input">
          <button
            type="button"
            className="button--icon chat__attach"
            title={t.chat.attach}
            disabled={!canSendFiles}
            onClick={() => void attach()}
          >
            📎
          </button>
          <div className="chat__textarea-wrap">
            <textarea
              ref={inputRef}
              rows={1}
              className="chat__textarea"
              value={draft}
              placeholder={
                canSend
                  ? t.chat.placeholder
                  : groupIdOf(conversation) !== null
                    ? t.chat.leftGroup
                    : selectedBlocked
                      ? t.chat.blocked
                      : t.chat.offline
              }
              disabled={!canSend || sending}
              onChange={(event) => handleDraftChange(event.target)}
              onPaste={(event) => void handlePaste(event)}
              onKeyDown={(event) => {
                // Enter で送信、Shift+Enter で改行。IME の変換確定では送らない
                if (
                  event.key === "Enter" &&
                  !event.shiftKey &&
                  !event.nativeEvent.isComposing
                ) {
                  event.preventDefault();
                  void send();
                }
              }}
            />
            <MentionSuggestList candidates={mentionCandidates} onPick={insertMention} />
          </div>
          <button
            type="button"
            onClick={() => void send()}
            disabled={!canSend || sending || draft.trim() === ""}
          >
            {t.chat.send}
          </button>
        </div>
      </div>

      {creating && (
        <GroupDialog
          tunnel={tunnel}
          group={null}
          onClose={() => setCreating(false)}
          onDone={(group) => {
            setCreating(false);
            setConversation(groupConversation(group.id));
          }}
        />
      )}
      {managing && (
        <GroupDialog
          tunnel={tunnel}
          group={managing}
          onClose={() => setManaging(null)}
          onDone={() => setManaging(null)}
          onLeft={() => {
            setManaging(null);
            setConversation(NETWORK_CONVERSATION);
          }}
        />
      )}
      {viewer && (
        <Modal title={viewer.name} onClose={() => setViewer(null)} wide>
          {viewer.kind === "image" ? (
            <div className="chat__viewer">
              <img src={viewer.src} alt={viewer.name} />
            </div>
          ) : (
            <div className="chat__viewer-text">
              {viewer.preview.truncated && (
                <p className="muted small">{t.chat.textTruncated}</p>
              )}
              <pre>{viewer.preview.text}</pre>
            </div>
          )}
        </Modal>
      )}
      {dropPaths &&
        (canSend ? (
          <ConfirmModal
            title={t.chat.dropTitle}
            confirmLabel={t.chat.send}
            onClose={() => setDropPaths(null)}
            onConfirm={() => {
              const paths = dropPaths;
              setDropPaths(null);
              void sendFiles(paths);
            }}
            message={
              <>
                <p>{t.chat.dropMessage(selected?.name ?? conversation)}</p>
                <ul className="chat__drop-list">
                  {dropPaths.map((path) => (
                    <li key={path} className="mono ellipsis" title={path}>
                      {baseName(path)}
                    </li>
                  ))}
                </ul>
              </>
            }
          />
        ) : (
          <ConfirmModal
            title={t.chat.dropTitle}
            confirmLabel={t.common.close}
            onClose={() => setDropPaths(null)}
            onConfirm={() => setDropPaths(null)}
            message={
              groupIdOf(conversation) !== null
                ? t.chat.leftGroup
                : t.chat.offline
            }
          />
        ))}
      {menu &&
        (() => {
          const key = menu.key;
          const pinned = pinIndex.has(key);
          const pos = pinIndex.get(key);
          const muted = mutes.has(key);
          const act = (fn: () => void) => {
            fn();
            setMenu(null);
            rerender();
          };
          return (
            <ul
              className="chat__menu"
              style={{ left: menu.x, top: menu.y }}
              onClick={(e) => e.stopPropagation()}
            >
              <li>
                <button
                  type="button"
                  onClick={() => act(() => togglePin(tunnel.config, key))}
                >
                  {pinned ? t.chat.unpin : t.chat.pin}
                </button>
              </li>
              {pinned && (
                <>
                  <li>
                    <button
                      type="button"
                      disabled={pos === 0}
                      onClick={() => act(() => movePin(tunnel.config, key, -1))}
                    >
                      {t.chat.moveUp}
                    </button>
                  </li>
                  <li>
                    <button
                      type="button"
                      disabled={pos === pinOrder.length - 1}
                      onClick={() => act(() => movePin(tunnel.config, key, 1))}
                    >
                      {t.chat.moveDown}
                    </button>
                  </li>
                </>
              )}
              <li>
                <button
                  type="button"
                  onClick={() => act(() => toggleMute(tunnel.config, key))}
                >
                  {muted ? t.chat.unmute : t.chat.mute}
                </button>
              </li>
            </ul>
          );
        })()}
    </div>
  );
}

/**
 * グループの作成・管理ダイアログ(M3-13c)。`group` が null なら作成、
 * あれば管理(改名・メンバー追加・退出)。
 */
function GroupDialog({
  tunnel,
  group,
  onClose,
  onDone,
  onLeft,
}: {
  tunnel: Tunnel;
  group: Group | null;
  onClose: () => void;
  onDone: (group: Group) => void;
  onLeft?: () => void;
}) {
  const [name, setName] = useState(group?.name ?? "");
  const [checked, setChecked] = useState<Set<string>>(new Set());
  const [removeChecked, setRemoveChecked] = useState<Set<string>>(new Set());
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [confirmLeave, setConfirmLeave] = useState(false);

  // 作成 = 自分以外の全メンバー / 管理 = まだグループに居ないメンバーが候補
  const candidates = tunnel.members.filter(
    (member) =>
      !member.isSelf && (group === null || !group.members.includes(member.ip)),
  );
  // 外せるのは自分以外の現メンバー(キック — 2026-07-20 検証 FB)
  const removables =
    group === null
      ? []
      : tunnel.members.filter(
          (member) => !member.isSelf && group.members.includes(member.ip),
        );
  const memberByIp = new Map(tunnel.members.map((m) => [m.ip, m]));

  const toggleIn =
    (setter: React.Dispatch<React.SetStateAction<Set<string>>>) =>
    (ip: string) => {
      setter((prev) => {
        const next = new Set(prev);
        if (next.has(ip)) {
          next.delete(ip);
        } else {
          next.add(ip);
        }
        return next;
      });
    };
  const toggle = toggleIn(setChecked);
  const toggleRemove = toggleIn(setRemoveChecked);

  const submit = async () => {
    setBusy(true);
    setError(null);
    try {
      const result =
        group === null
          ? await api.groupCreate(tunnel.config, name.trim(), [...checked])
          : await api.groupUpdate(
              tunnel.config,
              group.id,
              name.trim() !== group.name ? name.trim() : null,
              [...checked],
              [...removeChecked],
            );
      onDone(result);
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(false);
    }
  };

  const leave = async () => {
    setBusy(true);
    setError(null);
    try {
      await api.groupLeave(tunnel.config, group!.id);
      onLeft?.();
    } catch (e) {
      setError(errorMessage(e));
      setBusy(false);
      setConfirmLeave(false);
    }
  };

  return (
    <Modal
      title={group === null ? t.chat.groupTitle : t.chat.groupManage}
      onClose={onClose}
    >
      <div className="modal__body">
        <label className="field">
          <span>{t.chat.groupNameLabel}</span>
          <input
            value={name}
            autoFocus={group === null}
            placeholder={t.chat.groupNamePlaceholder}
            onChange={(event) => setName(event.target.value)}
          />
        </label>

        {group !== null && (
          <span className="muted small">
            {t.chat.groupMembersHead}:{" "}
            {group.members
              .map((ip) => memberByIp.get(ip)?.name ?? ip)
              .join("、")}
          </span>
        )}

        <span className="field__label">
          {group === null ? t.chat.groupMembersLabel : t.chat.groupAddLabel}
        </span>
        {candidates.length === 0 ? (
          <p className="muted small">{t.chat.groupNoCandidates}</p>
        ) : (
          <ul className="chat__pick">
            {candidates.map((member) => (
              <li key={member.ip}>
                <label className="chat__pick-row">
                  <input
                    type="checkbox"
                    checked={checked.has(member.ip)}
                    onChange={() => toggle(member.ip)}
                  />
                  <Avatar
                    publicKey={member.publicKey}
                    name={member.name}
                    online={member.online}
                    onlineLabel={
                      member.online
                        ? t.tunnel.member.online
                        : t.tunnel.member.offline
                    }
                  />
                  <span className="ellipsis">{member.name ?? member.ip}</span>
                  {!member.online && (
                    <span className="muted small">
                      {t.tunnel.member.offline}
                    </span>
                  )}
                </label>
              </li>
            ))}
          </ul>
        )}
        {removables.length > 0 && (
          <>
            <span className="field__label">{t.chat.groupRemoveLabel}</span>
            <ul className="chat__pick">
              {removables.map((member) => (
                <li key={member.ip}>
                  <label className="chat__pick-row">
                    <input
                      type="checkbox"
                      checked={removeChecked.has(member.ip)}
                      onChange={() => toggleRemove(member.ip)}
                    />
                    <Avatar
                      publicKey={member.publicKey}
                      name={member.name}
                      online={member.online}
                      onlineLabel={
                        member.online
                          ? t.tunnel.member.online
                          : t.tunnel.member.offline
                      }
                    />
                    <span className="ellipsis">{member.name ?? member.ip}</span>
                  </label>
                </li>
              ))}
            </ul>
          </>
        )}
        <p className="muted small">{t.chat.groupNote}</p>
        {error && <p className="error-text small">{error}</p>}
      </div>
      <div className="modal__actions">
        {group !== null && (
          <button
            type="button"
            className="button--danger"
            onClick={() => setConfirmLeave(true)}
            disabled={busy}
          >
            {t.chat.leave}
          </button>
        )}
        <button type="button" className="button--ghost" onClick={onClose}>
          {t.common.cancel}
        </button>
        <button
          type="button"
          onClick={() => void submit()}
          disabled={
            busy ||
            name.trim() === "" ||
            (group === null && checked.size === 0) ||
            (group !== null &&
              checked.size === 0 &&
              removeChecked.size === 0 &&
              name.trim() === group.name)
          }
        >
          {group === null ? t.chat.create : t.chat.save}
        </button>
      </div>

      {confirmLeave && group !== null && (
        <ConfirmModal
          title={t.chat.leaveTitle}
          confirmLabel={t.chat.leave}
          busy={busy}
          onClose={() => setConfirmLeave(false)}
          onConfirm={() => void leave()}
          message={t.chat.leaveConfirm(group.name)}
        />
      )}
    </Modal>
  );
}

function lastMessageOf(
  messages: ChatMessage[],
  selfIp: string,
  key: ConversationKey,
): ChatMessage | null {
  for (let i = messages.length - 1; i >= 0; i--) {
    if (conversationOf(messages[i], selfIp) === key) return messages[i];
  }
  return null;
}

function previewOf(message: ChatMessage, selfIp: string): string {
  // グループ操作のお知らせは本文そのまま(「自分: 」を付けない)
  if (message.system) return message.text;
  const prefix = message.from === selfIp ? t.chat.previewSelf : "";
  const body = message.file
    ? t.chat.filePreview(message.file.name)
    : message.text.replace(/\s+/g, " ");
  return `${prefix}${body}`;
}

/** テキストとしてプレビューする拡張子(M3-13e)。 */
const TEXT_EXTS = [
  "txt", "md", "log", "csv", "tsv", "json", "xml", "yaml", "yml", "toml",
  "ini", "conf", "sh", "ps1", "bat", "cmd", "py", "rs", "js", "ts", "jsx",
  "tsx", "c", "h", "cpp", "java", "go", "sql", "html", "css",
];

/** 拡張子からインラインプレビューの種類を決める(M3-13d 検証 FB)。 */
function mediaKind(name: string): "image" | "video" | "audio" | "text" | null {
  const ext = name.split(".").pop()?.toLowerCase() ?? "";
  if (["png", "jpg", "jpeg", "gif", "webp", "bmp", "avif", "svg"].includes(ext)) {
    return "image";
  }
  if (["mp4", "webm", "mov", "m4v"].includes(ext)) return "video";
  if (["mp3", "wav", "ogg", "m4a", "flac", "aac"].includes(ext)) return "audio";
  if (TEXT_EXTS.includes(ext)) return "text";
  return null;
}

/** テキストプレビューのバブル内表示(先頭数行だけ)。 */
function textSnippet(text: string): string {
  const lines = text.split("\n");
  const head = lines.slice(0, 6).join("\n").slice(0, 400);
  return head.length < text.length ? `${head}\n…` : head;
}

/** 本文を URL とそれ以外に分ける(M3-13e)。 */
function splitLinks(text: string): Array<{ link: boolean; value: string }> {
  const parts: Array<{ link: boolean; value: string }> = [];
  const re = /https?:\/\/\S+/g;
  let last = 0;
  let match: RegExpExecArray | null;
  while ((match = re.exec(text)) !== null) {
    // 文末の句読点・閉じ括弧はリンクに含めない
    const url = match[0].replace(/[.,!?;:。、」』）)\]>"']+$/u, "");
    if (url.length === 0) continue;
    if (match.index > last) {
      parts.push({ link: false, value: text.slice(last, match.index) });
    }
    parts.push({ link: true, value: url });
    last = match.index + url.length;
  }
  if (last < text.length) parts.push({ link: false, value: text.slice(last) });
  return parts;
}

/** 本文中の最初の URL(リンクプレビューの対象)。 */
function firstUrl(text: string): string | null {
  return splitLinks(text).find((part) => part.link)?.value ?? null;
}

/**
 * URL をクリックできるリンクにし、`@名前` / `@All` のメンションを強調表示
 * して本文を描画する(既定ブラウザで開く、ADR-0055 決定 1)。`memberNames`
 * に一致する名前(または自分の名前・All)だけをメンションとして認識する
 * (無関係な `@example.com` 等を誤検知しないため)。
 */
function linkify(text: string, myName: string, memberNames: string[]): ReactNode {
  const parts = splitLinks(text);
  let key = 0;
  const nodes: ReactNode[] = [];
  for (const part of parts) {
    if (part.link) {
      nodes.push(
        <a
          key={key++}
          className="msg__link"
          href={part.value}
          title={part.value}
          onClick={(event) => {
            event.preventDefault();
            void api.openLink(part.value).catch(() => {});
          }}
        >
          {part.value}
        </a>,
      );
      continue;
    }
    for (const seg of splitMentions(part.value, myName, memberNames)) {
      if (seg.mention) {
        nodes.push(
          <span
            key={key++}
            className={seg.self ? "msg__mention msg__mention--self" : "msg__mention"}
          >
            {seg.value}
          </span>,
        );
      } else if (seg.value !== "") {
        nodes.push(<span key={key++}>{seg.value}</span>);
      }
    }
  }
  return nodes;
}

/**
 * 本文を描画する(M5 F-5 Stage 4、ADR-0052 決定 1)。`@memo:id` トークンは
 * その位置でカード化し、残りの地の文は従来どおり URL のリンク化 + メンション
 * (`@名前` / `@All`)の強調表示を行う(ADR-0055 決定 1)。
 */
function renderMessageBody(
  text: string,
  configPath: string,
  onOpenRef: (kind: SharedRefKind, id: string) => void,
  myName: string,
  memberNames: string[],
): ReactNode {
  const parts = splitSharedRefs(text);
  if (!parts.some((part) => part.type === "ref")) return linkify(text, myName, memberNames);
  return parts.map((part, index) =>
    part.type === "ref" ? (
      <SharedRefCard
        key={index}
        configPath={configPath}
        token={part.token}
        onOpen={() => onOpenRef(part.token.kind, part.token.id)}
      />
    ) : (
      <span key={index}>{linkify(part.value, myName, memberNames)}</span>
    ),
  );
}

/**
 * 共有オブジェクト参照のカード(M5 F-5 Stage 4、ADR-0052 決定 1)。表示時に
 * 受信者自身の権限で解決する(メモはキャッシュ経由 = オフラインでも出る)。
 * 取得できない場合は「アクセスできません」カード(タイトル等は出さない)。
 */
function SharedRefCard({
  configPath,
  token,
  onOpen,
}: {
  configPath: string;
  token: SharedRefTokenValue;
  onOpen: () => void;
}) {
  const resolved = useSharedRefResolve(configPath, token);
  if (resolved === undefined) {
    return (
      <span className="msg__ref msg__ref--loading">
        <span className="msg__ref-icon" aria-hidden>
          {sharedRefIcon(token.kind)}
        </span>
        <span className="msg__ref-title muted">{t.sharedRef.loading}</span>
      </span>
    );
  }
  if (resolved === null) {
    return (
      <span className="msg__ref msg__ref--locked">
        <span className="msg__ref-icon" aria-hidden>
          🔒
        </span>
        <span className="msg__ref-title">
          {t.sharedRef.inaccessible(sharedRefNoun(token.kind))}
        </span>
      </span>
    );
  }
  return (
    <button type="button" className="msg__ref" onClick={onOpen}>
      <span className="msg__ref-icon" aria-hidden>
        {sharedRefIcon(token.kind)}
      </span>
      <span className="msg__ref-text">
        <span className="msg__ref-title">
          {resolved.title || t.memo.untitled}
        </span>
        {resolved.excerpt && (
          <span className="msg__ref-excerpt">{resolved.excerpt}</span>
        )}
      </span>
    </button>
  );
}

/** 表示用のホスト名。 */
function hostOf(url: string): string {
  try {
    return new URL(url).hostname;
  } catch {
    return url;
  }
}

// リンクプレビューの結果は URL ごとに使い回す(null = 取れなかった)。
// 同じ URL を含むメッセージが並んでも取得は 1 回で済む
const previewCache = new Map<string, LinkPreview | null>();
const previewPending = new Map<string, Promise<LinkPreview | null>>();

/**
 * リンクプレビューのカード(M3-13e、ADR-0017)。表示中の端末が自分で
 * ページ情報(OGP)を取りに行く。取れなかったら何も出さない。
 */
function LinkPreviewCard({ url }: { url: string }) {
  const [data, setData] = useState<LinkPreview | null | undefined>(() =>
    previewCache.get(url),
  );
  useEffect(() => {
    if (previewCache.has(url)) {
      setData(previewCache.get(url));
      return;
    }
    let alive = true;
    let pending = previewPending.get(url);
    if (!pending) {
      pending = api.linkPreview(url).then(
        (preview) => preview,
        () => null,
      );
      previewPending.set(url, pending);
    }
    void pending.then((preview) => {
      previewCache.set(url, preview);
      previewPending.delete(url);
      if (alive) setData(preview);
    });
    return () => {
      alive = false;
    };
  }, [url]);
  if (!data) return null;
  return (
    <a
      className="msg__preview"
      href={url}
      title={url}
      onClick={(event) => {
        event.preventDefault();
        void api.openLink(url).catch(() => {});
      }}
    >
      {data.image && (
        <img className="msg__preview-img" src={data.image} alt="" />
      )}
      <span className="msg__preview-text">
        {data.title && (
          <span className="msg__preview-title">{data.title}</span>
        )}
        {data.description && (
          <span className="msg__preview-desc">{data.description}</span>
        )}
        <span className="msg__preview-host">
          {data.siteName ?? hostOf(url)}
        </span>
      </span>
    </a>
  );
}

function timeOf(unixMs: number): string {
  const at = new Date(unixMs);
  const pad = (value: number) => String(value).padStart(2, "0");
  return `${pad(at.getHours())}:${pad(at.getMinutes())}`;
}

/** 吹き出しの列(日付が変わる箇所に区切り、グループ操作は中央の 1 行)。 */
function Bubbles({
  messages,
  selfIp,
  memberByIp,
  showNames,
  transfers,
  onSaveFile,
  onEnlarge,
  sendingSeqs,
  onResend,
  onCancelSend,
  onOpenText,
  configPath,
  onOpenRef,
}: {
  messages: ChatMessage[];
  selfIp: string;
  memberByIp: Map<string, Member>;
  showNames: boolean;
  transfers: Transfer[];
  sendingSeqs: Set<number>;
  onResend: (seq: number) => void;
  onCancelSend: (seq: number) => void;
  onSaveFile: (name: string) => void;
  onEnlarge: (src: string, name: string) => void;
  onOpenText: (name: string, preview: TextPreview) => void;
  configPath: string;
  /** 本文中の `@memo:id` / `@schedule:id` カードをクリックしたときの遷移先。 */
  onOpenRef: (kind: SharedRefKind, id: string) => void;
}) {
  let lastDate = "";
  // リンクプレビューはアプリ設定でオフにできる(M3-13e)
  const showLinkPreview = loadPrefs().linkPreview;
  // メンション強調表示(ADR-0055 決定 1)用に自分の名前・全メンバー名を控える
  const myName = memberByIp.get(selfIp)?.name ?? "";
  const memberNames = [...memberByIp.values()]
    .filter((m) => m.ip !== selfIp)
    .map((m) => m.name)
    .filter((name): name is string => Boolean(name));
  return (
    <>
      {messages.map((message) => {
        const date = new Date(message.sentAtMs).toLocaleDateString();
        const separator = date !== lastDate;
        lastDate = date;
        const own = message.from === selfIp;
        const sender = memberByIp.get(message.from) ?? null;
        const previewUrl =
          showLinkPreview && !message.system && !message.file
            ? firstUrl(message.text)
            : null;
        return (
          <div key={message.seq} className="chat__flow">
            {separator && <div className="chat__date">{date}</div>}
            {message.system ? (
              // グループ操作・メンションのお知らせ(ADR-0055 決定 1d の
              // ローカルお知らせ行も同じ形。`@memo:id` はカード化される — LINE 風)
              <div className="chat__system">
                {renderMessageBody(message.text, configPath, onOpenRef, myName, memberNames)}
              </div>
            ) : (
              <div className={own ? "msg msg--own" : "msg"}>
                {!own && (
                  <span className="msg__avatar">
                    {sender ? (
                      <Avatar
                        publicKey={sender.publicKey}
                        name={sender.name}
                        online={sender.online}
                        onlineLabel=""
                      />
                    ) : (
                      <span className="avatar" aria-hidden>
                        ?
                      </span>
                    )}
                  </span>
                )}
                <span className="msg__body">
                  {!own && showNames && (
                    <span className="msg__name muted small">
                      {sender?.name ?? message.from}
                    </span>
                  )}
                  <span className="msg__row">
                    {message.file ? (
                      <FileBubble
                        message={message}
                        own={own}
                        transfers={transfers}
                        onSave={onSaveFile}
                        onEnlarge={onEnlarge}
                        onOpenText={onOpenText}
                      />
                    ) : (
                      <span className="msg__bubble">
                        {renderMessageBody(
                          message.text,
                          configPath,
                          onOpenRef,
                          myName,
                          memberNames,
                        )}
                      </span>
                    )}
                    <span className="msg__time muted">
                      {timeOf(message.sentAtMs)}
                      {/* 送信キュー(E-E 3): 送信中 / 未送信(自動再送)+取消 /
                          失敗+再送 の 3 状態 */}
                      {own && sendingSeqs.has(message.seq) && !message.failed && (
                        <span className="muted"> {t.chat.sendingState}</span>
                      )}
                      {own && sendingSeqs.has(message.seq) && message.failed && (
                        <>
                          <span className="error-text"> {t.chat.retrying}</span>{" "}
                          <button
                            type="button"
                            className="msg__action"
                            onClick={() => onCancelSend(message.seq)}
                          >
                            {t.chat.cancelSend}
                          </button>
                        </>
                      )}
                      {own && !sendingSeqs.has(message.seq) && message.failed && (
                        <>
                          <span className="error-text"> {t.chat.failed}</span>{" "}
                          <button
                            type="button"
                            className="msg__action"
                            onClick={() => onResend(message.seq)}
                          >
                            {t.chat.resend}
                          </button>
                        </>
                      )}
                    </span>
                  </span>
                  {previewUrl && <LinkPreviewCard url={previewUrl} />}
                </span>
              </div>
            )}
          </div>
        );
      })}
    </>
  );
}

/**
 * ファイルバブル(M3-13d)。進捗は Tunnel.transfers と転送 id で突き合わせる
 * (転送一覧から流れた古いエントリは進捗なしで表示)。画像・動画・音声は
 * その場でプレビューし、画像はクリックで拡大(2026-07-11 検証 FB)。
 * 受信済みのファイルは「保存」で受信ボックスから任意の場所へ移せる。
 */
function FileBubble({
  message,
  own,
  transfers,
  onSave,
  onEnlarge,
  onOpenText,
}: {
  message: ChatMessage;
  own: boolean;
  transfers: Transfer[];
  onSave: (name: string) => void;
  onEnlarge: (src: string, name: string) => void;
  onOpenText: (name: string, preview: TextPreview) => void;
}) {
  // プレビューの読み込みに失敗したら通常のファイル表示に戻す
  // (保存・削除でファイルが移動した後など)
  const [broken, setBroken] = useState(false);
  /** テキストプレビューの中身(M3-13e。kind = text のとき読み込む)。 */
  const [text, setText] = useState<TextPreview | null>(null);
  const file = message.file!;
  const related = transfers.filter((tr) => file.transfers.includes(tr.id));
  const active = related.filter((tr) => !tr.done);
  const failed =
    message.failed ||
    (related.length > 0 && related.every((tr) => tr.error !== null));
  const totalSize = active.reduce((sum, tr) => sum + tr.size, 0);
  const transferred = active.reduce((sum, tr) => sum + tr.transferred, 0);
  const percent =
    totalSize === 0
      ? 100
      : Math.min(100, Math.floor((transferred * 100) / totalSize));

  // プレビュー: 種類が分かり、場所が分かり、転送が終わっているとき
  // (送信側は元ファイルが手元にあるので転送中でもよい)
  const kind = mediaKind(file.name);
  const ready = own || (active.length === 0 && !failed);
  const src =
    !broken && kind !== null && kind !== "text" && ready && file.path
      ? convertFileSrc(file.path)
      : null;

  // テキストはファイルの先頭を読んで数行だけ出す(クリックで全文 — M3-13e)。
  // 読めない(バイナリ・移動済みなど)ときは通常のファイル表示に戻す
  const filePath = file.path;
  const wantText = kind === "text" && ready && !broken && filePath !== null;
  useEffect(() => {
    if (!wantText || text !== null) return;
    let alive = true;
    api
      .readTextPreview(filePath!)
      .then((preview) => {
        if (alive) setText(preview);
      })
      .catch(() => {
        if (alive) setBroken(true);
      });
    return () => {
      alive = false;
    };
  }, [wantText, filePath, text]);

  return (
    <span className="msg__bubble msg__bubble--file">
      {src && kind === "image" && (
        <img
          className="msg__media msg__media--image"
          src={src}
          alt={file.name}
          loading="lazy"
          onClick={() => onEnlarge(src, file.name)}
          onError={() => setBroken(true)}
        />
      )}
      {src && kind === "video" && (
        // 再生できない形式(コーデック不足など)は通常表示に戻す
        <video
          className="msg__media"
          src={src}
          controls
          preload="metadata"
          onError={() => setBroken(true)}
        />
      )}
      {src && kind === "audio" && (
        <audio src={src} controls onError={() => setBroken(true)} />
      )}
      {wantText && text && (
        <pre
          className="msg__media msg__media--text"
          title={t.chat.textOpen}
          onClick={() => onOpenText(file.name, text)}
        >
          {textSnippet(text.text)}
        </pre>
      )}
      <span className="msg__file-name ellipsis" title={file.name}>
        📎 {file.name}
      </span>
      <span className={own ? "msg__file-meta" : "msg__file-meta muted"}>
        {formatBytes(file.size)}
      </span>
      {failed ? (
        <span className="error-text small">{t.chat.fileFailed}</span>
      ) : active.length > 0 ? (
        <span className="progress" title={`${percent}%`}>
          <span className="progress__bar" style={{ width: `${percent}%` }} />
        </span>
      ) : (
        !own && (
          <button
            type="button"
            className="msg__file-save"
            onClick={() => onSave(file.name)}
          >
            {t.inbox.save}
          </button>
        )
      )}
    </span>
  );
}
