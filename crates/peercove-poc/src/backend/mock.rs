//! テスト用: `WgBackend` の呼び出しを記録するモックバックエンド。

use std::sync::{Arc, Mutex};

use peercove_core::keys::PublicKey;

use super::{PeerSpec, PeerStats, TunnelSpec, WgBackend};

#[derive(Default)]
pub(crate) struct MockBackend {
    pub ops: Vec<String>,
    shared: Option<Arc<Mutex<Vec<String>>>>,
}

impl MockBackend {
    /// 操作記録を外部からも観測できるモック(デーモンのテスト用)。
    pub fn with_shared_ops(shared: Arc<Mutex<Vec<String>>>) -> Self {
        Self {
            ops: Vec::new(),
            shared: Some(shared),
        }
    }

    fn record(&mut self, op: String) {
        if let Some(shared) = &self.shared {
            shared.lock().unwrap().push(op.clone());
        }
        self.ops.push(op);
    }
}

impl WgBackend for MockBackend {
    fn up(&mut self, _spec: &TunnelSpec) -> anyhow::Result<()> {
        self.record("up".to_string());
        Ok(())
    }
    fn add_peer(&mut self, peer: &PeerSpec) -> anyhow::Result<()> {
        self.record(format!("add:{}", peer.public_key));
        Ok(())
    }
    fn remove_peer(&mut self, public_key: &PublicKey) -> anyhow::Result<()> {
        self.record(format!("remove:{public_key}"));
        Ok(())
    }
    fn stats(&mut self) -> anyhow::Result<Vec<PeerStats>> {
        Ok(Vec::new())
    }
    fn down(&mut self) -> anyhow::Result<()> {
        self.record("down".to_string());
        Ok(())
    }
    fn add_route(&mut self, subnet: ipnet::Ipv4Net) -> anyhow::Result<()> {
        self.record(format!("route-add:{subnet}"));
        Ok(())
    }
    fn remove_route(&mut self, subnet: ipnet::Ipv4Net) -> anyhow::Result<()> {
        self.record(format!("route-del:{subnet}"));
        Ok(())
    }
    fn sync_subnet_router(
        &mut self,
        _virtual_subnet: ipnet::Ipv4Net,
        subnets: &[ipnet::Ipv4Net],
        snat: bool,
    ) -> anyhow::Result<()> {
        self.record(format!("router:{subnets:?}:snat={snat}"));
        Ok(())
    }
}
