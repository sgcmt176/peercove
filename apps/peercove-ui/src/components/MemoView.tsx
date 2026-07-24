// 個人メモ(M5 F-1、ADR-0049)。ネットワーク非依存の 2 ペイン構成:
// 左 = 検索・フォルダー・タグ・一覧、右 = エディタ(編集 / プレビュー / 分割)。
// 保存はデーモン所有の SQLite へ IPC 経由(自動保存はデバウンス 600ms)。
import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import {
  MemoDetail,
  MemoFolder,
  MemoPatch,
  MemoQuery,
  MemoScope,
  MemoSort,
  MemoSummary,
  MemoTagCount,
  api,
  errorMessage,
} from "../ipc";
import { t } from "../i18n";
import { Modal } from "./Modal";
import {
  useResolvedWikiLinks,
  wikiLinkify,
  wikiLinkTitle,
} from "../memoLinks";

type EditorMode = "edit" | "preview" | "split";
type SaveState = "saved" | "saving" | "error";

const AUTOSAVE_DELAY_MS = 600;

/** 共有メモへコピーできる先(接続中かつ共有メモが使えるネットワーク)。 */
export interface SharedMemoTarget {
  configPath: string;
  label: string;
}

export function MemoView({
  sharedTargets = [],
}: {
  sharedTargets?: SharedMemoTarget[];
}) {
  const [scope, setScope] = useState<MemoScope>("active");
  const [folderId, setFolderId] = useState<string | null>(null);
  const [tag, setTag] = useState<string | null>(null);
  const [search, setSearch] = useState("");
  const [sort, setSort] = useState<MemoSort>("updated");

  const [memos, setMemos] = useState<MemoSummary[]>([]);
  const [folders, setFolders] = useState<MemoFolder[]>([]);
  const [tags, setTags] = useState<MemoTagCount[]>([]);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);

  const [selected, setSelected] = useState<MemoDetail | null>(null);
  const [draft, setDraft] = useState<{ title: string; body: string }>({
    title: "",
    body: "",
  });
  // メモ間リンクのバックリンク欄(M5 F-5 Stage 2、ADR-0052 決定 2)。
  const [backlinks, setBacklinks] = useState<MemoSummary[]>([]);
  const [mode, setMode] = useState<EditorMode>("edit");
  const [saveState, setSaveState] = useState<SaveState>("saved");
  const [saveError, setSaveError] = useState("");

  const bodyRef = useRef<HTMLTextAreaElement | null>(null);
  // 自動保存の競合を避けるための「保存済みの内容」と選択中 ID(レンダー非依存)
  const savedRef = useRef<{ id: string; title: string; body: string } | null>(
    null,
  );
  const timerRef = useRef<number | null>(null);

  const query: MemoQuery = useMemo(
    () => ({
      scope,
      folder_id: scope === "trash" ? undefined : (folderId ?? undefined),
      tag: tag ?? undefined,
      search: search.trim() ? search.trim() : undefined,
      sort,
    }),
    [scope, folderId, tag, search, sort],
  );

  const refresh = useCallback(async () => {
    try {
      const reply = await api.memoOp({ op: "list", query });
      if (reply.kind === "memos") {
        setMemos(reply.memos);
        setFolders(reply.folders);
        setTags(reply.tags);
        setLoadError(null);
      }
    } catch (error) {
      setLoadError(errorMessage(error));
    }
  }, [query]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  // メモ間リンク(ADR-0052 決定 2): タイトル解決とバックリンク取得
  const resolveTitles = useCallback(async (titles: string[]) => {
    try {
      const reply = await api.memoOp({ op: "resolve_titles", titles });
      return reply.kind === "titles" ? reply.map : {};
    } catch {
      return {};
    }
  }, []);
  const resolvedTitles = useResolvedWikiLinks(draft.body, resolveTitles);

  const fetchBacklinks = useCallback(async (id: string) => {
    try {
      const reply = await api.memoOp({ op: "backlinks", id });
      setBacklinks(reply.kind === "memos" ? reply.memos : []);
    } catch {
      setBacklinks([]);
    }
  }, []);

  // 一時通知は数秒で消す
  useEffect(() => {
    if (notice === null) return;
    const timer = window.setTimeout(() => setNotice(null), 5000);
    return () => window.clearTimeout(timer);
  }, [notice]);

  /** 保留中の自動保存を今すぐ送る(選択切替・アンマウント時)。 */
  const flush = useCallback(async () => {
    if (timerRef.current !== null) {
      window.clearTimeout(timerRef.current);
      timerRef.current = null;
    }
    const saved = savedRef.current;
    if (saved === null) return;
    const pending = pendingRef.current;
    if (
      pending === null ||
      (pending.title === saved.title && pending.body === saved.body)
    ) {
      return;
    }
    try {
      await api.memoOp({
        op: "update",
        id: saved.id,
        patch: { title: pending.title, body: pending.body },
      });
      savedRef.current = { id: saved.id, ...pending };
    } catch {
      // 切替時の失敗は次の編集で再試行される(エディタ側の表示は既に切替済み)
    }
  }, []);

  // 最新の draft を flush から参照するための ref(state だと古い値を掴む)
  const pendingRef = useRef<{ title: string; body: string } | null>(null);
  pendingRef.current = selected ? draft : null;

  useEffect(() => () => void flush(), [flush]);

  /** メモを開く(直前の編集は flush してから)。 */
  const open = useCallback(
    async (id: string) => {
      await flush();
      try {
        const reply = await api.memoOp({ op: "get", id });
        if (reply.kind === "memo") {
          setSelected(reply.memo);
          setDraft({ title: reply.memo.title, body: reply.memo.body });
          savedRef.current = {
            id: reply.memo.id,
            title: reply.memo.title,
            body: reply.memo.body,
          };
          setSaveState("saved");
          setSaveError("");
          void fetchBacklinks(reply.memo.id);
        }
      } catch (error) {
        setNotice(errorMessage(error));
      }
    },
    [flush, fetchBacklinks],
  );

  // 自動保存(タイトル・本文のデバウンス)
  useEffect(() => {
    const saved = savedRef.current;
    if (
      selected === null ||
      saved === null ||
      saved.id !== selected.id ||
      (saved.title === draft.title && saved.body === draft.body)
    ) {
      return;
    }
    setSaveState("saving");
    const id = selected.id;
    const timer = window.setTimeout(() => {
      timerRef.current = null;
      void (async () => {
        try {
          const reply = await api.memoOp({
            op: "update",
            id,
            patch: { title: draft.title, body: draft.body },
          });
          if (reply.kind === "memo") {
            savedRef.current = {
              id,
              title: reply.memo.title,
              body: reply.memo.body,
            };
            setSaveState("saved");
            setSaveError("");
            void refresh();
            // タイトル変更でバックリンクの対象が変わりうる(ADR-0052 決定 2)
            void fetchBacklinks(id);
          }
        } catch (error) {
          setSaveState("error");
          setSaveError(errorMessage(error));
        }
      })();
    }, AUTOSAVE_DELAY_MS);
    timerRef.current = timer;
    return () => window.clearTimeout(timer);
  }, [draft, selected, refresh, fetchBacklinks]);

  /** 属性の即時更新(ピン留め・アーカイブ・フォルダー・タグ)。 */
  const patch = useCallback(
    async (id: string, patch: MemoPatch) => {
      try {
        const reply = await api.memoOp({ op: "update", id, patch });
        if (reply.kind === "memo") {
          setSelected((current) =>
            current?.id === id ? reply.memo : current,
          );
          if (savedRef.current?.id === id) {
            savedRef.current = {
              id,
              title: reply.memo.title,
              body: reply.memo.body,
            };
          }
        }
        void refresh();
      } catch (error) {
        setNotice(errorMessage(error));
      }
    },
    [refresh],
  );

  const createMemo = useCallback(async () => {
    await flush();
    try {
      const reply = await api.memoOp({
        op: "create",
        title: "",
        body: "",
        folder_id: scope !== "trash" ? (folderId ?? undefined) : undefined,
      });
      if (reply.kind === "memo") {
        setScope("active");
        setSelected(reply.memo);
        setDraft({ title: "", body: "" });
        savedRef.current = { id: reply.memo.id, title: "", body: "" };
        setSaveState("saved");
        setBacklinks([]);
        void refresh();
      }
    } catch (error) {
      setNotice(errorMessage(error));
    }
  }, [flush, folderId, scope, refresh]);

  const run = useCallback(
    async (op: Parameters<typeof api.memoOp>[0], done?: () => void) => {
      try {
        await api.memoOp(op);
        done?.();
        void refresh();
      } catch (error) {
        setNotice(errorMessage(error));
      }
    },
    [refresh],
  );

  const closeSelected = useCallback(() => {
    setSelected(null);
    savedRef.current = null;
    setBacklinks([]);
  }, []);

  const [chooseSharedTarget, setChooseSharedTarget] = useState(false);

  const copyToShared = useCallback(
    async (target: SharedMemoTarget) => {
      if (!selected) return;
      try {
        await api.sharedMemoOp(target.configPath, {
          op: "create",
          title: selected.title,
          body: selected.body,
        });
        setNotice(t.memo.copiedToShared);
      } catch (error) {
        setNotice(errorMessage(error));
      }
    },
    [selected],
  );

  const onCopyToShared = useCallback(() => {
    if (sharedTargets.length === 0) return;
    if (sharedTargets.length === 1) {
      void copyToShared(sharedTargets[0]);
    } else {
      setChooseSharedTarget(true);
    }
  }, [sharedTargets, copyToShared]);

  const importTxt = useCallback(async () => {
    try {
      const count = await api.memoImport(folderId);
      if (count !== null) {
        setNotice(t.memo.imported(count));
        void refresh();
      }
    } catch (error) {
      setNotice(errorMessage(error));
    }
  }, [folderId, refresh]);

  if (loadError !== null) {
    return (
      <section className="card card--error">
        <h2>{t.memo.title}</h2>
        <p>{t.memo.loadFailed}</p>
        <pre className="error-detail">{loadError}</pre>
        <button type="button" onClick={() => void refresh()}>
          {t.common.retry}
        </button>
      </section>
    );
  }

  return (
    <div className="memo">
      {chooseSharedTarget && (
        <Modal
          title={t.memo.copyToSharedChoose}
          onClose={() => setChooseSharedTarget(false)}
        >
          <ul className="memo__list">
            {sharedTargets.map((target) => (
              <li key={target.configPath}>
                <button
                  type="button"
                  className="memo__item"
                  onClick={() => {
                    setChooseSharedTarget(false);
                    void copyToShared(target);
                  }}
                >
                  <span className="memo__item-title">{target.label}</span>
                </button>
              </li>
            ))}
          </ul>
        </Modal>
      )}
      <aside className="memo__side card">
        <div className="memo__side-head">
          <button type="button" onClick={() => void createMemo()}>
            ＋ {t.memo.newMemo}
          </button>
          <button
            type="button"
            className="button--ghost"
            title={t.memo.importNote}
            onClick={() => void importTxt()}
          >
            {t.memo.import}
          </button>
        </div>
        <input
          type="search"
          value={search}
          placeholder={t.memo.searchPlaceholder}
          onChange={(event) => setSearch(event.target.value)}
        />
        <div className="memo__filters">
          <ScopeTab
            label={t.memo.scopeActive}
            active={scope === "active"}
            onClick={() => setScope("active")}
          />
          <ScopeTab
            label={t.memo.scopeArchived}
            active={scope === "archived"}
            onClick={() => setScope("archived")}
          />
          <ScopeTab
            label={t.memo.scopeTrash}
            active={scope === "trash"}
            onClick={() => setScope("trash")}
          />
          <select
            value={sort}
            title={t.memo.sortLabel}
            onChange={(event) => setSort(event.target.value as MemoSort)}
          >
            <option value="updated">{t.memo.sortUpdated}</option>
            <option value="created">{t.memo.sortCreated}</option>
            <option value="title">{t.memo.sortTitle}</option>
          </select>
        </div>
        {scope !== "trash" && (
          <FolderList
            folders={folders}
            selected={folderId}
            onSelect={setFolderId}
            onChanged={refresh}
            onNotice={setNotice}
          />
        )}
        {tags.length > 0 && scope !== "trash" && (
          <div className="memo__tags">
            {tags.map((entry) => (
              <button
                key={entry.tag}
                type="button"
                className={
                  tag === entry.tag ? "tag tag--active" : "tag"
                }
                onClick={() => setTag(tag === entry.tag ? null : entry.tag)}
              >
                #{entry.tag} {entry.count}
              </button>
            ))}
          </div>
        )}
        {scope === "trash" && memos.length > 0 && (
          <button
            type="button"
            className="button--ghost button--ghost-danger"
            onClick={() => {
              if (window.confirm(t.memo.emptyTrashConfirm)) {
                if (selected?.deleted_at) closeSelected();
                void run({ op: "empty_trash" });
              }
            }}
          >
            {t.memo.emptyTrashAction}
          </button>
        )}
        <ul className="memo__list">
          {memos.length === 0 && (
            <li className="muted small memo__empty">
              {scope === "trash" ? t.memo.emptyTrash : t.memo.empty}
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
                  {memo.pinned && <span aria-hidden>📌 </span>}
                  {memo.title || t.memo.untitled}
                </span>
                {memo.excerpt && (
                  <span className="memo__item-excerpt muted small">
                    {memo.excerpt}
                  </span>
                )}
                <span className="memo__item-meta muted small">
                  {formatDate(memo.deleted_at ?? memo.updated_at)}
                  {(memo.checklist_total ?? 0) > 0 && (
                    <>
                      {" ・☑ "}
                      {memo.checklist_done ?? 0}/{memo.checklist_total}
                    </>
                  )}
                  {(memo.tags ?? []).map((tagName) => (
                    <span key={tagName} className="memo__item-tag">
                      #{tagName}
                    </span>
                  ))}
                </span>
              </button>
            </li>
          ))}
        </ul>
      </aside>

      <section className="memo__editor card">
        {notice && <p className="memo__notice small">{notice}</p>}
        {selected === null ? (
          <p className="muted memo__placeholder">{t.memo.selectPrompt}</p>
        ) : (
          <Editor
            key={selected.id}
            memo={selected}
            draft={draft}
            folders={folders}
            mode={mode}
            saveState={saveState}
            saveError={saveError}
            bodyRef={bodyRef}
            resolvedTitles={resolvedTitles}
            backlinks={backlinks}
            onWikiLink={(id) => void open(id)}
            onWikiLinkMissing={() => setNotice(t.memo.wikilinkMissing)}
            onMode={setMode}
            onDraft={setDraft}
            onPatch={(update) => void patch(selected.id, update)}
            onDuplicate={() => void run({ op: "duplicate", id: selected.id })}
            onCopyToShared={sharedTargets.length > 0 ? onCopyToShared : undefined}
            onTrash={() =>
              void run({ op: "trash", id: selected.id }, closeSelected)
            }
            onRestore={() =>
              void run({ op: "restore", id: selected.id }, closeSelected)
            }
            onDeleteForever={() => {
              if (window.confirm(t.memo.deleteForeverConfirm)) {
                void run({ op: "delete_forever", id: selected.id }, closeSelected);
              }
            }}
            onExport={() =>
              void api
                .memoExport(selected.id)
                .then((path) => {
                  if (path !== null) setNotice(t.memo.exported(path));
                })
                .catch((error) => setNotice(errorMessage(error)))
            }
          />
        )}
      </section>
    </div>
  );
}

function ScopeTab({
  label,
  active,
  onClick,
}: {
  label: string;
  active: boolean;
  onClick: () => void;
}) {
  return (
    <button
      type="button"
      className={active ? "memo__scope memo__scope--active" : "memo__scope"}
      onClick={onClick}
    >
      {label}
    </button>
  );
}

function FolderList({
  folders,
  selected,
  onSelect,
  onChanged,
  onNotice,
}: {
  folders: MemoFolder[];
  selected: string | null;
  onSelect: (id: string | null) => void;
  onChanged: () => Promise<void> | void;
  onNotice: (message: string) => void;
}) {
  const [adding, setAdding] = useState(false);
  const [name, setName] = useState("");

  const create = async () => {
    try {
      await api.memoOp({ op: "folder_create", name });
      setName("");
      setAdding(false);
      void onChanged();
    } catch (error) {
      onNotice(errorMessage(error));
    }
  };

  return (
    <div className="memo__folders">
      <div className="memo__folders-head muted small">
        {t.memo.folders}
        <button
          type="button"
          className="button--icon"
          title={t.memo.newFolder}
          onClick={() => setAdding(!adding)}
        >
          ＋
        </button>
      </div>
      {adding && (
        <form
          className="memo__folder-add"
          onSubmit={(event) => {
            event.preventDefault();
            void create();
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
            <span className="muted small"> {folder.memo_count}</span>
          </button>
          <button
            type="button"
            className="button--icon memo__folder-action"
            title={t.memo.renameFolder}
            onClick={() => {
              const next = window.prompt(t.memo.renamePrompt, folder.name);
              if (next === null || !next.trim()) return;
              void api
                .memoOp({ op: "folder_rename", id: folder.id, name: next })
                .then(() => onChanged())
                .catch((error) => onNotice(errorMessage(error)));
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
              void api
                .memoOp({ op: "folder_delete", id: folder.id })
                .then(() => {
                  if (selected === folder.id) onSelect(null);
                  return onChanged();
                })
                .catch((error) => onNotice(errorMessage(error)));
            }}
          >
            🗑
          </button>
        </div>
      ))}
    </div>
  );
}

function Editor({
  memo,
  draft,
  folders,
  mode,
  saveState,
  saveError,
  bodyRef,
  resolvedTitles,
  backlinks,
  onWikiLink,
  onWikiLinkMissing,
  onMode,
  onDraft,
  onPatch,
  onDuplicate,
  onCopyToShared,
  onTrash,
  onRestore,
  onDeleteForever,
  onExport,
}: {
  memo: MemoDetail;
  draft: { title: string; body: string };
  folders: MemoFolder[];
  mode: EditorMode;
  saveState: SaveState;
  saveError: string;
  bodyRef: React.MutableRefObject<HTMLTextAreaElement | null>;
  /** メモ間リンク `[[タイトル]]` の解決結果(タイトル → memo_id、ADR-0052)。 */
  resolvedTitles: Record<string, string>;
  /** このメモへのバックリンク一覧(0 件なら欄自体を出さない)。 */
  backlinks: MemoSummary[];
  onWikiLink: (id: string) => void;
  onWikiLinkMissing: () => void;
  onMode: (mode: EditorMode) => void;
  onDraft: (draft: { title: string; body: string }) => void;
  onPatch: (patch: MemoPatch) => void;
  onDuplicate: () => void;
  /** 共有メモへコピー(M5 F-3)。接続中で共有メモが使えるネットワークが無ければ undefined。 */
  onCopyToShared?: () => void;
  onTrash: () => void;
  onRestore: () => void;
  onDeleteForever: () => void;
  onExport: () => void;
}) {
  const inTrash = memo.deleted_at !== undefined && memo.deleted_at !== null;
  const [tagsInput, setTagsInput] = useState((memo.tags ?? []).join(", "));

  /** カーソル位置に Markdown 記法を差し込む(選択があれば囲む)。 */
  const insert = (before: string, after = "", block = false) => {
    const area = bodyRef.current;
    if (area === null) return;
    const start = area.selectionStart;
    const end = area.selectionEnd;
    const value = draft.body;
    let prefix = before;
    if (block && start > 0 && value[start - 1] !== "\n") {
      prefix = `\n${before}`;
    }
    const selectedText = value.slice(start, end);
    const next =
      value.slice(0, start) + prefix + selectedText + after + value.slice(end);
    onDraft({ ...draft, body: next });
    // 差し込んだ位置へカーソルを戻す(レンダー後)
    const cursor = start + prefix.length + selectedText.length;
    requestAnimationFrame(() => {
      area.focus();
      area.setSelectionRange(
        selectedText ? start + prefix.length : cursor,
        cursor,
      );
    });
  };

  /** プレビューのチェックボックス操作 → 本文の該当行の `[ ]`⇔`[x]` を反転。 */
  const toggleChecklistLine = (line: number) => {
    const lines = draft.body.split("\n");
    const index = line - 1;
    if (index < 0 || index >= lines.length) return;
    const current = lines[index];
    if (/\[[ ]\]/.test(current)) {
      lines[index] = current.replace("[ ]", "[x]");
    } else if (/\[[xX]\]/.test(current)) {
      lines[index] = current.replace(/\[[xX]\]/, "[ ]");
    } else {
      return;
    }
    onDraft({ ...draft, body: lines.join("\n") });
  };

  const stats = useMemo(() => {
    const chars = [...draft.body].length;
    const lines = draft.body === "" ? 0 : draft.body.split("\n").length;
    return t.memo.stats(chars, lines);
  }, [draft.body]);

  return (
    <div className="memo__editor-inner">
      <div className="memo__toolbar">
        <input
          className="memo__title"
          value={draft.title}
          placeholder={t.memo.titlePlaceholder}
          readOnly={inTrash}
          onChange={(event) => onDraft({ ...draft, title: event.target.value })}
        />
        <span
          className={
            saveState === "error"
              ? "memo__save memo__save--error small"
              : "memo__save muted small"
          }
          title={saveState === "error" ? saveError : undefined}
        >
          {inTrash
            ? t.memo.inTrash
            : saveState === "saving"
              ? t.memo.saving
              : saveState === "error"
                ? t.memo.saveFailed
                : t.memo.saved}
        </span>
      </div>

      <div className="memo__actions">
        {!inTrash && (
          <>
            <ModeButton mode="edit" current={mode} onMode={onMode} />
            <ModeButton mode="split" current={mode} onMode={onMode} />
            <ModeButton mode="preview" current={mode} onMode={onMode} />
            <span className="memo__actions-sep" />
            <button
              type="button"
              className="button--icon"
              title={memo.pinned ? t.memo.unpin : t.memo.pin}
              onClick={() => onPatch({ pinned: !memo.pinned })}
            >
              {memo.pinned ? "📌" : "📍"}
            </button>
            <button
              type="button"
              className="button--icon"
              title={memo.archived ? t.memo.unarchive : t.memo.archive}
              onClick={() => onPatch({ archived: !memo.archived })}
            >
              🗄
            </button>
            <button
              type="button"
              className="button--icon"
              title={t.memo.duplicate}
              onClick={onDuplicate}
            >
              ⧉
            </button>
            {onCopyToShared && (
              <button
                type="button"
                className="button--icon"
                title={t.memo.copyToShared}
                onClick={onCopyToShared}
              >
                📤
              </button>
            )}
            <button
              type="button"
              className="button--icon"
              title={t.memo.exportNote}
              onClick={onExport}
            >
              💾
            </button>
            <button
              type="button"
              className="button--icon"
              title={t.memo.toTrash}
              onClick={onTrash}
            >
              🗑
            </button>
            <select
              className="memo__folder-select"
              value={memo.folder_id ?? ""}
              title={t.memo.folderLabel}
              onChange={(event) =>
                onPatch({
                  folder: event.target.value
                    ? { id: event.target.value }
                    : {},
                })
              }
            >
              <option value="">{t.memo.noFolder}</option>
              {folders.map((folder) => (
                <option key={folder.id} value={folder.id}>
                  📁 {folder.name}
                </option>
              ))}
            </select>
          </>
        )}
        {inTrash && (
          <>
            <button type="button" onClick={onRestore}>
              {t.memo.restore}
            </button>
            <button
              type="button"
              className="button--ghost button--ghost-danger"
              onClick={onDeleteForever}
            >
              {t.memo.deleteForever}
            </button>
          </>
        )}
      </div>

      {!inTrash && mode !== "preview" && (
        <div className="memo__format">
          <FormatButton label={t.memo.fmtHeading} text="Ｈ" onClick={() => insert("## ", "", true)} />
          <FormatButton label={t.memo.fmtBold} text="Ｂ" onClick={() => insert("**", "**")} />
          <FormatButton label={t.memo.fmtItalic} text="Ｉ" onClick={() => insert("*", "*")} />
          <FormatButton label={t.memo.fmtStrike} text="Ｓ̶" onClick={() => insert("~~", "~~")} />
          <FormatButton label={t.memo.fmtList} text="•" onClick={() => insert("- ", "", true)} />
          <FormatButton label={t.memo.fmtCheck} text="☑" onClick={() => insert("- [ ] ", "", true)} />
          <FormatButton label={t.memo.fmtQuote} text="❝" onClick={() => insert("> ", "", true)} />
          <FormatButton label={t.memo.fmtCode} text="</>" onClick={() => insert("`", "`")} />
          <FormatButton
            label={t.memo.fmtCodeBlock}
            text="{ }"
            onClick={() => insert("```\n", "\n```", true)}
          />
          <FormatButton
            label={t.memo.fmtTable}
            text="▦"
            onClick={() =>
              insert("| 列1 | 列2 |\n| --- | --- |\n|  |  |\n", "", true)
            }
          />
          <FormatButton label={t.memo.fmtLink} text="🔗" onClick={() => insert("[", "](https://)")} />
          <FormatButton label={t.memo.fmtHr} text="―" onClick={() => insert("---\n", "", true)} />
        </div>
      )}

      <div
        className={
          mode === "split" && !inTrash
            ? "memo__panes memo__panes--split"
            : "memo__panes"
        }
      >
        {!inTrash && mode !== "preview" && (
          <textarea
            ref={bodyRef}
            className="memo__body"
            value={draft.body}
            placeholder={t.memo.bodyPlaceholder}
            spellCheck={false}
            onChange={(event) =>
              onDraft({ ...draft, body: event.target.value })
            }
          />
        )}
        {(inTrash || mode !== "edit") && (
          <div className="memo__preview markdown">
            <ReactMarkdown
              remarkPlugins={[remarkGfm]}
              components={{
                a: ({ href, children }) => {
                  const wikiTitle = wikiLinkTitle(href);
                  if (wikiTitle !== null) {
                    const targetId = resolvedTitles[wikiTitle];
                    return (
                      <a
                        href={href}
                        className={
                          targetId
                            ? "memo__wikilink"
                            : "memo__wikilink memo__wikilink--missing"
                        }
                        title={targetId ? undefined : t.memo.wikilinkMissing}
                        onClick={(event) => {
                          event.preventDefault();
                          if (targetId) {
                            onWikiLink(targetId);
                          } else {
                            onWikiLinkMissing();
                          }
                        }}
                      >
                        {children}
                      </a>
                    );
                  }
                  // それ以外は Tauri の WebView 内で遷移させず既定ブラウザで開く
                  return (
                    <a
                      href={href}
                      onClick={(event) => {
                        event.preventDefault();
                        if (href) void api.openLink(href);
                      }}
                    >
                      {children}
                    </a>
                  );
                },
                // チェックリストはプレビューから直接オン・オフできる
                input: ({ node, checked }) => {
                  const line = node?.position?.start.line;
                  return (
                    <input
                      type="checkbox"
                      checked={Boolean(checked)}
                      disabled={inTrash || line === undefined}
                      onChange={() => {
                        if (line !== undefined) toggleChecklistLine(line);
                      }}
                    />
                  );
                },
              }}
            >
              {wikiLinkify(draft.body)}
            </ReactMarkdown>
          </div>
        )}
      </div>

      {!inTrash && (
        <div className="memo__meta">
          <input
            className="memo__tags-input"
            value={tagsInput}
            placeholder={t.memo.tagsPlaceholder}
            onChange={(event) => setTagsInput(event.target.value)}
            onBlur={() =>
              onPatch({
                tags: tagsInput
                  .split(/[,、]/)
                  .map((entry) => entry.trim())
                  .filter((entry) => entry.length > 0),
              })
            }
          />
          <span className="muted small">{stats}</span>
          <span className="muted small">
            {t.memo.updatedAt(formatDate(memo.updated_at))}
          </span>
        </div>
      )}

      {backlinks.length > 0 && (
        <BacklinksSection backlinks={backlinks} onOpen={onWikiLink} />
      )}
    </div>
  );
}

/** バックリンク欄(ADR-0052 決定 2)。折りたたみ可能、0 件なら呼び出し元が出さない。 */
function BacklinksSection({
  backlinks,
  onOpen,
}: {
  backlinks: MemoSummary[];
  onOpen: (id: string) => void;
}) {
  return (
    <details className="memo__backlinks" open>
      <summary>{t.memo.backlinksTitle(backlinks.length)}</summary>
      <ul className="memo__backlinks-list">
        {backlinks.map((memo) => (
          <li key={memo.id}>
            <button
              type="button"
              className="memo__backlinks-item"
              onClick={() => onOpen(memo.id)}
            >
              {memo.title || t.memo.untitled}
            </button>
          </li>
        ))}
      </ul>
    </details>
  );
}

function ModeButton({
  mode,
  current,
  onMode,
}: {
  mode: EditorMode;
  current: EditorMode;
  onMode: (mode: EditorMode) => void;
}) {
  const label =
    mode === "edit"
      ? t.memo.modeEdit
      : mode === "preview"
        ? t.memo.modePreview
        : t.memo.modeSplit;
  return (
    <button
      type="button"
      className={current === mode ? "memo__mode memo__mode--active" : "memo__mode"}
      onClick={() => onMode(mode)}
    >
      {label}
    </button>
  );
}

function FormatButton({
  label,
  text,
  onClick,
}: {
  label: string;
  text: string;
  onClick: () => void;
}) {
  return (
    <button
      type="button"
      className="button--icon memo__fmt"
      title={label}
      onClick={onClick}
    >
      {text}
    </button>
  );
}

function formatDate(unixMs: number): string {
  const date = new Date(unixMs);
  const pad = (value: number) => String(value).padStart(2, "0");
  return `${date.getFullYear()}/${pad(date.getMonth() + 1)}/${pad(date.getDate())} ${pad(date.getHours())}:${pad(date.getMinutes())}`;
}
