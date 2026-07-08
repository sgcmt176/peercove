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
npm install     # 初回のみ

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
src/                        React(TypeScript)
  ipc.ts                    UI 用の型・コマンドラッパ・表示ヘルパ
  notify.ts                 メンバー参加/切断の差分検知と OS 通知(G6)
  App.tsx                   接続状態で画面を出し分け + ヘッダの設定/ログボタン
  components/StartView.tsx  待機中: ホスト開始 / トークンで参加
  components/TunnelView.tsx 稼働中: メンバー一覧・RTT・招待・削除・名前変更・切断
  components/InviteDialog.tsx 招待の発行と QR 表示(発行直後のみ)
  components/SettingsDialog.tsx 自分側の設定編集(表示名 / ポート / MTU / endpoint)
  components/LogsDialog.tsx デーモンのログビュー(1 秒間隔で差分取得)
  components/Modal.tsx      モーダルと確認ダイアログ
src-tauri/                  Rust(Tauri バックエンド)
  src/lib.rs                invoke コマンド(デーモン操作 + 設定ファイル操作)
  src/dto.rs                IPC 応答 → UI DTO の変換(camelCase)
  src/tray.rs               トレイ常駐(閉じても終了しない、メニューで復帰・終了)
  tauri.conf.json           ウィンドウ・CSP・バンドル設定
  capabilities/             権限(core:default + ファイル選択 + クリップボード)
scripts/make-icon.mjs       アプリアイコンの生成(外部素材に依存しない)
```

UI の役割は 2 つに分かれます(ADR-0007 / 0008):

| 操作 | 経路 | 権限 |
|---|---|---|
| トンネルの開始・停止・状態取得 | ローカル IPC → デーモン | デーモンが管理者/root |
| init / invite / join / メンバー管理 | `peercove-ops` を直接呼ぶ | UI のユーザー権限 |

設定ファイルを書き換えるだけの操作をデーモンに投げないのは、特権プロセスを
ファイルの書き手にして権限昇格の面を広げないためです。実行中のトンネルは
5 秒ごとの再読込で自動追随します。

**このアプリはルートの cargo ワークスペースから独立しています**
(`src-tauri/Cargo.toml` の空の `[workspace]` と、ルートの `exclude = ["apps"]`)。
`cargo test --workspace` が WebView のビルドに巻き込まれないようにするためです。

## 設計メモ

- `peercove-core::ipc` の serde 表現(内部タグ)を TypeScript から直接なぞらず、
  `src-tauri/src/dto.rs` で **UI 用 DTO(camelCase)** に変換しています。
  プロトコル表現の変更が UI に波及しないようにするためで、
  DTO の形は `dto.rs` のユニットテストで固定しています
- **招待トークンは発行直後のダイアログでしか表示しません**(ADR-0008)。
  トークンはメンバーの秘密鍵を含む(ADR-0005)ため、画面やファイルに残し続けません。
  取り消しはメンバー一覧からの削除で行います
- 設定ファイルの既定の置き場所はアプリのデータディレクトリ
  (Windows `%APPDATA%\app.peercove.desktop\`、Linux `~/.config/app.peercove.desktop/`)。
  「別の設定ファイルを使う」で任意のパスも選べます
- **ログはデーモンのメモリから IPC で取り出します**(ADR-0009)。ファイルに
  書かないので、特権プロセスが作ったファイルを読む権限問題も、消し忘れの
  残骸もありません。表示できるのはデーモンが記録しているレベルまでです
  (`--log-level warn` で起動していれば warn 以上だけ)
- **RTT はコントロールチャネルの ping/pong で測ります**(ADR-0009)。ICMP は
  raw socket に特権が要るため使いません。台帳には載せません(5 秒ごとに値が
  変わり、全メンバーへの再配布が走ってしまうため)
- **参加/切断の通知は UI が status の差分から出します**。デーモンは状態を
  持つだけです(G7 で Windows サービス化すると Session 0 からデスクトップへ
  通知できないため)。ウィンドウを閉じてもトレイに常駐して通知を出し続けます。
  通知の送出は Rust 側の `notify` コマンドが行い、frontend は
  `@tauri-apps/plugin-notification` に依存しません(npm 依存を増やさず、
  デスクトップでは不要な許可の問い合わせも避けるため)
- **設定編集で触れるのは「自分側の設定」だけ**です。メンバーの追加・削除・改名は
  メンバー一覧側の操作。MTU・待受ポート・ホストの endpoint はインターフェース
  生成時に決まるため、保存しても**再接続するまで反映されません**(UI が明示します)

## アイコンの再生成

元画像(1024x1024 の master)は `src-tauri/icons/source.png`。差し替えたら:

```bash
npx tauri icon src-tauri/icons/source.png            # 各サイズを生成
rm -rf src-tauri/icons/android src-tauri/icons/ios   # モバイルは対象外
```

`src/tray.rs` は `icons/32x32.png` を `include_bytes!` で埋め込んでいるので、
再生成すればトレイアイコンも一緒に入れ替わります。
