#!/bin/sh
# PeerCove deb: インストール後(configure)に systemd サービスを有効化・起動する
# (M2-G7b、ADR-0010)。systemd unit は /usr/lib/systemd/system/peercove-daemon.service
# に配置済み。ExecStart は /usr/bin/peercove daemon run。
#
# Tauri の deb バンドラは生成する postinst にこの内容を差し込む。$1 は dpkg の
# 引数(新規/更新とも "configure")。systemctl が無い環境でも壊さない。
set -e

if [ "$1" = "configure" ]; then
    if command -v systemctl >/dev/null 2>&1; then
        systemctl daemon-reload || true
        systemctl enable peercove-daemon.service >/dev/null 2>&1 || true
        # 新規は開始、更新は新バイナリで再起動(restart は停止中でも開始する)
        systemctl restart peercove-daemon.service || true
    fi
    # peercove:// スキームのハンドラ登録を確実にする(M3-5)。
    # 多くのディストリでは dpkg トリガが自動で走るが、無い環境に備えて明示する
    if command -v update-desktop-database >/dev/null 2>&1; then
        update-desktop-database /usr/share/applications || true
    fi
fi

exit 0
