// 共有シート(Excel ライク表、M6 G-2、ADR-0054)。共有メモ・共有スケジュール表
// (ScheduleView.tsx)の基盤(ホスト正本 DB・コントロールチャネル配信・読み取り
// キャッシュ)を転用する。閲覧・セル編集は全員、シートの作成・改名・削除は
// 作成者 + ホストだけ(`can_manage` で判定)。競合はセル単位の revision CAS
// (ADR-0054 決定 4)。**シート名・セル値は console に出さないこと。**
import {
  KeyboardEvent,
  ClipboardEvent,
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import { CellWrite, SheetCell, SheetMeta, SheetOp, api, errorMessage } from "../ipc";
import { t } from "../i18n";
import { sharedRefToken } from "../sharedRefs";

// crates/peercove-core/src/sheet.rs の上限と同期(ADR-0054 決定 7)。
const MAX_SHEET_ROWS = 1000;
const MAX_SHEET_COLS = 200;

const MIN_DISPLAY_ROWS = 20;
const MIN_DISPLAY_COLS = 8;
const DISPLAY_MARGIN = 2;

const NUMERIC_RE = /^-?\d+(\.\d+)?$/;

function keyOf(row: number, col: number): string {
  return `${row},${col}`;
}

/** 0-indexed 列番号 → A, B, ..., Z, AA, AB, ... */
function colLabel(n: number): string {
  let label = "";
  let x = n + 1;
  while (x > 0) {
    const rem = (x - 1) % 26;
    label = String.fromCharCode(65 + rem) + label;
    x = Math.floor((x - 1) / 26);
  }
  return label;
}

interface EditingState {
  row: number;
  col: number;
  value: string;
}

interface Selected {
  row: number;
  col: number;
}

export function SheetView({
  configPath,
  isHost,
  supported,
  seq,
  focusSheetId,
  onFocusConsumed,
}: {
  configPath: string;
  isHost: boolean;
  /** 共有メモ(相乗り)が使える状態か(member で false = ホスト未対応)。 */
  supported: boolean;
  /** 変更世代。進んだら再取得する。 */
  seq: number;
  /** チャットの `@sheet:id` カード(ADR-0054)から開くシート。 */
  focusSheetId?: string | null;
  onFocusConsumed?: () => void;
}) {
  const [sheets, setSheets] = useState<SheetMeta[]>([]);
  const [sheetsLoaded, setSheetsLoaded] = useState(false);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [activeSheetId, setActiveSheetId] = useState<string | null>(null);
  const [cells, setCells] = useState<SheetCell[]>([]);
  const [offline, setOffline] = useState(false);
  const [notice, setNotice] = useState<string | null>(null);
  const [menuFor, setMenuFor] = useState<string | null>(null);
  const [selected, setSelected] = useState<Selected | null>(null);
  const [editing, setEditing] = useState<EditingState | null>(null);
  const [conflictHighlight, setConflictHighlight] = useState<Set<string>>(
    new Set(),
  );

  const activeSheetIdRef = useRef<string | null>(null);
  const cellRefs = useRef<Map<string, HTMLTableCellElement>>(new Map());
  const inputRef = useRef<HTMLInputElement>(null);
  const skipNextBlurCommit = useRef(false);

  useEffect(() => {
    activeSheetIdRef.current = activeSheetId;
  }, [activeSheetId]);

  const sheetOp = useCallback(
    async (op: SheetOp) => {
      const reply = await api.sharedMemoOp(configPath, { op: "sheet", sheet: op });
      if (reply.kind !== "sheet") {
        throw new Error(`想定外の応答です: ${reply.kind}`);
      }
      return reply.reply;
    },
    [configPath],
  );

  const loadSheets = useCallback(async () => {
    try {
      const reply = await sheetOp({ op: "list" });
      if (reply.kind === "sheets") {
        setSheets(reply.sheets);
        setOffline(reply.offline ?? false);
        setLoadError(null);
      }
    } catch (error) {
      setLoadError(errorMessage(error));
    } finally {
      setSheetsLoaded(true);
    }
  }, [sheetOp]);

  const loadCells = useCallback(
    async (sheetId: string) => {
      try {
        const reply = await sheetOp({ op: "cells", sheet_id: sheetId });
        if (reply.kind === "cells_data" && activeSheetIdRef.current === sheetId) {
          setCells(reply.cells);
          setOffline(reply.offline ?? false);
        }
      } catch (error) {
        setNotice(errorMessage(error));
      }
    },
    [sheetOp],
  );

  useEffect(() => {
    void loadSheets();
    // seq(共有メモの変更世代)が進むたびに再取得 = リアルタイム反映
  }, [loadSheets, seq]);

  useEffect(() => {
    if (!activeSheetId) {
      setCells([]);
      return;
    }
    void loadCells(activeSheetId);
  }, [activeSheetId, seq, loadCells]);

  // シート一覧が変わったとき、選択中のシートが消えていたら先頭へ差し替える
  useEffect(() => {
    if (!sheetsLoaded) return;
    if (activeSheetId && sheets.some((s) => s.id === activeSheetId)) return;
    setActiveSheetId(sheets[0]?.id ?? null);
    setSelected(null);
    setEditing(null);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [sheets, sheetsLoaded]);

  // チャットの `@sheet:id` カードから開く
  useEffect(() => {
    if (!focusSheetId || !sheetsLoaded) return;
    if (sheets.some((s) => s.id === focusSheetId)) {
      setActiveSheetId(focusSheetId);
      setSelected(null);
      setEditing(null);
    }
    onFocusConsumed?.();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [focusSheetId, sheetsLoaded]);

  useEffect(() => {
    if (notice === null) return;
    const timer = window.setTimeout(() => setNotice(null), 6000);
    return () => window.clearTimeout(timer);
  }, [notice]);

  // 選択セルへフォーカスを追従させる(矢印キー操作を継続できるように)
  useEffect(() => {
    if (!selected || editing) return;
    const el = cellRefs.current.get(keyOf(selected.row, selected.col));
    el?.focus();
  }, [selected, editing]);

  // 編集開始時だけ input へフォーカス + 全選択(打鍵のたびには再実行しない)
  useEffect(() => {
    if (!editing) return;
    inputRef.current?.focus();
    inputRef.current?.select();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [editing?.row, editing?.col]);

  const readOnlyReason = offline
    ? t.sheet.offline
    : !supported && !isHost
      ? t.sheet.unsupported
      : null;

  const activeSheet = sheets.find((s) => s.id === activeSheetId) ?? null;

  const cellsByKey = useMemo(() => {
    const map = new Map<string, SheetCell>();
    for (const cell of cells) map.set(keyOf(cell.row, cell.col), cell);
    return map;
  }, [cells]);

  const { displayRows, displayCols } = useMemo(() => {
    let usedRows = 0;
    let usedCols = 0;
    for (const cell of cells) {
      usedRows = Math.max(usedRows, cell.row + 1);
      usedCols = Math.max(usedCols, cell.col + 1);
    }
    return {
      displayRows: Math.min(
        Math.max(usedRows + DISPLAY_MARGIN, MIN_DISPLAY_ROWS),
        MAX_SHEET_ROWS,
      ),
      displayCols: Math.min(
        Math.max(usedCols + DISPLAY_MARGIN, MIN_DISPLAY_COLS),
        MAX_SHEET_COLS,
      ),
    };
  }, [cells]);

  const applyConflicts = useCallback((conflicts: SheetCell[]) => {
    setCells((prev) => {
      const map = new Map(prev.map((c) => [keyOf(c.row, c.col), c] as const));
      for (const c of conflicts) {
        if (c.value === "") map.delete(keyOf(c.row, c.col));
        else map.set(keyOf(c.row, c.col), c);
      }
      return Array.from(map.values());
    });
    const keys = conflicts.map((c) => keyOf(c.row, c.col));
    setConflictHighlight((prev) => new Set([...prev, ...keys]));
    window.setTimeout(() => {
      setConflictHighlight((prev) => {
        const next = new Set(prev);
        for (const k of keys) next.delete(k);
        return next;
      });
    }, 4000);
  }, []);

  const writeCells = useCallback(
    async (writes: CellWrite[]) => {
      if (!activeSheetId || writes.length === 0) return;
      try {
        const reply = await sheetOp({ op: "write", sheet_id: activeSheetId, cells: writes });
        if (reply.kind === "write_result") {
          if (reply.conflicts.length > 0) {
            applyConflicts(reply.conflicts);
            setNotice(t.sheet.conflictNotice(reply.conflicts.length));
          }
          void loadCells(activeSheetId);
        } else if (reply.kind === "err") {
          setNotice(reply.message);
        }
      } catch (error) {
        setNotice(errorMessage(error));
      }
    },
    [activeSheetId, sheetOp, applyConflicts, loadCells],
  );

  const startEdit = useCallback(
    (row: number, col: number, value: string) => {
      if (readOnlyReason) return;
      setSelected({ row, col });
      setEditing({ row, col, value });
    },
    [readOnlyReason],
  );

  const cancelEdit = useCallback(() => setEditing(null), []);

  const commitEdit = useCallback(
    (opts: { moveDown?: boolean; moveRight?: boolean } = {}) => {
      if (!editing) return;
      const { row, col, value } = editing;
      const current = cellsByKey.get(keyOf(row, col));
      const baseRevision = current?.revision ?? 0;
      setEditing(null);
      setSelected(
        opts.moveDown
          ? { row: Math.min(row + 1, displayRows - 1), col }
          : opts.moveRight
            ? { row, col: Math.min(col + 1, displayCols - 1) }
            : { row, col },
      );
      const unchanged = current ? current.value === value : value === "";
      if (!unchanged) {
        void writeCells([{ row, col, value, base_revision: baseRevision }]);
      }
    },
    [editing, cellsByKey, displayRows, displayCols, writeCells],
  );

  const moveSelection = useCallback(
    (direction: "up" | "down" | "left" | "right", row: number, col: number) => {
      let nr = row;
      let nc = col;
      if (direction === "up") nr = Math.max(0, row - 1);
      if (direction === "down") nr = Math.min(displayRows - 1, row + 1);
      if (direction === "left") nc = Math.max(0, col - 1);
      if (direction === "right") nc = Math.min(displayCols - 1, col + 1);
      setSelected({ row: nr, col: nc });
    },
    [displayRows, displayCols],
  );

  const handleGridKeyDown = useCallback(
    (event: KeyboardEvent<HTMLTableCellElement>, row: number, col: number) => {
      const isEditingThis = editing && editing.row === row && editing.col === col;
      if (isEditingThis) {
        if (event.key === "Enter") {
          event.preventDefault();
          skipNextBlurCommit.current = true;
          commitEdit({ moveDown: true });
        } else if (event.key === "Tab") {
          event.preventDefault();
          skipNextBlurCommit.current = true;
          commitEdit({ moveRight: true });
        } else if (event.key === "Escape") {
          event.preventDefault();
          skipNextBlurCommit.current = true;
          cancelEdit();
        }
        // それ以外のキー(文字入力・カーソル移動)は input の既定動作に任せる
        return;
      }
      const arrowMap: Record<string, "up" | "down" | "left" | "right"> = {
        ArrowUp: "up",
        ArrowDown: "down",
        ArrowLeft: "left",
        ArrowRight: "right",
      };
      if (event.key in arrowMap) {
        event.preventDefault();
        moveSelection(arrowMap[event.key], row, col);
        return;
      }
      if (readOnlyReason) return;
      if (event.key === "Enter") {
        event.preventDefault();
        startEdit(row, col, cellsByKey.get(keyOf(row, col))?.value ?? "");
        return;
      }
      if (event.key === "Delete" || event.key === "Backspace") {
        event.preventDefault();
        const current = cellsByKey.get(keyOf(row, col));
        if (current) {
          void writeCells([{ row, col, value: "", base_revision: current.revision }]);
        }
        return;
      }
      if (
        event.key.length === 1 &&
        !event.ctrlKey &&
        !event.metaKey &&
        !event.altKey
      ) {
        event.preventDefault();
        startEdit(row, col, event.key);
      }
    },
    [editing, readOnlyReason, cellsByKey, commitEdit, cancelEdit, moveSelection, startEdit, writeCells],
  );

  const handleGridBlur = useCallback(
    (row: number, col: number) => {
      if (!(editing && editing.row === row && editing.col === col)) return;
      if (skipNextBlurCommit.current) {
        skipNextBlurCommit.current = false;
        return;
      }
      commitEdit({});
    },
    [editing, commitEdit],
  );

  const handleGridCopy = useCallback(
    (event: ClipboardEvent<HTMLTableCellElement>, row: number, col: number) => {
      if (editing && editing.row === row && editing.col === col) return;
      event.preventDefault();
      const value = cellsByKey.get(keyOf(row, col))?.value ?? "";
      event.clipboardData.setData("text/plain", value);
    },
    [editing, cellsByKey],
  );

  const handleGridPaste = useCallback(
    (event: ClipboardEvent<HTMLTableCellElement>, row: number, col: number) => {
      if (editing && editing.row === row && editing.col === col) return;
      if (readOnlyReason) return;
      event.preventDefault();
      const text = event.clipboardData.getData("text/plain");
      if (!text) return;
      const lines = text.replace(/\r\n/g, "\n").replace(/\r/g, "\n").split("\n");
      while (lines.length > 1 && lines[lines.length - 1] === "") lines.pop();
      const grid = lines.map((line) => line.split("\t"));
      const rowSpan = grid.length;
      const colSpan = grid.reduce((max, line) => Math.max(max, line.length), 0);
      if (row + rowSpan > MAX_SHEET_ROWS || col + colSpan > MAX_SHEET_COLS) {
        setNotice(t.sheet.pasteRejected);
        return;
      }
      const writes: CellWrite[] = [];
      grid.forEach((line, r) => {
        line.forEach((value, c) => {
          const targetRow = row + r;
          const targetCol = col + c;
          const current = cellsByKey.get(keyOf(targetRow, targetCol));
          writes.push({
            row: targetRow,
            col: targetCol,
            value,
            base_revision: current?.revision ?? 0,
          });
        });
      });
      void writeCells(writes);
    },
    [editing, readOnlyReason, cellsByKey, writeCells],
  );

  const createSheet = useCallback(() => {
    const name = window.prompt(t.sheet.newSheetNamePrompt, t.sheet.newSheetDefaultName);
    if (name === null) return;
    const trimmed = name.trim();
    if (!trimmed) return;
    void (async () => {
      try {
        const reply = await sheetOp({ op: "create", name: trimmed });
        if (reply.kind === "sheet") {
          setActiveSheetId(reply.sheet.id);
          void loadSheets();
        } else if (reply.kind === "err") {
          setNotice(reply.message);
        }
      } catch (error) {
        setNotice(errorMessage(error));
      }
    })();
  }, [sheetOp, loadSheets]);

  const renameSheet = useCallback(
    (sheet: SheetMeta) => {
      setMenuFor(null);
      const name = window.prompt(t.sheet.renamePrompt, sheet.name);
      if (name === null) return;
      const trimmed = name.trim();
      if (!trimmed) return;
      void (async () => {
        try {
          const reply = await sheetOp({ op: "rename", sheet_id: sheet.id, name: trimmed });
          if (reply.kind === "sheet" || reply.kind === "done") {
            void loadSheets();
          } else if (reply.kind === "err") {
            setNotice(reply.message);
          }
        } catch (error) {
          setNotice(errorMessage(error));
        }
      })();
    },
    [sheetOp, loadSheets],
  );

  const deleteSheet = useCallback(
    (sheet: SheetMeta) => {
      setMenuFor(null);
      if (!window.confirm(t.sheet.deleteConfirm(sheet.name))) return;
      void (async () => {
        try {
          const reply = await sheetOp({ op: "delete", sheet_id: sheet.id });
          if (reply.kind === "done") {
            if (activeSheetId === sheet.id) setActiveSheetId(null);
            void loadSheets();
          } else if (reply.kind === "err") {
            setNotice(reply.message);
          }
        } catch (error) {
          setNotice(errorMessage(error));
        }
      })();
    },
    [sheetOp, loadSheets, activeSheetId],
  );

  const copySheetLink = useCallback((sheet: SheetMeta) => {
    setMenuFor(null);
    void writeText(sharedRefToken("sheet", sheet.id)).then(() =>
      setNotice(t.sheet.copyLinkDone),
    );
  }, []);

  // メニュー表示中に他所をクリックしたら閉じる
  useEffect(() => {
    if (!menuFor) return;
    const onDocClick = () => setMenuFor(null);
    document.addEventListener("click", onDocClick);
    return () => document.removeEventListener("click", onDocClick);
  }, [menuFor]);

  if (loadError !== null) {
    return (
      <section className="card card--error">
        <h2>{t.sheet.title}</h2>
        <p>{t.sheet.loadFailed}</p>
        <pre className="error-detail">{loadError}</pre>
        <button type="button" onClick={() => void loadSheets()}>
          {t.common.retry}
        </button>
      </section>
    );
  }

  const rowIndexes = Array.from({ length: displayRows }, (_, i) => i);
  const colIndexes = Array.from({ length: displayCols }, (_, i) => i);

  return (
    <div className="sheet">
      <div className="sheet__tabbar">
        {sheets.map((sheet) => (
          <div key={sheet.id} className="sheet__tab-wrap">
            <button
              type="button"
              className={
                sheet.id === activeSheetId
                  ? "sheet__tab sheet__tab--active"
                  : "sheet__tab"
              }
              onClick={() => {
                setActiveSheetId(sheet.id);
                setSelected(null);
                setEditing(null);
              }}
            >
              {sheet.name}
            </button>
            <button
              type="button"
              className="button--icon sheet__tab-menu-btn"
              title={t.sheet.moreOptions}
              onClick={(event) => {
                event.stopPropagation();
                setMenuFor(menuFor === sheet.id ? null : sheet.id);
              }}
            >
              ⋮
            </button>
            {menuFor === sheet.id && (
              <div className="sheet__tab-menu" onClick={(event) => event.stopPropagation()}>
                <button type="button" onClick={() => copySheetLink(sheet)}>
                  🔗 {t.sheet.copyLink}
                </button>
                {sheet.can_manage && (
                  <>
                    <button type="button" onClick={() => renameSheet(sheet)}>
                      ✏ {t.sheet.renameSheet}
                    </button>
                    <button
                      type="button"
                      className="sheet__tab-menu-danger"
                      onClick={() => deleteSheet(sheet)}
                    >
                      🗑 {t.sheet.deleteSheet}
                    </button>
                  </>
                )}
              </div>
            )}
          </div>
        ))}
        <button
          type="button"
          className="button--icon"
          title={t.sheet.addSheet}
          disabled={readOnlyReason !== null}
          onClick={createSheet}
        >
          ＋
        </button>
      </div>

      {readOnlyReason && <p className="sheet__notice small">{readOnlyReason}</p>}
      {notice && <p className="sheet__notice small">{notice}</p>}

      {sheets.length === 0 && sheetsLoaded ? (
        <div className="sheet__empty card">
          <p className="muted">{t.sheet.empty}</p>
          <button type="button" disabled={readOnlyReason !== null} onClick={createSheet}>
            ＋ {t.sheet.addSheet}
          </button>
        </div>
      ) : activeSheet ? (
        <div className="sheet__table-wrap card">
          <table className="sheet__table">
            <thead>
              <tr>
                <th className="sheet__corner" />
                {colIndexes.map((c) => (
                  <th key={c} className="sheet__col-head">
                    {colLabel(c)}
                  </th>
                ))}
              </tr>
            </thead>
            <tbody>
              {rowIndexes.map((r) => (
                <tr key={r}>
                  <th className="sheet__row-head">{r + 1}</th>
                  {colIndexes.map((c) => {
                    const key = keyOf(r, c);
                    const cell = cellsByKey.get(key);
                    const value = cell?.value ?? "";
                    const isSelected = selected?.row === r && selected?.col === c;
                    const isEditingThis = editing?.row === r && editing?.col === c;
                    const isConflict = conflictHighlight.has(key);
                    const numeric = !isEditingThis && NUMERIC_RE.test(value);
                    return (
                      <td
                        key={c}
                        ref={(el) => {
                          if (el) cellRefs.current.set(key, el);
                          else cellRefs.current.delete(key);
                        }}
                        tabIndex={0}
                        className={[
                          "sheet__cell",
                          isSelected && "sheet__cell--selected",
                          numeric && "sheet__cell--numeric",
                          isConflict && "sheet__cell--conflict",
                        ]
                          .filter(Boolean)
                          .join(" ")}
                        onClick={() => setSelected({ row: r, col: c })}
                        onDoubleClick={() => startEdit(r, c, value)}
                        onKeyDown={(event) => handleGridKeyDown(event, r, c)}
                        onBlur={() => handleGridBlur(r, c)}
                        onCopy={(event) => handleGridCopy(event, r, c)}
                        onPaste={(event) => handleGridPaste(event, r, c)}
                      >
                        {isEditingThis ? (
                          <input
                            ref={inputRef}
                            className="sheet__cell-input"
                            value={editing.value}
                            onChange={(event) =>
                              setEditing((prev) =>
                                prev ? { ...prev, value: event.target.value } : prev,
                              )
                            }
                          />
                        ) : (
                          <span className="sheet__cell-value">{value}</span>
                        )}
                      </td>
                    );
                  })}
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      ) : null}
    </div>
  );
}
