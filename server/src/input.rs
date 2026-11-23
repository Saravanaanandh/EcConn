//! Input injection: validates `common::InputMsg` datagrams from the client
//! and synthesizes mouse/keyboard events on the captured monitor.
//!
//! Wire format (after the 24-byte frame header): 13 common bytes
//! (msg_type, reserved, seq_id, session_token) followed by type-specific
//! data at bytes 13..17. See `common` for the layout.
//!
//! Session rules: the first non-zero token establishes the session; later
//! datagrams must carry the same token and a strictly newer seq_id
//! (wrapping-safe). If no valid input arrives for 3 s while something is
//! held down, everything is released.
//!
//! Protocol limitation: the Key payload encodes the scancode in 7 bits
//! (bit 7 = key up), so SDL scancodes >= 128 (modifiers such as Ctrl/Shift,
//! which are 224+) cannot be forwarded yet.

use std::time::{Duration, Instant};

use common::{input_msg_decode, InputMsg};

/// Release everything if no valid input arrives for this long.
const INPUT_TIMEOUT: Duration = Duration::from_secs(3);

/// Desktop rectangle of the captured monitor, in virtual-screen pixels.
#[derive(Debug, Clone, Copy)]
pub struct MonitorRect {
    pub left: i32,
    pub top: i32,
    pub width: u32,
    pub height: u32,
}

pub struct InputInjector {
    rect: MonitorRect,
    orientation: u16,
    /// Session token; 0 until the first datagram establishes it.
    token: u64,
    last_seq: u16,
    have_seq: bool,
    /// Currently held mouse buttons (bit 0=left, 1=right, 2=middle).
    pressed_buttons: u8,
    /// Currently held keys (7-bit SDL scancodes).
    pressed_keys: Vec<u8>,
    last_input: Instant,
    received: u64,
    injected: u64,
    rejected: u64,
}

impl InputInjector {
    pub fn new(rect: MonitorRect) -> Self {
        Self {
            rect,
            orientation: 0,
            token: 0,
            last_seq: 0,
            have_seq: false,
            pressed_buttons: 0,
            pressed_keys: Vec::new(),
            last_input: Instant::now(),
            received: 0,
            injected: 0,
            rejected: 0,
        }
    }

    /// Update monitor rectangle on resize.
    pub fn set_rect(&mut self, rect: MonitorRect) {
        self.rect = rect;
    }

    /// Update display orientation.
    pub fn set_orientation(&mut self, orientation: u16) {
        self.orientation = orientation;
    }

    /// Validate and apply one input payload (the bytes after the header).
    pub fn process(&mut self, payload: &[u8]) {
        let Some((msg, seq, token)) = input_msg_decode(payload) else {
            self.rejected += 1;
            return;
        };

        // Session token: first non-zero token wins, then it must match.
        if token == 0 {
            self.rejected += 1;
            return;
        }
        if self.token == 0 {
            self.token = token;
        } else if self.token != token {
            self.rejected += 1;
            return;
        }

        // Anti-replay: accept only strictly newer seq_ids. The signed cast
        // makes the comparison correct across the u16 wrap.
        if self.have_seq && (seq.wrapping_sub(self.last_seq) as i16) <= 0 {
            self.rejected += 1;
            return;
        }
        self.have_seq = true;
        self.last_seq = seq;

        // Per-type payload length checks (data lives at bytes 13..17).
        let needed = match msg {
            InputMsg::MouseMove => 17,
            InputMsg::MouseBtn => 14,
            InputMsg::MouseWheel => 15,
            InputMsg::Key => 14,
            InputMsg::ReleaseAll => 14,
            InputMsg::SetOrientation => 15,
            InputMsg::DisplayConfig => 24,
        };
        if payload.len() < needed {
            self.rejected += 1;
            return;
        }

        self.received += 1;
        self.last_input = Instant::now();

        match msg {
            InputMsg::MouseMove => {
                let x = u16::from_be_bytes([payload[13], payload[14]]);
                let y = u16::from_be_bytes([payload[15], payload[16]]);
                let (mx, my) = match self.orientation {
                    90 if self.rect.width > self.rect.height => (y, 65535 - x),
                    180 => (65535 - x, 65535 - y),
                    270 if self.rect.width > self.rect.height => (65535 - y, x),
                    _ => (x, y),
                };
                self.injected += platform::mouse_move(self.rect, mx, my);
            }
            InputMsg::MouseBtn => {
                let mask = payload[13] & 0x07;
                let changed = self.pressed_buttons ^ mask;
                // Release buttons that went up, press buttons that went down.
                for bit in [0x01u8, 0x02, 0x04] {
                    if changed & bit != 0 {
                        let down = mask & bit != 0;
                        self.injected += platform::mouse_button(bit, down);
                    }
                }
                self.pressed_buttons = mask;
            }
            InputMsg::MouseWheel => {
                let delta = i16::from_be_bytes([payload[13], payload[14]]);
                if delta != 0 {
                    self.injected += platform::mouse_wheel(delta);
                }
            }
            InputMsg::Key => {
                let byte = payload[13];
                let up = byte & 0x80 != 0;
                let scode = byte & 0x7F;
                if up {
                    self.pressed_keys.retain(|&k| k != scode);
                } else if !self.pressed_keys.contains(&scode) {
                    self.pressed_keys.push(scode);
                }
                self.injected += platform::key(scode, up);
            }
            InputMsg::ReleaseAll => {
                self.release_all();
            }
            InputMsg::SetOrientation | InputMsg::DisplayConfig => {}
        }
    }

    /// Called periodically: release everything after an input timeout.
    pub fn tick(&mut self) {
        if self.last_input.elapsed() >= INPUT_TIMEOUT
            && (self.pressed_buttons != 0 || !self.pressed_keys.is_empty())
        {
            eprintln!("input timeout: releasing all keys/buttons");
            self.release_all();
        }
    }

    /// Release every held key and button (shutdown, timeout, ReleaseAll).
    pub fn release_all(&mut self) {
        for bit in [0x01u8, 0x02, 0x04] {
            if self.pressed_buttons & bit != 0 {
                self.injected += platform::mouse_button(bit, false);
            }
        }
        self.pressed_buttons = 0;
        for scode in self.pressed_keys.drain(..) {
            self.injected += platform::key(scode, true);
        }
    }

    /// Read and reset the per-second counters: (received, injected, rejected).
    pub fn take_counters(&mut self) -> (u64, u64, u64) {
        (
            std::mem::take(&mut self.received),
            std::mem::take(&mut self.injected),
            std::mem::take(&mut self.rejected),
        )
    }
}

#[cfg(windows)]
mod platform {
    use super::MonitorRect;
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT, KEYBD_EVENT_FLAGS,
        KEYEVENTF_KEYUP, MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP,
        MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP, MOUSEEVENTF_MOVE, MOUSEEVENTF_RIGHTDOWN,
        MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_VIRTUALDESK, MOUSEEVENTF_WHEEL, MOUSEINPUT, VIRTUAL_KEY,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN,
        SM_YVIRTUALSCREEN,
    };

    /// One wheel notch is 120 units in the Windows API.
    const WHEEL_DELTA: i32 = 120;

    fn send(inputs: &[INPUT]) -> u64 {
        let sent = unsafe { SendInput(inputs, std::mem::size_of::<INPUT>() as i32) };
        sent as u64
    }

    /// Map normalized (0..65535) coordinates onto the captured monitor, then
    /// onto the whole virtual desktop for MOUSEEVENTF_ABSOLUTE | VIRTUALDESK.
    pub fn mouse_move(rect: MonitorRect, nx: u16, ny: u16) -> u64 {
        let w = rect.width.max(1) as i64;
        let h = rect.height.max(1) as i64;
        let px = rect.left as i64 + (nx as i64 * (w - 1)) / 65535;
        let py = rect.top as i64 + (ny as i64 * (h - 1)) / 65535;

        let vx = unsafe { GetSystemMetrics(SM_XVIRTUALSCREEN) } as i64;
        let vy = unsafe { GetSystemMetrics(SM_YVIRTUALSCREEN) } as i64;
        let vw = (unsafe { GetSystemMetrics(SM_CXVIRTUALSCREEN) } as i64).max(1);
        let vh = (unsafe { GetSystemMetrics(SM_CYVIRTUALSCREEN) } as i64).max(1);

        let ax = ((px - vx) * 65535 / (vw - 1).max(1)) as i32;
        let ay = ((py - vy) * 65535 / (vh - 1).max(1)) as i32;

        let input = INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT {
                    dx: ax,
                    dy: ay,
                    mouseData: 0,
                    dwFlags: MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        };
        send(&[input])
    }

    /// `bit`: 0x01 left, 0x02 right, 0x04 middle.
    pub fn mouse_button(bit: u8, down: bool) -> u64 {
        let flags = match (bit, down) {
            (0x01, true) => MOUSEEVENTF_LEFTDOWN,
            (0x01, false) => MOUSEEVENTF_LEFTUP,
            (0x02, true) => MOUSEEVENTF_RIGHTDOWN,
            (0x02, false) => MOUSEEVENTF_RIGHTUP,
            (0x04, true) => MOUSEEVENTF_MIDDLEDOWN,
            (0x04, false) => MOUSEEVENTF_MIDDLEUP,
            _ => return 0,
        };
        let input = INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT {
                    dx: 0,
                    dy: 0,
                    mouseData: 0,
                    dwFlags: flags,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        };
        send(&[input])
    }

    pub fn mouse_wheel(delta: i16) -> u64 {
        let input = INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT {
                    dx: 0,
                    dy: 0,
                    mouseData: (delta as i32 * WHEEL_DELTA) as u32,
                    dwFlags: MOUSEEVENTF_WHEEL,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        };
        send(&[input])
    }

    pub fn key(scode: u8, up: bool) -> u64 {
        let Some(vk) = scancode_to_vk(scode) else {
            return 0;
        };
        let input = INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: VIRTUAL_KEY(vk),
                    wScan: 0,
                    dwFlags: if up { KEYEVENTF_KEYUP } else { KEYBD_EVENT_FLAGS(0) },
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        };
        send(&[input])
    }

    /// SDL (USB HID) scancode -> Windows virtual-key code, for the 7-bit
    /// range the protocol can carry. Returns None for unmapped scancodes.
    fn scancode_to_vk(scode: u8) -> Option<u16> {
        let vk = match scode {
            4..=29 => b'A' as u16 + (scode as u16 - 4),       // A..Z
            30..=38 => b'1' as u16 + (scode as u16 - 30),     // 1..9
            39 => b'0' as u16,                                // 0
            40 => 0x0D,                                       // Return
            41 => 0x1B,                                       // Escape
            42 => 0x08,                                       // Backspace
            43 => 0x09,                                       // Tab
            44 => 0x20,                                       // Space
            45 => 0xBD,                                       // - (OEM_MINUS)
            46 => 0xBB,                                       // = (OEM_PLUS)
            47 => 0xDB,                                       // [ (OEM_4)
            48 => 0xDD,                                       // ] (OEM_6)
            49 => 0xDC,                                       // \ (OEM_5)
            51 => 0xBA,                                       // ; (OEM_1)
            52 => 0xDE,                                       // ' (OEM_7)
            53 => 0xC0,                                       // ` (OEM_3)
            54 => 0xBC,                                       // , (OEM_COMMA)
            55 => 0xBE,                                       // . (OEM_PERIOD)
            56 => 0xBF,                                       // / (OEM_2)
            57 => 0x14,                                       // CapsLock
            58..=69 => 0x70 + (scode as u16 - 58),            // F1..F12
            70 => 0x2C,                                       // PrintScreen
            71 => 0x91,                                       // ScrollLock
            72 => 0x13,                                       // Pause
            73 => 0x2D,                                       // Insert
            74 => 0x24,                                       // Home
            75 => 0x21,                                       // PageUp
            76 => 0x2E,                                       // Delete
            77 => 0x23,                                       // End
            78 => 0x22,                                       // PageDown
            79 => 0x27,                                       // Right
            80 => 0x25,                                       // Left
            81 => 0x28,                                       // Down
            82 => 0x26,                                       // Up
            83 => 0x90,                                       // NumLock
            84 => 0x6F,                                       // KP /
            85 => 0x6A,                                       // KP *
            86 => 0x6D,                                       // KP -
            87 => 0x6B,                                       // KP +
            88 => 0x0D,                                       // KP Enter
            89..=97 => 0x61 + (scode as u16 - 89),            // KP 1..9
            98 => 0x60,                                       // KP 0
            99 => 0x6E,                                       // KP .
            101 => 0x5D,                                      // Application (menu)
            _ => return None,
        };
        Some(vk)
    }
}

#[cfg(not(windows))]
mod platform {
    use super::MonitorRect;
    use std::sync::atomic::{AtomicBool, Ordering};

    static WARNED: AtomicBool = AtomicBool::new(false);

    fn unsupported() -> u64 {
        if !WARNED.swap(true, Ordering::Relaxed) {
            eprintln!(
                "input injection is not implemented on this platform; \
                 input events are counted but not injected"
            );
        }
        0
    }

    pub fn mouse_move(_rect: MonitorRect, _nx: u16, _ny: u16) -> u64 {
        unsupported()
    }
    pub fn mouse_button(_bit: u8, _down: bool) -> u64 {
        unsupported()
    }
    pub fn mouse_wheel(_delta: i16) -> u64 {
        unsupported()
    }
    pub fn key(_scode: u8, _up: bool) -> u64 {
        unsupported()
    }
}