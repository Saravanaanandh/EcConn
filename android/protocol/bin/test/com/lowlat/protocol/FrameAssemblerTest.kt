package com.lowlat.protocol

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertNull
import kotlin.test.assertTrue

/** Edge cases ported from the Rust `Receiver`/`Partial` logic. */
class FrameAssemblerTest {
    private fun header(
        frameId: Long,
        index: Int,
        count: Int,
        flags: Int = 0,
        payloadLen: Int = 4,
    ) = FrameHeader(flags, frameId, index, count, 1000L + frameId, payloadLen)

    @Test
    fun duplicate_packets_are_ignored() {
        val assembler = FrameAssembler()
        val first = assembler.onPacket(header(1, 0, 2), byteArrayOf(1, 2, 3, 4))
        assertNull(first)
        assertNull(assembler.onPacket(header(1, 0, 2), byteArrayOf(9, 9, 9, 9)))
        val frame = assembler.onPacket(header(1, 1, 2), byteArrayOf(5, 6))
        assertEquals(6, frame?.data?.size)
        assertEquals(listOf<Byte>(1, 2, 3, 4, 5, 6), frame?.data?.toList())
    }

    @Test
    fun old_frames_are_dropped_after_a_newer_frame_completed() {
        val assembler = FrameAssembler()
        assembler.onPacket(header(2, 0, 1), byteArrayOf(2))
        assertNull(assembler.onPacket(header(1, 0, 1), byteArrayOf(1)))
    }

    @Test
    fun stale_frames_are_dropped_and_force_a_keyframe() {
        val assembler = FrameAssembler()
        assembler.onPacket(header(5, 0, 2, flags = Wire.FLAG_KEYFRAME), byteArrayOf(1), nowMs = 0)
        assembler.noteDecoded()
        assertFalse(assembler.awaitingKeyframe)

        val drop = assembler.dropStale(nowMs = 500)
        assertEquals(1, drop.count)
        assertTrue(drop.lostCurrent)
        assertTrue(assembler.awaitingKeyframe)
        assertEquals(0, assembler.pendingFrames)
    }

    @Test
    fun stale_frame_older_than_completed_one_does_not_force_keyframe() {
        val assembler = FrameAssembler()
        assembler.onPacket(header(3, 0, 2), byteArrayOf(1), nowMs = 0)
        assembler.onPacket(header(4, 0, 1), byteArrayOf(2), nowMs = 0)
        assembler.noteDecoded()

        val drop = assembler.dropStale(nowMs = 500)
        assertEquals(1, drop.count)
        assertFalse(drop.lostCurrent)
        assertFalse(assembler.awaitingKeyframe)
    }

    @Test
    fun packet_count_is_bounded_and_indices_validated() {
        val assembler = FrameAssembler()
        assertNull(assembler.onPacket(header(1, 0, 0), ByteArray(1)))
        assertNull(assembler.onPacket(header(1, 0, Wire.MAX_PACKETS_PER_FRAME + 1), ByteArray(1)))
        assertNull(assembler.onPacket(header(1, 2, 2), ByteArray(1)))
        assertNull(assembler.onPacket(header(1, 0, 1), ByteArray(Wire.MAX_PAYLOAD + 1)))
    }

    @Test
    fun capacity_evicts_the_oldest_partial() {
        val assembler = FrameAssembler(maxPartialFrames = 2)
        assembler.onPacket(header(1, 0, 2), byteArrayOf(1))
        assembler.onPacket(header(2, 0, 2), byteArrayOf(2))
        assertTrue(assembler.awaitingKeyframe)
        assembler.onPacket(header(3, 0, 2), byteArrayOf(3))
        assertEquals(2, assembler.pendingFrames)
    }

    @Test
    fun reset_clears_in_flight_frames() {
        val assembler = FrameAssembler()
        assembler.onPacket(header(9, 0, 2), byteArrayOf(1))
        assembler.resetForDecoder()
        assertEquals(0, assembler.pendingFrames)
        assertTrue(assembler.awaitingKeyframe)
    }
}
