# S8-3 运行手册：在 Ubuntu 上把 RDC 桌面端跑起来

> 目标：从源码到 `app/build/linux/x64/release/bundle/rdc` 可执行文件，启动后能进控制页、
> 自建本机回环并注入点击。当前验证环境：Ubuntu VM，**无 GPU**（llvmpipe 软件渲染），X11 单屏 1280×800。

## 1. 依赖

- Rust（仓库内 `rust-toolchain.toml` 指定版本）
- Flutter SDK（本项目在 `~/development/flutter`），需要 Linux desktop 支持（`flutter doctor` 的 Linux 项通过）
- `flutter_rust_bridge_codegen` 2.13.0（只在改 `api.rs` 后重新生成绑定时需要）
- 系统库：`libgtk-3-dev`、`libx11-dev`、`libxext-dev`、`libxrandr-dev`、`libxtst-dev`、`libxi-dev`
- `ffmpeg`（需带 `libx264`），本机回环的编解码都调用它
- 测试用工具：`xdotool`、`ffmpeg`（x11grab 截图）

## 2. 构建

```bash
cd ~/rdc-linux
cargo build -p rdc-bridge --release           # 产出 target/release/librdc_bridge.so

cd app
flutter build linux --release                 # 产出 build/linux/x64/release/bundle/
```

`app/linux/runner/CMakeLists.txt` 里有安装规则，会把 `librdc_bridge.so` 复制进
`build/linux/x64/release/bundle/`，所以运行时不需要 `LD_LIBRARY_PATH`。
改过 `crates/rdc-bridge/src/api.rs` 时还要跑一遍 FRB codegen（`frb_generated.rs` 与 `app/lib/src/rust/**` 已同步入库）。

## 3. 启动

```bash
cd ~/rdc-linux/app/build/linux/x64/release/bundle
GDK_SCALE=1 GDK_DPI_SCALE=1 ./rdc
```

- **必须**固定 `GDK_SCALE=1 GDK_DPI_SCALE=1`：GNOME 分数缩放会让 Flutter 的 DPR 与 X11 实际像素不一致，
  表现为点击整体偏移（本机用 100% 缩放复现过）。
- 进「远程控制」页即自动启动回环管线并启动输入通道；不需要额外点按钮。
- Linux 走 RGBA 帧降级通道（`path=rgba-frame`）：Rust 每帧给 RGBA 字节，Dart 侧 `decodeImageFromPixels` 上屏。

## 4. 最小回环验收（同一台机器）

1. 启动 app，点顶部「远程控制」标签。
2. 起一个可被点击验证的目标窗口（仓库已有小工具）：

```bash
~/rdc-linux/target/release/s8_2_target --x 1020 --y 580 --w 200 --h 200 --green 100 \
    --secs 300 --out /tmp/s8-3-target.log
```

3. 在控制页画面里点绿块（预览是整屏镜像）。目标端日志出现
   `PRESS t_ms=... x_root=1120 y_root=680 button=1`，即命中绿块中心（误差 0 px）。

## 5. 常见问题

| 现象 | 处理 |
| --- | --- |
| 窗口被 WM 最大化，`xdotool windowsize` 无效 | 先 `xdotool windowactivate $WID` 再 `xdotool key super+Down`，然后 resize |
| 截图全是登录页 | 会话被锁屏：`loginctl unlock-sessions` + `xset dpms force on` |
| 画面黑屏 / UI 只有部分图元 | VM 无 GPU，Impeller 在 llvmpipe 上不稳定（已知缺陷，见 TESTREPORT E9），换机器或有 GPU 环境复测 |
| 点击位置整体偏移 | 确认 `GDK_SCALE=1 GDK_DPI_SCALE=1`；再确认 `displayRect` 与输入矩阵一致（回归时会打出二者数值） |
| 点击后指针跑飞 | 说明注入回声未被抑制（120 ms 窗口），见 `SessionPage._inEchoWindow` |

## 6. 未验证项

- Wayland（当前只在 X11 下验证）
- 真实多显示器 / XRandR 热插拔
- 有 GPU 时的纹理零拷贝（Linux 仍走 RGBA 降级）
