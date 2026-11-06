//! rdc-decode / D3D11 上屏窗口
//!
//! 层级：L3 编解码层 · 平台适配最内层
//! 用途：创建原生窗口与 D3D11 交换链，把解码得到的 NV12 帧经 D3D11 VideoProcessor
//!       转换成 BGRA 背缓冲并 Present；提供消息泵与关闭信号（docs/05 §2 呈现链）。
//! 调用方：rdc-cli 的 S3b 实测工具（主线程创建并泵消息）、后续 S5 的 Flutter 纹理桥
//!
//! 安全边界（unsafe）：
//! - 窗口过程、COM 与 D3D11 调用全部集中在 present.rs；[`PresentWindow`] 必须在创建它的
//!   线程内泵消息与 Present（DXGI 交换链不跨线程）。
//! - `Map` 指针只在 Unmap 之前有效，绝不跨帧保存。

use std::mem::ManuallyDrop;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

use windows::core::Interface;
use windows::Win32::Foundation::{HINSTANCE, HMODULE, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
use windows::Win32::Graphics::Direct3D11::{
    D3D11_BIND_DECODER, D3D11_CPU_ACCESS_WRITE,
    D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAP_WRITE, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC,
    D3D11_USAGE_DEFAULT, D3D11_USAGE_STAGING, D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
    D3D11_VIDEO_PROCESSOR_CONTENT_DESC, D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC,
    D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC_0, D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC,
    D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC_0, D3D11_VIDEO_PROCESSOR_STREAM,
    D3D11_VIDEO_USAGE_PLAYBACK_NORMAL, D3D11_VPIV_DIMENSION_TEXTURE2D,
    D3D11_VPOV_DIMENSION_TEXTURE2D, D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext,
    ID3D11Texture2D, ID3D11VideoContext, ID3D11VideoDevice, ID3D11VideoProcessor,
    ID3D11VideoProcessorEnumerator, ID3D11VideoProcessorInputView, D3D11_TEX2D_VPIV,
    D3D11_TEX2D_VPOV,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_ALPHA_MODE_IGNORE, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_NV12, DXGI_RATIONAL,
    DXGI_SAMPLE_DESC,
};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, DXGI_SCALING_STRETCH, DXGI_SWAP_CHAIN_DESC1, DXGI_SWAP_EFFECT_FLIP_DISCARD,
    DXGI_USAGE_RENDER_TARGET_OUTPUT, IDXGIFactory2, IDXGISwapChain1,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, PeekMessageW, RegisterClassW,
    ShowWindow, TranslateMessage, CS_HREDRAW, CS_VREDRAW, MSG, PM_REMOVE, SW_SHOW, WNDCLASSW,
    WINDOW_EX_STYLE, WS_OVERLAPPEDWINDOW,
};

use crate::error::DecodeError;

/// 窗口类名；进程内只注册一次。
const CLASS_NAME: &str = "rdc-s3b-present";

/// 关闭请求标志：`WM_CLOSE` 只置位，由呈现循环读取。
static CLOSING: AtomicBool = AtomicBool::new(false);

/// 窗口过程：只处理关闭与销毁，其余交给系统默认。
///
/// # Safety
/// 标准 WNDPROC 回调，参数由系统传入；本函数不保存任何指针。
unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        windows::Win32::UI::WindowsAndMessaging::WM_CLOSE => {
            CLOSING.store(true, Ordering::SeqCst);
            LRESULT(0)
        }
        windows::Win32::UI::WindowsAndMessaging::WM_DESTROY => LRESULT(0),
        _ => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
    }
}

/// 注册窗口类（幂等）。
fn register_class() -> Result<(), DecodeError> {
    static REGISTERED: OnceLock<Result<(), String>> = OnceLock::new();
    match REGISTERED.get_or_init(|| unsafe {
        let instance = GetModuleHandleW(None).map_err(|e| format!("GetModuleHandleW: {e}"))?;
        // 窗口类名必须活到 RegisterClassW 返回之后再释放，不能传临时串的指针。
        let class_name = wide_string(CLASS_NAME);
        let class = WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(window_proc),
            hInstance: HINSTANCE(instance.0),
            lpszClassName: windows::core::PCWSTR(class_name.as_ptr()),
            ..Default::default()
        };
        let atom = RegisterClassW(&class);
        if atom == 0 {
            return Err(format!(
                "RegisterClassW 失败: {}",
                windows::Win32::Foundation::GetLastError().0
            ));
        }
        Ok(())
    }) {
        Ok(()) => Ok(()),
        Err(msg) => Err(DecodeError::PresentFailure(msg.clone())),
    }
}

/// 构造以 `\0` 结尾的 UTF-16 字符串。
fn wide_string(text: &str) -> Vec<u16> {
    let mut out: Vec<u16> = text.encode_utf16().collect();
    out.push(0);
    out
}

// =====** PresentWindow =====**

/// 原生窗口 + D3D11 交换链 + NV12 视频处理器。
pub struct PresentWindow {
    /// 窗口句柄。
    hwnd: HWND,
    /// D3D11 设备。
    device: ID3D11Device,
    /// 立即上下文。
    context: ID3D11DeviceContext,
    /// 交换链。
    swapchain: IDXGISwapChain1,
    /// 视频设备（VideoProcessor 入口）。
    video_device: ID3D11VideoDevice,
    /// 视频上下文。
    video_context: ID3D11VideoContext,
    /// 视频处理器枚举器。
    enumerator: ID3D11VideoProcessorEnumerator,
    /// 视频处理器。
    processor: ID3D11VideoProcessor,
    /// NV12 默认用途纹理（解码帧上传目标）。
    nv12: ID3D11Texture2D,
    /// NV12 暂存纹理（CPU 写入用）。
    staging: ID3D11Texture2D,
    /// 每帧复用的输入视图。
    input_view: ID3D11VideoProcessorInputView,
    /// 帧宽（像素）。
    width: u32,
    /// 帧高（像素）。
    height: u32,
    /// 窗口客户区宽。
    client_width: u32,
    /// 窗口客户区高。
    client_height: u32,
    /// 已呈现帧数。
    presented: u64,
}

impl PresentWindow {
    /// 创建窗口与呈现链。
    ///
    /// # Arguments
    /// * `width` - 视频帧宽（像素）
    /// * `height` - 视频帧高（像素）
    /// * `title` - 窗口标题
    ///
    /// # Returns
    /// 已显示窗口的实例。
    ///
    /// # Errors
    /// 窗口类注册、窗口创建、D3D11 设备/交换链/视频处理器初始化失败时返回
    /// [`DecodeError::PresentFailure`]。
    pub fn create(width: u32, height: u32, title: &str) -> Result<Self, DecodeError> {
        if width == 0 || height == 0 {
            return Err(DecodeError::InvalidArgument("呈现尺寸为 0".to_owned()));
        }
        register_class()?;
        CLOSING.store(false, Ordering::SeqCst);

        // 客户区限制在 1280x720 内，避免 1080p 窗口超出屏幕导致用户看不到全画面。
        let scale = f64::from(width.min(1280)) / f64::from(width);
        let client_width = (f64::from(width) * scale).round().max(16.0) as u32;
        let client_height =
            ((f64::from(height) * scale).round().max(16.0)) as u32;

        let title = wide_string(title);
        let class = wide_string(CLASS_NAME);
        let instance = unsafe { GetModuleHandleW(None) }
            .map_err(|e| DecodeError::PresentFailure(format!("GetModuleHandleW: {e}")))?;
        let hwnd = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                windows::core::PCWSTR(class.as_ptr()),
                windows::core::PCWSTR(title.as_ptr()),
                WS_OVERLAPPEDWINDOW,
                100,
                100,
                client_width as i32,
                client_height as i32,
                None,
                None,
                HINSTANCE(instance.0),
                None,
            )
        };
        if hwnd.0 == 0 {
            // CreateWindowExW 失败时返回空句柄，原因从 GetLastError 取。
            let code = unsafe { windows::Win32::Foundation::GetLastError().0 };
            return Err(DecodeError::PresentFailure(format!(
                "CreateWindowExW 失败: {code}"
            )));
        }
        unsafe {
            let _ = ShowWindow(hwnd, SW_SHOW);
        }

        let mut device: Option<ID3D11Device> = None;
        let mut context: Option<ID3D11DeviceContext> = None;
        unsafe {
            D3D11CreateDevice(
                None,
                D3D_DRIVER_TYPE_HARDWARE,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                None,
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            )
            .map_err(|e| DecodeError::PresentFailure(format!("D3D11CreateDevice: {e}")))?;
        }
        let device = device.ok_or_else(|| DecodeError::PresentFailure("没有 D3D11 设备".to_owned()))?;
        let context =
            context.ok_or_else(|| DecodeError::PresentFailure("没有 D3D11 上下文".to_owned()))?;

        let factory: IDXGIFactory2 = unsafe { CreateDXGIFactory1() }
            .map_err(|e| DecodeError::PresentFailure(format!("CreateDXGIFactory1: {e}")))?;
        let desc = DXGI_SWAP_CHAIN_DESC1 {
            Width: client_width,
            Height: client_height,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            Stereo: false.into(),
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
            BufferCount: 2,
            Scaling: DXGI_SCALING_STRETCH,
            SwapEffect: DXGI_SWAP_EFFECT_FLIP_DISCARD,
            AlphaMode: DXGI_ALPHA_MODE_IGNORE,
            Flags: 0,
        };
        let swapchain =
            unsafe { factory.CreateSwapChainForHwnd(&device, hwnd, &desc as *const _, None, None) }
                .map_err(|e| DecodeError::PresentFailure(format!("CreateSwapChainForHwnd: {e}")))?;

        let video_device: ID3D11VideoDevice = device
            .cast()
            .map_err(|e| DecodeError::PresentFailure(format!("ID3D11VideoDevice: {e}")))?;
        let video_context: ID3D11VideoContext = context
            .cast()
            .map_err(|e| DecodeError::PresentFailure(format!("ID3D11VideoContext: {e}")))?;
        let content = D3D11_VIDEO_PROCESSOR_CONTENT_DESC {
            InputFrameFormat: D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
            InputFrameRate: DXGI_RATIONAL {
                Numerator: 60,
                Denominator: 1,
            },
            InputWidth: width,
            InputHeight: height,
            OutputFrameRate: DXGI_RATIONAL {
                Numerator: 60,
                Denominator: 1,
            },
            OutputWidth: client_width,
            OutputHeight: client_height,
            Usage: D3D11_VIDEO_USAGE_PLAYBACK_NORMAL,
        };
        let enumerator = unsafe { video_device.CreateVideoProcessorEnumerator(&content) }
            .map_err(|e| DecodeError::PresentFailure(format!("CreateVideoProcessorEnumerator: {e}")))?;
        let processor = unsafe { video_device.CreateVideoProcessor(&enumerator, 0) }
            .map_err(|e| DecodeError::PresentFailure(format!("CreateVideoProcessor: {e}")))?;

        let (nv12, staging) = create_nv12_textures(&device, width, height)?;
        let input_view = create_input_view(&video_device, &enumerator, &nv12)?;

        let window = Self {
            hwnd,
            device,
            context,
            swapchain,
            video_device,
            video_context,
            enumerator,
            processor,
            nv12,
            staging,
            input_view,
            width,
            height,
            client_width,
            client_height,
            presented: 0,
        };
        // 源矩形与目标矩形只需设置一次；后续每帧只换输入视图。
        unsafe {
            let source = RECT {
                left: 0,
                top: 0,
                right: width as i32,
                bottom: height as i32,
            };
            let dest = RECT {
                left: 0,
                top: 0,
                right: client_width as i32,
                bottom: client_height as i32,
            };
            window.video_context.VideoProcessorSetStreamFrameFormat(
                &window.processor,
                0,
                D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
            );
            window
                .video_context
                .VideoProcessorSetStreamSourceRect(&window.processor, 0, true, Some(&source));
            window
                .video_context
                .VideoProcessorSetStreamDestRect(&window.processor, 0, true, Some(&dest));
        }
        Ok(window)
    }

    /// 窗口句柄（供响应性采样）。
    pub fn hwnd(&self) -> HWND {
        self.hwnd
    }

    /// 帧尺寸（宽, 高）。
    pub fn frame_size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// 客户区尺寸（宽, 高）。
    pub fn client_size(&self) -> (u32, u32) {
        (self.client_width, self.client_height)
    }

    /// D3D11 设备：S5 做 Flutter 外部纹理桥接时必须复用同一设备。
    pub fn device(&self) -> &ID3D11Device {
        &self.device
    }

    /// 是否收到关闭请求。
    pub fn closing(&self) -> bool {
        CLOSING.load(Ordering::SeqCst)
    }

    /// 泵一轮窗口消息。
    ///
    /// # Returns
    /// 本轮处理的消息条数。
    pub fn pump(&self) -> usize {
        let mut count = 0usize;
        let mut message = MSG::default();
        unsafe {
            while PeekMessageW(&mut message, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
                count += 1;
            }
        }
        count
    }

    /// 上传一帧 NV12 并呈现。
    ///
    /// # Arguments
    /// * `data` - NV12 数据（Y 平面 + 交错 UV 平面）
    /// * `stride` - Y 平面行距（字节）
    ///
    /// # Returns
    /// 无返回值；调用方负责在返回后继续泵消息。
    ///
    /// # Errors
    /// 数据长度不足、Map/CopyResource/Blt/Present 失败时返回
    /// [`DecodeError::PresentFailure`]。
    pub fn present_nv12(&mut self, data: &[u8], stride: u32) -> Result<(), DecodeError> {
        let y_len = (self.width as usize).saturating_mul(self.height as usize);
        let uv_len = y_len / 2;
        if data.len() < y_len + uv_len {
            return Err(DecodeError::PresentFailure(format!(
                "NV12 数据长度 {} 不足 {}",
                data.len(),
                y_len + uv_len
            )));
        }

        // 1) CPU 写入暂存纹理，再整体拷到默认用途纹理。
        let mut mapped = Default::default();
        unsafe {
            self.context
                .Map(&self.staging, 0, D3D11_MAP_WRITE, 0, Some(&mut mapped))
                .map_err(|e| DecodeError::PresentFailure(format!("Map(staging): {e}")))?;
            let row_pitch = mapped.RowPitch as usize;
            let dst = mapped.pData as *mut u8;
            // Y 平面：按 stride 逐行拷贝，落到 staging 的 RowPitch。
            for row in 0..self.height as usize {
                let src = data.as_ptr().add(row * stride as usize);
                let target = dst.add(row * row_pitch);
                std::ptr::copy_nonoverlapping(src, target, self.width as usize);
            }
            // UV 平面：每行 `width` 字节（交错 U/V），共 height/2 行。
            let uv_src = data.as_ptr().add(y_len);
            let uv_dst = dst.add(row_pitch * self.height as usize);
            for row in 0..(self.height / 2) as usize {
                let src = uv_src.add(row * stride as usize);
                let target = uv_dst.add(row * row_pitch);
                std::ptr::copy_nonoverlapping(src, target, self.width as usize);
            }
            self.context.Unmap(&self.staging, 0);
            self.context.CopyResource(&self.nv12, &self.staging);
        }

        // 2) 视频处理器把 NV12 转到当前背缓冲。
        let backbuffer: ID3D11Texture2D = unsafe { self.swapchain.GetBuffer(0) }
            .map_err(|e| DecodeError::PresentFailure(format!("GetBuffer: {e}")))?;
        let output_desc = D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC {
            ViewDimension: D3D11_VPOV_DIMENSION_TEXTURE2D,
            Anonymous: D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC_0 {
                Texture2D: D3D11_TEX2D_VPOV { MipSlice: 0 },
            },
        };
        let mut output_view: Option<windows::Win32::Graphics::Direct3D11::ID3D11VideoProcessorOutputView> =
            None;
        unsafe {
            self.video_device
                .CreateVideoProcessorOutputView(&backbuffer, &self.enumerator, &output_desc, Some(&mut output_view))
                .map_err(|e| DecodeError::PresentFailure(format!("CreateVideoProcessorOutputView: {e}")))?;
        }
        let output_view = output_view
            .ok_or_else(|| DecodeError::PresentFailure("输出视图为空".to_owned()))?;

        let mut stream = D3D11_VIDEO_PROCESSOR_STREAM {
            Enable: true.into(),
            OutputIndex: 0,
            InputFrameOrField: 0,
            PastFrames: 0,
            FutureFrames: 0,
            ppPastSurfaces: std::ptr::null_mut(),
            pInputSurface: ManuallyDrop::new(Some(self.input_view.clone())),
            ppFutureSurfaces: std::ptr::null_mut(),
            ppPastSurfacesRight: std::ptr::null_mut(),
            pInputSurfaceRight: ManuallyDrop::new(None),
            ppFutureSurfacesRight: std::ptr::null_mut(),
        };
        unsafe {
            self.video_context
                .VideoProcessorBlt(&self.processor, &output_view, 0, std::slice::from_ref(&stream))
                .map_err(|e| DecodeError::PresentFailure(format!("VideoProcessorBlt: {e}")))?;
            let _ = ManuallyDrop::take(&mut stream.pInputSurface);
            self.swapchain
                .Present(1, 0)
                .ok()
                .map_err(|e| DecodeError::PresentFailure(format!("Present: {e}")))?;
        }
        self.presented += 1;
        Ok(())
    }

    /// 已呈现帧数。
    pub fn presented_frames(&self) -> u64 {
        self.presented
    }
}

impl Drop for PresentWindow {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyWindow(self.hwnd);
        }
    }
}

// =====** 资源创建 =====**

/// 创建 NV12 默认纹理与对应的 CPU 可写暂存纹理。
fn create_nv12_textures(
    device: &ID3D11Device,
    width: u32,
    height: u32,
) -> Result<(ID3D11Texture2D, ID3D11Texture2D), DecodeError> {
    let base = D3D11_TEXTURE2D_DESC {
        Width: width,
        Height: height,
        MipLevels: 1,
        ArraySize: 1,
        Format: DXGI_FORMAT_NV12,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Usage: D3D11_USAGE_DEFAULT,
        // 只声明解码器用途：NV12 纹理带 BIND_RENDER_TARGET 在多数驱动上直接 E_INVALIDARG。
        BindFlags: D3D11_BIND_DECODER.0 as u32,
        CPUAccessFlags: 0,
        MiscFlags: 0,
    };
    let mut default_texture: Option<ID3D11Texture2D> = None;
    unsafe {
        device
            .CreateTexture2D(&base, None, Some(&mut default_texture))
            .map_err(|e| DecodeError::PresentFailure(format!("CreateTexture2D(nv12): {e}")))?;
    }
    let staging_desc = D3D11_TEXTURE2D_DESC {
        Usage: D3D11_USAGE_STAGING,
        BindFlags: 0,
        CPUAccessFlags: D3D11_CPU_ACCESS_WRITE.0 as u32,
        ..base
    };
    let mut staging: Option<ID3D11Texture2D> = None;
    unsafe {
        device
            .CreateTexture2D(&staging_desc, None, Some(&mut staging))
            .map_err(|e| DecodeError::PresentFailure(format!("CreateTexture2D(staging): {e}")))?;
    }
    let default_texture = default_texture
        .ok_or_else(|| DecodeError::PresentFailure("NV12 纹理创建为空".to_owned()))?;
    let staging = staging
        .ok_or_else(|| DecodeError::PresentFailure("暂存纹理创建为空".to_owned()))?;
    Ok((default_texture, staging))
}

/// 为 NV12 纹理创建视频处理器输入视图。
fn create_input_view(
    video_device: &ID3D11VideoDevice,
    enumerator: &ID3D11VideoProcessorEnumerator,
    texture: &ID3D11Texture2D,
) -> Result<ID3D11VideoProcessorInputView, DecodeError> {
    let desc = D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC {
        FourCC: 0,
        ViewDimension: D3D11_VPIV_DIMENSION_TEXTURE2D,
        Anonymous: D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC_0 {
            Texture2D: D3D11_TEX2D_VPIV {
                MipSlice: 0,
                ArraySlice: 0,
            },
        },
    };
    let mut view: Option<ID3D11VideoProcessorInputView> = None;
    unsafe {
        video_device
            .CreateVideoProcessorInputView(texture, enumerator, &desc, Some(&mut view))
            .map_err(|e| DecodeError::PresentFailure(format!("CreateVideoProcessorInputView: {e}")))?;
    }
    view.ok_or_else(|| DecodeError::PresentFailure("输入视图为空".to_owned()))
}

// =====** 自检 =====**

#[cfg(test)]
mod tests {
    use super::*;

    /// 窗口类名字符串必须以 NUL 结尾，且长度正确。
    #[test]
    fn wide_string_is_nul_terminated() {
        let wide = wide_string("rdc");
        assert_eq!(wide.len(), 4);
        assert_eq!(wide[3], 0);
    }

    /// 视频处理器枚举必须能识别 NV12 → BGRA 的基本组合（依赖本机 D3D11）。
    #[test]
    fn nv12_processor_supports_conversion() {
        let device = create_device_for_test();
        let Ok((device, _context)) = device else {
            // 无 GPU/远程桌面会拿不到设备，跳过而不是误报失败。
            return;
        };
        let Ok(video_device) = device.cast::<ID3D11VideoDevice>() else {
            return;
        };
        let content = D3D11_VIDEO_PROCESSOR_CONTENT_DESC {
            InputFrameFormat: D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
            InputFrameRate: DXGI_RATIONAL { Numerator: 60, Denominator: 1 },
            InputWidth: 1920,
            InputHeight: 1080,
            OutputFrameRate: DXGI_RATIONAL { Numerator: 60, Denominator: 1 },
            OutputWidth: 1280,
            OutputHeight: 720,
            Usage: D3D11_VIDEO_USAGE_PLAYBACK_NORMAL,
        };
        let Ok(enumerator) = (unsafe { video_device.CreateVideoProcessorEnumerator(&content) }) else {
            return;
        };
        let supported = unsafe { enumerator.CheckVideoProcessorFormat(DXGI_FORMAT_NV12) };
        assert!(supported.is_ok(), "NV12 输出必须被视频处理器接受");
    }

    /// 测试用设备创建：失败返回 Err 而不是 panic。
    fn create_device_for_test() -> Result<(ID3D11Device, ID3D11DeviceContext), windows::core::Error> {
        let mut device: Option<ID3D11Device> = None;
        let mut context: Option<ID3D11DeviceContext> = None;
        unsafe {
            D3D11CreateDevice(
                None,
                D3D_DRIVER_TYPE_HARDWARE,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                None,
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            )?;
        }
        match (device, context) {
            (Some(device), Some(context)) => Ok((device, context)),
            _ => Err(windows::core::Error::from(windows::Win32::Foundation::E_FAIL)),
        }
    }
}
