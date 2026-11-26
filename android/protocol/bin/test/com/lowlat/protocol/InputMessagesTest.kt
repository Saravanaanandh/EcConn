package com.lowlat.protocol

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertNull

/** Offsets from `common/src/lib.rs`; the Rust client writes 17 payload bytes. */
class InputMessagesTest {
    @Test
    fun input_layout_offsets() {
        val datagram = InputMessages.mouseMove(
            seqId = 0x0102,
            token = 0x0102030405060708L,
            x = 0x1234,
            y = 0x5678,
        )
        assertEquals(41, datagram.size)
        assertEquals(0, datagram[5].toInt()) // flags
        assertEquals(0, datagram[12].toInt()) // packet_count high
        assertEquals(1, datagram[13].toInt()) // packet_count low
        assertEquals(17, datagram[23].toInt()) // payload_len
        assertEquals(0x01, datagram[24].toInt()) // MouseMove
        assertEquals(0x01, datagram[27].toInt()) // seq high
        assertEquals(0x02, datagram[28].toInt()) // seq low
        assertEquals(0x01, datagram[29].toInt()) // token high
        assertEquals(0x08, datagram[36].toInt()) // token low
        assertEquals(0x12, datagram[37].toInt())
        assertEquals(0x34, datagram[38].toInt())
        assertEquals(0x56, datagram[39].toInt())
        assertEquals(0x78, datagram[40].toInt())
    }

    @Test
    fun buttons_wheel_and_keys_use_the_same_layout() {
        assertEquals(0x02, InputMessages.mouseButton(1, 0, 1)[24].toInt())
        assertEquals(1, InputMessages.mouseButton(1, 0, 1)[37].toInt())

        val wheelDown = InputMessages.wheel(1, 0, -1)
        assertEquals(0xFF, wheelDown[37].toInt() and 0xFF)
        assertEquals(0xFF, wheelDown[38].toInt() and 0xFF)

        val keyUp = InputMessages.key(1, 0, 4, up = true)
        assertEquals(0x84, keyUp[37].toInt() and 0xFF)

        val orientation = InputMessages.setOrientation(1, 0, 90)
        assertEquals(0, orientation[37].toInt())
        assertEquals(90, orientation[38].toInt())

        assertEquals(0x05, InputMessages.releaseAll(1, 0)[24].toInt())
        assertEquals(0, InputMessages.releaseAll(1, 0)[37].toInt())
    }

    @Test
    fun coordinates_are_clamped() {
        assertEquals(0, InputMessages.mouseMove(1, 0, -5, -5)[37].toInt())
        val max = InputMessages.mouseMove(1, 0, 70_000, 70_000)
        assertEquals(0xFF, max[37].toInt() and 0xFF)
        assertEquals(0xFF, max[38].toInt() and 0xFF)
        assertEquals(0xFF, max[39].toInt() and 0xFF)
        assertEquals(0xFF, max[40].toInt() and 0xFF)
    }

    @Test
    fun display_config_rejects_video_frames_and_short_payloads() {
        val video = FrameHeader(Wire.FLAG_KEYFRAME, 3, 0, 1, 0, 4).encode() + ByteArray(4)
        assertNull(InputMessages.decodeDisplayConfig(video))
        val shortInput = FrameHeader(0, 0, 0, 1, 0, 17).encode() +
            InputMessages.releaseAll(1, 0).copyOfRange(24, 41)
        assertNull(InputMessages.decodeDisplayConfig(shortInput))
    }
}
