// Shared protocol types for the `lowlat-screen` workspace.

// Video is sent as one H.264 access unit per frame, split across UDP
// datagrams. Every datagram carries the fixed 24-byte header below.

// ```text
//  0..4   magic          u32  0x4C4C5332 (\"LLS2\")
//  4..5   version        u8
//  5..6   flags          u8   bit0 keyframe, bit1 keyframe request
//  6..10  frame_id       u32
//  10..12  packet_index   u16
//  12..14  packet_count   u16
//  14..22  timestamp_us   u64
//  22..24  payload_len    u16
// ```

// Note: Input forwarding datagrams reuse this header format but carry an
// input payload described by `common::InputMsg` after the header.

// ```text
//  0       msg_type       u8   see InputMsg below
//  1..3    reserved       u8[2]  (zero-filled for future use)
//  3..5    seq_id         u16  monotonically increasing per-session
//  5..13   session_token  u64   identifies the streaming session
// 13..    payload       varies by msg_type
// ```

// ```rust
// use common::{InputMsg, input_msg_encode, input_msg_decode};

// let mut buf = [0u8; 38]; // 24-byte header + 14-byte payload
// input_msg_encode(InputMsg::MouseMove, 42, 0x123456789abcdef0, &mut buf[24..]);
// ```

// How many datagrams a frame of `payload_bytes` needs.
pub const MAX_PAYLOAD: usize = 1200;
pub fn packet_count(payload_bytes: usize) -> u16 {
    payload_bytes.div_ceil(MAX_PAYLOAD) as u16
}

fn run() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let bind: SocketAddr = value(&args, "--bind")
        .unwrap_or_else(|| format!("0.0.0.0:{DEFAULT_CLIENT_PORT}"))
        .parse()
        .context("--bind must look like 0.0.0.0:9100")?;
    let server: SocketAddr = value(&args, "--server")
        .unwrap_or_else(|| format!("127.0.0.1:{DEFAULT_SERVER_PORT}"))
        .parse()
        .context("--server must look like 127.0.0.1:9000")?;
    let (mut width, mut height) =
        parse_size(&value(&args, "--size").unwrap_or_else(|| "1280x720".to_string()))?;
    let requested_window = value(&args, "--window").map(|text| parse_size(&text)).transpose()?;
    // Default to fullscreen for extended display unless a specific window size is requested.
    let mut fullscreen = args.iter().any(|arg| arg == "--fullscreen") || requested_window.is_none();
    let flip = args.iter().any(|arg| arg == "--flip");
    let rotate_arg: u16 = value(&args, "--rotate")
        .or_else(|| value(&args, "--rotation"))
        .map(|s| s.parse::<u16>().context("--rotate must be 0, 90, 180, or 270"))
        .transpose()?
        .unwrap_or(0);
    let mut client_rotate: u16 = rotate_arg;
    let orientation_arg: Option<u16> = value(&args, "--orientation")
        .map(|s| s.parse().context("--orientation must be 0, 90, 180, or 270"))
        .transpose()?;
    let mut current_orientation: u16 = orientation_arg.unwrap_or(0);
    let mut orient: u16 = current_orientation;
    let seconds: u64 = match value(&args, "--seconds") {
        Some(text) => text.parse().context("--seconds must be a number")?,
        None => 0,
    };

// Frame header for video datagrams, exactly 24 bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C)]
pub struct FrameHeader {
    /// flags: bit0 keyframe, bit1 keyframe request
    pub flags: u8,
    /// frame_id monotonically increasing per session
    pub frame_id: u32,
    /// packet_index within the frame
    pub packet_index: u16,
    /// packet_count total packets per frame
    pub packet_count: u16,
    /// timestamp_us since epoch or stream start
    pub timestamp_us: u64,
    /// payload_len length of payload data following the header
    pub payload_len: u16,
}

impl FrameHeader {
    /// Fixed size of a frame header in bytes (24).
    pub const SIZE: usize = 24;

    /// Decode a FrameHeader from the first 24 bytes of a datagram.
    pub fn decode(data: &[u8]) -> Option<Self> {
        if data.len() < Self::SIZE {
            return None;
        }
        Some(Self {
            flags: data[5],
            frame_id: u32::from_be_bytes([data[6], data[7], data[8], data[9]]),
            packet_index: u16::from_be_bytes([data[10], data[11]]),
            packet_count: u16::from_be_bytes([data[12], data[13]]),
            timestamp_us: u64::from_be_bytes([
                data[14], data[15], data[16], data[17], data[18], data[19], data[20], data[21], data[22], data[23],
            ]),
            payload_len: u16::from_be_bytes([data[22], data[23]]),
        })
    }

    println!(
        "client: video on {bind}, keyframe requests to {server}, stream {width}x{height}, window {window_width}x{window_height}{}{}{}",
        if fullscreen { " (fullscreen)" } else { "" },
        if flip { " (flip 180°)" } else { "" },
        if client_rotate != 0 { format!(" (rotate {}°)", client_rotate) } else { "".to_string() }
    );
    println!("client size: {width}x{height}");

    /// Returns whether the keyframe flag is set.
    pub fn is_keyframe(self) -> bool {
        self.flags & FLAG_KEYFRAME != 0
    }

    /// Returns whether the keyframe request flag is set.
    pub fn is_keyframe_request(self) -> bool {
        self.flags & FLAG_KEYFRAME_REQUEST != 0
    }
}

pub const HEADER_LEN: usize = 24;
pub const MAGIC: u32 = 0x4C4C5332;
pub const VERSION: u8 = 1;

// Default network ports
pub const DEFAULT_CLIENT_PORT: u16 = 9100;
pub const DEFAULT_SERVER_PORT: u16 = 9000;

// Maximum packets per frame for partial frame accumulation.
pub const MAX_PACKETS_PER_FRAME: u16 = 32;
// Maximum number of partial frames to keep before evicting oldest.
pub const MAX_PARTIAL_FRAMES: usize = 8;
// Keyframe request interval in milliseconds.
pub const REQUEST_INTERVAL: u64 = 1000;

// Keyframe flag bit (bit0 of flags field)
pub const FLAG_KEYFRAME: u8 = 0x01;
// Keyframe request flag bit (bit1 of flags field)
pub const FLAG_KEYFRAME_REQUEST: u8 = 0x02;

// Input forwarding message types, sent over the existing UDP control channel
// (server port 9000). The first byte is the message type; the rest is type-
// specific payload. All input datagrams use the same 24-byte frame header as
// video frames, with the magic `0x4C4C5332` and version `1`, but the payload
// data after the header carries the input message.

// Layout of the *payload* (after the 24-byte video header):

// ```text
//  0       msg_type       u8   see InputMsg below
//  1..3    reserved       u8[2]  (zero-filled for future use)
//  3..5    seq_id         u16  monotonically increasing per-session
//  5..13   session_token  u64   identifies the streaming session
// 13..    payload       varies by msg_type
// ```

// ```text
//  0       msg_type       u8   0x01 = MouseMove
//                               0x02 = MouseBtn
//                               0x03 = MouseWheel
//                               0x04 = Key
//                               0x05 = ReleaseAll
//  1..3    reserved       u8[2]
//  3..5    seq_id         u16
//  5..13   session_token  u64
// 13..15  mouse_x        u16  normalized x (0..65535) â€” MouseMove only
// 15..17  mouse_y        u16  normalized y (0..65535) â€” MouseMove only
// 13      button_mask      u8   bits 0=left, 1=right, 2=middle â€” MouseBtn only
// 13..15  wheel_delta    i16  positive = up/away, negative = down/toward â€” MouseWheel only
// 13      scode            u8   SDL scancode; bit 7 = key up â€” Key only
// ```

// ```rust
// use common::{InputMsg, input_msg_encode, input_msg_decode};

// let mut buf = [0u8; 38]; // 24-byte header + 14-byte payload
// input_msg_encode(InputMsg::MouseMove, 42, 0x123456789abcdef0, &mut buf[24..]);
// ```

// Redistribution of this source is permitted under the MIT license that
// governs the rest of this project.

// How many datagrams a frame of `payload_bytes` needs.
pub const MAX_PAYLOAD: usize = 1200;
pub fn packet_count(payload_bytes: usize) -> u16 {
    payload_bytes.div_ceil(MAX_PAYLOAD) as u16
}

// Default network ports
pub const DEFAULT_CLIENT_PORT: u16 = 9100;
pub const DEFAULT_SERVER_PORT: u16 = 9000;

// Maximum packets per frame for partial frame accumulation.
pub const MAX_PACKETS_PER_FRAME: u16 = 32;
// Maximum number of partial frames to keep before evicting oldest.
pub const MAX_PARTIAL_FRAMES: usize = 8;
// Keyframe request interval in milliseconds.
pub const REQUEST_INTERVAL: u64 = 1000;

// Keyframe flag bit (bit0 of flags field)
pub const FLAG_KEYFRAME: u8 = 0x01;
// Keyframe request flag bit (bit1 of flags field)
pub const FLAG_KEYFRAME_REQUEST: u8 = 0x02;

// Input forwarding message types, sent over the existing UDP control channel
// (server port 9000). The first byte is the message type; the rest is type-.
// specific payload. All input datagrams use the same 24-byte frame header as
// video frames, with the magic `0x4C4C5332` and version `1`, but the payload
// data after the header carries the input message.

// Layout of the *payload* (after the 24-byte video header):

// ```text
//  0       msg_type       u8   see InputMsg below
//  1..3    reserved       u8[2]  (zero-filled for future use)
//  3..5    seq_id         u16  monotonically increasing per-session
//  5..13   session_token  u64   identifies the streaming session
// 13..    payload       varies by msg_type
// ```

// ```text
//  0       msg_type       u8   0x01 = MouseMove
//                               0x02 = MouseBtn
//                               0x03 = MouseWheel
//                               0x04 = Key
//                               0x05 = ReleaseAll
//  1..3    reserved       u8[2]
//  3..5    seq_id         u16
//  5..13   session_token  u64
// 13..15  mouse_x        u16  normalized x (0..65535) â€” MouseMove only
// 15..17  mouse_y        u16  normalized y (0..65535) â€” MouseMove only
// 13      button_mask      u8   bits 0=left, 1=right, 2=middle â€” MouseBtn only
// 13..15  wheel_delta    i16  positive = up/away, negative = down/toward â€” MouseWheel only
// 13      scode            u8   SDL scancode; bit 7 = key up â€” Key only
// ```

// ```rust
// use common::{InputMsg, input_msg_encode, input_msg_decode};

// let mut buf = [0u8; 38]; // 24-byte header + 14-byte payload
// input_msg_encode(InputMsg::MouseMove, 42, 0x123456789abcdef0, &mut buf[24..]);
// ```

// ```text
// // How many datagrams a frame of `payload_bytes` needs.
// pub const MAX_PAYLOAD: usize = 1200;
// pub fn packet_count(payload_bytes: usize) -> u16 {
//     payload_bytes.div_ceil(MAX_PAYLOAD) as u16
// }

// ```text
// // Default network ports
// pub const DEFAULT_CLIENT_PORT: u16 = 9100;
// pub const DEFAULT_SERVER_PORT: u16 = 9000;

// ```text
// // Maximum packets per frame for partial frame accumulation.
// pub const MAX_PACKETS_PER_FRAME: u16 = 32;

// ```text
// // Maximum number of partial frames to keep before evicting oldest.
// pub const MAX_PARTIAL_FRAMES: usize = 8;

// ```text
// // Keyframe request interval in milliseconds.
// pub const REQUEST_INTERVAL: u64 = 1000;

// ```rust
// ``` // Keyframe flag bit (bit0 of flags field)
// pub const FLAG_KEYFRAME: u8 = 0x01;
// // Keyframe request flag bit (bit1 of flags field)
// pub const FLAG_KEYFRAME_REQUEST: u8 = 0x02;

// Input forwarding message types, sent over the existing UDP control channel
// (server port 9000). The first byte is the message type; the rest is type-.
// specific payload. All input datagrams use the same 24-byte frame header as
// video frames, with the magic `0x4C4C5332` and version `1`, but the payload
// data after the header carries the input message.

// Layout of the *payload* (after the 24-byte video header):

// ```text
//  0       msg_type       u8   see InputMsg below
//  1..3    reserved       u8[2]  (zero-filled for future use)
//  3..5    seq_id         u16  monotonically increasing per-session
//  5..13   session_token  u64   identifies the streaming session
// 13..    payload       varies by msg_type
// ```

// ```text
//  0       msg_type       u8   0x01 = MouseMove
//                               0x02 = MouseBtn
//                               0x03 = MouseWheel
//                               0x04 = Key
//                               0x05 = ReleaseAll
//  1..3    reserved       u8[2]
//  3..5    seq_id         u16
//  5..13   session_token  u64
// 13..15  mouse_x        u16  normalized x (0..65535) â€” MouseMove only
// 15..17  mouse_y        u16  normalized y (0..65535) â€” MouseMove only
// 13      button_mask      u8   bits 0=left, 1=right, 2=middle â€” MouseBtn only
// 13..15  wheel_delta    i16  positive = up/away, negative = down/toward â€” MouseWheel only
// 13      scode            u8   SDL scancode; bit 7 = key up â€” Key only
// ```

// ```rust
// use common::{InputMsg, input_msg_encode, input_msg_decode};

// let mut buf = [0u8; 38]; // 24-byte header + 14-byte payload
// input_msg_encode(InputMsg::MouseMove, 42, 0x123456789abcdef0, &mut buf[24..]);
// ```

// ```text
// // Coalesce mouse moves: send at most one position every this many ms.
// const MOUSE_MOVE_INTERVAL_MS: u64 = 8;

// fn main() {
//     if let Err(err) = run() {
//         eprintln!("error: {err:#}");
//         std::process::exit(1);
//     }
// }

// fn run() -> Result<()> {
//     let args: Vec<String> = std::env::args().skip(1).collect();
//     let bind: SocketAddr = value(&args, "--bind")
//         .unwrap_or_else(|| format!("0.0.0.0:{DEFAULT_CLIENT_PORT}"))
//         .parse()
//         .context("--bind must look like 0.0.0.0:9100")?;
//     let server: SocketAddr = value(&args, "--server")
//         .unwrap_or_else(|| format!("127.0.0.1:{DEFAULT_SERVER_PORT}"))
//         .parse()
//         .context("--server must look like 127.0.0.1:9000")?;
//     let (mut width, mut height) =
//         parse_size(&value(&args, "--size").unwrap_or_else(|| "1280x720".to_string()))?;
//     let requested_window = value(&args, "--window").map(|text| parse_size(&text)).transpose()?;
//     // Default to fullscreen for extended display unless a specific window size is requested.
//     let mut fullscreen = args.iter().any(|arg| arg == "--fullscreen") || requested_window.is_none();
//     let flip = args.iter().any(|arg| arg == "--flip");
//     let orientation_arg: Option<u16> = value(&args, "--orientation")
//         .map(|s| s.parse().context("--orientation must be 0, 90, 180, or 270"))
//         .transpose()?;
//     let mut current_orientation: u16 = orientation_arg.unwrap_or(0);
//     let mut orient: u16 = current_orientation;
//     let seconds: u64 = match value(&args, "--seconds") {
//         Some(text) => text.parse().context("--seconds must be a number")?,
//         None => 0,
//     };

//     // Runtime diagnostics: show exactly which ports the processes are using.
//     println!("client_ports: bind={bind} server={server}");
//     println!("client size: {width}x{height}");

//     let sdl = sdl2::init().map_err(|err| anyhow!("SDL init failed: {err}"))?;
//     let video = sdl
//         .video()
//         .map_err(|err| anyhow!("SDL video init failed: {err}"))?;

//     // Default to a window that fits on the screen: the stream may well be
//     // 1280x720 on a 1366x768 laptop panel. SDL scales the texture for free.
//     let (window_width, window_height) = match requested_window {
//         Some(size) => size,
//         None => {
//             let desktop = video
//                 .desktop_display_mode(0)
//                 .map_err(|err| anyhow!("reading the desktop mode failed: {err}"))?;
//             let max_width = (desktop.w.max(0) as u32) * 9 / 10;
//             let max_height = (desktop.h.max(0) as u32) * 9 / 10;
//             (width.min(max_width), height.min(max_height))
//         }
//     };

//     println!(
//         "client: video on {bind}, keyframe requests to {server}, stream {width}x{height}, window {window_width}x{window_height}{}{}",
//         if fullscreen { " (fullscreen)" } else { "" },
//         if flip { " (flip 180°)" } else { "" }
//     );
//     println!("client size: {width}x{height}");

//     let stats = Arc::new(Stats::default());
//     let running = Arc::new(AtomicBool::new(true));

//     let (config_tx, config_rx) = std::sync::mpsc::sync_channel::<(u32, u32, u16, u8)>(4);
//     let (decoder, sink) = Decoder::new(width, height)?;
//     let receiver = Receiver::new(bind, server, width, height, config_tx, stats.clone(), running.clone())?
//         // The sender must be created before spawn() consumes the receiver.
//     let mut input = receiver.input_sender()?;
//     let net_thread = receiver.spawn(decoder);

//     if let Some(orient) = orientation_arg {
//         let _ = input.set_orientation(orient);
//     }

//     let mut builder = video.window("lowlat-screen", window_width, window_height);
//     builder.resizable();
//     if fullscreen {
//         builder.fullscreen_desktop();
//     }
//     let window = builder
//         .build()
//         .map_err(|err| anyhow!("creating the window failed: {err}"))?;

//     // Keep the cursor visible so the user can interact smoothly across screens.
//     sdl.mouse().show_cursor(true);
//     let mut canvas = window
//         .into_canvas()
//         .build()
//         .map_err(|err| anyhow!("creating the canvas failed: {err}"))?;
//     let texture_creator = canvas.texture_creator();
//     // Use IYUV planar format: Y plane, then U, then V. No BGRA conversion.
//     // SDL_PIXELFORMAT_IYUV layout: [Y_0...Y_{n-1} U_0...U_{n/2-1} V_0...V_{n/2-1}]
//     // with pitch: Y = width, U = width/2, V = width/2
//     let mut texture = texture_creator
//         .create_texture_streaming(PixelFormatEnum::IYUV, width, height)
//         .map_err(|err| anyhow!("creating the video texture failed: {err}"))?;
//     let mut event_pump = sdl
//         .event_pump()
//         .map_err(|err| anyhow!("getting the SDL event pump failed: {err}"))?;

//     let mut pitch = width as usize; // IYUV: Y plane pitch = width (bytes per row)
//     let started = Instant::now();
//     let limit = (seconds > 0).then(|| Duration::from_secs(seconds));
//     let mut last_report = Instant::now();
//     let mut frames_rendered = 0u64;
//     let mut decode_total_ms: f32 = 0.0;
//     let mut render_total_ms: f32 = 0.0;
//     let mut last_frame_at = Instant::now();

//     // Input forwarding state.
//     let mut input_events_sent: u64 = 0;
//     let mut btn_mask: u8 = 0;
//     let mut pending_move: Option<(i32, i32)> = None;
//     let mut last_mouse_send = Instant::now();

//     'outer: loop {
//         for event in event_pump.poll_iter() {
//             match event {
//                 Event::Quit { .. } => break 'outer,
//                 Event::KeyDown {
//                     keycode: Some(Keycode::Escape),
//                     ..
//                 } => break 'outer,
//                 Event::KeyDown {
//                     keycode: Some(Keycode::F11),
//                     repeat: false,
//                     ..
                    
                Event::KeyDown {
                    keycode: Some(Keycode::F11),
                    repeat: false,
                    ..
                } => {
                    // Toggle desktop fullscreen on F11.
                    let mode = if fullscreen {
                        FullscreenType::Off
                    } else {
                        FullscreenType::Desktop
                    };
                    match canvas.window_mut().set_fullscreen(mode) {
                        Ok(()) => {
                            fullscreen = !fullscreen;
                            eprintln!(
                                "F11 pressed - fullscreen {}",
                                if fullscreen { "enabled" } else { "disabled" }
                            );
                        }
                        Err(err) => eprintln!("fullscreen toggle failed: {err}"),
                    }
                }
                Event::KeyDown {
                    keycode: Some(Keycode::C),
                    repeat: false,
                    ..
                } => {
                    client_rotate = (client_rotate + 90) % 360;
                    println!("client-side screen rotation changed to {client_rotate}°");
                }
                Event::KeyDown {
                    keycode: Some(Keycode::R),
                    keymod,
                    repeat: false,
                    ..
                } => {
                    if keymod.contains(sdl2::keyboard::Mod::LSHIFTMOD) || keymod.contains(sdl2::keyboard::Mod::RSHIFTMOD) {
                        client_rotate = (client_rotate + 90) % 360;
                        println!("client-side screen rotation changed to {client_rotate}°");
                    } else {
                        current_orientation = match current_orientation {
                            0 => 90,
                            90 => 180,
                            180 => 270,
                            _ => 0,
                        };
                        println!("requesting orientation change to {current_orientation}°");
                        count_send(input.set_orientation(current_orientation), &mut input_events_sent, "set orientation");
                    }
                }
                Event::KeyDown {
                    scancode: Some(sc),
                    repeat: false,
                    ..
                } => {
                    // The protocol carries the scancode in 7 bits (bit 7 is
                    // the up flag), so scancodes >= 128 cannot be forwarded.
                    let code = sc as i32;
                    if (0..128).contains(&code) {
                        count_send(input.key(code as u8, false), &mut input_events_sent, "key down");
                    }
                }
                Event::KeyUp {
                    scancode: Some(sc),
                    ..
                } => {
                    let code = sc as i32;
                    if (0..128).contains(&code) {
                        count_send(input.key(code as u8, true), &mut input_events_sent, "key up");
                    }
                }
                Event::MouseMotion { x, y, .. } => {
                    // Keep only the newest position; it is flushed below at
                    // most once per MOUSE_MOVE_INTERVAL_MS.
                    pending_move = Some((x, y));
                }
                Event::MouseButtonDown { mouse_btn, .. } => {
                    btn_mask |= button_bit(mouse_btn);
                    count_send(input.mouse_buttons(btn_mask), &mut input_events_sent, "mouse button");
                }
                Event::MouseButtonUp { mouse_btn, .. } => {
                    btn_mask &= !button_bit(mouse_btn);
                    count_send(input.mouse_buttons(btn_mask), &mut input_events_sent, "mouse button");
                }
                Event::MouseWheel { y, .. } => {
                    if y != 0 {
                        let delta = y.clamp(i16::MIN as i32, i16::MAX as i32) as i16;
                        count_send(input.wheel(delta), &mut input_events_sent, "mouse wheel");
                    }
                }
                Event::Window {
                    win_event: WindowEvent::FocusLost,
                    ..
                } => {
                    // Never leave keys/buttons stuck on the server when the
                    // user switches away from the window.
                    btn_mask = 0;
                    pending_move = None;
                    let _ = input.release_all();
                }
                _ => {}
            }
        }

// Flush the coalesced mouse position, normalized to the current
// window size (which changes on resize and fullscreen toggles).
        if let Some((x, y)) = pending_move {
            if last_mouse_send.elapsed() >= Duration::from_millis(MOUSE_MOVE_INTERVAL_MS) {
                pending_move = None;
                let (ww, wh) = canvas.window().size();
                let effective_angle = (client_rotate + if flip { 180 } else { 0 }) % 360;
                let (nx, ny) = match effective_angle {
                    90 => (
                        normalize_coord(y, wh),
                        normalize_coord((ww as i32 - 1 - x).max(0), ww),
                    ),
                    180 => (
                        normalize_coord((ww as i32 - 1 - x).max(0), ww),
                        normalize_coord((wh as i32 - 1 - y).max(0), wh),
                    ),
                    270 => (
                        normalize_coord((wh as i32 - 1 - y).max(0), wh),
                        normalize_coord(x, ww),
                    ),
                    _ => (
                        normalize_coord(x, ww),
                        normalize_coord(y, wh),
                    ),
                };
                count_send(input.mouse_move(nx, ny), &mut input_events_sent, "mouse move");
                last_mouse_send = Instant::now();
            }
        }

// Check for server display configuration updates
        // Always apply the new config: width/height/orientation.
        // Recreate the texture whenever the config changes (size or rotation),
        // so portrait/landscape flips are visible immediately.  The --size
        // argument is only used until the first config arrives.
        while let Ok((new_w, new_h, new_orient, _new_rot)) = config_rx.try_recv() {
            current_orientation = new_orient;
            if new_w != width || new_h != height || orient != new_orient {
                println!(
                    "client size: {new_w}x{new_h} (orientation {new_orient}°)"
                );
                width = new_w;
                height = new_h;
                pitch = width as usize;
                texture = texture_creator
                    .create_texture_streaming(PixelFormatEnum::IYUV, width, height)
                    .map_err(|err| anyhow!("recreating the video texture failed: {err}"))?;
                if !fullscreen {
                    let _ = canvas.window_mut().set_size(width, height);
                }
                orient = new_orient;
            }
        }

        if let Some(limit) = limit {
            if started.elapsed() >= limit {
                break;
            }
        }

// Always render the newest frame; anything older is already gone.
        match sink.take_latest() {
            Some(frame) => {
                let expected_len = pitch * height as usize * 3 / 2;
                if frame.data.len() != expected_len {
                    continue;
                }
                last_frame_at = Instant::now();
                let render_started = Instant::now();
                if let Err(err) = texture.update(None, &frame.data, pitch) {
                    eprintln!("warning: uploading frame failed: {err}");
                    continue;
                }
                let effective_angle = (client_rotate + if flip { 180 } else { 0 }) % 360;
                match effective_angle {
                    90 => {
                        let (ww, wh) = canvas.window().size();
                        let dst_rect = sdl2::rect::Rect::new(
                            (ww as i32 - wh as i32) / 2,
                            (wh as i32 - ww as i32) / 2,
                            wh,
                            ww,
                        );
                        canvas
                            .copy_ex(&texture, None, Some(dst_rect), 90.0, None, false, false)
                            .map_err(|err| anyhow!("drawing rotated frame failed: {err}"))?;
                    }
                    180 => {
                        canvas
                            .copy_ex(&texture, None, None, 180.0, None, false, false)
                            .map_err(|err| anyhow!("drawing flipped frame failed: {err}"))?;
                    }
                    270 => {
                        let (ww, wh) = canvas.window().size();
                        let dst_rect = sdl2::rect::Rect::new(
                            (ww as i32 - wh as i32) / 2,
                            (wh as i32 - ww as i32) / 2,
                            wh,
                            ww,
                        );
                        canvas
                            .copy_ex(&texture, None, Some(dst_rect), 270.0, None, false, false)
                            .map_err(|err| anyhow!("drawing rotated frame failed: {err}"))?;
                    }
                    _ => {
                        canvas
                            .copy(&texture, None, None)
                            .map_err(|err| anyhow!("drawing a frame failed: {err}"))?;
                    }
                }
                canvas.present();

                let render_elapsed = render_started.elapsed().as_secs_f32() * 1000.0;
                let decode_ms = frame.decode_ms;
                let render_ms = render_elapsed;

                decode_total_ms += decode_ms;
                render_total_ms += render_ms;
                stats.record_decode(decode_ms);
                stats.record_render(render_ms);
                stats.frames_rendered.fetch_add(1, Ordering::Relaxed);
                frames_rendered += 1;
            }
            None => {
                std::thread::sleep(Duration::from_millis(1));
                // When server cuts or stops sending frames, close the client window
                if frames_rendered > 0 && last_frame_at.elapsed() >= Duration::from_secs(10) {
                    eprintln!("server stopped (no frames for 10s); closing client display");
                    break 'outer;
                }
            }
        }

        if last_report.elapsed() >= Duration::from_secs(1) {
            report(
                &stats,
                decode_total_ms,
                render_total_ms,
                frames_rendered,
                input_events_sent,
            );
            input_events_sent = 0;
            last_report = Instant::now();
        }
    }

// On clean exit, release all keys and mouse buttons on the server.
    let _ = input.release_all();
// Restore the local cursor on exit.
    sdl.mouse().show_cursor(true);

    running.store(false, Ordering::Relaxed);
    let _ = net_thread.join();
    println!("client stopped after {frames_rendered} rendered frames");
    Ok(())
}

// Count a successful send; log failures without killing the render loop.
fn count_send(result: Result<()>, counter: &mut u64, what: &str) {
    match result {
        Ok(()) => *counter += 1,
        Err(err) => eprintln!("failed to send {what}: {err}"),
    }
}

fn button_bit(btn: MouseButton) -> u8 {
    match btn {
        MouseButton::Left => 0x01,
        MouseButton::Right => 0x02,
        MouseButton::Middle => 0x04,
        _ => 0,
    }
}

// Window pixel coordinate -> normalized 0..65535.
fn normalize_coord(value: i32, extent: u32) -> u16 {
    let extent = extent.max(1) as f64;
    let normalized = (value.max(0) as f64 / extent) * 65535.0;
    normalized.min(65535.0) as u16
}

fn report(
    stats: &Stats,
    decode_total_ms: f32,
    render_total_ms: f32,
    frames_rendered: u64,
    input_events_sent: u64,
) {
    let packets = take_counter(&stats.packets);
    let bytes = take_counter(&stats.bytes);
    let received = take_counter(&stats.frames_received);
    let dropped = take_counter(&stats.frames_dropped);
    let replaced = take_counter(&stats.frames_replaced);
    let requests = take_counter(&stats.keyframe_requests);
    let rendered = take_counter(&stats.frames_rendered);

    let (assembly_avg, assembly_max) = stats.take_assembly();
    let (decode_avg, decode_max) = stats.take_decode();
    let (render_avg, render_max) = stats.take_render();

    println!(
        "recv {received:>3}/s {packets:>4} pkts {:>6.1} Mbit/s | shown {rendered:>3}/s | assembly {assembly_avg:>4.1}/{assembly_max:<4.1} ms | decode {decode_avg:>4.1}/{decode_max:<5.1} ms | render {render_avg:>4.1}/{render_max:<4.1} ms | dropped {dropped} replaced {replaced} kf-req {requests} | input sent {input_events_sent}/s",
        bytes as f64 * 8.0 / 1_000_000.0,
    );
    // Per-frame averages since last report
    if frames_rendered > 0 {
        let avg_decode = decode_total_ms / frames_rendered as f32;
        let avg_render = render_total_ms / frames_rendered as f32;
        println!(
            "  per-frame: decode {avg_decode:.1} ms | render {avg_render:.1} ms",
        );
    }
}

fn value(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|arg| arg == flag)
        .and_then(|index| args.get(index + 1))
        .cloned()
}

fn parse_size(text: &str) -> Result<(u32, u32)> {
    let (width, height) = text
        .split_once('x')
        .with_context(|| format!("size must look like 1280x720, got {text:?}"))?;
    Ok((
        width.trim().parse().context("invalid width in --size")?,
        height.trim().parse().context("invalid height in --size")?,
    ))
)
}

fn parse_size(text: &str) -> Result<(u32, u32)> {
    let (width, height) = text
        .split_once('x')
        .with_context(|| format!("size must look like 1280x720, got {text:?}"))?;
    Ok((
        width.trim().parse().context("invalid width in --size")?,
        height.trim().parse().context("invalid height in --size")?,
    ))
}size")?,
        height.trim().parse().context("invalid height in --size")?,
    ))
})
})
)
}

fn parse_size(text: &str) -> Result<(u32, u32)> {
    let (width, height) = text
        .split_once('x')
        .with_context(|| format!("size must look like 1280x720, got {text:?}"))?;
    Ok((
        width.trim().parse().context("invalid width in --size")?,
        height.trim().parse().context("invalid height in --size")?,
    ))
}size")?,
        height.trim().parse().context("invalid height in --size")?,
    ))
})
})
)
}

fn parse_size(text: &str) -> Result<(u32, u32)> {
    let (width, height) = text
        .split_once('x')
        .with_context(|| format!("size must look like 1280x720, got {text:?}"))?;
    Ok((
        width.trim().parse().context("invalid width in --size")?,
        height.trim().parse().context("invalid height in --size")?,
    ))
}size")?,
        height.trim().parse().context("invalid height in --size")?,
    ))
})
})
)
}

fn parse_size(text: &str) -> Result<(u32, u32)> {
    let (width, height) = text
        .split_once('x')
        .with_context(|| format!("size must look like 1280x720, got {text:?}"))?;
    Ok((
        width.trim().parse().context("invalid width in --size")?,
        height.trim().parse().context("invalid height in --size")?,
    ))
}size")?,
        height.trim().parse().context("invalid height in --size")?,
    ))
})
})
}