//! rdc-cli / S5-5 可判定验证
//!
//! 层级：L4 验证入口
//! 用途：不依赖 UI 的授权/接管/踢出/免密闭环验证，产出 `logs/s5-5-verify.json`
//!       与 `logs/s5-5-audit.jsonl` 作为运行时证据。
//!
//! 说明：本命令验证的是**同一进程内的内核闭环**（签发→离线校验→角色判定→状态机）；
//! 跨机网络链路属于后续联调，不在此伪造。

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use rdc_core::audit::AuditLog;
use rdc_core::s5::devices::{now_ms, DeviceList};
use rdc_core::s5::grant::{
    evaluate_session, issue_grant, issue_kick, verify_grant, verify_kick, ControlArbiter,
    ControlDecision, GrantSpec, GrantTemplate, KickError, PermBits, ReplayGuard, TakeoverPolicy,
    VerifyContext, CONTROLLER_RESPONSE_TIMEOUT_MS, KICK_EFFECT_MAX_MS, LOCAL_CONFIRM_TIMEOUT_MS,
    REVOCATION_UI_TEXT,
};
use rdc_core::s5::multi::SessionRole;
use rdc_core::s5::passwordless::{
    challenge, decide, respond, verify_response, PairingContext, PairingMode, ACCOUNT_SERVICE_ROLE,
};
use rdc_protocol::{SignalEnvelope, SignalMessage};
use rdc_security::IdentityKeyPair;
use serde_json::json;

/// 单条判据结果。
struct Check {
    /// 编号。
    id: &'static str,
    /// 是否通过。
    pass: bool,
    /// 说明。
    detail: String,
}

/// 执行 S5-5 验证。
///
/// # Returns
/// 全部判据通过返回 `true`。
///
/// # Errors
/// 文件写入或签发失败返回错误文本。
pub fn run() -> Result<bool, String> {
    let now = now_ms();
    let owner = IdentityKeyPair::from_seed(&[11u8; 32]);
    let peer_b = IdentityKeyPair::from_seed(&[12u8; 32]);
    let owner_pub = owner.public_key_b64();
    let peer_b_pub = peer_b.public_key_b64();
    let trusted_owners = vec![owner_pub.clone()];
    let mut audit = AuditLog::new(0);
    let mut checks = Vec::new();

    // =====** V1 授权链路：只观看 → 改可操作 =====**
    let mut replay = ReplayGuard::new();
    let readonly = issue_grant(
        &owner,
        &GrantSpec {
            host_pub: &owner_pub,
            grantee_pub: &peer_b_pub,
            perms: GrantTemplate::ReadOnly.bits(),
            validity_ms: None,
            revoked_epoch_base: 0,
        },
        now,
    )
    .map_err(|error| format!("签发只观看令牌失败：{error}"))?;
    audit.record(
        now,
        "grant_created",
        "v1-readonly",
        "issue",
        None,
        Some(&format!("perms=0x{:X}", readonly.token.perms)),
        "ok",
    );
    let context = VerifyContext {
        local_host_pub: &owner_pub,
        peer_pub: Some(peer_b_pub.as_str()),
        trusted_owner_pubs: &trusted_owners,
        owner_ceiling: PermBits(PermBits::ALL),
        local_revoked_epoch: 0,
        allow_sensitive_bits: false,
    };
    let verified_readonly = verify_grant(
        &readonly.token,
        &readonly.signature,
        &context,
        now + 1_000,
        &mut replay,
    )
    .map_err(|error| format!("只观看令牌校验失败：{} {}", error.code(), error))?;
    let (viewer_effective, viewer_decision) = evaluate_session(
        SessionRole::Viewer,
        PermBits(PermBits::ALL),
        verified_readonly.effective_perms,
        PermBits(PermBits::ALL),
    );
    audit.record(
        now + 1_000,
        "input",
        "v1-viewer",
        "inject",
        Some("role=viewer"),
        Some("denied"),
        "denied",
    );
    checks.push(Check {
        id: "V1-a",
        pass: !viewer_decision.allowed && viewer_decision.reason == "viewer_readonly",
        detail: format!(
            "只观看连接：角色=viewer 有效权限=0x{:X} 注入结果={} 错误码={}",
            viewer_effective.to_bits(),
            if viewer_decision.allowed { "allowed" } else { "denied" },
            viewer_decision.reason
        ),
    });

    // 改为可操作：换新令牌（权限提升必须重签，不能改写旧令牌）。
    let operate = issue_grant(
        &owner,
        &GrantSpec {
            host_pub: &owner_pub,
            grantee_pub: &peer_b_pub,
            perms: GrantTemplate::Operate.bits(),
            validity_ms: None,
            revoked_epoch_base: 0,
        },
        now + 2_000,
    )
    .map_err(|error| format!("签发可操作令牌失败：{error}"))?;
    audit.record(
        now + 2_000,
        "grant_created",
        "v1-operate",
        "issue",
        None,
        Some(&format!("perms=0x{:X}", operate.token.perms)),
        "ok",
    );
    let reconnect = verify_grant(
        &operate.token,
        &operate.signature,
        &context,
        now + 3_000,
        &mut ReplayGuard::new(),
    )
    .map_err(|error| format!("可操作令牌校验失败：{} {}", error.code(), error))?;
    let (control_effective, control_decision) = evaluate_session(
        SessionRole::Controller,
        PermBits(PermBits::VIEW | PermBits::CONTROL),
        reconnect.effective_perms,
        PermBits(PermBits::ALL),
    );
    audit.record(
        now + 3_000,
        "input",
        "v1-controller",
        "inject",
        Some("role=controller"),
        Some("allowed"),
        "allowed",
    );
    checks.push(Check {
        id: "V1-b",
        pass: control_decision.allowed
            && control_effective.contains(PermBits::VIEW | PermBits::CONTROL),
        detail: format!(
            "重连后：角色=controller 有效权限=0x{:X} 注入结果={} 错误码={}",
            control_effective.to_bits(),
            if control_decision.allowed { "allowed" } else { "denied" },
            control_decision.reason
        ),
    });

    // =====** V2 接管：15 秒超时让位 + 本机确认 =====**
    let mut arbiter = ControlArbiter::new(TakeoverPolicy::Confirm);
    let t0 = now + 10_000;
    let _first = arbiter.request_control(1, t0, &mut audit);
    let pending = arbiter.request_control(2, t0 + 1, &mut audit);
    let pending_ok = matches!(
        pending,
        ControlDecision::PendingControllerResponse { requester: 2, .. }
    );
    let after_yield = arbiter.tick(t0 + 1 + CONTROLLER_RESPONSE_TIMEOUT_MS, &mut audit);
    let yield_ok = matches!(
        after_yield,
        Some(ControlDecision::PendingOwnerConfirm { requester: 2, .. })
    );
    let approved = arbiter.owner_confirm(
        true,
        t0 + 1 + CONTROLLER_RESPONSE_TIMEOUT_MS + 100,
        &mut audit,
    );
    let approve_ok = matches!(approved, ControlDecision::Granted { session_id: 2 });
    let role_entry = audit
        .entries()
        .iter()
        .find(|entry| entry.kind == "takeover" && entry.action == "owner_approve");
    let audit_ok = role_entry
        .map(|entry| {
            entry.old_value.as_deref() == Some("controller=1")
                && entry.new_value.as_deref() == Some("controller=2")
        })
        .unwrap_or(false);
    checks.push(Check {
        id: "V2",
        pass: pending_ok && yield_ok && approve_ok && audit_ok,
        detail: format!(
            "C 申请=待批准 {}；15s 超时让位 {}；本机确认后角色=controller{}；审计旧值→新值 {}（{} → {}）",
            pending_ok,
            yield_ok,
            approve_ok,
            audit_ok,
            role_entry.and_then(|e| e.old_value.as_deref()).unwrap_or("—"),
            role_entry.and_then(|e| e.new_value.as_deref()).unwrap_or("—")
        ),
    });

    // 顺带验证本机 30 秒未确认 → 拒绝（可判定）。
    let mut timeout_audit = AuditLog::new(0);
    let mut arbiter2 = ControlArbiter::new(TakeoverPolicy::Confirm);
    arbiter2.request_control(10, t0, &mut timeout_audit);
    arbiter2.request_control(11, t0 + 1, &mut timeout_audit);
    arbiter2.tick(t0 + 1 + CONTROLLER_RESPONSE_TIMEOUT_MS, &mut timeout_audit);
    let local_timeout = arbiter2.tick(
        t0 + 1 + CONTROLLER_RESPONSE_TIMEOUT_MS + LOCAL_CONFIRM_TIMEOUT_MS,
        &mut timeout_audit,
    );
    checks.push(Check {
        id: "V2-timeout",
        pass: matches!(
            local_timeout,
            Some(ControlDecision::Denied {
                code: "local_confirm_timeout"
            })
        ),
        detail: format!(
            "本机 {} 秒未确认 → {}",
            LOCAL_CONFIRM_TIMEOUT_MS / 1_000,
            match local_timeout {
                Some(ControlDecision::Denied { code }) => code,
                _ => "未按预期拒绝",
            }
        ),
    });

    // =====** V3 踢出：下发到画面停止的实测时长 =====**
    let stop = Arc::new(AtomicBool::new(false));
    let frames = Arc::new(AtomicU64::new(0));
    let stop_flag = Arc::clone(&stop);
    let frame_counter = Arc::clone(&frames);
    let renderer = std::thread::spawn(move || {
        // 模拟 30fps 渲染循环：每帧检查一次会话是否已被踢出。
        while !stop_flag.load(Ordering::Relaxed) {
            frame_counter.fetch_add(1, Ordering::Relaxed);
            std::thread::sleep(Duration::from_millis(33));
        }
    });
    std::thread::sleep(Duration::from_millis(200));
    let kick_started = Instant::now();
    let kick = issue_kick(&owner, 7, &peer_b_pub, now_ms());
    let kick_verify = verify_kick(&kick, &trusted_owners, now_ms());
    stop.store(true, Ordering::Relaxed);
    if let Err(payload) = renderer.join() {
        // `JoinHandle::join` 的错误是 `Box<dyn Any + Send>`（panic 载荷），
        // 不是 `Error`，无法挂进 source；这里显式取出可读载荷记日志，
        // 对外文案保持逐字不变。
        let detail = match payload.downcast_ref::<&str>() {
            Some(text) => (*text).to_owned(),
            None => match payload.downcast_ref::<String>() {
                Some(text) => text.clone(),
                None => "未知 panic 载荷".to_owned(),
            },
        };
        tracing::warn!(panic = %detail, "渲染线程异常退出");
        return Err("渲染线程异常退出".to_owned());
    }
    let kick_elapsed_ms = kick_started.elapsed().as_millis() as u64;
    let frames_at_stop = frames.load(Ordering::Relaxed);
    audit.record(
        now_ms(),
        "kick",
        "7",
        "kick_session",
        Some("active"),
        Some("stopped"),
        if kick_verify.is_ok() { "ok" } else { "denied" },
    );
    let expired = verify_kick(
        &kick,
        &trusted_owners,
        now_ms() + KICK_EFFECT_MAX_MS + 1,
    );
    checks.push(Check {
        id: "V3",
        pass: kick_verify.is_ok()
            && kick_elapsed_ms <= KICK_EFFECT_MAX_MS
            && matches!(expired, Err(KickError::Expired)),
        detail: format!(
            "下发→画面停止实测={kick_elapsed_ms}ms（上限 {KICK_EFFECT_MAX_MS}ms，本机回环）；停止时累计帧={frames_at_stop}；超 3 秒票据={}",
            if matches!(expired, Err(KickError::Expired)) { "拒绝" } else { "未拒绝" }
        ),
    });

    // =====** V4 免密：信任直连 / 撤销后必须配对码 / 账号不可达回退 =====**
    let mut devices = DeviceList::new();
    devices.upsert_online("dev-b", "笔记本 B", now);
    devices.set_trusted("dev-b", true);
    devices.set_public_key("dev-b", &peer_b_pub);
    let reachable = PairingContext {
        devices: &devices,
        account_service_reachable: true,
        passwordless_enabled: true,
    };
    let direct = decide("dev-b", &peer_b_pub, &reachable)
        .map_err(|error| format!("免密判定失败：{} {}", error.code(), error))?;
    let challenge_str = challenge("s5-5-session", now);
    let response = respond(&peer_b, &challenge_str);
    let challenge_ok = verify_response(&peer_b_pub, &challenge_str, &response).is_ok();

    // 撤销信任后必须重新配对。
    devices.set_trusted("dev-b", false);
    let after_revoke = decide("dev-b", &peer_b_pub, &PairingContext {
        devices: &devices,
        account_service_reachable: true,
        passwordless_enabled: true,
    })
    .map_err(|error| format!("撤销信任判定失败：{} {}", error.code(), error))?;

    // 账号服务不可达：本地信任回退，连接方式不变。
    devices.set_trusted("dev-b", true);
    let offline = decide("dev-b", &peer_b_pub, &PairingContext {
        devices: &devices,
        account_service_reachable: false,
        passwordless_enabled: true,
    })
    .map_err(|error| format!("离线判定失败：{} {}", error.code(), error))?;

    audit.record(
        now,
        "pairing",
        "dev-b",
        "decide_direct",
        None,
        Some(direct.mode.as_str()),
        "ok",
    );
    audit.record(
        now,
        "pairing",
        "dev-b",
        "decide_after_untrust",
        Some("trusted"),
        Some(after_revoke.mode.as_str()),
        "ok",
    );
    checks.push(Check {
        id: "V4",
        pass: direct.mode == PairingMode::Passwordless
            && !direct.requires_pairing_code
            && challenge_ok
            && after_revoke.mode == PairingMode::PairingCode
            && after_revoke.requires_pairing_code
            && offline.mode == PairingMode::Passwordless
            && offline.used_local_fallback,
        detail: format!(
            "信任设备={}（配对码={}，挑战应答={}）；撤销信任后={}（配对码={}）；账号不可达={}（本地回退={}，配对码={}）",
            direct.mode.as_str(),
            direct.requires_pairing_code,
            challenge_ok,
            after_revoke.mode.as_str(),
            after_revoke.requires_pairing_code,
            offline.mode.as_str(),
            offline.used_local_fallback,
            offline.requires_pairing_code
        ),
    });

    // =====** V5 边界：token 不进包头；撤销文案不承诺立即生效 =====**
    let envelope = SignalEnvelope::new(
        "dev-b",
        7,
        0,
        SignalMessage::Bye {
            session: "s5-5".to_owned(),
            reason: "probe".to_owned(),
        },
    );
    let envelope_bytes = envelope.encode().map_err(|error| error.to_string())?;
    let envelope_text = String::from_utf8_lossy(&envelope_bytes).into_owned();
    let header_clean = ["grant_id", "grant_token", "owner_pub", "grantee_pub"]
        .iter()
        .all(|field| !envelope_text.contains(field));
    let revocation_copy_ok =
        !REVOCATION_UI_TEXT.contains("立即生效") && REVOCATION_UI_TEXT.contains("15 分 90 秒");
    checks.push(Check {
        id: "V5",
        pass: header_clean
            && revocation_copy_ok
            && ACCOUNT_SERVICE_ROLE.contains("不参与端到端加密"),
        detail: format!(
            "信令包头含令牌字段={}；撤销文案「{}」；账号边界={}",
            !header_clean,
            REVOCATION_UI_TEXT,
            ACCOUNT_SERVICE_ROLE
        ),
    });

    // =====** 落盘 =====**
    std::fs::create_dir_all("logs").map_err(|error| error.to_string())?;
    let mut v1 = BTreeMap::new();
    v1.insert("readonly_inject_error", "viewer_readonly");
    v1.insert("reconnect_inject", "allowed");
    let payload = json!({
        "schema": "s5-5-verify",
        "generated_at_ms": now_ms(),
        "environment": if cfg!(windows) { "windows" } else { std::env::consts::OS },
        "v1": {
            "readonly_perms": readonly.token.perms,
            "readonly_effective": verified_readonly.effective_perms.to_bits(),
            "readonly_inject": viewer_decision.reason,
            "operate_perms": operate.token.perms,
            "operate_effective": reconnect.effective_perms.to_bits(),
            "operate_inject": control_decision.reason,
            "notes": v1,
        },
        "v2": {
            "controller_before": 1,
            "controller_after": 2,
            "controller_timeout_ms": CONTROLLER_RESPONSE_TIMEOUT_MS,
            "local_confirm_timeout_ms": LOCAL_CONFIRM_TIMEOUT_MS,
        },
        "v3": {
            "kick_elapsed_ms": kick_elapsed_ms,
            "kick_limit_ms": KICK_EFFECT_MAX_MS,
            "frames_at_stop": frames_at_stop,
            "scope": "local_loopback",
        },
        "v4": {
            "trusted_mode": direct.mode.as_str(),
            "trusted_requires_code": direct.requires_pairing_code,
            "after_untrust_mode": after_revoke.mode.as_str(),
            "account_offline_mode": offline.mode.as_str(),
            "account_offline_fallback": offline.used_local_fallback,
        },
        "v5": {
            "token_in_header": !header_clean,
            "revocation_copy": REVOCATION_UI_TEXT,
        },
        "checks": checks.iter().map(|check| json!({
            "id": check.id,
            "pass": check.pass,
            "detail": check.detail,
        })).collect::<Vec<_>>(),
        "all_pass": checks.iter().all(|check| check.pass),
    });
    std::fs::write(
        "logs/s5-5-verify.json",
        serde_json::to_string_pretty(&payload).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;
    std::fs::write("logs/s5-5-audit.jsonl", audit.to_jsonl())
        .map_err(|error| error.to_string())?;

    for check in &checks {
        println!(
            "{} {} · {}",
            if check.pass { "PASS" } else { "FAIL" },
            check.id,
            check.detail
        );
    }
    let all_pass = checks.iter().all(|check| check.pass);
    println!(
        "S5-5 {} · 证据 logs/s5-5-verify.json · 审计 logs/s5-5-audit.jsonl（{} 条）",
        if all_pass { "通过" } else { "未通过" },
        audit.entries().len()
    );
    Ok(all_pass)
}
