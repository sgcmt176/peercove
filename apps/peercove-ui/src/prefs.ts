// アプリ全体の使い勝手設定(M3-13e)。localStorage 保存でこのマシンだけに
// 効く(外観テーマ M3-6 と同じ扱い)。ネットワークごとの設定(受信上限など)は
// 設定ファイル側 — SettingsDialog を参照。

export interface Prefs {
  /** OS 通知(参加・切断・チャット新着・ファイル受信)を出すか。 */
  notifications: boolean;
  /** チャットの URL からページ情報(OGP)を取得してプレビューを出すか。 */
  linkPreview: boolean;
  /** GitHub Releases へ最新版を確認するか。 */
  updateChecks: boolean;
}

const KEY = "peercove-prefs";

export const DEFAULT_PREFS: Prefs = {
  notifications: true,
  linkPreview: true,
  updateChecks: true,
};

export function loadPrefs(): Prefs {
  try {
    const raw = localStorage.getItem(KEY);
    if (!raw) return { ...DEFAULT_PREFS };
    return { ...DEFAULT_PREFS, ...(JSON.parse(raw) as Partial<Prefs>) };
  } catch {
    return { ...DEFAULT_PREFS };
  }
}

export function savePrefs(prefs: Prefs): void {
  try {
    localStorage.setItem(KEY, JSON.stringify(prefs));
  } catch {
    // 保存できなくても動作は続ける(次回起動で既定値に戻るだけ)
  }
}
