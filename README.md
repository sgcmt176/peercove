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
[packaging/licenses/README.md](packaging/licenses/README.md)。

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
(WiX フラグメント `windows/daemon-service.wxs`。方式は ADR-0010、Fable への
確認事項は `docs/peercove-g7b-msi-review-for-fable.md`)。

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

## 使い方

### ログの詳細度

すべてのコマンドで `--log-level <error|warn|info|debug|trace>` が使えます
(`RUST_LOG` 環境変数より優先)。既定は `info` で、通常運用では静かです。

- `debug`: 接続の受理・ピアの追加削除・破棄したパケットの理由など
- `trace`: パケット 1 個ごと(大量に出るので調査時のみ)

```bash
peercove --log-level warn daemon run   # 警告以上だけ
peercove --log-level debug host --config host.toml
```

### クイックスタート(ホストの初期化 → 招待)

```bash
# ホスト側(初期化は管理者権限不要)
./target/debug/peercove init                # host.key + host.toml を生成
sudo ./target/debug/peercove host --config host.toml   # Windows は管理者で
./target/debug/peercove invite --config host.toml --name alice

# メンバー側(トークンを受け取って)
./target/debug/peercove join --token "pcv1.…"
sudo ./target/debug/peercove member --config member.toml
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
sudo ./target/debug/peercove daemon run
# ログを絞りたいときは --log-level warn(既定は info)

# 2) 別ターミナル(ユーザー権限でよい)から操作
./target/debug/peercove daemon start-host --config host.toml
./target/debug/peercove daemon status
./target/debug/peercove daemon logs          # 直近のログ(最大 500 行)
./target/debug/peercove daemon logs --follow # 新しい行を待ち続ける
./target/debug/peercove daemon stop
./target/debug/peercove daemon shutdown

# OS サービスとして常駐させる場合(手動起動の代わり。要管理者/root)
./target/debug/peercove daemon service-install    # 登録 + 起動 + 自動起動
./target/debug/peercove daemon service-uninstall  # 停止 + 登録解除
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
  (Linux は `ps aux | grep peercove`)で `peercove` を探して終了して
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
./target/debug/peercove keygen --out host.key

# 事前共有鍵(PSK)を生成(任意)
./target/debug/peercove keygen --psk --out psk.key
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
sudo ./target/debug/peercove host --config host.toml

# メンバー(トンネル作成 + ホストへ接続)
sudo ./target/debug/peercove member --config member.toml
```

- 起動中は Ctrl+C で終了し、トンネルを自動でクリーンアップします
- 異常終了などで残骸が残った場合は `down` で掃除できます:

```bash
sudo ./target/debug/peercove down --config host.toml
```

> メモ: Windows のトンネルはユーザー空間実装のため、プロセスが終了すると
> アダプタも自動的に消えます。Linux はカーネル実装のため、異常終了時に
> インターフェースが残ることがあります(`down` で削除)。

### メンバーの招待(invite / join)— M1 の推奨フロー

M0 の「keygen → 公開鍵を伝える → add-peer」を 1 ステップにしたものです。

```bash
# ホスト側: 招待トークンを発行(メンバーの鍵と IP を自動生成して登録)
./target/debug/peercove invite --config host.toml --name alice
# → invite.token に保存。--endpoint 203.0.113.5:51820 で外部候補を追加、
#   --psk で事前共有鍵も発行、--print で文字列表示、--qr でターミナルに QR 表示

# メンバー側: トークンから鍵と設定を生成
./target/debug/peercove join --token "pcv1.…" --out-dir .
# ファイル渡しの場合: --token-file invite.token
# → member.key / member.toml(PSK ありなら member.psk)が生成される

# あとは通常どおり接続(管理者/sudo)
sudo ./target/debug/peercove member --config member.toml
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
./target/debug/peercove add-peer --config host.toml \
    --pubkey "MEMBER_A_PUBKEY_B64" --ip 100.100.42.2
```

- `add-peer` は host.toml に `[[peer]]` を追記するだけです。実行中の host
  プロセスは 5 秒間隔で設定を再読込し、新しいピアを自動で取り込みます
  (再起動不要。ピアの削除・変更の反映は M1 で対応)
- 状態確認は同じ設定ファイルを指定して:

```bash
./target/debug/peercove status --config host.toml
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
- **Windows ホストの場合**: peercove.exe の TCP 受信(51821)が
  ファイアウォールでブロックされると member の台帳が更新されません。
  初回のダイアログで「許可」を選んでください

### メンバーの削除(remove-peer)— M1-G3

```bash
# 名前 / 公開鍵 / 仮想 IP のいずれかで指定(管理者権限は不要)
./target/debug/peercove remove-peer --config host.toml --name alice
./target/debug/peercove remove-peer --config host.toml --ip 100.100.42.2
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
   `peercove keygen --out host.key` で鍵を作る
3. `.\target\debug\peercove.exe host --config host.toml` を実行し、
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
   `peercove keygen --out member_a.key` で鍵を作る
   (`public_key` / `endpoint` はこの時点ではダミーで可)
2. `sudo ./target/debug/peercove member --config member.toml` を実行し、
   「トンネル peercove0 を作成しました」が表示されること
3. 別ターミナルで `ip addr show peercove0` に `100.100.42.2/24` が表示され、
   `ip route` に `100.100.42.0/24 dev peercove0` があること
4. Ctrl+C で終了し、`ip link show peercove0` が「does not exist」になること
5. sudo なしで手順 2 を実行すると「root 権限が必要です」エラーになること
6. `sudo ./target/debug/peercove down --config member.toml` が
   (トンネルが無い状態でも)正常終了すること

## 検証手順(G-2: Host–Member 1対1 ping 疎通)

Host(Windows 11)と Member A(Ubuntu)の 2 台で行います。
まず両方の機械で G-1 の検証が通っていることが前提です。

### 準備

1. **Host(Windows・管理者)**
   1. `peercove keygen --out host.key` → 表示された公開鍵を控える(以下 `HOST_PUB`)
   2. `examples\host.example.toml` を `host.toml` にコピー(そのままで可)
2. **Member A(Ubuntu)**
   1. `./peercove keygen --out member_a.key` → 公開鍵を控える(以下 `MEMBER_A_PUB`)
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

1. Host: `.\peercove.exe host --config host.toml` を起動したまま、
   別ターミナルで `add-peer --config host.toml --pubkey "MEMBER_A_PUB" --ip 100.100.42.2`
2. Host のログに(5 秒以内に)「ピア … を追加しました」が出ること
3. Member A: `sudo ./peercove member --config member.toml` を起動
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
  ファイアウォールで peercove.exe の UDP 受信が許可されているか
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
2. `peercove keygen --out member_b.key` → 公開鍵を控える(`MEMBER_B_PUB`)
3. `examples\member.example.toml` を `member_b.toml` にコピーし編集:
   - `private_key_file = "member_b.key"`
   - `address = "100.100.42.3/24"`
   - `public_key = "HOST_PUB"` / `endpoint = "<HostのIP>:51820"`
4. Host 側で `add-peer --config host.toml --pubkey "MEMBER_B_PUB" --ip 100.100.42.3`
5. Member B(管理者)で `.\peercove.exe member --config member_b.toml`
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
   ./target/debug/peercove udp-echo --listen 0.0.0.0:9999
   ```
2. **Member B(Windows)** から:
   ```powershell
   .\target\debug\peercove.exe udp-ping --target 100.100.42.2:9999 --count 10
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
   .\target\debug\peercove.exe host --config host.toml --upnp
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
4. Member A: `./peercove join --token "pcv1.…" --out-dir ~/pc-test`
   - member.key(600)/ member.toml が生成されること
   - member.toml の endpoint が Host の LAN IP になっていること
5. Member A: `sudo ./peercove member --config ~/pc-test/member.toml`
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
cargo build --release -p peercove-cli
copy <wintunの場所>\wintun.dll target\release\

# 2) 既存の秘密ファイル(鍵 と PSK の両方)に SYSTEM の読み取りを付与
#    (1 回だけ。これから invite/init/join で作るものには自動で付きます)。
#    どれか 1 つでも漏れると、その項目の読み込みで os error 5 になります:
icacls "$env:APPDATA\app.peercove.desktop\*.key" /grant "*S-1-5-18:F"
icacls "$env:APPDATA\app.peercove.desktop\*.psk" /grant "*S-1-5-18:F"
# リポジトリ直下の host.key / peer-*.psk を使っている場合はそちらにも同様に

# 3) 手動起動していたデーモンが残っていれば止める(パイプ名が衝突するため)
#    (管理者ターミナルで)peercove daemon shutdown

# 4) サービス登録 + 起動(管理者 PowerShell)
#    ファイアウォールの受信許可(この exe 宛の UDP + TCP)も一緒に追加されます。
#    Session 0 のサービスには「許可しますか?」ダイアログが出ないため、
#    UDP が無いとハンドシェイクが、TCP が無いと台帳配布(コントロール
#    チャネル)が黙って遮断されます
.\target\release\peercove.exe daemon service-install
```

> **PowerShell の注意**: `sc` は PowerShell では Set-Content(ファイル書き込み)の
> エイリアスです。`sc query …` と打つと**何も表示されずに `query` という名前の
> ファイルができます**。必ず `sc.exe` と拡張子まで書くか、`Get-Service` を
> 使ってください。

確認項目:

1. `Get-Service peercove-daemon` が Running であること
   (または `sc.exe query peercove-daemon`)
2. **非管理者**のターミナルで `peercove daemon status` が通ること
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
cargo build --release -p peercove-cli
sudo ./target/release/peercove daemon service-install
```

7. `systemctl status peercove-daemon` が active (running) であること
8. `journalctl -u peercove-daemon -f` にログが流れること
9. UI から参加 → 疎通すること
10. **OS を再起動** → 自動起動していること(`systemctl status`)
11. `sudo ./target/release/peercove daemon service-uninstall` →
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
3. `which peercove` が `/usr/bin/peercove`、
   `ls /usr/lib/systemd/system/peercove-daemon.service` が在ること
4. アプリ一覧か `peer-cove`(UI)を起動 → ホスト/参加 → 疎通すること
5. **OS 再起動** → `systemctl status peercove-daemon` が自動で running
6. `sudo apt remove peercove`(パッケージ名は `dpkg -l | grep -i peercove` で確認)→
   - `systemctl status peercove-daemon` が not-found(prerm が停止・無効化)
   - `/usr/bin/peercove` と unit ファイルが消えていること
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
5. CLI でも見えること: `peercove daemon status` に `rtt 0.4 ms` が付く

### ログビュー(G5)

6. ヘッダ右上の **☰** を押す → 「デーモンのログ」が開き、
   `トンネルを開始しました` 等の行が時刻付きで並ぶこと
7. 1 秒ごとに新しい行が追記され、**最新行を追う**にチェックが入っていれば
   自動で最下部までスクロールすること
8. 「表示レベル」を `WARN 以上` にすると INFO 行が消えること
9. デーモンを **`peercove --log-level debug daemon run`** で起動し直すと、
   `DEBUG` を選んだときに詳細な行が出ること
   (逆に `--log-level warn` で起動すると、ログビューにも warn 以上しか出ません)
10. ターミナルからも同じものが読めること: `peercove daemon logs --follow`
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
    (デーモンが維持している。`peercove daemon status` で確認)
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

準備: 両方の PC で `peercove daemon run`(管理者 / sudo)を起動し、
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

- **ネットワーク名**は `init` 時に決めます(CLI: `peercove init --name game-lan`、
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
4. `peercove init --dir tmp --name "Game LAN"` → 「ネットワーク名: game-lan」と
   正規化されて表示されること
5. 旧バージョンで発行済みのトークンでも参加できること(ネットワーク名は home)

## 複数ネットワークの同時稼働(M3-0b)

デーモンは複数のトンネルを同時に張れます。それぞれ別のインターフェース
(`peercove0`, `peercove1`, …自動採番)・別のサブネットで動きます:

```bash
# 2 つのネットワークを順に開始(ホスト・メンバーの組み合わせは自由)
peercove daemon start-host --config networks/game-lan/host.toml
peercove daemon start-member --config networks/family/member.toml

peercove daemon status          # 稼働中の全ネットワークを表示
peercove daemon stop --config networks/game-lan/host.toml   # 個別に停止
peercove daemon stop            # 1 本だけ稼働中なら --config 省略可
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
cargo build --release -p peercove-cli
.\target\release\peercove.exe daemon service-uninstall
.\target\release\peercove.exe daemon service-install
```

```bash
# Linux
sudo systemctl stop peercove-daemon
cargo build --release -p peercove-cli   # 新しいバイナリを /usr/bin 等へ配置
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
- DNS 名は **M3-14a 以降は表示名から独立**した設定(下の
  「表示名と DNS 名の分離」参照)。アップグレード前に登録されたメンバーは
  従来どおり表示名から自動導出される(小文字英数とハイフンに正規化。日本語など
  変換できない名前は `member-<仮想IP末尾>`)
- **カスタムレコード**は UI のネットワーク詳細 → 「DNS」から追加・削除できる
  (ホストのみ。設定の `[[dns_record]]` に保存され、全メンバーへ配布される)。
  M3-14b 以降はターゲットにメンバーを指定でき、IP に自動追随する(下の
  「カスタム DNS レコードの拡張」参照)
- CLI 単発モード(`peercove host` / `member`)では DNS は動かない
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

## 表示名と DNS 名の分離(M3-14a)

メンバー一覧に出る**表示名**(日本語・空白可)と、DNS に使う **DNS 名**
(小文字英数とハイフンのみ)を別々に設定できます(ADR-0021)。

- **正本はホストの host.toml**(`[interface].dns_name` / `[[peer]].dns_name`)。
  台帳と一緒に全員へ配布される
- **invite が IP 割当と同時に DNS 名を確定・永続化**する(既定は表示名の
  正規化ラベル、日本語などは `member-<仮想IP末尾>`)。以後、表示名を変えても
  DNS 名は変わらない
- **重複・予約語(`localhost` `internal` `peercove` `dns` `gateway` など。
  `host` はホスト専用)は設定時にエラー**になる(自動リネームしない)
- 変更方法:
  - ホスト: メンバー一覧の DNS 名の横の ✎(全メンバー+自分)、または
    設定画面の「DNS 名」欄(自分の分)
  - メンバー: 自分の行の DNS 名の横の ✎。**接続中のみ**変更でき、ホストが
    検証してから適用される(重複などは理由つきで拒否される)
- アップグレード前に登録されたメンバーは従来どおり表示名から導出される
  (一度 ✎ で設定すると確定・永続化される)

### 検証手順(M3-14a: 表示名と DNS 名の分離)

前提: 新しいバイナリで 2 台以上が接続済み(例: ネットワーク名 `home`)。

1. ホストで新規 invite(表示名 `テスト機`)→ join → host.toml の該当
   `[[peer]]` に `dns_name = "member-N"` が書かれていること
2. ホスト UI: メンバー一覧で対象の DNS 名の ✎ → `game-pc` に変更 →
   約 10 秒で全員のメンバー一覧の DNS 名が
   `game-pc.home.peercove.internal` になり、`ping game-pc.home.peercove.internal`
   が通ること
3. **表示名との独立**: 同じメンバーの表示名(名前の ✎)を `ゲーム機` に変更
   → DNS 名は `game-pc` のまま変わらないこと
4. **メンバー本人の変更**: メンバー側 UI で自分の行の DNS 名の ✎ →
   `my-linux` に変更 → 「DNS 名を変更しました」と出て、約 10 秒で全員に
   反映されること
5. **重複エラー**: 別メンバーの DNS 名を `my-linux` に変更しようとすると
   「既に使用されています」と拒否されること
6. **予約語エラー**: DNS 名を `localhost` や `host`(メンバー)にすると
   拒否されること。日本語だけの DNS 名(例 `開発機`)も
   「英数字を含めてください」と拒否されること
7. **ホスト自身**: ホストの設定画面の「DNS 名」に `game-room` を入れて保存 →
   `game-room.home.peercove.internal` がホストの仮想 IP に解決されること。
   空欄に戻すと `host.home.peercove.internal` に戻ること
8. **正規化**: DNS 名に `Alice PC` と入力すると `alice-pc` として確定される
   こと(小文字化・空白→ハイフン)

## カスタム DNS レコードの拡張(M3-14b)

カスタムレコード(ホストのみ管理)が固定 IP 以外も指せるようになりました
(ADR-0022)。ネットワーク詳細 → 「DNS」の追加フォームで指定します。

- **エイリアス / サービス名**: ターゲットに**メンバー**を選ぶと、そのメンバーの
  仮想 IP に自動で追随する別名になる(`gamehost.home.peercove.internal` →
  山田さんの PC)。ゲームサーバーを別の PC に移すときはレコードのターゲットを
  付け替えるだけ(削除 → 同名で追加)
- **端末配下サブドメイン**: 「配置」で親メンバーを選ぶと
  `web.host.home.peercove.internal` のような階層になる。親が違えば同じ名前でも
  共存できる(`web.host` と `web.alice`)
- **LAN 機器**: 「配置」で親メンバーを選び、ターゲットを IP 指定にすると
  サブネットルーター(M3-7)配下の機器に名前を付けられる
  (`printer.alice.home.peercove.internal` → 192.168.10.50)。
  **その親の広告サブネット内の IP のみ**登録できる。ホスト自身の LAN 機器は
  従来どおり最上位の IP レコードで登録する(範囲制限なし)
- メンバー参照は公開鍵で保存されるため、**DNS 名の変更・鍵の更新・仮想 IP の
  変わる再招待をまたいでも追随**する。メンバーを削除すると、そのメンバーを
  参照するレコードも一緒に消える
- 旧バージョンのメンバーが混ざっていても、エイリアスは従来レコードとして
  そのまま引ける(サブドメインのドット付き名だけ旧バージョンでは解決されない)
- 注意: `member` / `under` を書いた host.toml は**旧バージョンでは読めない**
  (アップグレード前のバイナリで起動しない)。ホストを先に更新すること

### 検証手順(M3-14b: カスタムレコードの拡張)

前提: 新しいバイナリで 2 台以上が接続済み(例: ネットワーク名 `home`、
メンバー `alice`)。ホスト UI のネットワーク詳細 → 「DNS」で操作する。

1. **エイリアス**: 名前 `gamehost`、ターゲット「メンバー: alice」で追加 →
   host.toml に `member = "<公開鍵>"` の `[[dns_record]]` が書かれ、約 10 秒で
   **全員**が `ping gamehost.home.peercove.internal` で alice に届くこと。
   ホストの一覧に「→ alice」と現在の IP が出ること。**メンバー側の「DNS」
   画面にも配信されたレコードが見える**こと(閲覧のみ。検証フィードバックで
   status 経由の表示に修正済み)
2. **サブドメイン**: 名前 `web`、ターゲット「メンバー: ホスト」、配置
   「ホスト の配下」で追加 → `web.host.home.peercove.internal` がホストの
   仮想 IP に解決されること。さらに同じ名前 `web` を alice の配下にも
   追加できる(親が違えば同名可)こと
3. **DNS 名変更への追随**: alice の DNS 名を ✎ で `game-pc` に変更 →
   約 10 秒で `web.game-pc.home.peercove.internal` に変わり、
   `gamehost.…` はそのまま引けること
4. **LAN 機器**: alice に広告サブネット(例 `192.168.10.0/24`、M3-7)を設定
   した上で、名前 `printer`、ターゲット IP `192.168.10.50`、配置「alice の配下」
   で追加 → `printer.game-pc.home.peercove.internal` が 192.168.10.50 に
   解決されること。**範囲外**(例 `192.168.99.50`)は「広告サブネットの
   範囲外です」と拒否されること
5. **鍵更新への追随**: alice 側 UI で「鍵を更新」→ 再接続後も
   `gamehost.…` / `web.game-pc.…` が引き続き解決されること
   (host.toml の `member` / `under` が新しい公開鍵に書き換わっている)
6. **メンバー削除の掃除**: テスト用メンバーを削除 → そのメンバーを参照する
   レコードが一覧から消えること(固定 IP レコードは残る)
7. **重複**: 同じ名前 + 同じ配置のレコードを二重追加するとエラーになること。
   最上位でメンバーの DNS 名(例 `game-pc`)や予約語(`localhost`)は
   拒否されること

## サービス情報と URL コピー(M3-14c)

カスタム DNS レコードに任意の**スキーム**と**ポート**を付け、接続先 URL を
ホスト・メンバー双方の DNS 画面に表示できます(ADR-0023)。URL は
「URL をコピー」でクリップボードへコピーできます。

```toml
[[dns_record]]
name = "gamehost"
member = "<公開鍵>"
scheme = "http"
port = 8080
```

上の例では `http://gamehost.home.peercove.internal:8080/` と表示されます。
`http` の 80、`https` の 443 は既定ポートとして URL から省略します。
スキームなしでポートだけ指定した場合は URL にせず
`gamehost.home.peercove.internal:8080` と表示します。

- スキームは先頭が小文字英字、以降が小文字英数字または `+` `.` `-`、
  31 文字以内。UI からの入力は小文字化して保存する
- ポートは 1〜65535。スキーム・ポートはどちらも省略可
- これは UI 用のサービス情報であり、DNS サーバーの応答は従来どおり
  **A レコードのみ**。SRV レコードは配信しない
- サービス情報なしの既存レコードは表示・ワイヤ形式とも変わらない。
  `scheme` / `port` を書いた host.toml は旧バージョンでは読めないため、
  ホストを先に更新すること

### 検証手順(M3-14c: サービス情報と URL コピー)

前提: 新しいバイナリでホストと 1 台以上のメンバーが接続済み。ホスト UI の
ネットワーク詳細 →「DNS」で操作する。

1. 名前 `gamehost`、任意のターゲット、スキーム `http`、ポート `8080` で追加 →
   ホストの一覧に `http://gamehost.<ネットワーク名>.peercove.internal:8080/` が
   表示され、host.toml に `scheme = "http"` / `port = 8080` が書かれること
2. 約 10 秒後、**接続中の全メンバー**の DNS 画面にも同じ URL が表示されること
3. 「URL をコピー」→ ブラウザのアドレス欄へ貼り付けて開けること。
   実際のサービスを起動している場合はそのサービスへ到達できること
4. サービス情報を付けていない既存レコードは、従来どおり FQDN と IP だけが
   表示されること。ポートだけのレコードは `FQDN:ポート` と表示され、
   URL コピーボタンは出ないこと
5. `HTTP` は UI 経由では `http` に正規化されること。host.toml を手編集して
   `scheme = "1http"` / 32 文字超、または `port = 0` にすると、設定再読込時に
   理由つきで拒否されること。`http:80` / `https:443` は URL でポートが
   省略されること

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
  直接経路が途絶えたら自動でホスト経由へ戻る。
  **古いエンドポイント(観測から 5 分超)へは試行しない**
- **M3-4(実装済み)**: メンバー一覧に経路バッジ(**直接 / 中継 / 確立中…**)を
  表示
- **試行の無害化と固定間隔再試行(ADR-0019、検証フィードバックで改訂)**:
  試行中は AllowedIPs 空の「プローブ」としてピアを張るため、**試行が失敗しても
  そのメンバーとの通信(チャット・ファイル送信・ping)は中継経由で途切れない**。
  ハンドシェイクを確認してから経路を直接側へ切り替える。
  再試行は固定間隔(試行 45 秒 / 周期 60 秒。旧: 指数バックオフ 5 分〜1 時間)で、
  両側の試行タイミングがどれだけずれても窓が必ず重なるため、経路が開通し得る
  環境ならいずれ直接化される。2 回目以降の再試行はログ(debug)にも経路バッジにも
  出さず、裏で静かに続ける

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
8. パンチ不可の NAT(例: キャリア回線同士)の場合: 追加から 45 秒で
   「直接接続がタイムアウトしました(中継のまま、裏で再試行を続けます)」が
   出てバッジが「中継」になり、ping はホスト経由で通り続けること
   (通信は途切れない)

### 検証手順(検証フィードバック: 直接接続の無害化 ADR-0019)

前提: **全マシンのデーモンを新版に入れ替える**(UI の変更はなし)。
ホスト + メンバー 2 台(A・B)の 3 台構成。

1. **試行中に通信が途切れないこと**: A と B が「中継」の状態(直接未確立)から
   直接接続の試行が始まっても、その間 A → B の ping・チャットが
   途切れず届き続けること(旧版では試行中の最大 30 秒間、B 宛の通信が
   すべて捨てられていた)
2. **直接化の自己回復**: A・B のデーモンを**わざと時間をずらして**再起動する
   (例: A を再起動 → 2 分待って B を再起動)。両方が起動してから
   数分以内に経路バッジが「直接」になること(旧版ではタイミングがずれると
   指数バックオフで数十分〜回復不能だった)
3. **ログが静かなこと**: 直接化できない環境でも、UI のログビューに出る
   直接接続関連のログは初回の「試行します」「タイムアウトしました」の
   1 往復だけで、以後 60 秒ごとの再試行はログに出ないこと
4. **台帳ログの削減**: メンバーのログビューに「台帳を受信しました」が
   定期的に(毎分)出続けないこと。メンバーの増減・オンライン/オフライン・
   遮断の変更・DNS レコードの変更があったときだけ出ること
5. 退行チェック: 直接化できる環境で従来どおり「確立中…」→「直接」に変わり、
   RTT が改善すること。「メンバーと直接通信する」オフで直接化しないこと

## デバイス鍵のローテーション(M3-11)

招待方式(ADR-0005)ではメンバーの秘密鍵をホストが生成してトークンに同梱する
ため、「秘密鍵がトークンという文字列で経路を通る」「ホストがメンバーの秘密鍵を
知っている」という状態が残ります。M3-11 はこれを解消します(ADR-0020):

- **参加直後に一度だけ自動で**、メンバーが端末上で新しい鍵ペアを生成し、
  **公開鍵だけ**をコントロールチャネルでホストへ届けて差し替えます
  (新しい秘密鍵は端末から一切出ません)。切り替え時に数秒の断があります
- 既存のメンバーも(鍵はすべてトークン経由なので)**新版デーモンでの初回接続時に
  一度だけ**自動更新されます。完了すると member.toml に `key_source = "self"` が
  書かれ、以後は繰り返されません
- 手動での再更新: メンバーの UI → ネットワーク詳細 → ⚙ 設定 → 「鍵を更新」
  (招待トークンの漏洩が心配になったときなどに)
- 障害に強い設計: 新しい鍵は依頼を送る**前に** `member.key.new` として保存され、
  応答がどこで失われても「今の鍵で 45 秒受信がなければもう一方の鍵で入れ直す」
  ことで自力で収束します(鍵を失って締め出される状態がない)
- ホスト側は host.toml の `public_key` が書き換わるだけです(名前・IP・PSK・
  ACL・サブネットは維持)。他メンバーの直接接続は台帳の更新で自動的に張り直されます
- 注意: ローテーション後の member.toml(`key_source` あり)は**旧バージョンの
  peercove では読めません**(明示エラー)。全マシンのデーモンを先に
  更新してください。ホストが旧版のままの場合、依頼は無視され現行鍵のまま
  動き続けます(害はなく、ホスト更新後の接続で自動的に更新されます)

### 検証手順(M3-11: 鍵ローテーション)

前提: 全マシンのデーモンと UI を新版に入れ替える(ホストのデーモン更新が必須)。

1. **自動更新**: メンバーを接続 → ログビューに「デバイス鍵の更新をホストへ
   依頼しました」→「デバイス鍵を更新しました(新しい鍵で接続し直します)」が
   出て、数秒の再接続のあと通信が回復すること。ホスト側ログに
   「メンバー x.x.x.x(名前)の公開鍵を更新しました」が出て、host.toml の
   該当 `[[peer]]` の `public_key` が変わっていること
2. **一度きりであること**: member.toml の `[interface]` に
   `key_source = "self"` が入り、切断 → 再接続しても再度更新されないこと
3. **通信の回復**: 更新後、ping・チャット・ファイル送信が従来どおり動き、
   メンバー一覧も再受信できること。ホストログに「接続」と「切断」が 5 秒ごとに
   繰り返されないこと。直接通信できる環境では経路バッジが数分以内に「直接」へ戻ること
4. **手動更新**: メンバーの UI → ネットワーク詳細 → ⚙ 設定 → 「鍵を更新」→
   確認ダイアログ → 数秒の断のあと回復し、ホストの host.toml の鍵がまた
   変わること(2 回目以降も動く)
5. (任意)**古いトークンの無効化**: ローテーション済みメンバーの古い招待
   トークンで `join` し直すと、鍵がもう登録されていないため接続できないこと
   (= トークンが漏れても過去のものは使えない)

## サブネットルーター(M3-7)

あるメンバーの背後 LAN(NAS など、PeerCove を入れられない機器)へ、
全メンバーがトンネル越しに届くようにします(ADR-0014)。

- **ホストの設定が正本**です。**ホストのマシンで host.toml に対して**実行します
  (ルーター役のメンバー側で実行するものではありません。member.toml には
  効きません):
  ```bash
  # Windows ホストの例(PowerShell)
  # 設定(最後に CIDR を指定。スペース区切りで複数可)
  .\peercove.exe subnet --config "$env:APPDATA\app.peercove.desktop\networks\<名前>\host.toml" --name alice 192.168.10.0/24
  # 解除(CIDR を付けずに実行すると空になり、約 10 秒で全員から経路が消える)
  .\peercove.exe subnet --config "$env:APPDATA\app.peercove.desktop\networks\<名前>\host.toml" --name alice
  ```
  (`[[peer]]` に `subnets = ["192.168.10.0/24"]` を書くのと同じ。
  `--name` はメンバー一覧の表示名)
- Docker が入っている Linux(FORWARD 既定 DROP)でも動くよう、転送の
  FORWARD 許可ルールも自動で入ります(解除・切断で対削除)
- **UI から**(M3-7b): ホストはネットワーク詳細のメンバー行の 🖧 ボタンで
  サブネットを編集できます(空にして保存で解除)。設定されたサブネットは
  全メンバーのメンバー行に CIDR バッジとして表示されます

### 検証手順(M3-7b: サブネットルーター UI)

1. ホストの UI: メンバー行の 🖧 → CIDR を入力して保存 → CLI 手順 3〜4 と
   同様に約 10 秒で配布・疎通できること
2. 全マシンの UI で、そのメンバー行にサブネットのバッジが出ること
3. 不正な CIDR(`abc` 等)や仮想サブネットと重なる指定はエラーが表示されること
4. 空にして保存 → バッジが消え、経路も解除されること
- **ルーター役(広告される側)は Linux のみ**(V1)。転送と SNAT
  (MASQUERADE)を自動設定します(要 iptables)。Windows は「届く側」
  としてのみ動作します
- サブネット宛の通信は**常にホスト経由**です(直接通信の対象外)
- 制約: LAN 機器からはアクセス元がすべてルーター役の端末に見えます(SNAT)。
  ルーター役自身の LAN と広告サブネットのレンジが重複していると届きません。
  NAS への名前付けは DNS 管理画面のカスタムレコードが使えます
- 注意: `subnets` を書いた host.toml は**旧バージョンの peercove では
  読めません**(明示エラー)。ホストを先に更新してください

### 検証手順(M3-7a: サブネットルーター)

前提: ホスト(どちらの OS でも可)+ ルーター役 Linux メンバー + もう 1 台の
メンバー。全マシンでデーモンを新しいバイナリに入れ替えて再接続する。

1. ルーター役 Linux メンバーの LAN 側 IP を確認する(例 `192.168.10.5`。
   VM なら NAT ネットワーク側の IF でよい)
2. ホスト: `peercove subnet --config <host.toml> --name <ルーター役> 192.168.10.0/24`
3. 約 10 秒待つ。ルーター役のログに「サブネットルーターを有効化しました」、
   他メンバーのログに「サブネット 192.168.10.0/24 への経路を追加しました」が出ること
4. 他メンバー・ホストから LAN 機器へ ping / アクセスできること
   (まずはルーター役自身の LAN IP `192.168.10.5` 宛てが手軽):
   `ping 192.168.10.5`
5. `ip route`(Linux)/ `route print -4`(Windows)に 192.168.10.0/24 →
   トンネル IF の経路があること
6. ホストで解除(`subnet --config <host.toml> --name <ルーター役>`)→
   約 10 秒で経路が消え、ping が通らなくなること
7. ルーター役で `sudo iptables -t nat -L POSTROUTING` → 解除後に
   peercove-subnet-router のルールが残っていないこと(切断時も同様)

#### トラブルシューティング: Docker ネットワークを疑似 LAN にすると届かない

Docker 28 以降は、コンテナ IP へ**ブリッジ外の IF から直接届くパケット**を
raw テーブルで遮断します(`iptables -t raw -L PREROUTING -vn` に
`daddr <コンテナIP> iifname != br-… drop` が見える)。経路・FORWARD・NAT が
すべて正しくても FORWARD のカウンタに現れずに落ちるのが特徴です。
検証には保護を外したネットワークを使ってください:

```bash
docker network create --subnet 172.30.50.0/24 \
  -o com.docker.network.bridge.gateway_mode_ipv4=nat-unprotected testlan
```

実際の NAS などの物理 LAN 機器ではこの問題は起きません(Docker 固有の保護)。

## ファイル送信(M3-9)

メンバーへトンネル内でファイルを送れます(ADR-0015。Taildrop 相当)。
各デーモンが自分の仮想 IP の TCP 51822 で待受け、送信側が相手の仮想 IP へ
直接接続します。接続元の仮想 IP は WG が保証するため、台帳のメンバー以外
(広告サブネット内の LAN 機器など)からの接続は拒否されます。

- **受信は自動**です(相手の承認は不要)。受信ボックス
  `networks/<ネットワーク>.inbox/` に保存され、完全性は SHA-256 で検証されます
  (壊れた転送はファイルごと破棄)
- 送るには**相手がオンライン**である必要があります(オフライン宛の預かりは
  将来課題 — ADR-0015)
- **受信サイズ上限は既定 100 MB**です。**受け取る側**の設定
  `[interface] max_recv_file_mb = 500` で変更できます(0 で無制限。
  稼働中でも約 10 秒で反映)。超える送信は「相手が受信を拒否しました:
  サイズが受信側の上限(◯◯ MB)を超えています」になります。
  UI からの変更は M3-9b で対応予定。注意: この設定を書いた toml は
  旧バージョンの peercove では読めません(明示エラー)
- CLI(デーモン経由で送信し、進捗を表示します):
  ```bash
  # Windows の例(PowerShell)。--to は表示名または仮想 IP
  .\peercove.exe send-file --config "$env:APPDATA\app.peercove.desktop\networks\<名前>\host.toml" --to alice .\photo.jpg
  ```
- 相手が旧バージョン(リスナーなし)の場合は「相手に接続できません」に
  なります
- **UI から**(M3-9b、M3-13e で改良): メンバータブの「**📤 ファイル送信**」
  ボタン → ファイルを選び、**送る相手をチェックボックスで選んで**送信
  (複数人へ一度に送れます。オフラインの相手は選べません)。進捗は
  「受信」タブに宛先ごとに出ます。受け取った側は OS 通知 +「受信」タブの
  受信ボックスから「保存」(任意の場所へ移動)か「削除」を選びます。
  受信サイズ上限は設定画面の「受信ファイルの上限(MB)」で変更できます
- ハブ&スポーク経由の転送はホストで復号・再暗号化されます(直接通信が
  成立している相手とは E2E)。ファイル名・本文はログに出しません

### 検証手順(M3-9a: ファイル送信 CLI)

前提: 2 台以上でデーモンを新しいバイナリに入れ替えて再接続する
(送る側・受け取る側の両方)。UI は次ゴール(M3-9b)なので CLI で確認する。

1. 送信側: `peercove send-file --config <設定> --to <相手の表示名> <ファイル>`
   → 進捗(%)が進み「送信が完了しました」と出ること
2. 受信側: `networks/<ネットワーク>.inbox/` に同名ファイルが保存され、
   内容が一致すること(隣に送信者情報の `.pcvmeta` ができる)
3. 同じファイルをもう一度送る → `名前 (1).拡張子` として保存されること
   (上書きされない)
4. 存在しない宛先(`--to 10.0.0.99` 等)やオフラインの相手を指定すると
   分かりやすいエラーになること
5. 大きめのファイル(数百 MB)でも完走し、進捗が動くこと
6. (片方が旧バイナリの場合)送信が「相手に接続できません(相手のトンネルが
   動いていないか、相手の PeerCove が旧バージョンです)」で失敗すること
7. 受信サイズ上限: 既定のまま 100 MB 超のファイルを送ると
   「サイズが受信側の上限(100 MB)を超えています」で拒否されること。
   受け取る側の `[interface]` に `max_recv_file_mb = 1000` を書くと
   約 10 秒後(再起動不要)に同じファイルが通ること

### 検証手順(M3-9b: ファイル送信 UI)

前提: 全マシンで `git pull` + `npm ci` + UI 再起動。**デーモンも最新へ
入れ替える**(受信サイズ上限がデーモン側の機能のため。また、上限を
既定以外にすると設定に `max_recv_file_mb` が書かれ、それより古い
デーモンはその設定を読めなくなる)。

1. 送信: メンバータブの「📤 ファイル送信」→ ファイルを選び相手をチェック →
   「受信」タブに進捗バーが出て「完了」になること
2. 受信側: OS 通知「ファイルを受信しました」が出て、「受信」タブに
   バッジ(件数)が付くこと。受信ボックスの行に送信者・サイズ・日時が
   出ること
3. 「保存」→ 保存先を選ぶ → ファイルが移動し、一覧から消えること。
   「削除」でも消えること
4. 設定画面の「受信ファイルの上限(MB)」を小さくして保存 → 約 10 秒後、
   それを超えるファイルの送信が「受信」タブの転送一覧で
   「失敗: サイズが受信側の上限(◯◯ MB)を超えています」になること
5. ファイル送信ダイアログでオフラインのメンバーが選べない(チェック不可)こと

## チャット(M3-13)

メンバーとトンネル内でチャットできます(ADR-0016。外部サーバー不要)。
ファイル送信(M3-9)と同じ基盤(各デーモンの TCP 51822、台帳照合)を使い、
1:1・ネットワーク全体宛・**任意グループ宛(M3-13c)**に対応しています。

- **履歴は各端末のローカルのみ**に保存されます
  (`networks/<ネットワーク>.chat.jsonl`、上限 10,000 通。端末間の同期は
  ありません)
- 送れるのは**オンラインのメンバーだけ**です(ファイル送信と同じ)。
  全体宛・グループ宛は「送信時にオンラインだったメンバー」にしか届きません
- 本文の上限は 8 KB です
- CLI(デーモン経由):
  ```bash
  # 1:1(--to は表示名または仮想 IP)
  .\peercove.exe chat --config <設定> --to alice "こんにちは"
  # ネットワーク全体へ
  .\peercove.exe chat --config <設定> --all "全員へ"
  # グループへ(グループ名または ID。作成は UI から)
  .\peercove.exe chat --config <設定> --group 開発チーム "グループへ"
  # 履歴の表示(--follow で新着を待ち続ける)
  .\peercove.exe chat-log --config <設定> --follow
  ```
- 相手が旧バージョンの場合は届きません(履歴に「(送信失敗)」と付きます)
- ハブ&スポーク経由のチャットはホストで復号・再暗号化されます(直接通信が
  成立している相手とは E2E)。本文・グループ名はログに出しません
- **UI から**(M3-13b): ネットワーク詳細の「チャット」タブ。左の会話リスト
  (「全体」+ グループ + メンバー)から相手を選び、下の入力欄から送ります
  (Enter で送信、Shift+Enter で改行)。新着は OS 通知 + タブと会話リストの
  未読バッジで分かります(既読の記録はこの端末のローカル)

### グループ(M3-13c)

特定のメンバーだけのグループチャットが作れます(LINE のグループと同じ
使い勝手: 作成・メンバー追加・改名・退出)。

- 作成: チャットタブの会話リスト先頭「**＋ グループ作成**」→ 名前を付けて
  メンバーを選ぶ(オフラインのメンバーも入れられます)
- 管理: グループの会話を開いてヘッダの「**管理**」→ 改名・メンバー追加・
  退出。退出しても履歴はこの端末に残ります(会話リストからは消えます)
- グループの情報は**サーバーに置かず、メンバー同士が直接配り合います**
  (`group_update`、保存先は `networks/<ネットワーク>.groups.json`)。
  各メンバーは相手から**受領確認(ack)が取れるまで約 30 秒間隔で
  自動再送**するので、オフラインだったメンバーにもオンラインに戻ってから
  30 秒程度で届きます
- 作成・メンバー追加・退出・改名は、会話の中に**お知らせ**(中央のグレーの
  1 行)として出ます。改名は変更前後の名前が両方分かります
  (例: `グループ名が「A」から「B」に変わりました`)。お知らせは未読数や
  OS 通知の対象になりません
- 整合性は**最新リビジョン勝ち**です。2 人が同時に別々の変更(例: 同時に
  改名)をすると片方が失われます(小規模グループ前提の割り切り)
- 旧バージョンのデーモンにはグループ情報が届きません(グループ機能を使う
  マシンは全員このバージョン以降にしてください。1:1 と全体宛は従来どおり
  相互運用できます)

### チャットからのファイル送信 + ドラッグ&ドロップ(M3-13d)

会話の中からファイルを送れます(LINE と同じ感覚)。

- 送り方は 2 つ: 入力欄の左の **📎** でファイルを選ぶか、**ファイルを
  会話画面へドラッグ&ドロップ**する(確認ダイアログ → 送信。複数可)
- 宛先は**いま開いている会話**(1:1 / 全体 / グループ)。全体・グループ宛は
  オンラインの対象メンバーへの個別転送になります(履歴のバブルは 1 つ)
- 会話には**ファイルバブル**(📎 ファイル名・サイズ・転送中は進捗バー)が
  出ます。受け取った側はバブルの「**保存**」で好きな場所へ移せます
  (実体は従来どおり受信ボックス。受信タブからも操作できます)
- **画像・動画・音声はその場でプレビュー**されます(画像はクリックで
  拡大表示)。送信側は元ファイル、受信側は受信ボックスの実体を表示するため、
  保存・削除でファイルを動かした後は通常のファイル表示に戻ります。
  動画は OS の対応形式のみ再生できます(Linux は入っているコーデック次第)
- 相手が旧バージョン(M3-9 対応)の場合、ファイル自体は届きますが相手の
  チャットには出ません(受信ボックスに入るだけ)
- 進捗バーは転送一覧(直近分のみ保持)と突き合わせて出すため、古いファイル
  バブルは進捗なし(名前とサイズのみ)で表示されます。保存済み・削除済みの
  ファイルのバブルで「保存」を押すとエラーになります(実体がもう無いため)

### 検証手順(M3-13a: チャット CLI)

前提: 2 台以上でデーモンを新しいバイナリに入れ替えて再接続する。
UI は次ゴール(M3-13b)なので CLI で確認する。

1. A → B: `chat --to <Bの表示名> "こんにちは"` → B の
   `chat-log --follow` に数秒内に `<Aの名前>: こんにちは` が出ること
2. B → A に返信 → A の `chat-log --follow` に出ること。A 自身の送信分は
   `[→ <相手>] 自分: …` と表示されること
3. 3 台以上あれば: `chat --all "全員へ"` → オンラインの全メンバーの
   履歴に `[全体]` 付きで出ること
4. オフラインの相手へ `--to` で送る → 「オフラインです」のエラーになること
5. デーモンを再起動(またはトンネルを入れ直)しても `chat-log` に
   履歴が残っていること
6. (片方が旧バイナリの場合)送信後の履歴に「(送信失敗)」と付き、
   デーモンログに警告が出ること

### 検証手順(M3-13b: チャット UI)

前提: 全マシンで `git pull` + `npm ci` + UI 再起動(デーモンは M3-13a の
ものがあればそのままでよい)。

1. **1:1 送受**: 「チャット」タブ → 左のリストで相手を選ぶ → 入力欄から
   送信(Enter) → 自分の吹き出しが右に出ること。相手の画面に数秒内に
   左の吹き出し(アバター付き)で届くこと
2. **通知と未読**: 受信側がチャットタブを開いていない(他タブや一覧画面)
   とき、OS 通知(送信者名 + 本文)が出て、チャットタブと会話リストに
   未読バッジが付くこと。その会話を開くとバッジが消えること。
   **その会話を表示中**のときは通知が出ないこと
3. **全体宛**: 「全体」を選んで送信 → オンラインの全メンバーに届き、
   全体の会話に送信者名付きの吹き出しで出ること
4. **オフライン宛**: オフラインのメンバーを選ぶと入力欄が無効になり
   「オフラインのメンバーには送れません」と出ること
5. **履歴**: UI を再起動しても(デーモンを再起動しても)会話を開くと
   履歴が残っていること。日付が変わった通の間に日付の区切りが出ること
6. **IME**: 日本語入力の変換確定の Enter では送信されないこと
   (確定後にもう一度 Enter で送信)

### 検証手順(M3-13c: グループチャット)

前提: 3 台で **デーモンも UI も**このバージョンへ更新して再接続する
(グループのフレームは M3-13a のデーモンに無いため、今回はデーモンの
入れ替えも必要)。以下 A・B・C とする。

1. **作成と伝搬**: A がチャットタブ「＋ グループ作成」で B だけを入れた
   グループを作る → 数秒内に **B の会話リストにもグループが現れる**こと。
   C には現れないこと。A の会話に「グループ「…」を作成しました」、B の
   会話に「A があなたをグループ「…」に追加しました」のお知らせが出ること
2. **グループ送受**: A がグループへ送信 → B に届き(送信者名付き)、
   OS 通知のタイトルがグループ名になること。**C には届かない**こと
3. **メンバー追加**: A がグループの「管理」から C を追加 → C の会話リストに
   グループが現れ、以後のメッセージが C にも届くこと。A・B の会話に
   「A が C を追加しました」のお知らせが出ること
4. **改名**: B が「管理」から改名 → A・C の会話リストの名前も数秒内に
   変わり、**「グループ名が「旧名」から「新名」に変わりました」**の
   お知らせが全員の会話に出ること
5. **退出**: C が「管理」から退出 → C の会話リストから消え(履歴は残る)、
   A・B の会話に「C がグループから退出しました」のお知らせが出ること。
   以後 C には届かないこと
6. **オフラインの追いつき**: C の UI・トンネルを止める → A がグループに
   C を追加(または新しいグループに C を入れて作成)→ C が再接続すると、
   **30 秒程度以内にグループが C の会話リストに現れる**こと(ack が
   取れるまで自動再送)。届くまでの間は「グループ(同期中)」と表示される
7. **永続化**: デーモンを再起動してもグループと履歴が残っていること
8. **ダイアログの文字選択**: 「管理」のグループ名をマウスで範囲選択し、
   **ダイアログの外でボタンを離しても閉じない**こと(閉じるのは外側を
   クリックしたときだけ。設定など他のダイアログも同様)

### 検証手順(M3-13d: チャット内ファイル送信 + D&D)

前提: M3-13c と同じ(デーモン + UI をこのバージョンへ)。

1. **📎 で送信**: 1:1 の会話で 📎 → ファイルを選ぶ → 会話に自分の
   ファイルバブル(📎 名前・サイズ、転送中は進捗バー)が数秒内に出ること
2. **受信バブルと保存**: 相手の会話にもファイルバブルが出て、OS 通知
   (📎 ファイル名)が鳴ること。バブルの「保存」で任意の場所へ保存でき、
   受信タブの受信ボックスからそのファイルが消えること
3. **画像プレビュー**: 画像(PNG/JPG など)を送ると、送信側・受信側とも
   会話にサムネイルが表示されること(**Linux の受信側も**。受信ボックスは
   `~/.config` 配下 = 隠しディレクトリのため、以前は表示できなかった)。**クリックで拡大表示**され、Esc か
   外側クリックで閉じること。保存済みの画像のバブルはファイル表示に戻ること
4. **動画・音声**: mp4 などを送ると受信完了後に会話内で再生できること
   (再生できない形式なら通常のファイル表示になること)
5. **ドラッグ&ドロップ**: エクスプローラーからファイルを会話画面へ
   ドラッグ → 点線の受け入れ表示が出る → ドロップ → 確認ダイアログ →
   送信されること。複数ファイルを一度にドロップしても全部送られること
6. **グループ宛ファイル**: グループの会話にドロップ → グループの
   オンラインメンバー全員に届くこと(グループ外のメンバーには届かない)
7. **オフライン宛**: オフラインの相手の会話では 📎 が無効。ドロップすると
   「オフラインのメンバーには送れません」の案内が出て送信されないこと
8. **大きめファイル**: 数百 MB のファイルで、送受両方のバブルに進捗バーが
   出て、完了後に受信側で「保存」できること(受信タブの進捗と同じ値)

### チャットの使い勝手改善(M3-13e)

2026-07-11 の依頼者要望に対応した改良です。

- **テキストファイルのプレビュー**: txt / md / log / csv / json などの
  テキストファイルは、会話内に先頭数行が表示されます。クリックで全文
  (先頭 256 KiB まで)を表示します。バイナリだった場合や、保存・削除で
  ファイルを動かした後は通常のファイル表示に戻ります
- **受信失敗のお知らせ**: チャット内ファイルの受信が途中で失敗すると、
  受け取る側の会話に「◯◯からのファイル「…」を受信できませんでした」の
  お知らせ(中央のグレー 1 行)が出ます。ファイルバブルにも失敗の印が
  付きます(送る側は従来どおり「転送失敗」)
- **メンバー一覧からのファイル送信**: メンバー行の 📤 アイコンは廃止し、
  メンバータブの「📤 ファイル送信」ボタン → 相手をチェックボックスで
  選んで送る方式になりました(複数人へ一度に送れます)
- **通知のオン/オフ**: ヘッダーの ⚙(アプリ設定)で OS 通知をまとめて
  オフにできます。未読バッジや受信タブの表示は変わりません。設定は
  マシンごと(localStorage)です
- **URL のリンク化**: 本文の http(s) の URL はクリックできるリンクに
  なり、既定ブラウザで開きます
- **リンクプレビュー**(ADR-0017): URL を含むメッセージの下に、ページの
  タイトル・説明・画像のカードが出ます(LINE 風。カードのクリックでも
  ブラウザが開きます)。サーバーが無いため**表示している端末が自分で
  ページ情報を取りに行きます**(相手サイトにあなたの IP が伝わります)。
  気になる場合は ⚙ の「チャットの URL のプレビューを表示する」をオフに
  してください。プライベート IP や `.internal` 宛の URL は取得しません

### 検証手順(M3-13e: 使い勝手改善)

前提: 全マシンで `git pull` + `npm ci` + UI 再起動。**デーモンも最新へ
入れ替える**(受信失敗のお知らせがデーモン側の機能のため)。

1. **テキストプレビュー**: .txt や .md をチャットで送る → 送信側・受信側
   とも会話に先頭数行が表示されること。クリックで全文が開き、Esc か
   外側クリックで閉じること
2. **受信失敗のお知らせ**: 大きめのファイルをチャットで送り、転送中に
   送信側のトンネルを切断(または UI とデーモンを停止)→ 受信側の会話に
   「…を受信できませんでした」のお知らせが出ること
3. **ファイル送信ボタン**: メンバータブに「📤 ファイル送信」が出て、
   メンバー行に 📤 アイコンが**無い**こと。ボタン → ファイルを選ぶ →
   2 人以上をチェック → 送信で、受信タブの転送一覧に宛先ごとの進捗が
   出て全員に届くこと。オフラインのメンバーはチェックできないこと
4. **通知オフ**: ヘッダーの ⚙ → 「OS 通知を出す」をオフ → 他のマシンから
   チャットやファイルを送っても OS 通知が出ないこと(未読バッジは付く)。
   オンに戻すと再び通知されること
5. **URL リンク**: `https://example.com のページです` のような本文を送る →
   URL 部分がリンクになり、クリックで既定ブラウザが開くこと
   (送信側・受信側とも)
6. **リンクプレビュー**: OGP のあるページ(ニュース記事・YouTube など)の
   URL を送る → リンクの下にタイトル・説明・画像のカードが出ること。
   カードのクリックでもブラウザが開くこと。⚙ でプレビューをオフにすると
   カードが出なくなること(リンク化はそのまま)

## メンバー間の通信制御 ACL(M3-10)

ホストが「この 2 人の間の通信を遮断する」を制御できます(ADR-0018)。
遮断は ping などの IP 通信だけでなく、**チャット・ファイル送信・直接通信
(M3-3)にも効きます**。ホストとの通信は遮断できません(コントロール
チャネルが壊れるため)。

- **設定**: ホスト UI のサイドバー →「通信制御」。メンバーの
  組み合わせごとに「遮断する」をチェック。変更は約 5 秒で反映されます
- **正本**: `host.toml` の `[acl]` セクション(`deny = [["10.x.y.2", "10.x.y.3"]]`
  のような仮想 IP の組)。UI を使わず手編集でも同じです
- **メンバー側の見え方**: 遮断相手に「🚫 通信不可」バッジが付き、
  チャット・ファイル送信の宛先に選べなくなります。確立済みの直接通信も
  自動で解除されます
- **仕組み**: リレー(ホスト経由)はホストが転送段階で破棄
  (Windows = デバイス内リレーで判定 / Linux = iptables の DROP ルール)。
  直接通信は、ホストが台帳の配布時に遮断相手のエンドポイントを渡さない
  ことで成立しなくなります
- 注意: グループチャットや全体チャットでは、遮断された相手**にだけ**
  メッセージが届きません(他のメンバーには届きます)

### 検証手順(M3-10: 通信制御 ACL)

前提: 全マシンでデーモン + UI を最新へ入れ替え(`git pull` → デーモンの
ビルドと入れ替え → `npm ci` → UI 再起動)。メンバー A・B の 2 人以上が参加
していること。

1. **遮断前の疎通**: A から B へ ping(またはチャット)が通ることを確認
2. **遮断**: ホスト UI のサイドバー →「通信制御」→ A ⇔ B の組を
   「遮断する」にチェック → 約 5 秒待つ
3. **IP 遮断の確認**: A から B への ping が通らないこと(ホストへの ping は
   通ること)。直接通信中だった場合も、経路バッジが「中継」へ戻った後に
   通らなくなること
4. **UI の確認**: A・B それぞれのメンバー一覧で相手に「🚫 通信不可」が
   付くこと。チャットで相手を選ぶと入力欄が無効になり「ホストの通信制御に
   より、このメンバーには送れません」と出ること。「📤 ファイル送信」でも
   相手をチェックできないこと
5. **第三者への影響なし**: メンバー C がいる場合、A ⇔ C・B ⇔ C の通信は
   これまで通りできること
6. **解除**: ホストでチェックを外す → 約 5 秒で ping が再び通り、バッジが
   消えること。直接通信が有効なら、しばらくして「直接」へ戻ること
7. **(Linux ホストのみ)残骸なし**: 遮断中にホストで
   `sudo iptables -L FORWARD -n | grep peercove-acl` にルールが見え、
   解除・切断後には消えていること

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

## 更新通知とバージョン表示(M3-12)

PeerCove は GitHub Releases の最新版を確認し、新版がある場合だけサイドバーと設定画面に通知します。
自動確認は成功結果を 24 時間キャッシュし、5 秒で打ち切ります。確認に失敗しても VPN 接続には影響しません。

- 設定画面の「アップデート」で自動確認を無効化できます。確認時は GitHub に送信元 IP が見えます。
- 「今すぐ確認」はキャッシュを使わず、明示的に最新版を確認します。
- 更新がある場合は公式 GitHub Releases ページを開けます。バイナリの自動適用はコード署名後に対応します。
- 設定画面には UI とデーモンのバージョン、メンバー一覧には接続相手の申告バージョンを表示します。
  旧版など情報を送らない相手は `v?` と表示され、接続自体は継続します。

### 検証手順(M3-12: 更新通知・バージョン互換性)

1. デーモンと UI を起動し、アプリ設定の「アップデート」に UI とデーモンのバージョンが表示されること。
2. ホストとメンバーを接続し、メンバー一覧の名前にマウスを合わせると申告バージョンを確認できること。
3. 「今すぐ確認」を押し、通信可能なら「最新版です」または更新案内が表示されること。
4. 通信を遮断して手動確認し、エラーがアップデート欄だけに表示され、トンネルの接続・切断や状態更新を
   引き続き操作できること。
5. 自動確認をオフにして UI を再起動し、設定が保持されること。その後の手動確認は利用できること。
6. 旧版デーモンまたは `app_version` を持たない fixture を使用し、画面が壊れず「不明」/`v?` になること。

## ACL v2: 通信ルール(M3-25)

ホストのネットワーク詳細サイドバーにある「通信制御」ページでは、上から順に評価するルールを管理できます。
送信元と宛先にはメンバー、メンバーグループ、サブネットを指定でき、宛先にはポート付きの
DNSサービスも選べます。動作は許可／拒否、プロトコルはANY/TCP/UDP/ICMP、TCP/UDPは
単一ポートまたは`8000-8100`のような範囲に対応します。どのルールにも一致しない場合は
既定で許可されます（画面で拒否へ変更可能）。旧版のメンバー間遮断設定も同じ結果で読み込まれます。

細粒度ACLに関係するメンバー組は、直接通信を解除してホスト中継へ固定します。メンバー行には
「ACLにより中継」とルールIDが表示されます。Windowsホストはユーザー空間リレー、Ubuntuホストは
iptablesで同じ順序・プロトコル・ポート条件を強制します。
方向は新規通信の開始方向を表します。例えば`A→B 拒否`でも、BからAへ開始したTCP/UDP/ICMP通信と
その応答は通ります。Aから新規開始した通信は拒否されます。

### 検証手順(M3-25: ACL v2)

1. ホスト、メンバーA、メンバーBを接続し、旧「2人を遮断」設定を開いて保存しても、双方向の
   ping、チャット、ファイル送信が従来どおり遮断されること。
2. A→BだけUDPのテスト用ポートを許可し、その後にA→BのANY拒否を置く。Aからは指定UDPポートだけ
   成功し、TCP、範囲外ポート、ICMPが拒否されること。BからAへ開始したTCP/UDP/pingは応答も含めて
   成功し、同じポートをAから新規開始すると拒否されること。
3. ルールを上下ボタンで入れ替えると最初に一致した規則の結果へ変わり、無効化した規則は無視され、
   defaultを拒否へ変えると不一致通信が拒否されること。
4. メンバーグループと広告サブネットを対象にした規則が全所属メンバー／LAN宛に適用されること。
   メンバー削除後に公開鍵参照、空グループ、関連ルールが設定へ残らないこと。
5. ポート付きDNSサービスを宛先に選べ、レコード名を変更しても安定IDで追随すること。外部CNAMEは
   選択肢に出ず、手編集しても設定検証で拒否されること。
6. ルールに関係するA/Bの直接接続が解除され、両端に「ACLにより中継」とルールIDが表示されること。
   診断の`ACL`項目にもforce-relay数と同じルールIDが表示されること。
7. WindowsホストとUbuntuホストで同じルール一式を試し、通信結果が一致すること。設定を連続更新しても
   一瞬すべて許可される状態がなく、ホスト切断後はiptables規則が残らないこと。
8. 通信制御から別ページへ移動して再度開いても白画面にならず、保存済みの既定動作、グループ、ルールが
   再読込されること。メンバー側のサイドバーには「通信制御」が表示されないこと。

## 暗号化バックアップと端末移行(M3-24)

アプリ設定の「バックアップと復元」から、ネットワーク設定、秘密鍵・PSK、招待メタデータ、
DNS、ACL、グループを1つの `.pcvbackup` ファイルへ保存できます。12文字以上のパスフレーズから
Argon2idで鍵を作り、XChaCha20-Poly1305で暗号化します。パスフレーズは保存されません。
受信ファイル、チャット、品質履歴、ログ、診断結果はバックアップに含まれません。

復元では、復号後にネットワーク名、役割、作成元OS、含まれる項目を確認できます。既存の
ネットワークを黙って上書きせず、別名での復元か明示的な置換を選びます。置換対象が稼働中なら
拒否されます。別端末へ復元したメンバー設定は、接続後にネットワーク設定からデバイス鍵を更新する
ことを推奨します。

### 検証手順(M3-24: 暗号化バックアップ)

1. Windowsのホストとメンバーを切断し、それぞれを異なるパスフレーズでバックアップする。
2. 誤ったパスフレーズでは内容確認できず、ファイル末尾を削った場合や1 byte変更した場合も拒否されること。
3. Ubuntuへバックアップを移し、内容確認後に別名で復元して、設定・鍵・PSK・DNS・ACL・グループが読めること。
4. Ubuntuで作成したバックアップもWindowsへ復元でき、設定中の秘密ファイル参照が復元先の相対パスに
   なっていること。Linuxでは秘密ファイルが600、Windowsでは現在ユーザーとSYSTEM限定ACLであること。
5. 同名復元は明示的な置換を選ぶまで拒否され、稼働中ネットワークの置換も拒否されること。
6. メンバーを別PCへ復元した場合、鍵更新の推奨表示を確認し、接続後にデバイス鍵更新が完了すること。
7. Ubuntuで同名置換した後、ネットワーク一覧に同名項目が増えず、`.replace-old-*` などの内部退避名が
   表示されないこと。復元したネットワークを通常どおり削除できること。

## 接続診断センター(M3-21)

ネットワーク詳細の「診断」は、設定や OS を変更せず、デーモンが保持する状態と設定ファイルから障害原因を
確認します。外部診断サーバーは使用せず、到達を示す実ハンドシェイクがない項目は推測で正常にせず
「判定不能」と表示します。

- 問題・警告・判定不能を先に表示し、正常項目は折りたたみます。
- 設定、秘密ファイルの存在、Unix のファイル mode、インターフェース、ハンドシェイク、DNS、版互換、
  ホストからの削除、ACL を確認します。Windows は、存在する秘密ファイルをPeerCoveが作成時に
  「現在のユーザー＋SYSTEM」へ制限する管理対象として正常表示します。
- 秘密鍵、PSK、招待トークンは診断結果に含めません。診断結果は画面表示のみで、
  ファイル保存や外部送信は行いません。

### 検証手順(M3-21: 接続診断)

1. 稼働中ネットワークの詳細から「診断」を開き、実行時インターフェース、仮想 IP、台帳、ハンドシェイクの
   根拠が表示され、画面を開いても通信が継続すること。
2. トンネルを停止して再診断し、「トンネルの稼働状態」が失敗になる一方、設定検証など残りの項目も
   表示されること。
3. テスト用コピーの設定を不正な TOML にして診断し、設定エラーが出てもレポート全体を取得できること。
4. メンバーを ACL で遮断、またはホストから削除して再診断し、それぞれの固定診断項目が警告／失敗に
   なること。
5. Windows では存在する秘密ファイルが正常項目に入り、根拠が「PeerCove管理（現在のユーザー＋SYSTEM）」に
   なること。Ubuntu では group/other に読取権限を付けた秘密ファイルが警告になること。

## 招待の期限・一回利用・参加承認(M3-22)

新しい招待は token v3 として発行され、ホスト側にも秘密を含まない招待 ID・発行時刻・期限・初回参加状態を
保存します。既定期限は 7 日です。旧 token v1/v2 は期限なしの既存招待として引き続き利用できます。

- UI は 1 時間、1 日、7 日、30 日、無期限から選べます。CLI は `invite --expires-hours 168` が既定で、
  `--expires-hours 0` は無期限です。
- 未使用のまま期限が切れた peer はトンネルへ追加されず、初回 Hello でもホストが期限を再確認します。
- v3 の join は端末上で device ID を生成します。同じトークンを別端末へコピーした場合、最初に参加した
  device ID だけが再接続でき、後続端末は理由つきで拒否されます。拒否後は無駄な再接続を停止し、
  UI から新しい招待トークンが必要だと確認できます。
- 初回参加後は既存の鍵ローテーションにより、トークンに含まれた旧秘密鍵もホスト設定から外れます。
- 取消は従来どおりメンバー削除を使い、関連 PSK と ACL も対で削除します。トークン本体は保存・再表示しません。

- ホスト設定で「新しい端末の参加を承認する」を有効にすると、その後に発行した招待は初回接続後も
  「承認待ち」になります。Windows はユーザー空間デバイス、Ubuntu は iptables の INPUT / OUTPUT /
  FORWARD で、ホストのコントロール TCP と応答以外を破棄します。
- 承認待ち端末は台帳、DNS、直接接続 endpoint、チャット、ファイル送信、グループ同期の対象になりません。
  ホスト UI の「承認」で解除し、「×」(削除)で拒否します。UI を閉じてトレイ常駐中も参加要求を通知します。

### 検証手順(M3-22: 招待期限・一回利用・参加承認)

1. 期限 1 時間で招待を発行し、host.toml に `invite_id` / `invite_issued_at` /
   `invite_expires_at` がある一方、トークン・秘密鍵・PSK 本文がないこと。
2. 期限内に端末 A で join・接続し、ホスト側へ `invite_accepted_at` と `invite_device_id` が保存され、
   UI の状態が「参加済み」になること。切断・再接続も成功すること。
3. 同じ token v3 を別ディレクトリまたは端末 B で join し、端末 A の参加後は B が台帳を取得できず、
   ホストログに「別の端末で既に使用」と出ること。B の UI に「接続が拒否されました」と理由が出て、
   5 秒ごとの再接続ログが止まること。A の接続は切れないこと。
4. テスト用に未使用 peer の `invite_expires_at` を過去へ変更してホストを再読込し、その peer が期限切れ表示に
   なり、token から作った端末がハンドシェイク／台帳取得できないこと。
5. 旧版で作成した v1/v2 token が従来どおり join・接続できること。
6. 期限切れまたは未使用のメンバーを UI から削除し、peer と関連 PSK が消え、トークン文字列がどこにも
   再表示されないこと。
7. ホスト設定で「新しい端末の参加を承認する」を有効にして新しい招待を発行し、端末 C を接続する。
   ホスト UI と OS 通知に「承認待ち」が出る一方、C は台帳・DNS・チャットを利用できず、ホストの任意 TCP/UDP
   サービスや他メンバーへ ping/TCP/UDP で到達できないこと。Windows Host と Ubuntu Host の両方で確認する。
8. 「承認」を押すと数秒以内に C が台帳を取得し、通常通信できること。ホストと UI を再起動しても承認状態が
   保持されること。別の新規端末は「×」で拒否し、再接続・台帳取得できないこと。
9. Ubuntu で承認待ちを作ってからホストを停止し、`iptables-save` に `peercove-invite-isolation` の規則が
   残っていないこと。

## 通信品質履歴(M3-23)

ネットワーク詳細の「品質」では、UI を閉じている間もデーモンが収集した通信状態を確認できます。
外部の計測サーバーは使わず、履歴は各端末の
`networks/<ネットワーク>.quality/YYYY-MM-DD.jsonl` にだけ保存されます。

- 1分ごとの RTT（最新・最小・平均・P95）、ジッター、Ping 損失率
- 制御接続断を 100% 損失と区別した欠測表示
- 直接／ホスト経由／確立中の経路と切替回数、rx / tx 差分
- 15分、1時間、24時間、7日の表示。7日または合計32 MiBを超えた古い日から自動削除
- アプリ設定で任意の品質通知（既定オフ）。損失率が閾値を3分連続で超えた場合と、
  直接からホスト経由へ切り替わった場合に通知

### 検証手順(M3-23: 通信品質履歴)

1. ホストとメンバーを接続し、「品質」を開く。最初の1点だけでもRTTの点と0%損失の最小棒が見え、
   1分以上経過後はRTTの線、損失の棒、経路帯、下部の数値表が同じ時刻・値を示すこと。
2. UI を閉じたまま数分通信し、再度開いて、その時間の履歴も増えていること。デーモンを再起動しても
   再起動前の履歴が表示されること。
3. 制御接続を切り、該当時間が「制御接続なし」、RTT の線は途切れ、損失率は `—` になること。
   0 ms、0%、100% の測定値として表示されないこと。
4. メンバー3台構成で直接通信を成立させた後、直接通信を無効にするか到達不能にし、経路帯が
   「直接」から「ホスト経由」へ変わり、切替回数が増えること。
5. アプリ設定で品質通知を有効にして損失閾値を設定し、3窓連続で超えたときだけ通知されること。
   既定状態では通知されないこと。
6. Windows と Ubuntu の各端末で `*.quality` 配下に日次 JSONL が作られること。末尾へ壊れた1行を
   追加してデーモンを再起動しても、正常行が表示され、読み飛ばした件数だけ案内されること。
7. Ubuntuでrootのサービスデーモンが品質履歴を作成した後に切断し、一般ユーザーのUIからその
   ネットワークを削除できること。

## DNSサービスヘルスチェック(M3-14e)

スキームとポートを登録したカスタムDNSサービスは、ホストが60秒ごとにTCP接続を確認します。
状態は全メンバーのDNS画面に「稼働中／応答なし／未確認／確認オフ」と文字で表示されます。
確認失敗でもDNS回答やURLは取り下げません。

- タイムアウト3秒、最大8並列。監視失敗でトンネルやDNSサーバーは停止しません
- メンバー参照がオフラインなら接続せず「未確認」とします
- ホストは「ヘルスチェック設定」で平文HTTP HEAD、path、期待ステータスを選べます
- ポート未設定のレコードは確認対象にできないため、「ヘルスチェック設定」を表示しません
- 外部CNAMEへの確認は既定オフで、レコードごとの明示許可が必要です
- Cookie、認証ヘッダー、GET、本文一致、HTTPS内容検査は行いません

### 検証手順(M3-14e)

1. ホスト配下で待受中のTCPサービスを、スキームとポート付きDNSレコードとして登録する。
   DNS画面で「今すぐ確認」を押し、数秒後に「稼働中」と応答時間・確認時刻が表示されること。
2. サービスを停止して再確認し、「応答なし」になる一方、同じDNS名の名前解決とURLコピーは
   引き続きできること。サービスを戻すと次回確認で回復すること。
3. メンバー参照サービスのメンバーをオフラインにし、「未確認」と理由が表示され、
   100%損失や停止とは表示されないこと。
4. http サービスの設定でHTTP HEADと /health、期待状態を指定し、期待値の一致／不一致が
   「稼働中／応答なし」へ反映されること。サーバーログにGETや本文取得が無いこと。
5. 外部CNAMEを登録し、既定では「確認オフ」で外部接続しないこと。明示許可後だけ確認されること。
6. 同じネットワークのメンバー画面にも状態が届き、旧版メンバーでは名前解決が従来どおり動くこと。
7. スキーム・ポートなしのDNSレコードには「ヘルスチェック設定」が表示されず、ポートを設定した
   レコードではスキームなしでもTCP確認を設定できること。

## UI デザインパス(M3-6)

見た目と操作性の一括改善です。デーモン・プロトコルの変更はありません
(UI だけ新しければ動きます)。

- **テーマ切替**: ヘッダーの ☀/☾ ボタンでライトとダークを切り替えます。
  初回起動時だけ OS の設定に合わせ、以降は選んだ方がこのマシンに保存されます
- **自分の表示名**: 稼働中のネットワーク詳細ヘッダーに、ネットワーク名に続いて
  `表示名: <表示名>` を表示します。長い名前は省略し、マウスオーバーで全文を確認できます
- **メンバーの色分け**: メンバーごとに固有色のアバターが付きます。色は
  公開鍵から決まるので、**全員の画面で同じ色**になり、名前を変えても
  変わりません。オンライン状態はアバター右下の緑ドットです
- **転送量スパークライン**: メンバー一覧に転送速度(受信+送信)の直近約 90 秒の推移が出ます。
  長期の RTT・損失・経路は M3-23 の「品質」ページへ移行しました
- **タブ構成**: ネットワーク詳細が「メンバー」「統計」のタブになりました。
  今後のチャット(M3-13)・ファイル送信(M3-9)はここへタブとして増えます
- **トレイの拡充**: トレイの右クリックメニューから、ウィンドウを開かずに
  ネットワークごとの「接続」「切断」ができます(ホストとして接続する場合、
  UPnP は UI の既定値と同じくオンです)。トレイアイコンのツールチップに
  稼働状況(「n ネットワーク稼働中」)が出ます

### 検証手順(M3-6: UI デザインパス)

前提: 新しい UI を起動(デーモンは従来のままでよい)。

1. ヘッダーのテーマボタン(☀/☾)をクリック → ライト ⇄ ダークで全画面
   (一覧・詳細・各ダイアログ)の配色が切り替わること。アプリを
   再起動しても選んだテーマが保たれること
2. トンネルを開始してネットワーク詳細を開く → メンバーごとに色の違う
   アバターが出て、オンラインの相手には緑ドットが付くこと。
   ヘッダーはネットワーク名の次に自分の表示名が出ること。
   **別のマシンでも同じメンバーは同じ色**であること
3. メンバー間で通信(ping -t や動画再生など)しながら詳細画面を見る →
   メンバー行の折れ線(転送速度)が動き、現在値(〜/s)が更新されること
4. 「品質」ページ → ピアごとに RTT・損失・経路の履歴と数値表が出ること
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
3. 別ターミナル(**管理者 / sudo**)で `peercove daemon run` を起動
   → UI が数秒で「待機中」表示に切り替わること(**UI 側は非特権のまま**)
4. さらに別ターミナル(通常ユーザー)で
   `peercove daemon start-host --config host.toml`
   → UI が「ホストとして稼働中」になり、仮想 IP と設定ファイルが表示されること
5. メンバーを接続する → UI の **メンバー一覧**に ●(オンライン)で現れること。
   ピア統計(エンドポイント・最終ハンドシェイク・rx/tx)が 2 秒ごとに更新されること
6. `peercove remove-peer --config host.toml --name <名前>`
   → 約 10 秒で UI の一覧から消えること
7. `peercove daemon stop` → UI が「待機中」に戻ること
8. `peercove daemon shutdown` → UI が「デーモンに接続できません」に戻ること
   (UI は落ちない)

> 注: 開始・参加・招待の**操作**は M2-G3/G4 で追加します。G2 は「見える」ところまでです。

## 検証手順(M2-G1: デーモン + IPC)

daemon 経由でも従来と同じ疎通ができることを確認します(2 台構成)。

1. Host: `sudo ./target/debug/peercove daemon run` を起動したままにする
2. Host(別ターミナル): `daemon status` → 「待機中」と出ること
3. Host: `daemon start-host --config host.toml` → 「開始しました」
4. Host: `daemon status` → 「ホストとして稼働中」+ 仮想 IP + members が出ること
5. Member A: 従来どおり接続(`sudo ./peercove member --config member.toml`
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
2. `peercove keygen --out test.key` で公開鍵(base64 44 文字)が表示され、`test.key` に**秘密鍵だけ**が保存されること
3. もう一度同じコマンドを実行するとエラーになること(`--force` で成功すること)
4. Linux: `ls -l test.key` が `-rw-------` であること / Windows: `icacls test.key` の出力が自分のアカウントのみであること
5. `examples/member.example.toml` をコピーし、`public_key` を手順 2 の公開鍵に置き換えて `peercove member --config <コピー先>` を実行すると「設定 OK: …」と表示されること
6. `public_key` を適当な短い文字列に変えると、分かりやすいエラーで失敗すること

## ライセンス

MIT OR Apache-2.0(予定)
