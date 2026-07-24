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
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { flushSync } from "react-dom";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import {
  CellFormat,
  CellWrite,
  SheetCell,
  SheetMerge,
  SheetMeta,
  SheetOp,
  SheetPresencePeer,
  api,
  errorMessage,
} from "../ipc";
import { t } from "../i18n";
import { sharedRefToken } from "../sharedRefs";
import { ContextMenu, ContextMenuEntry } from "./ContextMenu";

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

// 固定枠(freeze panes、ADR-0055 決定 6/ADR-0055 決定 6 追補 H-7a)の
// ピクセルオフセット計算のフォールバック初期値(DOM 実測が終わるまでの
// 一瞬だけ使う。useLayoutEffect が paint 前に実測値へ差し替えるため、
// 画面には出ない)。H-5 時点はこの定数だけで offset を計算していたが、
// 実際の行高(可変)・ヘッダーの実描画高とずれて固定枠がヘッダーへ食い
// 込む不具合があったため、H-7a で DOM 実測ベースに切り替えた。
const HEADER_ROW_HEIGHT = DEFAULT_ROW_HEIGHT;
const ROW_HEAD_WIDTH = 44;

function numberArraysEqual(a: number[], b: number[]): boolean {
  if (a.length !== b.length) return false;
  for (let i = 0; i < a.length; i++) {
    if (a[i] !== b[i]) return false;
  }
  return true;
}

// プレゼンス送信のスロットル間隔(ADR-0055 決定 6: 選択位置の共有 ~4 回/秒)。
const PRESENCE_THROTTLE_MS = 250;

// Undo 履歴の最大件数(ADR-0055 決定 6)。
const MAX_UNDO_STACK = 50;

/** FNV-1a(32bit)。Avatar.tsx と同じ手法で、名前からプレゼンス色の色相を
 * 決定的に決める(暗号用途ではない)。 */
function hueOfName(name: string): number {
  let hash = 0x811c9dc5;
  for (let i = 0; i < name.length; i++) {
    hash ^= name.charCodeAt(i);
    hash = Math.imul(hash, 0x01000193);
  }
  return (hash >>> 0) % 360;
}

function presenceColor(name: string): string {
  return `hsl(${hueOfName(name)}, 70%, 45%)`;
}

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

/** Undo 1 セル分の旧状態(ADR-0055 決定 6)。 */
interface UndoCellSnapshot {
  row: number;
  col: number;
  value: string;
  format?: CellFormat;
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
  const [merges, setMerges] = useState<SheetMerge[]>([]);
  const [presence, setPresence] = useState<SheetPresencePeer[]>([]);
  const [offline, setOffline] = useState(false);
  const [notice, setNotice] = useState<string | null>(null);
  const [menuFor, setMenuFor] = useState<string | null>(null);
  const [selection, setSelection] = useState<Selection | null>(null);
  const [editing, setEditing] = useState<EditingState | null>(null);
  const [conflictHighlight, setConflictHighlight] = useState<Set<string>>(
    new Set(),
  );
  // 右クリックメニュー(ADR-0055 決定 6)
  const [cellMenu, setCellMenu] = useState<{
    x: number;
    y: number;
    row: number;
    col: number;
  } | null>(null);
  // シート内検索(Ctrl+F、ADR-0055 決定 6)
  const [searchOpen, setSearchOpen] = useState(false);
  const [searchQuery, setSearchQuery] = useState("");
  const [searchIndex, setSearchIndex] = useState(0);
  const searchInputRef = useRef<HTMLInputElement>(null);

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
  // Undo 履歴(自分の直近操作、ローカルのみ。ADR-0055 決定 6)。
  const undoStackRef = useRef<UndoCellSnapshot[][]>([]);
  // Undo 自体の書き込み中は、その書き込みを新たな Undo 履歴として積まない
  // (Redo は不要 = 積み直さない、ADR-0055 決定 6)。
  const isUndoingRef = useRef(false);
  // プレゼンス送信のスロットル(ADR-0055 決定 6)。
  const presenceThrottleRef = useRef<number | null>(null);
  const lastSentPresenceRef = useRef<string | null>(null);
  // 文字キーでの編集開始時、直後の自動 focus+select(既存 useEffect)を
  // 1 回だけ止める(ADR-0055 決定 6 追補 H-7a: IME バグ修正)。true の間は
  // その useEffect が select() で IME 変換中の状態を壊さないようにする。
  const skipAutoFocusSelectRef = useRef(false);
  // シートへの write を直列化する送信キュー(ADR-0055 決定 6 追補 H-7a:
  // SQLite 側の locking protocol エラー対策)。前の write の応答を待って
  // から次を送る。書式適用・編集確定・貼り付け・Undo すべて writeCells
  // 経由なのでここ 1 箇所で足りる。
  const writeQueueRef = useRef<Promise<unknown>>(Promise.resolve());
  // 文字色・背景色 <input type="color"> のデバウンス(ADR-0055 決定 6
  // 追補 H-7a)。ドラッグ中は onChange が大量発火するため、確定(次の
  // onChange が来ない = 入力が止まった、または blur)までまとめる。
  const textColorDebounceRef = useRef<number | null>(null);
  const bgColorDebounceRef = useRef<number | null>(null);
  // 固定枠(freeze panes)のピクセルオフセット実測用(ADR-0055 決定 6
  // 追補 H-7a)。DOM の実描画サイズを useLayoutEffect で読み取る。
  const theadRowRef = useRef<HTMLTableRowElement | null>(null);
  const cornerRef = useRef<HTMLTableCellElement | null>(null);
  const rowElRefs = useRef<Map<number, HTMLTableRowElement>>(new Map());
  const colHeadElRefs = useRef<Map<number, HTMLTableCellElement>>(new Map());
  const [measuredRowTopOffsets, setMeasuredRowTopOffsets] = useState<number[]>([]);
  const [measuredColLeftOffsets, setMeasuredColLeftOffsets] = useState<number[]>([]);

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
          setMerges(reply.merges ?? []);
          setPresence(reply.presence ?? []);
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
      setMerges([]);
      setPresence([]);
      return;
    }
    void loadCells(activeSheetId);
  }, [activeSheetId, seq, loadCells]);

  // シート切り替え時は Undo 履歴・検索・右クリックメニューをリセットする
  // (他シートのセル座標を誤って書き戻さないように)。
  useEffect(() => {
    undoStackRef.current = [];
    setSearchOpen(false);
    setSearchQuery("");
    setSearchIndex(0);
    setCellMenu(null);
  }, [activeSheetId]);

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

  // 編集開始時だけ input へフォーカス + 全選択(打鍵のたびには再実行しない)。
  // 文字キーでの編集開始(startTypedEdit)は既に同期的に focus 済みで、
  // かつ IME 変換中にここで select() すると変換が壊れうるため 1 回だけ
  // 飛ばす(ADR-0055 決定 6 追補 H-7a)。F2/ダブルクリック(startEdit)は
  // このガードを通らないので従来どおり focus+select する。
  useEffect(() => {
    if (!editing) return;
    if (skipAutoFocusSelectRef.current) {
      skipAutoFocusSelectRef.current = false;
      return;
    }
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

  // 固定枠(freeze panes)のピクセルオフセットを DOM の実描画サイズから
  // 計算する(ADR-0055 決定 6 追補 H-7a)。useLayoutEffect は commit 後・
  // paint 前に同期実行されるため、初回マウントや行高/列幅変更の際も
  // ずれた中間状態が画面に出ることはない。
  useLayoutEffect(() => {
    const freezeRows = activeSheet?.freeze_rows ?? 0;
    const freezeCols = activeSheet?.freeze_cols ?? 0;
    const headerH = theadRowRef.current?.offsetHeight ?? HEADER_ROW_HEIGHT;
    const rowHeadW = cornerRef.current?.offsetWidth ?? ROW_HEAD_WIDTH;
    const rowTops: number[] = [];
    let accR = headerH;
    for (let r = 0; r < freezeRows; r++) {
      rowTops.push(accR);
      accR += rowElRefs.current.get(r)?.offsetHeight ?? rowHeights.get(r) ?? DEFAULT_ROW_HEIGHT;
    }
    const colLefts: number[] = [];
    let accC = rowHeadW;
    for (let c = 0; c < freezeCols; c++) {
      colLefts.push(accC);
      accC += colHeadElRefs.current.get(c)?.offsetWidth ?? colWidths.get(c) ?? DEFAULT_COL_WIDTH;
    }
    setMeasuredRowTopOffsets((prev) => (numberArraysEqual(prev, rowTops) ? prev : rowTops));
    setMeasuredColLeftOffsets((prev) => (numberArraysEqual(prev, colLefts) ? prev : colLefts));
  }, [
    activeSheet?.freeze_rows,
    activeSheet?.freeze_cols,
    activeSheet?.gridlines,
    rowHeights,
    colWidths,
    cells,
    merges,
  ]);

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
    // 結合セルの範囲も必ず表示範囲に含める(colSpan/rowSpan が表示領域の
    // 外へはみ出さないように)。
    for (const merge of merges) {
      usedRows = Math.max(usedRows, merge.row + merge.row_span);
      usedCols = Math.max(usedCols, merge.col + merge.col_span);
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
  }, [cells, merges, editing?.row, editing?.col]);

  // 結合セルの検索用マップ(ADR-0055 決定 6)。mergeCovered は結合範囲内の
  // 全セル(左上を含む)→ その結合。mergeByTopLeft は左上セルだけの索引。
  const mergeCovered = useMemo(() => {
    const map = new Map<string, SheetMerge>();
    for (const merge of merges) {
      for (let r = merge.row; r < merge.row + merge.row_span; r++) {
        for (let c = merge.col; c < merge.col + merge.col_span; c++) {
          map.set(keyOf(r, c), merge);
        }
      }
    }
    return map;
  }, [merges]);

  /** 結合範囲内のセルはその結合の左上セルへ丸める(選択は結合全体を
   * 1 単位として扱う、ADR-0055 決定 6)。 */
  const resolveMergeAnchor = useCallback(
    (row: number, col: number): CellPos => {
      const merge = mergeCovered.get(keyOf(row, col));
      return merge ? { row: merge.row, col: merge.col } : { row, col };
    },
    [mergeCovered],
  );

  // ツールバー・右クリックメニュー・Undo など複数箇所から参照するため、
  // JSX 直前ではなくここで(hooks より前に)計算しておく。
  const selectionRect = selection ? rangeOf(selection) : null;
  const hasRangeSelection = selectionRect !== null && !isSingleCell(selectionRect);
  const toolbarDisabled = readOnlyReason !== null || !selection;

  // 現在の選択範囲がちょうど 1 件の既存結合と一致するか(単一セル選択かつ
  // その左上が結合の左上と一致)。一致すれば「結合」ボタンは「解除」になる。
  const mergeAtFocus = selection
    ? mergeCovered.get(keyOf(selection.focus.row, selection.focus.col))
    : undefined;
  const canUnmergeSelection =
    !!mergeAtFocus && selectionRect !== null && isSingleCell(selectionRect);

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
      // Undo 履歴(自分の直近操作、ADR-0055 決定 6): 書き込み前の状態を
      // 積む。Undo 自体の書き戻しは積み直さない(Redo は不要)。書き込み
      // キュー投入前(=呼び出し直後)の状態を積む必要があるため、送信の
      // 直列化より前にここで計算する。
      if (!isUndoingRef.current) {
        const snapshot: UndoCellSnapshot[] = writes.map((write) => {
          const current = cellsByKey.get(keyOf(write.row, write.col));
          return {
            row: write.row,
            col: write.col,
            value: current?.value ?? "",
            format: current?.format,
          };
        });
        undoStackRef.current = [...undoStackRef.current, snapshot].slice(
          -MAX_UNDO_STACK,
        );
      }
      const targetSheetId = activeSheetId;
      const send = async () => {
        try {
          const reply = await sheetOp({ op: "write", sheet_id: targetSheetId, cells: writes });
          if (reply.kind === "write_result") {
            if (reply.conflicts.length > 0) {
              applyConflicts(reply.conflicts);
              setNotice(t.sheet.conflictNotice(reply.conflicts.length));
            }
            void loadCells(targetSheetId);
          } else if (reply.kind === "err") {
            setNotice(reply.message);
          }
        } catch (error) {
          setNotice(errorMessage(error));
        }
      };
      // 送信の直列化(ADR-0055 決定 6 追補 H-7a): 前の write の応答を待って
      // から次を送る。書式適用・編集確定・貼り付けはすべてこの関数を通る
      // ので、ここ 1 箇所の直列化で足りる(大量の色ドラッグ連打などで
      // SQLite 側の locking protocol エラーを誘発しないようにする)。
      const next = writeQueueRef.current.then(send, send);
      writeQueueRef.current = next;
      await next;
    },
    [activeSheetId, sheetOp, applyConflicts, loadCells, cellsByKey],
  );

  /** 自分の直近操作を 1 つ取り消す(Ctrl+Z、ローカル、ADR-0055 決定 6)。
   * base_revision は「現在手元の revision」— 他人が先に変更していれば
   * CAS 競合として自然にスキップされる(競合通知は writeCells 側に乗る)。
   * 結合/解除・列幅/行高・シート設定は対象外(履歴に積んでいない)。 */
  const undo = useCallback(() => {
    const entry = undoStackRef.current.pop();
    if (!entry || !activeSheetId) return;
    const writes: CellWrite[] = entry.map((snapshot) => {
      const current = cellsByKey.get(keyOf(snapshot.row, snapshot.col));
      return {
        row: snapshot.row,
        col: snapshot.col,
        value: snapshot.value,
        base_revision: current?.revision ?? 0,
        format: snapshot.format ?? {},
      };
    });
    isUndoingRef.current = true;
    void writeCells(writes).finally(() => {
      isUndoingRef.current = false;
    });
  }, [activeSheetId, cellsByKey, writeCells]);

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
    (row: number, col: number, value: string | null) => {
      if (readOnlyReason) return;
      // 結合セルは左上のみ編集可(選択は結合全体を 1 単位として扱う、
      // ADR-0055 決定 6)。value が null なら既存値をそこから読み直す
      // (直接文字入力で上書き開始する場合は呼び出し側が実値を渡す)。
      const anchor = resolveMergeAnchor(row, col);
      setSelection({ anchor, focus: anchor });
      const resolvedValue =
        value ?? cellsByKey.get(keyOf(anchor.row, anchor.col))?.value ?? "";
      setEditing({ row: anchor.row, col: anchor.col, value: resolvedValue });
    },
    [readOnlyReason, resolveMergeAnchor, cellsByKey],
  );

  /** 文字キーで編集を開始する(ADR-0055 決定 6 追補 H-7a: IME バグ修正)。
   * 空値で編集モードへ入り、その場で input へ同期的に focus する。呼び
   * 出し側(handleGridKeyDown)は該当 keydown を preventDefault しないので、
   * ブラウザはこの直後の既定動作(文字挿入 / IME 変換開始)をこの時点で
   * focus されている要素、つまりここで作った input に対して行う。結果、
   * 1 打鍵目が生のキー文字として注入されることも、IME の変換開始が
   * 素通しされて "a" が混ざることもなくなる。
   * React 18+ はイベントハンドラ内の setState を自動バッチするため、
   * flushSync で同期コミットしないと inputRef.current がこの時点でまだ
   * 古い(または null の)ままになる。 */
  const startTypedEdit = useCallback(
    (row: number, col: number) => {
      if (readOnlyReason) return;
      skipAutoFocusSelectRef.current = true;
      flushSync(() => {
        startEdit(row, col, "");
      });
      inputRef.current?.focus();
    },
    [readOnlyReason, startEdit],
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
      // 結合セルへ着地したら左上へ丸める(結合内の非左上セルは td を
      // 描画しないため、丸めないとフォーカスできずキー操作が固まる)。
      const landed = resolveMergeAnchor(nr, nc);
      setSelection((prev) => {
        if (extend && prev) return { anchor: prev.anchor, focus: landed };
        return { anchor: landed, focus: landed };
      });
    },
    [displayRows, displayCols, resolveMergeAnchor],
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

  // <input type="color"> はドラッグ中に大量の onChange を発火するため、
  // 適用は入力確定時までデバウンスする(ADR-0055 決定 6 追補 H-7a)。
  // blur(ピッカーを閉じた/フォーカスが外れた)では即座に確定させる。
  const debouncedTextColor = useCallback(
    (color: string) => {
      if (textColorDebounceRef.current !== null) {
        window.clearTimeout(textColorDebounceRef.current);
      }
      textColorDebounceRef.current = window.setTimeout(() => {
        textColorDebounceRef.current = null;
        setTextColor(color);
      }, 250);
    },
    [setTextColor],
  );
  const flushTextColor = useCallback(
    (color: string) => {
      if (textColorDebounceRef.current !== null) {
        window.clearTimeout(textColorDebounceRef.current);
        textColorDebounceRef.current = null;
      }
      setTextColor(color);
    },
    [setTextColor],
  );
  const debouncedBgColor = useCallback(
    (bg: string) => {
      if (bgColorDebounceRef.current !== null) {
        window.clearTimeout(bgColorDebounceRef.current);
      }
      bgColorDebounceRef.current = window.setTimeout(() => {
        bgColorDebounceRef.current = null;
        setBgColor(bg);
      }, 250);
    },
    [setBgColor],
  );
  const flushBgColor = useCallback(
    (bg: string) => {
      if (bgColorDebounceRef.current !== null) {
        window.clearTimeout(bgColorDebounceRef.current);
        bgColorDebounceRef.current = null;
      }
      setBgColor(bg);
    },
    [setBgColor],
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

  // ---- セル結合(ADR-0055 決定 6) ----

  const toggleMerge = useCallback(() => {
    if (!activeSheetId || !selectionRect) return;
    if (canUnmergeSelection && mergeAtFocus) {
      void (async () => {
        try {
          const reply = await sheetOp({
            op: "unmerge",
            sheet_id: activeSheetId,
            row: mergeAtFocus.row,
            col: mergeAtFocus.col,
          });
          if (reply.kind === "err") setNotice(reply.message);
          else void loadCells(activeSheetId);
        } catch (error) {
          setNotice(errorMessage(error));
        }
      })();
      return;
    }
    if (isSingleCell(selectionRect)) return; // 単一セルは結合不可(2 セル以上)
    const { r0, r1, c0, c1 } = selectionRect;
    let dataLoss = false;
    for (let r = r0; r <= r1 && !dataLoss; r++) {
      for (let c = c0; c <= c1; c++) {
        if (r === r0 && c === c0) continue;
        const cell = cellsByKey.get(keyOf(r, c));
        if (cell && cell.value !== "") {
          dataLoss = true;
          break;
        }
      }
    }
    if (dataLoss && !window.confirm(t.sheet.mergeConfirmDataLoss)) return;
    void (async () => {
      try {
        const reply = await sheetOp({
          op: "merge",
          sheet_id: activeSheetId,
          merge: { row: r0, col: c0, row_span: r1 - r0 + 1, col_span: c1 - c0 + 1 },
        });
        if (reply.kind === "err") setNotice(reply.message);
        else void loadCells(activeSheetId);
      } catch (error) {
        setNotice(errorMessage(error));
      }
    })();
  }, [
    activeSheetId,
    selectionRect,
    canUnmergeSelection,
    mergeAtFocus,
    cellsByKey,
    sheetOp,
    loadCells,
  ]);

  // ---- シート設定: 目盛線・固定枠(ADR-0055 決定 6) ----

  const updateSheetSettings = useCallback(
    (patch: Partial<{ gridlines: boolean; freeze_rows: number; freeze_cols: number }>) => {
      if (!activeSheet) return;
      void (async () => {
        try {
          const reply = await sheetOp({
            op: "set_sheet_settings",
            sheet_id: activeSheet.id,
            gridlines: patch.gridlines ?? activeSheet.gridlines ?? true,
            freeze_rows: patch.freeze_rows ?? activeSheet.freeze_rows ?? 0,
            freeze_cols: patch.freeze_cols ?? activeSheet.freeze_cols ?? 0,
          });
          if (reply.kind === "err") setNotice(reply.message);
          else void loadSheets();
        } catch (error) {
          setNotice(errorMessage(error));
        }
      })();
    },
    [activeSheet, sheetOp, loadSheets],
  );

  const freezeAtSelection = useCallback(() => {
    if (!selectionRect) return;
    updateSheetSettings({ freeze_rows: selectionRect.r0, freeze_cols: selectionRect.c0 });
  }, [selectionRect, updateSheetSettings]);

  const clearFreeze = useCallback(() => {
    updateSheetSettings({ freeze_rows: 0, freeze_cols: 0 });
  }, [updateSheetSettings]);

  // ---- プレゼンス(選択セル共有、ADR-0055 決定 6) ----
  // 選択の focus セルが変わるたびスロットル(250ms)して送信する。オフライン・
  // 未対応時は送らない(readOnlyReason が立っている = 送っても無意味)。

  useEffect(() => {
    if (!selection || !activeSheetId || readOnlyReason) return;
    const presenceKey = `${activeSheetId}:${selection.focus.row}:${selection.focus.col}`;
    if (lastSentPresenceRef.current === presenceKey) return;
    if (presenceThrottleRef.current !== null) window.clearTimeout(presenceThrottleRef.current);
    presenceThrottleRef.current = window.setTimeout(() => {
      lastSentPresenceRef.current = presenceKey;
      void sheetOp({
        op: "presence",
        sheet_id: activeSheetId,
        row: selection.focus.row,
        col: selection.focus.col,
      }).catch(() => {
        // プレゼンスの送信失敗は静かに無視する(表示上の付随情報のため)
      });
    }, PRESENCE_THROTTLE_MS);
    return () => {
      if (presenceThrottleRef.current !== null) window.clearTimeout(presenceThrottleRef.current);
    };
  }, [selection, activeSheetId, readOnlyReason, sheetOp]);

  const presenceByCell = useMemo(() => {
    const map = new Map<string, SheetPresencePeer[]>();
    for (const peer of presence) {
      const key = keyOf(peer.row, peer.col);
      const list = map.get(key);
      if (list) list.push(peer);
      else map.set(key, [peer]);
    }
    return map;
  }, [presence]);

  // ---- クリップボード(コピー/貼り付け、独自右クリックメニュー用、
  // ADR-0055 決定 6)。Ctrl+C/Ctrl+V のブラウザ既定コピー/貼り付けは
  // handleGridCopy/handleGridPaste が別途面倒を見ている。 ----

  const copySelectionToClipboard = useCallback(() => {
    if (!selectionRect) return;
    const { r0, r1, c0, c1 } = selectionRect;
    const lines: string[] = [];
    for (let r = r0; r <= r1; r++) {
      const line: string[] = [];
      for (let c = c0; c <= c1; c++) line.push(cellsByKey.get(keyOf(r, c))?.value ?? "");
      lines.push(line.join("\t"));
    }
    void writeText(lines.join("\n"));
  }, [selectionRect, cellsByKey]);

  const pasteIntoSelection = useCallback(async () => {
    if (!selectionRect || readOnlyReason) return;
    let text: string;
    try {
      // navigator.clipboard.readText() を試みる(ADR-0055 決定 6 の指示どおり。
      // Tauri プラグイン側は allow-write-text しか許可していないため、読み取りは
      // Web API 経由 — 権限が無ければここで例外になり、Ctrl+V を促す)。
      text = (await navigator.clipboard.readText()) ?? "";
    } catch {
      setNotice(t.sheet.clipboardReadUnavailable);
      return;
    }
    if (!text) return;
    const startRow = selectionRect.r0;
    const startCol = selectionRect.c0;
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
  }, [selectionRect, readOnlyReason, cellsByKey, writeCells]);

  const clearSelection = useCallback(() => {
    if (!selectionRect) return;
    const { r0, r1, c0, c1 } = selectionRect;
    const writes: CellWrite[] = [];
    for (let r = r0; r <= r1; r++) {
      for (let c = c0; c <= c1; c++) {
        const current = cellsByKey.get(keyOf(r, c));
        if (current) writes.push({ row: r, col: c, value: "", base_revision: current.revision });
      }
    }
    if (writes.length > 0) void writeCells(writes);
  }, [selectionRect, cellsByKey, writeCells]);

  // ---- 独自右クリックメニュー(ADR-0055 決定 6) ----

  const openCellMenu = useCallback(
    (event: ReactMouseEvent<HTMLTableCellElement>, row: number, col: number) => {
      event.preventDefault();
      if (editing) return;
      const anchor = resolveMergeAnchor(row, col);
      const inCurrentSelection =
        selectionRect !== null &&
        anchor.row >= selectionRect.r0 &&
        anchor.row <= selectionRect.r1 &&
        anchor.col >= selectionRect.c0 &&
        anchor.col <= selectionRect.c1;
      if (!inCurrentSelection) {
        setSelection({ anchor, focus: anchor });
      }
      setCellMenu({ x: event.clientX, y: event.clientY, row: anchor.row, col: anchor.col });
    },
    [editing, resolveMergeAnchor, selectionRect],
  );

  // ---- シート内検索(Ctrl+F、ADR-0055 決定 6) ----

  const searchMatches = useMemo(() => {
    const query = searchQuery.trim().toLowerCase();
    if (!query) return [] as CellPos[];
    return cells
      .filter((cell) => cell.value.toLowerCase().includes(query))
      .sort((a, b) => a.row - b.row || a.col - b.col)
      .map((cell) => ({ row: cell.row, col: cell.col }));
  }, [cells, searchQuery]);

  const searchMatchKeys = useMemo(
    () => new Set(searchMatches.map((m) => keyOf(m.row, m.col))),
    [searchMatches],
  );

  const currentSearchMatch =
    searchMatches.length > 0 ? searchMatches[searchIndex % searchMatches.length] : null;
  const currentSearchKey = currentSearchMatch ? keyOf(currentSearchMatch.row, currentSearchMatch.col) : null;

  const gotoSearchMatch = useCallback(
    (index: number) => {
      if (searchMatches.length === 0) return;
      const clamped = ((index % searchMatches.length) + searchMatches.length) % searchMatches.length;
      setSearchIndex(clamped);
      const target = searchMatches[clamped];
      const anchor = resolveMergeAnchor(target.row, target.col);
      setSelection({ anchor, focus: anchor });
      window.setTimeout(() => {
        cellRefs.current
          .get(keyOf(anchor.row, anchor.col))
          ?.scrollIntoView({ block: "nearest", inline: "nearest" });
      }, 0);
    },
    [searchMatches, resolveMergeAnchor],
  );

  // 検索文字列が変わるたびに先頭の一致へジャンプする
  useEffect(() => {
    if (!searchOpen) return;
    setSearchIndex(0);
    gotoSearchMatch(0);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [searchQuery, searchOpen]);

  // Ctrl+F で検索バーを開く/フォーカスする(ウインドウ全体で監視。SheetView
  // は「表」サブタブがアクティブなときだけマウントされているので、他画面の
  // 入力を横取りする心配はない)。
  useEffect(() => {
    const onKeyDown = (event: globalThis.KeyboardEvent) => {
      if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "f") {
        if (!activeSheetIdRef.current) return;
        event.preventDefault();
        setSearchOpen(true);
        window.setTimeout(() => searchInputRef.current?.focus(), 0);
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, []);

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
      // Undo(Ctrl+Z / Cmd+Z、ローカル、ADR-0055 決定 6)。編集中でないときだけ
      // 横取りする(input 自身のネイティブ undo は妨げない)。
      if ((event.ctrlKey || event.metaKey) && !event.shiftKey && event.key.toLowerCase() === "z") {
        event.preventDefault();
        undo();
        return;
      }
      if (readOnlyReason) return;
      if (event.key === "Enter") {
        event.preventDefault();
        startEdit(row, col, null);
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
        // ここでは preventDefault しない(ADR-0055 決定 6 追補 H-7a: IME
        // バグ修正)。詳細は startTypedEdit のコメント参照。
        startTypedEdit(row, col);
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
      startTypedEdit,
      writeCells,
      undo,
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
      const anchor = resolveMergeAnchor(row, col);
      if (event.shiftKey && selection) {
        setSelection({ anchor: selection.anchor, focus: anchor });
      } else {
        setSelection({ anchor, focus: anchor });
      }
      isSelectingRef.current = true;
    },
    [editing, selection, resolveMergeAnchor],
  );

  const handleCellMouseEnter = useCallback(
    (row: number, col: number) => {
      if (!isSelectingRef.current) return;
      const focus = resolveMergeAnchor(row, col);
      setSelection((prev) => (prev ? { anchor: prev.anchor, focus } : { anchor: focus, focus }));
    },
    [resolveMergeAnchor],
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

  const gridlines = activeSheet?.gridlines ?? true;
  const freezeRows = activeSheet?.freeze_rows ?? 0;
  const freezeCols = activeSheet?.freeze_cols ?? 0;

  // 固定枠(freeze panes)のピクセルオフセットは DOM 実測ベース
  // (measuredRowTopOffsets/measuredColLeftOffsets、上の useLayoutEffect
  // 参照、ADR-0055 決定 6 追補 H-7a)。
  const rowTopOffsets = measuredRowTopOffsets;
  const colLeftOffsets = measuredColLeftOffsets;

  /** 右クリックメニューの項目(ADR-0055 決定 6)。 */
  function buildCellMenuItems(menu: { row: number; col: number }): ContextMenuEntry[] {
    const disabled = readOnlyReason !== null;
    return [
      { label: t.sheet.ctxCopy, onClick: copySelectionToClipboard },
      { label: t.sheet.ctxPaste, onClick: () => void pasteIntoSelection(), disabled },
      { label: t.sheet.ctxClear, onClick: clearSelection, disabled },
      { separator: true },
      { label: t.sheet.toolbarBorderOuter, onClick: applyBorderOuter, disabled },
      { label: t.sheet.toolbarBorderGrid, onClick: applyBorderGrid, disabled },
      { label: t.sheet.toolbarBorderNone, onClick: applyBorderNone, disabled },
      { separator: true },
      {
        label: canUnmergeSelection ? t.sheet.ctxUnmerge : t.sheet.ctxMerge,
        onClick: toggleMerge,
        disabled:
          disabled ||
          (!canUnmergeSelection && (!selectionRect || isSingleCell(selectionRect))),
      },
      { separator: true },
      {
        label: t.sheet.ctxResetColWidth,
        onClick: () => void resetLayout("col", menu.col),
      },
      {
        label: t.sheet.ctxResetRowHeight,
        onClick: () => void resetLayout("row", menu.row),
      },
    ];
  }

  return (
    <div className="sheet" onContextMenu={(event) => event.preventDefault()}>
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
                onChange={(event) => debouncedTextColor(event.target.value)}
                onBlur={(event) => flushTextColor(event.target.value)}
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
                onChange={(event) => debouncedBgColor(event.target.value)}
                onBlur={(event) => flushBgColor(event.target.value)}
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

          <div className="sheet__toolbar-group">
            <button
              type="button"
              className="sheet__toolbar-btn"
              title={canUnmergeSelection ? t.sheet.toolbarUnmerge : t.sheet.toolbarMerge}
              disabled={
                readOnlyReason !== null ||
                !selectionRect ||
                (!canUnmergeSelection && isSingleCell(selectionRect))
              }
              onClick={toggleMerge}
            >
              {canUnmergeSelection ? t.sheet.toolbarUnmerge : t.sheet.toolbarMerge}
            </button>
          </div>

          <div className="sheet__toolbar-group">
            <button
              type="button"
              className={
                gridlines
                  ? "sheet__toolbar-btn sheet__toolbar-btn--active"
                  : "sheet__toolbar-btn"
              }
              title={t.sheet.toolbarGridlines}
              disabled={readOnlyReason !== null}
              onClick={() => updateSheetSettings({ gridlines: !gridlines })}
            >
              {t.sheet.toolbarGridlines}
            </button>
            <button
              type="button"
              className="sheet__toolbar-btn"
              title={t.sheet.toolbarFreeze}
              disabled={readOnlyReason !== null || !selectionRect}
              onClick={freezeAtSelection}
            >
              {t.sheet.toolbarFreeze}
            </button>
            <button
              type="button"
              className="sheet__toolbar-btn"
              title={t.sheet.toolbarUnfreeze}
              disabled={readOnlyReason !== null || (freezeRows === 0 && freezeCols === 0)}
              onClick={clearFreeze}
            >
              {t.sheet.toolbarUnfreeze}
            </button>
          </div>
        </div>
      )}

      {activeSheet && searchOpen && (
        <div className="sheet__search-bar">
          <input
            ref={searchInputRef}
            type="text"
            className="sheet__search-input"
            placeholder={t.sheet.searchPlaceholder}
            value={searchQuery}
            onChange={(event) => setSearchQuery(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === "Enter") {
                event.preventDefault();
                gotoSearchMatch(searchIndex + (event.shiftKey ? -1 : 1));
              } else if (event.key === "Escape") {
                event.preventDefault();
                setSearchOpen(false);
              }
            }}
          />
          <span className="sheet__search-count muted small">
            {searchMatches.length > 0
              ? t.sheet.searchCount(searchIndex + 1, searchMatches.length)
              : t.sheet.searchNoMatch}
          </span>
          <button
            type="button"
            className="button--icon"
            title={t.sheet.searchPrev}
            disabled={searchMatches.length === 0}
            onClick={() => gotoSearchMatch(searchIndex - 1)}
          >
            ▲
          </button>
          <button
            type="button"
            className="button--icon"
            title={t.sheet.searchNext}
            disabled={searchMatches.length === 0}
            onClick={() => gotoSearchMatch(searchIndex + 1)}
          >
            ▼
          </button>
          <button
            type="button"
            className="button--icon"
            title={t.common.close}
            onClick={() => setSearchOpen(false)}
          >
            ×
          </button>
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
          <table
            className={
              gridlines ? "sheet__table" : "sheet__table sheet__table--no-gridlines"
            }
          >
            <thead>
              <tr ref={theadRowRef}>
                <th className="sheet__corner" ref={cornerRef} />
                {colIndexes.map((c) => (
                  <th
                    key={c}
                    ref={(el) => {
                      if (el) colHeadElRefs.current.set(c, el);
                      else colHeadElRefs.current.delete(c);
                    }}
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
                <tr
                  key={r}
                  ref={(el) => {
                    if (el) rowElRefs.current.set(r, el);
                    else rowElRefs.current.delete(r);
                  }}
                  style={{ height: rowHeights.get(r) }}
                >
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
                    // 結合範囲内で左上以外のセルは td を描画しない
                    // (左上セルの colSpan/rowSpan がその領域を覆う)。
                    const merge = mergeCovered.get(key);
                    if (merge && (merge.row !== r || merge.col !== c)) return null;

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
                    const peersHere = presenceByCell.get(key);
                    const isSearchMatch = searchOpen && searchMatchKeys.has(key);
                    const isCurrentSearchMatch = searchOpen && currentSearchKey === key;
                    const isFrozenRow = r < freezeRows;
                    const isFrozenCol = c < freezeCols;
                    const cellStyle: CSSProperties = {
                      ...boxStyle(cell?.format, colWidths.get(c)),
                    };
                    if (isFrozenRow || isFrozenCol) {
                      cellStyle.position = "sticky";
                      if (isFrozenRow) cellStyle.top = rowTopOffsets[r];
                      if (isFrozenCol) cellStyle.left = colLeftOffsets[c];
                    }
                    if (peersHere && peersHere.length > 0) {
                      (cellStyle as Record<string, string>)["--presence-color"] =
                        presenceColor(peersHere[0].name);
                    }
                    return (
                      <td
                        key={c}
                        ref={(el) => {
                          if (el) cellRefs.current.set(key, el);
                          else cellRefs.current.delete(key);
                        }}
                        tabIndex={0}
                        rowSpan={merge?.row_span}
                        colSpan={merge?.col_span}
                        style={cellStyle}
                        className={[
                          "sheet__cell",
                          isFocusCell && "sheet__cell--selected",
                          inRange && !isFocusCell && "sheet__cell--in-range",
                          numeric && "sheet__cell--numeric",
                          isConflict && "sheet__cell--conflict",
                          (isFrozenRow || isFrozenCol) && "sheet__cell--frozen",
                          peersHere && peersHere.length > 0 && "sheet__cell--presence",
                          isSearchMatch && "sheet__cell--search-match",
                          isCurrentSearchMatch && "sheet__cell--search-current",
                        ]
                          .filter(Boolean)
                          .join(" ")}
                        onMouseDown={(event) => handleCellMouseDown(event, r, c)}
                        onMouseEnter={() => handleCellMouseEnter(r, c)}
                        onDoubleClick={() => startEdit(r, c, null)}
                        onKeyDown={(event) => handleGridKeyDown(event, r, c)}
                        onCopy={(event) => handleGridCopy(event, r, c)}
                        onPaste={(event) => handleGridPaste(event, r, c)}
                        onContextMenu={(event) => openCellMenu(event, r, c)}
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
                        {peersHere && peersHere.length > 0 && (
                          <span className="sheet__presence-badge">
                            {peersHere.map((peer) => peer.name).join(", ")}
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

      {activeSheet && (
        // 選択範囲の常設ステータスバー(ADR-0055 決定 6 追補 H-7a)。
        // グリッド枠外の左下に置き、単一セルでも常に表示することで、
        // 複数選択との切り替えでツールバーが出たり消えたりして
        // グリッドが上下にずれる問題を無くす。
        <div className="sheet__status-bar">
          <span className="sheet__status-bar-range">
            {selectionRect
              ? hasRangeSelection
                ? `${colLabel(selectionRect.c0)}${selectionRect.r0 + 1}:${colLabel(
                    selectionRect.c1,
                  )}${selectionRect.r1 + 1}`
                : `${colLabel(selectionRect.c0)}${selectionRect.r0 + 1}`
              : ""}
          </span>
        </div>
      )}

      {cellMenu && activeSheetId && (
        <ContextMenu
          x={cellMenu.x}
          y={cellMenu.y}
          onClose={() => setCellMenu(null)}
          items={buildCellMenuItems(cellMenu)}
        />
      )}
    </div>
  );
}
