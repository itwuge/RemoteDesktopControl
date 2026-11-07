//! rdc-linux-e2e / X11 上屏窗口
//!
//! 用途：把解码得到的 BGRA 整帧用 `PutImage` 推到自建窗口，形成「上屏」这一跳。
//! 为什么用 override-redirect：绕过任何窗口管理器，保证窗口尺寸/位置与
//! 解码帧严格 1:1（或调用方指定的缩放），便于判据计算。
//!
//! 注意：本模块只负责「提交上屏请求」；X11 没有垂直同步回执，
//! 因此延迟统计到 `GetInputFocus` 同步往返为止，报告中会写明该口径。

use x11rb::connection::Connection;
use x11rb::protocol::xproto::{
    ConnectionExt, CreateGCAux, CreateWindowAux, EventMask, Gcontext, ImageFormat,
    Window, WindowClass,
};
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as WrapperExt;

// =====** 缩放 =====**

/// 最近邻缩放到目标尺寸（BGRX 4 字节/像素）。
///
/// # Arguments
/// * `src` - 源 BGRA 缓冲。
/// * `sw` / `sh` - 源宽高。
/// * `dst` - 目标缓冲（长度须为 `dw*dh*4`）。
/// * `dw` / `dh` - 目标宽高。
///
/// # Returns
/// 无返回值；尺寸非法时直接返回不做越界写入。
///
/// # Errors
/// 不会失败。
pub fn scale_nearest(src: &[u8], sw: u32, sh: u32, dst: &mut [u8], dw: u32, dh: u32) {
    if sw == 0 || sh == 0 || dw == 0 || dh == 0 {
        return;
    }
    if src.len() < sw as usize * sh as usize * 4 || dst.len() < dw as usize * dh as usize * 4 {
        return;
    }
    for y in 0..dh as usize {
        let sy = (y as u64 * sh as u64 / dh as u64) as usize;
        let src_row = sy * sw as usize * 4;
        let dst_row = y * dw as usize * 4;
        for x in 0..dw as usize {
            let sx = (x as u64 * sw as u64 / dw as u64) as usize;
            let si = src_row + sx * 4;
            let di = dst_row + x * 4;
            dst[di..di + 4].copy_from_slice(&src[si..si + 4]);
        }
    }
}

// =====** Display =====**

/// 一个 override-redirect 的 X11 上屏窗口。
pub struct Display {
    /// X 连接。
    conn: RustConnection,
    /// 窗口。
    window: Window,
    /// 绘图上下文。
    gc: Gcontext,
    /// 窗口内部宽。
    width: u16,
    /// 窗口内部高。
    height: u16,
    /// 窗口位深（取自根窗口，通常 24）。
    depth: u8,
    /// 最近一帧（供 Expose 重绘）。
    last: Option<Vec<u8>>,
    /// 累计上屏帧数。
    presented: u64,
}

impl Display {
    /// 创建窗口并映射。
    ///
    /// # Arguments
    /// * `width` / `height` - 窗口内部尺寸（像素）。
    /// * `x` / `y` - 屏幕位置（root 坐标）。
    /// * `title` - 调试标题（写入 WM_NAME）。
    ///
    /// # Returns
    /// 已映射的窗口实例。
    ///
    /// # Errors
    /// X11 连接失败或建窗请求失败时返回说明文本。
    pub fn create(width: u32, height: u32, x: i16, y: i16, title: &str) -> Result<Self, String> {
        let (conn, screen_num) = x11rb::connect(None).map_err(|error| {
            format!("连接 X11 失败（DISPLAY/XAUTHORITY 是否设置？）: {error}")
        })?;
        let screen = &conn.setup().roots[screen_num];
        let root = screen.root;
        let depth = screen.root_depth;
        let visual = screen.root_visual;

        let window = conn.generate_id().map_err(|error| format!("分配窗口 id 失败: {error}"))?;
        let gc = conn.generate_id().map_err(|error| format!("分配 GC id 失败: {error}"))?;
        let aux = CreateWindowAux::new()
            .background_pixel(0x0010_1010u32)
            .event_mask(EventMask::EXPOSURE | EventMask::STRUCTURE_NOTIFY | EventMask::KEY_PRESS)
            .override_redirect(1u32);
        conn.create_window(
            depth,
            window,
            root,
            x,
            y,
            width as u16,
            height as u16,
            0,
            WindowClass::INPUT_OUTPUT,
            visual,
            &aux,
        )
        .map_err(|error| format!("创建窗口失败: {error}"))?;
        conn.create_gc(gc, window, &CreateGCAux::new())
            .map_err(|error| format!("创建 GC 失败: {error}"))?;
        conn.change_property8(
            x11rb::protocol::xproto::PropMode::REPLACE,
            window,
            x11rb::protocol::xproto::AtomEnum::WM_NAME,
            x11rb::protocol::xproto::AtomEnum::STRING,
            title.as_bytes(),
        )
        .map_err(|error| format!("设置窗口标题失败: {error}"))?;
        conn.map_window(window)
            .map_err(|error| format!("映射窗口失败: {error}"))?;
        conn.flush().map_err(|error| format!("刷新 X11 失败: {error}"))?;

        Ok(Self {
            conn,
            window,
            gc,
            width: width as u16,
            height: height as u16,
            depth,
            last: None,
            presented: 0,
        })
    }

    /// 上屏一帧并等待服务端处理完。
    ///
    /// # Arguments
    /// * `bgra` - 与窗口同尺寸的 BGRA 缓冲（内部尺寸不得小于窗口）。
    ///
    /// # Returns
    /// 服务端已处理请求返回 `Ok(())`（延迟口径见模块注释）。
    ///
    /// # Errors
    /// 缓冲不足或 X11 请求失败时返回说明文本。
    pub fn present(&mut self, bgra: &[u8]) -> Result<(), String> {
        let need = self.width as usize * self.height as usize * 4;
        if bgra.len() < need {
            return Err(format!("上屏缓冲 {} 字节，小于 {}x{}", bgra.len(), self.width, self.height));
        }
        self.conn
            .put_image(
                ImageFormat::Z_PIXMAP,
                self.window,
                self.gc,
                self.width,
                self.height,
                0,
                0,
                0,
                self.depth,
                &bgra[..need],
            )
            .map_err(|error| format!("PutImage 失败: {error}"))?;
        // 用一次往返（GetInputFocus）保证服务端已按序处理到 PutImage；
        // 这是本演示能给出的最强「上屏完成」证据（X11 无 vsync 回执）。
        self.conn
            .flush()
            .map_err(|error| format!("flush 失败: {error}"))?;
        self.conn
            .get_input_focus()
            .map_err(|error| format!("同步请求失败: {error}"))?
            .reply()
            .map_err(|error| format!("同步回复失败: {error}"))?;
        self.last = Some(bgra[..need].to_vec());
        self.presented = self.presented.saturating_add(1);
        Ok(())
    }

    /// 处理待处理事件（Expose 重绘、Esc 退出）。
    ///
    /// # Returns
    /// 窗口仍可用返回 `true`；收到 Esc 或连接断开返回 `false`。
    ///
    /// # Errors
    /// 不会失败；事件读取异常按窗口关闭处理。
    pub fn pump(&mut self) -> bool {
        loop {
            match self.conn.poll_for_event() {
                Ok(Some(event)) => match event {
                    x11rb::protocol::Event::Expose(expose) => {
                        if expose.count == 0 {
                            let last = self.last.clone();
                            if let Some(last) = last {
                                let _ = self.present(&last);
                            }
                        }
                    }
                    x11rb::protocol::Event::KeyPress(key) => {
                        // keycode 9 在常见布局上为 Esc。
                        if key.detail == 9 {
                            return false;
                        }
                    }
                    _ => {}
                },
                Ok(None) => return true,
                Err(_) => return false,
            }
        }
    }

    /// 窗口 id（调试与目标窗口定位用）。
    ///
    /// # Returns
    /// X11 窗口 id。
    ///
    /// # Errors
    /// 不会失败。
    pub fn window_id(&self) -> u32 {
        self.window
    }

    /// 累计上屏帧数。
    ///
    /// # Returns
    /// `present` 成功次数。
    ///
    /// # Errors
    /// 不会失败。
    pub fn presented(&self) -> u64 {
        self.presented
    }

    /// 主动销毁窗口并断开连接。
    ///
    /// # Returns
    /// 无返回值。
    ///
    /// # Errors
    /// 不会失败。
    pub fn destroy(self) {
        let _ = self.conn.destroy_window(self.window);
        let _ = self.conn.free_gc(self.gc);
        let _ = self.conn.flush();
    }
}
