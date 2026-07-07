# PeerCove 技術判断記録(ADR)

M0 中に行ったアーキテクチャ上の判断を ADR 形式(背景/選択肢/決定/理由)で追記する。

---

## ADR-0001: WG バックエンド選定

- **日付**: 2026-07-07
- **状態**: 承認済み(2026-07-07 依頼者承認)

### 背景

M0 では Windows 10/11 と Ubuntu 22.04+ の双方で WireGuard プロトコルのトンネルを
CLI から作成・制御する必要がある。handoff 5.1 の第一候補は
「Linux: カーネル WG + netlink 制御 crate」「Windows: wintun + ユーザー空間実装(boringtun 系)」。
boringtun のメンテ状況は流動的とされていたため、着手前に最新状況を Web 調査した(2026-07-07 実施)。

### 調査結果(2026-07-07 時点)

| crate / プロジェクト | 最新版・日付 | ライセンス | 所見 |
|---|---|---|---|
| `boringtun` (Cloudflare) | 0.7.1 / 2026-05-01 | BSD-3 | 2024 年に放置疑惑(issue #407)→ その後再構築され 0.7 系でリリース再開。0.7 は `device` モジュールが削除され、プロトコルエンジン(`noise::Tunn`)+ `x25519` のみ。Windows 用デバイスループは元々無いので影響なし |
| NepTUN (NordSecurity) | v1.0.8 / 2025-07 | BSD-3 | boringtun フォーク。活発だが crates.io 未公開(git 依存が必要) |
| GotaTun (Mullvad) | 2025-12 発表 | — | boringtun フォーク。Android のみ・第三者監査前。汎用ライブラリとして未成熟 |
| `defguard_wireguard_rs` | 0.10.0 / 2026-06 | Apache-2.0 | 活発。Linux はカーネル WG を netlink で制御する高レベル API(インターフェース作成・ピア設定・統計取得)。Windows は wireguard-nt(wireguard.dll)方式で、`x86_64-pc-windows-gnu` ツールチェーン + MSYS2 が必要という開発摩擦あり |
| `wireguard-control` (innernet) | 2.0.0 / 2026-07-02 | LGPL-2.1+ | 活動あり。ただし LGPL で、innernet 向け設計。C ライブラリ依存の記述あり |
| `netlink-packet-wireguard` (rust-netlink) | — | MIT | 低レベル(netlink パケットの構築のみ)。rtnetlink と組み合わせるグルーコードが多い |
| `wintun` | 0.5.1 / 2025-01 | MIT | wintun.dll の安全なバインディング。安定してメンテされている |

### 選択肢

1. **Linux: `defguard_wireguard_rs`(kernel モード) / Windows: `wintun` + `boringtun` 0.7 の `noise::Tunn` + 自前デバイスループ**
2. 両 OS とも `defguard_wireguard_rs`(Windows は wireguard-nt 方式)
3. Linux: `netlink-packet-wireguard` + `rtnetlink` を直接使用 / Windows は 1 と同じ
4. Windows のエンジンに NepTUN(git 依存)を使用
5. Windows: wireguard-go サイドカー

### 決定(提案)

**選択肢 1** を採用する。

- Linux: `defguard_wireguard_rs`(カーネル WG を netlink 制御)
- Windows: `wintun` crate + `boringtun` 0.7 `noise::Tunn` + 自前の非同期デバイスループ
  (wintun 読み書き ↔ UDP ソケット ↔ Tunn、タイマー tick 含む)
- 両者は `peercove-poc` 内の `WgBackend` trait の背後に隠し、`#[cfg(target_os)]` で切替

### 理由

- Linux のカーネル WG は性能・安定性で最良、handoff の第一候補どおり。
  `defguard_wireguard_rs` は活発(直近 1 ヶ月にリリース)で、インターフェース作成〜
  ピア設定〜統計取得(`status` に必要)まで高レベル API で揃う。
  低レベル netlink crate 直叩き(選択肢 3)よりグルーコードが大幅に少ない
- Windows で `defguard_wireguard_rs`(選択肢 2)を使うと wireguard-nt 方式になり、
  windows-gnu ツールチェーン + MSYS2 必須という開発環境摩擦が大きい。
  また handoff は wintun + ユーザー空間実装を第一候補としている
- boringtun はリリースが再開(0.7.1 / 2026-05)しており、必要なのはプロトコル
  エンジン(`noise::Tunn`)のみ。デバイスループは Windows では元々自前実装が
  必要だったため、0.7 の `device` モジュール削除は影響しない
- NepTUN(選択肢 4)は crates.io 未公開で git 依存になるため第一候補にしないが、
  boringtun 0.7 の API に想定外の欠落があった場合の**フォールバック**とする
  (API は boringtun 系で互換性が高い)
- wireguard-go サイドカー(選択肢 5)は別バイナリの配布・プロセス管理が増えるため
  M0 では不採用

### 却下理由まとめ

- GotaTun: 未成熟(Android のみ・監査前・ライブラリとして未公開)
- `wireguard-control`: LGPL と innernet 向け設計。Apache/MIT 系で揃えられる代替がある

### 備考

- wintun.dll は再配布せず、開発機に手動配置(入手手順を README に記載)
- UPnP(G-6)の crate 選定は着手時に別 ADR として記録する(`igd` 系の最新状況を確認)
