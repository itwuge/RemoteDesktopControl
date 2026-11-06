// ============================================================
// 文件用途：格式化工具
//
// 层级：core（纯函数，无 IO、无状态）
// 被谁调用：view/components、controller
// ============================================================

/// 数值格式化工具。
class Formatters {
  const Formatters._();

  /// 把 bps 格式化为 `Mbps` / `Kbps` 文本。
  ///
  /// [bps] 为每秒比特数；小于 1 Mbps 时显示 Kbps，保留 1 位小数。
  static String bitrate(int bps) {
    if (bps <= 0) return '0 Kbps';
    if (bps >= 1000 * 1000) {
      return '${(bps / 1000 / 1000).toStringAsFixed(1)} Mbps';
    }
    return '${(bps / 1000).toStringAsFixed(1)} Kbps';
  }

  /// 把毫秒数格式化为 `xx ms`。
  static String latency(int ms) => '$ms ms';

  /// 把 0~1 的比例格式化为百分比（整数）。
  static String percent(double ratio) => '${(ratio * 100).round()}%';
}
