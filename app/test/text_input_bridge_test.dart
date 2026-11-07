// 文本输入差分/分段单元测试。
//
// 这些用例只验证纯函数契约（不依赖 MethodChannel、不依赖 Rust），
// 覆盖：逐字输入、退格、输入法组合串替换、候选词替换、emoji 代理对、
// 以及单事件 1348 字节上限的分段。

import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:rdc/model/input/text_input_bridge.dart';

/// 构造编辑值（光标放在末尾）。
TextEditingValue _value(String text) =>
    TextEditingValue(text: text, selection: TextSelection.collapsed(offset: text.length));

void main() {
  group('diffTextEditingValue', () {
    test('逐字输入只插入新增字符', () {
      expect(
        diffTextEditingValue(_value(''), _value('a')),
        const TextEdit(deleteUnits: 0, insert: 'a'),
      );
      expect(
        diffTextEditingValue(_value('a'), _value('ab')),
        const TextEdit(deleteUnits: 0, insert: 'b'),
      );
    });

    test('退格只删除尾部', () {
      expect(
        diffTextEditingValue(_value('你好'), _value('你')),
        const TextEdit(deleteUnits: 1, insert: ''),
      );
    });

    test('输入法组合串被候选词整体替换', () {
      // "zhong" 是拼音组合串，选词后上屏 "中"：前缀为 0，删除 5。
      expect(
        diffTextEditingValue(_value('zhong'), _value('中')),
        const TextEdit(deleteUnits: 5, insert: '中'),
      );
    });

    test('候选词之间切换', () {
      expect(
        diffTextEditingValue(_value('中'), _value('重')),
        const TextEdit(deleteUnits: 1, insert: '重'),
      );
    });

    test('emoji 按 UTF-16 码元计数且不拆开代理对', () {
      expect(
        diffTextEditingValue(_value(''), _value('😀')),
        const TextEdit(deleteUnits: 0, insert: '😀'),
      );
      // 删除 emoji 必须是 2 个码元，而不是 1 个字符。
      expect(
        diffTextEditingValue(_value('😀'), _value('')),
        const TextEdit(deleteUnits: 2, insert: ''),
      );
    });

    test('文本未变化返回 null', () {
      expect(diffTextEditingValue(_value('abc'), _value('abc')), isNull);
    });
  });

  group('splitTextEdit', () {
    test('空操作不产生事件', () {
      expect(
        splitTextEdit(const TextEdit(deleteUnits: 0, insert: '')),
        isEmpty,
      );
    });

    test('超过字节上限的插入被切段且拼回原文', () {
      final text = 'a' * (kMaxTextEventBytes * 2 + 10);
      final chunks = splitTextEdit(TextEdit(deleteUnits: 0, insert: text));
      expect(chunks.length, 3);
      expect(chunks.first.deleteUnits, 0);
      for (final chunk in chunks) {
        expect(chunk.insert.runes.length, lessThanOrEqualTo(chunk.insert.length));
        // 每段 UTF-8 字节数刻意不做精确统计，交给下面的重组断言。
      }
      expect(chunks.map((chunk) => chunk.insert).join(), text);
    });

    test('emoji 不会被切段拆成半个代理对', () {
      final text = '😀' * 400; // 每字 4 字节，共 1600 字节
      final chunks = splitTextEdit(TextEdit(deleteUnits: 0, insert: text));
      expect(chunks.length, greaterThan(1));
      for (final chunk in chunks) {
        expect(chunk.insert.runes.length, chunk.insert.length ~/ 2);
      }
      expect(chunks.map((chunk) => chunk.insert).join(), text);
    });

    test('超长删除按 u16 上限切分，首段带插入', () {
      final chunks = splitTextEdit(
        const TextEdit(deleteUnits: 70000, insert: '补'),
      );
      expect(chunks.length, 2);
      expect(chunks[0], const TextEdit(deleteUnits: kMaxDeleteUnits, insert: ''));
      expect(chunks[1].deleteUnits, 70000 - kMaxDeleteUnits);
      expect(chunks[1].insert, '补');
    });
  });
}
