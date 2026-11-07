//! rdc-filetransfer / 集成测试（判据 S4-3-b ~ S4-3-f）
//!
//! 覆盖：小分块往返完整性、篡改块拒绝、断点续传、进度回调、重复/缺块。
//! 说明：这些测试只走 crate 内状态机，不涉及网络与 rdc-core 接线。

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use rdc_filetransfer::{
    decode_state, encode_state, AcceptOutcome, Chunk, FileReceiver, FileSender, FileTransferError,
    Progress,
};

// =====** 测试辅助 =====**

static SERIAL: AtomicU64 = AtomicU64::new(0);

/// 生成独立的临时文件路径。
fn temp_path(name: &str) -> PathBuf {
    let serial = SERIAL.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "rdc-filetransfer-it-{}-{}-{}.bin",
        std::process::id(),
        name,
        serial
    ))
}

/// 伪随机内容（固定种子，测试可复现）。
fn random_bytes(len: usize, seed: u64) -> Vec<u8> {
    let mut state = seed;
    (0..len)
        .map(|_| {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (state >> 33) as u8
        })
        .collect()
}

/// 把发送端全部剩余分块推进接收端。
fn pump(sender: &mut FileSender, receiver: &mut FileReceiver) -> u32 {
    let mut count = 0;
    while let Some(chunk) = sender.next_chunk().expect("读块") {
        match receiver.accept(&chunk).expect("接收") {
            AcceptOutcome::Accepted(_) => count += 1,
            AcceptOutcome::Duplicate => {}
        }
        sender.confirm(chunk.index, true).expect("确认");
    }
    count
}

// =====** S4-3-b 往返完整性 =====**

/// 随机内容按 7 字节块往返，逐字节一致且整文件哈希一致。
#[test]
fn roundtrip_with_small_chunks() {
    let src = temp_path("rt-src");
    let dst = temp_path("rt-dst");
    let content = random_bytes(64 * 1024 + 13, 0x1234_5678_9abc_def0);
    std::fs::write(&src, &content).expect("写源");

    let mut sender = FileSender::open(&src, 7).expect("发送端");
    let mut receiver = FileReceiver::create(
        &dst,
        sender.plan().total_bytes(),
        sender.plan().chunk_size(),
    )
    .expect("接收端");
    let accepted = pump(&mut sender, &mut receiver);
    assert_eq!(accepted, sender.plan().chunk_count());

    let actual = receiver.finish(Some(sender.file_hash())).expect("收尾");
    assert_eq!(actual, sender.file_hash());
    assert_eq!(std::fs::read(&dst).expect("读目标"), content);

    let _ = std::fs::remove_file(&src);
    let _ = std::fs::remove_file(&dst);
}

// =====** S4-3-c 逐块校验 =====**

/// 篡改块内容 → `ChunkHashMismatch`，该块不计入已收；重传正确块后恢复。
#[test]
fn tampered_chunk_is_rejected() {
    let src = temp_path("tamper-src");
    let dst = temp_path("tamper-dst");
    std::fs::write(&src, b"0123456789abcdef").expect("写源");

    let mut sender = FileSender::open(&src, 8).expect("发送端");
    let mut receiver = FileReceiver::create(&dst, 16, 8).expect("接收端");

    let mut chunk = sender.request_chunk(0).expect("取块");
    chunk.data[0] ^= 0xFF;
    match receiver.accept(&chunk) {
        Err(FileTransferError::ChunkHashMismatch(0)) => {}
        other => panic!("期望 ChunkHashMismatch，实际 {other:?}"),
    }
    assert_eq!(receiver.progress().chunks_done, 0);

    // 重新取正确分块（模拟调用方决定重传）→ 接收成功。
    let fresh = sender.request_chunk(0).expect("重取块");
    assert!(matches!(
        receiver.accept(&fresh).expect("接收"),
        AcceptOutcome::Accepted(_)
    ));
    assert_eq!(receiver.progress().chunks_done, 1);

    let _ = std::fs::remove_file(&src);
    let _ = std::fs::remove_file(&dst);
}

/// 偏移/长度与计划不符 → `InvalidChunkLayout`，在哈希校验之前就被拦下。
#[test]
fn chunk_layout_mismatch_is_rejected() {
    let src = temp_path("layout-src");
    let dst = temp_path("layout-dst");
    std::fs::write(&src, b"0123456789abcdef").expect("写源");

    let sender = FileSender::open(&src, 8).expect("发送端");
    let mut receiver = FileReceiver::create(&dst, 16, 8).expect("接收端");
    let wrong = Chunk::new(1, 0, vec![0u8; 8]); // 下标 1 应为偏移 8
    match receiver.accept(&wrong) {
        Err(FileTransferError::InvalidChunkLayout {
            index,
            expected_offset,
            expected_len,
        }) => {
            assert_eq!(index, 1);
            assert_eq!(expected_offset, 8);
            assert_eq!(expected_len, 8);
        }
        other => panic!("期望 InvalidChunkLayout，实际 {other:?}"),
    }
    let _ = sender.plan();
    let _ = std::fs::remove_file(&src);
    let _ = std::fs::remove_file(&dst);
}

// =====** S4-3-d 断点续传 =====**

/// 已确认块重连后不重发；接收端按位图续写，最终哈希一致。
#[test]
fn resume_skips_confirmed_chunks() {
    let src = temp_path("resume-src");
    let dst = temp_path("resume-dst");
    let content = random_bytes(100, 0xdead_beef);
    std::fs::write(&src, &content).expect("写源");

    let mut sender = FileSender::open(&src, 10).expect("发送端");
    let mut receiver = FileReceiver::create(&dst, 100, 10).expect("接收端");

    // 先收前 4 块，模拟连接中断。
    for expected in 0..4 {
        let chunk = sender.next_chunk().expect("读块").expect("有块");
        assert_eq!(chunk.index, expected);
        receiver.accept(&chunk).expect("接收");
        sender.confirm(chunk.index, true).expect("确认");
    }
    let bitmap = encode_state(receiver.received());
    drop(receiver);

    // 重连：双方各自从位图恢复。
    let decoded = decode_state(&bitmap, sender.plan().chunk_count()).expect("解码位图");
    sender.resume_with(&decoded).expect("发送端续传");
    let next = sender.next_chunk().expect("读块").expect("有块");
    assert_eq!(next.index, 4, "已确认块不应重发");

    let mut receiver = FileReceiver::resume(&dst, 100, 10, &decoded).expect("接收端续传");
    assert_eq!(receiver.progress().chunks_done, 4);
    receiver.accept(&next).expect("接收");
    sender.confirm(next.index, true).expect("确认");
    pump(&mut sender, &mut receiver);

    let actual = receiver.finish(Some(sender.file_hash())).expect("收尾");
    assert_eq!(actual, sender.file_hash());
    assert_eq!(std::fs::read(&dst).expect("读目标"), content);

    let _ = std::fs::remove_file(&src);
    let _ = std::fs::remove_file(&dst);
}

// =====** S4-3-e 进度回调 =====**

/// 每次首次确认触发一次回调；进度单调不减；终值等于总数。
#[test]
fn progress_hook_fires_once_per_confirmed_chunk() {
    let src = temp_path("progress-src");
    let content = random_bytes(37, 42);
    std::fs::write(&src, &content).expect("写源");

    let seen: Arc<Mutex<Vec<Progress>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&seen);
    let mut sender = FileSender::open(&src, 5)
        .expect("发送端")
        .with_progress(move |progress| sink.lock().expect("锁").push(progress));

    let total = sender.plan().chunk_count();
    for _ in 0..total {
        let chunk = sender.next_chunk().expect("读块").expect("有块");
        sender.confirm(chunk.index, true).expect("确认");
    }
    // 重复确认不重复回调。
    sender.confirm(total - 1, true).expect("重复确认");
    // 失败确认不计数不回调。
    sender.confirm(0, false).expect("失败确认");

    let seen = seen.lock().expect("锁");
    assert_eq!(seen.len(), total as usize);
    let mut last_done = 0;
    for progress in seen.iter() {
        assert!(progress.chunks_done >= last_done, "进度必须单调不减");
        assert_eq!(progress.chunks_total, total);
        last_done = progress.chunks_done;
    }
    let final_progress = sender.progress();
    assert_eq!(final_progress.chunks_done, total);
    assert_eq!(final_progress.bytes_done, 37);
    assert_eq!(final_progress.fraction(), 1.0);

    let _ = std::fs::remove_file(&src);
}

// =====** S4-3-f 重复/缺块 =====**

/// 重复块返回 `Duplicate` 不重复计数；缺块 finish 返回 `MissingChunks(n)`。
#[test]
fn duplicate_and_missing_chunks() {
    let src = temp_path("dup-src");
    let dst = temp_path("dup-dst");
    std::fs::write(&src, b"0123456789").expect("写源");

    let mut sender = FileSender::open(&src, 4).expect("发送端");
    let mut receiver = FileReceiver::create(&dst, 10, 4).expect("接收端");

    let first = sender.next_chunk().expect("读块").expect("有块");
    assert!(matches!(
        receiver.accept(&first).expect("首次接收"),
        AcceptOutcome::Accepted(_)
    ));
    assert_eq!(
        receiver.accept(&first).expect("重复接收"),
        AcceptOutcome::Duplicate
    );
    assert_eq!(receiver.progress().chunks_done, 1);

    // 3 块只收了 1 块 → 缺 2 块。
    match receiver.finish(None) {
        Err(FileTransferError::MissingChunks(n)) => assert_eq!(n, 2),
        other => panic!("期望 MissingChunks，实际 {other:?}"),
    }

    let _ = std::fs::remove_file(&src);
    let _ = std::fs::remove_file(&dst);
}

/// 收齐但整文件哈希与声明不符 → `FileHashMismatch`。
#[test]
fn file_hash_mismatch_is_detected() {
    let src = temp_path("hash-src");
    let dst = temp_path("hash-dst");
    std::fs::write(&src, b"hello world").expect("写源");

    let mut sender = FileSender::open(&src, 5).expect("发送端");
    let mut receiver = FileReceiver::create(&dst, 11, 5).expect("接收端");
    pump(&mut sender, &mut receiver);

    let mut wrong = sender.file_hash();
    wrong[0] ^= 0xFF;
    assert!(matches!(
        receiver.finish(Some(wrong)),
        Err(FileTransferError::FileHashMismatch)
    ));
    // 正确哈希仍可通过（finish 幂等，不依赖上一次失败状态）。
    receiver.finish(Some(sender.file_hash())).expect("收尾");

    let _ = std::fs::remove_file(&src);
    let _ = std::fs::remove_file(&dst);
}
