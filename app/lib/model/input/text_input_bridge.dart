// =====** 文本输入（IME）桥 =====**
//
// 职责：把 Flutter 的 `TextInputConnection` 编辑流转换成远端可注入的
// “删除 N 个 UTF-16 码元 + 插入一段文本”事件（见 crates/rdc-input/src/event.rs）。
//
// 为什么不用扫描码：中文/日文输入法的组合串、候选词没有稳定扫描码，
// 逐扫描码模拟只能打出英文；UNICODE 注入路径才能覆盖任意文本。
//
// 该文件只依赖 Flutter SDK，不依赖 Rust 生成代码，便于单元测试。

import 'package:flutter/services.dart';

/// 单条文本事件正文上限（UTF-8 字节）。
///
/// 与 `crates/rdc-input/src/wire.rs` 的 `MAX_TEXT_EVENT_BYTES` 保持一致：
/// 单包明文 1352 - S4 消息 tag 1 - 文本事件头 3 = 1348。
const int kMaxTextEventBytes = 1348;

/// 单次可删除的 UTF-16 码元上限（协议字段为 u16）。
const int kMaxDeleteUnits = 65535;

/// 文本事件能力位（与 `crates/rdc-protocol/src/capabilities.rs` 的
/// `bits::TEXT_INPUT = 1 << 13` 保持一致）。
///
/// 协商结果没有该位时，本地输入法只保留组合与候选，不把文本发往远端。
const int kTextInputCapabilityBit = 1 << 13;

// =====** TextEdit =====**

/// 一次远端文本编辑：先删除 [deleteUnits] 个 UTF-16 码元，再插入 [insert]。
class TextEdit {
  /// 创建编辑。
  const TextEdit({required this.deleteUnits, required this.insert});

  /// 插入前先发送的退格次数（UTF-16 码元数）。
  final int deleteUnits;

  /// 要插入的文本；空串表示纯删除。
  final String insert;

  /// 是否为空操作。
  bool get isNoop => deleteUnits == 0 && insert.isEmpty;

  @override
  String toString() => 'TextEdit(delete=$deleteUnits, insert="$insert")';

  @override
  bool operator ==(Object other) =>
      identical(this, other) ||
      other is TextEdit &&
          other.deleteUnits == deleteUnits &&
          other.insert == insert;

  @override
  int get hashCode => Object.hash(deleteUnits, insert);
}

// =====** 差分 =====**

/// 计算两次编辑值之间的最小“删除 + 插入”。
///
/// 取最长公共前缀即可覆盖三种常见场景：
/// - 逐字输入：前缀增长，删除 0；
/// - 退格：公共前缀缩短，删除差值、插入空串；
/// - 输入法组合串被候选词替换（`"zhong"` → `"中"`）：前缀为 0，全部删除再插入。
///
/// 公共前缀按 UTF-16 码元比较，但不会停在高/低代理项之间：否则会把一个
/// emoji 拆成两次注入，远端会看到两个半字符。
///
/// # Arguments
/// * `previous` - 上一次的编辑值
/// * `next` - 本次的编辑值
///
/// # Returns
/// 空操作时返回 `null`，否则返回最小编辑。
TextEdit? diffTextEditingValue(
  TextEditingValue previous,
  TextEditingValue next,
) {
  final before = previous.text;
  final after = next.text;
  if (before == after) {
    return null;
  }

  var prefix = 0;
  final maxPrefix = before.length < after.length ? before.length : after.length;
  while (prefix < maxPrefix &&
      before.codeUnitAt(prefix) == after.codeUnitAt(prefix)) {
    prefix++;
  }
  // 不要把代理对劈开：若前缀末尾是高位代理且下一位是低位代理，回退一位。
  if (prefix > 0 && prefix < before.length && prefix < after.length) {
    final last = before.codeUnitAt(prefix - 1);
    final following = before.codeUnitAt(prefix);
    if (_isHighSurrogate(last) && _isLowSurrogate(following)) {
      prefix -= 1;
    }
  }

  final deleteUnits = before.length - prefix;
  final insert = after.substring(prefix);
  if (deleteUnits == 0 && insert.isEmpty) {
    return null;
  }
  return TextEdit(deleteUnits: deleteUnits, insert: insert);
}

/// 高位代理项（U+D800..U+DBFF）。
bool _isHighSurrogate(int unit) => unit >= 0xD800 && unit <= 0xDBFF;

/// 低位代理项（U+DC00..U+DFFF）。
bool _isLowSurrogate(int unit) => unit >= 0xDC00 && unit <= 0xDFFF;

// =====** 分段 =====**

/// 把一次编辑拆成多条满足协议上限的事件。
///
/// 删除按 [kMaxDeleteUnits] 切；插入按 UTF-8 字节上限 [maxBytes] 切。
/// 正文按 `runes` 遍历，天然不会拆开代理对（emoji 始终落在同一段）。
/// 删除与插入的先后关系保持不变：先发完所有删除，再发插入。
///
/// # Arguments
/// * `edit` - 待发送的编辑
/// * `maxBytes` - 单条正文上限，默认 [kMaxTextEventBytes]
///
/// # Returns
/// 可直接逐条发送的事件列表；空操作返回空列表。
List<TextEdit> splitTextEdit(TextEdit edit, {int maxBytes = kMaxTextEventBytes}) {
  if (edit.isNoop) {
    return const <TextEdit>[];
  }
  final chunks = <TextEdit>[];

  var remainingDelete = edit.deleteUnits;
  while (remainingDelete > kMaxDeleteUnits) {
    chunks.add(const TextEdit(deleteUnits: kMaxDeleteUnits, insert: ''));
    remainingDelete -= kMaxDeleteUnits;
  }

  // 插入分段：每段累加 UTF-8 字节数，超限前收口。
  final segments = <String>[];
  final buffer = StringBuffer();
  var bytes = 0;
  for (final rune in edit.insert.runes) {
    final runeBytes = _utf8Length(rune);
    if (bytes + runeBytes > maxBytes && buffer.isNotEmpty) {
      segments.add(buffer.toString());
      buffer.clear();
      bytes = 0;
    }
    buffer.writeCharCode(rune);
    bytes += runeBytes;
  }
  if (buffer.isNotEmpty) {
    segments.add(buffer.toString());
  }

  // 第一段带上剩余的删除量；若没有插入，则删除单独成一条。
  if (segments.isEmpty) {
    if (remainingDelete > 0) {
      chunks.add(TextEdit(deleteUnits: remainingDelete, insert: ''));
    }
    return chunks;
  }
  for (var index = 0; index < segments.length; index++) {
    final delete = index == 0 ? remainingDelete : 0;
    if (delete == 0 && segments[index].isEmpty) {
      continue;
    }
    chunks.add(TextEdit(deleteUnits: delete, insert: segments[index]));
  }
  return chunks;
}

/// 单个 Unicode 标量的 UTF-8 字节数。
int _utf8Length(int rune) {
  if (rune <= 0x7F) {
    return 1;
  }
  if (rune <= 0x7FF) {
    return 2;
  }
  if (rune <= 0xFFFF) {
    return 3;
  }
  return 4;
}

// =====** RemoteTextInput =====**

/// 远端文本框：接管本地 IME 的编辑流，把差分逐段交给 [onEdit]。
///
/// 典型用法：Android 会话页在“软键盘”按钮按下时 [attach]，收起时 [detach]。
/// 本类不持有远端桌面坐标，也不渲染任何 UI，只负责文本事件。
class RemoteTextInput with TextInputClient {
  /// 创建远端文本框。
  ///
  /// # Arguments
  /// * `onEdit` - 每条已分段的文本事件回调（调用方负责发给 Rust）
  RemoteTextInput({required this.onEdit});

  /// 已分段文本事件回调。
  final void Function(TextEdit edit) onEdit;

  TextInputConnection? _connection;
  TextEditingValue _value = TextEditingValue.empty;

  /// 是否已连接输入法。
  bool get isAttached => _connection?.attached ?? false;

  /// 当前本地编辑值（用于测试与状态恢复）。
  TextEditingValue get editingValue => _value;

  /// 连接输入法并显示软键盘。
  void attach() {
    if (isAttached) {
      return;
    }
    _connection = TextInput.attach(
      this,
      const TextInputConfiguration(
        inputType: TextInputType.text,
        autocorrect: false,
        enableSuggestions: false,
        textCapitalization: TextCapitalization.none,
      ),
    );
    _connection!
      ..setEditingState(_value)
      ..show();
  }

  /// 断开输入法（收起软键盘）；不发送任何编辑。
  void detach() {
    _connection?.close();
    _connection = null;
  }

  /// 清空本地缓存（远端窗口切换时调用，避免下一次差分基于旧文本）。
  void reset() {
    _value = TextEditingValue.empty;
    if (isAttached) {
      _connection!.setEditingState(_value);
    }
  }

  /// 直接提交一段文本（例如回车、粘贴），会走同一套分段逻辑。
  void submit(String text, {int deleteUnits = 0}) {
    final edit = TextEdit(deleteUnits: deleteUnits, insert: text);
    for (final chunk in splitTextEdit(edit)) {
      onEdit(chunk);
    }
    final keep = (_value.text.length - deleteUnits).clamp(0, _value.text.length);
    final merged = '${_value.text.substring(0, keep)}$text';
    _value = TextEditingValue(
      text: merged,
      selection: TextSelection.collapsed(offset: merged.length),
    );
    if (isAttached) {
      _connection!.setEditingState(_value);
    }
  }

  // =====** TextInputClient 接口 =====**

  @override
  TextEditingValue? get currentTextEditingValue => _value;

  @override
  AutofillScope? get currentAutofillScope => null;

  @override
  void updateEditingValue(TextEditingValue value) {
    final edit = diffTextEditingValue(_value, value);
    _value = value;
    if (edit == null) {
      return;
    }
    for (final chunk in splitTextEdit(edit)) {
      onEdit(chunk);
    }
  }

  @override
  void performAction(TextInputAction action) {
    // 远端桌面没有“表单动作”概念，只有真正产生字符的动作才上报。
    if (action == TextInputAction.newline) {
      submit('\n');
    }
  }

  @override
  void performPrivateCommand(String action, Map<String, dynamic> data) {
    // 输入法私有命令（如手写/语音的厂商扩展）不跨设备转发。
  }

  @override
  void updateFloatingCursor(RawFloatingCursorPoint point) {
    // 远端桌面不支持浮标光标，忽略。
  }

  @override
  void showAutocorrectionPromptRect(int start, int end) {
    // 远端窗口自行渲染候选框，本地不显示提示。
  }

  @override
  void connectionClosed() {
    _connection = null;
  }
}
