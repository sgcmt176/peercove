# CLAUDE.md — PeerCove リポジトリ指示

このリポジトリは PeerCove(サーバーレス P2P VPN、WireGuard プロトコル利用)の開発リポジトリです。
**M0(技術検証 PoC)・M1(招待/コントロールチャネル/台帳/メンバー削除/init)・M2(デーモン分離 + Tauri/React UI + インストーラ)は完了済み**。**M3 の各機能**(トンネル内 DNS、メンバー間直接通信、サブネットルーター、ファイル送信、チャット/グループ、通信制御 ACL、デバイス鍵ローテーション、暗号化バックアップ、接続診断、通信品質履歴、DNS ヘルスチェック、更新通知など)も実装・実機検証済み。デュアルライセンス・依存ライセンス/脆弱性 CI・手動セキュリティレビューまで完了し、v0.1.0 をリリース済み。現在は **M4(Android 版、メンバー専用)** の設計が完了し実装はこれから(ADR-0039、`docs/roadmap.md` §6 トラック E)。経緯は `docs/roadmap.md`、技術判断は `docs/decisions.md` の ADR。

実装前に必ず読むこと:
1. `docs/roadmap.md` … 全体像・現在地・**難易度別の作業振り分けガイド**(§5)・開発ワークフロー
2. `docs/decisions.md` … 確定済みの技術判断(ADR 一覧)。矛盾する実装をしないこと
3. `docs/verification.md` … 実機での動作確認手順(新しいゴールはここへ追記)

過去のマイルストーン仕様・計画・計測記録は `docs/history/` に保管(歴史的記録。
現行の正本は上記 3 つと `docs/user-guide.md`)。

## プロジェクトの前提

- 事業者サーバーは持たない。ホスト PC がコーディネーター兼リレー
- Phase 1 は WireGuard AllowedIPs + OS ルーティングによるハブ&スポーク(ホスト経由)
- アプリ層 UDP プロキシは作らない。TCP/UDP/ICMP はトンネル内の通常 IP 通信として扱う
- 対応 OS: Windows 10/11, Ubuntu 22.04+。M4 で Android(メンバー専用)を追加(ADR-0039)。**macOS のコードは書かない**(将来対応を妨げない抽象化はする)
- 製品名・crate 名・バイナリ名に "WireGuard" を含めない(商標方針)。説明文での言及は可

## ワークスペース構成

- `crates/peercove-core` … OS 非依存(鍵、設定型、IP 割当、プロトコル型)。ユニットテスト必須
- `crates/peercove-ipc` … デーモン制御 IPC のクライアント(UI と CLI が共用)
- `crates/peercove-ops` … 設定ファイル操作(init / invite / join / メンバー管理)。表示を持たず構造体を返す。UI と CLI が共用
- `crates/peercove-cli` … CLI + デーモン。OS 依存層(TUN/WG/ルーティング/フォワーディング)は trait で抽象化し、`#[cfg(target_os)]` で実装を分ける
- `crates/peercove-mobile` … M4 モバイル用 Rust コア。UniFFI で Kotlin へ公開(頭脳は Rust、OS 連携は Kotlin = ADR-0039)
- `apps/peercove-ui` … M2 デスクトップ UI(Tauri + React)。**ルートのワークスペースから独立**しているので、`cargo test --workspace` には含まれない。UI を変更したら `apps/peercove-ui/src-tauri` で `cargo test` と `npm run build` を別途通すこと
- `apps/peercove-android` … M4 Android アプリ(Kotlin + Compose)。ワークスペース外。ビルドは `gradlew assembleDebug`(cargo-ndk → uniffi-bindgen → APK を一気通貫。`docs/development.md` 参照)

## コーディング規約

- Rust stable / edition 2021+。非同期は tokio
- エラー: バイナリは `anyhow`、ライブラリは `thiserror`
- ログ: `tracing`。**秘密鍵・PSK・トークンを絶対にログ・標準出力へ出さない**(公開鍵は可)
- unsafe は OS API 境界のみ。理由をコメントで残す
- コミット前に `cargo fmt --check` と `cargo clippy --all-targets -- -D warnings` を通す

## 作業ルール

- 外部 crate を採用する前に、メンテ状況(最終リリース、issue の放置状況)を確認する。特に WG バックエンド(boringtun 等)は流動的なので必ず最新情報を調べる
- アーキテクチャ上の判断(バックエンド選定、ピア動的追加の方式、フォワーディング設定方式など)は `docs/decisions.md` に ADR 形式(背景/選択肢/決定/理由)で追記する
- ネットワーク実疎通を伴う検証は自動テスト化しなくてよい。代わりに `docs/verification.md` に人間が再現できる手順を書く
- root/管理者権限が必要な操作は、コード内で必要性を検出して分かりやすいエラーを出す(黙って失敗しない)
- クリーンアップ(`down`)を常に対で実装する。TUN・ルート・フォワーディング設定の残骸を残さない
- 1 つのゴール(タスク)を終えるごとに、動作確認手順を `docs/verification.md` に反映してからコミットする(README はリリース向けの製品紹介。機能の使い方は `docs/user-guide.md`、ビルド手順は `docs/development.md` に書く)
- **難易度の高い実装(Windows デバイスループ内部、コントロールチャネルのプロトコル設計、WgBackend trait の変更、OS API 境界)は、docs/roadmap.md §5 の振り分けガイドに従うこと**

## よく使うコマンド

```bash
cargo build --workspace
cargo test --workspace          # Windows ではデバイスのループバックテストも走る
cargo clippy --all-targets -- -D warnings
cargo fmt --check

# Windows 開発機から Linux 側のコンパイル/lint を検証する場合(要 zig + cargo-zigbuild)
cargo-zigbuild clippy --workspace --all-targets --target x86_64-unknown-linux-gnu -- -D warnings

# Linux での起動例(要 sudo)
sudo ./target/debug/peercove host --config host.toml
sudo ./target/debug/peercove member --config member.toml
```

## やらないこと(現時点)

- macOS、iOS、IPv6(構造上は妨げないが対象外。Android は M4 で対応する = ADR-0039)
- コード署名(**見送り** = 2026-07-23 依頼者判断。証明書費用が発生するため。
  リリースは未署名のまま。自動アップデート適用も引き続き保留)
- IPC の接続元認可(同一 PC のユーザー/グループを問わず操作可。単一ユーザー PC
  前提 = ADR-0007。一度実装したが依頼者判断で撤回 = ADR-0038)
