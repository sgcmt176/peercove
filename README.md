# PeerCove

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
- [x] M2-G2: Tauri + React UI の骨組みと状態表示 ※実機検証待ち
- [ ] M2-G3〜G6: 接続/参加・招待/削除・設定/ログ・トレイ
- [ ] M2-G7: インストーラ・自動起動

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

## ビルド

```bash
cargo build --workspace
```

テスト・lint:

```bash
cargo test -p peercove-core
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

## 使い方

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

# 2) 別ターミナル(ユーザー権限でよい)から操作
./target/debug/peercove-poc daemon start-host --config host.toml
./target/debug/peercove-poc daemon status
./target/debug/peercove-poc daemon stop
./target/debug/peercove-poc daemon shutdown
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
6. `peercove-poc remove-peer --name <名前>` → 約 10 秒で UI の一覧から消えること
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
