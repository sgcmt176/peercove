// リリースビルドで Windows にコンソールウィンドウを出さない
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    peercove_ui_lib::run()
}
