# PeerCove

事業者サーバーを持たない P2P 型 VPN(技術検証中)。ホスト PC がコーディネーター兼リレーを担い、トンネルには WireGuard プロトコルを使用します。

現在のフェーズは **M0: 技術検証(PoC)** です。仕様は [docs/peercove-m0-handoff.md](docs/peercove-m0-handoff.md)、技術判断は [docs/decisions.md](docs/decisions.md) を参照してください。

## 進捗(M0 ゴール)

- [x] ワークスペース・keygen・設定 TOML 読み込み(G-1 前半)
- [ ] G-1: トンネル作成・破棄(Windows / Linux)
- [ ] G-2: 1対1疎通(Host–Member ping)
- [ ] G-3: ハブ&スポーク疎通(Member ↔ Member、Host 経由)
- [ ] G-4: TCP 疎通
- [ ] G-5: UDP 疎通(udp-echo / udp-ping)
- [ ] G-6: UPnP による到達性セットアップ
- [ ] G-7: 計測レポート

## 必要環境

### 共通

- [Rust(stable)](https://rustup.rs/) — `rustup` でインストール

### Windows 10/11

- Visual Studio(C++ ビルドツール)— rustup のインストール時に案内されます
- トンネル操作(G-1 以降)は **管理者として実行した** PowerShell / ターミナルが必要です
- wintun.dll(G-1 以降で必要。入手手順は G-1 実装時に追記)

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

[examples/host.example.toml](examples/host.example.toml) と [examples/member.example.toml](examples/member.example.toml) をコピーして編集してください。

`host` / `member` / `down` / `status` コマンドは現時点では**設定の読み込み・検証のみ**行います(トンネル操作は G-1 以降で実装)。

```bash
./target/debug/peercove-poc member --config member.toml
# → 設定 OK: interface=peercove0 address=100.100.42.2/24 mtu=1420 peers=1
```

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
