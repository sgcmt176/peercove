import { useEffect, useMemo, useRef, useState } from "react";
import { ChatMessage, Member, Tunnel, api, errorMessage } from "../ipc";
import {
  ConversationKey,
  NETWORK_CONVERSATION,
  appendLocal,
  chatMessages,
  conversationOf,
  markRead,
  setActiveConversation,
  unreadCounts,
} from "../chat";
import { Avatar } from "./Avatar";
import { t } from "../i18n";

/**
 * チャットタブ(M3-13b、ADR-0016)。LINE 風の 2 ペイン:
 * 左 = 会話リスト(全体 + メンバー 1:1、未読バッジ)、右 = 吹き出しの会話。
 * 履歴は chat.ts のストア(App の 2 秒ポーリングが差分フェッチ済み)を読む。
 */
export function ChatPanel({ tunnel }: { tunnel: Tunnel }) {
  const [conversation, setConversation] = useState<ConversationKey>(
    NETWORK_CONVERSATION,
  );
  const [draft, setDraft] = useState("");
  const [sending, setSending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // 送信直後・既読直後にポーリングを待たず再描画するためのカウンタ
  const [, setBump] = useState(0);
  const rerender = () => setBump((n) => n + 1);

  const selfIp = tunnel.address;
  const messages = chatMessages(tunnel.config);
  const unread = unreadCounts(tunnel);
  const memberByIp = new Map(tunnel.members.map((m) => [m.ip, m]));

  // 会話リスト: 全体 → メンバー(台帳順) → 履歴にだけ残っている相手(退出済み)
  const conversations = useMemo(() => {
    const items: {
      key: ConversationKey;
      name: string;
      online: boolean;
      member: Member | null;
    }[] = [
      {
        key: NETWORK_CONVERSATION,
        name: t.chat.all,
        online: true,
        member: null,
      },
    ];
    for (const member of tunnel.members) {
      if (member.isSelf) continue;
      items.push({
        key: member.ip,
        name: member.name ?? member.ip,
        online: member.online,
        member,
      });
    }
    const known = new Set(items.map((item) => item.key));
    for (const message of messages) {
      const key = conversationOf(message, selfIp);
      if (!known.has(key)) {
        known.add(key);
        items.push({ key, name: key, online: false, member: null });
      }
    }
    return items;
  }, [tunnel.members, messages, selfIp]);

  const current = messages.filter(
    (message) => conversationOf(message, selfIp) === conversation,
  );
  const selected = conversations.find((item) => item.key === conversation);
  const canSend =
    conversation === NETWORK_CONVERSATION || (selected?.online ?? false);

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

  // 自動スクロール: 最下部付近にいるときだけ追従する(遡り閲覧を邪魔しない)
  const listRef = useRef<HTMLDivElement>(null);
  const stickBottom = useRef(true);
  useEffect(() => {
    stickBottom.current = true;
  }, [conversation]);
  useEffect(() => {
    const el = listRef.current;
    if (el && stickBottom.current) el.scrollTop = el.scrollHeight;
  }, [conversation, current.length]);

  const send = async () => {
    const text = draft.trim();
    if (!text || sending || !canSend) return;
    setSending(true);
    setError(null);
    try {
      const peer =
        conversation === NETWORK_CONVERSATION ? null : conversation;
      const message = await api.chatSend(tunnel.config, peer, text);
      appendLocal(tunnel.config, message);
      markRead(tunnel.config, conversation, message.seq);
      setDraft("");
      stickBottom.current = true;
      rerender();
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setSending(false);
    }
  };

  return (
    <div className="chat">
      <div className="chat__list">
        {conversations.map((item) => {
          const count = unread.get(item.key) ?? 0;
          const last = lastMessageOf(messages, selfIp, item.key);
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
              ) : (
                <span className="avatar chat__all-icon" aria-hidden>
                  {item.key === NETWORK_CONVERSATION ? "@" : "?"}
                </span>
              )}
              <span className="chat__conv-text">
                <span className="chat__conv-name ellipsis">{item.name}</span>
                <span className="chat__conv-preview ellipsis">
                  {last ? previewOf(last, selfIp) : t.chat.noMessages}
                </span>
              </span>
              {count > 0 && <span className="chat__badge">{count}</span>}
            </button>
          );
        })}
      </div>

      <div className="chat__main">
        <div className="chat__head">
          <strong>{selected?.name ?? conversation}</strong>
          {conversation === NETWORK_CONVERSATION ? (
            <span className="muted small">{t.chat.allNote}</span>
          ) : selected?.member ? (
            <span className="muted small">
              {selected.online
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
              showNames={conversation === NETWORK_CONVERSATION}
            />
          )}
        </div>

        {error && <p className="error-text small chat__error">{error}</p>}
        <div className="chat__input">
          <textarea
            rows={2}
            value={draft}
            placeholder={canSend ? t.chat.placeholder : t.chat.offline}
            disabled={!canSend || sending}
            onChange={(event) => setDraft(event.target.value)}
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
          <button
            type="button"
            onClick={() => void send()}
            disabled={!canSend || sending || draft.trim() === ""}
          >
            {t.chat.send}
          </button>
        </div>
      </div>
    </div>
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
  const prefix = message.from === selfIp ? t.chat.previewSelf : "";
  return `${prefix}${message.text.replace(/\s+/g, " ")}`;
}

function timeOf(unixMs: number): string {
  const at = new Date(unixMs);
  const pad = (value: number) => String(value).padStart(2, "0");
  return `${pad(at.getHours())}:${pad(at.getMinutes())}`;
}

/** 吹き出しの列(日付が変わる箇所に区切りを挟む)。 */
function Bubbles({
  messages,
  selfIp,
  memberByIp,
  showNames,
}: {
  messages: ChatMessage[];
  selfIp: string;
  memberByIp: Map<string, Member>;
  showNames: boolean;
}) {
  let lastDate = "";
  return (
    <>
      {messages.map((message) => {
        const date = new Date(message.sentAtMs).toLocaleDateString();
        const separator = date !== lastDate;
        lastDate = date;
        const own = message.from === selfIp;
        const sender = memberByIp.get(message.from) ?? null;
        return (
          <div key={message.seq} className="chat__flow">
            {separator && <div className="chat__date">{date}</div>}
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
                  <span className="msg__bubble">{message.text}</span>
                  <span className="msg__time muted">
                    {timeOf(message.sentAtMs)}
                    {message.failed && (
                      <span className="error-text"> {t.chat.failed}</span>
                    )}
                  </span>
                </span>
              </span>
            </div>
          </div>
        );
      })}
    </>
  );
}
