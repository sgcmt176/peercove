#!/bin/sh
# PeerCove deb: 削除・パージ後に systemd の状態を再読込して、消えた unit の
# 参照を片付ける(M2-G7b)。unit ファイル自体は dpkg が削除済み。
set -e

if [ "$1" = "remove" ] || [ "$1" = "purge" ]; then
    if command -v systemctl >/dev/null 2>&1; then
        systemctl daemon-reload || true
    fi
fi

exit 0
