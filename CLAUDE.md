# CLAUDE.md — PeerCove リポジトリ指示

このリポジトリは PeerCove(サーバーレス P2P VPN、WireGuard プロトコル利用)の開発リポジトリです。
現在のフェーズは **M0: 技術検証(PoC)** です。仕様の正本は `docs/peercove-m0-handoff.md` にあります。実装前に必ず読んでください。

## プロジェクトの前提

- 事業者サーバーは持たない。ホスト PC がコーディネーター兼リレー
- Phase 1 は WireGuard AllowedIPs + OS ルーティングによるハブ&スポーク(ホスト経由)
- アプリ層 UDP プロキシは作らない。TCP/UDP/ICMP はトンネル内の通常 IP 通信として扱う
- 対応 OS: Windows 10/11, Ubuntu 22.04+。**macOS のコードは書かない**(将来対応を妨げない抽象化はする)
- M0 は CLI のみ。Tauri/React UI はまだ作らない
- 製品名・crate 名・バイナリ名に "WireGuard" を含めない(商標方針)。説明文での言及は可

## ワークスペース構成

- `crates/peercove-core` … OS 非依存(鍵、設定型、IP 割当)。ユニットテスト必須
- `crates/peercove-poc` … CLI。OS 依存層(TUN/WG/ルーティング/フォワーディング)は trait で抽象化し、`#[cfg(target_os)]` で実装を分ける

## コーディング規約

- Rust stable / edition 2021+。非同期は tokio
- エラー: バイナリは `anyhow`、ライブラリは `thiserror`
- ログ: `tracing`。**秘密鍵・PSK・トークンを絶対にログ・標準出力へ出さない**(公開鍵は可)
- unsafe は OS API 境界のみ。理由をコメントで残す
- コミット前に `cargo fmt --check` と `cargo clippy --all-targets -- -D warnings` を通す

## 作業ルール

- 外部 crate を採用する前に、メンテ状況(最終リリース、issue の放置状況)を確認する。特に WG バックエンド(boringtun 等)は流動的なので必ず最新情報を調べる
- アーキテクチャ上の判断(バックエンド選定、ピア動的追加の方式、フォワーディング設定方式など)は `docs/decisions.md` に ADR 形式(背景/選択肢/決定/理由)で追記する
- ネットワーク実疎通を伴う検証は自動テスト化しなくてよい。代わりに README に人間が再現できる手順を書く
- root/管理者権限が必要な操作は、コード内で必要性を検出して分かりやすいエラーを出す(黙って失敗しない)
- クリーンアップ(`down`)を常に対で実装する。TUN・ルート・フォワーディング設定の残骸を残さない
- 1 つのゴール(handoff の G-1〜G-7)を終えるごとに、動作確認手順を README に反映してからコミットする

## よく使うコマンド

```bash
cargo build --workspace
cargo test -p peercove-core
cargo clippy --all-targets -- -D warnings
cargo fmt --check

# Linux での起動例(要 sudo)
sudo ./target/debug/peercove-poc host --config host.toml
sudo ./target/debug/peercove-poc member --config member.toml
```

## やらないこと(M0)

- 招待トークン / QR / コントロールチャネル / 台帳 / メンバー削除(M1 で実装)
- UDP ホールパンチング、直接通信(Phase 2)
- UI、インストーラ、コード署名、macOS、モバイル、IPv6
