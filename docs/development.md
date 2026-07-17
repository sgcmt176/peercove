# 開発者向けガイド(ビルド・配布物)

PeerCove をソースからビルドする手順、Linux 検証機への同期、インストーラ
(MSI / deb / ポータブル ZIP)の作り方をまとめます。

- コーディング規約・作業ルールは [CLAUDE.md](../CLAUDE.md)
- 全体像・ロードマップは [roadmap.md](roadmap.md)、技術判断は [decisions.md](decisions.md)
- 実機での動作確認手順は [verification.md](verification.md)

## 必要環境

### 共通

- [Rust(stable)](https://rustup.rs/) — `rustup` でインストール

### Windows 10/11

- Visual Studio(C++ ビルドツール)— rustup のインストール時に案内されます
- トンネル操作(G-1 以降)は **管理者として実行した** PowerShell / ターミナルが必要です
- **wintun.dll**(TUN ドライバ)
  1. <https://www.wintun.net> から wintun のzipをダウンロード
  2. zip 内の `bin/amd64/wintun.dll` を `peercove.exe` と同じフォルダ
     (通常 `target\debug\`)にコピー

### Ubuntu 22.04+

- `build-essential`(`sudo apt install build-essential`)
- カーネル WireGuard モジュール(Ubuntu 22.04+ は標準搭載)
- トンネル操作(G-1 以降)は `sudo` が必要です
- **デスクトップ UI をビルドする場合のみ**: トレイ常駐(M2-G6)に
  `sudo apt install libayatana-appindicator3-dev` が必要です
  (GNOME でトレイアイコンを出すには AppIndicator 拡張も要りますが、
  Ubuntu では既定で有効です)

## ビルド

```bash
cargo build --workspace
```

### Linux 検証機への同期(git bundle)

このリポジトリはまだリモート未設定です(roadmap C-1)。Linux VM へは
**git bundle で履歴ごと**同期してください。ファイルの手コピーは
`package.json` と `src/` の版がずれてビルド不能になります(実際に起きました):

```bash
# Windows 側: 共有フォルダへバンドルを書き出す
git bundle create D:/Development/VirtualBoxShare/peercove.bundle main

# Linux 側: バンドルから取り込んで作業ツリーを揃える
cd ~/workspaces/peercove
git fetch /media/sf_VirtualBoxShare/peercove.bundle main  # マウント先は ls /media で確認
git log --oneline -1 FETCH_HEAD   # Windows 側の最新コミットが見えること
git reset --hard FETCH_HEAD       # 手元の混合状態を捨てて揃える
git clean -fd apps crates         # 追跡外の残骸を掃除(鍵・*.toml は ignored なので消えない)
git log --oneline -1              # 揃ったことを確認
```

> `reset --hard` は追跡ファイルへのローカル変更を捨てます。開発はすべて
> Windows 側で行っている前提です(検証用の鍵・設定は ignored なので残ります)。

テスト・lint:

```bash
cargo test -p peercove-core
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

### インストーラのビルド(M2-G7b)

配布物は **インストーラ主(Windows MSI / Ubuntu deb)+ ZIP(上級者向け)**
です(ADR-0010)。再配布素材(wintun)はリポジトリに含めないので、
ビルド前に手で配置します。

#### 事前準備(wintun。Windows インストーラ / ZIP に必要)

<https://www.wintun.net> の署名済み zip(現行 0.14.1)を展開し、**同じ zip の版**から
**2 ファイルとも**下記の名前ぴったりで配置します(どちらか欠けると MSI ビルドが
`LGHT0103 The system cannot find the file` で失敗します):

```powershell
# DLL 本体
copy <展開先>\bin\amd64\wintun.dll   apps\peercove-ui\src-tauri\windows\wintun.dll
# ライセンス本文(同梱義務)。zip 内は LICENSE.txt。
# 置き先の名前は wintun-LICENSE.txt(× LICENCE。綴りは S)
copy <展開先>\LICENSE.txt            packaging\licenses\wintun-LICENSE.txt
```

どちらも gitignore 済み(コミットしない)。詳細は
[packaging/licenses/README.md](../packaging/licenses/README.md)。

#### Windows MSI

```powershell
# 1) デーモンのリリースビルド(MSI が同梱・サービス登録する)
cargo build --release -p peercove-cli
# 2) 上の「事前準備」で wintun.dll と wintun-LICENSE.txt を配置済みにする
# 3) UI + MSI をビルド(WiX v3 は Tauri が自動取得)
cd apps\peercove-ui
npm install
npm run tauri build
#   → src-tauri\target\release\bundle\msi\PeerCove_0.1.0_x64_ja-JP.msi
```

インストーラは**日本語**です(`tauri.conf.json` の `wix.language = ja-JP`。
標準ダイアログは WiX の ja-jp、独自文言は `windows/wix-ja-JP.wxl` で日本語化)。
`bundle.targets` は `["msi", "deb"]` に絞ってあります(NSIS はサービス登録の
WiX フラグメントが効かず、デーモンが登録されない壊れたインストーラになるため。
ポータブル版は別途 `packaging/make-*.sh`)。

MSI は UI 本体に加え、`peercove.exe` / `wintun.dll` / `wintun-LICENSE.txt` を
同梱し、インストール時に `daemon service-install`(サービス登録 + ファイア
ウォール許可)、アンインストール時に `daemon service-uninstall` を呼びます
(WiX フラグメント `windows/daemon-service.wxs`。方式は ADR-0010)。

#### Ubuntu deb

**deb は Linux 上でしかビルドできません**(`dpkg-deb` が要る)。検証機で:

```bash
# 前提(トレイのビルド): sudo apt install libayatana-appindicator3-dev
cargo build --release -p peercove-cli      # deb が /usr/bin/peercove に入れる
cd apps/peercove-ui
npm install
npm run tauri build
#   → src-tauri/target/release/bundle/deb/PeerCove_0.1.0_amd64.deb
```

deb は UI 本体に加え、`peercove` を `/usr/bin/` へ、systemd ユニットを
`/usr/lib/systemd/system/peercove-daemon.service` へ入れ、postinst で
`systemctl enable --now peercove-daemon`、prerm で `disable --now` します
(`packaging/deb/*.sh`、`packaging/systemd/peercove-daemon.service`)。
Linux は wintun 不要(カーネル WireGuard)なので DLL/ライセンスは同梱しません。

#### ZIP / tar.gz(上級者向けポータブル版)

インストーラを使わない配布。中身はバイナリ + `wintun.dll`(Windows)+
使い方(`packaging/README-portable.md`)で、セットアップは手動です
(`daemon service-install` か `daemon run`)。上の MSI / deb 用のビルドを
済ませてから、組み立てスクリプトを実行します:

```powershell
# Windows: packaging\dist\PeerCove-portable-windows-x64.zip
powershell -ExecutionPolicy Bypass -File packaging\make-zip.ps1
```

```bash
# Linux: packaging/dist/PeerCove-portable-linux-x64.tar.gz
sh packaging/make-tar.sh
```

#### 第三者ライセンス謝辞(THIRD-PARTY-NOTICES.txt)

配布物には依存(Rust + npm)のライセンス表示を集約したファイルを同梱します
(方針は [packaging/licenses/README.md](../packaging/licenses/README.md))。
UI の依存を入れた状態(`npm ci` 済み)で:

```bash
cargo install cargo-about --features cli   # 初回のみ(cli フィーチャ必須)
sh packaging/make-notices.sh
#   → packaging/dist/THIRD-PARTY-NOTICES.txt
```

ポータブル版の組み立て(`make-zip.ps1` / `make-tar.sh`)は、この生成物があれば
自動で同梱します(無ければ警告して続行)。MSI / deb への同梱は現状は手動で、
生成した `THIRD-PARTY-NOTICES.txt` をインストール先の `licenses\`(Windows)/
`/usr/share/doc/peercove/`(deb)へ含める運用です(自動化は今後の課題)。

## リリース手順(タグ + バイナリ添付)

初回リリースは手動です(コード署名・リリース自動化 CI は未整備。§下記の注意)。
Windows 機と Linux 機の両方でビルドする必要があります(MSI は Windows、deb は
Linux でしか作れないため)。

### 1. バージョンを上げる

`0.1.0` を新バージョンに揃えます(4 か所。ずれるとインストーラのファイル名と
UI 表示が食い違います):

```
Cargo.toml                              … [workspace.package] version
apps/peercove-ui/package.json           … "version"
apps/peercove-ui/src-tauri/tauri.conf.json … "version"
apps/peercove-ui/src-tauri/Cargo.toml   … [package] version
```

ゲート(`cargo fmt --check` / `clippy` / `test`、UI の `npm run build`)を通し、
コミットしておきます。

### 2. 成果物をビルドする

**Windows 機**(MSI + ポータブル ZIP):

```powershell
# 事前準備で wintun.dll / wintun-LICENSE.txt を配置(上の「事前準備」参照)
cargo build --release -p peercove-cli
cd apps\peercove-ui; npm ci; npm run tauri build       # → MSI
cd ..\..
sh packaging/make-notices.sh                            # → THIRD-PARTY-NOTICES.txt
powershell -ExecutionPolicy Bypass -File packaging\make-zip.ps1   # → ZIP
```

**Linux 機**(deb + ポータブル tar.gz):

```bash
sudo apt install libayatana-appindicator3-dev   # 初回のみ(トレイのビルド)
cargo build --release -p peercove-cli
cd apps/peercove-ui && npm ci && npm run tauri build   # → deb
cd ../..
sh packaging/make-notices.sh                           # → THIRD-PARTY-NOTICES.txt
sh packaging/make-tar.sh                                # → tar.gz
```

集まる成果物(合計 5 点):

- `PeerCove_<版>_x64_ja-JP.msi`(Windows インストーラ)
- `PeerCove_<版>_amd64.deb`(Ubuntu インストーラ)
- `PeerCove-portable-windows-x64.zip`
- `PeerCove-portable-linux-x64.tar.gz`
- `THIRD-PARTY-NOTICES.txt`

### 3. タグを打って push する

```bash
git tag -a v<版> -m "PeerCove v<版>"
git push origin v<版>
```

### 4. GitHub Release を作成して添付する

[gh CLI](https://cli.github.com/) を使う場合(推奨):

```bash
gh release create v<版> \
  --title "PeerCove v<版>" \
  --notes "変更点をここに" \
  PeerCove_<版>_x64_ja-JP.msi \
  PeerCove_<版>_amd64.deb \
  PeerCove-portable-windows-x64.zip \
  PeerCove-portable-linux-x64.tar.gz \
  THIRD-PARTY-NOTICES.txt
```

gh を使わない場合は GitHub の «Releases» → «Draft a new release» でタグ `v<版>` を
選び、上記ファイルをドラッグして公開します。README のインストール節は Releases
からの入手を前提にしているので、初回リリース後はリンクが有効になります。

> **注意(未整備・既知の割り切り)**:
> - **コード署名なし**。Windows は SmartScreen の警告、Ubuntu は未署名 deb の
>   警告が出ます(利用者に案内が必要)。署名は将来対応(ADR-0010 で言及)。
> - **リリースビルドの自動化 CI は無し**。上記はすべて手元での手動ビルドです。
>   タグ push で MSI/deb を自動ビルドして添付する GitHub Actions ワークフローは
>   今後の課題(Tauri のクロスビルド + 署名の設計が必要)。
> - **バージョンの一斉更新スクリプトは無し**。上の 4 ファイルを手で揃えます。

## コミット前チェック(ゲート)

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --workspace          # Windows ではデバイスのループバックテストも走る

# Windows 開発機から Linux 側のコンパイル/lint を検証する場合(要 zig + cargo-zigbuild)
cargo-zigbuild clippy --workspace --all-targets --target x86_64-unknown-linux-gnu -- -D warnings
```

UI(`apps/peercove-ui`)はルートのワークスペースから独立しています。UI を変更したら
`apps/peercove-ui/src-tauri` で `cargo fmt --check` / `cargo clippy` / `cargo test` と、
`apps/peercove-ui` で `npm run build` を別途通してください。

## 依存ライセンスの確認

PeerCove 本体は MIT OR Apache-2.0(デュアル)です。依存 crate に強いコピーレフト
(GPL / AGPL 等)が混ざっていないかは [cargo-license](https://crates.io/crates/cargo-license)
で確認できます:

```bash
cargo install cargo-license   # 初回のみ

# ルート(CLI/デーモン/コア)の依存ライセンスを種別ごとに一覧
cargo license
cargo license | awk -F'(' '{print $1}' | sort -u   # 種別だけ重複排除で見る

# UI(Tauri)側は独立ワークスペースなので個別に
cd apps/peercove-ui/src-tauri && cargo license
```

現状、単独のコピーレフトは **MPL-2.0(弱い・ファイル単位)** のみで、GPL/AGPL は
ありません。方針と配布時の第三者謝辞の扱いは
[packaging/licenses/README.md](../packaging/licenses/README.md) を参照。

CI では [cargo-deny](https://crates.io/crates/cargo-deny) の `licenses` チェックが
毎 push 走ります(`deny.toml`。GPL / AGPL 等が混ざると失敗)。

## 依存の脆弱性チェック

3 層で守っています:

1. **cargo-deny の advisories チェック(CI・毎 push)** — RustSec advisory DB の
   既知脆弱性・メンテ終了告知を照合します。実行時リスクが無い/上流待ちと判断した
   ものは `deny.toml` の `[advisories].ignore` に**理由つき**で記録してあります
   (新しい advisory が出ると CI が落ちるので、その都度判断して修正か ignore を選ぶ)。
   ローカル実行:

   ```bash
   cargo deny check advisories
   cargo deny --manifest-path apps/peercove-ui/src-tauri/Cargo.toml --config deny.toml check advisories
   ```

2. **npm audit(CI・毎 push)** — フロントエンド依存の既知脆弱性。High 以上で失敗。

3. **Dependabot(`.github/dependabot.yml`)** — 修正版が出たら週次で更新 PR を作成
   (セキュリティ修正は検知し次第すぐ)。対象は cargo(ルート + UI)・npm・
   GitHub Actions。リポジトリ設定で Dependabot alerts が有効になっている必要が
   あります(Settings → Security → Dependabot)。
