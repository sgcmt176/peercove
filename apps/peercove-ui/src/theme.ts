// テーマ切替(M3-6)。「システムに合わせる / ライト / ダーク」の 3 状態。
//
// 適用は <html data-theme="..."> の付け替えだけで行い、色の実体は styles.css の
// CSS 変数に閉じる。設定はこのマシンの見た目の好みであってネットワーク設定では
// ないので、TOML ではなく localStorage に保存する。

export const THEMES = ["system", "light", "dark"] as const;
export type Theme = (typeof THEMES)[number];

const STORAGE_KEY = "peercove.theme";

export function loadTheme(): Theme {
  const stored = localStorage.getItem(STORAGE_KEY);
  return (THEMES as readonly string[]).includes(stored ?? "")
    ? (stored as Theme)
    : "system";
}

/** テーマを適用して保存する。"system" は data-theme を外して OS 設定に委ねる。 */
export function applyTheme(theme: Theme): void {
  if (theme === "system") {
    delete document.documentElement.dataset.theme;
    localStorage.removeItem(STORAGE_KEY);
  } else {
    document.documentElement.dataset.theme = theme;
    localStorage.setItem(STORAGE_KEY, theme);
  }
}

/** トグルボタン用: システム → ライト → ダーク → システム … と巡回する。 */
export function nextTheme(theme: Theme): Theme {
  return THEMES[(THEMES.indexOf(theme) + 1) % THEMES.length];
}
