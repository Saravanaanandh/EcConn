package com.lowlat.protocol

import kotlin.test.Test
import kotlin.test.assertContentEquals
import kotlin.test.assertEquals
import kotlin.test.assertNotNull
import kotlin.test.assertNull

/**
 * Byte-for-byte compatibility checks against vectors produced by the Rust
 * workspace (`cargo run -p common --example wire_vectors`). If the protocol
 * changes on either side, these fail first.
 */
class RustVectorsTest {
    private fun payload(len: Int, seed: Int): ByteArray =
        ByteArray(len) { index -> ((index * 31 + seed) % 251).toByte() }

    /** FNV-1a 64, implemented identically in `common/examples/wire_vectors.rs`. */
    private fun fnv1a(bytes: ByteArray): String {
        var hash = -0x340d631b7bdddcdbL // 0xcbf29ce484222325 as a signed Long
        for (byte in bytes) {
            hash = hash xor (byte.toLong() and 0xFF)
            hash *= 0x100000001b3L
        }
        return java.lang.Long.toUnsignedString(hash, 16).padStart(16, '0')
    }

    @Test
    fun frame_headers_and_reassembly_match_rust() {
        val frameId = 7L
        val timestampUs = 123_456_789L
        val bodies = listOf(payload(1200, 0), payload(1200, 1), payload(600, 2))
        val expectedHeaders = listOf(
            "4c4c53320101000000070000000300000000075bcd1504b0",
            "4c4c53320101000000070001000300000000075bcd1504b0",
            "4c4c53320101000000070002000300000000075bcd150258",
        )
        val expectedDatagramHashes = listOf(
            "6ac04553757d1a28",
            "9a1675dcd6c42094",
            "294012f158db7c06",
        )

        val assembler = FrameAssembler()
        var frame: FrameAssembler.Frame? = null

        // Deliver packets in reverse to prove index-based reassembly.
        for (index in bodies.indices.reversed()) {
            val header = FrameHeader(
                flags = Wire.FLAG_KEYFRAME,
                frameId = frameId,
                packetIndex = index,
                packetCount = bodies.size,
                timestampUs = timestampUs,
                payloadLen = bodies[index].size,
            )
            val encoded = header.encode()
            assertEquals(expectedHeaders[index], toHex(encoded), "header $index")
            assertEquals(expectedDatagramHashes[index], fnv1a(encoded + bodies[index]), "datagram $index")
            frame = assembler.onPacket(header, bodies[index])
        }

        val assembled = assertNotNull(frame)
        assertEquals(3000, assembled.data.size)
        assertEquals("459353f6eddca362", fnv1a(assembled.data))
        assertEquals(frameId, assembled.frameId)
        assertEquals(true, assembled.keyframe)
        assertEquals(timestampUs, assembled.timestampUs)
    }

    @Test
    fun input_mouse_move_matches_rust() {
        val datagram = InputMessages.mouseMove(
            seqId = 42,
            token = 0x123456789abcdef0L,
            x = 0x1234,
            y = 0xABCD,
        )
        assertEquals(
            "4c4c53320100000000000000000100000000000000000011010000002a123456789abcdef01234abcd",
            toHex(datagram),
        )
        assertEquals(41, datagram.size)
    }

    @Test
    fun display_config_datagram_from_server_decodes() {
        val bytes = fromHex(
            "4c4c53320100000000000000000100000000000000000018070000000b0000000000001234" +
                "0000078000000438005a02",
        )
        assertNotNull(bytes)
        val config = InputMessages.decodeDisplayConfig(bytes)
        assertNotNull(config)
        assertEquals(1920, config.width)
        assertEquals(1080, config.height)
        assertEquals(90, config.orientation)
        assertEquals(2, config.rotation)
    }

    @Test
    fun beacon_and_pairing_lines_match_rust() {
        val beacon = Beacon(
            id = "recv-01",
            name = "Kitchen display",
            platform = "android",
            width = 1280,
            height = 720,
            orientation = 90,
            videoPort = 9100,
            pairPort = 9201,
        )
        assertEquals("LOWLAT1|recv-01|Kitchen display|android|1280|720|90|9100|9201", beacon.encode())
        assertEquals(beacon, Beacon.parse(beacon.encode()))

        val accept = PairMessage.decode(
            "PAIR1|ACCEPT|123456|deadbeef12345678|" +
                "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f",
        )
        val parsed = assertNotNull(accept as? PairMessage.Accept)
        assertEquals("123456", parsed.code)
        assertEquals(0xDEADBEEF12345678UL.toLong(), parsed.token)
        assertContentEquals(ByteArray(32) { it.toByte() }, parsed.key)
        assertEquals(
            "PAIR1|ACCEPT|123456|deadbeef12345678|" +
                "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f",
            parsed.encode(),
        )
    }

    @Test
    fun header_decoder_rejects_foreign_datagrams() {
        val valid = FrameHeader(0, 1, 0, 1, 0, 4).encode() + byteArrayOf(1, 2, 3, 4)
        assertNotNull(FrameHeader.decode(valid))
        val wrongMagic = valid.copyOf().also { it[0] = 0x00 }
        assertNull(FrameHeader.decode(wrongMagic))
        val wrongVersion = valid.copyOf().also { it[4] = 9 }
        assertNull(FrameHeader.decode(wrongVersion))
        assertNull(FrameHeader.decode(valid, 0, 23))
    }
}
