// ============================================================
// 文件用途：Android 动态库自检（Dart FFI 直调 C 符号）
//
// 层级：model（数据来源层的诊断入口）
// 被谁调用：main.dart（启动时一次，结果写日志）
// 边界：只在 Android 执行；不替代 RustGateway（FRB 仍负责业务调用）
//
// 为什么需要在 FRB 之外再自检一次：
//   FRB 初始化可能因为版本/生成代码不匹配失败，但动态库本身是否进了 APK、
//   符号是否可解析、C ABI 是否可用，需要一条独立可判定的证据链。
// ============================================================

import 'dart:convert';
import 'dart:ffi';
import 'dart:io';

/// 自检结果：成功时 [text] 为库返回的字符串，失败时 [error] 有值。
class AndroidSelfCheckResult {
  /// 创建自检结果。
  const AndroidSelfCheckResult({this.text, this.error});

  /// 库返回的自检字符串（形如 `rdc-bridge selfcheck OK os=android ...`）。
  final String? text;

  /// 失败原因；`null` 表示成功或未执行（非 Android 平台）。
  final String? error;

  /// 是否成功拿到自检字符串。
  bool get passed => error == null && text != null;

  /// 用于日志的单行描述。
  @override
  String toString() {
    if (passed) return 'PASS: $text';
    return 'FAIL: ${error ?? 'unknown'}';
  }
}

/// 执行 Android 自检；非 Android 平台直接返回 `null`（不参与判断）。
///
/// 返回：成功/失败结果对象；调用方只在 Android 上使用。
AndroidSelfCheckResult? runAndroidSelfCheck() {
  // 桌面平台没有 jniLibs 布局，跳过以免产生误导性失败。
  if (!Platform.isAndroid) return null;

  DynamicLibrary? library;
  try {
    // APK 内 jniLibs/<abi>/librdc_bridge.so 由系统按库名解析（无需绝对路径）。
    library = DynamicLibrary.open('librdc_bridge.so');
  } catch (error) {
    return AndroidSelfCheckResult(error: 'dlopen 失败: $error');
  }

  try {
    final symbol = library
        .lookup<NativeFunction<Pointer<Int8> Function()>>('rdc_selfcheck');
    final call = symbol.asFunction<Pointer<Int8> Function()>();

    // 不使用 package:ffi 的 Utf8 类型：项目未声明该依赖，
    // 这里按 NUL 结尾的字节序列直接读取，限制长度防止异常库造成死循环。
    final text = _readCString(call());
    if (text.isEmpty) {
      return const AndroidSelfCheckResult(error: '自检字符串为空');
    }
    return AndroidSelfCheckResult(text: text);
  } catch (error) {
    return AndroidSelfCheckResult(error: '符号调用失败: $error');
  }
}

/// 读取 NUL 结尾的 C 字符串（最多 256 字节）。
///
/// [pointer] 为 C 侧返回的 `const char*`；返回解码后的文本。
String _readCString(Pointer<Int8> pointer) {
  if (pointer == nullptr) return '';
  final bytes = <int>[];
  final unsigned = pointer.cast<Uint8>();
  for (var index = 0; index < 256; index++) {
    final byte = unsigned[index];
    if (byte == 0) break;
    bytes.add(byte);
  }
  // Rust 侧只写 ASCII，utf8 解码不会失败；容错走 allowMalformed。
  return utf8.decode(bytes, allowMalformed: true);
}
