package com.lowlat.protocol

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertNull
import kotlin.test.assertTrue

/** Mirrors `common/src/discovery.rs` tests. */
class BeaconTest {
    private fun sample() = Beacon(
        id = "recv-01",
        name = "Kitchen display",
        platform = "linux",
        width = 1280,
        height = 720,
        orientation = 90,
        videoPort = 9100,
        pairPort = Wire.PAIR_PORT,
    )

    @Test
    fun beacon_roundtrip() {
        val beacon = sample()
        val text = beacon.encode()
        assertTrue(text.startsWith("LOWLAT1|"))
        assertEquals(beacon, Beacon.parse(text))
    }

    @Test
    fun beacon_rejects_wrong_magic() {
        val text = sample().encode().replace("LOWLAT1", "OTHER99")
        assertNull(Beacon.parse(text))
    }

    @Test
    fun beacon_rejects_bad_numbers() {
        val text = sample().encode().replace("|1280|", "|wide|")
        assertNull(Beacon.parse(text))
    }

    @Test
    fun beacon_rejects_missing_and_extra_fields() {
        assertNull(Beacon.parse("LOWLAT1|only|a|few"))
        val text = sample().encode() + "|extra"
        assertNull(Beacon.parse(text))
    }

    @Test
    fun beacon_sanitises_separators() {
        val beacon = sample().copy(name = "bad|name\nwith controls")
        val parsed = Beacon.parse(beacon.encode())
        assertEquals("bad_name_with controls", parsed?.name)
    }

    @Test
    fun beacon_rejects_empty_identity() {
        val beacon = sample().copy(id = "")
        assertNull(Beacon.parse(beacon.encode()))
    }
}
