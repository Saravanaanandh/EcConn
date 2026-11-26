package com.lowlat.protocol

import kotlin.test.Test
import kotlin.test.assertContentEquals
import kotlin.test.assertEquals
import kotlin.test.assertNull

class AvcTest {
    /** AUD, SPS, PPS, IDR with mixed 3- and 4-byte start codes. */
    private val keyframe = byteArrayOf(
        0, 0, 0, 1, 0x09, 0xf0.toByte(),
        0, 0, 0, 1, 0x67, 0x42, 0x00, 0x1e,
        0, 0, 1, 0x68, 0xce.toByte(), 0x3c, 0x80.toByte(),
        0, 0, 1, 0x65, 0x88.toByte(), 0x84.toByte(),
    )

    @Test
    fun splits_nal_units_with_both_start_code_lengths() {
        assertEquals(listOf(9, 7, 8, 5), Avc.types(keyframe))
    }

    @Test
    fun extracts_sps_and_pps_including_start_codes() {
        val (sps, pps) = Avc.csd(keyframe)!!
        assertContentEquals(byteArrayOf(0, 0, 0, 1, 0x67, 0x42, 0x00, 0x1e), sps)
        assertContentEquals(byteArrayOf(0, 0, 1, 0x68, 0xce.toByte(), 0x3c, 0x80.toByte()), pps)
    }

    @Test
    fun p_frame_without_headers_has_no_csd() {
        val pFrame = byteArrayOf(0, 0, 0, 1, 0x41, 0x9a.toByte())
        assertEquals(listOf(1), Avc.types(pFrame))
        assertNull(Avc.csd(pFrame))
    }

    @Test
    fun empty_input_is_safe() {
        assertEquals(emptyList(), Avc.types(ByteArray(0)))
        assertNull(Avc.csd(ByteArray(0)))
    }
}
