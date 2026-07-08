# peercove-ui(M2 デスクトップ UI)

Tauri 2 + React のデスクトップ UI。**非特権**で動き、管理者/root のデーモンを
ローカル IPC で操作します(ADR-0007)。

## 必要環境

- Node.js 20+ / npm(開発機は Node 24 で確認)
- Rust stable + 各 OS の Tauri 前提条件
  - Windows: WebView2(Windows 11 は標準搭載)+ Visual Studio C++ ビルドツール
  - Ubuntu: `sudo apt install libwebkit2gtk-4.1-dev build-essential curl wget file libxdo-dev libssl-dev libayatana-appindicator3-dev librsvg2-dev`

## 開発

```bash
cd apps/peercove-ui
npm install

# 別ターミナルでデーモンを起動しておく(管理者 / sudo)
#   peercove-poc daemon run

npm run tauri dev     # UI をホットリロードで起動
npm run build         # 型チェック + フロントエンドのビルド
npm run tauri build   # 配布用バイナリ(M2-G7 で本格対応)
```

デーモンが起動していなくても UI は立ち上がり、接続方法を案内します。
デーモンを起動すると数秒で状態表示に切り替わります(2 秒間隔でポーリング)。

## 構成

```
src/                  React(TypeScript)
  ipc.ts              UI 用の型と表示ヘルパ
  App.tsx             状態表示・メンバー一覧・ピア統計
src-tauri/            Rust(Tauri バックエンド)
  src/lib.rs          invoke コマンド(daemon_status)
  src/dto.rs          IPC 応答 → UI DTO の変換(camelCase)
  tauri.conf.json     ウィンドウ・CSP・バンドル設定
  capabilities/       権限(core:default のみ)
scripts/make-icon.mjs アプリアイコンの生成(外部素材に依存しない)
```

**このアプリはルートの cargo ワークスペースから独立しています**
(`src-tauri/Cargo.toml` の空の `[workspace]` と、ルートの `exclude = ["apps"]`)。
`cargo test --workspace` が WebView のビルドに巻き込まれないようにするためです。

## 設計メモ

- `peercove-core::ipc` の serde 表現(内部タグ)を TypeScript から直接なぞらず、
  `src-tauri/src/dto.rs` で **UI 用 DTO(camelCase)** に変換しています。
  プロトコル表現の変更が UI に波及しないようにするためで、
  DTO の形は `dto.rs` のユニットテストで固定しています
- 招待・参加・削除はデーモンを介さず設定ファイル操作で行う設計(ADR-0007)なので、
  UI もそれらは自前で実行します(M2-G3/G4 で実装)

## アイコンの再生成

```bash
node scripts/make-icon.mjs                    # src-tauri/icons/source.png
npx tauri icon src-tauri/icons/source.png     # 各サイズを生成
rm -rf src-tauri/icons/android src-tauri/icons/ios   # モバイルは対象外
```
