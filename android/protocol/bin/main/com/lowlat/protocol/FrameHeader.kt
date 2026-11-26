package com.lowlat.protocol

/** One 24-byte video datagram header, exactly as `common::FrameHeader`. */
data class FrameHeader(
    val flags: Int,
    val frameId: Long,
    val packetIndex: Int,
    val packetCount: Int,
    val timestampUs: Long,
    val payloadLen: Int,
) {
    fun isKeyframe(): Boolean = flags and Wire.FLAG_KEYFRAME != 0

    fun isKeyframeRequest(): Boolean = flags and Wire.FLAG_KEYFRAME_REQUEST != 0

    /**
     * Write this header into [dest] at [offset]. Returns [dest] for chaining.
     * Mirrors `FrameHeader::encode`, including the magic and version.
     */
    fun encode(dest: ByteArray = ByteArray(Wire.HEADER_LEN), offset: Int = 0): ByteArray {
        require(dest.size - offset >= Wire.HEADER_LEN) { "header does not fit" }
        Wire.putU32(dest, offset, Wire.MAGIC.toLong())
        dest[offset + 4] = Wire.VERSION.toByte()
        dest[offset + 5] = flags.toByte()
        Wire.putU32(dest, offset + 6, frameId)
        Wire.putU16(dest, offset + 10, packetIndex)
        Wire.putU16(dest, offset + 12, packetCount)
        Wire.putU64(dest, offset + 14, timestampUs)
        Wire.putU16(dest, offset + 22, payloadLen)
        return dest
    }

    companion object {
        /**
         * Decode a header. Returns `null` for a short buffer or foreign traffic
         * (wrong magic/version), so random UDP packets are ignored.
         */
        fun decode(data: ByteArray, offset: Int = 0, length: Int = data.size - offset): FrameHeader? {
            if (length < Wire.HEADER_LEN || data.size - offset < Wire.HEADER_LEN) return null
            if (Wire.u32(data, offset) != Wire.MAGIC.toLong()) return null
            if (Wire.u8(data, offset + 4) != Wire.VERSION) return null
            return FrameHeader(
                flags = Wire.u8(data, offset + 5),
                frameId = Wire.u32(data, offset + 6),
                packetIndex = Wire.u16(data, offset + 10),
                packetCount = Wire.u16(data, offset + 12),
                timestampUs = Wire.u64(data, offset + 14),
                payloadLen = Wire.u16(data, offset + 22),
            )
        }
    }
}
