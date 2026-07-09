// i18n のエントリポイント。
//
// いまは日本語のみ。UI からは `import { t } from "../i18n"` で参照し、`t.xxx`
// を使う。文言の実体は ja.tsx にある。
//
// ── 別の言語を足すには ────────────────────────────────────────────────
//   1. ja.tsx を en.tsx などにコピーし、値だけ訳す(キー構造は変えない)。
//      `const en: Strings = { ... }` と型注釈を付ければ、訳し忘れ・キーの
//      食い違いを TypeScript がコンパイル時に弾いてくれる。
//   2. 下の catalogs に登録する。
//   3. 実行時に言語を切り替えるなら、`t` を固定 export から「選択された
//      カタログを返す」仕組み(状態＋再レンダー)に変える。現状は切り替え
//      UI が無いので、既定言語をそのまま束ねている。
// ────────────────────────────────────────────────────────────────────

import { ja } from "./ja";

/** 文言カタログの形。ja を正本とし、そこから型を導出する。 */
export type Strings = typeof ja;

/** 登録済みの言語。将来 en 等をここに足す。 */
export const catalogs = { ja } satisfies Record<string, Strings>;

export type LocaleId = keyof typeof catalogs;

/** 既定の言語。 */
export const defaultLocale: LocaleId = "ja";

/** 現在の言語の文言。UI はこれを参照する。 */
export const t: Strings = catalogs[defaultLocale];
