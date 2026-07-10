# PeerCove

[![CI](https://github.com/sgcmt176/peercove/actions/workflows/ci.yml/badge.svg)](https://github.com/sgcmt176/peercove/actions/workflows/ci.yml)

事業者サーバーを持たない P2P 型 VPN(技術検証中)。ホスト PC がコーディネーター兼リレーを担い、トンネルには WireGuard プロトコルを使用します。

現在のフェーズは **M0: 技術検証(PoC)** です。仕様は [docs/peercove-m0-handoff.md](docs/peercove-m0-handoff.md)、技術判断は [docs/decisions.md](docs/decisions.md) を参照してください。

## 進捗(M0 ゴール)

- [x] ワークスペース・keygen・設定 TOML 読み込み(G-1 前半)
- [x] G-1: トンネル作成・破棄(Windows / Linux)✅ 実機検証済み
- [x] G-2: 1対1疎通(Host–Member ping)✅ 実機検証済み(双方向 0% loss, RTT ~1ms)
- [x] G-3: ハブ&スポーク疎通(Member ↔ Member、Host 経由)✅ 実機検証済み
- [x] G-4: TCP 疎通 ✅ 実機検証済み
- [x] G-5: UDP 疎通(udp-echo / udp-ping)✅ 実機検証済み
- [x] G-6: UPnP による到達性セットアップ ✅ 実機検証済み(別 NAT からの到達含む)
- [x] G-7: 計測レポート ✅ 実測値記入済み([docs/m0-report-template.md](docs/m0-report-template.md))

**M0 の全ゴールを達成しました(2026-07-08)。**

### M1(進行中)

- [x] M1-G1: 招待トークン pcv1(invite / join、QR 対応)✅ 実機検証済み
- [x] M1-G2: トンネル内コントロールチャネル(台帳配布)✅ 実機検証済み
- [x] M1-G3: メンバー削除(remove-peer)✅ 実機検証済み
- [x] M1-6: init コマンド(ランダムサブネット生成、Tailscale 衝突回避)✅ 実機検証済み
- [x] M1-7: ピア設定変更の動的反映 ✅ 実機検証済み

**M1 の全タスクを達成しました(2026-07-08)。**

### M2(進行中)

- [x] M2-G1: デーモン分離 + ローカル IPC(daemon run / status / start / stop)✅ 実機検証済み
- [x] M2-G2: Tauri + React UI の骨組みと状態表示 ✅ 実機検証済み
- [x] M2-G3: 接続/切断・参加 UI ※実機検証待ち
- [x] M2-G4: 招待・メンバー管理 UI ※実機検証待ち
- [x] M2-G5: 設定編集・ログビュー・RTT 表示 ※実機検証待ち
- [x] M2-G6: トレイ常駐・参加/切断の通知 ※実機検証待ち
- [x] M2-G7a: デーモンのサービス化(Windows サービス / systemd、ADR-0010)✅ 実機検証済み
- [ ] M2-G7b: インストーラ(MSI / deb / ZIP)— Opus 担当。**MSI 実装済み(検証待ち)**、
  deb / ZIP は実装中(docs/peercove-g7-packaging-handoff.md)

## 必要環境

### 共通

- [Rust(stable)](https://rustup.rs/) — `rustup` でインストール

### Windows 10/11

- Visual Studio(C++ ビルドツール)— rustup のインストール時に案内されます
- トンネル操作(G-1 以降)は **管理者として実行した** PowerShell / ターミナルが必要です
- **wintun.dll**(TUN ドライバ)
  1. <https://www.wintun.net> から wintun のzipをダウンロード
  2. zip 内の `bin/amd64/wintun.dll` を `peercove-poc.exe` と同じフォルダ
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
[packaging/licenses/README.md](packaging/licenses/README.md)。

#### Windows MSI

```powershell
# 1) デーモンのリリースビルド(MSI が同梱・サービス登録する)
cargo build --release -p peercove-poc
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

MSI は UI 本体に加え、`peercove-poc.exe` / `wintun.dll` / `wintun-LICENSE.txt` を
同梱し、インストール時に `daemon service-install`(サービス登録 + ファイア
ウォール許可)、アンインストール時に `daemon service-uninstall` を呼びます
(WiX フラグメント `windows/daemon-service.wxs`。方式は ADR-0010、Fable への
確認事項は `docs/peercove-g7b-msi-review-for-fable.md`)。

#### Ubuntu deb

**deb は Linux 上でしかビルドできません**(`dpkg-deb` が要る)。検証機で:

```bash
# 前提(トレイのビルド): sudo apt install libayatana-appindicator3-dev
cargo build --release -p peercove-poc      # deb が /usr/bin/peercove-poc に入れる
cd apps/peercove-ui
npm install
npm run tauri build
#   → src-tauri/target/release/bundle/deb/PeerCove_0.1.0_amd64.deb
```

deb は UI 本体に加え、`peercove-poc` を `/usr/bin/` へ、systemd ユニットを
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

## 使い方

### ログの詳細度

すべてのコマンドで `--log-level <error|warn|info|debug|trace>` が使えます
(`RUST_LOG` 環境変数より優先)。既定は `info` で、通常運用では静かです。

- `debug`: 接続の受理・ピアの追加削除・破棄したパケットの理由など
- `trace`: パケット 1 個ごと(大量に出るので調査時のみ)

```bash
peercove-poc --log-level warn daemon run   # 警告以上だけ
peercove-poc --log-level debug host --config host.toml
```

### クイックスタート(ホストの初期化 → 招待)

```bash
# ホスト側(初期化は管理者権限不要)
./target/debug/peercove-poc init                # host.key + host.toml を生成
sudo ./target/debug/peercove-poc host --config host.toml   # Windows は管理者で
./target/debug/peercove-poc invite --config host.toml --name alice

# メンバー側(トークンを受け取って)
./target/debug/peercove-poc join --token "pcv1.…"
sudo ./target/debug/peercove-poc member --config member.toml
```

`init` はトンネルのサブネットを **ランダムな 10.x.y.0/24** から選びます
(Tailscale の 100.64.0.0/10 や一般的な家庭 LAN・Docker と衝突しない帯。
ADR-0006)。既存の設定もそのまま使えますが、CGNAT レンジ内のサブネットを
使っていると起動時に警告が出ます。

### デーモン経由の操作(M2-G1、UI の土台)

将来の GUI は常駐デーモンをローカル IPC(Windows: 名前付きパイプ /
Linux: Unix ドメインソケット)で操作します。CLI からも同じ IPC を使えます:

```bash
# 1) デーモンを起動(トンネル操作をするので管理者/root で)
sudo ./target/debug/peercove-poc daemon run
# ログを絞りたいときは --log-level warn(既定は info)

# 2) 別ターミナル(ユーザー権限でよい)から操作
./target/debug/peercove-poc daemon start-host --config host.toml
./target/debug/peercove-poc daemon status
./target/debug/peercove-poc daemon logs          # 直近のログ(最大 500 行)
./target/debug/peercove-poc daemon logs --follow # 新しい行を待ち続ける
./target/debug/peercove-poc daemon stop
./target/debug/peercove-poc daemon shutdown

# OS サービスとして常駐させる場合(手動起動の代わり。要管理者/root)
./target/debug/peercove-poc daemon service-install    # 登録 + 起動 + 自動起動
./target/debug/peercove-poc daemon service-uninstall  # 停止 + 登録解除
```

- 招待・参加・削除(invite / join / remove-peer)は**デーモンを介さず**
  従来どおり実行します(設定ファイル操作なので。実行中のトンネルは
  5 秒ごとの再読込で自動追随します)
- 従来の `host` / `member`(プロセス内実行)も引き続き使えます
- **デーモンは管理者/root、操作コマンドは通常ユーザーで実行できます**
  (Windows はパイプに認証済みユーザーの許可を付与、Linux はソケットを 0666 に設定)

#### デーモンのトラブルシューティング

- **`daemon status` が「アクセスが拒否されました (os error 5)」**:
  古い peercove デーモンが残っている可能性があります。タスクマネージャー
  (Linux は `ps aux | grep peercove`)で `peercove-poc` を探して終了して
  ください。**管理者で起動したデーモンは管理者ターミナルからしか終了できません**
  (`daemon shutdown` を管理者ターミナルで実行するのが確実)
- **`daemon run` が「名前付きパイプを作成できません」**: 同上(二重起動)
- **Linux で「そのようなファイルやディレクトリはありません (os error 2)」**:
  デーモンのソケットは root 実行時 `/run/peercove.sock`、通常ユーザー実行時
  `$XDG_RUNTIME_DIR/peercove.sock` に作られます。クライアントは両方を順に
  探すので通常は問題ありませんが、環境変数 `PEERCOVE_SOCKET` で明示的に
  同じパスを指定することもできます(デーモン・クライアントの双方に設定)

### 鍵の生成(keygen)

管理者権限は不要です。

```bash
# 鍵ペアを生成(秘密鍵はファイルへ保存、公開鍵は画面に表示)
./target/debug/peercove-poc keygen --out host.key

# 事前共有鍵(PSK)を生成(任意)
./target/debug/peercove-poc keygen --psk --out psk.key
```

- 秘密鍵・PSK は**画面には表示されません**。ファイルの中身を人に送らないでください(公開鍵は共有可)
- 保存先ファイルは Linux ではパーミッション 600、Windows では現在のユーザーのみアクセス可の ACL になります
- 既存ファイルがある場合は失敗します。上書きは `--force`

### 設定ファイル

[examples/host.example.toml](examples/host.example.toml) と [examples/member.example.toml](examples/member.example.toml) をコピーして編集してください。設定内の相対パス(`private_key_file` 等)は**設定ファイルのあるディレクトリ基準**で解決されます。

### トンネルの起動と停止

**Windows は管理者ターミナル、Linux は sudo が必要です。**

```bash
# ホスト(トンネル作成 + UDP 51820 待受)
sudo ./target/debug/peercove-poc host --config host.toml

# メンバー(トンネル作成 + ホストへ接続)
sudo ./target/debug/peercove-poc member --config member.toml
```

- 起動中は Ctrl+C で終了し、トンネルを自動でクリーンアップします
- 異常終了などで残骸が残った場合は `down` で掃除できます:

```bash
sudo ./target/debug/peercove-poc down --config host.toml
```

> メモ: Windows のトンネルはユーザー空間実装のため、プロセスが終了すると
> アダプタも自動的に消えます。Linux はカーネル実装のため、異常終了時に
> インターフェースが残ることがあります(`down` で削除)。

### メンバーの招待(invite / join)— M1 の推奨フロー

M0 の「keygen → 公開鍵を伝える → add-peer」を 1 ステップにしたものです。

```bash
# ホスト側: 招待トークンを発行(メンバーの鍵と IP を自動生成して登録)
./target/debug/peercove-poc invite --config host.toml --name alice
# → invite.token に保存。--endpoint 203.0.113.5:51820 で外部候補を追加、
#   --psk で事前共有鍵も発行、--print で文字列表示、--qr でターミナルに QR 表示

# メンバー側: トークンから鍵と設定を生成
./target/debug/peercove-poc join --token "pcv1.…" --out-dir .
# ファイル渡しの場合: --token-file invite.token
# → member.key / member.toml(PSK ありなら member.psk)が生成される

# あとは通常どおり接続(管理者/sudo)
sudo ./target/debug/peercove-poc member --config member.toml
```

- **トークンはメンバーの秘密鍵を含む秘密情報です**。本人以外へ渡さず、
  受け渡し後は双方で削除してください(既定でファイル保存なのはこのため)
- トークンにはエンドポイント候補が複数入ります(LAN は自動、外部は
  `--endpoint` で追加)。join は先頭候補を設定し、残りはコメントとして
  member.toml に残します。**同一 LAN なら LAN 候補**を使ってください
- 招待の取り消しは remove-peer(M1-G3)で行う予定です

### メンバーの追加(add-peer)と状態確認(status)

```bash
# ホスト側: メンバーの公開鍵と割当 IP を登録(管理者権限は不要)
./target/debug/peercove-poc add-peer --config host.toml \
    --pubkey "MEMBER_A_PUBKEY_B64" --ip 100.100.42.2
```

- `add-peer` は host.toml に `[[peer]]` を追記するだけです。実行中の host
  プロセスは 5 秒間隔で設定を再読込し、新しいピアを自動で取り込みます
  (再起動不要。ピアの削除・変更の反映は M1 で対応)
- 状態確認は同じ設定ファイルを指定して:

```bash
./target/debug/peercove-poc status --config host.toml
# peer: <公開鍵>
#   endpoint: 203.0.113.10:53210
#   allowed_ips: 100.100.42.2/32
#   latest handshake: 3 秒前
#   transfer: rx 1.2 KiB, tx 892 B
```

`status` は host / member プロセスが 5 秒ごとに書き出す
`<設定名>.status.txt` を表示します(プロセスが動いていないとエラー)。

### 台帳(メンバー一覧)の自動配布 — M1-G2

host と member のプロセスは、トンネル内のコントロールチャネル
(ホスト仮想 IP の TCP 51821)で自動的に繋がり、**メンバー一覧(台帳)** が
全員に配布されます。`status` の先頭に表示されます:

```
members:
  ● host(100.100.42.1) [host]
  ● alice(100.100.42.2)
  ○ bob(100.100.42.3)        ← ○ はオフライン(最終ハンドシェイク 180 秒超)
```

- 台帳の正本はホストの host.toml(`[[peer]]` の `name`)です。invite で
  メンバーを追加すると約 5〜10 秒で全員の `status` に反映されます
- member 側の接続先は join が書き込む `control_host`(通常はホストの
  仮想 IP)。トンネル確立後に自動接続・自動再接続します
- **Windows ホストの場合**: peercove-poc.exe の TCP 受信(51821)が
  ファイアウォールでブロックされると member の台帳が更新されません。
  初回のダイアログで「許可」を選んでください

### メンバーの削除(remove-peer)— M1-G3

```bash
# 名前 / 公開鍵 / 仮想 IP のいずれかで指定(管理者権限は不要)
./target/debug/peercove-poc remove-peer --config host.toml --name alice
./target/debug/peercove-poc remove-peer --config host.toml --ip 100.100.42.2
```

- host.toml から該当 `[[peer]]` を削除します(コメント等は保持)。
  invite --psk で作られたホスト側 PSK ファイルも片付けます
- 実行中の host は約 10 秒で反映します:
  **1) 本人へ削除通知(コントロールチャネル)→ 2) トンネルから除外**。
  以後そのメンバーのトークン・鍵では接続できません
- 全員の `status` の members からも自動で消えます(台帳配布)

### ピア設定の手動変更も自動反映されます(M1-7)

host.toml の `[[peer]]` の `endpoint` / `allowed_ips` /
`persistent_keepalive` / `preshared_key_file` を編集して保存すると、
実行中の host が約 5 秒で検知して反映します(内部的には削除→再追加のため、
そのピアは数秒間だけ再ハンドシェイクになります)。member 側の設定変更は
これまでどおり member の再起動が必要です。

## 検証手順(G-1: トンネル作成・破棄)

Windows / Linux 各 1 台で、それぞれ単体で確認できます(相手は不要)。

### Windows(管理者 PowerShell)

1. wintun.dll を `target\debug\` に配置する(上記「必要環境」参照)
2. `examples\host.example.toml` を `host.toml` にコピーし、
   `peercove-poc keygen --out host.key` で鍵を作る
3. `.\target\debug\peercove-poc.exe host --config host.toml` を実行し、
   「トンネル peercove0 を作成しました」「待受ポート: UDP 51820」が表示されること
4. 別の(管理者でなくてよい)ターミナルで `ipconfig` に `peercove0` が現れ、
   IPv4 アドレスが `100.100.42.1` であること
5. `ping 100.100.42.1` に応答があること(自分宛)
6. Ctrl+C で終了し、`ipconfig` から `peercove0` が消えること
7. 管理者**でない**ターミナルで手順 3 を実行すると、管理者権限を促す
   エラーになること(黙って失敗しないこと)
8. wintun.dll を一時的にリネームして手順 3 を実行すると、入手手順を含む
   エラーになること

### Ubuntu 22.04+

1. `examples/member.example.toml` を `member.toml` にコピーし、
   `peercove-poc keygen --out member_a.key` で鍵を作る
   (`public_key` / `endpoint` はこの時点ではダミーで可)
2. `sudo ./target/debug/peercove-poc member --config member.toml` を実行し、
   「トンネル peercove0 を作成しました」が表示されること
3. 別ターミナルで `ip addr show peercove0` に `100.100.42.2/24` が表示され、
   `ip route` に `100.100.42.0/24 dev peercove0` があること
4. Ctrl+C で終了し、`ip link show peercove0` が「does not exist」になること
5. sudo なしで手順 2 を実行すると「root 権限が必要です」エラーになること
6. `sudo ./target/debug/peercove-poc down --config member.toml` が
   (トンネルが無い状態でも)正常終了すること

## 検証手順(G-2: Host–Member 1対1 ping 疎通)

Host(Windows 11)と Member A(Ubuntu)の 2 台で行います。
まず両方の機械で G-1 の検証が通っていることが前提です。

### 準備

1. **Host(Windows・管理者)**
   1. `peercove-poc keygen --out host.key` → 表示された公開鍵を控える(以下 `HOST_PUB`)
   2. `examples\host.example.toml` を `host.toml` にコピー(そのままで可)
2. **Member A(Ubuntu)**
   1. `./peercove-poc keygen --out member_a.key` → 公開鍵を控える(以下 `MEMBER_A_PUB`)
   2. `examples/member.example.toml` を `member.toml` にコピーし編集:
      - `address = "100.100.42.2/24"`
      - `public_key = "HOST_PUB"`
      - `endpoint = "<HostのIP>:51820"`
        (**同一 LAN なら Host の LAN IP**。別 NAT の場合は G-6 まではルーターで
        UDP 51820 を Host へ手動ポートフォワードしておく)
3. 事前共有鍵を使う場合(任意): どちらかで `keygen --psk --out psk.key` を実行し、
   **同じファイル**を両方に配置して、双方の `[[peer]]` に
   `preshared_key_file = "psk.key"` を追記

### 疎通確認

1. Host: `.\peercove-poc.exe host --config host.toml` を起動したまま、
   別ターミナルで `add-peer --config host.toml --pubkey "MEMBER_A_PUB" --ip 100.100.42.2`
2. Host のログに(5 秒以内に)「ピア … を追加しました」が出ること
3. Member A: `sudo ./peercove-poc member --config member.toml` を起動
4. 数秒以内に、両側の `status` で `latest handshake` が「n 秒前」になること
   (ハンドシェイクが「なし」のままなら疎通失敗 → 下の確認ポイント参照)
5. **Member A → Host**: `ping 100.100.42.1` が通ること
6. **Host → Member A**: `ping 100.100.42.2` が通ること
7. Member A の `status` の transfer(rx/tx)が ping のたびに増えること
8. 片方を Ctrl+C → もう一方の `status` の handshake 経過秒が増え続けること
   を確認後、再起動すると自動で復帰すること(メンバー再起動の場合)

### 失敗時の確認ポイント

- **handshake が「なし」**: endpoint の IP・ポートが正しいか。別 NAT の場合は
  ルーターのポートフォワード(UDP 51820 → Host)を確認。Host 側 Windows
  ファイアウォールで peercove-poc.exe の UDP 受信が許可されているか
  (初回起動時のダイアログで「許可」を選ぶ。出なかった場合は
  「Windows Defender ファイアウォール > アプリの許可」で追加)
- **handshake は成立するが ping が通らない**: Windows の受信規則で
  ICMP エコー(ファイルとプリンターの共有 (エコー要求 - ICMPv4 受信))が
  ブロックされていないか。まず Member→Host と Host→Member を両方試し、
  片方向だけ失敗するならファイアウォールが原因
- **同一 LAN で繋がらない**: endpoint に外部 IP でなく **LAN IP** を指定する
  (ルーターの hairpin NAT 非対応のため)
- **Tailscale が入っているマシンで、handshake も transfer も正常なのに ping が届かない**:
  Tailscale はなりすまし防止のため「送信元が 100.64.0.0/10(CGNAT レンジ)なのに
  tailscale0 以外から入ってきたパケット」を iptables(`ts-input`)で DROP します。
  PeerCove の既定例 `100.100.42.x` はこのレンジ内のため衝突します。
  `sudo tailscale down` で一時停止するか、両側の設定の仮想 IP を
  `10.100.42.x` などレンジ外に変更してください(tcpdump にはパケットが
  映るのに ping に届かない、が典型症状)
- **host を再起動した直後に通らない**: Windows の host はユーザー空間実装のため、
  再起動するとセッションが消えます。member 側は「データを送っても応答が無い」
  ことを検知してから再ハンドシェイクするため、**復帰まで最大 15〜20 秒**
  かかります(host 側には「セッション不一致」の警告が出ます)。ping を
  30 秒以上続けるか、member 側を再起動すると確実です

## 検証手順(G-3: Member A ↔ Member B、Host 経由)

Host + Member A(G-2 と同じ)に、**Member B(Windows 10/11)** を追加します。
A と B の間に直接のピア設定はなく、すべて Host がリレーします。

ホスト側の転送は自動で有効になります(Windows ホスト: デバイス内リレー /
Linux ホスト: インターフェース単位の IP フォワーディング。OS のグローバル
設定は変更しません)。

### 準備(Member B)

1. wintun.dll を `target\debug\` に配置(「必要環境」参照)
2. `peercove-poc keygen --out member_b.key` → 公開鍵を控える(`MEMBER_B_PUB`)
3. `examples\member.example.toml` を `member_b.toml` にコピーし編集:
   - `private_key_file = "member_b.key"`
   - `address = "100.100.42.3/24"`
   - `public_key = "HOST_PUB"` / `endpoint = "<HostのIP>:51820"`
4. Host 側で `add-peer --config host.toml --pubkey "MEMBER_B_PUB" --ip 100.100.42.3`
5. Member B(管理者)で `.\peercove-poc.exe member --config member_b.toml`
6. Member B が Tailscale 入りの場合は一時停止しておく(トラブルシューティング参照)

### 疎通確認

1. Host・A・B の 3 者を起動し、Host の `status` で **2 ピアとも** handshake が
   成立していること
2. **A → B**: Member A(Ubuntu)から `ping -c 10 100.100.42.3`
3. **B → A**: Member B(Windows)から `ping 100.100.42.2`
4. ping 中、**Host の `status`** で A・B 両ピアの transfer が増えること
   (= リレーが Host を通っている証拠)
5. B → Host(`ping 100.100.42.1`)も通ること(G-2 の Windows 版に相当)

### 失敗時の確認ポイント

- **A→B だけ失敗**: Member B(Windows)の ICMP 受信がファイアウォールで
  ブロックされていないか(G-2 のトラブルシューティング参照。
  `netsh advfirewall firewall add rule name="PeerCove ICMPv4-In" protocol=icmpv4:8,any dir=in action=allow`)
- **両方向失敗**: Host の `status` で B の handshake が成立しているか。
  B の endpoint(Host の IP:51820)と add-peer の IP(100.100.42.3)を確認
- **Linux ホストの場合のみ**: `cat /proc/sys/net/ipv4/conf/peercove0/forwarding`
  が `1` になっているか

## 検証手順(G-4: TCP 疎通)

G-3 と同じ 3 台構成(Host + Member A + Member B、トンネル起動済み)で行います。

1. **Member A(Ubuntu)** で HTTP サーバーを起動:
   ```bash
   python3 -m http.server 8080 --bind 100.100.42.2
   ```
2. **Member B(Windows)** から:
   ```powershell
   curl.exe http://100.100.42.2:8080/
   ```
   ディレクトリ一覧の HTML が返れば成功
3. 逆方向も確認する場合: Member B で `python -m http.server 8080`(または任意の
   TCP サーバー)を起動し、Member A から `curl http://100.100.42.3:8080/`

### 失敗時の確認ポイント

- まず `ping` が通ること(G-3 まで OK か)。ping が通って curl だけ失敗なら
  サーバー側ファイアウォールの TCP 受信ブロックが原因
- **Windows 側でサーバーを立てる場合**: 初回起動時のファイアウォール
  ダイアログで「許可」を選ぶ。ダイアログを閉じてしまった場合は
  「Windows Defender ファイアウォール > アプリの許可」で python 等を許可する
- **Ubuntu 側**: `sudo ufw status` が active の場合は
  `sudo ufw allow in on peercove0 to any port 8080 proto tcp` を追加
- ダウンロードが途中で止まる場合は MTU の問題の可能性(member.toml の
  `mtu` を 1412 や 1380 に下げて再起動)

## 検証手順(G-5: UDP 疎通)

同じく 3 台構成で行います。`udp-echo` / `udp-ping` はトンネルとは独立した
ツールなので、管理者権限は不要です。

1. **Member A(Ubuntu)** で echo サーバーを起動:
   ```bash
   ./target/debug/peercove-poc udp-echo --listen 0.0.0.0:9999
   ```
2. **Member B(Windows)** から:
   ```powershell
   .\target\debug\peercove-poc.exe udp-ping --target 100.100.42.2:9999 --count 10
   ```
3. 期待する結果:
   - Member B に `xx バイト受信 seq=n rtt=x.xx ms` が 10 行と、
     `損失 0%`・`rtt min/avg/max` の統計
   - Member A 側に `100.100.42.3:xxxxx から 26 バイト受信 -> 返送` が 10 行
     (**送信元がトンネル内 IP(100.100.42.3)であること**が、UDP が
     トンネル内の通常 IP 通信として届いている証拠)
4. 逆方向(B で udp-echo、A から udp-ping)も同様に確認

### 失敗時の確認ポイント

- `udp-ping` は応答ゼロだとエラー終了し、確認ポイント(echo サーバー起動・
  ファイアウォール・トンネル疎通)を表示します
- **Windows 側で udp-echo を立てる場合**: 初回起動時のファイアウォール
  ダイアログで「許可」を選ぶ(TCP と同様)
- Ubuntu 側で ufw が active の場合:
  `sudo ufw allow in on peercove0 to any port 9999 proto udp`

## 検証手順(G-6: UPnP ポート自動開放)

Host(ルーター直下)で行います。ルーターの UPnP 設定を切り替えて 2 パターン確認します。

### パターン 1: UPnP 有効

1. ルーターの設定画面で UPnP が有効になっていることを確認
2. Host で `--upnp` を付けて起動:
   ```powershell
   .\target\debug\peercove-poc.exe host --config host.toml --upnp
   ```
3. 以下が表示されること:
   - `UPnP ポート開放に成功しました(UDP 51820、リース 24 時間)`
   - `外部エンドポイント(推定): <グローバルIP>:51820`
4. 表示されたグローバル IP が確認サイト等の値と一致すること
5. ルーターの設定画面(ポートマッピング/UPnP 一覧)に「PeerCove」の
   エントリ(UDP 51820)が現れること
6. **可能なら**: 別 NAT のメンバー(テザリング等)の endpoint に表示された
   外部エンドポイントを指定し、handshake が成立すること(到達性の実証)
7. Ctrl+C で終了 → ルーターの一覧から PeerCove のエントリが消えること

### パターン 2: UPnP 無効(エラーメッセージの確認)

1. ルーターの UPnP を無効にする(または一時的に確認できる環境で)
2. 手順 2 と同じコマンドを実行
3. トンネル自体は起動しつつ、「UPnP 対応ルーターが見つかりませんでした」
   の警告に **(1) UPnP 設定の確認 (2) 手動ポートフォワードの案内
   (3) CGNAT の可能性** が含まれていること

> 補足: ルーターの外部 IP がグローバル IP でない場合(二重 NAT / CGNAT)は
> 成功時でも警告が出ます。この場合、開放しても外部からは届かないことが
> あります(上位ルーターの設定変更か ISP への確認が必要)。

## 検証手順(G-7: 計測レポート)

G-3 と同じ 3 台構成で計測し、結果を [docs/m0-report-template.md](docs/m0-report-template.md)
に記入します。

### 準備(iperf3)

- Ubuntu: `sudo apt install iperf3`
- Windows: `winget install iperf3`(または <https://iperf.fr> のバイナリ)。
  サーバー側にする場合は初回のファイアウォールダイアログで「許可」

### RTT 計測

```bash
# Member A から(Linux)
ping -c 100 100.100.42.1   # A -> Host
ping -c 100 100.100.42.3   # A -> B(リレー経由)
# 参考値: トンネル外の物理 LAN(A -> Host の実 IP)
ping -c 100 192.168.0.12
```

Windows からは `ping -n 100 <宛先>`。それぞれ min/avg/max をテンプレートに記入します。
「A → B」と「A → Host」の差がリレー 1 ホップ分のオーバーヘッドです。

### スループット計測(iperf3)

```bash
# Member A(サーバー)
iperf3 -s

# Member B(クライアント)から 3 回ずつ
iperf3 -c 100.100.42.2 -t 10        # TCP B->A
iperf3 -c 100.100.42.2 -t 10 -R     # TCP A->B(逆方向)
iperf3 -c 100.100.42.2 -u -b 100M   # UDP(帯域・損失率・ジッター)
```

- 中央値をテンプレートに記入。UDP は損失率とジッターも記録
- 可能ならトンネル外(物理 LAN の IP 宛)でも 1 回計測し、参考値として記入
- スループットが伸びない場合は member.toml の `mtu` を調整して差を記録すると
  今後の参考になります

## 検証手順(M1-G1: invite / join)

Host(Windows)+ Member A(Ubuntu)の 2 台。G-2 と同じ構成ですが、
**鍵交換・設定編集を一切手作業しない**のがポイントです。

1. Host: 新しい作業フォルダで `keygen --out host.key` と host.toml を用意し、
   `host --config host.toml` を起動しておく
2. Host(別ターミナル): `invite --config host.toml --name test-a --print`
   - `invite.token` が作られ、トークン文字列が表示されること
   - host.toml の末尾に `name = "test-a"` 付きの `[[peer]]` が追記されること
   - 起動中の host のログに 5 秒以内に「ピア … を追加しました」が出ること
3. トークン文字列を Member A へコピペ(チャット等を想定)
4. Member A: `./peercove-poc join --token "pcv1.…" --out-dir ~/pc-test`
   - member.key(600)/ member.toml が生成されること
   - member.toml の endpoint が Host の LAN IP になっていること
5. Member A: `sudo ./peercove-poc member --config ~/pc-test/member.toml`
   → `ping 100.100.42.1` が通ること(手作業の設定なしで疎通)
6. QR 確認: Host で `invite --config host.toml --name test-b --qr` を実行し、
   ターミナルに QR が表示され、スマホのカメラで `pcv1.` から始まる文字列として
   読み取れること(読み取った文字列で join できればなお良い)
7. エラー系: トークンの末尾を数文字削って join → 「コピー漏れ」を示唆する
   エラーになること

## 検証手順(M1-G2: 台帳配布)

M1-G1 の構成(Host + join 済み Member A)をそのまま使います。

1. Host・Member A を起動し、疎通(ping)を確認
2. Member A: `status --config member.toml` の先頭に `members:` セクションが現れ、
   host と自分が ● (オンライン)で表示されること
   (トンネル確立から最大 10 秒程度かかります)
3. Host: `invite --config host.toml --name test-b` を実行(Member B は起動しなくてよい)
4. 約 10 秒以内に **Member A の status** に `○ test-b(100.100.42.3)` が
   追加されること(= 台帳変更がトンネル越しに配布された)
5. Host のログに「メンバー 100.100.42.2(…)が接続しました」が出ていること
6. Member A を Ctrl+C → Host のログに切断ログ、再起動 → 再接続ログが出ること

失敗時: Windows ホストのファイアウォールで TCP 51821 の受信許可
(初回ダイアログ)を確認。Tailscale 起動中なら停止(README 上部参照)。

## 検証手順(M2-G7a: デーモンのサービス化 PoC)

**G7 最大のリスク検証です**: Windows サービス(LocalSystem / Session 0)から
wintun のトンネルが作れて疎通することを確認します。Linux は systemd で同等を確認します。

### Windows(ホスト機)

```powershell
# 1) リリースビルド + wintun.dll を隣に置く
cargo build --release -p peercove-poc
copy <wintunの場所>\wintun.dll target\release\

# 2) 既存の秘密ファイル(鍵 と PSK の両方)に SYSTEM の読み取りを付与
#    (1 回だけ。これから invite/init/join で作るものには自動で付きます)。
#    どれか 1 つでも漏れると、その項目の読み込みで os error 5 になります:
icacls "$env:APPDATA\app.peercove.desktop\*.key" /grant "*S-1-5-18:F"
icacls "$env:APPDATA\app.peercove.desktop\*.psk" /grant "*S-1-5-18:F"
# リポジトリ直下の host.key / peer-*.psk を使っている場合はそちらにも同様に

# 3) 手動起動していたデーモンが残っていれば止める(パイプ名が衝突するため)
#    (管理者ターミナルで)peercove-poc daemon shutdown

# 4) サービス登録 + 起動(管理者 PowerShell)
#    ファイアウォールの受信許可(この exe 宛の UDP + TCP)も一緒に追加されます。
#    Session 0 のサービスには「許可しますか?」ダイアログが出ないため、
#    UDP が無いとハンドシェイクが、TCP が無いと台帳配布(コントロール
#    チャネル)が黙って遮断されます
.\target\release\peercove-poc.exe daemon service-install
```

> **PowerShell の注意**: `sc` は PowerShell では Set-Content(ファイル書き込み)の
> エイリアスです。`sc query …` と打つと**何も表示されずに `query` という名前の
> ファイルができます**。必ず `sc.exe` と拡張子まで書くか、`Get-Service` を
> 使ってください。

確認項目:

1. `Get-Service peercove-daemon` が Running であること
   (または `sc.exe query peercove-daemon`)
2. **非管理者**のターミナルで `peercove-poc daemon status` が通ること
   (SYSTEM のサービス ↔ 非特権クライアントのパイプ権限)
3. UI(`npm run tauri dev` か通常起動)からホスト開始 →
   **メンバーと ping が通ること(← これが Session 0 PoC の本丸)**
4. UI のログビュー(☰)にサービスのログが出ること
   (サービスの標準エラーはどこにも出ないので、ログビューが唯一の窓口です)
5. **OS を再起動** → ログイン後、何もせず `Get-Service peercove-daemon` が Running、
   UI を開くと「待機中」表示 → そのまま「開始」で前回のネットワークに繋がること
6. トンネル稼働中に(管理者で)`daemon service-uninstall` →
   「停止しています…」の後に登録解除され、`ipconfig` に peercove0 が
   残っていないこと(クリーンアップの確認)。ファイアウォールルール
   「PeerCove Daemon」も消えていること
   (`Get-NetFirewallRule -DisplayName "PeerCove Daemon"` がエラーになる)

### Ubuntu(メンバー機)

```bash
cargo build --release -p peercove-poc
sudo ./target/release/peercove-poc daemon service-install
```

7. `systemctl status peercove-daemon` が active (running) であること
8. `journalctl -u peercove-daemon -f` にログが流れること
9. UI から参加 → 疎通すること
10. **OS を再起動** → 自動起動していること(`systemctl status`)
11. `sudo ./target/release/peercove-poc daemon service-uninstall` →
    ユニットが消え、`ip link` に peercove0 が残っていないこと

> 手動の `daemon run`(コンソールモード)も従来どおり使えます。ただし
> **サービスと同時には動かせません**(パイプ/ソケットが衝突します)。

## 検証手順(M2-G7b: Windows MSI インストーラ)

**クリーンな Windows 環境(できれば別マシン / VM)で確認するのが理想**です。
開発機で試す場合は、先に `daemon service-uninstall` と手動追加した
ファイアウォールルール・鍵の ACL を元に戻してから(MSI が全部やり直すため)。

準備: 上記「インストーラのビルド(Windows MSI)」で `.msi` を作る。

1. `.msi` をダブルクリック → インストール(UAC で昇格)。完了まで進むこと
2. `Get-Service peercove-daemon` が **Running**(MSI がサービス登録 + 起動した)
3. `Get-NetFirewallRule -DisplayName "PeerCove Daemon"` が **2 本**(UDP/TCP)
   返ること
4. スタートメニューの **PeerCove** を起動(通常権限)→ 「待機中」表示
5. ホスト開始 → 別マシンのメンバーと **ping + 台帳** が通ること
   (通らないときの第一容疑者はファイアウォール)
6. **通知の表示元が「PeerCove」**になっていること
   (dev では「PowerShell」だったのがショートカット登録で解消)
7. 「アプリと機能」から **アンインストール** →
   - `Get-Service peercove-daemon` がエラー(サービス消滅)
   - `Get-NetFirewallRule -DisplayName "PeerCove Daemon"` がエラー(ルール消滅)
   - インストール先フォルダが残っていないこと
   - `ipconfig` に peercove0 が無いこと
8. インストール先に `wintun-LICENSE.txt` が含まれていたこと(同梱義務の確認)

> **未検証の設計です**(Opus 実装 / Fable 確認待ち)。MSI は WiX の
> カスタムアクションで `daemon service-install` / `service-uninstall` を呼ぶ方式
> です(ADR-0010、`docs/peercove-g7b-msi-review-for-fable.md`)。失敗する場合は
> インストールログ(`msiexec /i PeerCove_*.msi /l*v install.log`)を取得して
> ください。

## 検証手順(M2-G7b: Ubuntu deb インストーラ)

準備: 上記「インストーラのビルド(Ubuntu deb)」で `.deb` を作る。

1. `sudo apt install ./PeerCove_0.1.0_amd64.deb`(依存も解決される)
2. `systemctl status peercove-daemon` が **active (running)**
   (postinst が有効化 + 起動した)
3. `which peercove-poc` が `/usr/bin/peercove-poc`、
   `ls /usr/lib/systemd/system/peercove-daemon.service` が在ること
4. アプリ一覧か `peer-cove`(UI)を起動 → ホスト/参加 → 疎通すること
5. **OS 再起動** → `systemctl status peercove-daemon` が自動で running
6. `sudo apt remove peercove`(パッケージ名は `dpkg -l | grep -i peercove` で確認)→
   - `systemctl status peercove-daemon` が not-found(prerm が停止・無効化)
   - `/usr/bin/peercove-poc` と unit ファイルが消えていること
   - `ip link` に peercove0 が残っていないこと

> **未検証の設計です**(Opus 実装)。Tauri の deb バンドラが生成する
> maintainer script に `packaging/deb/*.sh` が正しく差し込まれるか(特に
> postinst の重複や `$1` 引数の扱い)を実機で確認してください。失敗時は
> `sudo dpkg -i ...` の出力と `journalctl -u peercove-daemon` を見てください。

## 検証手順(M2-G5/G6: 設定・ログ・RTT・トレイ常駐)

前提:

- Linux は `sudo apt install libayatana-appindicator3-dev`(トレイのビルドに必要)
- **デーモンは新しいバイナリで起動し直す**(`daemon logs` と RTT は新プロトコル。
  古いデーモンのままだとログビューが「想定外の応答」で失敗します)
- ホストとメンバーがそれぞれ `daemon run` を起動し、UI から接続済みであること
  (M2-G3/G4 の手順まで終わった状態)

### RTT 表示(G5)

1. トンネル稼働中の画面で、メンバー一覧の各行に **`12 ms` のようなタグ**が
   出ること(制御接続が張れるまで数秒かかります。同一 LAN なら `< 1 ms`)
2. 「ピア統計」テーブルに **RTT 列**が増え、2 秒ごとに更新されること
3. メンバー側の画面でも、ホストに対して RTT が出ること
4. メンバーの LAN ケーブルを抜く等で切断 → RTT のタグが消え、
   数十秒後に ○(オフライン)になること
5. CLI でも見えること: `peercove-poc daemon status` に `rtt 0.4 ms` が付く

### ログビュー(G5)

6. ヘッダ右上の **☰** を押す → 「デーモンのログ」が開き、
   `トンネルを開始しました` 等の行が時刻付きで並ぶこと
7. 1 秒ごとに新しい行が追記され、**最新行を追う**にチェックが入っていれば
   自動で最下部までスクロールすること
8. 「表示レベル」を `WARN 以上` にすると INFO 行が消えること
9. デーモンを **`peercove-poc --log-level debug daemon run`** で起動し直すと、
   `DEBUG` を選んだときに詳細な行が出ること
   (逆に `--log-level warn` で起動すると、ログビューにも warn 以上しか出ません)
10. ターミナルからも同じものが読めること: `peercove-poc daemon logs --follow`
11. **秘密鍵・PSK・トークンがログに一切出ていないこと**(重要)

### 設定編集(G5)

12. ヘッダ右上の **⚙** を押す → インターフェース名・仮想 IP・設定ファイルのパスと、
    表示名 / 待受ポート / MTU の入力欄が出ること
13. **表示名**を変えて「保存」→「数秒でトンネルに反映されます」と出て、
    約 5 秒後に相手側のメンバー一覧の名前が変わること(切断不要)
14. **MTU** を `1380` にして「保存」→「切断して接続し直すと反映されます」と出ること。
    実際に切断 → 接続すると新しい MTU で立ち上がること
15. MTU に `100` を入れると「MTU は 576〜1500 の範囲で…」と弾かれ、
    **設定ファイルが書き換わっていない**こと
16. メンバー側の ⚙ には **「ホストのエンドポイント」** 欄が増えていること
    (ホスト側には出ない)。不正な値(`203.0.113.5` のようにポート無し)は
    「IP:ポート形式で指定してください」と弾かれること
17. 設定ファイルを直接開き、**コメントと他の項目が保持されている**こと

### トレイ常駐と通知(G6)

18. ウィンドウの **× を押してもアプリが終了せず**、タスクトレイ(通知領域)に
    PeerCove のアイコンが残ること
19. トレイアイコンを **左クリック** → ウィンドウが復帰すること。
    **右クリック** → 「PeerCove を表示 / 終了」のメニューが出ること
20. ウィンドウを閉じた状態でメンバーが接続 → **ホスト側**に「メンバーが参加しました」
    の OS 通知が出ること
21. ホストが **メンバーを削除**(× ボタン)すると、ホスト側に「メンバーが切断しました」
    が出ること(削除は台帳から即消えるので、すぐ通知されます)
22. トレイの「終了」でアプリが終了し、**トンネルは切れないこと**
    (デーモンが維持している。`peercove-poc daemon status` で確認)
23. UI を起動し直しても、既に接続済みのメンバーについて**通知が鳴らない**こと
    (初回の状態は基準にするだけで通知しない)

> **メンバーが自分から「切断」した場合の通知は最大 3 分遅れます。** WireGuard は
> 明示的な切断信号を持たず、ホスト側はセッション有効期限(180 秒)を過ぎて初めて
> オフラインと判定するためです。**すぐに反映されるのは「ホストによる削除」**です。
> 短時間の切断→再接続(180 秒以内)は、オンラインのままなので通知されません。

> **Windows で通知元が「PowerShell」と表示される件**: これは `tauri dev`(=
> `target/debug` から起動)の仕様です。通知プラグインは**インストール済みのときだけ**
> アプリの AppUserModelID を設定します(未登録だと Windows が既定の PowerShell の
> AUMID を使う)。**M2-G7 のインストーラでショートカットを登録すると「PeerCove」に
> なります**。Linux では最初から「PeerCove」と出ます。

### メンバー削除時のメンバー側表示(G6・検証フィードバック)

24. メンバーが接続中に、ホストが **× でそのメンバーを削除** する
25. 数秒で**メンバー側の画面に赤い「ホストから削除されました」バナー**が出ること
    (以前は「参加中」のまま気づけませんでした)。「切断する」ボタンで待機中に戻ること

> **Linux でトレイアイコンが出ない場合**: `libayatana-appindicator3-dev` を
> 入れてからビルドし直してください。GNOME では AppIndicator 拡張が要ります。

## 検証手順(M2-G3/G4: UI だけで E2E)

**CLI を一度も使わずに** ホスト開始 → 招待 → 参加 → 削除 ができることを確認します。
デーモンだけは管理者/root のターミナルで起動しておきます(サービス化は M2-G7)。

準備: 両方の PC で `peercove-poc daemon run`(管理者 / sudo)を起動し、
`cd apps/peercove-ui && npm run tauri dev` で UI を開きます。

### ホスト側(G3 前半)

1. 「ホストを始める」カードに、設定の場所と「新しく作成します」と表示されること
2. 「ルーターのポートを自動で開ける(UPnP)」にチェックしたまま
   **「作成して開始」** → 「ホストとして稼働中」に切り替わること
   (鍵とランダムな 10.x.y.0/24 が自動生成される)
3. メンバー一覧に自分が `[host]` 付き ● で表示されること

### 招待(G4 前半)

4. **「メンバーを招待」** → 名前を入れて「招待を発行」
5. **QR コードとトークン**が表示され、「このトークンはこの画面でしか表示されません」
   の警告が出ること。「トークンをコピー」でクリップボードに入ること
6. ダイアログを閉じる → メンバー一覧に招待した人が ○(オフライン)で現れること
7. もう一度「メンバーを招待」を開いても、**さっきのトークンは表示されない**こと

### メンバー側(G3 後半)

8. メンバー機の UI の「参加する」→「招待トークンで新しく参加」にトークンを
   貼り付けて **「参加する」**
   → 「メンバーとして参加中」になること(鍵も設定も自動生成)
9. ホスト側の一覧で、そのメンバーが ●(オンライン)に変わること
10. 両者で `ping <相手の仮想 IP>` が通ること

### 保存済み設定での再接続(操作性改善)

10a. メンバー側で **「切断」** → 待機中に戻ること
10b. 「参加する」カードに **「保存済みの設定で再接続」** が現れ、
     **「前回のネットワークに再接続」** を押すだけで(トークン不要で)
     同じネットワークに戻れること
10c. 「別の設定ファイルを使う」で、任意の member.toml を選んで参加できること
10d. ホスト側でも、切断後に「ホストを始める」の **「開始」**(トークン/作成不要)で
     同じネットワークを再開できること

### メンバー管理(G4 後半)

11. ホスト側の一覧で **✎(鉛筆)** を押し、名前を変えて Enter
    → 約 10 秒で**メンバー側の一覧にも新しい名前**が反映されること
12. **×** を押す → 確認ダイアログ → 「削除する」
    → メンバー側のログに「ホストから削除されました」が出て ping が止まり、
    ホスト側の一覧からも消えること
13. ホスト側で **「切断」** → 「待機中」に戻ること

### そのほか

14. 「別の設定ファイルを使う」でファイル選択ダイアログが開き、既存の
    host.toml を選んで開始できること
15. 不正なトークン(末尾を削る等)を貼って参加 → 分かりやすいエラーが出ること

> 設定ファイルの既定の置き場所は、Windows `%APPDATA%\app.peercove.desktop\`、
> Linux `~/.config/app.peercove.desktop/` です(UI が表示します)。

## 設定の配置とネットワーク名(M3-0a 以降)

複数ネットワーク対応(ADR-0012)の下準備として、設定は
**1 ネットワーク = 1 ディレクトリ**で置かれます:

```
<設定ディレクトリ>/networks/<ネットワーク名>/
  host.toml + host.key (+ peer-*.psk)   … ホストするネットワーク
  member.toml + member.key (+ member.psk) … 参加しているネットワーク
```

- **ネットワーク名**は `init` 時に決めます(CLI: `peercove-poc init --name game-lan`、
  UI は現状 `home` 固定 — 名前入力は M3-0c で追加)。名前は小文字英数とハイフンに
  正規化され、将来の DNS(`<メンバー>.<ネットワーク名>.peercove.internal`)と
  ディレクトリ名に使われます
- **招待トークンにネットワーク名が入る**ため、参加側では自動的に同じ名前の
  ディレクトリに設定が作られます。旧形式のトークンも従来どおり参加できます
  (既定名 `home` になります)
- **旧配置からの自動移行**: 設定ディレクトリ直下の host.toml / member.toml 一式は、
  UI の起動時に networks/ へ自動で移動されます。**トンネルを切断した状態で
  アップデート後の UI を起動してください**(稼働中に移すと、デーモンの定期再読込が
  旧パスを見失います)
- CLI の `--config` は従来どおり任意のパスを指定できます(networks/ 配下でなくても
  動きます)

### 検証手順(M3-0a: 設定配置と移行)

1. 旧配置(設定ディレクトリ直下に host.toml など)がある状態で UI を起動
   → `networks/home/` へ移動され、「保存済みの設定で再接続」がそのまま使えること
2. UI から新規ホスト作成 → `networks/home/host.toml` に作られること
3. 新しい招待トークンで参加(別マシン)→ ホスト側の `--name`(未指定なら home)と
   同じ名前のディレクトリに member.toml が作られること
4. `peercove-poc init --dir tmp --name "Game LAN"` → 「ネットワーク名: game-lan」と
   正規化されて表示されること
5. 旧バージョンで発行済みのトークンでも参加できること(ネットワーク名は home)

## 複数ネットワークの同時稼働(M3-0b)

デーモンは複数のトンネルを同時に張れます。それぞれ別のインターフェース
(`peercove0`, `peercove1`, …自動採番)・別のサブネットで動きます:

```bash
# 2 つのネットワークを順に開始(ホスト・メンバーの組み合わせは自由)
peercove-poc daemon start-host --config networks/game-lan/host.toml
peercove-poc daemon start-member --config networks/family/member.toml

peercove-poc daemon status          # 稼働中の全ネットワークを表示
peercove-poc daemon stop --config networks/game-lan/host.toml   # 個別に停止
peercove-poc daemon stop            # 1 本だけ稼働中なら --config 省略可
```

- **サブネットが重複するネットワークは起動を拒否**します(init のランダム
  サブネットなら実用上ぶつかりません。エラーが出たらどちらかを作り直し)
- 同一マシンで複数ホストする場合、UI の init は**待受ポートを自動で
  ずらします**(51820, 51821, …)。CLI は `init --port` で指定してください
- UI は現状 1 ネットワークの表示のみ(一覧 UI は M3-0c)。「切断」は
  表示中のネットワークだけを止めます

### 検証手順(M3-0b: 多重トンネル)

1. ホスト用設定を 2 つ作る(例: `init --dir n1 --name one` と
   `init --dir n2 --name two --port 51821`)
2. `daemon start-host` を 2 回(それぞれの設定で)→ 両方成功し、
   `ip link`(Linux)/ `Get-NetAdapter`(Windows)に `peercove0` と
   `peercove1` が見えること
3. `daemon status` に 2 ネットワークが表示されること
4. `daemon stop`(--config なし)→ 「複数のネットワークが稼働中です」の
   エラーになること
5. `daemon stop --config n1/host.toml` → one だけ止まり、two は生きていること
6. 実通信: 別マシン 2 台がそれぞれ別ネットワークに参加し、ホスト経由で
   ping が通ること。**ネットワークをまたいだ ping は通らない**こと(分離の確認)
7. `daemon shutdown` → 残りのトンネルも片付いて終了すること

### トラブルシューティング: デーモンが古い

UI とデーモンは対で更新が必要です。**古いデーモン(サービス)が動いたまま**
新しい UI を使うと、稼働中のトンネルが「停止中」に見える・接続時に
「既にトンネルが動いています。先に stop してください」と言われる、
といった症状になります(M3-0 の実機検証で発覚)。

現在の UI は不一致を検出して「デーモンの更新が必要です」と警告します。
サービスの入れ替え手順:

```powershell
# Windows(管理者 PowerShell)
Stop-Service peercove-daemon
cargo build --release -p peercove-poc
.\target\release\peercove-poc.exe daemon service-uninstall
.\target\release\peercove-poc.exe daemon service-install
```

```bash
# Linux
sudo systemctl stop peercove-daemon
cargo build --release -p peercove-poc   # 新しいバイナリを /usr/bin 等へ配置
sudo systemctl start peercove-daemon
```

## トンネル内 DNS(M3-1)

メンバー同士を **`<名前>.<ネットワーク名>.peercove.internal`** で呼べます
(ADR-0011)。仮想 IP を覚える必要がなくなります。

```
alice.game-lan.peercove.internal   → alice の仮想 IP
host.game-lan.peercove.internal    → ホストの仮想 IP
nas.game-lan.peercove.internal     → カスタムレコード(ホストが登録)
```

仕組み(デーモンが自動で行うので普段は意識不要):

- 各 PC のデーモンが**自分のトンネル IP の UDP 53** で最小 DNS サーバを動かし、
  台帳から導出したレコードを返す(ホストに問い合わせない = ホスト障害の影響なし)
- OS には **`*.peercove.internal` だけ**を内蔵リゾルバへ向けるスプリット DNS を
  設定する(Windows: NRPT / Linux: systemd-resolved の per-link 設定)。
  他ドメインの解決には一切干渉しない。切断・デーモン終了で解除される
  (Windows の NRPT はデーモン起動時にも残骸を掃除する)
- 名前は表示名から自動で決まる(小文字英数とハイフンに正規化)。日本語など
  変換できない名前は `member-<仮想IP末尾>` になる。名前の変更(✎)は
  約 10 秒で DNS にも反映される
- **カスタムレコード**は UI のネットワーク詳細 → 「DNS」から追加・削除できる
  (ホストのみ。設定の `[[dns_record]]` に保存され、全メンバーへ配布される)
- CLI 単発モード(`peercove-poc host` / `member`)では DNS は動かない
  (デーモン経由 = UI か `daemon start-*` で使うこと)

### 検証手順(M3-1: DNS)

前提: 2 台以上でネットワークに接続済み(例: ネットワーク名 `home`、
メンバー `alice`)。

1. メンバー側で `ping host.home.peercove.internal` → ホストの仮想 IP に
   解決されて応答があること
2. ホスト側で `ping alice.home.peercove.internal` → メンバーに届くこと
3. `nslookup alice.home.peercove.internal`(Windows)/
   `resolvectl query alice.home.peercove.internal`(Linux)で
   トンネル IP のサーバから A レコードが返ること
4. **他のドメインに影響がない**こと: `nslookup example.com` が普段どおり
   解決されること
5. ホストの UI: ネットワーク詳細 → 「DNS」→ カスタムレコードを追加
   (例 `nas` / 好きな仮想 IP)→ 約 10 秒で**全員**が
   `nas.home.peercove.internal` を引けること。削除すると引けなくなること。
   メンバー側の「DNS」画面にもレコードが見える(編集は不可)こと
6. メンバー名を UI で変更 → 新しい名前で引けること(旧名は NXDOMAIN)。
   メンバー一覧と「DNS」画面の表示も新しい DNS 名になること
7. 切断 → `nslookup host.home.peercove.internal` が**解決されなくなる**こと
   (スプリット DNS の解除確認)。Windows は
   `Get-DnsClientNrptRule | Where-Object Comment -eq 'PeerCove'` が空になること
8. 複数ネットワーク稼働時: それぞれの `<ネットワーク名>` 階層で引けること

### 検証手順(M3-0c: ネットワーク一覧 UI)

UI がネットワークのカード一覧になり、複数ネットワークを個別に操作できます。

1. UI 起動 → 「ネットワーク」一覧が出ること(設定済みが無ければ案内文)
2. 「新しくホストする」→ 名前(例: `game-lan`)を入れて作成
   → カードが増え、「接続」でホスト開始 → ● 稼働中になること
3. もう 1 つ「新しくホストする」(例: `family`)→ 2 枚のカードがそれぞれ
   独立に接続/切断できること
4. 「招待トークンで参加する」→ 参加後、トークンのネットワーク名のカードが
   増えて稼働中になること
5. カードの「開く」→ 詳細(メンバー一覧・招待・ピア統計)。「← 一覧へ」で戻る
6. カードの「設定」→ そのネットワークの設定が開くこと
7. 停止中のカードの「削除」→ 確認 → カードが消えること
   (**稼働中は削除ボタンが無効**であること)
8. メンバーの参加/切断の OS 通知に**ネットワーク名が含まれる**こと

## メンバー間直接通信(M3-2〜4、段階導入中)

メンバー同士の通信をホスト経由(2 ホップ)から**直接(1 ホップ)**へ
切り替える機能を段階導入しています(ADR-0013)。直接化できない環境でも
従来どおりホスト経由で通信は維持されます。

- **M3-2(実装済み)**: ホストが各メンバーの外部エンドポイント
  (NAT 変換後の IP:port)を台帳に載せて配布する。エンドポイントには観測経過秒が
  付き、古い情報は配布されない(オンラインのメンバーのみ)
- **M3-3(実装済み)**: 各メンバーが配布されたエンドポイントへ双方から WG
  ハンドシェイクを送り合って NAT に穴を開け、成功したら直接通信に切り替える。
  30 秒で穴が開かなければ諦めてホスト経由のまま(5 分後、または相手の
  エンドポイントが変わったら再試行)。直接経路が途絶えたら自動でホスト経由へ
  戻る。**古いエンドポイント(観測から 5 分超)へは試行しない**
- **M3-4(実装済み)**: 再試行は指数バックオフ(5 分 → 10 分 → … → 上限
  1 時間。相手のエンドポイントが変わったら即時)。メンバー一覧に経路バッジ
  (**直接 / 中継 / 確立中…**)を表示

直接通信を使いたくないマシンでは、ネットワーク詳細 → 「設定」の
「**メンバーと直接通信する**」をオフにすると常にホスト経由になります
(既定はオン。約 10 秒で反映、再接続不要)。設定ファイル(member.toml)の
`[interface]` に `direct = false` を書いても同じです。

> **プライバシーについて**: この機能により、あなたの**グローバル IP アドレス
> (外部 IP:port)が同じネットワークの他のメンバー全員に共有**されます
> (Tailscale 等の P2P VPN と同様の仕組みです)。ネットワークは招待制なので
> 共有範囲は招待した相手に限られますが、知られたくない相手をネットワークに
> 招待しないでください。

### 検証手順(M3-2: エンドポイント配布)

挙動が変わらないことの確認(退行チェック)が主です。

1. 2 台以上で接続し、従来どおり ping・DNS・メンバー一覧が動くこと
2. 旧バージョンのメンバーが混ざっても台帳配布が壊れないこと(旧側は
   エンドポイント情報を無視するだけ)

### 検証手順(M3-3/4: メンバー間直接通信)

前提: **全マシンのデーモンを新版に入れ替える**(「トラブルシューティング:
デーモンが古い」参照)。ホスト + メンバー 2 台(A・B)の 3 台構成。
メンバー同士が**別の家(別 NAT)**にあると本来のパンチングを検証できます
(同一 LAN 内でも動作はするが、ルーターのヘアピン対応次第で中継になる)。

1. 3 台で同じネットワークに接続する
2. メンバー A の UI: メンバー一覧の B の行に経路バッジが
   「**確立中…**」→「**直接**」と変わること(数十秒以内)。
   ログ(UI のログビュー / `journalctl -u peercove-daemon`)にも
   「直接接続を試行します(…)」→「直接通信を確立しました(…)」が出る。
   メンバー一覧の下に外部 IP 共有についての説明文が出ること
3. A から B へ `ping <B の仮想 IP>`(または DNS 名)が通ること
4. A の UI のピア統計テーブルに、ホストに加えて **B のピア行**
   (エンドポイント付き)が現れ、最終ハンドシェイクが更新され続けること
5. RTT の改善(任意): メンバー一覧の RTT 表示が中継時代より下がること
   (ホストが地理的に遠い構成ほど顕著)
6. フォールバック: B を切断 → A の B の行がオフラインになりバッジが
   消えること(ログ:「台帳から外れたため直接ピアを解除します」)。
   B を再接続すると自動的にまた「直接」へ戻ること
7. 無効化: A の「設定」→「メンバーと直接通信する」をオフにして保存
   (稼働中でも約 10 秒で反映)→ バッジが「中継」になり、以後 B と
   直接ピアを張らないこと。ping はホスト経由で通り続けること。
   オンに戻せば再び「直接」に戻ること
8. パンチ不可の NAT(例: キャリア回線同士)の場合: 追加から 30 秒で
   「直接接続がタイムアウトしました」が出てバッジが「中継」になり、
   ping はホスト経由で通り続けること(通信は途切れない)

## 招待ディープリンク(M3-5)

招待を **`peercove://join?token=…` のリンク**として渡せます。PeerCove が
インストール済みの相手なら、リンクをクリックするだけで UI が前面に出て
参加フォームにトークンが入った状態になります(未起動なら起動します)。

- ホスト側: 招待ダイアログの「**参加リンクをコピー**」でリンクを取得
  (従来の「トークンをコピー」も残っています。アプリ未導入の相手にはトークンを)
- 受け側: リンクを開くと OS が PeerCove を起動する(初回はブラウザ等が
  確認ダイアログを出すことがあります)
- `peercove://` スキームはインストーラが登録するほか、**UI の起動時にも
  自動登録**されます(開発ビルドでも動きます)
- トークンは秘密情報です。リンクも同じ扱いで、チャット等で渡したら
  相手が参加したことを確認してください(1 回使えば無効です)

### 検証手順(M3-5: 招待ディープリンク)

前提: 受け側マシンで新しい UI を一度起動しておく(スキーム自動登録)。
デーモンの変更はありません(UI だけ新しければよい)。

1. ホスト: メンバーを招待 → 「参加リンクをコピー」
2. 受け側でリンクを開く(チャットアプリのリンククリックの代わり):
   - Windows: **Win+R にリンクを貼って Enter**(または PowerShell で
     `Start-Process "<コピーしたリンク>"`)
   - Linux: `xdg-open "<コピーしたリンク>"`
3. PeerCove のウィンドウが前面に出て(未起動なら起動して)、
   ネットワーク一覧の**参加フォームがトークン入りで開く**こと
4. そのまま「参加する」で従来どおり参加できること
5. UI 稼働中にもう一度リンクを開く → 二重起動せず、既存ウィンドウが
   前面に出てフォームが開くこと
6. 無関係な URL(`peercove://foo`)を開いても何も起きないこと

#### トラブルシューティング: Linux で「指定した場所はサポートしていません」

`gio: peercove://…: 指定した場所はサポートしていません` は
**スキームのハンドラが未登録**という意味です。順に確認してください:

1. 登録状態の確認:
   ```bash
   xdg-mime query default x-scheme-handler/peercove
   ```
   何も出なければ未登録。
2. **deb でインストールした場合**: このトラブルシューティングが追加された
   バージョン以降の deb で入れ直してください(.desktop に
   `MimeType=x-scheme-handler/peercove` と `Exec … %u` が入り、
   インストール時にデータベースを更新します)。確認:
   ```bash
   grep -E "MimeType|Exec" /usr/share/applications/PeerCove.desktop
   ```
3. **開発ビルドの場合**: UI 起動時の自動登録は `update-desktop-database`
   (desktop-file-utils)と `xdg-mime`(xdg-utils)に依存します:
   ```bash
   sudo apt install desktop-file-utils xdg-utils
   ```
   を入れてから **UI をターミナルから起動**してください。登録に失敗して
   いれば標準エラーに警告が出ます。成功すると
   `~/.local/share/applications/peercove-ui-handler.desktop` ができます。

## UI デザインパス(M3-6)

見た目と操作性の一括改善です。デーモン・プロトコルの変更はありません
(UI だけ新しければ動きます)。

- **テーマ切替**: ヘッダーの ◐/☀/☾ ボタンで「システムに合わせる → ライト →
  ダーク」を巡回します。設定はこのマシンにだけ保存されます
- **メンバーの色分け**: メンバーごとに固有色のアバターが付きます。色は
  公開鍵から決まるので、**全員の画面で同じ色**になり、名前を変えても
  変わりません。オンライン状態はアバター右下の緑ドットです
- **転送量・RTT スパークライン**: メンバー一覧に転送速度(受信+送信)の
  直近約 90 秒の推移、「統計」タブに RTT と速度の推移が出ます。履歴は
  UI が開いている間だけ溜まります(閉じるとリセット)
- **タブ構成**: ネットワーク詳細が「メンバー」「統計」のタブになりました。
  今後のチャット(M3-13)・ファイル送信(M3-9)はここへタブとして増えます
- **トレイの拡充**: トレイの右クリックメニューから、ウィンドウを開かずに
  ネットワークごとの「接続」「切断」ができます(ホストとして接続する場合、
  UPnP は UI の既定値と同じくオンです)。トレイアイコンのツールチップに
  稼働状況(「n ネットワーク稼働中」)が出ます

### 検証手順(M3-6: UI デザインパス)

前提: 新しい UI を起動(デーモンは従来のままでよい)。

1. ヘッダーのテーマボタンを 3 回クリック → ライト → ダーク → システム、と
   全画面(一覧・詳細・各ダイアログ)の配色が切り替わること。アプリを
   再起動しても選んだテーマが保たれること
2. トンネルを開始してネットワーク詳細を開く → メンバーごとに色の違う
   アバターが出て、オンラインの相手には緑ドットが付くこと。
   **別のマシンでも同じメンバーは同じ色**であること
3. メンバー間で通信(ping -t や動画再生など)しながら詳細画面を見る →
   メンバー行の折れ線(転送速度)が動き、現在値(〜/s)が更新されること
4. 「統計」タブ → ピアごとに RTT と速度の折れ線・現在値・累計が出ること
5. ウィンドウを閉じてトレイの右クリックメニューを開く →
   「「<ネットワーク名>」に接続 / を切断」が出て、クリックで実際に
   接続/切断されること(数秒後にメニュー表記も追随)。
   トレイにマウスを乗せると稼働状況のツールチップが出ること
6. デーモンを止めた状態でトレイから「接続」→ 失敗が **OS 通知**で出ること

## 検証手順(M2-G2: デスクトップ UI の骨組み)

UI(非特権)から、管理者/root のデーモンの状態が見えることを確認します。
UI の詳細は [apps/peercove-ui/README.md](apps/peercove-ui/README.md) を参照。

1. 初回のみ: `cd apps/peercove-ui && npm install`
2. **デーモンを起動せずに** `npm run tauri dev` → 「PeerCove」ウィンドウが開き、
   「デーモンに接続できません」と起動方法の案内が出ること(「再試行」ボタンあり)
3. 別ターミナル(**管理者 / sudo**)で `peercove-poc daemon run` を起動
   → UI が数秒で「待機中」表示に切り替わること(**UI 側は非特権のまま**)
4. さらに別ターミナル(通常ユーザー)で
   `peercove-poc daemon start-host --config host.toml`
   → UI が「ホストとして稼働中」になり、仮想 IP と設定ファイルが表示されること
5. メンバーを接続する → UI の **メンバー一覧**に ●(オンライン)で現れること。
   ピア統計(エンドポイント・最終ハンドシェイク・rx/tx)が 2 秒ごとに更新されること
6. `peercove-poc remove-peer --config host.toml --name <名前>`
   → 約 10 秒で UI の一覧から消えること
7. `peercove-poc daemon stop` → UI が「待機中」に戻ること
8. `peercove-poc daemon shutdown` → UI が「デーモンに接続できません」に戻ること
   (UI は落ちない)

> 注: 開始・参加・招待の**操作**は M2-G3/G4 で追加します。G2 は「見える」ところまでです。

## 検証手順(M2-G1: デーモン + IPC)

daemon 経由でも従来と同じ疎通ができることを確認します(2 台構成)。

1. Host: `sudo ./target/debug/peercove-poc daemon run` を起動したままにする
2. Host(別ターミナル): `daemon status` → 「待機中」と出ること
3. Host: `daemon start-host --config host.toml` → 「開始しました」
4. Host: `daemon status` → 「ホストとして稼働中」+ 仮想 IP + members が出ること
5. Member A: 従来どおり接続(`sudo ./peercove-poc member --config member.toml`
   か、Member 側でも daemon を使うなら `daemon start-member`)
6. Host↔Member A で ping が通ること(= daemon 経由でもトンネルが機能)
7. Host: `daemon start-host …` を**もう一度**実行 → 「既にトンネルが動いています」
   エラーになること(同時 1 ネットワークの制約)
8. Host: `daemon stop` → ping が止まり、`daemon status` が「待機中」に戻ること
9. Host: `daemon shutdown` → daemon run のプロセスが終了すること
10. 異常系: daemon を起動せずに `daemon status` → 「接続できません」と
    案内が出ること
11. 権限: **手順 1 を管理者/sudo で、手順 2 以降を通常ユーザーで**実行しても
    すべて成功すること(UI が非特権で動く前提のため)

## 検証手順(M1-G3: メンバー削除)

M1-G2 の構成(Host + Member A 接続中)の続きで行います。

1. Member A が接続中(ping が通る状態)であることを確認
2. Host: `remove-peer --config host.toml --name test-a`
3. **Member A のログ**に約 10 秒以内に「ホストから削除されました」の警告が出ること
4. Member A から `ping 100.100.42.1` が**通らなくなる**こと
   (既存セッションの残りで数十秒通る場合があります。3 分以内に完全に停止)
5. Host の `status` の members から test-a が消えること
6. host.toml から該当 `[[peer]]` が消え、コメント等は残っていること
7. **再参加**: Host で `invite --name test-a2` → Member A で
   `join --force` → member 再起動 → 疎通が復活すること
   (削除されたトークンは無効、新しいトークンで再参加できる)

## 検証手順(G-1 前半: keygen・設定読み込み)

管理者権限不要。Windows / Linux どちらでも同じです。

1. `cargo build --workspace` が成功すること
2. `peercove-poc keygen --out test.key` で公開鍵(base64 44 文字)が表示され、`test.key` に**秘密鍵だけ**が保存されること
3. もう一度同じコマンドを実行するとエラーになること(`--force` で成功すること)
4. Linux: `ls -l test.key` が `-rw-------` であること / Windows: `icacls test.key` の出力が自分のアカウントのみであること
5. `examples/member.example.toml` をコピーし、`public_key` を手順 2 の公開鍵に置き換えて `peercove-poc member --config <コピー先>` を実行すると「設定 OK: …」と表示されること
6. `public_key` を適当な短い文字列に変えると、分かりやすいエラーで失敗すること

## ライセンス

MIT OR Apache-2.0(予定)
