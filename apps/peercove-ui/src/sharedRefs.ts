// 共有オブジェクト参照 `@種別:id`(M5 F-5 Stage 4、ADR-0052 決定 1)。チャット
// 本文にそのまま書ける軽量トークンをカード表示するための汎用パーサ + 種別
// レジストリ。プロトコル変更なし(本文の一部。旧クライアントには文字列の
// まま見える)。種別を増やすときは SHARED_REF_KINDS に 1 エントリ足すだけで
// よい(例: 将来の schedule・sheet)。
//
// カードの内容は表示時に受信者自身の権限で解決する(メモはキャッシュ経由 =
// オフラインでも出る)。取得できなければ「アクセスできません」カードとし、
// タイトル等は一切出さない。**メモのタイトル・本文はログへ出さない**。
import { useEffect, useState } from "react";
import { ScheduleEvent, SheetMeta, api } from "./ipc";
import { t } from "./i18n";

export type SharedRefKind = "memo" | "schedule" | "sheet";

export interface SharedRefResolved {
  /** カードのタイトル(見出し)。 */
  title: string;
  /** 抜粋 1 行。 */
  excerpt: string;
}

interface SharedRefKindSpec {
  icon: string;
  /** 「アクセスできない◯◯」の◯◯部分(t.sharedRef.inaccessible へ渡す)。 */
  noun: string;
  resolve: (
    configPath: string,
    id: string,
  ) => Promise<SharedRefResolved | null>;
}

function firstBodyLine(body: string): string {
  const line = body.split("\n").find((l) => l.trim() !== "") ?? "";
  return line.trim().slice(0, 80);
}

// 共有スケジュール表(M6 G-1、ADR-0053)。`Get` op が無いため一覧から id を
// 引く。一覧は configPath ごとに短時間メモ化する(カードが並ぶ場面で 1 件
// ごとに list を叩かないため)。TTL 切れ後は次の解決で取り直す = リアルタイム
// 反映は `seq` を見ているカレンダー本体側に任せ、ここは近似で良い。
const SCHEDULE_LIST_TTL_MS = 5000;
const scheduleListCache = new Map<
  string,
  { events: ScheduleEvent[]; at: number }
>();

async function listScheduleEvents(configPath: string): Promise<ScheduleEvent[]> {
  const cached = scheduleListCache.get(configPath);
  const now = Date.now();
  if (cached && now - cached.at < SCHEDULE_LIST_TTL_MS) return cached.events;
  const reply = await api.sharedMemoOp(configPath, {
    op: "schedule",
    schedule: { op: "list" },
  });
  const events =
    reply.kind === "schedule" && reply.reply.kind === "events"
      ? reply.reply.events
      : [];
  scheduleListCache.set(configPath, { events, at: now });
  return events;
}

/** 予定の抜粋文字列(例 "7/28 14:00" / 終日 "7/28 終日")。 */
function scheduleExcerpt(event: ScheduleEvent): string {
  const start = new Date(event.start_unix_ms);
  const md = `${start.getMonth() + 1}/${start.getDate()}`;
  if (event.all_day) return t.schedule.excerptAllDay(md);
  const pad = (value: number) => String(value).padStart(2, "0");
  return t.schedule.excerptTimed(md, `${pad(start.getHours())}:${pad(start.getMinutes())}`);
}

// 共有シート(M6 G-2、ADR-0054)。`Get` op が無いため一覧から id を引く
// (schedule と同じ TTL メモ化)。
const SHEET_LIST_TTL_MS = 5000;
const sheetListCache = new Map<string, { sheets: SheetMeta[]; at: number }>();

async function listSheets(configPath: string): Promise<SheetMeta[]> {
  const cached = sheetListCache.get(configPath);
  const now = Date.now();
  if (cached && now - cached.at < SHEET_LIST_TTL_MS) return cached.sheets;
  const reply = await api.sharedMemoOp(configPath, {
    op: "sheet",
    sheet: { op: "list" },
  });
  const sheets =
    reply.kind === "sheet" && reply.reply.kind === "sheets"
      ? reply.reply.sheets
      : [];
  sheetListCache.set(configPath, { sheets, at: now });
  return sheets;
}

/** 対応している種別のレジストリ。増やすときはここへ 1 エントリ足すだけでよい。 */
const SHARED_REF_KINDS: Record<SharedRefKind, SharedRefKindSpec> = {
  memo: {
    icon: "📝",
    noun: t.sharedRef.nounMemo,
    resolve: async (configPath, id) => {
      const reply = await api.sharedMemoOp(configPath, { op: "get", id });
      if (reply.kind !== "memo") return null;
      return {
        title: reply.memo.title,
        excerpt: firstBodyLine(reply.memo.body),
      };
    },
  },
  schedule: {
    icon: "📅",
    noun: t.sharedRef.nounSchedule,
    resolve: async (configPath, id) => {
      const events = await listScheduleEvents(configPath);
      const event = events.find((e) => e.id === id);
      if (!event) return null;
      return { title: event.title, excerpt: scheduleExcerpt(event) };
    },
  },
  sheet: {
    icon: "📊",
    noun: t.sharedRef.nounSheet,
    resolve: async (configPath, id) => {
      const sheets = await listSheets(configPath);
      const sheet = sheets.find((s) => s.id === id);
      if (!sheet) return null;
      return {
        title: sheet.name,
        excerpt: t.memo.updatedAt(new Date(sheet.updated_at).toLocaleString()),
      };
    },
  },
};

function isKnownKind(kind: string): kind is SharedRefKind {
  return Object.prototype.hasOwnProperty.call(SHARED_REF_KINDS, kind);
}

export function sharedRefIcon(kind: SharedRefKind): string {
  return SHARED_REF_KINDS[kind].icon;
}

export function sharedRefNoun(kind: SharedRefKind): string {
  return SHARED_REF_KINDS[kind].noun;
}

/** チャットへ貼る参照子の文字列(共有メモの「リンクをコピー」用)。 */
export function sharedRefToken(kind: SharedRefKind, id: string): string {
  return `@${kind}:${id}`;
}

export interface SharedRefTokenValue {
  kind: SharedRefKind;
  id: string;
}

export type SharedRefPart =
  | { type: "text"; value: string }
  | { type: "ref"; token: SharedRefTokenValue };

// 種別:id(id は 16 進英数字)。id の後ろが英数字だとトークンの境界が曖昧
// なので \b で区切る(例: @memo:abc123z のような不完全な id には反応しない)。
const SHARED_REF_RE = /@([a-zA-Z][a-zA-Z0-9]*):([0-9a-fA-F]+)\b/g;

/** 本文を `@種別:id` トークンと地の文へ分割する(未登録の種別はただの文字列のまま)。 */
export function splitSharedRefs(text: string): SharedRefPart[] {
  const parts: SharedRefPart[] = [];
  const re = new RegExp(SHARED_REF_RE.source, "g");
  let last = 0;
  let match: RegExpExecArray | null;
  while ((match = re.exec(text)) !== null) {
    const kind = match[1].toLowerCase();
    if (!isKnownKind(kind)) continue; // 未登録種別はテキストとして残す
    if (match.index > last) {
      parts.push({ type: "text", value: text.slice(last, match.index) });
    }
    parts.push({ type: "ref", token: { kind, id: match[2] } });
    last = match.index + match[0].length;
  }
  if (last < text.length) {
    parts.push({ type: "text", value: text.slice(last) });
  }
  return parts;
}

// 解決結果は configPath::種別:id ごとに使い回す(表示のたびに引き直さない)。
const resolveCache = new Map<string, SharedRefResolved | null>();
const resolvePending = new Map<string, Promise<SharedRefResolved | null>>();

/** 参照子カードの内容を解決する(表示時に自分の権限で。失敗・権限なしは null)。 */
export function useSharedRefResolve(
  configPath: string,
  token: SharedRefTokenValue,
): SharedRefResolved | null | undefined {
  const key = `${configPath}::${token.kind}:${token.id}`;
  const [data, setData] = useState<SharedRefResolved | null | undefined>(() =>
    resolveCache.get(key),
  );
  useEffect(() => {
    if (resolveCache.has(key)) {
      setData(resolveCache.get(key));
      return;
    }
    let alive = true;
    let pending = resolvePending.get(key);
    if (!pending) {
      pending = SHARED_REF_KINDS[token.kind]
        .resolve(configPath, token.id)
        .catch(() => null);
      resolvePending.set(key, pending);
    }
    void pending.then((resolved) => {
      resolveCache.set(key, resolved);
      resolvePending.delete(key);
      if (alive) setData(resolved);
    });
    return () => {
      alive = false;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [key]);
  return data;
}
