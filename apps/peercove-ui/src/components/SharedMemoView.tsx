// 共有メモ(M5 F-2、ADR-0049)。ホスト正本 + 権限 + 単一編集者ロック +
// リアルタイム閲覧。閲覧は常に可能(メンバーはキャッシュ = オフラインでも
// 読める)。編集は「編集」ボタンでロックを取得してから。保存はリビジョン
// CAS 付きでホストへ送られ、他の閲覧者へ数秒以内に配信される。
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import {
  MemoFolder,
  Member,
  SharedMemberPerm,
  SharedMemoDetail,
  SharedMemoOp,
  SharedMemoSummary,
  SharedPermLevel,
  api,
  errorMessage,
} from "../ipc";
import { t } from "../i18n";
import { Modal } from "./Modal";

const AUTOSAVE_DELAY_MS = 600;

export function SharedMemoView({
  configPath,
  isHost,
  supported,
  seq,
  members,
}: {
  configPath: string;
  isHost: boolean;
  /** 共有メモが使える状態か(member で false = ホスト未対応 or 未同期)。 */
  supported: boolean;
  /** 変更世代。進んだら再取得する。 */
  seq: number;
  members: Member[];
}) {
  const [folderId, setFolderId] = useState<string | null>(null);
  const [search, setSearch] = useState("");
  const [trashView, setTrashView] = useState(false);
  const [memos, setMemos] = useState<SharedMemoSummary[]>([]);
  const [folders, setFolders] = useState<MemoFolder[]>([]);
  const [offline, setOffline] = useState(false);
  const [notice, setNotice] = useState<string | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);

  const [selected, setSelected] = useState<SharedMemoDetail | null>(null);
  /** 編集ロックを取得済みで編集中か。true の間は配信で draft を上書きしない。 */
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState({ title: "", body: "" });
  const [mode, setMode] = useState<"edit" | "preview" | "split">("edit");
  const [saveState, setSaveState] = useState<"saved" | "saving" | "error">(
    "saved",
  );
  const [saveError, setSaveError] = useState("");
  const [permsFor, setPermsFor] = useState<SharedMemoDetail | null>(null);

  const bodyRef = useRef<HTMLTextAreaElement | null>(null);
  // 自動保存の土台(CAS 用リビジョンと保存済み内容)
  const baseRef = useRef<{
    id: string;
    revision: number;
    title: string;
    body: string;
  } | null>(null);
  const editingRef = useRef(false);
  editingRef.current = editing;
  const pendingRef = useRef<{ title: string; body: string } | null>(null);
  pendingRef.current = editing ? draft : null;

  const op = useCallback(
    (op: SharedMemoOp) => api.sharedMemoOp(configPath, op),
    [configPath],
  );

  const refresh = useCallback(async () => {
    try {
      const reply = await op({
        op: "list",
        query: {
          trash: trashView || undefined,
          folder_id: folderId ?? undefined,
          search: search.trim() || undefined,
        },
      });
      if (reply.kind === "memos") {
        setMemos(reply.memos);
        setFolders(reply.folders);
        setOffline(reply.offline ?? false);
        setLoadError(null);
      }
    } catch (error) {
      setLoadError(errorMessage(error));
    }
  }, [op, trashView, folderId, search]);

  useEffect(() => {
    void refresh();
    // seq(共有メモの変更世代)が進むたびに再取得 = リアルタイム反映
  }, [refresh, seq]);

  // 選択中メモも配信に追随する(編集中は上書きしない)
  useEffect(() => {
    const current = selected?.id;
    if (!current || editingRef.current) return;
    void op({ op: "get", id: current })
      .then((reply) => {
        if (reply.kind === "memo") {
          setSelected(reply.memo);
          setDraft({ title: reply.memo.title, body: reply.memo.body });
        }
      })
      .catch(() => setSelected(null)); // 削除・権限喪失
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [seq]);

  useEffect(() => {
    if (notice === null) return;
    const timer = window.setTimeout(() => setNotice(null), 6000);
    return () => window.clearTimeout(timer);
  }, [notice]);

  /** 保留中の変更を保存してロックを手放す(編集終了・切替時)。 */
  const stopEditing = useCallback(async () => {
    const base = baseRef.current;
    const pending = pendingRef.current;
    if (base && pending && (pending.title !== base.title || pending.body !== base.body)) {
      try {
        const reply = await op({
          op: "update",
          id: base.id,
          base_revision: base.revision,
          title: pending.title,
          body: pending.body,
        });
        if (reply.kind === "memo") {
          baseRef.current = {
            id: base.id,
            revision: reply.memo.revision,
            title: reply.memo.title,
            body: reply.memo.body,
          };
        }
      } catch {
        // 保存失敗は編集中の表示で気づけている(ここでは黙って抜ける)
      }
    }
    if (base) {
      try {
        await op({ op: "release_lock", id: base.id });
      } catch {
        // 切断時などはホスト側の自動解放に任せる
      }
    }
    baseRef.current = null;
    setEditing(false);
    setSaveState("saved");
  }, [op]);

  useEffect(() => () => void stopEditing(), [stopEditing]);

  const open = useCallback(
    async (id: string) => {
      await stopEditing();
      try {
        const reply = await op({ op: "get", id });
        if (reply.kind === "memo") {
          setSelected(reply.memo);
          setDraft({ title: reply.memo.title, body: reply.memo.body });
          setMode("edit");
        }
      } catch (error) {
        setNotice(errorMessage(error));
      }
    },
    [op, stopEditing],
  );

  /** 編集ロックを取得して編集を始める(最新内容が返る)。 */
  const startEditing = useCallback(
    async (id: string) => {
      try {
        const reply = await op({ op: "acquire_lock", id });
        if (reply.kind === "memo") {
          setSelected(reply.memo);
          setDraft({ title: reply.memo.title, body: reply.memo.body });
          baseRef.current = {
            id,
            revision: reply.memo.revision,
            title: reply.memo.title,
            body: reply.memo.body,
          };
          setEditing(true);
          setSaveState("saved");
          setSaveError("");
        }
      } catch (error) {
        setNotice(errorMessage(error));
      }
    },
    [op],
  );

  // 自動保存(CAS)。編集中のみ
  useEffect(() => {
    const base = baseRef.current;
    if (
      !editing ||
      base === null ||
      (base.title === draft.title && base.body === draft.body)
    ) {
      return;
    }
    setSaveState("saving");
    const timer = window.setTimeout(() => {
      const base = baseRef.current;
      if (base === null) return;
      void op({
        op: "update",
        id: base.id,
        base_revision: base.revision,
        title: draft.title,
        body: draft.body,
      })
        .then((reply) => {
          if (reply.kind === "memo") {
            baseRef.current = {
              id: base.id,
              revision: reply.memo.revision,
              title: reply.memo.title,
              body: reply.memo.body,
            };
            setSelected(reply.memo);
            setSaveState("saved");
            setSaveError("");
          }
        })
        .catch((error) => {
          setSaveState("error");
          setSaveError(errorMessage(error));
        });
    }, AUTOSAVE_DELAY_MS);
    return () => window.clearTimeout(timer);
  }, [draft, editing, op]);

  const createMemo = useCallback(async () => {
    await stopEditing();
    try {
      const reply = await op({
        op: "create",
        title: "",
        body: "",
        folder_id: folderId ?? undefined,
      });
      if (reply.kind === "memo") {
        setTrashView(false);
        setSelected(reply.memo);
        setDraft({ title: "", body: "" });
        await startEditing(reply.memo.id);
      }
    } catch (error) {
      setNotice(errorMessage(error));
    }
  }, [op, stopEditing, startEditing, folderId]);

  const run = useCallback(
    async (operation: SharedMemoOp, done?: () => void) => {
      try {
        await op(operation);
        done?.();
        void refresh();
      } catch (error) {
        setNotice(errorMessage(error));
      }
    },
    [op, refresh],
  );

  const closeSelected = useCallback(() => {
    setSelected(null);
    baseRef.current = null;
    setEditing(false);
  }, []);

  const copyToPersonal = useCallback(async () => {
    if (!selected) return;
    try {
      await api.memoOp({
        op: "create",
        title: selected.title,
        body: selected.body,
      });
      setNotice(t.sharedMemo.copiedToPersonal);
    } catch (error) {
      setNotice(errorMessage(error));
    }
  }, [selected]);

  const stats = useMemo(() => {
    const chars = [...draft.body].length;
    const lines = draft.body === "" ? 0 : draft.body.split("\n").length;
    return t.memo.stats(chars, lines);
  }, [draft.body]);

  if (loadError !== null) {
    return (
      <section className="card card--error">
        <h2>{t.sharedMemo.title}</h2>
        <p>{t.sharedMemo.loadFailed}</p>
        <pre className="error-detail">{loadError}</pre>
        <button type="button" onClick={() => void refresh()}>
          {t.common.retry}
        </button>
      </section>
    );
  }

  const readOnlyReason = offline
    ? t.sharedMemo.offline
    : !supported && !isHost
      ? t.sharedMemo.unsupported
      : null;

  return (
    <div className="memo">
      {permsFor && (
        <PermsDialog
          memo={permsFor}
          members={members}
          onClose={() => setPermsFor(null)}
          onSave={(everyone, memberPerms) =>
            void run(
              {
                op: "set_perms",
                id: permsFor.id,
                everyone,
                members: memberPerms,
              },
              () => setPermsFor(null),
            )
          }
        />
      )}
      <aside className="memo__side card">
        <div className="memo__side-head">
          <button
            type="button"
            disabled={readOnlyReason !== null}
            onClick={() => void createMemo()}
          >
            ＋ {t.memo.newMemo}
          </button>
        </div>
        {readOnlyReason && (
          <p className="memo__notice small">{readOnlyReason}</p>
        )}
        <p className="muted small">{t.sharedMemo.plaintextNote}</p>
        <input
          type="search"
          value={search}
          placeholder={t.memo.searchPlaceholder}
          onChange={(event) => setSearch(event.target.value)}
        />
        <div className="memo__filters">
          <button
            type="button"
            className={!trashView ? "memo__scope memo__scope--active" : "memo__scope"}
            onClick={() => setTrashView(false)}
          >
            {t.sharedMemo.scopeAll}
          </button>
          <button
            type="button"
            className={trashView ? "memo__scope memo__scope--active" : "memo__scope"}
            onClick={() => {
              setTrashView(true);
              closeSelected();
            }}
          >
            {t.memo.scopeTrash}
          </button>
        </div>
        {!trashView && (
          <SharedFolderList
            folders={folders}
            selected={folderId}
            isHost={isHost}
            onSelect={setFolderId}
            onRun={run}
            onNotice={setNotice}
          />
        )}
        <ul className="memo__list">
          {memos.length === 0 && (
            <li className="muted small memo__empty">
              {trashView ? t.memo.emptyTrash : t.sharedMemo.empty}
            </li>
          )}
          {memos.map((memo) => (
            <li key={memo.id}>
              <button
                type="button"
                className={
                  selected?.id === memo.id
                    ? "memo__item memo__item--active"
                    : "memo__item"
                }
                onClick={() => void open(memo.id)}
              >
                <span className="memo__item-title">
                  {memo.title || t.memo.untitled}
                </span>
                {memo.locked_by && (
                  <span className="memo__item-lock small">
                    ✏ {t.sharedMemo.editingBy(memo.locked_by)}
                  </span>
                )}
                {memo.excerpt && (
                  <span className="memo__item-excerpt muted small">
                    {memo.excerpt}
                  </span>
                )}
                <span className="memo__item-meta muted small">
                  {formatDate(memo.deleted_at ?? memo.updated_at)}
                  {memo.updated_by && ` ・${memo.updated_by}`}
                  {(memo.checklist_total ?? 0) > 0 && (
                    <>
                      {" ・☑ "}
                      {memo.checklist_done ?? 0}/{memo.checklist_total}
                    </>
                  )}
                  {!memo.can_edit && (
                    <span className="tag">{t.sharedMemo.viewerBadge}</span>
                  )}
                </span>
              </button>
            </li>
          ))}
        </ul>
      </aside>

      <section className="memo__editor card">
        {notice && <p className="memo__notice small">{notice}</p>}
        {selected === null ? (
          <p className="muted memo__placeholder">{t.sharedMemo.selectPrompt}</p>
        ) : (
          <div className="memo__editor-inner">
            <div className="memo__toolbar">
              <input
                className="memo__title"
                value={editing ? draft.title : selected.title}
                placeholder={t.memo.titlePlaceholder}
                readOnly={!editing}
                onChange={(event) =>
                  setDraft({ ...draft, title: event.target.value })
                }
              />
              <span
                className={
                  saveState === "error"
                    ? "memo__save memo__save--error small"
                    : "memo__save muted small"
                }
                title={saveState === "error" ? saveError : undefined}
              >
                {editing
                  ? saveState === "saving"
                    ? t.memo.saving
                    : saveState === "error"
                      ? t.memo.saveFailed
                      : t.memo.saved
                  : selected.locked_by
                    ? t.sharedMemo.editingBy(selected.locked_by)
                    : t.sharedMemo.viewing}
              </span>
            </div>
            {saveState === "error" && (
              <p className="memo__notice small">{saveError}</p>
            )}

            <div className="memo__actions">
              {!editing && !trashView && (
                <button
                  type="button"
                  disabled={
                    readOnlyReason !== null ||
                    !selected.can_edit ||
                    (selected.locked_by !== undefined &&
                      selected.locked_by !== null)
                  }
                  onClick={() => void startEditing(selected.id)}
                >
                  ✏ {t.sharedMemo.startEdit}
                </button>
              )}
              {editing && (
                <button type="button" onClick={() => void stopEditing()}>
                  ✓ {t.sharedMemo.stopEdit}
                </button>
              )}
              {isHost && !editing && selected.locked_by && (
                <button
                  type="button"
                  className="button--ghost"
                  onClick={() =>
                    window.confirm(t.sharedMemo.forceUnlockConfirm) &&
                    void run({ op: "force_unlock", id: selected.id })
                  }
                >
                  {t.sharedMemo.forceUnlock}
                </button>
              )}
              <span className="memo__actions-sep" />
              {editing && (
                <>
                  <ModeBtn label={t.memo.modeEdit} value="edit" mode={mode} onMode={setMode} />
                  <ModeBtn label={t.memo.modeSplit} value="split" mode={mode} onMode={setMode} />
                  <ModeBtn label={t.memo.modePreview} value="preview" mode={mode} onMode={setMode} />
                </>
              )}
              {!trashView && selected.can_manage && (
                <button
                  type="button"
                  className="button--icon"
                  title={t.sharedMemo.perms}
                  onClick={() => setPermsFor(selected)}
                >
                  🔑
                </button>
              )}
              <button
                type="button"
                className="button--icon"
                title={t.sharedMemo.copyToPersonal}
                onClick={() => void copyToPersonal()}
              >
                ⧉
              </button>
              <button
                type="button"
                className="button--icon"
                title={t.memo.exportNote}
                onClick={() =>
                  void api
                    .sharedMemoExport(configPath, selected.id)
                    .then((path) => path && setNotice(t.memo.exported(path)))
                    .catch((error) => setNotice(errorMessage(error)))
                }
              >
                💾
              </button>
              {!trashView && selected.can_manage && (
                <button
                  type="button"
                  className="button--icon"
                  title={t.memo.toTrash}
                  onClick={() =>
                    void run({ op: "trash", id: selected.id }, closeSelected)
                  }
                >
                  🗑
                </button>
              )}
              {trashView && selected.can_manage && (
                <>
                  <button
                    type="button"
                    onClick={() =>
                      void run({ op: "restore", id: selected.id }, closeSelected)
                    }
                  >
                    {t.memo.restore}
                  </button>
                  <button
                    type="button"
                    className="button--ghost button--ghost-danger"
                    onClick={() => {
                      if (window.confirm(t.memo.deleteForeverConfirm)) {
                        void run(
                          { op: "delete_forever", id: selected.id },
                          closeSelected,
                        );
                      }
                    }}
                  >
                    {t.memo.deleteForever}
                  </button>
                </>
              )}
            </div>

            <div
              className={
                editing && mode === "split"
                  ? "memo__panes memo__panes--split"
                  : "memo__panes"
              }
            >
              {editing && mode !== "preview" && (
                <textarea
                  ref={bodyRef}
                  className="memo__body"
                  value={draft.body}
                  placeholder={t.memo.bodyPlaceholder}
                  spellCheck={false}
                  onChange={(event) =>
                    setDraft({ ...draft, body: event.target.value })
                  }
                />
              )}
              {(!editing || mode !== "edit") && (
                <div className="memo__preview markdown">
                  <ReactMarkdown
                    remarkPlugins={[remarkGfm]}
                    components={{
                      a: ({ href, children }) => (
                        <a
                          href={href}
                          onClick={(event) => {
                            event.preventDefault();
                            if (href) void api.openLink(href);
                          }}
                        >
                          {children}
                        </a>
                      ),
                      input: ({ checked }) => (
                        <input type="checkbox" checked={Boolean(checked)} disabled readOnly />
                      ),
                    }}
                  >
                    {editing ? draft.body : selected.body}
                  </ReactMarkdown>
                </div>
              )}
            </div>

            <div className="memo__meta">
              <span className="muted small">
                {t.sharedMemo.ownerLabel(
                  selected.owner_name || t.sharedMemo.hostName,
                )}
              </span>
              {selected.updated_by && (
                <span className="muted small">
                  {t.sharedMemo.updatedBy(selected.updated_by)}
                </span>
              )}
              <span className="muted small">
                {t.memo.updatedAt(formatDate(selected.updated_at))}
              </span>
              <span className="muted small">rev {selected.revision}</span>
              {editing && <span className="muted small">{stats}</span>}
            </div>
          </div>
        )}
      </section>
    </div>
  );
}

function ModeBtn({
  label,
  value,
  mode,
  onMode,
}: {
  label: string;
  value: "edit" | "preview" | "split";
  mode: string;
  onMode: (mode: "edit" | "preview" | "split") => void;
}) {
  return (
    <button
      type="button"
      className={mode === value ? "memo__mode memo__mode--active" : "memo__mode"}
      onClick={() => onMode(value)}
    >
      {label}
    </button>
  );
}

function SharedFolderList({
  folders,
  selected,
  isHost,
  onSelect,
  onRun,
  onNotice,
}: {
  folders: MemoFolder[];
  selected: string | null;
  isHost: boolean;
  onSelect: (id: string | null) => void;
  onRun: (op: SharedMemoOp, done?: () => void) => Promise<void>;
  onNotice: (message: string) => void;
}) {
  const [adding, setAdding] = useState(false);
  const [name, setName] = useState("");
  return (
    <div className="memo__folders">
      <div className="memo__folders-head muted small">
        {t.sharedMemo.folders}
        {isHost && (
          <button
            type="button"
            className="button--icon"
            title={t.memo.newFolder}
            onClick={() => setAdding(!adding)}
          >
            ＋
          </button>
        )}
      </div>
      {adding && (
        <form
          className="memo__folder-add"
          onSubmit={(event) => {
            event.preventDefault();
            void onRun({ op: "folder_create", name }, () => {
              setName("");
              setAdding(false);
            });
          }}
        >
          <input
            value={name}
            autoFocus
            placeholder={t.memo.folderNamePlaceholder}
            onChange={(event) => setName(event.target.value)}
          />
          <button type="submit" disabled={!name.trim()}>
            {t.common.add}
          </button>
        </form>
      )}
      <button
        type="button"
        className={
          selected === null ? "memo__folder memo__folder--active" : "memo__folder"
        }
        onClick={() => onSelect(null)}
      >
        {t.memo.allMemos}
      </button>
      {folders.map((folder) => (
        <div key={folder.id} className="memo__folder-row">
          <button
            type="button"
            className={
              selected === folder.id
                ? "memo__folder memo__folder--active"
                : "memo__folder"
            }
            onClick={() => onSelect(selected === folder.id ? null : folder.id)}
          >
            📁 {folder.name}
            {folder.memo_count > 0 && (
              <span className="muted small"> {folder.memo_count}</span>
            )}
          </button>
          {isHost && (
            <>
              <button
                type="button"
                className="button--icon memo__folder-action"
                title={t.memo.renameFolder}
                onClick={() => {
                  const next = window.prompt(t.memo.renamePrompt, folder.name);
                  if (next === null || !next.trim()) return;
                  void onRun({
                    op: "folder_rename",
                    id: folder.id,
                    name: next,
                  }).catch((error) => onNotice(errorMessage(error)));
                }}
              >
                ✎
              </button>
              <button
                type="button"
                className="button--icon memo__folder-action"
                title={t.memo.deleteFolder}
                onClick={() => {
                  if (!window.confirm(t.memo.folderDeleteConfirm(folder.name))) {
                    return;
                  }
                  void onRun({ op: "folder_delete", id: folder.id }, () => {
                    if (selected === folder.id) onSelect(null);
                  });
                }}
              >
                🗑
              </button>
            </>
          )}
        </div>
      ))}
    </div>
  );
}

/** 権限設定(所有者・ホスト管理者)。全体レベル + メンバー個別の上書き。 */
function PermsDialog({
  memo,
  members,
  onClose,
  onSave,
}: {
  memo: SharedMemoDetail;
  members: Member[];
  onClose: () => void;
  onSave: (everyone: SharedPermLevel, members: SharedMemberPerm[]) => void;
}) {
  const [everyone, setEveryone] = useState<SharedPermLevel>(
    memo.everyone ?? "viewer",
  );
  // member_id → 個別レベル("inherit" = 全体に従う)
  const [overrides, setOverrides] = useState<Map<string, SharedPermLevel>>(
    () => new Map((memo.members ?? []).map((perm) => [perm.member_id, perm.level])),
  );

  const candidates = members.filter(
    (member) => !member.isHost && member.memberId !== null,
  );
  const nameOf = (memberId: string) =>
    candidates.find((member) => member.memberId === memberId)?.name ??
    (memo.members ?? []).find((perm) => perm.member_id === memberId)?.name ??
    memberId;

  return (
    <Modal title={t.sharedMemo.permsTitle} onClose={onClose}>
      <p className="muted small">{t.sharedMemo.permsNote}</p>
      <div className="field">
        <label>{t.sharedMemo.everyoneLabel}</label>
        <select
          value={everyone}
          onChange={(event) =>
            setEveryone(event.target.value as SharedPermLevel)
          }
        >
          <option value="viewer">{t.sharedMemo.levelViewer}</option>
          <option value="editor">{t.sharedMemo.levelEditor}</option>
          <option value="none">{t.sharedMemo.levelNone}</option>
        </select>
      </div>
      {candidates.length > 0 && (
        <div className="memo-perms__members">
          {candidates.map((member) => {
            const memberId = member.memberId!;
            const value = overrides.get(memberId) ?? "inherit";
            return (
              <div key={memberId} className="memo-perms__row">
                <span className="memo-perms__name">
                  {member.name ?? member.ip}
                  {member.isSelf && (
                    <span className="muted small">(自分)</span>
                  )}
                </span>
                <select
                  value={value}
                  onChange={(event) => {
                    const next = new Map(overrides);
                    if (event.target.value === "inherit") {
                      next.delete(memberId);
                    } else {
                      next.set(
                        memberId,
                        event.target.value as SharedPermLevel,
                      );
                    }
                    setOverrides(next);
                  }}
                >
                  <option value="inherit">{t.sharedMemo.levelInherit}</option>
                  <option value="viewer">{t.sharedMemo.levelViewer}</option>
                  <option value="editor">{t.sharedMemo.levelEditor}</option>
                  <option value="none">{t.sharedMemo.levelNone}</option>
                </select>
              </div>
            );
          })}
        </div>
      )}
      <div className="modal__actions">
        <button type="button" className="button--ghost" onClick={onClose}>
          {t.common.cancel}
        </button>
        <button
          type="button"
          onClick={() =>
            onSave(
              everyone,
              [...overrides.entries()].map(([memberId, level]) => ({
                member_id: memberId,
                name: nameOf(memberId) ?? "",
                level,
              })),
            )
          }
        >
          {t.common.save}
        </button>
      </div>
    </Modal>
  );
}

function formatDate(unixMs: number): string {
  const date = new Date(unixMs);
  const pad = (value: number) => String(value).padStart(2, "0");
  return `${date.getFullYear()}/${pad(date.getMonth() + 1)}/${pad(date.getDate())} ${pad(date.getHours())}:${pad(date.getMinutes())}`;
}
