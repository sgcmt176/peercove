//! 内蔵 DNS リゾルバ(ADR-0011 §3、M3-1)。
//!
//! `*.peercove.internal` の A レコードだけを返す最小の UDP DNS サーバ。
//! 情報源は全ネットワークのゾーンを合算した共有テーブル([`SharedZones`])で、
//! supervisor が台帳の更新に合わせて書き換える。
//!
//! 方針(ADR-0011):
//! - A クエリ: ゾーンにあれば応答(TTL 30 秒)、無ければ NXDOMAIN
//! - A 以外のタイプ: 名前があれば NOERROR/NODATA、無ければ NXDOMAIN
//! - 再帰なし(RA=0)。スプリット DNS でこのサフィックスしか来ない前提
//! - hickory 等の crate は使わず自前(質問 1 問のパースと応答組み立てだけ)

use std::collections::HashMap;
use std::net::Ipv4Addr;
use std::sync::{Arc, RwLock};

use tokio::net::UdpSocket;

use peercove_core::dns::{zone_for, CnameRecord, DnsRecord, DNS_SUFFIX, DNS_TTL_SECS};
use peercove_core::names::is_custom_dns_name;
use peercove_core::proto::LedgerEntry;

/// CNAME の解決対象(ADR-0025)。`ip` が埋まっていればフラット化して A で返し、
/// 無ければ CNAME RR で返す。
#[derive(Debug, Clone)]
pub struct CnameTarget {
    pub target: String,
    pub ip: Option<Ipv4Addr>,
}

/// 合算済みのゾーン表(A + CNAME、ADR-0025)。fqdn は小文字・末尾ドットなし。
#[derive(Debug, Default, Clone)]
pub struct Zone {
    /// fqdn → IPv4(A レコード)
    pub a: HashMap<String, Ipv4Addr>,
    /// fqdn → 別名(CNAME レコード。フラット化 IP つき)
    pub cname: HashMap<String, CnameTarget>,
}

/// 全ネットワーク合算のゾーン共有テーブル。
pub type SharedZones = Arc<RwLock<Zone>>;

/// 1 ネットワーク分のゾーン素材(ネットワーク名, 台帳, A レコード, CNAME)。
/// merge_zones と daemon の refresh_zones が共有する(ADR-0025)。
pub type NetworkZoneData = (String, Vec<LedgerEntry>, Vec<DnsRecord>, Vec<CnameRecord>);

/// クエリ 1 個の最大サイズ(RFC 1035 の UDP メッセージ上限)。
const MAX_PACKET: usize = 512;

// DNS RCODE
const RCODE_NOERROR: u8 = 0;
const RCODE_FORMERR: u8 = 1;
const RCODE_NXDOMAIN: u8 = 3;
const RCODE_NOTIMP: u8 = 4;

const TYPE_A: u16 = 1;
const TYPE_CNAME: u16 = 5;
const CLASS_IN: u16 = 1;

/// 全ネットワークのゾーンを 1 つの表に合算する(fqdn → IP)。
///
/// 同名ネットワークが複数あると fqdn が衝突しうる(後勝ち)。M3-0a の join が
/// ディレクトリを分けても network_name までは変えないため起こりうるが、
/// 名前解決が片方に寄るだけで通信は壊れない(debug ログに残す)。
pub fn merge_zones(networks: &[NetworkZoneData]) -> Zone {
    let mut zone = Zone::default();
    for (network, ledger, custom, cnames) in networks {
        for entry in zone_for(network, ledger, custom) {
            if let Some(old) = zone.a.insert(entry.fqdn.clone(), entry.ip) {
                if old != entry.ip {
                    tracing::debug!(
                        "DNS 名 {} が複数ネットワークで衝突しています({old} → {})",
                        entry.fqdn,
                        entry.ip
                    );
                }
            }
        }
        for record in cnames {
            if !is_custom_dns_name(&record.name) {
                continue; // 不正ラベルは無視(配布側の検証をすり抜けても壊さない)
            }
            let fqdn = format!("{}.{network}.{DNS_SUFFIX}", record.name);
            // A レコードのある名前は A を優先(CNAME と A は共存しない — RFC 1912)
            if zone.a.contains_key(&fqdn) {
                continue;
            }
            zone.cname.insert(
                fqdn,
                CnameTarget {
                    target: record.target.clone(),
                    ip: record.resolved_ip,
                },
            );
        }
    }
    zone
}

/// トンネル用の待受け(デーモンがトンネルごとに 1 タスク起動する — M3-1b)。
///
/// トンネル作成直後は Windows がアドレスを数秒「準備中」として扱い bind に
/// 失敗するため、成功するまでリトライする(control.rs と同じパターン)。
/// 53 番が他ソフトに取られている場合もここでリトライし続けるだけで、
/// トンネル自体には影響しない(最初の失敗だけ警告する)。
pub async fn run_for_tunnel(bind_ip: Ipv4Addr, zones: SharedZones) {
    let mut logged = false;
    let socket = loop {
        match UdpSocket::bind((bind_ip, 53)).await {
            Ok(socket) => break socket,
            Err(e) => {
                if !logged {
                    tracing::info!("内蔵 DNS の待受け待ち({bind_ip}:53): {e}");
                    logged = true;
                }
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            }
        }
    };
    tracing::info!("内蔵 DNS を {bind_ip}:53 で待受けます");
    serve(socket, zones).await;
}

/// 受信ループ。ソケットは呼び出し側が bind する(テストはエフェメラルポート、
/// 本番はトンネル IP の :53)。
pub async fn serve(socket: UdpSocket, zones: SharedZones) {
    let mut buf = [0u8; MAX_PACKET];
    loop {
        let (len, from) = match socket.recv_from(&mut buf).await {
            Ok(received) => received,
            Err(e) => {
                tracing::debug!("DNS 受信エラー: {e}");
                continue;
            }
        };
        let response = {
            let zone = zones.read().unwrap();
            respond(&buf[..len], &zone)
        };
        if let Some(response) = response {
            if let Err(e) = socket.send_to(&response, from).await {
                tracing::debug!("DNS 応答の送信に失敗: {e}");
            }
        }
    }
}

/// 1 段のワイルドカード検索(先頭ラベルを `*` に置換して引く — ADR-0024/0025)。
fn wildcard_lookup<'a, T>(map: &'a HashMap<String, T>, name: &str) -> Option<&'a T> {
    name.split_once('.')
        .and_then(|(_, rest)| map.get(&format!("*.{rest}")))
}

/// ドメイン名を DNS ワイヤ形式(長さ + ラベル … + ルート 0)へエンコードする。
fn encode_name(name: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(name.len() + 2);
    for label in name.split('.') {
        if label.is_empty() {
            continue;
        }
        let bytes = label.as_bytes();
        let take = bytes.len().min(63);
        out.push(take as u8);
        out.extend_from_slice(&bytes[..take]);
    }
    out.push(0); // ルートラベル
    out
}

/// 1 クエリに対する応答バイト列を組み立てる(純関数 — テストの主対象)。
/// ヘッダすら読めないパケットは `None`(黙って捨てる)。
pub fn respond(query: &[u8], zone: &Zone) -> Option<Vec<u8>> {
    if query.len() < 12 {
        return None;
    }
    let id = [query[0], query[1]];
    let flags1 = query[2];
    if flags1 & 0x80 != 0 {
        return None; // QR=1(応答)には応答しない(ループ防止)
    }
    let opcode = (flags1 >> 3) & 0x0F;
    let rd = flags1 & 0x01;
    let qdcount = u16::from_be_bytes([query[4], query[5]]);

    // 応答ヘッダの共通部。QR=1, opcode 引き継ぎ, AA=1, RD 引き継ぎ, RA=0
    let header = |rcode: u8, ancount: u16, echo_question: bool| -> Vec<u8> {
        let mut out = Vec::with_capacity(64);
        out.extend_from_slice(&id);
        out.push(0x80 | (opcode << 3) | 0x04 | rd); // QR|opcode|AA|RD
        out.push(rcode);
        out.extend_from_slice(&(u16::from(echo_question)).to_be_bytes()); // QDCOUNT
        out.extend_from_slice(&ancount.to_be_bytes()); // ANCOUNT
        out.extend_from_slice(&[0, 0, 0, 0]); // NSCOUNT, ARCOUNT
        out
    };

    if opcode != 0 {
        return Some(header(RCODE_NOTIMP, 0, false));
    }
    if qdcount != 1 {
        return Some(header(RCODE_FORMERR, 0, false));
    }

    // 質問セクション(QNAME + QTYPE + QCLASS)をパース
    let Some((name, qtype, qclass, question_end)) = parse_question(query) else {
        return Some(header(RCODE_FORMERR, 0, false));
    };
    let question = &query[12..question_end];

    // A: 完全一致 → 無ければ 1 段のワイルドカード(先頭ラベルを `*` に置換)。
    // 例: `foo.app.net.peercove.internal` は `*.app.net.peercove.internal` に一致(ADR-0024)
    let a_ip = zone
        .a
        .get(&name)
        .copied()
        .or_else(|| wildcard_lookup(&zone.a, &name).copied());
    if let Some(ip) = a_ip {
        // 名前は存在する。A/IN のときだけ答えを返す(それ以外は NODATA)
        let answer = (qtype == TYPE_A && qclass == CLASS_IN).then_some(ip);
        let mut out = header(RCODE_NOERROR, u16::from(answer.is_some()), true);
        out.extend_from_slice(question);
        if let Some(ip) = answer {
            out.extend_from_slice(&[0xC0, 0x0C]); // 質問の名前への圧縮ポインタ
            out.extend_from_slice(&TYPE_A.to_be_bytes());
            out.extend_from_slice(&CLASS_IN.to_be_bytes());
            out.extend_from_slice(&DNS_TTL_SECS.to_be_bytes());
            out.extend_from_slice(&4u16.to_be_bytes()); // RDLENGTH
            out.extend_from_slice(&ip.octets());
        }
        return Some(out);
    }

    // CNAME: 完全一致 → 1 段ワイルドカード(ADR-0025)。
    let cname = zone
        .cname
        .get(&name)
        .cloned()
        .or_else(|| wildcard_lookup(&zone.cname, &name).cloned());
    if let Some(cn) = cname {
        // フラット化済み(転送先を IPv4 へ解決済み)なら A で返す。スプリット DNS
        // でもクライアントが直接使えるため、外部ドメインでも到達できる
        if let Some(ip) = cn.ip {
            tracing::debug!("DNS CNAME {name} → {} (A {ip})", cn.target);
            let answer = (qtype == TYPE_A && qclass == CLASS_IN).then_some(ip);
            let mut out = header(RCODE_NOERROR, u16::from(answer.is_some()), true);
            out.extend_from_slice(question);
            if let Some(ip) = answer {
                out.extend_from_slice(&[0xC0, 0x0C]);
                out.extend_from_slice(&TYPE_A.to_be_bytes());
                out.extend_from_slice(&CLASS_IN.to_be_bytes());
                out.extend_from_slice(&DNS_TTL_SECS.to_be_bytes());
                out.extend_from_slice(&4u16.to_be_bytes());
                out.extend_from_slice(&ip.octets());
            }
            return Some(out);
        }
        // 未解決(外部解決前・in-zone 先など)は CNAME RR で返し、追跡させる
        tracing::debug!("DNS CNAME {name} → {} (未解決)", cn.target);
        let rdata = (qclass == CLASS_IN).then(|| encode_name(&cn.target));
        let mut out = header(RCODE_NOERROR, u16::from(rdata.is_some()), true);
        out.extend_from_slice(question);
        if let Some(rdata) = rdata {
            out.extend_from_slice(&[0xC0, 0x0C]); // 質問の名前への圧縮ポインタ
            out.extend_from_slice(&TYPE_CNAME.to_be_bytes());
            out.extend_from_slice(&CLASS_IN.to_be_bytes());
            out.extend_from_slice(&DNS_TTL_SECS.to_be_bytes());
            out.extend_from_slice(&(rdata.len() as u16).to_be_bytes()); // RDLENGTH
            out.extend_from_slice(&rdata);
        }
        return Some(out);
    }

    // A も CNAME も無い → NXDOMAIN
    tracing::debug!("DNS NXDOMAIN {name}(qtype={qtype})");
    let mut out = header(RCODE_NXDOMAIN, 0, true);
    out.extend_from_slice(question);
    Some(out)
}

/// 質問セクションをパースして (小文字 FQDN, QTYPE, QCLASS, 終端オフセット) を返す。
/// 圧縮ポインタは質問には現れない前提(RFC 上も慣習上も生のラベル列)。
fn parse_question(query: &[u8]) -> Option<(String, u16, u16, usize)> {
    let mut pos = 12;
    let mut name = String::new();
    loop {
        let len = *query.get(pos)? as usize;
        pos += 1;
        if len == 0 {
            break;
        }
        if len > 63 || pos + len > query.len() {
            return None;
        }
        if !name.is_empty() {
            name.push('.');
        }
        for &b in &query[pos..pos + len] {
            name.push(b.to_ascii_lowercase() as char);
        }
        pos += len;
        if name.len() > 253 {
            return None;
        }
    }
    let qtype = u16::from_be_bytes([*query.get(pos)?, *query.get(pos + 1)?]);
    let qclass = u16::from_be_bytes([*query.get(pos + 2)?, *query.get(pos + 3)?]);
    Some((name, qtype, qclass, pos + 4))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// テスト用のクエリを組み立てる。
    fn build_query(id: u16, name: &str, qtype: u16) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&id.to_be_bytes());
        out.extend_from_slice(&[0x01, 0x00]); // RD=1
        out.extend_from_slice(&[0, 1, 0, 0, 0, 0, 0, 0]); // QDCOUNT=1
        for label in name.split('.') {
            out.push(label.len() as u8);
            out.extend_from_slice(label.as_bytes());
        }
        out.push(0);
        out.extend_from_slice(&qtype.to_be_bytes());
        out.extend_from_slice(&CLASS_IN.to_be_bytes());
        out
    }

    fn zones() -> Zone {
        let mut zone = Zone::default();
        zone.a.insert(
            "alice.home.peercove.internal".to_string(),
            "10.68.1.2".parse().unwrap(),
        );
        zone
    }

    fn rcode(response: &[u8]) -> u8 {
        response[3] & 0x0F
    }

    fn ancount(response: &[u8]) -> u16 {
        u16::from_be_bytes([response[6], response[7]])
    }

    #[test]
    fn merge_zones_combines_networks() {
        use peercove_core::keys::PrivateKey;
        let entry = |name: &str, ip: &str| LedgerEntry {
            name: Some(name.to_string()),
            dns_name: None,
            ip: ip.parse().unwrap(),
            public_key: PrivateKey::generate().public_key(),
            app_version: None,
            platform: None,
            capabilities: vec![],
            member_id: None,
            invite_status: None,
            invite_expires_at: None,
            online: true,
            is_host: false,
            endpoint: None,
            endpoint_age_secs: None,
            subnets: vec![],
            blocked: false,
            force_relay: false,
            acl_rule_id: None,
        };
        let networks = vec![
            (
                "game".to_string(),
                vec![entry("alice", "10.1.0.2")],
                vec![DnsRecord {
                    name: "nas".to_string(),
                    ip: "10.1.0.50".parse().unwrap(),
                    scheme: None,
                    port: None,
                    health: None,
                }],
                vec![CnameRecord {
                    name: "docs".to_string(),
                    target: "example.com".to_string(),
                    resolved_ip: None,
                    scheme: None,
                    port: None,
                    health: None,
                }],
            ),
            (
                "family".to_string(),
                vec![entry("alice", "10.2.0.2")],
                vec![],
                vec![],
            ),
        ];
        let merged = merge_zones(&networks);
        assert_eq!(merged.a.len(), 3);
        assert_eq!(
            merged.a["alice.game.peercove.internal"].to_string(),
            "10.1.0.2"
        );
        assert_eq!(
            merged.a["alice.family.peercove.internal"].to_string(),
            "10.2.0.2",
            "同じ表示名でもネットワーク階層で分離される"
        );
        assert_eq!(
            merged.a["nas.game.peercove.internal"].to_string(),
            "10.1.0.50"
        );
        assert_eq!(
            merged.cname["docs.game.peercove.internal"].target, "example.com",
            "CNAME は別枠で合算される"
        );
    }

    #[test]
    fn answers_known_a_query_case_insensitively() {
        let query = build_query(0x1234, "Alice.HOME.peercove.internal", TYPE_A);
        let response = respond(&query, &zones()).unwrap();

        assert_eq!(&response[0..2], &[0x12, 0x34], "ID を引き継ぐ");
        assert_eq!(response[2] & 0x80, 0x80, "QR=1");
        assert_eq!(rcode(&response), RCODE_NOERROR);
        assert_eq!(ancount(&response), 1);
        // 末尾 4 バイトが A レコードの IP
        assert_eq!(&response[response.len() - 4..], &[10, 68, 1, 2]);
        // TTL(RDATA の 6 バイト前から 4 バイト)
        let ttl_at = response.len() - 10;
        assert_eq!(
            u32::from_be_bytes(response[ttl_at..ttl_at + 4].try_into().unwrap()),
            DNS_TTL_SECS
        );
    }

    #[test]
    fn unknown_name_is_nxdomain() {
        let query = build_query(1, "nobody.home.peercove.internal", TYPE_A);
        let response = respond(&query, &zones()).unwrap();
        assert_eq!(rcode(&response), RCODE_NXDOMAIN);
        assert_eq!(ancount(&response), 0);
    }

    #[test]
    fn wildcard_matches_one_label_deep() {
        let mut map = zones();
        map.a.insert(
            "*.app.home.peercove.internal".to_string(),
            "10.68.9.9".parse().unwrap(),
        );
        // 先頭 1 ラベルはワイルドカードに一致
        let query = build_query(1, "foo.app.home.peercove.internal", TYPE_A);
        let response = respond(&query, &map).unwrap();
        assert_eq!(rcode(&response), RCODE_NOERROR);
        assert_eq!(ancount(&response), 1);
        assert_eq!(&response[response.len() - 4..], &[10, 68, 9, 9]);

        // 完全一致が優先される(ワイルドカードに食われない)
        map.a.insert(
            "exact.app.home.peercove.internal".to_string(),
            "10.68.9.1".parse().unwrap(),
        );
        let query = build_query(1, "exact.app.home.peercove.internal", TYPE_A);
        let response = respond(&query, &map).unwrap();
        assert_eq!(&response[response.len() - 4..], &[10, 68, 9, 1]);

        // 2 段深いラベルはワイルドカード(1 段)に一致しない
        let query = build_query(1, "a.b.app.home.peercove.internal", TYPE_A);
        let response = respond(&query, &map).unwrap();
        assert_eq!(rcode(&response), RCODE_NXDOMAIN);
    }

    #[test]
    fn cname_returns_cname_rr_and_a_wins() {
        let cn = |target: &str, ip: Option<Ipv4Addr>| CnameTarget {
            target: target.to_string(),
            ip,
        };
        let mut map = zones();
        // 未解決の CNAME は CNAME RR で返す
        map.cname.insert(
            "docs.home.peercove.internal".to_string(),
            cn("example.com", None),
        );
        let query = build_query(1, "docs.home.peercove.internal", TYPE_A);
        let response = respond(&query, &map).unwrap();
        assert_eq!(rcode(&response), RCODE_NOERROR);
        assert_eq!(ancount(&response), 1);
        // RDATA に example.com のエンコード(7 example 3 com 0)が含まれる
        let rdata = [
            7, b'e', b'x', b'a', b'm', b'p', b'l', b'e', 3, b'c', b'o', b'm', 0,
        ];
        assert!(
            response.windows(rdata.len()).any(|w| w == rdata),
            "CNAME の RDATA にターゲットのエンコードが含まれる"
        );

        // フラット化済み(解決 IP つき)は A レコードで返す(ADR-0025)
        map.cname.insert(
            "flat.home.peercove.internal".to_string(),
            cn("example.com", Some("93.184.216.34".parse().unwrap())),
        );
        let query = build_query(1, "flat.home.peercove.internal", TYPE_A);
        let response = respond(&query, &map).unwrap();
        assert_eq!(ancount(&response), 1);
        assert_eq!(
            &response[response.len() - 4..],
            &[93, 184, 216, 34],
            "A で返る"
        );

        // ワイルドカード CNAME も 1 段一致する
        map.cname.insert(
            "*.svc.home.peercove.internal".to_string(),
            cn("api.example.com", None),
        );
        let query = build_query(1, "any.svc.home.peercove.internal", TYPE_A);
        let response = respond(&query, &map).unwrap();
        assert_eq!(ancount(&response), 1);

        // 同名に A と CNAME があれば A が優先(RFC 1912)
        map.cname.insert(
            "alice.home.peercove.internal".to_string(),
            cn("somewhere.example", None),
        );
        let query = build_query(1, "alice.home.peercove.internal", TYPE_A);
        let response = respond(&query, &map).unwrap();
        assert_eq!(&response[response.len() - 4..], &[10, 68, 1, 2], "A が勝つ");
    }

    #[test]
    fn known_name_with_other_type_is_nodata() {
        let aaaa = 28;
        let query = build_query(1, "alice.home.peercove.internal", aaaa);
        let response = respond(&query, &zones()).unwrap();
        assert_eq!(
            rcode(&response),
            RCODE_NOERROR,
            "名前はあるので NXDOMAIN ではない"
        );
        assert_eq!(ancount(&response), 0, "答えは無い(NODATA)");
    }

    #[test]
    fn rejects_garbage_and_non_queries() {
        assert!(respond(&[0; 4], &zones()).is_none(), "ヘッダ未満は捨てる");

        // QR=1(応答パケット)には応答しない
        let mut echo = build_query(1, "alice.home.peercove.internal", TYPE_A);
        echo[2] |= 0x80;
        assert!(respond(&echo, &zones()).is_none());

        // 質問が壊れている(ラベル長がパケットを超える)→ FORMERR
        let mut broken = build_query(1, "alice.home.peercove.internal", TYPE_A);
        broken.truncate(14);
        broken[12] = 63;
        let response = respond(&broken, &zones()).unwrap();
        assert_eq!(rcode(&response), RCODE_FORMERR);

        // opcode != QUERY → NOTIMP
        let mut status = build_query(1, "alice.home.peercove.internal", TYPE_A);
        status[2] = 0x10; // opcode=2 (STATUS)
        let response = respond(&status, &zones()).unwrap();
        assert_eq!(rcode(&response), RCODE_NOTIMP);
    }

    /// UDP で実際に往復する(エフェメラルポート = 特権不要)。
    #[tokio::test]
    async fn serves_over_udp() {
        let server_socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let server_addr = server_socket.local_addr().unwrap();
        let zones: SharedZones = Arc::new(RwLock::new(zones()));
        let server = tokio::spawn(serve(server_socket, Arc::clone(&zones)));

        let client = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        client
            .send_to(
                &build_query(7, "alice.home.peercove.internal", TYPE_A),
                server_addr,
            )
            .await
            .unwrap();
        let mut buf = [0u8; MAX_PACKET];
        let (len, _) = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            client.recv_from(&mut buf),
        )
        .await
        .unwrap()
        .unwrap();
        let response = &buf[..len];
        assert_eq!(rcode(response), RCODE_NOERROR);
        assert_eq!(&response[len - 4..], &[10, 68, 1, 2]);

        // ゾーンの動的更新が次のクエリに反映される
        zones.write().unwrap().a.insert(
            "bob.home.peercove.internal".to_string(),
            "10.68.1.3".parse().unwrap(),
        );
        client
            .send_to(
                &build_query(8, "bob.home.peercove.internal", TYPE_A),
                server_addr,
            )
            .await
            .unwrap();
        let (len, _) = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            client.recv_from(&mut buf),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(&buf[len - 4..len], &[10, 68, 1, 3]);

        server.abort();
    }
}
