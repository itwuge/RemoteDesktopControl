//! rdc-protocol · 协议类型与版本常量的唯一真源
//!
//! 层级：L2 能力层（最底层，无任何 IO 与平台依赖）
//! 用途：定义信令/媒体报文的类型、版本协商、错误码、通道枚举；供 rdc-signal、
//!       rdc-relay、rdc-transport、rdc-connect、rdc-core 与 rdc-bridge 共同引用
//! 调用方：所有其它 crate（见 docs/02 §1 依赖图）
//!
//! 纪律：本 crate 只允许标准库 + serde。任何文件不得出现 socket、线程或平台 API。

// =====** 模块声明 =====**

pub mod error;
pub mod message;
pub mod ports;
pub mod stream;
pub mod version;

// =====** 对外再导出 =====**

pub use error::{ErrorCode, ProtocolError};
pub use message::{Capabilities, SignalEnvelope, SignalMessage};
pub use ports::{PORT_HEALTH, PORT_RELAY_CONTROL, PORT_RELAY_MEDIA, PORT_SIGNAL};
pub use stream::StreamKind;
pub use version::{version_string, PROTOCOL_VERSION, PROTOCOL_VERSION_MAJOR, PROTOCOL_VERSION_MINOR};
