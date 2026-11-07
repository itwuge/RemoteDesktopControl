// ============================================================
// 组件用途：控制页工具条的「剪贴板」「文件」两个对话框
// 由谁组装：view/pages/session_page.dart
// 数据来源：SessionController → Rust（rdc-clipboard / rdc-filetransfer）
// 边界：对话框只做参数输入与结果展示；哈希、去重、回环抑制都在 Rust 侧
// ============================================================

import 'dart:io';

import 'package:flutter/material.dart';

import '../../controller/session_controller.dart';
import '../../core/theme/tokens.dart';
import '../../model/gateway/rust_types.dart';
import 'rdc_button.dart';

/// 打开剪贴板同步对话框。
Future<void> showClipboardDialog(
  BuildContext context,
  SessionController controller,
) {
  return showDialog<void>(
    context: context,
    builder: (_) => _ClipboardDialog(controller: controller),
  );
}

/// 打开文件传输对话框。
Future<void> showFileTransferDialog(
  BuildContext context,
  SessionController controller,
) {
  return showDialog<void>(
    context: context,
    builder: (_) => _FileTransferDialog(controller: controller),
  );
}

/// 对话框骨架：统一宽度、标题与内容区。
class _DialogFrame extends StatelessWidget {
  const _DialogFrame({required this.title, required this.child});

  final String title;
  final Widget child;

  @override
  Widget build(BuildContext context) {
    return Dialog(
      backgroundColor: RdcTokens.surface,
      shape: RoundedRectangleBorder(
        borderRadius: BorderRadius.circular(RdcTokens.radiusCard),
      ),
      child: ConstrainedBox(
        constraints: const BoxConstraints(maxWidth: 540, maxHeight: 640),
        child: Padding(
          padding: const EdgeInsets.all(RdcTokens.spaceCard),
          child: Column(
            mainAxisSize: MainAxisSize.min,
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              Row(
                children: [
                  Expanded(
                    child: Text(
                      title,
                      style: const TextStyle(
                        fontSize: RdcTokens.fontTitle,
                        fontWeight: FontWeight.w600,
                      ),
                    ),
                  ),
                  IconButton(
                    icon: const Icon(Icons.close, size: 18),
                    onPressed: () => Navigator.of(context).pop(),
                  ),
                ],
              ),
              const SizedBox(height: RdcTokens.spaceMd),
              Flexible(child: SingleChildScrollView(child: child)),
            ],
          ),
        ),
      ),
    );
  }
}

/// 一行「标签 : 值」。
class _Kv extends StatelessWidget {
  const _Kv(this.label, this.value, {this.color});

  final String label;
  final String value;
  final Color? color;

  @override
  Widget build(BuildContext context) {
    return Padding(
      padding: const EdgeInsets.only(bottom: 6),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          SizedBox(
            width: 92,
            child: Text(
              label,
              style: const TextStyle(
                fontSize: RdcTokens.fontCaption,
                color: RdcTokens.textMuted,
              ),
            ),
          ),
          Expanded(
            child: SelectableText(
              value,
              style: TextStyle(
                fontSize: RdcTokens.fontCaption,
                color: color ?? RdcTokens.textPrimary,
              ),
            ),
          ),
        ],
      ),
    );
  }
}

// =====** 剪贴板同步 =====**

/// 剪贴板对话框：双向文本同步 + 回环演示。
class _ClipboardDialog extends StatefulWidget {
  const _ClipboardDialog({required this.controller});

  final SessionController controller;

  @override
  State<_ClipboardDialog> createState() => _ClipboardDialogState();
}

class _ClipboardDialogState extends State<_ClipboardDialog> {
  final TextEditingController _text = TextEditingController(
    text: '来自控制端的回环测试文本\n第二行用于验证换行归一化',
  );

  ClipboardSyncInfo? _status;
  String _remotePreview = '';
  String _localPreview = '';
  bool _busy = false;
  String? _error;

  SessionController get controller => widget.controller;

  @override
  void initState() {
    super.initState();
    _run(() => controller.startClipboard());
  }

  @override
  void dispose() {
    _text.dispose();
    super.dispose();
  }

  /// 统一执行：串行、置忙、抓错误、回填状态。
  Future<void> _run(Future<ClipboardSyncInfo> Function() action) async {
    if (_busy) return;
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      final status = await action();
      final remote = await controller.clipboardRemoteText();
      final local = await controller.clipboardLocalText();
      if (!mounted) return;
      setState(() {
        _status = status;
        _remotePreview = remote;
        _localPreview = local;
      });
    } catch (e) {
      if (!mounted) return;
      setState(() => _error = '$e');
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  @override
  Widget build(BuildContext context) {
    final status = _status;
    return _DialogFrame(
      title: '剪贴板同步',
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          if (status != null) ...[
            _Kv('服务', status.running ? '运行中' : '未启动',
                color: status.running ? RdcTokens.success : RdcTokens.textMuted),
            _Kv('外发 / 写回', '${status.sent} / ${status.applied}'),
            _Kv('重复抑制 / 回环抑制',
                '${status.skippedDuplicates} / ${status.skippedEchoes}'),
            _Kv('回读校验', status.verified ? '一致' : '未校验',
                color: status.verified ? RdcTokens.success : RdcTokens.textMuted),
            _Kv('最近哈希', status.lastHash),
            if (status.lastError != null)
              _Kv('错误', status.lastError!, color: RdcTokens.danger),
            const SizedBox(height: RdcTokens.spaceMd),
          ],
          if (_error != null) ...[
            _Kv('操作失败', _error!, color: RdcTokens.danger),
            const SizedBox(height: RdcTokens.spaceSm),
          ],
          Wrap(
            spacing: RdcTokens.spaceSm,
            runSpacing: RdcTokens.spaceSm,
            children: [
              RdcButton(
                label: '本机 → 远端',
                icon: Icons.arrow_upward,
                onPressed: _busy ? null : () => _run(controller.clipboardSend),
              ),
              RdcButton(
                label: '远端 → 本机',
                icon: Icons.arrow_downward,
                onPressed: _busy ? null : () => _run(controller.clipboardPull),
              ),
              RdcButton(
                label: '刷新',
                icon: Icons.refresh,
                onPressed: _busy ? null : () => _run(controller.startClipboard),
              ),
            ],
          ),
          const SizedBox(height: RdcTokens.spaceLg),
          const Text(
            '模拟对端写入（防止本机回写形成回环）',
            style: TextStyle(
              fontSize: RdcTokens.fontCaption,
              color: RdcTokens.textMuted,
            ),
          ),
          const SizedBox(height: RdcTokens.spaceSm),
          TextField(
            controller: _text,
            minLines: 2,
            maxLines: 4,
            style: const TextStyle(fontSize: RdcTokens.fontBody),
            decoration: InputDecoration(
              isDense: true,
              contentPadding: const EdgeInsets.all(RdcTokens.spaceSm),
              border: OutlineInputBorder(
                borderRadius: BorderRadius.circular(RdcTokens.radiusControl),
              ),
            ),
          ),
          const SizedBox(height: RdcTokens.spaceSm),
          Align(
            alignment: Alignment.centerLeft,
            child: RdcButton(
              label: '写入远端',
              icon: Icons.edit_outlined,
              compact: true,
              onPressed: _busy
                  ? null
                  : () => _run(() => controller.clipboardSetRemote(_text.text)),
            ),
          ),
          const SizedBox(height: RdcTokens.spaceLg),
          _Kv('远端缓冲', _remotePreview.isEmpty ? '（空）' : _remotePreview),
          _Kv('本机剪贴板', _localPreview.isEmpty ? '（空）' : _localPreview),
        ],
      ),
    );
  }
}

// =====** 文件传输 =====**

/// 文件传输对话框：选路径 → 发送 → 展示两端 SHA-256 与续传信息。
class _FileTransferDialog extends StatefulWidget {
  const _FileTransferDialog({required this.controller});

  final SessionController controller;

  @override
  State<_FileTransferDialog> createState() => _FileTransferDialogState();
}

class _FileTransferDialogState extends State<_FileTransferDialog> {
  final TextEditingController _path = TextEditingController();

  bool _resume = true;
  bool _busy = false;
  FileTransferInfo? _result;
  String? _error;

  SessionController get controller => widget.controller;

  @override
  void dispose() {
    _path.dispose();
    super.dispose();
  }

  /// 生成 10 MiB 示例文件，省去手动找大文件。
  Future<void> _makeSample() async {
    if (_busy) return;
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      final dir = Directory(
        '${Platform.environment['LOCALAPPDATA'] ?? Directory.systemTemp.path}'
        r'\RDC\samples',
      );
      await dir.create(recursive: true);
      final file = File(
        '${dir.path}\\sample-${DateTime.now().millisecondsSinceEpoch}.bin',
      );
      final sink = file.openWrite();
      for (var i = 0; i < 10 * 1024; i++) {
        sink.add(List<int>.generate(1024, (j) => (i + j) % 251));
      }
      await sink.close();
      if (!mounted) return;
      setState(() => _path.text = file.path);
    } catch (e) {
      if (!mounted) return;
      setState(() => _error = '$e');
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  /// 发送；[_resume] 为真时先在 2 块处中断，再由续传补齐。
  Future<void> _send() async {
    if (_busy) return;
    final path = _path.text.trim();
    if (path.isEmpty) {
      setState(() => _error = '请先选择或填入文件路径');
      return;
    }
    setState(() {
      _busy = true;
      _error = null;
      _result = null;
    });
    try {
      final info = await controller.sendFile(
        path,
        interruptAfterChunks: _resume ? 2 : 0,
      );
      if (!mounted) return;
      setState(() => _result = info);
    } catch (e) {
      if (!mounted) return;
      setState(() => _error = '$e');
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  @override
  Widget build(BuildContext context) {
    final result = _result;
    return _DialogFrame(
      title: '发送文件',
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          const Text(
            '文件发往被控端接收目录；落盘后两端 SHA-256 必须一致。',
            style: TextStyle(
              fontSize: RdcTokens.fontCaption,
              color: RdcTokens.textMuted,
            ),
          ),
          const SizedBox(height: RdcTokens.spaceSm),
          TextField(
            controller: _path,
            style: const TextStyle(fontSize: RdcTokens.fontBody),
            decoration: InputDecoration(
              isDense: true,
              hintText: r'例如 C:\Users\Public\demo.bin',
              contentPadding: const EdgeInsets.all(RdcTokens.spaceSm),
              border: OutlineInputBorder(
                borderRadius: BorderRadius.circular(RdcTokens.radiusControl),
              ),
            ),
          ),
          const SizedBox(height: RdcTokens.spaceSm),
          Row(
            children: [
              Checkbox(
                value: _resume,
                onChanged: _busy
                    ? null
                    : (value) => setState(() => _resume = value ?? false),
              ),
              const Expanded(
                child: Text(
                  '模拟中断后自动续传（1 MiB 分块，第 2 块断开）',
                  style: TextStyle(fontSize: RdcTokens.fontCaption),
                ),
              ),
            ],
          ),
          Wrap(
            spacing: RdcTokens.spaceSm,
            runSpacing: RdcTokens.spaceSm,
            children: [
              RdcButton(
                label: '发送',
                icon: Icons.upload_file,
                primary: true,
                onPressed: _busy ? null : _send,
              ),
              RdcButton(
                label: '生成 10 MiB 示例',
                icon: Icons.insert_drive_file_outlined,
                onPressed: _busy ? null : _makeSample,
              ),
            ],
          ),
          if (_error != null) ...[
            const SizedBox(height: RdcTokens.spaceMd),
            _Kv('错误', _error!, color: RdcTokens.danger),
          ],
          if (result != null) ...[
            const SizedBox(height: RdcTokens.spaceLg),
            _Kv('大小 / 分块', '${result.bytes} B / ${result.chunks}'),
            _Kv('两端一致',
                result.equal ? '是（SHA-256 相同）' : '否',
                color: result.equal ? RdcTokens.success : RdcTokens.danger),
            _Kv('续传', result.resumed
                ? '是（跳过 ${result.resumedBytes} B）'
                : '否'),
            _Kv('源 SHA-256', result.sourceSha256),
            _Kv('落盘 SHA-256', result.destSha256),
            _Kv('落盘路径', result.dest),
          ],
        ],
      ),
    );
  }
}
