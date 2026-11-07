//! rdc-decode / 构建脚本
//!
//! 层级：L3 编解码层 · 构建期
//! 用途：仅在 `ffmpeg` feature 打开时编译 FFmpeg 解码 C shim，并把 import lib 与
//!       运行期 DLL 准备到可执行文件旁边。默认 feature 关闭，仓库照旧保持
//!       「纯 Rust + 系统 API」的构建前提；启用时必须显式给出 `FFMPEG_DIR`
//!       指向 LGPL **共享**构建（禁止静态链接，禁止 GPL 组件）。
//! 调用方：cargo（编译 rdc-decode 时自动执行）。

use std::env;
use std::path::{Path, PathBuf};

fn main() {
    println!("cargo:rerun-if-changed=shim/ffdec.c");
    println!("cargo:rerun-if-env-changed=FFMPEG_DIR");

    if env::var_os("CARGO_FEATURE_FFMPEG").is_none() {
        return;
    }
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        panic!("ffmpeg 解码后端仅实现 Windows（d3d11va + MF 对照）路径");
    }

    let ffmpeg_dir = env::var("FFMPEG_DIR").unwrap_or_else(|_| {
        panic!(
            "启用 ffmpeg feature 时必须设置 FFMPEG_DIR（指向 LGPL 共享构建根目录，\
             其中含 include/、lib/avcodec.lib、bin/avcodec-*.dll）；\
             本仓库不允许静态链接或 GPL 组件。"
        )
    });
    let root = PathBuf::from(&ffmpeg_dir);
    let include = root.join("include");
    let lib = root.join("lib");
    if !include.join("libavcodec").join("avcodec.h").exists() {
        panic!("FFMPEG_DIR={ffmpeg_dir} 下找不到 include/libavcodec/avcodec.h");
    }

    cc::Build::new()
        .file("shim/ffdec.c")
        .include(&include)
        .opt_level(2)
        // 源文件是 UTF-8（含中文诊断串），MSVC 默认按本地代码页解析会报 C2001。
        .flag_if_supported("-utf-8")
        .warnings(false)
        .compile("rdc_ffdec");

    println!("cargo:rustc-link-search=native={}", lib.display());
    println!("cargo:rustc-link-lib=dylib=avcodec");
    println!("cargo:rustc-link-lib=dylib=avutil");

    stage_runtime(&root);
}

/// 把运行期 DLL 与许可证文本放到 cargo 的 profile 目录（`cargo run` 的 exe 旁边）。
///
/// 仅服务于本机实测；正式打包仍必须按 LGPL 要求随包附 DLL、LICENSE 与源码获取方式。
fn stage_runtime(root: &Path) {
    let Ok(out_dir) = env::var("OUT_DIR") else {
        return;
    };
    let Some(profile_dir) = Path::new(&out_dir).ancestors().nth(3) else {
        return;
    };
    let bin = root.join("bin");
    let Ok(entries) = std::fs::read_dir(&bin) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        // 只带解码必需的最小运行库；其余（avformat/avfilter/swscale/avdevice/ffmpeg.exe）不随包。
        // S7 实测：avcodec-61.dll 只依赖 KERNEL32/msvcrt/avutil-59；avutil-59.dll 还依赖
        // libwinpthread-1.dll（MinGW 构建的线程运行库），三者缺一不可。
        let needed = name.starts_with("avcodec-")
            || name.starts_with("avutil-")
            || name == "libwinpthread-1.dll";
        if needed && name.ends_with(".dll") {
            let _ = std::fs::copy(entry.path(), profile_dir.join(name.as_ref()));
        }
    }
    // 许可文本不在这里重复放置：打包脚本从 packaging/licenses 复制并附构建参数；
    // 本函数只负责让 `cargo run` 能在 exe 旁边找到运行期 DLL。
}
