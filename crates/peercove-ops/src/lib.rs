//! PeerCove の設定ファイル操作(ADR-0008)。
//!
//! init / invite / join / メンバー管理を CLI と UI の双方から使えるようにする。
//! これらは**デーモンを介さない**(設定ファイル操作なので UI/CLI がユーザー権限で
//! 行い、実行中のトンネルは 5 秒ごとの再読込で追随する — ADR-0002 / 0007)。
//!
//! 関数は表示を持たず、結果を構造体で返す。整形は呼び出し側の責務。

pub mod acl;
pub mod dns;
pub mod init;
pub mod invite;
pub mod join;
pub mod net;
pub mod networks;
pub mod peers;
pub mod secret;
pub mod settings;
