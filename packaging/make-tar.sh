#!/bin/sh
# PeerCove ポータブル版(Linux tar.gz)を組み立てる(M2-G7b、上級者向け配布)。
#
# 前提(先に済ませておく):
#   cargo build --release -p peercove-cli
#   cd apps/peercove-ui && npm install && npm run tauri build   (UI 実行ファイルを作る)
#
# 使い方(リポジトリのどこからでも):
#   sh packaging/make-tar.sh
#
# 出力: packaging/dist/PeerCove-portable-linux-x64.tar.gz
#
# Linux は wintun 不要(カーネル WireGuard)なので DLL/ライセンスは同梱しない。

set -e
script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
root=$(dirname -- "$script_dir")   # packaging の 1 つ上 = リポジトリルート

require() {
    if [ ! -e "$1" ]; then
        echo "見つかりません: $1" >&2
        echo "  → $2" >&2
        exit 1
    fi
}

daemon="$root/target/release/peercove"
require "$daemon" "先に 'cargo build --release -p peercove-cli' を実行してください"
require "$root/packaging/README-portable.md" "リポジトリの packaging にあるはず"

# UI 実行ファイル名は cargo/tauri で異なりうるので候補を順に探す
ui=""
for cand in \
    "$root/apps/peercove-ui/src-tauri/target/release/peer-cove" \
    "$root/apps/peercove-ui/src-tauri/target/release/PeerCove" \
    "$root/apps/peercove-ui/src-tauri/target/release/peercove-ui"; do
    if [ -x "$cand" ]; then ui="$cand"; break; fi
done
if [ -z "$ui" ]; then
    echo "UI 実行ファイルが見つかりません。apps/peercove-ui で 'npm run tauri build' を実行してください" >&2
    exit 1
fi

stage="$root/packaging/dist/stage-linux"
out="$root/packaging/dist/PeerCove-portable-linux-x64.tar.gz"
rm -rf "$stage"
mkdir -p "$stage/PeerCove"

cp "$daemon" "$stage/PeerCove/peercove"
# UI は MSI / Windows ポータブルと同じく peercove-ui にする(命名を揃える)。
cp "$ui" "$stage/PeerCove/peercove-ui"
cp "$root/packaging/README-portable.md" "$stage/PeerCove/README.md"
chmod +x "$stage/PeerCove/peercove" "$stage/PeerCove/peercove-ui"

# 第三者ライセンス謝辞(packaging/make-notices.sh で生成)。配布物には必須。
notices="$root/packaging/dist/THIRD-PARTY-NOTICES.txt"
if [ -f "$notices" ]; then
    cp "$notices" "$stage/PeerCove/THIRD-PARTY-NOTICES.txt"
else
    echo "!! THIRD-PARTY-NOTICES.txt がありません。配布前に 'sh packaging/make-notices.sh' を実行してください" >&2
fi

mkdir -p "$root/packaging/dist"
rm -f "$out"
tar -czf "$out" -C "$stage" PeerCove
rm -rf "$stage"

echo "作成しました: $out"
echo "内容: PeerCove/{peercove, peercove-ui, README.md, THIRD-PARTY-NOTICES.txt}"
