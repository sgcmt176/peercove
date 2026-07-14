import { UpdateInfo, api } from "./ipc";

const KEY = "peercove-update-check";
const CACHE_MS = 24 * 60 * 60 * 1000;

interface CachedUpdate {
  checkedAt: number;
  info: UpdateInfo;
}

export function loadCachedUpdate(): CachedUpdate | null {
  try {
    const raw = localStorage.getItem(KEY);
    if (!raw) return null;
    const cached = JSON.parse(raw) as Partial<CachedUpdate>;
    if (typeof cached.checkedAt !== "number" || !cached.info) return null;
    return cached as CachedUpdate;
  } catch {
    return null;
  }
}

export async function checkForUpdate(
  currentVersion: string,
  force = false,
): Promise<UpdateInfo> {
  const cached = loadCachedUpdate();
  if (
    !force &&
    cached?.info.currentVersion === currentVersion &&
    Date.now() - cached.checkedAt < CACHE_MS
  ) {
    return cached.info;
  }
  const info = await api.checkUpdate();
  try {
    localStorage.setItem(
      KEY,
      JSON.stringify({ checkedAt: Date.now(), info } satisfies CachedUpdate),
    );
  } catch {
    // キャッシュできなくても今回の結果は表示する。
  }
  return info;
}

export function clearUpdateCache(): void {
  try {
    localStorage.removeItem(KEY);
  } catch {
    // localStorage を使えない環境でも設定変更自体は続ける。
  }
}
