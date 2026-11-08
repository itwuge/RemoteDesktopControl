//! rdc-input / 输入事件线上编码
//!
//! 层级：L3 输入层
//! 用途：把 [`InputEvent`] 编成单事件二进制载荷（小端、定长、无分配解码），供
//!       `StreamKind::Input` 通道传输；serde 派生只服务 FFI，线上不传 JSON。
//! 调用方：rdc-core 的 `rdc-in` 线程（编码后放入 P0 输入队列）、对端注入前解码
//!
//! 通道约定（docs/04 §2）：输入走 `stream=2` / `priority=0`，插队发送、不占令牌桶、
//! 不可丢弃；明文载荷进入既有 AEAD 加密路径（32 字节包头作 AAD），本模块不触碰密钥。

use crate::error::InputError;
use crate::event::{
    InputEvent, KeyEvent, PointerButton, PointerEvent, TouchEvent, TouchPhase, WheelEvent,
};

// =====** 常量 =====**

/// 单事件载荷最大字节数（触摸变体最长），必须小于单包明文上限 1352（docs/04 §1.2）。
pub const MAX_INPUT_EVENT_PAYLOAD: usize = 14;

/// 指针事件 tag。
const TAG_POINTER: u8 = 0;
/// 键盘事件 tag。
const TAG_KEY: u8 = 1;
/// 滚轮事件 tag。
const TAG_WHEEL: u8 = 2;
/// 触摸事件 tag。
const TAG_TOUCH: u8 = 3;

/// 指针事件定长：tag + x(4) + y(4) + button(1) + pressed(1)。
const POINTER_LEN: usize = 11;
/// 键盘事件定长：tag + scan_code(2) + pressed(1) + modifiers(1)。
const KEY_LEN: usize = 5;
/// 滚轮事件定长：tag + delta_x(4) + delta_y(4)。
const WHEEL_LEN: usize = 9;
/// 触摸事件定长：tag + id(4) + x(4) + y(4) + phase(1)。
const TOUCH_LEN: usize = 14;

// =====** 编码 =====**

/// 把单个输入事件编码为线上载荷。
///
/// # Arguments
/// * `event` - 已完成坐标映射的输入事件
///
/// # Returns
/// 长度 5~14 字节的小端载荷；长度由事件类型决定，不做填充。
///
/// # Errors
/// 不会失败；`Vec` 容量按事件类型精确预留，不产生额外分配。
pub fn encode_event(event: &InputEvent) -> Vec<u8> {
    match event {
        InputEvent::Pointer(p) => {
            let mut out = Vec::with_capacity(POINTER_LEN);
            out.push(TAG_POINTER);
            out.extend_from_slice(&p.x.to_le_bytes());
            out.extend_from_slice(&p.y.to_le_bytes());
            out.push(button_code(&p.button));
            out.push(u8::from(p.pressed));
            out
        }
        InputEvent::Key(k) => {
            let mut out = Vec::with_capacity(KEY_LEN);
            out.push(TAG_KEY);
            out.extend_from_slice(&k.scan_code.to_le_bytes());
            out.push(u8::from(k.pressed));
            out.push(k.modifiers);
            out
        }
        InputEvent::Wheel(w) => {
            let mut out = Vec::with_capacity(WHEEL_LEN);
            out.push(TAG_WHEEL);
            out.extend_from_slice(&w.delta_x.to_le_bytes());
            out.extend_from_slice(&w.delta_y.to_le_bytes());
            out
        }
        InputEvent::Touch(t) => {
            let mut out = Vec::with_capacity(TOUCH_LEN);
            out.push(TAG_TOUCH);
            out.extend_from_slice(&t.id.to_le_bytes());
            out.extend_from_slice(&t.x.to_bits().to_le_bytes());
            out.extend_from_slice(&t.y.to_bits().to_le_bytes());
            out.push(touch_phase_code(&t.phase));
            out
        }
    }
}

// =====** 解码 =====**

/// 从线上载荷还原输入事件。
///
/// # Arguments
/// * `bytes` - 单事件载荷（不含包头与 AEAD tag）
///
/// # Returns
/// 还原后的事件；字段值逐字节一致。
///
/// # Errors
/// 载荷长度与 tag 不匹配、tag 未知、布尔/枚举字节非法时返回
/// [`InputError::MalformedPayload`]；不 panic、不做部分解析。
pub fn decode_event(bytes: &[u8]) -> Result<InputEvent, InputError> {
    let (tag, body) = bytes.split_first().ok_or(InputError::MalformedPayload)?;
    match *tag {
        TAG_POINTER => {
            expect_len(body, POINTER_LEN - 1)?;
            let x = read_i32(&body[0..4])?;
            let y = read_i32(&body[4..8])?;
            let button = match body[8] {
                0 => None,
                1 => Some(PointerButton::Left),
                2 => Some(PointerButton::Right),
                3 => Some(PointerButton::Middle),
                _ => return Err(InputError::MalformedPayload),
            };
            let pressed = read_bool(body[9])?;
            Ok(InputEvent::Pointer(PointerEvent {
                x,
                y,
                button,
                pressed,
            }))
        }
        TAG_KEY => {
            expect_len(body, KEY_LEN - 1)?;
            let scan_code = u16::from_le_bytes([body[0], body[1]]);
            let pressed = read_bool(body[2])?;
            Ok(InputEvent::Key(KeyEvent {
                scan_code,
                pressed,
                modifiers: body[3],
            }))
        }
        TAG_WHEEL => {
            expect_len(body, WHEEL_LEN - 1)?;
            let delta_x = read_i32(&body[0..4])?;
            let delta_y = read_i32(&body[4..8])?;
            Ok(InputEvent::Wheel(WheelEvent { delta_x, delta_y }))
        }
        TAG_TOUCH => {
            expect_len(body, TOUCH_LEN - 1)?;
            let id = u32::from_le_bytes([body[0], body[1], body[2], body[3]]);
            let x = f32::from_bits(u32::from_le_bytes([body[4], body[5], body[6], body[7]]));
            let y = f32::from_bits(u32::from_le_bytes([body[8], body[9], body[10], body[11]]));
            let phase = match body[12] {
                0 => TouchPhase::Down,
                1 => TouchPhase::Move,
                2 => TouchPhase::Up,
                3 => TouchPhase::Cancel,
                _ => return Err(InputError::MalformedPayload),
            };
            Ok(InputEvent::Touch(TouchEvent { id, x, y, phase }))
        }
        _ => Err(InputError::MalformedPayload),
    }
}

// =====** 内部辅助 =====**

/// 指针按钮的线上编码。
fn button_code(button: &Option<PointerButton>) -> u8 {
    match button {
        None => 0,
        Some(PointerButton::Left) => 1,
        Some(PointerButton::Right) => 2,
        Some(PointerButton::Middle) => 3,
    }
}

/// 触摸阶段的线上编码。
fn touch_phase_code(phase: &TouchPhase) -> u8 {
    match phase {
        TouchPhase::Down => 0,
        TouchPhase::Move => 1,
        TouchPhase::Up => 2,
        TouchPhase::Cancel => 3,
    }
}

/// 校验载荷长度（不含 tag）。
fn expect_len(body: &[u8], expected: usize) -> Result<(), InputError> {
    if body.len() == expected {
        Ok(())
    } else {
        Err(InputError::MalformedPayload)
    }
}

/// 读取 4 字节小端 i32；长度由 [`expect_len`] 先行保证，越界返回载荷非法。
fn read_i32(bytes: &[u8]) -> Result<i32, InputError> {
    // [`expect_len`] 已保证调用点传入 4 字节切片，不存在转换失败分支。
    let mut arr = [0u8; 4];
    arr.copy_from_slice(bytes);
    Ok(i32::from_le_bytes(arr))
}

/// 读取线上布尔；只接受 0/1。
fn read_bool(byte: u8) -> Result<bool, InputError> {
    match byte {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(InputError::MalformedPayload),
    }
}

// =====** 自检 =====**

#[cfg(test)]
mod tests {
    use super::*;

    /// 构造四种变体，覆盖按钮/阶段枚举的全部取值。
    fn samples() -> Vec<InputEvent> {
        vec![
            InputEvent::Pointer(PointerEvent {
                x: -1920,
                y: 1079,
                button: None,
                pressed: false,
            }),
            InputEvent::Pointer(PointerEvent {
                x: 960,
                y: 540,
                button: Some(PointerButton::Left),
                pressed: true,
            }),
            InputEvent::Pointer(PointerEvent {
                x: 0,
                y: 0,
                button: Some(PointerButton::Right),
                pressed: false,
            }),
            InputEvent::Pointer(PointerEvent {
                x: 7,
                y: 9,
                button: Some(PointerButton::Middle),
                pressed: true,
            }),
            InputEvent::Key(KeyEvent {
                scan_code: 0xE048,
                pressed: true,
                modifiers: 0b0011,
            }),
            InputEvent::Key(KeyEvent {
                scan_code: 0x1C,
                pressed: false,
                modifiers: 0,
            }),
            InputEvent::Wheel(WheelEvent {
                delta_x: -3,
                delta_y: 5,
            }),
            InputEvent::Wheel(WheelEvent {
                delta_x: 0,
                delta_y: 0,
            }),
            InputEvent::Touch(TouchEvent {
                id: 42,
                x: 0.25,
                y: 0.75,
                phase: TouchPhase::Down,
            }),
            InputEvent::Touch(TouchEvent {
                id: 1,
                x: 1.0,
                y: 0.0,
                phase: TouchPhase::Cancel,
            }),
        ]
    }

    /// 四变体逐字节往返，且长度不超过单包上限。
    #[test]
    fn all_variants_roundtrip() {
        for event in samples() {
            let bytes = encode_event(&event);
            assert!(
                bytes.len() <= MAX_INPUT_EVENT_PAYLOAD,
                "载荷超长: {bytes:?}"
            );
            let decoded = decode_event(&bytes).expect("解码");
            assert_eq!(decoded, event);
            // 再编一次必须逐字节相同，保证两端一致。
            assert_eq!(encode_event(&decoded), bytes);
        }
    }

    /// 指针/键盘/滚轮/触摸的固定长度必须与常量一致。
    #[test]
    fn lengths_match_tags() {
        let ptr = InputEvent::Pointer(PointerEvent {
            x: 1,
            y: 2,
            button: None,
            pressed: false,
        });
        let key = InputEvent::Key(KeyEvent {
            scan_code: 0x1C,
            pressed: true,
            modifiers: 0,
        });
        let wheel = InputEvent::Wheel(WheelEvent {
            delta_x: 1,
            delta_y: -1,
        });
        let touch = InputEvent::Touch(TouchEvent {
            id: 1,
            x: 0.5,
            y: 0.5,
            phase: TouchPhase::Move,
        });
        assert_eq!(encode_event(&ptr).len(), POINTER_LEN);
        assert_eq!(encode_event(&key).len(), KEY_LEN);
        assert_eq!(encode_event(&wheel).len(), WHEEL_LEN);
        assert_eq!(encode_event(&touch).len(), TOUCH_LEN);
    }

    /// 空载荷、截断、多余字节、未知 tag 都必须拒绝而不是 panic。
    #[test]
    fn malformed_payloads_are_rejected() {
        assert!(decode_event(&[]).is_err());
        let key = encode_event(&InputEvent::Key(KeyEvent {
            scan_code: 0x1C,
            pressed: true,
            modifiers: 0,
        }));
        assert!(decode_event(&key[..key.len() - 1]).is_err());
        let mut long = key.clone();
        long.push(0);
        assert!(decode_event(&long).is_err());
        assert!(decode_event(&[0x7F, 1, 2, 3]).is_err());
        // 非法布尔/枚举字节。
        let mut bad_bool = encode_event(&InputEvent::Pointer(PointerEvent {
            x: 0,
            y: 0,
            button: None,
            pressed: false,
        }));
        *bad_bool.last_mut().expect("非空") = 2;
        assert!(decode_event(&bad_bool).is_err());
    }
}
