#!/bin/sh
# PeerCove の第三者ライセンス謝辞(THIRD-PARTY-NOTICES.txt)を生成する。
#
# 生成物は Rust 依存(CLI/デーモン + UI/Tauri)+ npm 依存(フロントエンド)の
# ライセンス表示を 1 ファイルに集約したもの。リリースの MSI / deb / ZIP / tar に
# 同梱する(配布時の第三者表示義務のため。方針は packaging/licenses/README.md)。
#
# 前提ツール:
#   cargo install cargo-about        # Rust 依存の集約
#   node(npm 依存の集約。apps/peercove-ui/node_modules が入っていること = npm ci 済み)
#
# 使い方(リポジトリのどこからでも。Windows は Git Bash で):
#   sh packaging/make-notices.sh
#
# 出力: packaging/dist/THIRD-PARTY-NOTICES.txt

set -e
script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
root=$(dirname -- "$script_dir")
out_dir="$root/packaging/dist"
out="$out_dir/THIRD-PARTY-NOTICES.txt"
mkdir -p "$out_dir"

if ! command -v cargo-about >/dev/null 2>&1 && ! cargo about --version >/dev/null 2>&1; then
    echo "cargo-about が必要です: cargo install cargo-about" >&2
    exit 1
fi

{
    echo "PeerCove — 第三者ソフトウェアのライセンス謝辞"
    echo
    echo "PeerCove 本体のライセンスは MIT OR Apache-2.0 です(LICENSE-MIT / LICENSE-APACHE)。"
    echo "以下は同梱・利用している第三者ソフトウェアのライセンス表示です。"
    echo "このファイルは packaging/make-notices.sh により自動生成されます。"
    echo
} > "$out"

echo "==> Rust 依存(CLI / デーモン / コア)"
cargo about generate "$root/about.hbs" \
    --manifest-path "$root/Cargo.toml" \
    --config "$root/about.toml" >> "$out"

echo "==> Rust 依存(デスクトップ UI / Tauri)"
cargo about generate "$root/about.hbs" \
    --manifest-path "$root/apps/peercove-ui/src-tauri/Cargo.toml" \
    --config "$root/about.toml" >> "$out"

# --- npm(フロントエンド)依存 ---
nm="$root/apps/peercove-ui/node_modules"
if [ -d "$nm" ] && command -v node >/dev/null 2>&1; then
    echo "==> npm 依存(フロントエンド)"
    node "$script_dir/collect-npm-licenses.mjs" "$nm" >> "$out"
else
    echo "!! npm 依存をスキップ(node_modules 無し or node 無し)。npm ci 後に再実行してください" >&2
    echo "" >> "$out"
    echo "[npm 依存のライセンスは未収集。node_modules を用意して再生成してください]" >> "$out"
fi

echo "作成しました: $out"
