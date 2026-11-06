//! rdc-transport / 前向纠错（GF(256) 系统化 Reed-Solomon）
//!
//! 层级：L2 能力层
//! 用途：按组生成冗余分片、在丢包后恢复数据分片（docs/04 §3.1：K 个数据包 + M 个校验包）。
//!       仅用于视频通道；FEC 治突发、NACK 治稀疏，两者互补。
//! 调用方：rdc-core 的发送任务（组包后编码）、接收任务（乱序收集后恢复）
//!
//! 实现约定：
//! - 求值点取 GF(256) 上的 `α^i`（α = 2，本原多项式 0x11D）；组内数据分片 ≤ 32、
//!   冗余 ≤ 6，所有点互不相同，满足 MDS 性质；
//! - 组内所有分片必须等长（发送端对最后一刀补零，接收端按帧总长截断）；
//! - 每个字节独立编码：分片第 `b` 字节只与其它分片的第 `b` 字节相关。

use std::sync::OnceLock;

use crate::error::TransportError;
use crate::params::MAX_FEC_GROUP;

// =====** GF(256) 运算表 =====**

/// GF(256) 的指数/对数表，进程内只构建一次。
struct Gf256 {
    /// `exp[i] = α^i`，长度 512，便于直接索引 `log[a] + log[b]`（最大 508）。
    exp: [u8; 512],
    /// `log[v]` 满足 `α^log[v] = v`；`log[0]` 无意义，固定为 0。
    log: [u8; 256],
}

/// 全局表。
static GF: OnceLock<Gf256> = OnceLock::new();

impl Gf256 {
    /// 构建指数/对数表。
    fn build() -> Self {
        let mut exp = [0u8; 512];
        let mut log = [0u8; 256];
        let mut value = 1u8;
        for (i, slot) in exp.iter_mut().enumerate().take(255) {
            *slot = value;
            log[value as usize] = i as u8;
            // 乘 2 后若越过 8 位，按 0x11D（x^8 + x^4 + x^3 + x^2 + 1）约简。
            let doubled = (u16::from(value)) << 1;
            value = if doubled & 0x100 != 0 {
                (doubled ^ 0x011D) as u8
            } else {
                doubled as u8
            };
        }
        for i in 255..512 {
            exp[i] = exp[i - 255];
        }
        Self { exp, log }
    }
}

/// 取全局表，首次调用时构建。
fn gf() -> &'static Gf256 {
    GF.get_or_init(Gf256::build)
}

/// GF(256) 加法（即异或）。
fn gf_add(a: u8, b: u8) -> u8 {
    a ^ b
}

/// GF(256) 乘法。
fn gf_mul(a: u8, b: u8) -> u8 {
    if a == 0 || b == 0 {
        return 0;
    }
    let table = gf();
    table.exp[table.log[a as usize] as usize + table.log[b as usize] as usize]
}

/// GF(256) 乘法逆元：`a^(254)`。
fn gf_inv(a: u8) -> u8 {
    debug_assert!(a != 0, "零没有乘法逆元");
    let table = gf();
    table.exp[255 - table.log[a as usize] as usize]
}

/// 求 `α^i`（i < 255）。
fn gf_point(i: usize) -> u8 {
    let table = gf();
    table.exp[i % 255]
}

/// 计算在 `points` 上插值、于 `target` 处的拉格朗日系数。
///
/// 返回的系数与 `points` 一一对应：`f(target) = Σ weights[k] * f(points[k])`。
fn lagrange_weights(points: &[u8], target: u8) -> Vec<u8> {
    let mut weights = Vec::with_capacity(points.len());
    for (k, &xk) in points.iter().enumerate() {
        let mut numerator = 1u8;
        let mut denominator = 1u8;
        for (i, &xi) in points.iter().enumerate() {
            if i != k {
                numerator = gf_mul(numerator, gf_add(target, xi));
                denominator = gf_mul(denominator, gf_add(xk, xi));
            }
        }
        weights.push(gf_mul(numerator, gf_inv(denominator)));
    }
    weights
}

/// 用拉格朗日系数把若干分片线性组合成一个新分片。
fn combine(weights: &[u8], shards: &[&[u8]], len: usize) -> Vec<u8> {
    let mut out = vec![0u8; len];
    for (weight, shard) in weights.iter().zip(shards) {
        if *weight == 0 {
            continue;
        }
        for (index, byte) in shard.iter().enumerate().take(len) {
            out[index] ^= gf_mul(*weight, *byte);
        }
    }
    out
}

// =====** FecEncoder =====**

/// FEC 编码器。
pub struct FecEncoder {
    /// 每组附加的冗余分片数；0 在构造期即被拒绝。
    redundancy: u8,
}

impl FecEncoder {
    /// 构造编码器。
    ///
    /// # Arguments
    /// * `redundancy` - 每组附加的冗余分片数，必须大于 0
    ///
    /// # Returns
    /// 编码器实例。
    ///
    /// # Errors
    /// `redundancy` 为 0 时返回 [`TransportError::InvalidRate`]：
    /// 无冗余的 FEC 等同于关闭纠错，属于调用方配置错误。
    pub fn new(redundancy: u8) -> Result<Self, TransportError> {
        if redundancy == 0 {
            return Err(TransportError::InvalidRate);
        }
        Ok(Self { redundancy })
    }

    /// 为一组数据分片生成冗余分片。
    ///
    /// # Arguments
    /// * `group` - 同组的原始数据分片，长度不得超过 [`MAX_FEC_GROUP`]，且必须等长
    ///
    /// # Returns
    /// 需要追加发送的冗余分片列表，长度等于构造时的 `redundancy`。
    ///
    /// # Errors
    /// 组为空、超过 [`MAX_FEC_GROUP`]、分片长度不一致，或 `K + M > 255`
    /// 时返回 [`TransportError::InvalidRate`] / [`TransportError::Internal`]。
    pub fn encode(&self, group: &[Vec<u8>]) -> Result<Vec<Vec<u8>>, TransportError> {
        if group.is_empty() || group.len() > MAX_FEC_GROUP {
            return Err(TransportError::InvalidRate);
        }
        let len = group[0].len();
        if group.iter().any(|shard| shard.len() != len) {
            return Err(TransportError::Internal("FEC 分片长度不一致".to_owned()));
        }
        let data_count = group.len();
        if data_count + self.redundancy as usize > 255 {
            return Err(TransportError::InvalidRate);
        }
        let points: Vec<u8> = (0..data_count).map(gf_point).collect();
        let slices: Vec<&[u8]> = group.iter().map(Vec::as_slice).collect();
        let mut parity = Vec::with_capacity(self.redundancy as usize);
        for j in 0..self.redundancy as usize {
            let target = gf_point(data_count + j);
            let weights = lagrange_weights(&points, target);
            parity.push(combine(&weights, &slices, len));
        }
        Ok(parity)
    }
}

// =====** FecDecoder =====**

/// FEC 解码器，按组恢复丢失的数据分片。
pub struct FecDecoder {
    /// 与编码端约定的冗余分片数。
    redundancy: u8,
}

impl FecDecoder {
    /// 构造解码器。
    ///
    /// # Arguments
    /// * `redundancy` - 与编码端一致的冗余分片数；0 表示直通（不做恢复）
    ///
    /// # Returns
    /// 解码器实例。
    pub fn new(redundancy: u8) -> Self {
        Self { redundancy }
    }

    /// 从组内分片（含空洞）恢复数据分片。
    ///
    /// # Arguments
    /// * `shards` - 组内槽位，前 K 个是数据分片、后 M 个是校验分片，
    ///   `None` 表示该分片丢失
    ///
    /// # Returns
    /// 按序号排列的完整数据分片（前 K 个）。
    ///
    /// # Errors
    /// - 丢失分片数超过冗余度，或可用分片不足 K 个（无法插值）→ [`TransportError::Internal`]；
    /// - 分片长度不一致 → [`TransportError::Internal`]；
    /// - 槽位总数不足以划分数据/校验 → [`TransportError::InvalidRate`]。
    pub fn recover(&self, shards: &[Option<Vec<u8>>]) -> Result<Vec<Vec<u8>>, TransportError> {
        let parity_count = self.redundancy as usize;
        let total = shards.len();
        if parity_count > 0 && total <= parity_count {
            return Err(TransportError::InvalidRate);
        }
        let data_count = total.saturating_sub(parity_count);
        if data_count == 0 {
            return Err(TransportError::InvalidRate);
        }

        // 组内所有存在的分片必须等长，否则无法按字节插值。
        let mut len = None;
        for shard in shards.iter().flatten() {
            match len {
                None => len = Some(shard.len()),
                Some(expected) if expected != shard.len() => {
                    return Err(TransportError::Internal("FEC 分片长度不一致".to_owned()));
                }
                Some(_) => {}
            }
        }
        let len = len.unwrap_or(0);

        let missing: Vec<usize> = (0..data_count).filter(|i| shards[*i].is_none()).collect();
        if missing.is_empty() {
            return Ok(shards[..data_count]
                .iter()
                .map(|s| s.clone().unwrap_or_default())
                .collect());
        }
        if missing.len() > parity_count || parity_count == 0 {
            return Err(TransportError::Internal(format!(
                "FEC 无法恢复：丢失 {} 个数据分片，冗余仅 {parity_count}",
                missing.len()
            )));
        }

        let available: Vec<usize> = (0..total).filter(|i| shards[*i].is_some()).collect();
        if available.len() < data_count {
            return Err(TransportError::Internal(
                "FEC 可用分片少于数据分片数，无法插值".to_owned(),
            ));
        }
        let points: Vec<u8> = available.iter().map(|i| gf_point(*i)).collect();
        let slices: Vec<&[u8]> = available
            .iter()
            .map(|i| shards[*i].as_deref().expect("可用分片必然存在"))
            .collect();

        let mut out: Vec<Vec<u8>> = Vec::with_capacity(data_count);
        for (i, shard) in shards.iter().enumerate().take(data_count) {
            match shard {
                Some(shard) => out.push(shard.clone()),
                None => {
                    let weights = lagrange_weights(&points, gf_point(i));
                    out.push(combine(&weights, &slices, len));
                }
            }
        }
        Ok(out)
    }
}

// =====** 自检 =====**

#[cfg(test)]
mod tests {
    use super::*;

    /// 构造一组等长的数据分片。
    fn data_shards(count: usize, len: usize) -> Vec<Vec<u8>> {
        (0..count)
            .map(|i| (0..len).map(|b| (i * 31 + b) as u8).collect())
            .collect()
    }

    /// 编码出的校验分片数量必须等于冗余度，且与数据等长。
    #[test]
    fn encode_parity_shape() {
        let encoder = FecEncoder::new(2).expect("冗余 2 合法");
        let parity = encoder.encode(&data_shards(4, 64)).expect("编码成功");
        assert_eq!(parity.len(), 2);
        assert!(parity.iter().all(|p| p.len() == 64));
    }

    /// 无丢包时 recover 应原样返回数据分片。
    #[test]
    fn recover_without_loss_returns_data() {
        let encoder = FecEncoder::new(2).expect("合法");
        let data = data_shards(4, 32);
        let parity = encoder.encode(&data).expect("编码成功");
        let mut slots: Vec<Option<Vec<u8>>> = data.iter().cloned().map(Some).collect();
        slots.extend(parity.into_iter().map(Some));
        let decoder = FecDecoder::new(2);
        assert_eq!(decoder.recover(&slots).expect("恢复成功"), data);
    }

    /// 丢 1 个数据分片可被恢复，且字节完全一致。
    #[test]
    fn recover_single_lost_shard() {
        let encoder = FecEncoder::new(2).expect("合法");
        let data = data_shards(4, 48);
        let parity = encoder.encode(&data).expect("编码成功");
        let mut slots: Vec<Option<Vec<u8>>> = data.iter().cloned().map(Some).collect();
        slots.extend(parity.into_iter().map(Some));
        slots[1] = None;
        let recovered = FecDecoder::new(2).recover(&slots).expect("应可恢复");
        assert_eq!(recovered, data);
    }

    /// 丢 2 个数据分片（等于冗余度）仍可恢复。
    #[test]
    fn recover_two_lost_shards() {
        let encoder = FecEncoder::new(2).expect("合法");
        let data = data_shards(6, 40);
        let parity = encoder.encode(&data).expect("编码成功");
        let mut slots: Vec<Option<Vec<u8>>> = data.iter().cloned().map(Some).collect();
        slots.extend(parity.into_iter().map(Some));
        slots[0] = None;
        slots[4] = None;
        let recovered = FecDecoder::new(2).recover(&slots).expect("应可恢复");
        assert_eq!(recovered, data);
    }

    /// 丢 3 个数据分片超过冗余度，必须报不可恢复而不是返回错数据。
    #[test]
    fn unrecoverable_group_reports_error() {
        let encoder = FecEncoder::new(2).expect("合法");
        let data = data_shards(6, 16);
        let parity = encoder.encode(&data).expect("编码成功");
        let mut slots: Vec<Option<Vec<u8>>> = data.iter().cloned().map(Some).collect();
        slots.extend(parity.into_iter().map(Some));
        for index in [0, 2, 5] {
            slots[index] = None;
        }
        match FecDecoder::new(2).recover(&slots) {
            Err(TransportError::Internal(_)) => {}
            other => panic!("应返回内部错误，实际 {other:?}"),
        }
    }

    /// 分片长度不一致必须拒绝，避免按错误长度插值。
    #[test]
    fn unequal_length_is_rejected() {
        let encoder = FecEncoder::new(1).expect("合法");
        let group = vec![vec![1u8; 8], vec![2u8; 7]];
        assert!(encoder.encode(&group).is_err());
        let decoder = FecDecoder::new(1);
        assert!(decoder.recover(&[Some(vec![1u8; 8]), None, Some(vec![2u8; 7])]).is_err());
    }

    /// 空组与超限组都被拒绝。
    #[test]
    fn invalid_group_is_rejected() {
        let encoder = FecEncoder::new(1).expect("合法");
        assert!(encoder.encode(&[]).is_err());
        let oversized = vec![vec![0u8; 4]; MAX_FEC_GROUP + 1];
        assert!(encoder.encode(&oversized).is_err());
        assert!(FecEncoder::new(0).is_err());
    }

    /// 冗余度 0 的解码器是直通模式：存在丢包时直接报错，不做伪恢复。
    #[test]
    fn zero_redundancy_decoder_cannot_recover() {
        let decoder = FecDecoder::new(0);
        assert!(decoder.recover(&[Some(vec![1]), None]).is_err());
        assert_eq!(decoder.recover(&[Some(vec![1]), Some(vec![2])]).unwrap().len(), 2);
    }
}
