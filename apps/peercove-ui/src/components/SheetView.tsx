// 共有シート(Excel ライク表、M6 G-2/H-4、ADR-0054/ADR-0055 決定 6)。共有メモ・
// 共有スケジュール表(ScheduleView.tsx)の基盤(ホスト正本 DB・コントロール
// チャネル配信・読み取りキャッシュ)を転用する。閲覧・セル編集は全員、シートの
// 作成・改名・削除は作成者 + ホストだけ(`can_manage` で判定)。競合はセル単位の
// revision CAS(ADR-0054 決定 4)。**シート名・セル値は console に出さないこと。**
import {
  ClipboardEvent,
  CSSProperties,
  KeyboardEvent,
  MouseEvent as ReactMouseEvent,
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import { CellFormat, CellWrite, SheetCell, SheetMeta, SheetOp, api, errorMessage } from "../ipc";
import { t } from "../i18n";
import { sharedRefToken } from "../sharedRefs";

// crates/peercove-core/src/sheet.rs の上限と同期(ADR-0054 決定 7、ADR-0055 決定 6)。
const MAX_SHEET_ROWS = 1000;
const MAX_SHEET_COLS = 200;
const MIN_COL_WIDTH = 20;
const MAX_COL_WIDTH = 600;
const MIN_ROW_HEIGHT = 16;
const MAX_ROW_HEIGHT = 400;
const DEFAULT_COL_WIDTH = 90;
const DEFAULT_ROW_HEIGHT = 26;

const MIN_DISPLAY_ROWS = 30;
const MIN_DISPLAY_COLS = 12;
const DISPLAY_MARGIN = 2;

const NUMERIC_RE = /^-?\d+(\.\d+)?$/;

const FONT_SIZE_CHOICES = [8, 9, 10, 11, 12, 14, 16, 18, 20, 24, 28, 32, 36];

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

interface CellPos {
  row: number;
  col: number;
}

/** 複数セル選択(矩形): anchor が始点、focus が終点(操作中に動く方)。 */
interface Selection {
  anchor: CellPos;
  focus: CellPos;
}

interface SelectionRect {
  r0: number;
  r1: number;
  c0: number;
  c1: number;
}

function rangeOf(sel: Selection): SelectionRect {
  return {
    r0: Math.min(sel.anchor.row, sel.focus.row),
    r1: Math.max(sel.anchor.row, sel.focus.row),
    c0: Math.min(sel.anchor.col, sel.focus.col),
    c1: Math.max(sel.anchor.col, sel.focus.col),
  };
}

function isSingleCell(rect: SelectionRect): boolean {
  return rect.r0 === rect.r1 && rect.c0 === rect.c1;
}

interface EditingState {
  row: number;
  col: number;
  value: string;
}

/** セルのテキスト装飾(太字・色・配置など)を CSS へ。 */
function textStyle(format: CellFormat | undefined): CSSProperties {
  if (!format) return {};
  const style: CSSProperties = {};
  if (format.bold) style.fontWeight = 700;
  if (format.italic) style.fontStyle = "italic";
  const decorations: string[] = [];
  if (format.underline) decorations.push("underline");
  if (format.strike) decorations.push("line-through");
  if (decorations.length > 0) style.textDecoration = decorations.join(" ");
  if (format.font_size) style.fontSize = `${format.font_size}px`;
  if (format.color) style.color = format.color;
  return style;
}

/** セルの箱の装飾(背景・罫線・配置)を CSS へ。既定の格子線(1px #d0d0d0)は
 * CSS 側で敷いてあるので、書式の罫線は上書きするぶんだけ強調して分かるよう
 * 太めにする。 */
function boxStyle(format: CellFormat | undefined, width?: number): CSSProperties {
  const style: CSSProperties = {};
  if (width !== undefined) {
    style.width = width;
    style.minWidth = width;
    style.maxWidth = width;
  }
  if (!format) return style;
  if (format.bg) style.backgroundColor = format.bg;
  if (format.align) style.textAlign = format.align;
  if (format.border_top) style.borderTop = "2px solid #333333";
  if (format.border_bottom) style.borderBottom = "2px solid #333333";
  if (format.border_left) style.borderLeft = "2px solid #333333";
  if (format.border_right) style.borderRight = "2px solid #333333";
  return style;
}

function formatIsEmpty(format: CellFormat | undefined): boolean {
  if (!format) return true;
  return (
    !format.bold &&
    !format.italic &&
    !format.underline &&
    !format.strike &&
    format.font_size === undefined &&
    format.color === undefined &&
    format.bg === undefined &&
    format.align === undefined &&
    !format.border_top &&
    !format.border_bottom &&
    !format.border_left &&
    !format.border_right
  );
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
  const [colWidths, setColWidths] = useState<Map<number, number>>(new Map());
  const [rowHeights, setRowHeights] = useState<Map<number, number>>(new Map());
  const [offline, setOffline] = useState(false);
  const [notice, setNotice] = useState<string | null>(null);
  const [menuFor, setMenuFor] = useState<string | null>(null);
  const [selection, setSelection] = useState<Selection | null>(null);
  const [editing, setEditing] = useState<EditingState | null>(null);
  const [conflictHighlight, setConflictHighlight] = useState<Set<string>>(
    new Set(),
  );

  const activeSheetIdRef = useRef<string | null>(null);
  const cellRefs = useRef<Map<string, HTMLTableCellElement>>(new Map());
  const inputRef = useRef<HTMLInputElement>(null);
  const skipNextBlurCommit = useRef(false);
  // IME 変換中フラグ(ADR-0055 決定 6: バグ修正)。変換中は Enter/Tab/Escape
  // をセル操作(確定・移動)に横取りしない。event.nativeEvent.isComposing に
  // 加えて保持しておく(compositionend 直後の keydown で isComposing が
  // 既に false になっている実装差を吸収するため)。
  const isComposingRef = useRef(false);
  // マウスドラッグによる複数セル選択中か(ADR-0055 決定 6)。
  const isSelectingRef = useRef(false);
  // 列幅・行高のドラッグリサイズ中の状態(ADR-0055 決定 6)。
  const resizingRef = useRef<{
    kind: "col" | "row";
    index: number;
    startPos: number;
    startSize: number;
  } | null>(null);

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
          setColWidths(new Map((reply.col_widths ?? []).map(([c, w]) => [c, w])));
          setRowHeights(new Map((reply.row_heights ?? []).map(([r, h]) => [r, h])));
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
      setColWidths(new Map());
      setRowHeights(new Map());
      return;
    }
    void loadCells(activeSheetId);
  }, [activeSheetId, seq, loadCells]);

  // シート一覧が変わったとき、選択中のシートが消えていたら先頭へ差し替える
  useEffect(() => {
    if (!sheetsLoaded) return;
    if (activeSheetId && sheets.some((s) => s.id === activeSheetId)) return;
    setActiveSheetId(sheets[0]?.id ?? null);
    setSelection(null);
    setEditing(null);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [sheets, sheetsLoaded]);

  // チャットの `@sheet:id` カードから開く
  useEffect(() => {
    if (!focusSheetId || !sheetsLoaded) return;
    if (sheets.some((s) => s.id === focusSheetId)) {
      setActiveSheetId(focusSheetId);
      setSelection(null);
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

  // 選択中の focus セルへフォーカスを追従させる(矢印キー操作を継続できるように)
  useEffect(() => {
    if (!selection || editing) return;
    const el = cellRefs.current.get(keyOf(selection.focus.row, selection.focus.col));
    el?.focus();
  }, [selection, editing]);

  // 編集開始時だけ input へフォーカス + 全選択(打鍵のたびには再実行しない)
  useEffect(() => {
    if (!editing) return;
    inputRef.current?.focus();
    inputRef.current?.select();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [editing?.row, editing?.col]);

  // マウスドラッグでの範囲選択・列幅/行高のリサイズは、ボタンを離した瞬間を
  // グリッドの外で検知する必要があるため window に付ける
  useEffect(() => {
    const onWindowMouseUp = () => {
      isSelectingRef.current = false;
      if (resizingRef.current) {
        const { kind, index } = resizingRef.current;
        resizingRef.current = null;
        const size =
          kind === "col" ? colWidthsRef.current.get(index) : rowHeightsRef.current.get(index);
        if (size !== undefined) {
          void commitLayout(kind, index, size);
        }
      }
    };
    const onWindowMouseMove = (event: MouseEvent) => {
      const resize = resizingRef.current;
      if (!resize) return;
      const delta =
        resize.kind === "col"
          ? event.clientX - resize.startPos
          : event.clientY - resize.startPos;
      const min = resize.kind === "col" ? MIN_COL_WIDTH : MIN_ROW_HEIGHT;
      const max = resize.kind === "col" ? MAX_COL_WIDTH : MAX_ROW_HEIGHT;
      const next = Math.max(min, Math.min(max, resize.startSize + delta));
      if (resize.kind === "col") {
        setColWidths((prev) => new Map(prev).set(resize.index, next));
      } else {
        setRowHeights((prev) => new Map(prev).set(resize.index, next));
      }
    };
    window.addEventListener("mouseup", onWindowMouseUp);
    window.addEventListener("mousemove", onWindowMouseMove);
    return () => {
      window.removeEventListener("mouseup", onWindowMouseUp);
      window.removeEventListener("mousemove", onWindowMouseMove);
    };
    // commitLayout は configPath/activeSheetId が変わらない限り安定させたいが、
    // 依存関係を素直に並べると毎回張り直しになるため ref 経由にする
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // 最新の colWidths/rowHeights を mouseup 時点で読むための ref(クロージャの
  // 古い state を掴まないように)
  const colWidthsRef = useRef(colWidths);
  useEffect(() => {
    colWidthsRef.current = colWidths;
  }, [colWidths]);
  const rowHeightsRef = useRef(rowHeights);
  useEffect(() => {
    rowHeightsRef.current = rowHeights;
  }, [rowHeights]);

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
    // 編集中セルは(seq ポーリングで cells が変わっても)必ず表示範囲に
    // 含める。範囲外に落ちて input が消える = 事実上の再マウントになるのを防ぐ。
    if (editing) {
      usedRows = Math.max(usedRows, editing.row + 1);
      usedCols = Math.max(usedCols, editing.col + 1);
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
    // editing.value(打鍵ごと)は範囲計算に無関係なので row/col だけを見る
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [cells, editing?.row, editing?.col]);

  const applyConflicts = useCallback((conflicts: SheetCell[]) => {
    setCells((prev) => {
      const map = new Map(prev.map((c) => [keyOf(c.row, c.col), c] as const));
      for (const c of conflicts) {
        if (c.value === "" && formatIsEmpty(c.format)) map.delete(keyOf(c.row, c.col));
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

  const commitLayout = useCallback(
    async (kind: "col" | "row", index: number, size: number) => {
      if (!activeSheetId) return;
      try {
        const reply =
          kind === "col"
            ? await sheetOp({ op: "set_col_width", sheet_id: activeSheetId, col: index, width: size })
            : await sheetOp({
                op: "set_row_height",
                sheet_id: activeSheetId,
                row: index,
                height: size,
              });
        if (reply.kind === "err") setNotice(reply.message);
      } catch (error) {
        setNotice(errorMessage(error));
      }
    },
    [activeSheetId, sheetOp],
  );

  const resetLayout = useCallback(
    async (kind: "col" | "row", index: number) => {
      if (!activeSheetId) return;
      if (kind === "col") {
        setColWidths((prev) => {
          const next = new Map(prev);
          next.delete(index);
          return next;
        });
      } else {
        setRowHeights((prev) => {
          const next = new Map(prev);
          next.delete(index);
          return next;
        });
      }
      try {
        const reply =
          kind === "col"
            ? await sheetOp({ op: "set_col_width", sheet_id: activeSheetId, col: index })
            : await sheetOp({ op: "set_row_height", sheet_id: activeSheetId, row: index });
        if (reply.kind === "err") setNotice(reply.message);
      } catch (error) {
        setNotice(errorMessage(error));
      }
    },
    [activeSheetId, sheetOp],
  );

  const startColResize = useCallback(
    (event: ReactMouseEvent, col: number) => {
      event.preventDefault();
      event.stopPropagation();
      resizingRef.current = {
        kind: "col",
        index: col,
        startPos: event.clientX,
        startSize: colWidths.get(col) ?? DEFAULT_COL_WIDTH,
      };
    },
    [colWidths],
  );

  const startRowResize = useCallback(
    (event: ReactMouseEvent, row: number) => {
      event.preventDefault();
      event.stopPropagation();
      resizingRef.current = {
        kind: "row",
        index: row,
        startPos: event.clientY,
        startSize: rowHeights.get(row) ?? DEFAULT_ROW_HEIGHT,
      };
    },
    [rowHeights],
  );

  const startEdit = useCallback(
    (row: number, col: number, value: string) => {
      if (readOnlyReason) return;
      setSelection({ anchor: { row, col }, focus: { row, col } });
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
      const next = opts.moveDown
        ? { row: Math.min(row + 1, displayRows - 1), col }
        : opts.moveRight
          ? { row, col: Math.min(col + 1, displayCols - 1) }
          : { row, col };
      setSelection({ anchor: next, focus: next });
      const unchanged = current ? current.value === value : value === "";
      if (!unchanged) {
        void writeCells([{ row, col, value, base_revision: baseRevision }]);
      }
    },
    [editing, cellsByKey, displayRows, displayCols, writeCells],
  );

  const moveActiveCell = useCallback(
    (direction: "up" | "down" | "left" | "right", row: number, col: number, extend: boolean) => {
      let nr = row;
      let nc = col;
      if (direction === "up") nr = Math.max(0, row - 1);
      if (direction === "down") nr = Math.min(displayRows - 1, row + 1);
      if (direction === "left") nc = Math.max(0, col - 1);
      if (direction === "right") nc = Math.min(displayCols - 1, col + 1);
      setSelection((prev) => {
        if (extend && prev) return { anchor: prev.anchor, focus: { row: nr, col: nc } };
        return { anchor: { row: nr, col: nc }, focus: { row: nr, col: nc } };
      });
    },
    [displayRows, displayCols],
  );

  /** 選択範囲の全セルへ書式を適用する(値は現値を維持したまま送る、
   * ADR-0055 決定 6)。`patchFor` はセルごとの既存書式から新しい書式を返す。 */
  const applyFormatToSelection = useCallback(
    (patchFor: (current: CellFormat, row: number, col: number) => CellFormat) => {
      if (!selection || readOnlyReason) return;
      const { r0, r1, c0, c1 } = rangeOf(selection);
      const writes: CellWrite[] = [];
      for (let r = r0; r <= r1; r++) {
        for (let c = c0; c <= c1; c++) {
          const current = cellsByKey.get(keyOf(r, c));
          const format = patchFor(current?.format ?? {}, r, c);
          writes.push({
            row: r,
            col: c,
            value: current?.value ?? "",
            base_revision: current?.revision ?? 0,
            format,
          });
        }
      }
      void writeCells(writes);
    },
    [selection, readOnlyReason, cellsByKey, writeCells],
  );

  const primaryFormat = useMemo(() => {
    if (!selection) return {} as CellFormat;
    return cellsByKey.get(keyOf(selection.focus.row, selection.focus.col))?.format ?? {};
  }, [selection, cellsByKey]);

  const toggleFormat = useCallback(
    (key: "bold" | "italic" | "underline" | "strike") => {
      const next = !primaryFormat[key];
      applyFormatToSelection((current) => ({ ...current, [key]: next || undefined }));
    },
    [primaryFormat, applyFormatToSelection],
  );

  const setFontSize = useCallback(
    (size: number | undefined) => {
      applyFormatToSelection((current) => ({ ...current, font_size: size }));
    },
    [applyFormatToSelection],
  );

  const setTextColor = useCallback(
    (color: string | undefined) => {
      applyFormatToSelection((current) => ({ ...current, color }));
    },
    [applyFormatToSelection],
  );

  const setBgColor = useCallback(
    (bg: string | undefined) => {
      applyFormatToSelection((current) => ({ ...current, bg }));
    },
    [applyFormatToSelection],
  );

  const setAlign = useCallback(
    (align: "left" | "center" | "right") => {
      applyFormatToSelection((current) => ({ ...current, align }));
    },
    [applyFormatToSelection],
  );

  const applyBorderOuter = useCallback(() => {
    if (!selection) return;
    const { r0, r1, c0, c1 } = rangeOf(selection);
    applyFormatToSelection((current, r, c) => ({
      ...current,
      border_top: r === r0 ? true : current.border_top,
      border_bottom: r === r1 ? true : current.border_bottom,
      border_left: c === c0 ? true : current.border_left,
      border_right: c === c1 ? true : current.border_right,
    }));
  }, [selection, applyFormatToSelection]);

  const applyBorderGrid = useCallback(() => {
    applyFormatToSelection((current) => ({
      ...current,
      border_top: true,
      border_bottom: true,
      border_left: true,
      border_right: true,
    }));
  }, [applyFormatToSelection]);

  const applyBorderNone = useCallback(() => {
    applyFormatToSelection((current) => ({
      ...current,
      border_top: undefined,
      border_bottom: undefined,
      border_left: undefined,
      border_right: undefined,
    }));
  }, [applyFormatToSelection]);

  const handleGridKeyDown = useCallback(
    (event: KeyboardEvent<HTMLTableCellElement>, row: number, col: number) => {
      const isEditingThis = editing && editing.row === row && editing.col === col;
      if (isEditingThis) {
        // IME 変換中の Enter/Tab/Escape はセルの確定・移動に横取りしない
        // (ADR-0055 決定 6: バグ修正)。isComposing の他に keyCode 229 も
        // 見て古い挙動のブラウザを吸収する。
        if (
          isComposingRef.current ||
          event.nativeEvent.isComposing ||
          event.keyCode === 229
        ) {
          return;
        }
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
        moveActiveCell(arrowMap[event.key], row, col, event.shiftKey);
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
        const rect = selection ? rangeOf(selection) : { r0: row, r1: row, c0: col, c1: col };
        const writes: CellWrite[] = [];
        for (let r = rect.r0; r <= rect.r1; r++) {
          for (let c = rect.c0; c <= rect.c1; c++) {
            const current = cellsByKey.get(keyOf(r, c));
            if (current) writes.push({ row: r, col: c, value: "", base_revision: current.revision });
          }
        }
        if (writes.length > 0) void writeCells(writes);
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
    [
      editing,
      readOnlyReason,
      selection,
      cellsByKey,
      commitEdit,
      cancelEdit,
      moveActiveCell,
      startEdit,
      writeCells,
    ],
  );

  // 入力バグの根本原因(ADR-0055 決定 6): このハンドラは以前 <td> に付けて
  // いた。フォーカスが <td> からその子である <input> へ移ると、ブラウザは
  // (子孫であっても)<td> 自身の blur/focusout を発火させる。編集開始時に
  // input へ focus() する実装と組み合わさり、「1 文字入力 → 即座に
  // handleGridBlur が発火 → commitEdit でその 1 文字だけを書き込んで
  // 編集終了 → 選択セルの focus 復帰で <td> にフォーカスが戻る」というサイ
  // クルが**打鍵のたびに**回っていた。結果、複数文字(IME 変換中の文字列も
  // 含む)を打っても直前の内容が毎回上書きされ、最後の 1 文字しか残らない
  // (日本語 IME は変換確定の Enter でも同じ経路から即終了するため入力不可
  // だった)。<input> 自身に付け替えることで、フォーカスが本当にセルの外へ
  // 出たときだけ blur が発火するようにする。
  const handleInputBlur = useCallback(
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
      const rect = selection ? rangeOf(selection) : { r0: row, r1: row, c0: col, c1: col };
      const lines: string[] = [];
      for (let r = rect.r0; r <= rect.r1; r++) {
        const line: string[] = [];
        for (let c = rect.c0; c <= rect.c1; c++) {
          line.push(cellsByKey.get(keyOf(r, c))?.value ?? "");
        }
        lines.push(line.join("\t"));
      }
      event.clipboardData.setData("text/plain", lines.join("\n"));
    },
    [editing, selection, cellsByKey],
  );

  const handleGridPaste = useCallback(
    (event: ClipboardEvent<HTMLTableCellElement>, row: number, col: number) => {
      if (editing && editing.row === row && editing.col === col) return;
      if (readOnlyReason) return;
      event.preventDefault();
      const text = event.clipboardData.getData("text/plain");
      if (!text) return;
      // 貼り付け開始位置は選択範囲の左上(Excel と同様、複数セル選択中でも
      // 貼り付けは矩形の起点から展開する)
      const rect = selection ? rangeOf(selection) : { r0: row, r1: row, c0: col, c1: col };
      const startRow = rect.r0;
      const startCol = rect.c0;
      const lines = text.replace(/\r\n/g, "\n").replace(/\r/g, "\n").split("\n");
      while (lines.length > 1 && lines[lines.length - 1] === "") lines.pop();
      const grid = lines.map((line) => line.split("\t"));
      const rowSpan = grid.length;
      const colSpan = grid.reduce((max, line) => Math.max(max, line.length), 0);
      if (startRow + rowSpan > MAX_SHEET_ROWS || startCol + colSpan > MAX_SHEET_COLS) {
        setNotice(t.sheet.pasteRejected);
        return;
      }
      const writes: CellWrite[] = [];
      grid.forEach((line, r) => {
        line.forEach((value, c) => {
          const targetRow = startRow + r;
          const targetCol = startCol + c;
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
    [editing, readOnlyReason, selection, cellsByKey, writeCells],
  );

  const handleCellMouseDown = useCallback(
    (event: ReactMouseEvent<HTMLTableCellElement>, row: number, col: number) => {
      if (event.button !== 0) return;
      if (editing) return;
      if (event.shiftKey && selection) {
        setSelection({ anchor: selection.anchor, focus: { row, col } });
      } else {
        setSelection({ anchor: { row, col }, focus: { row, col } });
      }
      isSelectingRef.current = true;
    },
    [editing, selection],
  );

  const handleCellMouseEnter = useCallback((row: number, col: number) => {
    if (!isSelectingRef.current) return;
    setSelection((prev) =>
      prev ? { anchor: prev.anchor, focus: { row, col } } : { anchor: { row, col }, focus: { row, col } },
    );
  }, []);

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
  const selectionRect = selection ? rangeOf(selection) : null;
  const hasRangeSelection = selectionRect !== null && !isSingleCell(selectionRect);
  const toolbarDisabled = readOnlyReason !== null || !selection;

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
                setSelection(null);
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

      {activeSheet && (
        <div className="sheet__toolbar" role="toolbar" aria-label={t.sheet.title}>
          <div className="sheet__toolbar-group">
            <button
              type="button"
              className={primaryFormat.bold ? "sheet__toolbar-btn sheet__toolbar-btn--active" : "sheet__toolbar-btn"}
              title={t.sheet.toolbarBold}
              disabled={toolbarDisabled}
              onClick={() => toggleFormat("bold")}
            >
              <b>B</b>
            </button>
            <button
              type="button"
              className={primaryFormat.italic ? "sheet__toolbar-btn sheet__toolbar-btn--active" : "sheet__toolbar-btn"}
              title={t.sheet.toolbarItalic}
              disabled={toolbarDisabled}
              onClick={() => toggleFormat("italic")}
            >
              <i>I</i>
            </button>
            <button
              type="button"
              className={primaryFormat.underline ? "sheet__toolbar-btn sheet__toolbar-btn--active" : "sheet__toolbar-btn"}
              title={t.sheet.toolbarUnderline}
              disabled={toolbarDisabled}
              onClick={() => toggleFormat("underline")}
            >
              <u>U</u>
            </button>
            <button
              type="button"
              className={primaryFormat.strike ? "sheet__toolbar-btn sheet__toolbar-btn--active" : "sheet__toolbar-btn"}
              title={t.sheet.toolbarStrike}
              disabled={toolbarDisabled}
              onClick={() => toggleFormat("strike")}
            >
              <s>S</s>
            </button>
          </div>

          <div className="sheet__toolbar-group">
            <select
              className="sheet__toolbar-select"
              aria-label={t.sheet.toolbarFontSize}
              disabled={toolbarDisabled}
              value={primaryFormat.font_size ?? ""}
              onChange={(event) =>
                setFontSize(event.target.value ? Number(event.target.value) : undefined)
              }
            >
              <option value="">{t.sheet.toolbarFontSizeDefault}</option>
              {FONT_SIZE_CHOICES.map((size) => (
                <option key={size} value={size}>
                  {size}
                </option>
              ))}
            </select>
          </div>

          <div className="sheet__toolbar-group">
            <label className="sheet__toolbar-color" title={t.sheet.toolbarTextColor}>
              A
              <input
                type="color"
                disabled={toolbarDisabled}
                value={primaryFormat.color ?? "#000000"}
                onChange={(event) => setTextColor(event.target.value)}
              />
            </label>
            <button
              type="button"
              className="sheet__toolbar-btn"
              title={`${t.sheet.toolbarTextColor}: ${t.sheet.toolbarColorNone}`}
              disabled={toolbarDisabled}
              onClick={() => setTextColor(undefined)}
            >
              {t.sheet.toolbarColorNone}
            </button>
            <label className="sheet__toolbar-color sheet__toolbar-color--bg" title={t.sheet.toolbarBgColor}>
              🎨
              <input
                type="color"
                disabled={toolbarDisabled}
                value={primaryFormat.bg ?? "#ffffff"}
                onChange={(event) => setBgColor(event.target.value)}
              />
            </label>
            <button
              type="button"
              className="sheet__toolbar-btn"
              title={`${t.sheet.toolbarBgColor}: ${t.sheet.toolbarColorNone}`}
              disabled={toolbarDisabled}
              onClick={() => setBgColor(undefined)}
            >
              {t.sheet.toolbarColorNone}
            </button>
          </div>

          <div className="sheet__toolbar-group">
            <button
              type="button"
              className={
                primaryFormat.align === "left"
                  ? "sheet__toolbar-btn sheet__toolbar-btn--active"
                  : "sheet__toolbar-btn"
              }
              title={t.sheet.toolbarAlignLeft}
              disabled={toolbarDisabled}
              onClick={() => setAlign("left")}
            >
              ⯇
            </button>
            <button
              type="button"
              className={
                primaryFormat.align === "center"
                  ? "sheet__toolbar-btn sheet__toolbar-btn--active"
                  : "sheet__toolbar-btn"
              }
              title={t.sheet.toolbarAlignCenter}
              disabled={toolbarDisabled}
              onClick={() => setAlign("center")}
            >
              ▬
            </button>
            <button
              type="button"
              className={
                primaryFormat.align === "right"
                  ? "sheet__toolbar-btn sheet__toolbar-btn--active"
                  : "sheet__toolbar-btn"
              }
              title={t.sheet.toolbarAlignRight}
              disabled={toolbarDisabled}
              onClick={() => setAlign("right")}
            >
              ⯈
            </button>
          </div>

          <div className="sheet__toolbar-group">
            <button
              type="button"
              className="sheet__toolbar-btn"
              title={t.sheet.toolbarBorderOuter}
              disabled={toolbarDisabled}
              onClick={applyBorderOuter}
            >
              ▢
            </button>
            <button
              type="button"
              className="sheet__toolbar-btn"
              title={t.sheet.toolbarBorderGrid}
              disabled={toolbarDisabled}
              onClick={applyBorderGrid}
            >
              ▦
            </button>
            <button
              type="button"
              className="sheet__toolbar-btn"
              title={t.sheet.toolbarBorderNone}
              disabled={toolbarDisabled}
              onClick={applyBorderNone}
            >
              ▭
            </button>
          </div>
          {hasRangeSelection && selectionRect && (
            <span className="sheet__toolbar-range muted small">
              {colLabel(selectionRect.c0)}
              {selectionRect.r0 + 1}:{colLabel(selectionRect.c1)}
              {selectionRect.r1 + 1}
            </span>
          )}
        </div>
      )}

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
                  <th
                    key={c}
                    className="sheet__col-head"
                    style={boxStyle(undefined, colWidths.get(c))}
                  >
                    {colLabel(c)}
                    <span
                      className="sheet__col-resize-handle"
                      title={t.sheet.resetToDefault}
                      onMouseDown={(event) => startColResize(event, c)}
                      onDoubleClick={() => void resetLayout("col", c)}
                    />
                  </th>
                ))}
              </tr>
            </thead>
            <tbody>
              {rowIndexes.map((r) => (
                <tr key={r} style={{ height: rowHeights.get(r) }}>
                  <th className="sheet__row-head">
                    {r + 1}
                    <span
                      className="sheet__row-resize-handle"
                      title={t.sheet.resetToDefault}
                      onMouseDown={(event) => startRowResize(event, r)}
                      onDoubleClick={() => void resetLayout("row", r)}
                    />
                  </th>
                  {colIndexes.map((c) => {
                    const key = keyOf(r, c);
                    const cell = cellsByKey.get(key);
                    const value = cell?.value ?? "";
                    const isFocusCell =
                      selection?.focus.row === r && selection?.focus.col === c;
                    const inRange =
                      selectionRect !== null &&
                      r >= selectionRect.r0 &&
                      r <= selectionRect.r1 &&
                      c >= selectionRect.c0 &&
                      c <= selectionRect.c1;
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
                        style={boxStyle(cell?.format, colWidths.get(c))}
                        className={[
                          "sheet__cell",
                          isFocusCell && "sheet__cell--selected",
                          inRange && !isFocusCell && "sheet__cell--in-range",
                          numeric && "sheet__cell--numeric",
                          isConflict && "sheet__cell--conflict",
                        ]
                          .filter(Boolean)
                          .join(" ")}
                        onMouseDown={(event) => handleCellMouseDown(event, r, c)}
                        onMouseEnter={() => handleCellMouseEnter(r, c)}
                        onDoubleClick={() => startEdit(r, c, value)}
                        onKeyDown={(event) => handleGridKeyDown(event, r, c)}
                        onCopy={(event) => handleGridCopy(event, r, c)}
                        onPaste={(event) => handleGridPaste(event, r, c)}
                      >
                        {isEditingThis ? (
                          <input
                            ref={inputRef}
                            className="sheet__cell-input"
                            style={textStyle(cell?.format)}
                            value={editing.value}
                            onChange={(event) =>
                              setEditing((prev) =>
                                prev ? { ...prev, value: event.target.value } : prev,
                              )
                            }
                            // blur は <td> ではなく <input> 自身に付ける(理由は
                            // handleInputBlur 定義側のコメント参照)
                            onBlur={() => handleInputBlur(r, c)}
                            onCompositionStart={() => {
                              isComposingRef.current = true;
                            }}
                            onCompositionEnd={() => {
                              isComposingRef.current = false;
                            }}
                          />
                        ) : (
                          <span className="sheet__cell-value" style={textStyle(cell?.format)}>
                            {value}
                          </span>
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
