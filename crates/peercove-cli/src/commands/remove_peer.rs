//! `remove-peer`: メンバーの削除コマンド。ロジックは `peercove-ops::peers`。

use std::path::Path;

pub use peercove_ops::peers::Selector;

pub fn run(config_path: &Path, selector: &Selector) -> anyhow::Result<()> {
    let removed = peercove_ops::peers::remove_peer(config_path, selector)?;

    if let Some(psk_path) = &removed.removed_psk_file {
        println!("PSK ファイル {} を削除しました", psk_path.display());
    }
    println!("メンバー {} を host.toml から削除しました", removed.display);
    println!(
        "実行中の host には約 10 秒で反映されます(本人へ削除通知 → トンネルから除外)。\
         本人が保持しているトークン・鍵は以後使えません"
    );
    Ok(())
}
