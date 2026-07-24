// 日本の祝日(カレンダーの配色、ADR-0055 決定 4)。holidays-jp API
// (https://holidays-jp.github.io/api/v1/date.json)から取得し、端末ローカルへ
// 30 日キャッシュする。取得に失敗しても例外を投げない — 呼び出し側は常に
// Record を受け取り、キャッシュがあればそれを、無ければ空(週末色のみで
// 動作)を返す。エラーは console に出さない(祝日が取れないのは異常事態
// ではないため)。
//
// 取得は Tauri コマンド `fetch_holidays`(Rust 側 reqwest)経由 — UI の CSP
// (`default-src 'self'`)は外部オリジンへの fetch を許さないため。

import { invoke } from "@tauri-apps/api/core";

const STORAGE_KEY = "peercove-holidays-jp";
const TTL_MS = 30 * 24 * 60 * 60 * 1000; // 30日

/** yyyy-mm-dd → 祝日名。 */
export type HolidayMap = Record<string, string>;

interface CachedHolidays {
  fetchedAt: number;
  holidays: HolidayMap;
}

function loadCache(): CachedHolidays | null {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (!raw) return null;
    const parsed = JSON.parse(raw) as Partial<CachedHolidays>;
    if (
      typeof parsed.fetchedAt !== "number" ||
      typeof parsed.holidays !== "object" ||
      parsed.holidays === null
    ) {
      return null;
    }
    return parsed as CachedHolidays;
  } catch {
    return null;
  }
}

function saveCache(holidays: HolidayMap): void {
  try {
    const cached: CachedHolidays = { fetchedAt: Date.now(), holidays };
    localStorage.setItem(STORAGE_KEY, JSON.stringify(cached));
  } catch {
    // 保存できなくても今回取得した分はメモリ上のキャッシュで使える
  }
}

let memoryCache: HolidayMap | null = null;
let inFlight: Promise<HolidayMap> | null = null;

/**
 * 祝日マップを返す(yyyy-mm-dd → 祝日名)。有効なキャッシュがあればそれを
 * 即返し、無ければ 1 回だけ fetch する(呼び出しが重なっても 1 回に集約)。
 * 失敗時は既存キャッシュ(古くても)があればそれを、無ければ空を返す。
 */
export async function getHolidays(): Promise<HolidayMap> {
  if (memoryCache) return memoryCache;
  if (inFlight) return inFlight;

  const cached = loadCache();
  if (cached && Date.now() - cached.fetchedAt < TTL_MS) {
    memoryCache = cached.holidays;
    return memoryCache;
  }

  inFlight = (async () => {
    try {
      const data = await invoke<HolidayMap>("fetch_holidays");
      saveCache(data);
      memoryCache = data;
      return data;
    } catch {
      const fallback = cached?.holidays ?? {};
      memoryCache = fallback;
      return fallback;
    } finally {
      inFlight = null;
    }
  })();
  return inFlight;
}

/** "yyyy-mm-dd"(ローカル日付)。holidays-jp API のキー形式に合わせる。 */
export function holidayKey(date: Date): string {
  const pad = (value: number) => String(value).padStart(2, "0");
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}`;
}
