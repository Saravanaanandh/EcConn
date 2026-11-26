package com.lowlat.protocol

/**
 * H.264 Annex-B helpers: split NAL units and pull SPS/PPS out of the first
 * keyframe for `MediaCodec`'s `csd-0` / `csd-1`.
 *
 * The server encodes with `x264 --repeat-headers=1` and `aud=1`, so every
 * keyframe starts with AUD/SPS/PPS (plus optional SEI) before the IDR slice.
 */
object Avc {
    /** A NAL unit including its start code: `[start, end)` in the input. */
    data class Nal(val type: Int, val start: Int, val end: Int)

    fun nals(data: ByteArray): List<Nal> {
        val starts = ArrayList<Int>(8)
        var index = 0
        while (index + 2 < data.size) {
            if (data[index] == 0.toByte() && data[index + 1] == 0.toByte()) {
                if (data[index + 2] == 1.toByte()) {
                    starts.add(index)
                    index += 3
                    continue
                }
                if (index + 3 < data.size && data[index + 2] == 0.toByte() && data[index + 3] == 1.toByte()) {
                    starts.add(index)
                    index += 4
                    continue
                }
            }
            index += 1
        }
        if (starts.isEmpty()) return emptyList()

        val out = ArrayList<Nal>(starts.size)
        for ((position, start) in starts.withIndex()) {
            val startCode = if (start + 2 < data.size && data[start + 2] == 1.toByte()) 3 else 4
            val header = start + startCode
            val end = if (position + 1 < starts.size) starts[position + 1] else data.size
            if (header >= end) continue
            out.add(Nal(data[header].toInt() and 0x1F, start, end))
        }
        return out
    }

    fun types(data: ByteArray): List<Int> = nals(data).map { it.type }

    /** Returns true if the Annex-B byte stream contains an IDR NAL unit (type 5). */
    fun hasIdr(data: ByteArray): Boolean {
        for (nal in nals(data)) {
            if (nal.type == 5) return true
        }
        return false
    }

    /** First SPS (7) and PPS (8), including start codes, or `null`. */
    fun csd(data: ByteArray): Pair<ByteArray, ByteArray>? {
        var sps: ByteArray? = null
        var pps: ByteArray? = null
        for (nal in nals(data)) {
            when (nal.type) {
                7 -> if (sps == null) sps = data.copyOfRange(nal.start, nal.end)
                8 -> if (pps == null) pps = data.copyOfRange(nal.start, nal.end)
            }
        }
        val foundSps = sps ?: return null
        val foundPps = pps ?: return null
        return foundSps to foundPps
    }
}
