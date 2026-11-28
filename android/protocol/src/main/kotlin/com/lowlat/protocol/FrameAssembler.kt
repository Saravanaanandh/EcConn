package com.lowlat.protocol

import java.io.ByteArrayOutputStream

/**
 * Reassembles one H.264 access unit from 24-byte-header datagrams.
 *
 * This is a direct port of the frame-tracking half of `client/src/net.rs`
 * (`Partial`, `newest_done`, stale eviction, keyframe gating) so the Android
 * receiver tolerates loss and reordering the same way the Rust client does.
 *
 * Call [onPacket] for every video datagram, [dropStale] once per receive-loop
 * tick, and consult [awaitingKeyframe] before feeding the decoder.
 */
class FrameAssembler(
    private val maxPartialFrames: Int = 4,
    private val staleAfterMs: Long = 200,
) {
    data class Frame(
        val frameId: Long,
        val keyframe: Boolean,
        val timestampUs: Long,
        val data: ByteArray,
    )

    data class StaleDrop(val count: Int, val lostCurrent: Boolean)

    private class Partial(val count: Int, val keyframe: Boolean) {
        val packets = arrayOfNulls<ByteArray>(count)
        var got: Int = 0
        var lastAt: Long = 0
    }

    private val partials = HashMap<Long, Partial>()

    /** Highest frame id already assembled (or deliberately written off). */
    private var newestDone: Long? = null

    /** Until a keyframe arrives, P-frames cannot be decoded (missing references). */
    var awaitingKeyframe: Boolean = true
        private set

    val pendingFrames: Int
        get() = partials.size

    /** Clear partial frames and frame-id history (used when host restarts stream). */
    fun reset() {
        partials.clear()
        newestDone = null
        awaitingKeyframe = true
    }

    /**
     * Feed one datagram payload. Returns the assembled frame when the last
     * missing packet arrives, otherwise `null`.
     */
    fun onPacket(
        header: FrameHeader,
        payload: ByteArray,
        nowMs: Long = System.currentTimeMillis(),
    ): Frame? {
        if (header.packetCount <= 0 || header.packetCount > Wire.MAX_PACKETS_PER_FRAME) return null
        if (header.packetIndex < 0 || header.packetIndex >= header.packetCount) return null
        if (payload.size > Wire.MAX_PAYLOAD) return null

        newestDone?.let { newest -> if (header.frameId <= newest) return null }

        if (!partials.containsKey(header.frameId) && partials.size >= maxPartialFrames) {
            evictOldest()
        }

        val partial = partials.getOrPut(header.frameId) {
            Partial(header.packetCount, header.isKeyframe())
        }
        // A corrupt/duplicate header with a different packet count is ignored.
        if (partial.count != header.packetCount) return null
        partial.lastAt = nowMs
        if (partial.packets[header.packetIndex] != null) return null

        partial.packets[header.packetIndex] = payload.copyOf()
        partial.got += 1
        if (partial.got < partial.count) return null

        partials.remove(header.frameId)
        newestDone = header.frameId

        val out = ByteArrayOutputStream(partial.count * 256)
        for (packet in partial.packets) {
            out.write(packet ?: return null)
        }
        return Frame(header.frameId, partial.keyframe, header.timestampUs, out.toByteArray())
    }

    /**
     * Write off frames whose packets stopped arriving. Mirrors `drop_stale`:
     * only a stale frame newer than [newestDone] forces a keyframe request.
     */
    fun dropStale(nowMs: Long = System.currentTimeMillis()): StaleDrop {
        if (partials.isEmpty()) return StaleDrop(0, false)
        val stale = partials.entries
            .filter { nowMs - it.value.lastAt > staleAfterMs }
            .map { it.key }
        if (stale.isEmpty()) return StaleDrop(0, false)

        var lostCurrent = false
        for (frameId in stale) {
            partials.remove(frameId)
            val newest = newestDone
            if (newest == null || frameId > newest) lostCurrent = true
        }
        if (lostCurrent) awaitingKeyframe = true
        return StaleDrop(stale.size, lostCurrent)
    }

    /** The decoder consumed a frame successfully; P-frames may follow. */
    fun noteDecoded() {
        awaitingKeyframe = false
    }

    /** Ask for a keyframe before feeding more data. */
    fun requireKeyframe() {
        awaitingKeyframe = true
    }

    /** Drop in-flight frames, e.g. after a decoder reset or size change. */
    fun resetForDecoder() {
        partials.clear()
        awaitingKeyframe = true
    }

    private fun evictOldest() {
        val oldest = partials.keys.minOrNull() ?: return
        partials.remove(oldest)
        awaitingKeyframe = true
    }
}
