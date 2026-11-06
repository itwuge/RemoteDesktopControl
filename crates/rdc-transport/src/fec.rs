//! rdc-transport / 前向纠错
//!
//! 层级：L2 能力层
//! 用途：按组生成冗余分片、并在丢包后恢复数据分片；S1 只保留参数校验与接口
//! 调用方：rdc-core 的发送任务（组包后编码）、接收任务（乱序收集后恢复）

use crate::error::TransportError;

// =====** FecEncoder =====**

/// FEC 编码器。
pub struct FecEncoder {
    /// 每组附加的冗余分片数；0 在构造期即被拒绝。
    _redundancy: u8,
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
        Ok(Self { _redundancy: redundancy })
    }

    /// 为一组数据分片生成冗余分片。
    ///
    /// # Arguments
    /// * `group` - 同组的原始数据分片，长度不得超过 `params::MAX_FEC_GROUP`
    ///
    /// # Returns
    /// 需要追加发送的冗余分片列表。
    ///
    /// # Errors
    /// 组为空或超过 `MAX_FEC_GROUP` 时返回 [`TransportError::InvalidRate`]。
    ///
    /// # Panics
    /// S1 空壳：实现落地前调用会 panic。
    pub fn encode(&self, _group: &[Vec<u8>]) -> Result<Vec<Vec<u8>>, TransportError> {
        todo!("S1 空壳：FEC 编码在 S2 实现")
    }
}

// =====** FecDecoder =====**

/// FEC 解码器，按组恢复丢失的数据分片。
pub struct FecDecoder {
    /// 与编码端约定的冗余分片数。
    _redundancy: u8,
}

impl FecDecoder {
    /// 构造解码器。
    ///
    /// # Arguments
    /// * `redundancy` - 与编码端一致的冗余分片数
    ///
    /// # Returns
    /// 解码器实例。
    ///
    /// # Errors
    /// 不会失败；冗余度为 0 时解码器退化为直通模式。
    pub fn new(redundancy: u8) -> Self {
        Self { _redundancy: redundancy }
    }

    /// 从组内分片（含空洞）恢复数据分片。
    ///
    /// # Arguments
    /// * `shards` - 组内分片槽位，`None` 表示该分片丢失
    ///
    /// # Returns
    /// 按序号排列的完整数据分片。
    ///
    /// # Errors
    /// 丢失分片数超过冗余度、无法恢复时返回 [`TransportError::Internal`]；
    /// 分片长度不一致时同样返回该错误。
    ///
    /// # Panics
    /// S1 空壳：实现落地前调用会 panic。
    pub fn recover(&self, _shards: &[Option<Vec<u8>>]) -> Result<Vec<Vec<u8>>, TransportError> {
        todo!("S1 空壳：FEC 解码在 S2 实现")
    }
}
