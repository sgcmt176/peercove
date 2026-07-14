//! 端末ローカルの通信品質履歴(M3-23、ADR-0032)。

use std::collections::HashMap;
use std::io::Write as _;
use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use peercove_core::ipc::DirectStatus;
use peercove_core::proto::LedgerEntry;
use peercove_core::quality::{QualityAvailability, QualityReport, QualityRoute, QualitySample};

use crate::backend::PeerStats;
use crate::commands::tunnel::Role;
use crate::control::ProbeWindow;

const WINDOW_MS: u64 = 60_000;
pub const RETENTION_DAYS: u32 = 7;
const RETENTION_MS: u64 = RETENTION_DAYS as u64 * 24 * 60 * 60 * 1_000;
const MAX_TOTAL_BYTES: u64 = 32 * 1024 * 1024;

pub type SharedQuality = Arc<Mutex<QualityStore>>;

struct Aggregate {
    public_key: String,
    ip: Ipv4Addr,
    name: Option<String>,
    availability: Option<QualityAvailability>,
    rtts: Vec<f64>,
    sent: u32,
    received: u32,
    route_direct_secs: u32,
    route_relay_secs: u32,
    route_trying_secs: u32,
    route_switches: u32,
    rx_bytes: u64,
    tx_bytes: u64,
}

impl Default for Aggregate {
    fn default() -> Self {
        Self {
            public_key: String::new(),
            ip: Ipv4Addr::UNSPECIFIED,
            name: None,
            availability: None,
            rtts: Vec::new(),
            sent: 0,
            received: 0,
            route_direct_secs: 0,
            route_relay_secs: 0,
            route_trying_secs: 0,
            route_switches: 0,
            rx_bytes: 0,
            tx_bytes: 0,
        }
    }
}

impl Aggregate {
    fn sample(&self, window_start_unix_ms: u64, now_unix_ms: u64) -> QualitySample {
        let mut sorted = self.rtts.clone();
        sorted.sort_by(f64::total_cmp);
        let avg = (!sorted.is_empty()).then(|| sorted.iter().sum::<f64>() / sorted.len() as f64);
        let p95 = (!sorted.is_empty()).then(|| {
            let index = ((sorted.len() as f64 * 0.95).ceil() as usize)
                .saturating_sub(1)
                .min(sorted.len() - 1);
            sorted[index]
        });
        let jitter = (self.rtts.len() >= 2).then(|| {
            self.rtts
                .windows(2)
                .map(|pair| (pair[1] - pair[0]).abs())
                .sum::<f64>()
                / (self.rtts.len() - 1) as f64
        });
        let availability = self.availability.unwrap_or(QualityAvailability::Unmeasured);
        let loss_percent =
            (availability == QualityAvailability::Connected && self.sent > 0).then(|| {
                let lost = self.sent.saturating_sub(self.received);
                lost as f64 * 100.0 / self.sent as f64
            });
        let route = [
            (self.route_direct_secs, QualityRoute::Direct),
            (self.route_relay_secs, QualityRoute::Relay),
            (self.route_trying_secs, QualityRoute::Trying),
        ]
        .into_iter()
        .max_by_key(|(seconds, _)| *seconds)
        .map(|(_, route)| route)
        .unwrap_or(QualityRoute::Relay);
        QualitySample {
            window_start_unix_ms,
            window_secs: ((now_unix_ms.saturating_sub(window_start_unix_ms)) / 1_000).clamp(1, 60)
                as u32,
            public_key: self.public_key.clone(),
            ip: self.ip,
            name: self.name.clone(),
            availability,
            rtt_latest_ms: self.rtts.last().copied(),
            rtt_min_ms: sorted.first().copied(),
            rtt_avg_ms: avg,
            rtt_p95_ms: p95,
            jitter_ms: jitter,
            probes_sent: self.sent,
            probes_received: self.received,
            loss_percent,
            route,
            route_switches: self.route_switches,
            rx_bytes: self.rx_bytes,
            tx_bytes: self.tx_bytes,
        }
    }
}

pub struct QualityStore {
    directory: PathBuf,
    #[cfg(unix)]
    owner: Option<(u32, u32)>,
    entries: Vec<QualitySample>,
    current_window: Option<u64>,
    current: HashMap<String, Aggregate>,
    previous_bytes: HashMap<String, (u64, u64)>,
    previous_routes: HashMap<String, QualityRoute>,
    skipped_corrupt_lines: u32,
}

impl QualityStore {
    pub fn load(config_path: &Path) -> SharedQuality {
        let directory = config_path.with_extension("quality");
        let mut store = Self {
            directory,
            #[cfg(unix)]
            owner: config_owner(config_path),
            entries: Vec::new(),
            current_window: None,
            current: HashMap::new(),
            previous_bytes: HashMap::new(),
            previous_routes: HashMap::new(),
            skipped_corrupt_lines: 0,
        };
        store.repair_ownership();
        store.read_history(now_unix_ms());
        Arc::new(Mutex::new(store))
    }

    #[allow(clippy::too_many_arguments)]
    pub fn observe(
        &mut self,
        now_unix_ms: u64,
        role: Role,
        self_ip: Ipv4Addr,
        ledger: &[LedgerEntry],
        stats: &[PeerStats],
        probes: &HashMap<Ipv4Addr, ProbeWindow>,
        direct: &HashMap<Ipv4Addr, DirectStatus>,
    ) {
        let window = now_unix_ms / WINDOW_MS * WINDOW_MS;
        if self.current_window.is_some_and(|current| current != window) {
            self.finish_window(window);
        }
        self.current_window.get_or_insert(window);

        let stats_by_key: HashMap<&[u8; 32], &PeerStats> = stats
            .iter()
            .map(|stat| (stat.public_key.as_bytes(), stat))
            .collect();
        for peer in ledger.iter().filter(|entry| entry.ip != self_ip) {
            let key = peer.public_key.to_base64();
            let aggregate = self.current.entry(key.clone()).or_default();
            aggregate.public_key = key.clone();
            aggregate.ip = peer.ip;
            aggregate.name = peer.name.clone();

            let control_expected = role == Role::Host || peer.is_host;
            aggregate.availability = if control_expected {
                Some(match probes.get(&peer.ip).map(|probe| probe.connected) {
                    Some(true) => QualityAvailability::Connected,
                    _ => QualityAvailability::Disconnected,
                })
            } else {
                Some(QualityAvailability::Unmeasured)
            };
            if let Some(probe) = probes.get(&peer.ip) {
                aggregate.sent = aggregate.sent.saturating_add(probe.sent);
                aggregate.received = aggregate.received.saturating_add(probe.received);
                aggregate.rtts.extend(probe.rtts_ms.iter().copied());
            }

            let route = if role == Role::Member && !peer.is_host {
                match direct.get(&peer.ip) {
                    Some(DirectStatus::Direct) => QualityRoute::Direct,
                    Some(DirectStatus::Trying) => QualityRoute::Trying,
                    None => QualityRoute::Relay,
                }
            } else {
                QualityRoute::Relay
            };
            if self
                .previous_routes
                .insert(key.clone(), route)
                .is_some_and(|previous| previous != route)
            {
                aggregate.route_switches = aggregate.route_switches.saturating_add(1);
            }
            match route {
                QualityRoute::Direct => aggregate.route_direct_secs += 5,
                QualityRoute::Relay => aggregate.route_relay_secs += 5,
                QualityRoute::Trying => aggregate.route_trying_secs += 5,
            }

            if let Some(stat) = stats_by_key.get(peer.public_key.as_bytes()) {
                if let Some((old_rx, old_tx)) = self
                    .previous_bytes
                    .insert(key, (stat.rx_bytes, stat.tx_bytes))
                {
                    aggregate.rx_bytes = aggregate
                        .rx_bytes
                        .saturating_add(stat.rx_bytes.saturating_sub(old_rx));
                    aggregate.tx_bytes = aggregate
                        .tx_bytes
                        .saturating_add(stat.tx_bytes.saturating_sub(old_tx));
                }
            }
        }
    }

    pub fn report(&self, since_unix_ms: u64) -> QualityReport {
        let now = now_unix_ms();
        let mut samples: Vec<QualitySample> = self
            .entries
            .iter()
            .filter(|sample| sample.window_start_unix_ms >= since_unix_ms)
            .cloned()
            .collect();
        if let Some(window) = self.current_window {
            samples.extend(
                self.current
                    .values()
                    .map(|aggregate| aggregate.sample(window, now))
                    .filter(|sample| sample.window_start_unix_ms >= since_unix_ms),
            );
        }
        samples.sort_by(|a, b| {
            a.window_start_unix_ms
                .cmp(&b.window_start_unix_ms)
                .then_with(|| a.public_key.cmp(&b.public_key))
        });
        QualityReport {
            generated_at_unix_ms: now,
            retention_days: RETENTION_DAYS,
            skipped_corrupt_lines: self.skipped_corrupt_lines,
            samples,
        }
    }

    fn finish_window(&mut self, next_window: u64) {
        let Some(window) = self.current_window else {
            return;
        };
        let finished: Vec<_> = self
            .current
            .values()
            .map(|aggregate| aggregate.sample(window, window + WINDOW_MS))
            .collect();
        for sample in &finished {
            if let Err(error) = self.append(sample) {
                tracing::warn!("通信品質履歴の書き出しに失敗しました: {error:#}");
            }
        }
        self.entries.extend(finished);
        self.current.clear();
        self.current_window = Some(next_window);
        self.prune(next_window);
    }

    fn append(&self, sample: &QualitySample) -> anyhow::Result<()> {
        std::fs::create_dir_all(&self.directory)?;
        self.apply_owner(&self.directory);
        let path = self
            .directory
            .join(format!("{}.jsonl", utc_date(sample.window_start_unix_ms)));
        let mut line = serde_json::to_string(sample)?;
        line.push('\n');
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)?;
        file.write_all(line.as_bytes())?;
        self.apply_owner(&path);
        Ok(())
    }

    /// Linux のサービス(root)が作った履歴を、設定ファイルの所有者へ戻す。
    /// これを行わないと一般ユーザーの UI からネットワークを削除できない。
    fn repair_ownership(&self) {
        self.apply_owner(&self.directory);
        if let Ok(entries) = std::fs::read_dir(&self.directory) {
            for entry in entries.flatten() {
                self.apply_owner(&entry.path());
            }
        }
    }

    fn apply_owner(&self, path: &Path) {
        #[cfg(unix)]
        if let Some((uid, gid)) = self.owner {
            let _ = std::os::unix::fs::chown(path, Some(uid), Some(gid));
        }
        #[cfg(not(unix))]
        let _ = path;
    }

    fn read_history(&mut self, now_unix_ms: u64) {
        let cutoff = now_unix_ms.saturating_sub(RETENTION_MS);
        let Ok(files) = std::fs::read_dir(&self.directory) else {
            return;
        };
        for path in files
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.extension().and_then(|value| value.to_str()) == Some("jsonl"))
        {
            if let Ok(content) = std::fs::read_to_string(path) {
                for line in content.lines() {
                    match serde_json::from_str::<QualitySample>(line) {
                        Ok(sample) if sample.window_start_unix_ms >= cutoff => {
                            self.entries.push(sample)
                        }
                        Ok(_) => {}
                        Err(_) => {
                            self.skipped_corrupt_lines =
                                self.skipped_corrupt_lines.saturating_add(1)
                        }
                    }
                }
            }
        }
        self.entries
            .sort_by_key(|sample| sample.window_start_unix_ms);
        self.prune(now_unix_ms);
    }

    fn prune(&mut self, now_unix_ms: u64) {
        let cutoff = now_unix_ms.saturating_sub(RETENTION_MS);
        self.entries
            .retain(|sample| sample.window_start_unix_ms >= cutoff);
        let Ok(files) = std::fs::read_dir(&self.directory) else {
            return;
        };
        let mut files: Vec<_> = files
            .flatten()
            .filter_map(|entry| {
                let path = entry.path();
                let size = entry.metadata().ok()?.len();
                (path.extension().and_then(|value| value.to_str()) == Some("jsonl"))
                    .then_some((path, size))
            })
            .collect();
        files.sort_by(|a, b| a.0.cmp(&b.0));
        let cutoff_date = utc_date(cutoff);
        for (path, _) in &files {
            if path.file_stem().and_then(|value| value.to_str()) < Some(&cutoff_date) {
                let _ = std::fs::remove_file(path);
            }
        }
        files.retain(|(path, _)| path.exists());
        let mut total: u64 = files.iter().map(|(_, size)| size).sum();
        for (path, size) in files {
            if total <= MAX_TOTAL_BYTES {
                break;
            }
            if std::fs::remove_file(path).is_ok() {
                total = total.saturating_sub(size);
            }
        }
    }
}

#[cfg(unix)]
fn config_owner(config_path: &Path) -> Option<(u32, u32)> {
    use std::os::unix::fs::MetadataExt as _;
    std::fs::metadata(config_path)
        .ok()
        .map(|metadata| (metadata.uid(), metadata.gid()))
}

pub fn now_unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

/// UNIX 日からグレゴリオ暦の日付を求める(UTC、外部時刻 crate を増やさない)。
fn utc_date(unix_ms: u64) -> String {
    let days = (unix_ms / 86_400_000) as i64;
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let mut year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    year += (month <= 2) as i64;
    format!("{year:04}-{month:02}-{day:02}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_config(label: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "peercove-quality-{label}-{}-{}",
            std::process::id(),
            now_unix_ms()
        ));
        std::fs::create_dir_all(&root).unwrap();
        root.join("test.toml")
    }

    fn persisted_sample(at: u64) -> QualitySample {
        QualitySample {
            window_start_unix_ms: at,
            window_secs: 60,
            public_key: "public".into(),
            ip: "10.0.0.2".parse().unwrap(),
            name: Some("peer".into()),
            availability: QualityAvailability::Connected,
            rtt_latest_ms: Some(10.0),
            rtt_min_ms: Some(9.0),
            rtt_avg_ms: Some(10.0),
            rtt_p95_ms: Some(11.0),
            jitter_ms: Some(1.0),
            probes_sent: 12,
            probes_received: 12,
            loss_percent: Some(0.0),
            route: QualityRoute::Relay,
            route_switches: 0,
            rx_bytes: 1,
            tx_bytes: 2,
        }
    }

    #[test]
    fn utc_dates_are_stable() {
        assert_eq!(utc_date(0), "1970-01-01");
        assert_eq!(utc_date(1_783_987_200_000), "2026-07-14");
    }

    #[test]
    fn aggregate_calculates_percentile_jitter_and_loss() {
        let aggregate = Aggregate {
            public_key: "key".into(),
            ip: "10.0.0.2".parse().unwrap(),
            availability: Some(QualityAvailability::Connected),
            rtts: vec![10.0, 14.0, 12.0, 40.0],
            sent: 5,
            received: 4,
            ..Default::default()
        };
        let sample = aggregate.sample(60_000, 120_000);
        assert_eq!(sample.rtt_min_ms, Some(10.0));
        assert_eq!(sample.rtt_p95_ms, Some(40.0));
        assert_eq!(sample.loss_percent, Some(20.0));
        assert!((sample.jitter_ms.unwrap() - 34.0 / 3.0).abs() < 0.001);
    }

    #[test]
    fn disconnected_is_not_reported_as_total_loss() {
        let aggregate = Aggregate {
            availability: Some(QualityAvailability::Disconnected),
            sent: 3,
            ..Default::default()
        };
        assert_eq!(aggregate.sample(0, 60_000).loss_percent, None);
    }

    #[test]
    fn restart_loads_history_and_skips_corrupt_lines() {
        let config = temp_config("restart");
        let now = now_unix_ms() / WINDOW_MS * WINDOW_MS;
        let store = QualityStore::load(&config);
        store
            .lock()
            .unwrap()
            .append(&persisted_sample(now))
            .unwrap();
        let directory = config.with_extension("quality");
        let path = directory.join(format!("{}.jsonl", utc_date(now)));
        let mut file = std::fs::OpenOptions::new().append(true).open(path).unwrap();
        writeln!(file, "not-json").unwrap();

        let loaded = QualityStore::load(&config);
        let report = loaded.lock().unwrap().report(now.saturating_sub(1));
        assert_eq!(report.samples.len(), 1);
        assert_eq!(report.skipped_corrupt_lines, 1);
        let _ = std::fs::remove_dir_all(config.parent().unwrap());
    }

    #[test]
    fn load_drops_entries_older_than_seven_days() {
        let config = temp_config("retention");
        let now = now_unix_ms() / WINDOW_MS * WINDOW_MS;
        let directory = config.with_extension("quality");
        std::fs::create_dir_all(&directory).unwrap();
        let old = now.saturating_sub(RETENTION_MS + WINDOW_MS);
        let fresh = now.saturating_sub(RETENTION_MS - WINDOW_MS);
        for sample in [persisted_sample(old), persisted_sample(fresh)] {
            let path = directory.join(format!("{}.jsonl", utc_date(sample.window_start_unix_ms)));
            let mut file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .unwrap();
            writeln!(file, "{}", serde_json::to_string(&sample).unwrap()).unwrap();
        }
        let loaded = QualityStore::load(&config);
        assert_eq!(loaded.lock().unwrap().entries.len(), 1);
        let _ = std::fs::remove_dir_all(config.parent().unwrap());
    }
}
