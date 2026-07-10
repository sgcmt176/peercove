// テーマ切替(M3-6)。「ライト ⇄ ダーク」の 2 状態。
// (「システムに合わせる」は 3 状態だとクリックしても見た目が変わらない
// タイミングがあり分かりづらい、との検証フィードバックで廃止。
// 初回起動時だけ OS 設定を初期値にする)
//
// 適用は <html data-theme="..."> の付け替えだけで行い、色の実体は styles.css の
// CSS 変数に閉じる。設定はこのマシンの見た目の好みであってネットワーク設定では
// ないので、TOML ではなく localStorage に保存する。

export type Theme = "light" | "dark";

const STORAGE_KEY = "peercove.theme";

export function loadTheme(): Theme {
  const stored = localStorage.getItem(STORAGE_KEY);
  if (stored === "light" || stored === "dark") return stored;
  // 初回は OS 設定に合わせる(以降は明示的な選択として保存される)
  return matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light";
}

export function applyTheme(theme: Theme): void {
  document.documentElement.dataset.theme = theme;
  localStorage.setItem(STORAGE_KEY, theme);
}

export function nextTheme(theme: Theme): Theme {
  return theme === "light" ? "dark" : "light";
}
