#!/bin/sh
# PeerCove deb: インストール後(configure)に systemd サービスを有効化・起動する
# (M2-G7b、ADR-0010)。systemd unit は /usr/lib/systemd/system/peercove-daemon.service
# に配置済み。ExecStart は /usr/bin/peercove daemon run。
#
# Tauri の deb バンドラは生成する postinst にこの内容を差し込む。$1 は dpkg の
# 引数(新規/更新とも "configure")。systemctl が無い環境でも壊さない。
set -e

if [ "$1" = "configure" ]; then
    # IPC を操作してよいユーザー(所有者)の uid を決めて、サービスへ渡す
    # (ADR-0038)。sudo apt install なら SUDO_UID、無ければ最初の一般ユーザー。
    owner_uid="${SUDO_UID:-}"
    if [ -z "$owner_uid" ]; then
        owner_uid=$(getent passwd | awk -F: '$3 >= 1000 && $3 < 65534 { print $3; exit }')
    fi
    if [ -n "$owner_uid" ] && [ "$owner_uid" != "0" ]; then
        mkdir -p /etc/default
        printf 'PEERCOVE_OWNER_UID=%s\n' "$owner_uid" > /etc/default/peercove-daemon
    fi
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
