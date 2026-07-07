# PeerCove M0(技術検証 / PoC)ハンドオフ資料

- **版数**: 1.0
- **作成日**: 2026-07-07
- **目的**: Claude Code に M0(技術検証コード)の実装を依頼するための引継ぎ資料
- **上位文書**: 「PeerCove 要件定義書・機能設計書 v1.0」「PeerCove 方針まとめ」

---

## 1. プロジェクト概要(3行)

PeerCove は、事業者サーバーを持たない P2P 型 VPN デスクトップアプリ。ホスト PC がコーディネーター兼リレーを担い、参加者は QR/招待文字列で参加する。トンネルには WireGuard プロトコルを使用し、Phase 1 はホスト経由のハブ&スポーク、Phase 2 でピア間直接通信を追加する。

## 2. 確定済み方針(M0 に関係するもの)

| 項目 | 方針 |
|---|---|
| プロジェクト名 | PeerCove(製品名・crate名・リポジトリ名に "WireGuard" を含めない) |
| 対応OS (Phase 1) | Windows 10/11, Ubuntu 22.04+ 相当。**macOS は対象外** |
| 経路方式 | WireGuard AllowedIPs + OS ルーティングによるハブ&スポーク(ホスト経由) |
| アプリ層UDPプロキシ | 作らない。TCP/UDP/ICMP はすべてトンネル内の通常 IP 通信として扱う |
| ゲーム用途 | 仮想IP:ポート直接指定での接続を基本対応。LAN 自動検出(ブロードキャスト/mDNS)は保証しない |
| 仮想IP | 例: Host `100.100.42.1`, Member A `.2`, Member B `.3`(/24) |
| ルーティング | メンバーはホストピアに `AllowedIPs = 100.100.42.0/24` を設定(全メンバー宛をホスト経由) |
| 技術スタック | Rust (stable) + tokio。UI は将来 Tauri + React(M0 では UI なし、CLI のみ) |

## 3. M0 のスコープ

「方針まとめ」13章の優先順位のうち **手順 1〜7** を M0 とする。招待トークン・QR・メンバー削除・UI(手順 8〜12)は M0 に含めない(設定交換は手動コピペで良い)。

### 3.1 ゴール

異なる NAT 配下を含む 3 台(Host 1 + Member 2)で、以下がすべて成功すること。

1. **G-1 トンネル作成**: Windows / Linux 双方で、CLI から WireGuard トンネル(TUN)を作成・破棄できる
2. **G-2 1対1疎通**: Host–Member A 間で仮想 IP による ping 疎通
3. **G-3 ハブ&スポーク疎通**: Member A ↔ Member B が **Host 経由**で ping 疎通(直接ピア設定なし)
4. **G-4 TCP 疎通**: Member A 上の HTTP サーバー(例: `python -m http.server` や同梱の簡易サーバー)へ Member B から `curl http://100.100.42.2:8080` で到達
5. **G-5 UDP 疎通**: Member A 上の UDP echo サーバーへ Member B から到達(同梱の `udp-echo` サブコマンドで検証)
6. **G-6 到達性セットアップ**: Host 側で UPnP IGD によるポート自動開放を試行し、成否と外部エンドポイント(推定)をレポート。失敗時は手動ポート開放を案内するメッセージを表示
7. **G-7 計測レポート**: 経路(リレー)での ping RTT と iperf3(または同等)スループットの計測手順を README に記載し、実測値を記録できるフォーマットを用意

### 3.2 スコープ外(M0 ではやらない)

- 招待トークン、QR、コントロールチャネル、台帳、メンバー削除
- UDP ホールパンチング、直接通信
- Tauri / React UI、インストーラ、コード署名
- macOS 対応、モバイル対応
- IPv6(構造上妨げない設計にはするが、実装・検証しない)

## 4. 成果物

```
peercove/
├─ CLAUDE.md                    # Claude Code 用リポジトリ指示(別紙を配置)
├─ README.md                    # セットアップ・検証手順(Windows/Linux 別、管理者権限の注意含む)
├─ docs/
│   ├─ m0-report-template.md    # G-7 の計測記録フォーマット
│   └─ decisions.md             # M0 中に行った技術判断の記録(ADR 形式で追記)
├─ Cargo.toml                   # ワークスペース
└─ crates/
    ├─ peercove-core/           # OS 非依存: 鍵生成、設定型(TOML)、IP割当ヘルパ
    └─ peercove-poc/            # CLI バイナリ(OS 依存コードは cfg で分離 or 内部モジュール分割)
```

将来 `peercove-daemon-win` / `peercove-daemon-linux` へ分離する前提で、`peercove-poc` 内でも OS 依存層(TUN/WG/ルーティング/フォワーディング)を trait で抽象化しておくこと。

### 4.1 CLI 仕様(最小)

```
peercove-poc keygen
    # X25519 鍵ペアを生成し、公開鍵を表示。秘密鍵はファイル(パーミッション制限)へ

peercove-poc host --config host.toml
    # TUN 作成、100.100.42.1/24 割当、UDP 51820 待受
    # IP フォワーディング有効化(自動 or 手順案内。docs/decisions.md に判断を記録)
    # --upnp 指定時: UPnP IGD でポート開放を試行し、結果と外部エンドポイントを表示

peercove-poc member --config member.toml
    # TUN 作成、割当 IP 設定、ホストをピア登録(AllowedIPs = 100.100.42.0/24)
    # persistent keepalive 25s

peercove-poc add-peer --config host.toml --pubkey <b64> --ip 100.100.42.2
    # ホスト側にメンバーピアを追加(AllowedIPs = <ip>/32)。実行中プロセスへ反映
    # M0 では設定ファイル追記+再読込(SIGHUP or 再起動)でも可。方式は decisions.md に記録

peercove-poc udp-echo --listen 0.0.0.0:9999      # UDP echo サーバー
peercove-poc udp-ping --target 100.100.42.2:9999 # UDP 疎通クライアント(RTT表示)

peercove-poc status --config <toml>
    # ピア一覧、最終ハンドシェイク、転送量を表示

peercove-poc down --config <toml>
    # トンネル・ルート・(可能なら)フォワーディング設定のクリーンアップ
```

設定ファイル(TOML)例:

```toml
# member.toml
[interface]
private_key_file = "member_a.key"
address = "100.100.42.2/24"
mtu = 1420

[[peer]]
public_key = "HOST_PUBKEY_B64"
endpoint = "203.0.113.5:51820"
allowed_ips = ["100.100.42.0/24"]
persistent_keepalive = 25
preshared_key_file = "psk.key"   # 任意
```

## 5. 技術方針と選定タスク

### 5.1 WG バックエンド(M0 内で確定させる)

| OS | 第一候補 | 備考 |
|---|---|---|
| Linux | カーネル WireGuard を netlink で制御 | `wireguard-control` 等の crate を調査し、メンテ状況を確認して採用可否を判断。不可ならユーザー空間実装へ |
| Windows | wintun(TUNドライバ)+ ユーザー空間 WG 実装(boringtun 系) | wintun.dll は開発機に手動配置で可(README に入手手順を記載)。boringtun のメンテ状況・API を着手時に必ず確認し、代替(wireguard-go サイドカー等)との比較を decisions.md に記録 |

> **重要**: crate 選定は実装前に必ず最新状況を Web で確認すること(boringtun のメンテ状況は流動的)。「動くこと」を最優先し、選定理由・却下理由を `docs/decisions.md` に ADR として残す。性能比較(wireguard-nt との差)は数値レポートのみで良く、wireguard-nt 実装自体は M0 必須ではない。

### 5.2 既知の技術的注意点(必ず対処・検証)

1. **ホストの IP フォワーディング**: Linux は `sysctl net.ipv4.ip_forward=1`(またはトンネル IF 単位)。Windows は `Set-NetIPInterface -InterfaceAlias <tun> -Forwarding Enabled`(+物理側も必要か検証)。自動設定する場合は `down` で原状回復すること。
2. **Windows ファイアウォール**: メンバー側でトンネル IF 宛の受信が既定ブロックされ得る。G-4/G-5 の検証手順に「失敗時の確認ポイント」として明記。アプリによる自動ルール追加は M0 では行わず、手順書対応。
3. **MTU**: 既定 1420。PPPoE 環境向けに設定可能にする。
4. **hairpin/同一LAN**: Host と Member が同一 LAN の場合、外部エンドポイントでは繋がらないことがある。設定で LAN 内エンドポイントを指定できれば M0 は十分。
5. **クリーンアップ**: 異常終了後の残骸(TUN、ルート、フォワーディング設定)を `down` および起動時チェックで除去。
6. **権限**: TUN 作成には管理者/root が必要。README の全手順に明記(Windows は「管理者として実行」、Linux は `sudo`)。
7. **UPnP**: `igd` 系 crate を使用。ルーターが非対応/無効の場合のエラーメッセージは、ユーザーが次に取るべき行動(手動ポートフォワード、CGNAT の可能性)を含めること。

### 5.3 コーディング規約(M0)

- Rust stable、edition 2021+。`cargo fmt` / `cargo clippy -D warnings` を CI 相当として通す
- 非同期は tokio。エラーは `anyhow`(バイナリ)+ `thiserror`(core)
- ログは `tracing`。**秘密鍵・PSK をログに出力しない**
- unsafe は OS API 境界のみ許可し、コメントで理由を残す
- テスト: `peercove-core` はユニットテスト必須。ネットワーク実疎通は手動手順(README)で可

## 6. 受け入れ基準(M0 完了の定義)

1. G-1〜G-7 がすべて達成され、README の手順どおりに第三者(= 依頼者)が再現できる
2. Windows(管理者)/ Linux(sudo)双方でビルド・起動・クリーンアップが動作する
3. `docs/decisions.md` に「WG バックエンド選定」「add-peer 反映方式」「フォワーディング設定方式」の判断が記録されている
4. `docs/m0-report-template.md` に実測値(RTT / スループット)を記入した状態で納品されている(依頼者環境での再計測は別途)
5. 秘密鍵が平文でログ・標準出力に出ないこと(M0 ではファイル保存は可、パーミッション 600 相当)

## 7. 検証環境(依頼者側で用意)

- Host: Windows 11(自宅ルーター配下、UPnP 有効/無効を切替可能)
- Member A: Ubuntu 22.04(別 NAT 推奨。難しければ同一 LAN + LAN エンドポイントで代替)
- Member B: Windows 10/11(モバイル回線テザリング等、可能なら別 NAT)

※ 開発初期は 1 台内の VM 2〜3 台(NAT 分離)での検証でも可。最終確認のみ実機 3 台で行う。

## 8. M0 の次(参考: M1 予告)

M1 では、招待トークン(mlk1 形式→ pcv1 形式に改名予定)、WG トンネル内コントロールチャネル、台帳配布、メンバー削除を CLI で E2E 実装する。M0 の trait 分離・core crate はこれを見越して設計すること。
