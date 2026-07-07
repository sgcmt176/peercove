# PeerCove ロードマップと開発引き継ぎガイド

- **最終更新**: 2026-07-08(M0 完了時点)
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
| **M1** | 招待トークン(pcv1)・トンネル内コントロールチャネル・台帳配布・メンバー削除の CLI E2E | ⬅️ **次はここ** |
| M2(推定) | Tauri + React UI、インストーラ、コード署名 | 未着手(上位文書の確認が必要) |
| Phase 2 | UDP ホールパンチングによるメンバー間直接通信 | 未着手 |
| 対象外(当面) | macOS、モバイル、IPv6(構造上は妨げない) | — |

M0 の仕様正本は [peercove-m0-handoff.md](peercove-m0-handoff.md)。M0 の実測値と
検証記録は [m0-report-template.md](m0-report-template.md) と README の検証手順。
**M1 の正式な handoff 資料は依頼者から受領予定**(handoff §8 が予告)。

## 3. アーキテクチャ現状(M0 完了時点)

```
crates/
├─ peercove-core/            # OS 非依存。ユニットテスト必須
│   ├─ keys.rs               # X25519 鍵/PSK。Debug でも秘匿。base64
│   ├─ config.rs             # TOML 設定型 + 検証 + 相対パス解決
│   └─ ipalloc.rs            # 空き仮想 IP の割当ヘルパ
└─ peercove-poc/             # CLI(将来 daemon-win / daemon-linux に分離前提)
    ├─ main.rs               # clap。keygen/host/member/add-peer/status/down/udp-*
    ├─ commands/
    │   ├─ tunnel.rs         # host/member/down。supervisor ループ
    │   │                    #  (5 秒ごと: 設定再読込→新規ピア動的追加、status 書き出し)
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
| メモ | 仮想 IP 100.100.42.x は Tailscale(100.64.0.0/10)と衝突 → **M1 で既定レンジ再検討** |

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

## 5. M1 ロードマップ(暫定タスク分解)

> 正式な M1 handoff を受領したらこの節を更新すること。以下は M0 handoff §8 の
> 予告(招待トークン pcv1 / トンネル内コントロールチャネル / 台帳配布 /
> メンバー削除の CLI E2E)に基づく暫定分解。

| # | タスク | 内容(概要) | 主な変更箇所 | 難易度 | 状態 |
|---|---|---|---|---|---|
| M1-1 | 招待トークン(pcv1) | ADR-0005 案 B(メンバー鍵同梱)。base64url + QR(fast_qr) | core/token.rs | ★★ | ✅ 実装済み(2026-07-08) |
| M1-2 | 台帳 | 独立ファイルにせず host.toml の `[[peer]]`(name 付き)を正本に。配布型は core/proto.rs | core | ★★ | ✅ 実装済み |
| M1-3 | invite / join コマンド | トークン発行(既定ファイル保存、--print/--qr)と参加設定生成 | commands/invite.rs, join.rs | ★★ | ✅ 実装済み |
| M1-4 | コントロールチャネル | ホスト仮想 IP の TCP 51821、JSON Lines。hello / 台帳配布 / 削除通知 | control.rs, tunnel.rs | ★★★★ | ✅ 実装済み |
| M1-5 | メンバー削除 | remove-peer(toml_edit)+ 2 段階反映(通知→実削除)+ WgBackend::remove_peer | backend/, commands/remove_peer.rs | ★★★★ | ✅ 実装済み |
| M1-6 | 仮想 IP 既定レンジの再検討 | Tailscale 衝突の恒久対応(ランダム 10.x /24 生成など)。ADR 化 | core/config、examples | ★★(Opus 可、ADR は要レビュー) | 未着手 |
| M1-7 | ピア設定変更の動的反映 | エンドポイント・PSK 変更等の反映(追加・削除は対応済み) | tunnel.rs、backend/ | ★★★(中〜高) | 未着手 |

M1-1〜M1-5 は実装・ユニットテスト完了、**実機検証待ち**(README の
「検証手順(M1-G1〜G3)」参照)。

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

## 6. 開発ワークフロー(全員共通)

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

## 7. 既知の技術的負債・将来課題

- **スループット**: VM + Windows ユーザー空間実装で TCP ~30 Mbps
  (m0-report-template.md)。ボトルネック候補: 暗号処理のシングルスレッド、
  wintun リング往復、2KB バッファのコピー。実機での再計測 → 必要なら
  デバイスループのマルチスレッド化 / wireguard-nt 比較(数値レポートのみで可)
- **status のリアルタイム性**: ステータスファイル方式(5 秒周期)。M1 の
  コントロールチャネル or IPC 導入時に置き換え検討
- **udp-ping の統計未取得**(m0 レポート §5)。M1 検証時についでに埋める
- boringtun はフォーク群(NepTUN 等)が活発。メジャーバージョン更新時は
  ADR-0001 の再評価を(乗り換え先候補: NepTUN)
