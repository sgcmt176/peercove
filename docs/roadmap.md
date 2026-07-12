# PeerCove ロードマップと開発引き継ぎガイド

- **最終更新**: 2026-07-12(M3-14a 表示名/DNS 名の分離を実装(ADR-0021)。実機検証待ち。M3-12 は Opus 担当予定)
- **対象読者**: このリポジトリで作業するすべての AI アシスタント・開発者
- **必読**: [CLAUDE.md](../CLAUDE.md)(規約)→ 本書(全体像)→ [decisions.md](decisions.md)(技術判断)

---

## 1. プロダクト全体像(3 行)

PeerCove は**事業者サーバーを持たない** P2P 型 VPN デスクトップアプリ。
ホスト PC がコーディネーター兼リレーを担い、参加者は招待(将来は QR/招待文字列)で参加する。
トンネルは WireGuard プロトコル(製品名・crate 名には "WireGuard" を含めない)。

**Tailscale との違い(設計思想)**: Tailscale の「ログインだけで繋がる」体験は
事業者のコーディネーション/STUN/DERP サーバー群に依存する。PeerCove は
サーバーレスを選んでいるため、**ホスト 1 台だけは外部到達性(UPnP or 手動開放)が
必要**。メンバーは一切ポート開放不要。この設計判断を変える場合は事業判断になる。

## 2. フェーズ構成と現在地

| フェーズ | 内容 | 状態 |
|---|---|---|
| **M0** | 技術検証 PoC(トンネル・ハブ&スポーク・TCP/UDP 疎通・UPnP・計測) | ✅ **完了(2026-07-08、実機検証済み)** |
| **M1** | 招待トークン(pcv1)・コントロールチャネル・台帳配布・メンバー削除・init | ✅ **完了(2026-07-08、実機検証済み)** |
| **M2** | daemon 分離 + Tauri/React フル UI + インストーラ(仕様: `peercove-m2-handoff.md` v1.0 確定) | ⬅️ **ほぼ完了。G1〜G6 + G7a(サービス化)実機検証済み。G7b(MSI/deb/ZIP)実装済み・インストーラの実機検証待ち** |
| Phase 2 | UDP ホールパンチングによるメンバー間直接通信 | 未着手(§6 参照) |
| 対象外(当面) | macOS、モバイル、IPv6(構造上は妨げない) | — |

M0 の仕様正本は [peercove-m0-handoff.md](peercove-m0-handoff.md)。M0 の実測値と
検証記録は [m0-report-template.md](m0-report-template.md) と README の検証手順。
**M1 の正式な handoff 資料は依頼者から受領予定**(handoff §8 が予告)。

## 3. アーキテクチャ現状(M0 完了時点)

```
apps/
└─ peercove-ui/              # M2 デスクトップ UI(Tauri 2 + React)
    │                        #  ★ ルートのワークスペースから独立(exclude)
    ├─ src/                  # React。ipc.ts(UI 用の型)、App.tsx、notify.ts
    │                        #  components/: Start / Tunnel / Invite / Settings / Logs
    └─ src-tauri/src/        # lib.rs(invoke コマンド)、dto.rs(IPC→UI DTO)
                             #  tray.rs(トレイ常駐)

crates/
├─ peercove-core/            # OS 非依存。ユニットテスト必須
│   ├─ keys.rs               # X25519 鍵/PSK。Debug でも秘匿。base64
│   ├─ config.rs             # TOML 設定型 + 検証 + 相対パス解決
│   ├─ proto.rs              # コントロールチャネルのメッセージ(JSON Lines)
│   │                        #  Hello / Ledger / Removed / Ping / Pong(RTT 計測)
│   ├─ ipc.rs                # デーモン制御 IPC のプロトコル型(Status / Logs / …)
│   ├─ token.rs              # 招待トークン pcv1
│   └─ ipalloc.rs            # 空き仮想 IP の割当ヘルパ
├─ peercove-ipc/             # IPC クライアント(UI と CLI が共用)
├─ peercove-ops/             # 設定ファイル操作(UI と CLI が共用、非特権)
│   ├─ init.rs / invite.rs / join.rs / peers.rs   # 鍵・トークン・[[peer]] 編集
│   ├─ settings.rs           # [interface] と member の endpoint 編集(M2-G5)
│   └─ secret.rs / net.rs    # 秘密ファイルの権限、ローカル IP の推定
└─ peercove-poc/             # CLI + デーモン(将来 daemon-win / -linux に分離前提)
    ├─ main.rs               # clap。keygen/host/member/add-peer/status/down/udp-*/daemon
    ├─ daemon.rs             # IPC サーバー(名前付きパイプ / UDS)
    ├─ logbuf.rs             # 直近 500 行のリングバッファ(tracing Layer、ADR-0009)
    ├─ control.rs            # コントロールチャネル(台帳配布・削除通知・RTT ping)
    ├─ commands/
    │   ├─ tunnel.rs         # host/member/down。supervisor ループ
    │   │                    #  (5 秒ごと: 設定再読込→ピア同期、status/snapshot 書き出し)
    │   ├─ add_peer.rs       # host.toml へ [[peer]] をテキスト追記(コメント保持)
    │   ├─ status.rs         # <config>.status.txt の表示/整形/書き出し
    │   ├─ keygen.rs         # 鍵生成(Unix 600 / Windows icacls)
    │   └─ udp.rs            # udp-echo / udp-ping(G-5 検証ツール)
    ├─ upnp.rs               # igd-next。トンネル作成前に実行(重要)
    └─ backend/
        ├─ mod.rs            # ★ WgBackend trait(up/add_peer/stats/down)+ TunnelSpec
        ├─ linux.rs          # カーネル WG を defguard_wireguard_rs (netlink) で制御
        └─ windows/
            ├─ mod.rs        # wintun アダプタ管理、TunIo 実装
            └─ device.rs     # ★ ユーザー空間 WG デバイス(boringtun noise::Tunn)
                             #   3 スレッド: UDP受信 / TUN受信 / タイマー(250ms)
                             #   ピア間直接リレー(ハブ&スポーク)、roaming 学習
```

- **OS 依存はすべて `WgBackend` trait の背後**。新機能は可能な限り trait 経由で
- Windows は**ユーザー空間実装**なので、実行中プロセスの状態に別プロセスから
  触れない。add-peer は「設定追記+ホストの定期再読込」、status は
  「ステータスファイル」で解決している(ADR-0002)。**M1 のコントロール
  チャネルはこの制約を正面から解決するもの**でもある
- `windows/device.rs` は `TunIo` trait でモック可能になっており、
  **実トンネルなしで WG プロトコル一式のループバックテストができる**
  (2 ノード疎通・3 ノードリレー・ホスト再起動復帰の 3 本が既にある)。
  デバイスに触る変更は必ずここにテストを足すこと

### 確定済みの技術判断(詳細は decisions.md)

| ADR | 内容 |
|---|---|
| 0001 | Linux=defguard_wireguard_rs(kernel netlink)/ Windows=wintun+boringtun 0.7 自前デバイス |
| 0002 | add-peer=設定追記+5 秒再読込 / status=ステータスファイル(削除・変更の反映は M1) |
| 0003 | ハブ&スポーク: Windows=デバイス内リレー / Linux=IF 単位 /proc forwarding |
| 0004 | UPnP=igd-next、リース 24h、down で対削除 |
| 0005 | M1: 招待トークン pcv1(メンバー鍵を同梱)+ トンネル内コントロールチャネル |
| 0006 | 仮想 IP はランダムな `10.x.y.0/24`(CGNAT 衝突回避)+ `init` コマンド |
| 0007 | デーモン分離とローカル IPC(名前付きパイプ / UDS)。設定操作は IPC に乗せない |
| 0008 | `peercove-ops` crate。設定は UI が非特権で書く。トークンは発行直後のみ表示 |
| 0009 | ログ=デーモン内リングバッファ + IPC / RTT=制御チャネルの ping-pong / 通知は UI が検知 |

## 4. M0 で得た運用知見(検証でハマった点)

新しく作業する人は必ず目を通すこと。README のトラブルシューティングにも記載あり。

1. **Tailscale 併用機は 100.64.0.0/10 宛先を DROP** する(ts-input)。検証時は
   `sudo tailscale down`。症状: handshake・transfer 正常なのに ping 不通
2. **同一 LAN のメンバーは endpoint に LAN IP** を指定(家庭用ルーターは
   hairpin NAT 非対応が多い)。外部エンドポイントは別 NAT のメンバー専用
3. **Windows ホスト再起動後は約 15 秒**、メンバーの再ハンドシェイクを待つ
   (ユーザー空間実装はセッションを持ち越せない)
4. **UPnP の SSDP 探索はトンネル作成前に行う**(TUN のマルチキャスト経路に
   探索パケットが吸われる)。upnp.rs の doc コメント参照
5. wintun.dll は exe と同じフォルダのものを明示ロード(システムに別の
   wintun.dll がいる環境がある)
6. カーネル WG は「handshake 未成立」を UNIX エポックで返す(status で None 扱い)
7. **昇格プロセスが作る IPC 端点は、非特権クライアントから使えるよう明示的に
   権限を与える**こと。Windows は名前付きパイプに DACL を付与するが、ACE に
   総称権(`GA`/`GR`/`GW`)を書くとオブジェクト固有権へマップされず拒否される
   → `FR`/`FW`/`FA` を使う。Linux は root 作成の UDS を 0666 にする
   (daemon.rs の `winsec` とテスト `pipe_security_descriptor_allows_client_connect` 参照)
8. 管理者で起動したデーモンは非特権シェルから終了できない。二重起動でパイプ名が
   衝突すると新デーモンは即死する(エラー文言で誘導済み)
9. **Linux の UDS パスを「自分の euid」で決めると、root のデーモンと非特権の
   クライアントですれ違う**。サーバーは euid で bind 先を決め、クライアントは
   `/run/peercove.sock` → ユーザー領域の順に**候補を順に試す**
   (`peercove-ipc::socket_candidates`)。Windows はパイプ名が固定なので起きない
   = **OS 間で対称に見える設計でも、権限モデルの差でバグが出る**
10. **コントロールチャネルに定期メッセージ(ping 等)を足すときの罠が 2 つ**
    (M2-G5 の RTT 追加で顕在化。ADR-0009 §実装上の落とし穴):
    - `AsyncReadExt::take(n)` の上限は reader の**累計**。1 行ごとに `set_limit`
      で戻さないと、数時間後に EOF 誤認で制御接続が落ちる
    - `read_line` は**キャンセル安全でない**。`select!` の分岐に置くと、
      タイマー分岐が先に完了したときに読みかけの行が失われる。読み側は
      専用タスクの素直なループで回す
    どちらも「稀にしかメッセージが流れない」うちは表面化しない。
    回帰テストは `control.rs` の `read_line_*` 3 本

## 5. M1 ロードマップ(✅ 全タスク完了・実機検証済み 2026-07-08)

| # | タスク | 内容(概要) | 主な変更箇所 | 難易度 | 状態 |
|---|---|---|---|---|---|
| M1-1 | 招待トークン(pcv1) | ADR-0005 案 B(メンバー鍵同梱)。base64url + QR(fast_qr) | core/token.rs | ★★ | ✅ 実装済み(2026-07-08) |
| M1-2 | 台帳 | 独立ファイルにせず host.toml の `[[peer]]`(name 付き)を正本に。配布型は core/proto.rs | core | ★★ | ✅ 実装済み |
| M1-3 | invite / join コマンド | トークン発行(既定ファイル保存、--print/--qr)と参加設定生成 | commands/invite.rs, join.rs | ★★ | ✅ 実装済み |
| M1-4 | コントロールチャネル | ホスト仮想 IP の TCP 51821、JSON Lines。hello / 台帳配布 / 削除通知 | control.rs, tunnel.rs | ★★★★ | ✅ 実装済み |
| M1-5 | メンバー削除 | remove-peer(toml_edit)+ 2 段階反映(通知→実削除)+ WgBackend::remove_peer | backend/, commands/remove_peer.rs | ★★★★ | ✅ 実装済み |
| M1-6 | 仮想 IP 既定レンジの再検討 | ADR-0006: `init` コマンドがランダム 10.x.y.0/24 を生成。CGNAT レンジ使用時は起動警告 | commands/init.rs, core/ipalloc.rs | ★★ | ✅ 実装済み |
| M1-7 | ピア設定変更の動的反映 | sync_peers がフィンガープリント比較で変更検知 → 削除+再追加で反映 | tunnel.rs | ★★★ | ✅ 実装済み |

**M1 は全タスク実装・実機検証済み(2026-07-08)**。実現した UX:
`init`(ホスト初期化)→ `invite`(トークン発行、QR 可)→ `join`(貼るだけ参加)
→ 台帳自動配布 → `remove-peer`(削除通知付き)。関連 ADR: 0005/0006。

### 作業の振り分けガイド(依頼者の意向)

- **Opus に任せてよいもの(★〜★★★の一部)**: peercove-core の純粋ロジック
  (トークン・台帳・IP 割当)、CLI コマンドの追加、ドキュメント、ユニットテスト、
  README 手順の整備
- **高難度 = メインセッション(Fable)で続けて実装するもの(★★★★)**:
  - `backend/windows/device.rs` の内部(スレッド・ロック・boringtun の
    TunnResult 処理・セッション管理)に触る変更
  - コントロールチャネルのプロトコル設計と、その暗号・認証の判断
  - `WgBackend` trait のシグネチャ変更(両 OS 実装 + テストへの波及が大きい)
  - OS API 境界(unsafe、netlink、wintun FFI)の変更
- 判断に迷う変更・アーキテクチャに関わる選択は、実装前に依頼者へ確認し、
  決定は decisions.md へ ADR 追記(これは担当者を問わず必須)

## 6. 今後の作業トラック(M1 完了後の整理)

3 つのトラックに整理する。**A と B は依頼者の判断(事業・体験の優先度)が必要**。
C はどのトラックとも並行でき、着手判断を待たずに進められる。

### トラック A: M2 — プロダクト化(UI・配布)

「CLI を使えない人でも招待 URL/QR だけで参加できる」体験を作る。

| # | タスク | 概要 | 難易度 | 担当目安 |
|---|---|---|---|---|
| A-1 | M2 要件の確定 | ✅ 完了。`peercove-m2-handoff.md` v1.0(§9 で Q1〜Q6 確定) | — | **依頼者**(+相談) |
| A-2 | daemon 分離 | ✅ 完了(M2-G1、ADR-0007) | ★★★★ | Fable |
| A-3 | Tauri + React UI | ✅ 完了(M2-G2〜G6)。状態表示・接続/参加・招待/管理・設定/ログ/RTT・トレイ/通知 | ★★★ | Fable |
| A-4 | インストーラ・自動起動 | **M2-G7 として残り**。Windows MSI / Linux deb、wintun.dll 同梱(Q5 で可)、サービス化(Windows サービス / systemd) | ★★★★ | Fable(サービス化)+ Opus(バンドル) |
| A-5 | コード署名 | M3 送り | ★★ | 依頼者(手続き)+ Opus |

### トラック B: Phase 2 — メンバー間直接通信(ホールパンチング)

リレー負荷とレイテンシを下げる。ゲーム用途の品質向上に直結。

| # | タスク | 概要 | 難易度 | 担当目安 |
|---|---|---|---|---|
| B-1 | 外部エンドポイント観測 | ホストは各メンバーの外部 IP:ポートを既に学習している。これを台帳に載せて配布 | ★★ | Opus 可 |
| B-2 | パンチング調停 | コントロールチャネルで 2 メンバーに同時ハンドシェイクを指示。成功したらピア追加(AllowedIPs /32 直接) | ★★★★★ | Fable |
| B-3 | フォールバック | 失敗時・NAT 種別非対応時はホスト経由を維持。経路の自動切替と status 表示 | ★★★★ | Fable |
| B-4 | NAT 挙動の実地調査 | 依頼者環境(テザリング含む)での成功率計測 | — | 依頼者(検証) |

### トラック C: 品質・性能(並行可・小粒)

| # | タスク | 概要 | 難易度 | 担当目安 |
|---|---|---|---|---|
| C-1 | GitHub リポジトリ + CI | ✅ **完了(2026-07-09)**。公開前クリーニング(履歴書き換え)+ https://github.com/sgcmt176/peercove へ push。CI は `.github/workflows/ci.yml`: rust ジョブ(Ubuntu/Windows で fmt・clippy -D warnings・test)+ ui ジョブ(npm build + src-tauri test)。Linux は実ランナーで検証するため zigbuild は手元確認用に降格 | ★★ | 完了 |
| C-2 | スループット実機計測 | VM でなく実機 3 台で iperf3 再計測(m0-report 更新) | — | 依頼者(検証) |
| C-3 | Windows デバイス性能改善 | C-2 の結果次第。暗号処理の並列化・バッファ最適化・wireguard-nt 数値比較 | ★★★★ | Fable |
| C-4 | 鍵ローテーション | ADR-0005 の将来課題。コントロールチャネル経由でメンバー鍵を更新 | ★★★★ | Fable |
| C-5 | status のリアルタイム化 | ✅ A-2 に合流(IPC の `Status` / `Logs` で取得。ステータスファイルは CLI 互換のため残置) | ★★★ | 完了 |

### トラック D: M3 — 機能拡張(2026-07-09 依頼者指定)

依頼者が選定した 9 機能 + 複数ネットワーク対応(2026-07-09 追加要望)。
番号は推奨実装順(依存関係を考慮)。1 ゴールずつ
「設計(必要なら ADR)→ 実装 → README → コミット → 実機検証依頼」で進める。

| # | 機能 | 概要 | 難易度 | 依存・備考 |
|---|---|---|---|---|
| M3-0a | 複数ネットワーク: 設定配置 | ✅ 完了(実機検証済み 2026-07-10)。networks/ + 自動移行 + ネットワーク名(ADR-0012) | ★★★ | —  |
| M3-0b | 複数ネットワーク: デーモン | ✅ 完了(実機検証済み 2026-07-10)。多重トンネル + IPC 改訂 + CLI 追随 | ★★★★ | —  |
| M3-0c | 複数ネットワーク: UI | ✅ 完了(実機検証済み 2026-07-10)。カード一覧 + 個別接続/切断 + 追加/削除 | ★★★ | —  |
| M3-1 | DNS(内蔵リゾルバ) | ✅ 完了(実機検証済み 2026-07-10)。`<名前>.<ネットワーク名>.peercove.internal`。内蔵 DNS(a: 土台)+ スプリット DNS(b: NRPT / resolvectl)+ 管理画面(c)(ADR-0011) | ★★★★ | なし |
| M3-2 | 直接通信 B-1 | ✅ 完了(実機検証済み 2026-07-10)。外部エンドポイント + 観測経過秒を台帳配布(オンラインのみ)+ Windows デバイスの最長一致修正(ADR-0013) | ★★ | —  |
| M3-3 | 直接通信 B-2 | ✅ 完了(実機検証済み 2026-07-10)。台帳駆動の暗黙パンチング。成功時は /32 直接ピア。`direct` 設定フラグ(UI トグルあり)+ 鮮度ガード + タイムアウト/クールダウン(ADR-0013) | ★★★★★ | —  |
| M3-4 | 直接通信 B-3 | ✅ 完了(実機検証済み 2026-07-10)。経路バッジ(直接/中継/確立中)+ 外部 IP 共有の UI 説明文(ADR-0013)。※検証フィードバックで再試行を指数バックオフ → **試行の無害化 + 固定間隔 60 秒**に改訂(ADR-0019、実機検証済み 2026-07-12) | ★★★★ | —  |
| M3-5 | 招待ディープリンク | ✅ 完了(実機検証済み 2026-07-10、Windows/Linux)。`peercove://join?token=…`(tauri-plugin-deep-link + single-instance、起動時自動登録)。**ログイン時自動起動は不要**(2026-07-10 依頼者判断で対象外) | ★★ | —  |
| M3-6 | UI デザインパス | ✅ 完了(実機検証済み 2026-07-10)。テーマ切替(ライト/ダーク、初回は OS 設定)・メンバー色分けアバター(公開鍵から決定的)・転送量/RTT スパークライン・トレイからのネットワーク別接続/切断。詳細画面をタブ構成にし、**ファイル送信(M3-9)・チャット(M3-13)はタブ追加で収まる**(2026-07-10 依頼者指定) | ★★ | なし |
| M3-7 | サブネットルーター | ✅ **完了(7a/7b とも実機検証済み 2026-07-10、Windows ホスト + Linux ルーター役 + Docker 疑似 LAN)**(ADR-0014)。host.toml `[[peer]].subnets` が正本 → 台帳配布 → AllowedIPs/OS ルート。ルーター役は V1 Linux 限定(転送 + SNAT 自動設定)。UI = メンバー行の 🖧 で編集 + CIDR バッジ | ★★★ | ACL より先に |
| M3-8 | ~~Wake-on-LAN~~ | **不要**(2026-07-10 依頼者判断で対象外) | — | —  |
| M3-9 | ファイル送信 | ✅ **完了(9a/9b とも実機検証済み 2026-07-11、800 MB 転送確認)**(ADR-0015)。各デーモンが仮想 IP の TCP 51822 で待受け(台帳照合)、受信ボックス `networks/<net>.inbox/` に自動保存(SHA-256 検証)。受信サイズ上限(既定 100 MB、受信側設定、0 で無制限)。UI = 📤 送信・受信タブ(進捗/受信ボックス)・OS 通知・設定に上限 | ★★★ | チャット(M3-13)と基盤(トンネル内メッセージング)を共有 |
| M3-10 | アクセス制御(ACL) | ✅ **完了(実機検証済み 2026-07-12)**(ADR-0018)。host.toml `[acl]` の deny 組(仮想 IP ペア)が正本。リレーはホストが破棄(Windows = デバイス内リレー判定 / Linux = iptables DROP)、直接経路は台帳のメンバーごとフィルタ(endpoint 非配布 + blocked フラグ)で解除。UI = ホストの「🚦 通信制御」ダイアログ + 「通信不可」バッジ + チャット/ファイル送信の抑止 | ★★★★ | B 完了後の条件は満たした(リレー・直接の両経路に適用) |
| M3-11 | 鍵ローテーション | C-4 を合流。✅ **完了(実機検証済み 2026-07-12)**(ADR-0020)。メンバーが端末上で新鍵を生成し、公開鍵だけをコントロールチャネルでホストへ届けて差し替える(参加直後に自動 1 回 + UI の「鍵を更新」)。応答喪失時は受信 45 秒停止で新旧鍵を交互試行して自己回復 | ★★★★ | B 完了後(プロトコル変更の競合回避) |
| M3-12 | 自動アップデート通知 | GitHub Releases を見て新版を通知。**Opus 担当予定**(2026-07-12 依頼者指定) | ★★ | **最後**(依頼者指定)。自動適用はコード署名(M4)後 |
| M3-14 | DNS 拡張トラック | 依頼者要望(2026-07-12)の DNS 機能群。**14a(表示名/DNS 名の分離・固定化)は実装済み(2026-07-12、ADR-0021)、実機検証待ち**。以降 14b(エイリアス・サービス名・LAN 機器レコード)→ 14c(サービス情報 UI、Opus 候補)→ 14d(逆引き)/14f(ワイルドカード、Opus 候補)→ 14e(状態連動)→ 14g(短縮名) | ★〜★★★★ | 14a が他すべての前提 |
| M3-13 | チャット | ✅ **完了(13a〜13e すべて実機検証済み 2026-07-11)**(ADR-0016/0017)。LINE 風の 1:1 / 任意グループ / ネットワーク全体。チャット内ファイル送信 + D&D、画像/動画/テキストのインラインプレビュー、受信失敗のお知らせ、一括ファイル送信ダイアログ、OS 通知のオン/オフ設定、URL のリンク化 + リンクプレビュー(表示端末が自分で OGP 取得)。対象は VPN 内メンバーのみ・トンネル内通信で外部サーバー不要・履歴はローカル保存 | ★★★ | M3-9 と基盤(メンバー間のトンネル内メッセージング)を共有 |

### 推奨順序(2026-07-08 時点の現在地)

1. ~~C-1(GitHub + CI)~~ → **完了(2026-07-09)**。公開リポジトリ + CI が品質ゲートとして稼働
2. **現在はトラック D(M3)を M3-1 から順に進める**(2026-07-09 依頼者決定)
2. ~~A-1(M2 要件確認)~~ → 完了。A-2 / A-3 も完了
3. **A-4 = M2-G7 は前後半とも実装済み**。前半(G7a: サービス化、ADR-0010)は
   Session 0 PoC まで実機検証済み。後半(G7b: MSI / deb / ZIP)は実装済みで
   **インストーラの実機検証待ち**(指示書 `docs/peercove-g7-packaging-handoff.md`、
   MSI 方式変更の確認 `docs/peercove-g7b-msi-review-for-fable.md`)。
   残る M2 の仕上げはコード署名(M3)を除けば実機検証のみ
4. **B(直接通信)は M2-G7 と並行可**。daemon 分離(A-2)が前提だったので、
   その条件は満たしている。ゲーム用途の体感を早く上げたいなら B を先にする判断もあり

## 7. 開発ワークフロー(全員共通)

1. **1 ゴール(タスク)ずつ**実装 → 動作確認手順を README に追記 → コミット
2. ネットワーク実疎通の確認は**依頼者が実機で行う**。「検証依頼」として手順を
   提示し、結果報告を待ってから次へ進む(自動テスト化はしなくてよい)
3. コミット前に必ず:
   ```bash
   cargo fmt --check
   cargo clippy --all-targets -- -D warnings
   cargo test --workspace
   ```
4. **Linux 側のコンパイル検証**(Windows 開発機の場合): 素の cross check は
   ring の C ビルドで失敗するため cargo-zigbuild を使う:
   ```bash
   cargo-zigbuild clippy --workspace --all-targets --target x86_64-unknown-linux-gnu -- -D warnings
   ```
5. Windows デバイス(device.rs)に触ったら、既存のループバックテスト
   (`cargo test -p peercove-poc`)が通ることを必ず確認。挙動追加時はテストも追加
6. 秘密鍵・PSK・トークンは**ログ・標準出力に出さない**(公開鍵は可)。
   新しい秘密型には redact された Debug 実装とそのテストを書く(keys.rs が手本)

## 8. 既知の技術的負債・メモ

- **スループット**: VM + Windows ユーザー空間実装で TCP ~30 Mbps
  (m0-report-template.md)。ボトルネック候補: 暗号処理のシングルスレッド、
  wintun リング往復、2KB バッファのコピー(→ トラック C-2/C-3)
- **メンバー側の設定変更**: member.toml の変更は再起動が必要(host 側のみ動的反映)
- **udp-ping の統計未取得**(m0 レポート §5)。次回の実機検証時についでに埋める
- boringtun はフォーク群(NepTUN 等)が活発。メジャーバージョン更新時は
  ADR-0001 の再評価を(乗り換え先候補: NepTUN)
- git リモート: https://github.com/sgcmt176/peercove (public、2026-07-09〜)。
  公開時に filter-repo で履歴を書き換えたため(target-smoke 除去・作者メール
  noreply 化など)、**それ以前のクローン・バンドルとはハッシュが不一致**。
  古いクローン(Linux VM 等)は pull でなく再クローンすること
