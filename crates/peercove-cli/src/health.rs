//! DNS サービスのホスト主体ヘルスチェック(M3-14e、ADR-0033)。

use std::collections::{HashMap, HashSet, VecDeque};
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use peercove_core::config::{Config, DnsRecordConfig, MemberRef};
use peercove_core::dns::{
    CnameRecord, DnsRecord, HealthCheckKind, ServiceHealth, ServiceHealthReason,
    ServiceHealthStatus, DNS_SUFFIX,
};
use peercove_core::proto::LedgerEntry;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

const CHECK_INTERVAL: Duration = Duration::from_secs(60);
const CHECK_TIMEOUT: Duration = Duration::from_secs(3);
const MAX_CONCURRENT: usize = 8;

pub type SharedHealth = Arc<Mutex<HealthMonitor>>;

#[derive(Default)]
pub struct HealthMonitor {
    results: HashMap<String, ServiceHealth>,
    last_started: Option<Instant>,
    running: bool,
    force: bool,
    /// 実行中の監視バッチ。トンネル停止時に abort して外部接続を打ち切る。
    batch: Option<tokio::task::JoinHandle<()>>,
    /// 直近(約5秒ごと更新)のオンライン・メンバー IP。プローブ中にメンバーが
    /// 落ちた場合、失敗を Unhealthy でなく Offline に落とすために参照する。
    online_members: HashSet<Ipv4Addr>,
}

#[derive(Clone)]
enum Target {
    Ip(Ipv4Addr),
    Name(String),
}

#[derive(Clone)]
struct Job {
    key: String,
    fqdn: String,
    target: Target,
    port: u16,
    kind: HealthCheckKind,
    path: String,
    expected_status: Option<u16>,
    /// member/under 参照レコードの対象メンバー IP。プローブ失敗時、このメンバーが
    /// 既にオフラインなら結果を Offline(unknown)へ落とす。
    gated_member_ip: Option<Ipv4Addr>,
}

impl HealthMonitor {
    /// 次の supervisor 周期で間隔を無視して再確認する。
    pub fn request_now(&mut self) {
        self.force = true;
    }

    /// トンネル停止時に実行中の監視バッチを打ち切り、外部への接続を残さない。
    pub fn stop(&mut self) {
        if let Some(batch) = self.batch.take() {
            batch.abort();
        }
        self.running = false;
    }

    /// 直近結果を配布用レコードへ付ける。未測定は明示的な unknown とする。
    pub fn enrich(&self, records: &mut [DnsRecord], cnames: &mut [CnameRecord]) {
        for record in records {
            record.health = Some(
                self.results
                    .get(&record.name)
                    .cloned()
                    .unwrap_or_else(|| ServiceHealth::unknown(ServiceHealthReason::NotChecked)),
            );
        }
        for record in cnames {
            record.health = Some(
                self.results
                    .get(&record.name)
                    .cloned()
                    .unwrap_or_else(|| ServiceHealth::unknown(ServiceHealthReason::NotChecked)),
            );
        }
    }
}

/// 必要なら監視バッチを開始する。ネットワーク I/O は別タスクで実行し、
/// supervisor を待たせない。ワーカーは最大8個で、レコード数だけタスクを作らない。
pub fn schedule(shared: &SharedHealth, config: &Config, ledger: &[LedgerEntry]) {
    {
        let mut monitor = shared.lock().unwrap();
        // オンライン集合は毎周期(約5秒)更新する。バッチ実行中でも最新化して、
        // プローブ中に落ちたメンバーを検知できるようにする。
        monitor.online_members = ledger
            .iter()
            .filter(|entry| entry.online)
            .map(|entry| entry.ip)
            .collect();
        let due = monitor
            .last_started
            .is_none_or(|last| last.elapsed() >= CHECK_INTERVAL);
        if monitor.running || (!monitor.force && !due) {
            return;
        }
        monitor.force = false;
        monitor.running = true;
        monitor.last_started = Some(Instant::now());
    }

    let (jobs, immediate) = build_jobs(config, ledger);
    let task_shared = Arc::clone(shared);
    let handle = tokio::spawn(async move {
        let queue = Arc::new(tokio::sync::Mutex::new(VecDeque::from(jobs)));
        let results = Arc::new(tokio::sync::Mutex::new(immediate));
        let workers = {
            let count = queue.lock().await.len().min(MAX_CONCURRENT);
            (0..count)
                .map(|_| {
                    let queue = Arc::clone(&queue);
                    let results = Arc::clone(&results);
                    let worker_shared = Arc::clone(&task_shared);
                    tokio::spawn(async move {
                        loop {
                            let Some(job) = queue.lock().await.pop_front() else {
                                break;
                            };
                            let key = job.key.clone();
                            let gated = job.gated_member_ip;
                            let mut result = check(job).await;
                            // プローブ中にメンバーが落ちた場合、接続失敗を「停止」でなく
                            // 「未確認(オフライン)」に落とす(ADR-0033)。
                            if result.status == ServiceHealthStatus::Unhealthy {
                                if let Some(ip) = gated {
                                    if !worker_shared.lock().unwrap().online_members.contains(&ip) {
                                        result = ServiceHealth {
                                            status: ServiceHealthStatus::Unknown,
                                            reason: ServiceHealthReason::Offline,
                                            checked_at_unix_ms: result.checked_at_unix_ms,
                                            response_ms: None,
                                            http_status: None,
                                        };
                                    }
                                }
                            }
                            results.lock().await.insert(key, result);
                        }
                    })
                })
                .collect::<Vec<_>>()
        };
        for worker in workers {
            let _ = worker.await;
        }
        let completed = results.lock().await.clone();
        let mut monitor = task_shared.lock().unwrap();
        monitor.results = completed;
        monitor.running = false;
        monitor.batch = None;
    });
    shared.lock().unwrap().batch = Some(handle);
}

fn build_jobs(
    config: &Config,
    ledger: &[LedgerEntry],
) -> (Vec<Job>, HashMap<String, ServiceHealth>) {
    let mut jobs = Vec::new();
    let mut immediate = HashMap::new();
    let network = config.network_name();
    let now = unix_ms();
    for record in &config.dns_records {
        let (key, target) = if let Some(cname) = &record.cname {
            (record_name(record, ledger), Target::Name(cname.clone()))
        } else {
            let resolved =
                peercove_core::dns::resolve_records(std::slice::from_ref(record), ledger);
            match resolved.first() {
                Some(resolved) => (resolved.name.clone(), Target::Ip(resolved.ip)),
                None => (record.name.clone(), Target::Ip(Ipv4Addr::UNSPECIFIED)),
            }
        };
        // 既定 ON は「内部(トンネル/LAN の私設 IP)を指す」レコードだけ。素の公開 IP は
        // 外部宛なので、明示的な health_check = true が無い限り自動確認しない(ADR-0033)。
        let internal_target = match &target {
            Target::Ip(ip) => is_internal_ip(*ip),
            Target::Name(_) => false,
        };
        let enabled_by_default = record.cname.is_none()
            && record.scheme.is_some()
            && record.port.is_some()
            && internal_target;
        let enabled = record.health_check.unwrap_or(enabled_by_default);

        if !enabled || (record.cname.is_some() && !record.health_external) {
            immediate.insert(key, disabled());
            continue;
        }
        let Some(port) = record.port else {
            immediate.insert(key, disabled());
            continue;
        };
        if referenced_offline(record, ledger) {
            immediate.insert(
                key,
                ServiceHealth {
                    status: ServiceHealthStatus::Unknown,
                    reason: ServiceHealthReason::Offline,
                    checked_at_unix_ms: Some(now),
                    response_ms: None,
                    http_status: None,
                },
            );
            continue;
        }
        let gated_member_ip = if record.member.is_some() || record.under.is_some() {
            match &target {
                Target::Ip(ip) => Some(*ip),
                Target::Name(_) => None,
            }
        } else {
            None
        };
        let fqdn = format!("{key}.{network}.{DNS_SUFFIX}");
        jobs.push(Job {
            key,
            fqdn,
            target,
            port,
            kind: record.health_kind.unwrap_or(HealthCheckKind::Tcp),
            path: record.health_path.clone().unwrap_or_else(|| "/".into()),
            expected_status: record.health_expect_status,
            gated_member_ip,
        });
    }
    (jobs, immediate)
}

/// トンネル/LAN で到達する私設アドレスか。公開 IP は外部宛とみなす。
fn is_internal_ip(ip: Ipv4Addr) -> bool {
    ip.is_private() || ip.is_loopback()
}

fn record_name(record: &DnsRecordConfig, ledger: &[LedgerEntry]) -> String {
    peercove_core::dns::resolve_cnames(std::slice::from_ref(record), ledger)
        .first()
        .map(|resolved| resolved.name.clone())
        .unwrap_or_else(|| record.name.clone())
}

fn referenced_offline(record: &DnsRecordConfig, ledger: &[LedgerEntry]) -> bool {
    [&record.member, &record.under]
        .into_iter()
        .flatten()
        .any(|reference| {
            ledger
                .iter()
                .find(|entry| match reference {
                    MemberRef::Host => entry.is_host,
                    MemberRef::Key(key) => entry.public_key == *key,
                })
                .is_none_or(|entry| !entry.online)
        })
}

async fn check(job: Job) -> ServiceHealth {
    check_with_timeout(job, CHECK_TIMEOUT).await
}

async fn check_with_timeout(job: Job, timeout: Duration) -> ServiceHealth {
    let started = Instant::now();
    let result = tokio::time::timeout(timeout, run_check(&job)).await;
    let response_ms = started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64;
    let checked_at = Some(unix_ms());
    match result {
        Err(_) => ServiceHealth {
            status: ServiceHealthStatus::Unhealthy,
            reason: ServiceHealthReason::Timeout,
            checked_at_unix_ms: checked_at,
            response_ms: Some(response_ms),
            http_status: None,
        },
        Ok(Err(CheckError::Resolution)) => ServiceHealth {
            status: ServiceHealthStatus::Unknown,
            reason: ServiceHealthReason::NameResolutionFailed,
            checked_at_unix_ms: checked_at,
            response_ms: Some(response_ms),
            http_status: None,
        },
        Ok(Err(CheckError::Connection)) => ServiceHealth {
            status: ServiceHealthStatus::Unhealthy,
            reason: ServiceHealthReason::ConnectionFailed,
            checked_at_unix_ms: checked_at,
            response_ms: Some(response_ms),
            http_status: None,
        },
        Ok(Ok(http_status)) => {
            let healthy = match (job.expected_status, http_status) {
                (Some(expected), Some(actual)) => expected == actual,
                (None, Some(actual)) => (200..400).contains(&actual),
                (_, None) => true,
            };
            ServiceHealth {
                status: if healthy {
                    ServiceHealthStatus::Healthy
                } else {
                    ServiceHealthStatus::Unhealthy
                },
                reason: if healthy {
                    ServiceHealthReason::NotChecked
                } else {
                    ServiceHealthReason::UnexpectedStatus
                },
                checked_at_unix_ms: checked_at,
                response_ms: Some(response_ms),
                http_status,
            }
        }
    }
}

enum CheckError {
    Resolution,
    Connection,
}

async fn run_check(job: &Job) -> Result<Option<u16>, CheckError> {
    let address = match &job.target {
        Target::Ip(ip) => SocketAddr::from((*ip, job.port)),
        Target::Name(name) => tokio::net::lookup_host((name.as_str(), job.port))
            .await
            .map_err(|_| CheckError::Resolution)?
            .next()
            .ok_or(CheckError::Resolution)?,
    };
    let mut stream = tokio::net::TcpStream::connect(address)
        .await
        .map_err(|_| CheckError::Connection)?;
    if job.kind == HealthCheckKind::Tcp {
        return Ok(None);
    }
    let request = format!(
        "HEAD {} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\r\n",
        job.path, job.fqdn
    );
    stream
        .write_all(request.as_bytes())
        .await
        .map_err(|_| CheckError::Connection)?;
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader
        .read_line(&mut line)
        .await
        .map_err(|_| CheckError::Connection)?;
    let status = line
        .split_whitespace()
        .nth(1)
        .and_then(|value| value.parse::<u16>().ok())
        .ok_or(CheckError::Connection)?;
    Ok(Some(status))
}

fn disabled() -> ServiceHealth {
    ServiceHealth {
        status: ServiceHealthStatus::Disabled,
        reason: ServiceHealthReason::Disabled,
        checked_at_unix_ms: None,
        response_ms: None,
        http_status: None,
    }
}

fn unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job(address: SocketAddr, kind: HealthCheckKind) -> Job {
        Job {
            key: "service".into(),
            fqdn: "service.test.peercove.internal".into(),
            target: Target::Ip(match address.ip() {
                std::net::IpAddr::V4(ip) => ip,
                std::net::IpAddr::V6(_) => panic!("IPv4 only"),
            }),
            port: address.port(),
            kind,
            path: "/health".into(),
            expected_status: None,
            gated_member_ip: None,
        }
    }

    #[tokio::test]
    async fn tcp_open_and_refused_are_distinguished() {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let open = check(job(address, HealthCheckKind::Tcp)).await;
        assert_eq!(open.status, ServiceHealthStatus::Healthy);
        drop(listener);
        let refused = check(job(address, HealthCheckKind::Tcp)).await;
        assert_eq!(refused.status, ServiceHealthStatus::Unhealthy);
        assert_eq!(refused.reason, ServiceHealthReason::ConnectionFailed);
    }

    #[tokio::test]
    async fn http_head_checks_status_without_getting_body() {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut reader = BufReader::new(stream);
            let mut first = String::new();
            reader.read_line(&mut first).await.unwrap();
            assert_eq!(first, "HEAD /health HTTP/1.1\r\n");
            reader
                .get_mut()
                .write_all(b"HTTP/1.1 204 No Content\r\n\r\n")
                .await
                .unwrap();
        });
        let result = check(job(address, HealthCheckKind::HttpHead)).await;
        server.await.unwrap();
        assert_eq!(result.status, ServiceHealthStatus::Healthy);
        assert_eq!(result.http_status, Some(204));
    }

    #[tokio::test]
    async fn http_stall_is_reported_as_timeout() {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (_stream, _) = listener.accept().await.unwrap();
            tokio::time::sleep(Duration::from_secs(1)).await;
        });
        let result = check_with_timeout(
            job(address, HealthCheckKind::HttpHead),
            Duration::from_millis(30),
        )
        .await;
        server.abort();
        assert_eq!(result.status, ServiceHealthStatus::Unhealthy);
        assert_eq!(result.reason, ServiceHealthReason::Timeout);
    }

    #[test]
    fn worker_count_is_bounded() {
        assert_eq!(MAX_CONCURRENT, 8);
    }
}
