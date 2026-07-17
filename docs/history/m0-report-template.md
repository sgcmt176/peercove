# M0 計測レポート(G-7)

計測手順は [verification.md](../verification.md) の「検証手順(G-7: 計測レポート)」を参照。

## 1. 計測環境

| 項目 | 内容 |
|---|---|
| 計測日 | 2026-07-08 |
| Host | Windows 11(自宅ルーター 192.168.0.1 配下、UPnP 有効)ユーザー空間実装(wintun + boringtun) |
| Member A | Ubuntu 22.04(VirtualBox ブリッジ 192.168.0.14、Host と同一 LAN)カーネル WG |
| Member B | Ubuntu(VirtualBox、100.100.42.3)。別途スマホテザリング経由の外部接続も確認 |
| 備考 | Member VM の Tailscale は停止(README トラブルシューティング参照)。Host は ICMP 受信許可を追加済み |

## 2. ゴール達成状況

| ゴール | 結果 | 備考 |
|---|---|---|
| G-1 トンネル作成・破棄 | ✅ | Windows / Ubuntu 両方。非管理者・dll 欠落時のエラーも確認 |
| G-2 1対1 ping | ✅ | 双方向 0% loss |
| G-3 ハブ&スポーク ping | ✅ | A ↔ B 双方向 0% loss(Host のデバイス内リレー経由) |
| G-4 TCP(curl) | ✅ | python http.server ↔ curl |
| G-5 UDP(udp-ping) | ✅ | 送信元がトンネル内 IP で届くことを確認 |
| G-6 UPnP 開放 | ✅ | 外部エンドポイント: 203.0.113.5:51820(グローバル IP)。**テザリング(別 NAT)の外部メンバーから handshake・疎通成功**。UPnP 無効相当時の案内メッセージも確認 |

## 3. RTT(ping 100 回の min/avg/max/mdev)

| 経路 | min (ms) | avg (ms) | max (ms) | mdev (ms) | 損失 |
|---|---|---|---|---|---|
| 【参考】A → Host 物理 LAN(トンネル外) | 0.294 | 0.402 | 0.947 | 0.103 | 0% |
| A → Host(100.100.42.1) | 0.918 | 1.292 | 2.611 | 0.267 | 0% |
| A → B(100.100.42.3、**リレー経由**) | 1.452 | 2.249 | 6.803 | 0.820 | 0% |

- トンネル化のオーバーヘッド(A→Host − 物理 LAN): **約 0.89 ms**
- リレーのオーバーヘッド(A→B avg − A→Host avg): **約 0.96 ms**(1 ホップ分)

## 4. スループット(iperf3、10 秒 × 1 回)

サーバー: Member A(iperf3 -s)/ クライアント: Member B(すべて **Host リレー経由**)

| 経路・条件 | 実測値 | 備考 |
|---|---|---|
| B → A TCP(リレー経由) | **30.1 Mbps**(receiver 29.8) | Retr 28 |
| A → B TCP(`-R` 逆方向) | **27.4 Mbps**(receiver 27.1) | Retr 63 |
| B → A UDP `-b 100M` | 送信 63.4 Mbps / **受信 40.0 Mbps** | 損失 37%、ジッター 0.318 ms |

- 計測は各 1 回(テンプレートの 3 回中央値は省略)
- UDP はオファー 100 Mbps に対しリレー経路の実効が ~40 Mbps で飽和し、超過分が損失
  として現れている(TCP は輻輳制御が効くため損失小)

## 5. udp-ping(トンネル内 UDP RTT)

今回のセッションでは統計値未取得(G-5 検証時に B→A で損失 0% を確認済み)。

## 6. 所感・気付き

- 同一 LAN + VM 構成では、トンネル化 +0.9ms・リレー +1.0ms 程度で、ping/TCP/UDP
  とも安定。ゲーム用途(仮想 IP:ポート直接指定)の成立を確認できた
- スループット ~30 Mbps(TCP)は VM 環境 + Windows ユーザー空間実装(boringtun
  シングルスレッドの暗号処理 + wintun 往復)がボトルネックと推定。実機・実 NIC
  での再計測と、必要ならマルチスレッド化・wireguard-nt 比較は M1 以降の課題
- 検証で得た運用知見: Tailscale 併用時の 100.64.0.0/10 衝突(decisions.md 参照)、
  同一 LAN メンバーは LAN エンドポイント必須(hairpin NAT 非対応ルーター)、
  Host 再起動後の再ハンドシェイクに ~15 秒
