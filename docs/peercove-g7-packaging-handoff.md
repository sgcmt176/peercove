# M2-G7b パッケージング作業指示書(Opus 向け)

- **作成**: 2026-07-09(Fable)
- **前提**: M2-G7a(デーモンのサービス化)は実装済み。この文書は**残り半分 =
  パッケージング**の指示書です

## 実装状況(2026-07-09、Opus 追記)

| タスク | 状態 |
|---|---|
| 1. Windows MSI | ✅ 実装済み・**実機検証待ち**。MSI は WiX ネイティブ ServiceInstall ではなく**カスタムアクションで `daemon service-install/uninstall` を呼ぶ方式に変更**(ADR-0010 改訂)。ファイアウォールも CLI 内で処理 |
| 2. Ubuntu deb | ✅ 実装済み・**実機検証待ち**。Tauri の `files` + maintainer script(`packaging/deb/*.sh`)で systemd unit 同梱・自動起動。dpkg 再組みは不要だった |
| 3. wintun 同梱 + ライセンス | ✅ 実装済み。`packaging/licenses/`(本文は zip 由来・gitignore)、MSI が同梱、アプリにも謝辞フッター |
| 4. ZIP / tar(上級者向け) | ✅ 実装済み。`packaging/make-zip.ps1` / `make-tar.sh` |
| 5. UI 自動起動 | ⏸ **見送り**。デーモンはサービスで自動起動するため UI 自動起動の価値が低く、依存追加(要メンテ調査)を避けた。将来やるなら `tauri-plugin-autostart` |

**残りは実機検証のみ**(README「検証手順(M2-G7b: …)」)。検証で問題が出たら
下記の各タスクの指示に戻って修正。**MSI の方式変更は後日 Fable がレビュー予定**。

## 0. 最初に読むもの(この順で)

1. `CLAUDE.md` — リポジトリ全体のルール(商標・秘密情報・コミット前チェック)
2. `docs/roadmap.md` §5(難易度振り分け)と §7(ワークフロー)
3. `docs/peercove-m2-handoff.md` §9(確定事項。特に Q5 = wintun.dll 同梱可)
4. `docs/decisions.md` の **ADR-0010**(配布形態とサービス化の決定事項)

## 1. 完了済み(触らないでよい/変えてはいけない部分)

| 済んでいること | 場所 |
|---|---|
| Windows サービス実装(SCM ハンドシェイク・停止時クリーンアップ) | `crates/peercove-poc/src/service.rs` |
| `daemon service`(SCM エントリ)/ `service-install` / `service-uninstall` CLI | `crates/peercove-poc/src/main.rs` |
| systemd ユニットの単一ソース | `packaging/systemd/peercove-daemon.service` |
| 秘密鍵 ACL への SYSTEM 追加(Session 0 対応) | `crates/peercove-ops/src/secret.rs` |
| MSI 用 WiX フラグメント(下書き。§2 で検証して使う) | `apps/peercove-ui/src-tauri/windows/daemon-service.wxs` |
| サービス PoC の検証手順 | `README.md`「検証手順(M2-G7a)」 |

**変えてはいけないもの**: サービス名 `peercove-daemon`・起動引数 `daemon service`
(`service.rs` の定数と WiX フラグメントの両方に現れる。片方だけ変えると
インストール済み環境が壊れる)。`WgBackend` trait・`daemon.rs`・`control.rs`・
`backend/windows/device.rs` は今回のスコープ外(触る必要が出たら中断して
依頼者に「Fable に戻す」と報告すること — roadmap §5)。

## 2. タスク 1: Windows MSI

1. `tauri.conf.json` に WiX フラグメントを組み込む:
   ```json
   "bundle": { "windows": { "wix": {
     "fragmentPaths": ["windows/daemon-service.wxs"],
     "componentRefs": ["PeercoveDaemonComponent", "WintunDllComponent"]
   }}}
   ```
2. フラグメント内の `Source` パス 2 つを検証する(candle の作業ディレクトリが
   src-tauri 基準かはビルドして確かめる。違ったらパスを直してフラグメントの
   コメントも更新):
   - デーモン exe: `..\..\..\target\release\peercove-poc.exe`
     (**先に `cargo build --release -p peercove-poc` が必要**。ビルド手順として
     README に明記する)
   - `windows\wintun.dll`(§4 参照。リポジトリには無いので配置手順を README へ)
3. `npm run tauri build` で MSI を作る(WiX は Tauri が自動ダウンロード)
4. **UI 本体のインストールとの整合**: デーモン exe はフラグメントが所有するので
   `externalBin` は使わないこと(WiX の規則で ServiceInstall はサービス exe を
   KeyPath に持つ Component 内に必要 — フラグメントのコメント参照)

5. **ファイアウォールの受信許可ルール**(PoC で発覚した必須要件 — ADR-0010
   「Session 0 の追加要件」参照)。Session 0 のサービスには許可ダイアログが
   出ないため、ルールを作らないと inbound UDP が黙って遮断される。
   - ルール名は **`PeerCove Daemon`**(`service.rs` の `FIREWALL_RULE` と同じ。
     変えないこと)。対象 = インストールされたデーモン exe、dir=in、
     **UDP と TCP の 2 本**(UDP = WG 待受、TCP = トンネル内コントロール
     チャネル。TCP を忘れると「ping は通るのに台帳が来ない」になる)
   - 実現方法は要調査で 2 案: (a) WiX の FirewallExtension
     (`xmlns:fire` + `<fire:FirewallException>`。Tauri が WiX 拡張を
     渡せるかを確認)、(b) カスタムアクションで
     `netsh advfirewall firewall add rule ...` / アンインストール時に delete。
   - **アンインストールでルールが残らないこと**が受け入れ条件

受け入れ確認(依頼者に検証依頼する項目):
- インストール → サービス `peercove-daemon` が登録され RUNNING
- スタートメニューから PeerCove(UI)起動 → ホスト開始 → 疎通
  (疎通しない場合の第一容疑者はファイアウォールルールの不備)
- **通知の表示元が「PeerCove」になっている**(AppUserModelID がショートカット
  経由で登録されるため。dev では PowerShell 表示だった件の解消確認)
- アンインストール → サービス・ファイル・トンネル・ファイアウォールルールの
  残骸ゼロ(`sc.exe query` / `ipconfig` / `Get-NetFirewallRule` で確認)

## 3. タスク 2: Ubuntu deb

1. `tauri.conf.json` の `bundle.linux.deb` で以下を同梱:
   - `packaging/systemd/peercove-daemon.service` → `/usr/lib/systemd/system/`
   - `target/release/peercove-poc`(リリースビルド) → `/usr/bin/peercove-poc`
   (`files` マッピングで可能。Tauri 2 の deb 設定は context7 か公式ドキュメントで
   最新の書式を確認すること)
2. postinst / prerm 相当:
   - postinst: `systemctl daemon-reload && systemctl enable --now peercove-daemon`
   - prerm: `systemctl disable --now peercove-daemon`
   Tauri 2 の deb 設定にメンテナスクリプトのフィールドが**あるか要調査**
   (`preInstallScript` 等)。無ければ、(a) tauri build 後に `dpkg-deb -R/-b` で
   組み直すスクリプトを `packaging/` に置く、(b) デーモンだけ `cargo-deb` で
   別 deb にする(UI の deb が Depends で参照)のどちらかを選び、
   **選んだ方式と理由を ADR-0010 に追記**する
3. ビルドは Linux 実機(検証機)か `npm run tauri build -- --target ...` の
   クロスビルド可否を調査。無理なら「Linux 検証機でビルドする手順」を README に
   書けば十分(依頼者がそのまま実行できること)

受け入れ確認: `apt install ./peercove_*.deb` → サービス自動起動 → UI で疎通 →
`apt remove` → ユニット・バイナリ・トンネル残骸ゼロ。

## 4. タスク 3: wintun.dll の同梱と謝辞

- wintun.net の署名済み zip から `bin/amd64/wintun.dll` を
  `apps/peercove-ui/src-tauri/windows/wintun.dll` へ配置(**gitignore 済み**。
  コミットしない)
- ダウンロード元 URL・バージョン・配置手順を README のビルド手順に明記
- wintun の **Prebuilt Binaries License 全文**を同梱する(例:
  `packaging/licenses/wintun-LICENSE.txt` を作り MSI/deb でインストール先へ
  コピー)。M2 handoff Q5 の条件(無改変・Permitted API のみ)は満たしている

## 5. タスク 4: ZIP(上級者向け)

`packaging/make-zip.ps1`(Windows)/ `packaging/make-tar.sh`(Linux)を作る:
- 内容物: `peercove-poc(.exe)` + PeerCove UI 実行ファイル + `wintun.dll`
  (Windows のみ)+ 使い方の短い `README-portable.md`
- README-portable の骨子: (1) 管理者/root で `peercove-poc daemon
  service-install`(常駐する場合)か `daemon run`(その場だけ)、(2) UI を起動、
  (3) やめるときは `service-uninstall`
- ZIP では通知の表示元が PowerShell になる制約(ADR-0009)を明記

## 6. タスク 5(任意): UI のログイン時自動起動

`tauri-plugin-autostart` を調査して導入(設定画面にチェックボックス)。
crate の最終リリース日と issue の放置状況を確認してから採用すること
(CLAUDE.md の作業ルール)。時間があれば。無理に入れない。

## 7. 守ること(CLAUDE.md の再掲 + G7 固有)

- 製品名・crate 名・バイナリ名・サービス名に "WireGuard" を含めない
- 秘密鍵・PSK・トークンをログ・画面に出さない
- コミット前: ルートで `cargo fmt --check` と
  `cargo clippy --all-targets -- -D warnings`、
  `cargo-zigbuild clippy --workspace --all-targets --target x86_64-unknown-linux-gnu -- -D warnings`、
  UI を触ったら `apps/peercove-ui/src-tauri` で `cargo test` と `npm run build`
- **1 タスク終えるごとに README へ検証手順を追記してからコミット**し、
  実機検証が必要な箇所は「検証依頼」として依頼者の報告を待つ
- 環境依存の実疎通は自動テスト化しない(README の手順で人間が再現)
- アーキテクチャ判断(deb のスクリプト方式など)は ADR-0010 に追記
- Linux 検証機への同期は **git bundle**(README「Linux 検証機への同期」)。
  ファイルの手コピーは事故のもと(実際に起きた)

## 8. 引っかかったら

- サービスの挙動・SCM・wintun・デバイスループ・IPC プロトコルに手を入れたく
  なったら**中断して依頼者に報告**(Fable の担当領域)
- Tauri bundler の仕様が不明なときは context7 / 公式ドキュメントで最新を確認
  (バージョンは Tauri 2.x。学習データより新しい可能性が高い)
