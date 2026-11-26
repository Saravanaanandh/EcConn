package com.lowlat.protocol

/** Input message types from `common::InputMsg`. */
enum class InputType(val code: Int) {
    MouseMove(0x01),
    MouseBtn(0x02),
    MouseWheel(0x03),
    Key(0x04),
    ReleaseAll(0x05),
    SetOrientation(0x06),
    DisplayConfig(0x07),
}

/**
 * Encoder/decoder for the 24-byte-header + 17-byte-payload input datagrams
 * defined in `common/src/lib.rs`.
 *
 * Payload layout:
 *
 * ```
 *  0      msg_type
 *  1..3   reserved (zero)
 *  3..5   seq_id       u16
 *  5..13  session_token u64
 * 13..17  data (type-specific, unused bytes stay zero)
 * ```
 *
 * The Rust client always writes 17 payload bytes so the server can read the
 * type-specific fields at 13..17 for every message.
 */
object InputMessages {
    const val PAYLOAD_LEN: Int = 17

    data class DisplayConfig(
        val width: Int,
        val height: Int,
        val orientation: Int,
        val rotation: Int,
    )

    fun datagram(
        type: InputType,
        seqId: Int,
        token: Long,
        data: ByteArray = ByteArray(0),
    ): ByteArray {
        require(data.size <= 4) { "input data is at most 4 bytes" }
        val out = ByteArray(Wire.HEADER_LEN + PAYLOAD_LEN)
        FrameHeader(
            flags = 0,
            frameId = 0,
            packetIndex = 0,
            packetCount = 1,
            timestampUs = 0,
            payloadLen = PAYLOAD_LEN,
        ).encode(out, 0)
        val base = Wire.HEADER_LEN
        out[base] = type.code.toByte()
        Wire.putU16(out, base + 3, seqId)
        Wire.putU64(out, base + 5, token)
        data.copyInto(out, base + 13)
        return out
    }

    /** Normalized 0..65535 coordinates over the displayed video area. */
    fun mouseMove(seqId: Int, token: Long, x: Int, y: Int): ByteArray {
        val data = ByteArray(4)
        Wire.putU16(data, 0, x.coerceIn(0, 65535))
        Wire.putU16(data, 2, y.coerceIn(0, 65535))
        return datagram(InputType.MouseMove, seqId, token, data)
    }

    /** Full button state: bit 0=left, 1=right, 2=middle. */
    fun mouseButton(seqId: Int, token: Long, mask: Int): ByteArray =
        datagram(InputType.MouseBtn, seqId, token, byteArrayOf((mask and 0xFF).toByte()))

    /** Wheel notches; positive = up/away, negative = down/toward. */
    fun wheel(seqId: Int, token: Long, delta: Int): ByteArray {
        val data = ByteArray(2)
        Wire.putU16(data, 0, delta and 0xFFFF)
        return datagram(InputType.MouseWheel, seqId, token, data)
    }

    /** SDL scancode (7 bits); [up] sets bit 7 of the payload byte. */
    fun key(seqId: Int, token: Long, scancode: Int, up: Boolean): ByteArray {
        val value = (scancode and 0x7F) or if (up) 0x80 else 0
        return datagram(InputType.Key, seqId, token, byteArrayOf(value.toByte()))
    }

    fun setOrientation(seqId: Int, token: Long, degrees: Int): ByteArray {
        val data = ByteArray(2)
        Wire.putU16(data, 0, degrees.coerceIn(0, 359))
        return datagram(InputType.SetOrientation, seqId, token, data)
    }

    fun releaseAll(seqId: Int, token: Long): ByteArray =
        datagram(InputType.ReleaseAll, seqId, token)

    /**
     * Recognize the server's DisplayConfig datagram: flags 0, frame_id 0,
     * one packet, a payload of at least 24 bytes, message type 0x07.
     */
    fun decodeDisplayConfig(datagram: ByteArray): DisplayConfig? {
        val header = FrameHeader.decode(datagram) ?: return null
        if (header.flags != 0 || header.frameId != 0L) return null
        if (header.packetCount != 1 || header.payloadLen < 24) return null
        if (datagram.size < Wire.HEADER_LEN + 24) return null
        val base = Wire.HEADER_LEN
        if (Wire.u8(datagram, base) != InputType.DisplayConfig.code) return null
        val width = Wire.u32(datagram, base + 13).toInt()
        val height = Wire.u32(datagram, base + 17).toInt()
        val orientation = Wire.u16(datagram, base + 21)
        val rotation = Wire.u8(datagram, base + 23)
        if (width <= 0 || height <= 0) return null
        return DisplayConfig(width, height, orientation, rotation)
    }
}
