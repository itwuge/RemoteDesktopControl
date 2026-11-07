//! rdc-linux-e2e / 目标控件定位
//!
//! 用途：在解码帧里找「目标窗口的纯绿方块」，给控制端自动点击用。
//! 判据要求点击偏差 ≤2px，因此定位口径必须可解释：
//! 取连通面积最大的绿色连通块，用其像素重心作为点击坐标。
//!
//! 为什么取「最大」而不是「全部」：控制端自己的上屏窗口也在被采集的画面里，
//! 它会带出一个缩小版的绿块镜像；真目标的绿块尺寸最大，按面积筛选即可区分。

/// 最小绿块面积（像素），过滤压缩噪声与边缘单点。
pub const MIN_AREA: u64 = 400;

/// 绿色连通块。
#[derive(Debug, Clone, Copy)]
pub struct Blob {
    /// 重心 X（像素）。
    pub cx: u32,
    /// 重心 Y（像素）。
    pub cy: u32,
    /// 面积（像素）。
    pub area: u64,
    /// 外接框左上 X。
    pub min_x: u32,
    /// 外接框左上 Y。
    pub min_y: u32,
    /// 外接框右下 X。
    pub max_x: u32,
    /// 外接框右下 Y。
    pub max_y: u32,
}

/// 判断像素是否为目标绿。
///
/// # Arguments
/// * `bgra` - BGRA 缓冲。
/// * `idx` - 像素首字节下标。
///
/// # Returns
/// 高绿、低红、低蓝返回 `true`。
///
/// # Errors
/// 不会失败。
fn is_green(bgra: &[u8], idx: usize) -> bool {
    let b = bgra[idx];
    let g = bgra[idx + 1];
    let r = bgra[idx + 2];
    let gi = i32::from(g);
    g >= 150 && gi - i32::from(r) >= 60 && gi - i32::from(b) >= 60
}

/// 找出画面里面积最大的绿色连通块。
///
/// # Arguments
/// * `bgra` - 整帧 BGRA 缓冲。
/// * `width` / `height` - 帧宽高。
///
/// # Returns
/// 命中返回绿块（含重心与外接框）；没有达到 [`MIN_AREA`] 的块返回 `None`。
///
/// # Errors
/// 不会失败；缓冲不足时返回 `None`。
pub fn largest_green(bgra: &[u8], width: u32, height: u32) -> Option<Blob> {
    let w = width as usize;
    let h = height as usize;
    if w == 0 || h == 0 || bgra.len() < w * h * 4 {
        return None;
    }
    let mut visited = vec![false; w * h];
    let mut best: Option<Blob> = None;
    let mut stack: Vec<(usize, usize)> = Vec::with_capacity(1024);

    for y in 0..h {
        for x in 0..w {
            let p = y * w + x;
            if visited[p] || !is_green(bgra, p * 4) {
                continue;
            }
            visited[p] = true;
            stack.clear();
            stack.push((x, y));
            let mut area = 0u64;
            let mut sum_x = 0u64;
            let mut sum_y = 0u64;
            let (mut min_x, mut min_y, mut max_x, mut max_y) = (x, y, x, y);
            while let Some((cx, cy)) = stack.pop() {
                area += 1;
                sum_x += cx as u64;
                sum_y += cy as u64;
                min_x = min_x.min(cx);
                min_y = min_y.min(cy);
                max_x = max_x.max(cx);
                max_y = max_y.max(cy);
                // 四邻域扩散；边界检查避免越界。
                if cx > 0 {
                    let np = cy * w + (cx - 1);
                    if !visited[np] && is_green(bgra, np * 4) {
                        visited[np] = true;
                        stack.push((cx - 1, cy));
                    }
                }
                if cx + 1 < w {
                    let np = cy * w + (cx + 1);
                    if !visited[np] && is_green(bgra, np * 4) {
                        visited[np] = true;
                        stack.push((cx + 1, cy));
                    }
                }
                if cy > 0 {
                    let np = (cy - 1) * w + cx;
                    if !visited[np] && is_green(bgra, np * 4) {
                        visited[np] = true;
                        stack.push((cx, cy - 1));
                    }
                }
                if cy + 1 < h {
                    let np = (cy + 1) * w + cx;
                    if !visited[np] && is_green(bgra, np * 4) {
                        visited[np] = true;
                        stack.push((cx, cy + 1));
                    }
                }
            }
            if area < MIN_AREA {
                continue;
            }
            let candidate = Blob {
                cx: (sum_x / area) as u32,
                cy: (sum_y / area) as u32,
                area,
                min_x: min_x as u32,
                min_y: min_y as u32,
                max_x: max_x as u32,
                max_y: max_y as u32,
            };
            let replace = match &best {
                Some(current) => candidate.area > current.area,
                None => true,
            };
            if replace {
                best = Some(candidate);
            }
        }
    }
    best
}

// =====** 自检 =====**

#[cfg(test)]
mod tests {
    use super::*;

    /// 构造纯色 BGRA 帧。
    fn frame(width: u32, height: u32, color: [u8; 4]) -> Vec<u8> {
        let mut data = vec![0u8; (width * height * 4) as usize];
        for chunk in data.chunks_exact_mut(4) {
            chunk.copy_from_slice(&color);
        }
        data
    }

    /// 两个绿块时取面积大的那个，重心正确。
    #[test]
    fn picks_largest_blob() {
        let (w, h) = (64u32, 64u32);
        let mut data = frame(w, h, [0, 0, 0, 255]);
        // 小绿块 8×8 在 (2,2)。
        for y in 2..10u32 {
            for x in 2..10u32 {
                let i = ((y * w + x) * 4) as usize;
                data[i..i + 4].copy_from_slice(&[0, 255, 0, 255]);
            }
        }
        // 大绿块 20×20 在 (30,30)，但小于 MIN_AREA 时会返回 None。
        for y in 30..50u32 {
            for x in 30..50u32 {
                let i = ((y * w + x) * 4) as usize;
                data[i..i + 4].copy_from_slice(&[0, 255, 0, 255]);
            }
        }
        let blob = largest_green(&data, w, h).expect("应找到绿块");
        assert_eq!(blob.area, 400);
        assert_eq!(blob.cx, 39);
        assert_eq!(blob.cy, 39);
    }

    /// 没有绿块时返回 None。
    #[test]
    fn ignores_non_green() {
        let data = frame(32, 32, [20, 20, 20, 255]);
        assert!(largest_green(&data, 32, 32).is_none());
    }
}
