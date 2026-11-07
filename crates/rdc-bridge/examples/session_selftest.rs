// ============================================================
// 示例用途：会话收尾自检（bye 往返 + 服务停止 + 句柄/端口核查）
// 运行：cargo run -p rdc-bridge --example session_selftest
// 输出：每步结果与关键计数，供 logs/ 存档
// ============================================================

fn main() {
    // 1. 建立会话
    let status = rdc_bridge::api::session_start("controller".to_owned()).expect("建立会话");
    println!(
        "session_start: id={} role={} local_port={} peer_port={}",
        status.session_id, status.role, status.local_port, status.peer_port
    );

    // 2. 拉起两个子服务，制造「有资源要收」的现场
    let input = rdc_bridge::api::input_start(1920, 1080).expect("启动输入");
    println!("input_start: running={} injected={}", input.running, input.injected);
    let clipboard = rdc_bridge::api::clipboard_start().expect("启动剪贴板");
    println!("clipboard_start: running={} sent={}", clipboard.running, clipboard.sent);

    // 3. 断开：bye → 回执 → 停服务 → 核查
    let report = rdc_bridge::api::session_disconnect("自检断开".to_owned()).expect("断开会话");
    println!("bye: sent={} received={} acked={}", report.bye_sent, report.bye_received, report.bye_acked);
    println!("bye_bytes={} ack_latency_us={}", report.bye_bytes, report.ack_latency_us);
    for step in &report.steps {
        println!("step {} ok={} detail={}", step.name, step.ok, step.detail);
    }
    println!(
        "after: services_running={} handles_before={} handles_after={} delta={} ports_released={}",
        report.services_running_after,
        report.handles_before,
        report.handles_after,
        report.handles_delta,
        report.ports_released
    );
    println!("ok={} last_error={:?}", report.ok, report.last_error);

    // 4. 幂等：再断开一次必须返回同一报告且不报错
    let again = rdc_bridge::api::session_disconnect("重复断开".to_owned()).expect("幂等断开");
    println!(
        "idempotent: same_session={} ok={}",
        again.session_id == report.session_id,
        again.ok
    );
    let state = rdc_bridge::api::session_status().expect("会话状态");
    println!("status_after: running={}", state.running);

    if !(report.ok && again.ok && !state.running) {
        eprintln!("session_selftest FAILED");
        std::process::exit(1);
    }
    println!("session_selftest ALL PASS");
}
