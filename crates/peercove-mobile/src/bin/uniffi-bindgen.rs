//! Kotlin バインディング生成の入口(uniffi のライブラリモード)。
//! apps/peercove-android のビルドから呼ばれる。手動実行:
//!   cargo run -p peercove-mobile --bin uniffi-bindgen -- \
//!     generate --library <libpeercove_mobile.so> --language kotlin --out-dir <dir>

fn main() {
    uniffi::uniffi_bindgen_main()
}
