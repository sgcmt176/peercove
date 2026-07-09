#!/bin/sh
# PeerCove deb: 削除の前(remove)にサービスを停止・無効化する(M2-G7b)。
# 停止でトンネルのクリーンアップが走る。更新(upgrade)時は止めない
# (postinst の restart が新バイナリで入れ替える)。
set -e

if [ "$1" = "remove" ]; then
    if command -v systemctl >/dev/null 2>&1; then
        systemctl disable --now peercove-daemon.service >/dev/null 2>&1 || true
    fi
fi

exit 0
