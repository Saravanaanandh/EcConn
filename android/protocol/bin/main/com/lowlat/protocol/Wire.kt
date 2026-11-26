package com.lowlat.protocol

/**
 * Constants and big-endian helpers for the lowlat UDP protocol.
 *
 * These values mirror `common/src/lib.rs` and `common/src/discovery.rs`
 * exactly. The video datagram header is 24 bytes:
 *
 * ```
 *  0..4   magic          u32  0x4C4C5332 ("LLS2")
 *  4..5   version        u8   1
 *  5..6   flags          u8   bit0 keyframe, bit1 keyframe request
 *  6..10  frame_id       u32
 *  10..12 packet_index   u16
 *  12..14 packet_count   u16
 *  14..22 timestamp_us   u64
 *  22..24 payload_len    u16
 * ```
 */
object Wire {
    const val MAGIC: Int = 0x4C4C5332
    const val VERSION: Int = 1
    const val HEADER_LEN: Int = 24
    const val MAX_PAYLOAD: Int = 1200
    const val MAX_PACKETS_PER_FRAME: Int = 4096

    const val FLAG_KEYFRAME: Int = 0x01
    const val FLAG_KEYFRAME_REQUEST: Int = 0x02

    const val BEACON_PORT: Int = 9200
    const val PAIR_PORT: Int = 9201
    const val VIDEO_PORT: Int = 9100
    const val CONTROL_PORT: Int = 9000

    /** mDNS service type. Android's NsdManager adds the `.local.` suffix. */
    const val MDNS_SERVICE: String = "_lowlat._udp."

    const val BEACON_MAGIC: String = "LOWLAT1"
    const val PAIR_MAGIC: String = "PAIR1"
    const val KEY_LEN: Int = 32

    internal fun u8(bytes: ByteArray, offset: Int): Int = bytes[offset].toInt() and 0xFF

    internal fun u16(bytes: ByteArray, offset: Int): Int =
        (u8(bytes, offset) shl 8) or u8(bytes, offset + 1)

    internal fun u32(bytes: ByteArray, offset: Int): Long =
        (u8(bytes, offset).toLong() shl 24) or
            (u8(bytes, offset + 1).toLong() shl 16) or
            (u8(bytes, offset + 2).toLong() shl 8) or
            u8(bytes, offset + 3).toLong()

    internal fun u64(bytes: ByteArray, offset: Int): Long {
        var value = 0L
        for (index in 0 until 8) {
            value = (value shl 8) or u8(bytes, offset + index).toLong()
        }
        return value
    }

    internal fun putU16(bytes: ByteArray, offset: Int, value: Int) {
        bytes[offset] = ((value ushr 8) and 0xFF).toByte()
        bytes[offset + 1] = (value and 0xFF).toByte()
    }

    internal fun putU32(bytes: ByteArray, offset: Int, value: Long) {
        bytes[offset] = ((value ushr 24) and 0xFF).toByte()
        bytes[offset + 1] = ((value ushr 16) and 0xFF).toByte()
        bytes[offset + 2] = ((value ushr 8) and 0xFF).toByte()
        bytes[offset + 3] = (value and 0xFF).toByte()
    }

    internal fun putU64(bytes: ByteArray, offset: Int, value: Long) {
        for (index in 0 until 8) {
            bytes[offset + index] = ((value ushr (56 - index * 8)) and 0xFF).toByte()
        }
    }

    /** Split on `|`, keeping empty fields (Kotlin's `split` drops trailing ones). */
    internal fun splitFields(text: String): List<String> {
        val out = ArrayList<String>(9)
        var start = 0
        for (index in text.indices) {
            if (text[index] == '|') {
                out.add(text.substring(start, index))
                start = index + 1
            }
        }
        out.add(text.substring(start))
        return out
    }
}
