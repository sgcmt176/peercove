import { useEffect, useMemo, useRef, useState } from "react";
import { ChatMessage, Group, Member, Tunnel, api, errorMessage } from "../ipc";
import {
  ConversationKey,
  NETWORK_CONVERSATION,
  appendLocal,
  chatMessages,
  conversationOf,
  groupConversation,
  groupIdOf,
  markRead,
  setActiveConversation,
  unreadCounts,
} from "../chat";
import { Avatar } from "./Avatar";
import { ConfirmModal, Modal } from "./Modal";
import { t } from "../i18n";

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
 * チャットタブ(M3-13b/c、ADR-0016)。LINE 風の 2 ペイン:
 * 左 = 会話リスト(全体 + グループ + メンバー 1:1、未読バッジ、グループ作成)、
 * 右 = 吹き出しの会話。履歴は chat.ts のストア(App の 2 秒ポーリングが
 * 差分フェッチ済み)を読む。
 */
export function ChatPanel({ tunnel }: { tunnel: Tunnel }) {
  const [conversation, setConversation] = useState<ConversationKey>(
    NETWORK_CONVERSATION,
  );
  const [draft, setDraft] = useState("");
  const [sending, setSending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  /** グループ作成ダイアログ(M3-13c)。 */
  const [creating, setCreating] = useState(false);
  /** グループ管理ダイアログの対象。 */
  const [managing, setManaging] = useState<Group | null>(null);
  // 送信直後・既読直後にポーリングを待たず再描画するためのカウンタ
  const [, setBump] = useState(0);
  const rerender = () => setBump((n) => n + 1);

  const selfIp = tunnel.address;
  const messages = chatMessages(tunnel.config);
  const unread = unreadCounts(tunnel);
  const memberByIp = new Map(tunnel.members.map((m) => [m.ip, m]));
  const groupById = new Map(tunnel.groups.map((g) => [g.id, g]));

  // 会話リスト: 全体 → 参加中のグループ → メンバー(台帳順)
  // → 履歴にだけ残っている会話(退出済みグループ・居なくなった相手)
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

  const current = messages.filter(
    (message) => conversationOf(message, selfIp) === conversation,
  );
  const selected = conversations.find((item) => item.key === conversation);
  const canSend =
    conversation === NETWORK_CONVERSATION ||
    (selected !== undefined &&
      !selected.left &&
      (selected.group !== null || selected.online));

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
      const groupId = groupIdOf(conversation);
      const peer =
        conversation === NETWORK_CONVERSATION || groupId !== null
          ? null
          : conversation;
      const message = await api.chatSend(tunnel.config, peer, groupId, text);
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

  const showNames =
    conversation === NETWORK_CONVERSATION || groupIdOf(conversation) !== null;

  return (
    <div className="chat">
      <div className="chat__list">
        <button
          type="button"
          className="chat__new-group"
          onClick={() => setCreating(true)}
        >
          ＋ {t.chat.groupCreate}
        </button>
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
            <span className="muted small">{t.chat.leftGroup}</span>
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
              showNames={showNames}
            />
          )}
        </div>

        {error && <p className="error-text small chat__error">{error}</p>}
        <div className="chat__input">
          <textarea
            rows={2}
            value={draft}
            placeholder={
              canSend
                ? t.chat.placeholder
                : groupIdOf(conversation) !== null
                  ? t.chat.leftGroup
                  : t.chat.offline
            }
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
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [confirmLeave, setConfirmLeave] = useState(false);

  // 作成 = 自分以外の全メンバー / 管理 = まだグループに居ないメンバーが候補
  const candidates = tunnel.members.filter(
    (member) =>
      !member.isSelf && (group === null || !group.members.includes(member.ip)),
  );
  const memberByIp = new Map(tunnel.members.map((m) => [m.ip, m]));

  const toggle = (ip: string) => {
    setChecked((prev) => {
      const next = new Set(prev);
      if (next.has(ip)) {
        next.delete(ip);
      } else {
        next.add(ip);
      }
      return next;
    });
  };

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
          <>
            <span className="muted small">
              {t.chat.groupMembersHead}:{" "}
              {group.members
                .map((ip) => memberByIp.get(ip)?.name ?? ip)
                .join("、")}
            </span>
          </>
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
            (group !== null && checked.size === 0 && name.trim() === group.name)
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
