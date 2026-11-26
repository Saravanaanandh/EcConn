package com.lowlat.protocol

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertNull
import kotlin.test.assertTrue

/** Mirrors `common/src/store.rs` tests. */
class PairedHostStoreTest {
    private fun host(id: String): PairedHost = PairedHost(
        hostId = id,
        hostName = "$id laptop",
        token = 0x0102030405060708L,
        key = ByteArray(Wire.KEY_LEN) { (it * 7).toByte() },
    )

    @Test
    fun text_roundtrip() {
        val store = PairedHostStore()
        store.upsert(host("host-a"))
        store.upsert(host("host-b"))
        val text = store.toText()
        assertTrue(text.startsWith(PairedHostStore.HEADER))
        val parsed = PairedHostStore.fromText(text)
        assertEquals(2, parsed.hosts.size)
        assertEquals("host-a", parsed.hosts[0].hostId)
        assertEquals("host-b", parsed.hosts[1].hostId)
        assertTrue(store.hosts[0].key.contentEquals(parsed.hosts[0].key))
        assertEquals(store.hosts[0].token, parsed.hosts[0].token)
    }

    @Test
    fun upsert_replaces_by_id() {
        val store = PairedHostStore()
        store.upsert(host("host-a"))
        store.upsert(host("host-a").copy(token = 99))
        assertEquals(1, store.hosts.size)
        assertEquals(99, store.find("host-a")?.token)
        assertEquals(99, store.credentials("host-a")?.token)
    }

    @Test
    fun comments_and_bad_lines_are_ignored() {
        val text = PairedHostStore.HEADER + "\n" +
            "# a comment\n" +
            "\n" +
            "bad line\n" +
            "host-a|A|0000000000000001|" + toHex(ByteArray(Wire.KEY_LEN)) + "\n"
        val store = PairedHostStore.fromText(text)
        assertEquals(1, store.hosts.size)
        assertEquals("host-a", store.hosts[0].hostId)
    }

    @Test
    fun missing_host_is_null() {
        assertNull(PairedHostStore().credentials("nope"))
    }

    @Test
    fun token_is_written_as_sixteen_unsigned_hex_digits() {
        val store = PairedHostStore()
        store.upsert(PairedHost("host-a", "A", -1L, ByteArray(Wire.KEY_LEN)))
        assertTrue(store.toText().contains("host-a|A|ffffffffffffffff|"))
        assertEquals(-1L, PairedHostStore.fromText(store.toText()).hosts[0].token)
    }
}
