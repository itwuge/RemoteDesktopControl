//! rdc-transport / Pacer 与固定码率控制器基准
//!
//! 层级：bench（不属于产物）
//! 运行方式：`cargo bench -p rdc-transport`
//! 说明：FEC 编码与拥塞控制在 stage3/4 才实现，S1 先建立 Pacer 目标值读取与
//!       固定控制器基线的可运行骨架；实现完成后在同一文件补充真实用例。

// =====** 基准实现 =====**

use std::time::Instant;

use rdc_transport::{create_controller, CongestionKind, Pacer};

/// 基准迭代次数。
const ITERATIONS: u32 = 100_000;

/// 运行基准并打印 `ns/op`。
///
/// 返回：无返回值；结果输出到 stdout 供 CI 存档。
fn main() {
    // =====** Pacer 构造与目标码率读取 =====**
    let start = Instant::now();
    for _ in 0..ITERATIONS {
        let pacer = Pacer::new(8_000_000, 1024).expect("合法码率不应失败");
        std::hint::black_box(pacer.target_bitrate());
    }
    let pacer_ns = start.elapsed().as_nanos() as f64 / ITERATIONS as f64;

    // =====** 固定控制器（Box dyn 分派开销） =====**
    let start = Instant::now();
    for _ in 0..ITERATIONS {
        let controller = create_controller(CongestionKind::Fixed, 8_000_000);
        std::hint::black_box(controller.target_bitrate());
    }
    let fixed_ns = start.elapsed().as_nanos() as f64 / ITERATIONS as f64;

    println!("pacer_new+target   {pacer_ns:8.1} ns/op");
    println!("fixed_controller   {fixed_ns:8.1} ns/op");
    println!("fec_encode         n/a（stage3 实现后补充）");
    println!("gcc_on_ack         n/a（stage4 实现后补充）");
}
