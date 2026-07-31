// i18n のエントリポイント。
//
// 日本語・英語に対応(M7 J-4)。UI からは `import { t } from "../i18n"` で
// 参照し、`t.xxx` を使う。文言の実体は ja.tsx / en.tsx にある。
//
// 言語の切り替えは「保存 → 画面を再読み込み」方式。`t` は下の通り静的
// export のままで(モジュール読み込み時に t.xxx を掴んでいる箇所が複数
// あるため、実行時の差し替えだと古い値が残る箇所が出る)、切り替え時は
// setLocale() が保存してから reload() する。theme.ts と同じ「初回は OS
// 設定、以降は明示的な選択をこのマシンに保存」という作法に揃えている。
//
// ── 別の言語を足すには ────────────────────────────────────────────────
//   1. ja.tsx を xx.tsx などにコピーし、値だけ訳す(キー構造は変えない)。
//      `const xx: Strings = { ... }` と型注釈を付ければ、訳し忘れ・キーの
//      食い違いを TypeScript がコンパイル時に弾いてくれる。
//   2. 下の catalogs に登録する。
//   3. localeLabel と、必要なら detectLocale() の判定を更新する。
// ────────────────────────────────────────────────────────────────────

import { ja } from "./ja";
import { en } from "./en";

/** 文言カタログの形。ja を正本とし、そこから型を導出する。 */
export type Strings = typeof ja;

/** 登録済みの言語。 */
export const catalogs = { ja, en } satisfies Record<string, Strings>;

export type LocaleId = keyof typeof catalogs;

/** 既定の言語。 */
export const defaultLocale: LocaleId = "ja";

const STORAGE_KEY = "peercove.locale";

/** 選択中の言語。保存済みが無ければ OS の言語設定から推定する。 */
export function detectLocale(): LocaleId {
  const stored = localStorage.getItem(STORAGE_KEY);
  if (stored === "ja" || stored === "en") return stored;
  // 初回は OS 設定に合わせる(以降は明示的な選択として保存される)
  return navigator.language.toLowerCase().startsWith("ja") ? "ja" : "en";
}

/** 現在保存されている言語(未保存なら OS 設定からの推定値)。 */
export function currentLocale(): LocaleId {
  return detectLocale();
}

/**
 * 言語を保存して画面を再読み込みする。`t` はモジュール読み込み時に固定
 * されるカタログのため、実行時の差し替えではなく再読み込みで反映する。
 */
export function setLocale(id: LocaleId): void {
  localStorage.setItem(STORAGE_KEY, id);
  window.location.reload();
}

/** 現在の言語の文言。UI はこれを参照する。 */
export const t: Strings = catalogs[detectLocale()];
