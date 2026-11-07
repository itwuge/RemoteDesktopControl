//! rdc-update / 端到端流程测试（本地 HTTP 服务，不依赖外网）
//!
//! 覆盖判据 C1/C2/C3/C5：
//! - 正常发布：检查更新 → 验签 → 下载 → SHA256 通过；
//! - 安装包被篡改：下载后哈希不匹配必须拒绝，且暂存目录不留下可执行文件；
//! - `.sha256` 与清单不一致：在下载前就拒绝。
//!
//! 说明：测试用独立公钥（`IdentityKeyPair::generate`），不接触生产私钥。

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};

use rdc_security::IdentityKeyPair;
use rdc_update::error::codes;
use rdc_update::manifest::{Artifact, UpdateManifest};
use rdc_update::stage;

/// 测试服务器共享状态。
struct ServerState {
    /// 安装包内容（可被测试切换成篡改版本）。
    installer: Vec<u8>,
    /// `.sha256` 内容。
    sha256_text: String,
    /// 清单原始字节。
    manifest: Vec<u8>,
    /// 清单签名。
    signature: String,
    /// 地址前缀（启动后才知道端口）。
    base: String,
}

/// 启动一次性本地 HTTP 服务；每个连接只处理一个请求。
///
/// # Arguments
/// * `state` - 共享状态
///
/// # Returns
/// 监听端口。
///
/// # Errors
/// 绑定失败时 panic（测试环境异常应当立即暴露）。
fn spawn_server(state: Arc<Mutex<ServerState>>) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("绑定本地端口");
    let port = listener.local_addr().expect("读取端口").port();
    state.lock().expect("锁").base = format!("http://127.0.0.1:{port}");
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            handle(stream, &state);
        }
    });
    port
}

/// 处理单个请求。
///
/// # Arguments
/// * `stream` - 连接
/// * `state` - 共享状态
///
/// # Returns
/// 无返回值。
///
/// # Errors
/// 不会失败：错误只导致连接关闭。
fn handle(mut stream: TcpStream, state: &Arc<Mutex<ServerState>>) {
    let mut buffer = [0u8; 4096];
    let read = stream.read(&mut buffer).unwrap_or(0);
    let request = String::from_utf8_lossy(&buffer[..read]);
    let path = request
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .unwrap_or("/");

    let state = state.lock().expect("锁");
    let body: Vec<u8> = match path {
        "/release" => {
            // 资产 URL 在启动后才确定，这里用 base 拼出来。
            let release = format!(
                r#"{{"tag_name":"v0.1.2","body":"测试发布","assets":[
                    {{"name":"update-manifest.json","browser_download_url":"{0}/manifest","size":{1}}},
                    {{"name":"update-manifest.json.sig","browser_download_url":"{0}/manifest.sig","size":88}},
                    {{"name":"RDC-0.1.2-windows-x64-setup.exe","browser_download_url":"{0}/setup.exe","size":{2}}},
                    {{"name":"RDC-0.1.2-windows-x64-setup.exe.sha256","browser_download_url":"{0}/setup.exe.sha256","size":90}}
                ]}}"#,
                state.base,
                state.manifest.len(),
                state.installer.len()
            );
            release.into_bytes()
        }
        "/manifest" => state.manifest.clone(),
        "/manifest.sig" => state.signature.clone().into_bytes(),
        "/setup.exe" => state.installer.clone(),
        "/setup.exe.sha256" => state.sha256_text.clone().into_bytes(),
        _ => b"not found".to_vec(),
    };
    drop(state);

    let header = format!(
        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: application/octet-stream\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(header.as_bytes());
    let _ = stream.write_all(&body);
    let _ = stream.flush();
}

/// 构造清单与签名。
///
/// # Arguments
/// * `identity` - 测试密钥
/// * `sha256` - 安装包哈希
/// * `size` - 安装包大小
///
/// # Returns
/// `(清单字节, 签名)`。
///
/// # Errors
/// 不会失败。
fn build_manifest(
    identity: &IdentityKeyPair,
    sha256: &str,
    size: usize,
) -> (Vec<u8>, String) {
    let manifest = UpdateManifest {
        product: "RDC".to_owned(),
        version: "0.1.2".to_owned(),
        installer: Artifact {
            name: "RDC-0.1.2-windows-x64-setup.exe".to_owned(),
            sha256: sha256.to_owned(),
            size: size as u64,
            url: String::new(),
        },
        notes: "测试发布".to_owned(),
        min_supported_version: String::new(),
    };
    let bytes = serde_json::to_vec(&manifest).expect("序列化清单");
    let signature = identity.sign_b64(&bytes);
    (bytes, signature)
}

/// 完整流程 + 两类拒绝路径。
#[test]
fn check_download_and_reject_tampered_payload() {
    let staging = std::env::temp_dir().join(format!(
        "rdc-update-it-{}-{}",
        std::process::id(),
        rdc_update::stage::unix_seconds()
    ));
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging).expect("创建暂存目录");
    // 环境变量覆盖暂存根：测试绝不写入真实 %LOCALAPPDATA%。
    std::env::set_var("RDC_UPDATE_STAGING_DIR", &staging);

    let identity = IdentityKeyPair::generate();
    let installer = b"fake-installer-bytes".to_vec();
    let sha = rdc_update::hash::sha256_hex(&installer);
    let (manifest_bytes, signature) = build_manifest(&identity, &sha, installer.len());
    let sha_text = format!("{sha}  RDC-0.1.2-windows-x64-setup.exe\n");

    let state = Arc::new(Mutex::new(ServerState {
        installer: installer.clone(),
        sha256_text: sha_text,
        manifest: manifest_bytes,
        signature,
        base: String::new(),
    }));
    let port = spawn_server(state.clone());
    let api = format!("http://127.0.0.1:{port}/release");
    let key = identity.public_key_b64();

    // 1. 检查更新：0.1.1 → 0.1.2 应可用。
    let check = stage::check(&api, None, "0.1.1", &key).expect("检查更新");
    assert!(check.available, "应检测到新版本");
    assert_eq!(check.latest_version, "0.1.2");
    assert_eq!(check.installer_sha256, sha);

    // 2. 下载成功：文件落盘且哈希一致。
    let staged = stage::download(&api, None, &key, "0.1.2").expect("下载");
    assert!(std::path::Path::new(&staged.installer_path).is_file());
    assert_eq!(staged.sha256, sha);

    // 3. 篡改安装包：必须拒绝并删除半成品。
    {
        let mut guard = state.lock().expect("锁");
        guard.installer = b"tampered-payload".to_vec();
    }
    let error = stage::download(&api, None, &key, "0.1.2").expect_err("哈希不符必须失败");
    assert_eq!(error.code, codes::HASH_MISMATCH);
    let final_path = std::path::Path::new(&staged.installer_path);
    // 篡改后下载失败，旧的成功文件仍在（按设计不删除已验证文件）；
    // 但绝不能出现 .part 残留。
    let part_path = final_path.with_extension("exe.part");
    assert!(!part_path.exists(), "失败后不得留下 .part 文件");

    // 4. `.sha256` 与清单不一致：下载前拒绝。
    {
        let mut guard = state.lock().expect("锁");
        guard.installer = installer.clone();
        guard.sha256_text = format!("{}  RDC-0.1.2-windows-x64-setup.exe\n", "0".repeat(64));
    }
    let error = stage::download(&api, None, &key, "0.1.2").expect_err("哈希文件不一致必须失败");
    assert_eq!(error.code, codes::HASH_FILE_INVALID);

    // 5. 标签与清单不一致：检查阶段拒绝。
    {
        let mut guard = state.lock().expect("锁");
        guard.sha256_text = format!("{sha}  RDC-0.1.2-windows-x64-setup.exe\n");
        let (bytes, signature) = build_manifest(&identity, &sha, installer.len());
        let mut manifest: UpdateManifest =
            serde_json::from_slice(&bytes).expect("解析清单");
        manifest.version = "0.1.3".to_owned();
        let tampered = serde_json::to_vec(&manifest).expect("重序列化");
        guard.manifest = tampered;
        guard.signature = signature;
    }
    let error = stage::check(&api, None, "0.1.1", &key).expect_err("版本不一致必须失败");
    assert!(
        error.code == codes::VER_TAG_MISMATCH || error.code == codes::SIG_UNVERIFIED,
        "实际错误码：{}",
        error.code
    );

    let _ = std::fs::remove_dir_all(&staging);
}
