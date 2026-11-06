//! MF 编码器枚举诊断工具（仅 Windows）
//!
//! 用途：当 `probe_encoders()` 报告「无硬件」时，用本工具逐项验证
//!       MFTEnumEx 的过滤条件是否过严（类型过滤 / 标志位 / 类别）。
//! 运行：`cargo run --release -p rdc-codec --example mf_probe`

#[cfg(target_os = "windows")]
fn main() {
    if let Err(e) = windows_impl::run() {
        eprintln!("诊断失败: {e}");
        std::process::exit(1);
    }
    if let Err(e) = windows_impl::negotiate() {
        eprintln!("协商诊断失败: {e}");
    }
}

#[cfg(not(target_os = "windows"))]
fn main() {
    eprintln!("该诊断工具仅支持 Windows");
}

#[cfg(target_os = "windows")]
mod windows_impl {
    use windows::core::Interface;
    use windows::Win32::Media::MediaFoundation::*;
    use windows::Win32::System::Com::CoTaskMemFree;

    /// 构造 NV12 输入类型。
    fn nv12_type(width: u32, height: u32, fps: u32) -> Result<IMFMediaType, String> {
        unsafe {
            let t = MFCreateMediaType().map_err(|e| format!("MFCreateMediaType: {e}"))?;
            t.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video).map_err(|e| e.to_string())?;
            t.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12).map_err(|e| e.to_string())?;
            t.SetUINT64(&MF_MT_FRAME_SIZE, pack(width, height)).map_err(|e| e.to_string())?;
            t.SetUINT64(&MF_MT_FRAME_RATE, pack(fps, 1)).map_err(|e| e.to_string())?;
            t.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, pack(1, 1)).map_err(|e| e.to_string())?;
            t.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)
                .map_err(|e| e.to_string())?;
            Ok(t)
        }
    }

    /// 构造 H.264 输出类型。
    fn h264_type(width: u32, height: u32, fps: u32, bitrate: u32) -> Result<IMFMediaType, String> {
        unsafe {
            let t = MFCreateMediaType().map_err(|e| format!("MFCreateMediaType: {e}"))?;
            t.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video).map_err(|e| e.to_string())?;
            t.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264).map_err(|e| e.to_string())?;
            t.SetUINT32(&MF_MT_AVG_BITRATE, bitrate).map_err(|e| e.to_string())?;
            t.SetUINT64(&MF_MT_FRAME_SIZE, pack(width, height)).map_err(|e| e.to_string())?;
            t.SetUINT64(&MF_MT_FRAME_RATE, pack(fps, 1)).map_err(|e| e.to_string())?;
            t.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)
                .map_err(|e| e.to_string())?;
            t.SetUINT32(&MF_MT_MPEG2_PROFILE, eAVEncH264VProfile_High.0 as u32)
                .map_err(|e| e.to_string())?;
            Ok(t)
        }
    }

    fn pack(hi: u32, lo: u32) -> u64 {
        (u64::from(hi) << 32) | u64::from(lo)
    }

    /// 逐组合枚举，打印数量与名称。
    pub fn run() -> Result<(), String> {
        unsafe {
            MFStartup(MF_VERSION, MFSTARTUP_LITE).map_err(|e| format!("MFStartup: {e}"))?;

            let input_nv12 = MFT_REGISTER_TYPE_INFO {
                guidMajorType: MFMediaType_Video,
                guidSubtype: MFVideoFormat_NV12,
            };
            let output_h264 = MFT_REGISTER_TYPE_INFO {
                guidMajorType: MFMediaType_Video,
                guidSubtype: MFVideoFormat_H264,
            };

            let combos: [(&str, MFT_ENUM_FLAG, bool); 6] = [
                ("HW+NV12->H264", MFT_ENUM_FLAG_HARDWARE, true),
                ("HW|SORT+NV12->H264", MFT_ENUM_FLAG_HARDWARE | MFT_ENUM_FLAG_SORTANDFILTER, true),
                ("ALL+NV12->H264", MFT_ENUM_FLAG_ALL, true),
                ("ALL(no filter)", MFT_ENUM_FLAG_ALL, false),
                ("ASYNCMFT(no filter)", MFT_ENUM_FLAG_ASYNCMFT, false),
                ("HARDWARE(no filter)", MFT_ENUM_FLAG_HARDWARE, false),
            ];

            for (label, flags, filter) in combos {
                let mut raw: *mut Option<IMFActivate> = std::ptr::null_mut();
                let mut count = 0u32;
                let (pin, pout) = if filter {
                    (Some(&input_nv12 as *const _), Some(&output_h264 as *const _))
                } else {
                    (None, None)
                };
                let result = MFTEnumEx(
                    MFT_CATEGORY_VIDEO_ENCODER,
                    flags,
                    pin,
                    pout,
                    &mut raw,
                    &mut count,
                );
                match result {
                    Ok(()) => {
                        println!("[{label}] count={count}");
                        if !raw.is_null() {
                            let slice = std::slice::from_raw_parts(raw, count as usize);
                            for act in slice.iter().flatten() {
                                println!("    {}", friendly_name(act));
                            }
                            CoTaskMemFree(Some(raw as *const core::ffi::c_void));
                        }
                    }
                    Err(e) => println!("[{label}] 失败: {e}"),
                }
            }

            // 无过滤条件时统计所有视频编码器类别（含软件），确认类别本身是否可枚举。
            let mut raw: *mut Option<IMFActivate> = std::ptr::null_mut();
            let mut count = 0u32;
            let _ = MFTEnumEx(
                MFT_CATEGORY_VIDEO_ENCODER,
                MFT_ENUM_FLAG_ALL,
                None,
                None,
                &mut raw,
                &mut count,
            );
            println!("[VIDEO_ENCODER category] count={count}");
            if !raw.is_null() {
                let slice = std::slice::from_raw_parts(raw, count as usize);
                for act in slice.iter().flatten() {
                    println!("    {}", friendly_name(act));
                }
                CoTaskMemFree(Some(raw as *const core::ffi::c_void));
            }
            let _ = MFShutdown();
        }
        Ok(())
    }

    /// 针对第一个硬件 MFT 逐个验证「解锁 → 类型协商」的顺序组合。
    pub fn negotiate() -> Result<(), String> {
        unsafe {
            MFStartup(MF_VERSION, MFSTARTUP_LITE).map_err(|e| format!("MFStartup: {e}"))?;
            let input_nv12 = MFT_REGISTER_TYPE_INFO {
                guidMajorType: MFMediaType_Video,
                guidSubtype: MFVideoFormat_NV12,
            };
            let output_h264 = MFT_REGISTER_TYPE_INFO {
                guidMajorType: MFMediaType_Video,
                guidSubtype: MFVideoFormat_H264,
            };
            let mut raw: *mut Option<IMFActivate> = std::ptr::null_mut();
            let mut count = 0u32;
            MFTEnumEx(
                MFT_CATEGORY_VIDEO_ENCODER,
                MFT_ENUM_FLAG_HARDWARE,
                Some(&input_nv12 as *const _),
                Some(&output_h264 as *const _),
                &mut raw,
                &mut count,
            )
            .map_err(|e| format!("MFTEnumEx: {e}"))?;
            if count == 0 || raw.is_null() {
                return Err("无硬件 MFT".to_owned());
            }
            let activate = (*raw).clone().ok_or("激活对象为空")?;
            CoTaskMemFree(Some(raw as *const core::ffi::c_void));

            // 1) 在激活对象上解锁（有些驱动要求这一步）。
            let unlock_on_activate = match activate.cast::<IMFAttributes>() {
                Ok(attrs) => attrs
                    .SetUINT32(&MF_TRANSFORM_ASYNC_UNLOCK, 1)
                    .map(|_| "ok".to_owned())
                    .unwrap_or_else(|e| format!("{e}")),
                Err(e) => format!("cast 失败: {e}"),
            };
            println!("unlock on activate: {unlock_on_activate}");

            let transform: IMFTransform = activate
                .ActivateObject()
                .map_err(|e| format!("ActivateObject: {e}"))?;

            // 2) 在 transform 上解锁（必须走 GetAttributes，AMD 不实现 cast）。
            let unlock_on_transform = match transform.GetAttributes() {
                Ok(attrs) => attrs
                    .SetUINT32(&MF_TRANSFORM_ASYNC_UNLOCK, 1)
                    .map(|_| "ok".to_owned())
                    .unwrap_or_else(|e| format!("{e}")),
                Err(e) => format!("GetAttributes 失败: {e}"),
            };
            println!("unlock on transform: {unlock_on_transform}");
            println!(
                "is async MFT: {}",
                transform
                    .GetAttributes()
                    .and_then(|a| a.GetUINT32(&MF_TRANSFORM_ASYNC))
                    .map(|v| v != 0)
                    .unwrap_or(false)
            );

            let input = nv12_type(1920, 1080, 60)?;
            let output = h264_type(1920, 1080, 60, 8_000_000)?;

            println!("=== 可用输入类型 ===");
            for i in 0..12 {
                match transform.GetInputAvailableType(0, i) {
                    Ok(t) => println!("  [in {i}] {}", describe(&t)),
                    Err(_) => break,
                }
            }
            println!("=== 可用输出类型 ===");
            for i in 0..12 {
                match transform.GetOutputAvailableType(0, i) {
                    Ok(t) => println!("  [out {i}] {}", describe(&t)),
                    Err(_) => break,
                }
            }
            // 用「驱动自报的可用类型」逐个尝试：先输出后输入。
            let mut chosen_in: Option<IMFMediaType> = None;
            let mut chosen_out: Option<IMFMediaType> = None;
            for i in 0..12 {
                if let Ok(candidate) = transform.GetOutputAvailableType(0, i) {
                    if transform.SetOutputType(0, &candidate, 0).is_ok() {
                        println!("[negotiate] 输出类型 #{i} 接受");
                        chosen_out = Some(candidate);
                        break;
                    }
                } else {
                    break;
                }
            }
            for i in 0..12 {
                if let Ok(candidate) = transform.GetInputAvailableType(0, i) {
                    if transform.SetInputType(0, &candidate, 0).is_ok() {
                        println!("[negotiate] 输入类型 #{i} 接受: {}", describe(&candidate));
                        chosen_in = Some(candidate);
                        break;
                    }
                } else {
                    break;
                }
            }
            println!(
                "自报类型结果: in={} out={}",
                chosen_in.is_some(),
                chosen_out.is_some()
            );

            println!("--- 顺序 A：input → output ---");
            println!("SetInputType : {:?}", transform.SetInputType(0, &input, 0).err());
            println!("SetOutputType: {:?}", transform.SetOutputType(0, &output, 0).err());
            println!("--- 顺序 B：output → input ---");
            println!("SetOutputType: {:?}", transform.SetOutputType(0, &output, 0).err());
            println!("SetInputType : {:?}", transform.SetInputType(0, &input, 0).err());
            let _ = MFShutdown();
        }
        Ok(())
    }

    /// 描述媒体类型的关键属性。
    fn describe(t: &IMFMediaType) -> String {
        unsafe {
            let sub = t.GetGUID(&MF_MT_SUBTYPE).map(|g| format!("{:?}", g)).unwrap_or("?".into());
            let size = t
                .GetUINT64(&MF_MT_FRAME_SIZE)
                .map(|v| format!("{}x{}", v >> 32, v as u32))
                .unwrap_or("-".into());
            let fps = t
                .GetUINT64(&MF_MT_FRAME_RATE)
                .map(|v| format!("{}/{}", v >> 32, v as u32))
                .unwrap_or("-".into());
            let profile = t
                .GetUINT32(&MF_MT_MPEG2_PROFILE)
                .map(|v| format!("{v}"))
                .unwrap_or("-".into());
            let bitrate = t
                .GetUINT32(&MF_MT_AVG_BITRATE)
                .map(|v| format!("{v}"))
                .unwrap_or("-".into());
            format!("{sub} size={size} fps={fps} profile={profile} bitrate={bitrate}")
        }
    }

    /// 读取 MFT 友好名称。
    fn friendly_name(activate: &IMFActivate) -> String {
        unsafe {
            let mut buf = [0u16; 256];
            if activate
                .GetString(&MFT_FRIENDLY_NAME_Attribute, &mut buf, None)
                .is_ok() {
                let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
                return String::from_utf16_lossy(&buf[..len]);
            }
        }
        "<无名>".to_owned()
    }
}
