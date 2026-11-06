//! rdc-protocol / 信封序列化基准
//!
//! 层级：bench（不属于产物，只用于 S1 基线与后续回归对比）
//! 运行方式：`cargo bench -p rdc-protocol`
//! 关注点：单次 JSON 编解码耗时，S1 只建立基线，不做性能门槛。

// =====** 基准实现 =====**

use std::time::Instant;

use rdc_protocol::{Capabilities, SignalEnvelope, SignalMessage};

/// 基准迭代次数：足够稳定又不会让 CI 太久。
const ITERATIONS: u32 = 10_000;

/// 运行基准并打印 `ns/op`。
///
/// 返回：无返回值；结果输出到 stdout 供 CI 存档。
fn main() {
    let envelope = SignalEnvelope::new(
        SignalMessage::Register {
            peer_id: "peer-0001".to_owned(),
            public_key: "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=".to_owned(),
        },
        Capabilities(Capabilities::H264 | Capabilities::HEVC),
    );

    // =====** 序列化 =====**
    let start = Instant::now();
    let mut total_bytes = 0usize;
    for _ in 0..ITERATIONS {
        let json = serde_json::to_vec(&envelope).expect("信封序列化失败");
        total_bytes += json.len();
    }
    let encode_ns = start.elapsed().as_nanos() as f64 / ITERATIONS as f64;

    // =====** 反序列化 =====**
    let encoded = serde_json::to_vec(&envelope).expect("信封序列化失败");
    let start = Instant::now();
    for _ in 0..ITERATIONS {
        let _decoded: SignalEnvelope = serde_json::from_slice(&encoded).expect("信封反序列化失败");
    }
    let decode_ns = start.elapsed().as_nanos() as f64 / ITERATIONS as f64;

    println!("envelope_encode    {encode_ns:8.1} ns/op  avg_payload={} bytes", total_bytes / ITERATIONS as usize);
    println!("envelope_decode    {decode_ns:8.1} ns/op");
}
