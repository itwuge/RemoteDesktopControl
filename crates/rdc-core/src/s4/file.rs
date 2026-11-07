//! rdc-core / S4-3 文件传输任务
//!
//! 层级：L3 编排层
//! 用途：把 `rdc-filetransfer` 的分块状态机接到会话通道——4 MiB 块在本层切成
//!       ≤1352 字节分片逐条加密发送，接收端重组并校验后再交给 `FileReceiver`。
//! 调用方：rdc-core 会话主流程、rdc-cli 的 `demo-file`。
//!
//! 分片与确认：每块先发 `FileChunkMeta`（块哈希），再发若干 `FileFragment`；
//! 接收端按块重组，校验通过回 `FileAck{ok=true}`，失败回 `ok=false`，发送端重传。
//! 整文件收齐后接收端 `finish` 比对整文件 SHA-256，并用 `FileDone` 回报。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use rdc_connect::EstablishedSession;
use rdc_filetransfer::{Chunk, FileReceiver, FileSender};

use crate::s4::message::{S4Message, FILE_FRAGMENT_BYTES};
use crate::s4::{recv, send, S4Error};

/// 会话内固定传输标识（同一会话同一时刻只跑一个文件）。
pub const TRANSFER_ID: u32 = 1;

/// 失败块最大重传次数（docs/10 §5：≤3 次）。
pub const MAX_RETRANSMISSIONS: u32 = 3;

// =====** 统计 =====**

/// 发送端统计。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SendFileStats {
    /// 文件总字节数。
    pub total_bytes: u64,
    /// 分块数。
    pub chunks: u32,
    /// 发出的分片总数（含重传）。
    pub fragments: u64,
    /// 重传次数（整块）。
    pub retransmissions: u32,
    /// 从发清单到收到最终确认的耗时。
    pub elapsed: Duration,
    /// 吞吐（MB/s，按 1 MB = 10^6 字节）。
    pub throughput_mbps: f64,
}

/// 接收端统计。
#[derive(Debug, Clone, PartialEq)]
pub struct RecvFileStats {
    /// 落盘路径。
    pub path: PathBuf,
    /// 文件总字节数。
    pub total_bytes: u64,
    /// 分块数。
    pub chunks: u32,
    /// 收到的分片总数（含重传）。
    pub fragments: u64,
    /// 接收耗时。
    pub elapsed: Duration,
    /// 接收端算出的整文件 SHA-256。
    pub actual_hash: [u8; 32],
    /// 与发送端声明是否一致。
    pub ok: bool,
}

// =====** 发送端 =====**

/// 发送一个文件。
///
/// # Arguments
/// * `session` - 已建立的安全会话
/// * `path` - 源文件
/// * `chunk_size` - 分块大小（0 表示默认 4 MiB）
/// * `corrupt_first_chunk` - 首次发送第一块时故意改坏一个字节（演示校验与重传）
///
/// # Returns
/// 发送统计。
///
/// # Errors
/// 读盘、通道、确认失败或超过重传上限时返回 [`S4Error`]。
pub async fn send_file(
    session: &mut EstablishedSession,
    path: impl AsRef<Path>,
    chunk_size: u64,
    corrupt_first_chunk: bool,
) -> Result<SendFileStats, S4Error> {
    let mut sender = FileSender::open(path, chunk_size)?;
    let plan = *sender.plan();
    let started = Instant::now();

    send(
        session,
        &S4Message::FileManifest {
            transfer_id: TRANSFER_ID,
            total_bytes: plan.total_bytes(),
            chunk_size: plan.chunk_size(),
            file_hash: sender.file_hash(),
        },
    )
    .await?;

    let mut fragments = 0u64;
    let mut retransmissions = 0u32;
    let mut first_chunk = true;

    while let Some(chunk) = sender.next_chunk()? {
        let index = chunk.index;
        let mut first_try = Some(chunk);
        let mut delivered = false;
        for attempt in 0..=MAX_RETRANSMISSIONS {
            let mut current = match first_try.take() {
                Some(chunk) if attempt == 0 => chunk,
                _ => sender.request_chunk(index)?,
            };
            if corrupt_first_chunk && first_chunk && attempt == 0 {
                if let Some(byte) = current.data.first_mut() {
                    *byte ^= 0xFF;
                }
            }
            fragments += send_fragments(session, &current).await?;
            if wait_ack(session, index).await? {
                sender.confirm(index, true)?;
                delivered = true;
                break;
            }
            retransmissions += 1;
        }
        if !delivered {
            return Err(S4Error::Protocol(format!(
                "分块 {index} 超过 {MAX_RETRANSMISSIONS} 次重传仍未通过校验"
            )));
        }
        first_chunk = false;
    }

    send(session, &S4Message::Done).await?;
    let (actual, ok) = match recv(session).await? {
        S4Message::FileDone {
            transfer_id,
            file_hash,
            ok,
        } => {
            if transfer_id != TRANSFER_ID {
                return Err(S4Error::Protocol(
                    "文件完成回报的 transfer_id 不匹配".into(),
                ));
            }
            (file_hash, ok)
        }
        other => {
            return Err(S4Error::Protocol(format!(
                "文件通道期望 FileDone，收到 {other:?}"
            )))
        }
    };
    if !ok || actual != sender.file_hash() {
        return Err(S4Error::Protocol(format!(
            "接收端整文件哈希不一致：sent={:02x?} recv={:02x?}",
            &sender.file_hash()[..4],
            &actual[..4]
        )));
    }

    let elapsed = started.elapsed();
    Ok(SendFileStats {
        total_bytes: plan.total_bytes(),
        chunks: plan.chunk_count(),
        fragments,
        retransmissions,
        elapsed,
        throughput_mbps: throughput(plan.total_bytes(), elapsed),
    })
}

/// 把一整块切成 1352 字节分片发送，返回分片数。
async fn send_fragments(session: &mut EstablishedSession, chunk: &Chunk) -> Result<u64, S4Error> {
    if chunk.data.is_empty() {
        return Err(S4Error::Protocol("空分块不应出现在传输中".into()));
    }
    send(
        session,
        &S4Message::FileChunkMeta {
            transfer_id: TRANSFER_ID,
            chunk_index: chunk.index,
            chunk_hash: chunk.hash,
        },
    )
    .await?;
    let frag_count = chunk.data.len().div_ceil(FILE_FRAGMENT_BYTES);
    for (frag_index, piece) in chunk.data.chunks(FILE_FRAGMENT_BYTES).enumerate() {
        send(
            session,
            &S4Message::FileFragment {
                transfer_id: TRANSFER_ID,
                chunk_index: chunk.index,
                frag_index: frag_index as u32,
                frag_count: frag_count as u32,
                data: piece.to_vec(),
            },
        )
        .await?;
    }
    Ok(frag_count as u64)
}

/// 等待指定分块的确认。
async fn wait_ack(session: &mut EstablishedSession, chunk_index: u32) -> Result<bool, S4Error> {
    match recv(session).await? {
        S4Message::FileAck {
            transfer_id,
            chunk_index: index,
            ok,
        } => {
            if transfer_id != TRANSFER_ID || index != chunk_index {
                return Err(S4Error::Protocol(format!(
                    "确认错位：期望块 {chunk_index}，收到块 {index}"
                )));
            }
            Ok(ok)
        }
        other => Err(S4Error::Protocol(format!(
            "等待确认时收到非确认消息: {other:?}"
        ))),
    }
}

// =====** 接收端 =====**

/// 接收一个文件写入 `dest`。
///
/// # Arguments
/// * `session` - 已建立的安全会话
/// * `dest` - 落盘路径（会被新建/截断）
///
/// # Returns
/// 接收统计（含最终哈希与是否一致）。
///
/// # Errors
/// 清单缺失/非法、分片错位、写盘失败或整文件哈希不一致时返回 [`S4Error`]。
pub async fn recv_file(
    session: &mut EstablishedSession,
    dest: impl AsRef<Path>,
) -> Result<RecvFileStats, S4Error> {
    let started = Instant::now();
    let (total_bytes, chunk_size, expected_hash) = match recv(session).await? {
        S4Message::FileManifest {
            transfer_id,
            total_bytes,
            chunk_size,
            file_hash,
        } => {
            if transfer_id != TRANSFER_ID {
                return Err(S4Error::Protocol("文件清单的 transfer_id 不匹配".into()));
            }
            (total_bytes, chunk_size, file_hash)
        }
        other => {
            return Err(S4Error::Protocol(format!(
                "文件通道期望 FileManifest，收到 {other:?}"
            )))
        }
    };

    let path = dest.as_ref().to_path_buf();
    let mut receiver = FileReceiver::create(&path, total_bytes, chunk_size)?;
    let plan = *receiver.plan();
    let mut pending: HashMap<u32, PendingChunk> = HashMap::new();
    let mut fragments = 0u64;

    loop {
        match recv(session).await? {
            S4Message::FileChunkMeta {
                transfer_id,
                chunk_index,
                chunk_hash,
            } => {
                if transfer_id != TRANSFER_ID {
                    return Err(S4Error::Protocol("分块元信息 transfer_id 不匹配".into()));
                }
                pending.insert(
                    chunk_index,
                    PendingChunk {
                        chunk_hash,
                        frag_count: 0,
                        received: Vec::new(),
                        present: 0,
                    },
                );
            }
            S4Message::FileFragment {
                transfer_id,
                chunk_index,
                frag_index,
                frag_count,
                data,
            } => {
                if transfer_id != TRANSFER_ID {
                    return Err(S4Error::Protocol("分片 transfer_id 不匹配".into()));
                }
                fragments += 1;
                let entry = pending.get_mut(&chunk_index).ok_or_else(|| {
                    S4Error::Protocol(format!("分块 {chunk_index} 的分片先于元信息到达"))
                })?;
                if entry.frag_count == 0 {
                    entry.frag_count = frag_count;
                    entry.received = vec![None; frag_count as usize];
                } else if entry.frag_count != frag_count {
                    return Err(S4Error::Protocol(format!(
                        "分块 {chunk_index} 分片总数不一致: {} vs {frag_count}",
                        entry.frag_count
                    )));
                }
                let slot = &mut entry.received[frag_index as usize];
                if slot.is_none() {
                    entry.present += 1;
                }
                *slot = Some(data);

                if entry.present == entry.frag_count {
                    let done = pending.remove(&chunk_index).expect("刚判定齐全");
                    let mut data =
                        Vec::with_capacity(plan.chunk_len(chunk_index).unwrap_or(0) as usize);
                    for part in done.received {
                        data.extend(part.expect("齐全校验"));
                    }
                    let offset = plan.chunk_offset(chunk_index).unwrap_or(0);
                    let expected_len = plan.chunk_len(chunk_index).unwrap_or(0) as usize;
                    let chunk = Chunk {
                        index: chunk_index,
                        offset,
                        data,
                        hash: done.chunk_hash,
                    };
                    let ok = chunk.data.len() == expected_len && chunk.verify();
                    if ok {
                        receiver.accept(&chunk)?;
                    }
                    send(
                        session,
                        &S4Message::FileAck {
                            transfer_id: TRANSFER_ID,
                            chunk_index,
                            ok,
                        },
                    )
                    .await?;
                }
            }
            S4Message::Done => break,
            other => {
                return Err(S4Error::Protocol(format!(
                    "文件通道收到非文件消息: {other:?}"
                )))
            }
        }
    }

    let elapsed = started.elapsed();
    let actual_hash = match receiver.finish(None) {
        Ok(hash) => {
            let ok = hash == expected_hash;
            send(
                session,
                &S4Message::FileDone {
                    transfer_id: TRANSFER_ID,
                    file_hash: hash,
                    ok,
                },
            )
            .await?;
            if !ok {
                return Err(S4Error::Protocol(
                    "接收端整文件 SHA-256 与清单不一致".into(),
                ));
            }
            hash
        }
        Err(err) => {
            // 缺块等失败也要回报，避免发送端无限等待。
            send(
                session,
                &S4Message::FileDone {
                    transfer_id: TRANSFER_ID,
                    file_hash: [0u8; 32],
                    ok: false,
                },
            )
            .await?;
            return Err(err.into());
        }
    };

    Ok(RecvFileStats {
        path,
        total_bytes,
        chunks: plan.chunk_count(),
        fragments,
        elapsed,
        actual_hash,
        ok: true,
    })
}

/// 正在重组的分块。
struct PendingChunk {
    /// 发送端声明的块哈希。
    chunk_hash: [u8; 32],
    /// 分片总数（首个分片到达后确定）。
    frag_count: u32,
    /// 按分片下标存放的内容。
    received: Vec<Option<Vec<u8>>>,
    /// 已到达的分片数。
    present: u32,
}

/// 吞吐（MB/s）。
fn throughput(bytes: u64, elapsed: Duration) -> f64 {
    let secs = elapsed.as_secs_f64();
    if secs <= f64::EPSILON {
        return 0.0;
    }
    bytes as f64 / 1_000_000.0 / secs
}
