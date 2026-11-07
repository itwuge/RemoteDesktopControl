// ============================================================
// 组件用途：控制页工具条的「剪贴板」「文件」两个对话框
// 由谁组装：view/pages/session_page.dart
// 数据来源：SessionController → Rust（rdc-clipboard / rdc-filetransfer）
// 边界：对话框只做参数输入与结果展示；哈希、去重、回环抑制都在 Rust 侧
// 依据：清单 P0-10 / G9 / P1-15（主面板只留主操作，诊断信息折叠收纳）
// ============================================================

import 'package:flutter/material.dart';

import '../../controller/session_controller.dart';
import '../../core/l10n/strings.dart';
import '../../core/theme/tokens.dart';
import '../../model/gateway/rust_types.dart';
import 'fields.dart';
import 'rdc_button.dart';
import 'rdc_dialog.dart';

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

// =====** 剪贴板同步 =====**

/// 剪贴板对话框：双向文本同步。
class _ClipboardDialog extends StatefulWidget {
  const _ClipboardDialog({required this.controller});

  final SessionController controller;

  @override
  State<_ClipboardDialog> createState() => _ClipboardDialogState();
}

class _ClipboardDialogState extends State<_ClipboardDialog> {
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
    return RdcDialog(
      title: AppStrings.clipboard,
      width: RdcTokens.dialogWidthLarge,
      padding: RdcTokens.dialogPaddingLarge,
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          if (_error != null) ...[
            Text(
              _error!,
              style: const TextStyle(
                fontSize: RdcTokens.fontCaption,
                color: RdcTokens.danger,
              ),
            ),
            const SizedBox(height: RdcTokens.spaceMd),
          ],
          // 主面板只保留一个方向选择，两个按钮就是全部主操作。
          Wrap(
            spacing: RdcTokens.spaceSm,
            runSpacing: RdcTokens.spaceSm,
            children: [
              RdcButton(
                label: AppStrings.copyToRemote,
                icon: Icons.arrow_upward,
                onPressed: _busy ? null : () => _run(controller.clipboardSend),
              ),
              RdcButton(
                label: AppStrings.copyToLocal,
                icon: Icons.arrow_downward,
                onPressed: _busy ? null : () => _run(controller.clipboardPull),
              ),
            ],
          ),
          const SizedBox(height: RdcTokens.spaceLg),
          _buildPreview(
            '对方剪贴板',
            _remotePreview.isEmpty ? '（空）' : _remotePreview,
          ),
          const SizedBox(height: RdcTokens.spaceSm),
          _buildPreview(
            '本机剪贴板',
            _localPreview.isEmpty ? '（空）' : _localPreview,
          ),
          if (status != null) ...[
            const SizedBox(height: RdcTokens.spaceSm),
            // 哈希 / 去重 / 回环计数只给排障用，默认收起（清单 P0-10）。
            RdcDiagnostics(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  _kv('服务', status.running ? '运行中' : '未启动'),
                  _kv('外发 / 写回', '${status.sent} / ${status.applied}'),
                  _kv(
                    '重复抑制 / 回环抑制',
                    '${status.skippedDuplicates} / ${status.skippedEchoes}',
                  ),
                  _kv('回读校验', status.verified ? '一致' : '未校验'),
                  _kv('最近哈希', status.lastHash),
                  if (status.lastError != null)
                    _kv('错误', status.lastError!, color: RdcTokens.danger),
                ],
              ),
            ),
          ],
        ],
      ),
    );
  }

  /// 最近一条文本预览（最多 4 行，超出省略）。
  Widget _buildPreview(String label, String value) {
    final empty = value == '（空）';
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Text(
          label,
          style: const TextStyle(
            fontSize: RdcTokens.fontCaption,
            color: RdcTokens.textMuted,
          ),
        ),
        const SizedBox(height: 6),
        Container(
          width: double.infinity,
          padding: const EdgeInsets.all(RdcTokens.spaceSm),
          decoration: BoxDecoration(
            color: RdcTokens.surfaceRaised,
            borderRadius: BorderRadius.circular(RdcTokens.radiusControl),
            border: Border.all(color: RdcTokens.border),
          ),
          child: Text(
            value,
            maxLines: 4,
            overflow: TextOverflow.ellipsis,
            style: TextStyle(
              fontSize: RdcTokens.fontBody,
              color: empty ? RdcTokens.textMuted : RdcTokens.textPrimary,
              height: 1.4,
            ),
          ),
        ),
      ],
    );
  }

  /// 诊断行。
  Widget _kv(String label, String value, {Color? color}) {
    return Padding(
      padding: const EdgeInsets.only(bottom: 4),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          SizedBox(
            width: 110,
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
                color: color ?? RdcTokens.textMuted,
              ),
            ),
          ),
        ],
      ),
    );
  }
}

// =====** 文件传输 =====**

/// 文件传输对话框：选路径 → 发送 → 结果与诊断。
class _FileTransferDialog extends StatefulWidget {
  const _FileTransferDialog({required this.controller});

  final SessionController controller;

  @override
  State<_FileTransferDialog> createState() => _FileTransferDialogState();
}

class _FileTransferDialogState extends State<_FileTransferDialog> {
  final TextEditingController _path = TextEditingController();

  bool _busy = false;
  FileTransferInfo? _result;
  String? _error;

  SessionController get controller => widget.controller;

  @override
  void dispose() {
    _path.dispose();
    super.dispose();
  }

  /// 发送文件到被控端。
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
      final info = await controller.sendFile(path);
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
    return RdcDialog(
      title: AppStrings.files,
      width: RdcTokens.dialogWidthLarge,
      padding: RdcTokens.dialogPaddingLarge,
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          const Text(
            '文件将发送到被控端的接收目录。',
            style: TextStyle(
              fontSize: RdcTokens.fontCaption,
              color: RdcTokens.textMuted,
            ),
          ),
          const SizedBox(height: RdcTokens.spaceMd),
          RdcField(
            label: '本机文件路径',
            expand: false,
            child: RdcTextField(
              controller: _path,
              hint: r'例如 C:\Users\Public\demo.bin',
            ),
          ),
          const SizedBox(height: RdcTokens.spaceMd),
          Align(
            alignment: Alignment.centerLeft,
            child: RdcButton(
              label: _busy ? '发送中…' : '发送',
              icon: Icons.upload_file,
              primary: true,
              onPressed: _busy ? null : _send,
            ),
          ),
          if (_busy) ...[
            const SizedBox(height: RdcTokens.spaceMd),
            const LinearProgressIndicator(minHeight: 4),
          ],
          if (_error != null) ...[
            const SizedBox(height: RdcTokens.spaceMd),
            Text(
              _error!,
              style: const TextStyle(
                fontSize: RdcTokens.fontCaption,
                color: RdcTokens.danger,
              ),
            ),
          ],
          if (result != null) ...[
            const SizedBox(height: RdcTokens.spaceLg),
            Row(
              children: [
                Icon(
                  result.equal ? Icons.check_circle : Icons.error_outline,
                  size: 18,
                  color: result.equal ? RdcTokens.success : RdcTokens.danger,
                ),
                const SizedBox(width: 8),
                Text(
                  result.equal ? '发送完成 · 两端一致' : '发送完成 · 校验不一致',
                  style: TextStyle(
                    fontSize: RdcTokens.fontBody,
                    fontWeight: FontWeight.w600,
                    color: result.equal ? RdcTokens.success : RdcTokens.danger,
                  ),
                ),
              ],
            ),
            const SizedBox(height: RdcTokens.spaceSm),
            Text(
              '大小 ${result.bytes} B · 分块 ${result.chunks}'
              '${result.resumed ? ' · 已自动续传' : ''}',
              style: const TextStyle(
                fontSize: RdcTokens.fontCaption,
                color: RdcTokens.textMuted,
              ),
            ),
            const SizedBox(height: 4),
            Text(
              '落盘位置：${result.dest}',
              style: const TextStyle(
                fontSize: RdcTokens.fontCaption,
                color: RdcTokens.textMuted,
              ),
            ),
            // SHA-256 只在需要核对时展开，不占据主面板（清单 P0-10）。
            RdcDiagnostics(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  _kv('源 SHA-256', result.sourceSha256),
                  _kv('落盘 SHA-256', result.destSha256),
                  _kv('断点续传', result.resumed
                      ? '是（跳过 ${result.resumedBytes} B）'
                      : '否'),
                ],
              ),
            ),
          ],
        ],
      ),
    );
  }

  /// 诊断行。
  Widget _kv(String label, String value) {
    return Padding(
      padding: const EdgeInsets.only(bottom: 4),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          SizedBox(
            width: 96,
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
              style: const TextStyle(
                fontSize: RdcTokens.fontCaption,
                color: RdcTokens.textMuted,
              ),
            ),
          ),
        ],
      ),
    );
  }
}
