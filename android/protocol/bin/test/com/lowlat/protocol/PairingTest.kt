package com.lowlat.protocol

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertNotNull
import kotlin.test.assertNull
import kotlin.test.assertTrue

/** Mirrors `common/src/pairing.rs` tests. */
class PairingTest {
    private fun key(): ByteArray = ByteArray(Wire.KEY_LEN) { it.toByte() }

    @Test
    fun hex_roundtrip() {
        val bytes = ByteArray(256) { it.toByte() }
        val text = toHex(bytes)
        assertEquals(512, text.length)
        assertTrue(bytes.contentEquals(fromHex(text)))
        assertNull(fromHex("0"))
        assertNull(fromHex("zz"))
    }

    @Test
    fun request_and_challenge_roundtrip() {
        val request = PairMessage.Request("host-1", "Saravana laptop")
        assertEquals(request, PairMessage.decode(request.encode()))
        val challenge = PairMessage.Challenge("012345")
        assertEquals(challenge, PairMessage.decode(challenge.encode()))
    }

    @Test
    fun accept_roundtrip_carries_key_and_token() {
        val accept = PairMessage.Accept("123456", 0xDEADBEEF12345678UL.toLong(), key())
        val decoded = assertNotNull(PairMessage.decode(accept.encode()) as? PairMessage.Accept)
        assertEquals(accept, decoded)
        assertEquals(0xDEADBEEF12345678UL.toLong(), decoded.token)
        assertTrue(key().contentEquals(decoded.key))
    }

    @Test
    fun reject_roundtrip() {
        val reject = PairMessage.Reject("user denied")
        assertEquals(reject, PairMessage.decode(reject.encode()))
    }

    @Test
    fun decode_rejects_bad_input() {
        assertNull(PairMessage.decode("nope"))
        assertNull(PairMessage.decode("PAIR1|UNKNOWN|x"))
        assertNull(PairMessage.decode("PAIR1|ACCEPT|123456|nothex|00"))
        assertNull(PairMessage.decode("PAIR1|ACCEPT|123456|0000000000000001|00"))
        assertNull(PairMessage.decode("PAIR1|REQ|onlyid"))
        assertNull(PairMessage.decode("PAIR1|CHALLENGE|"))
    }

    @Test
    fun machine_request_then_allow() {
        val machine = PairingMachine()
        val challenge = machine.onRequest("host-1", "Laptop", null)
        assertEquals(6, challenge.code.length)
        assertTrue(challenge.code.all { it.isDigit() })
        assertEquals("host-1", machine.pending()?.hostId)
        assertEquals(false, machine.pending()?.resume)

        val accept = assertNotNull(machine.allow())
        assertEquals(challenge.code, accept.code)
        assertEquals(Wire.KEY_LEN, accept.key.size)
        assertTrue(accept.token != 0L)
        assertTrue(machine.state is PairingMachine.State.Accepted)
        assertNull(machine.pending())
    }

    @Test
    fun machine_resume_reuses_stored_credentials() {
        val stored = Credentials(0x1122334455667788L, key())
        val machine = PairingMachine()
        machine.onRequest("host-1", "Laptop", stored)
        assertEquals(true, machine.pending()?.resume)
        val accept = assertNotNull(machine.allow())
        assertEquals(stored.token, accept.token)
        assertTrue(stored.key.contentEquals(accept.key))
    }

    @Test
    fun machine_deny() {
        val machine = PairingMachine()
        machine.onRequest("host-1", "Laptop", null)
        assertEquals(PairMessage.Reject("user denied"), machine.deny("user denied"))
        assertTrue(machine.state is PairingMachine.State.Rejected)
        assertNull(machine.allow())
    }

    @Test
    fun machine_allow_without_request_is_none() {
        val machine = PairingMachine()
        assertNull(machine.allow())
        assertNull(machine.deny("x"))
    }
}
