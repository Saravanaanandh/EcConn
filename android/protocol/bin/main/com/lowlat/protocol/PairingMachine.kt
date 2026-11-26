package com.lowlat.protocol

import java.security.SecureRandom
import java.util.Locale

/** Credentials remembered for a host: input token + future encryption key. */
data class Credentials(val token: Long, val key: ByteArray) {
    override fun equals(other: Any?): Boolean =
        other is Credentials && token == other.token && key.contentEquals(other.key)

    override fun hashCode(): Int = 31 * token.hashCode() + key.contentHashCode()
}

/**
 * Receiver-side pairing state machine, a port of
 * `common::pairing::PairingMachine`.
 *
 * Unlike the Rust prototype (which uses a splitmix64 seeded from clock+PID),
 * this uses [SecureRandom]: the 32-byte key is meant to protect the stream in
 * a later milestone. The wire format is unchanged.
 */
class PairingMachine(private val random: SecureRandom = SecureRandom()) {
    sealed class State {
        data object Idle : State()
        data class AwaitingAllow(
            val code: String,
            val hostId: String,
            val hostName: String,
            val resume: Boolean,
            val known: Credentials?,
        ) : State()

        data class Accepted(
            val hostId: String,
            val hostName: String,
            val token: Long,
            val key: ByteArray,
        ) : State()

        data class Rejected(val reason: String) : State()
    }

    var state: State = State.Idle
        private set

    fun pending(): State.AwaitingAllow? = state as? State.AwaitingAllow

    /**
     * Handle a REQ/RESUME. Always moves to AwaitingAllow and returns the
     * Challenge the host should display. [known] reuses stored credentials.
     */
    fun onRequest(hostId: String, hostName: String, known: Credentials?): PairMessage.Challenge {
        val code = String.format(Locale.US, "%06d", random.nextInt(1_000_000))
        state = State.AwaitingAllow(
            code = code,
            hostId = Beacon.sanitize(hostId),
            hostName = Beacon.sanitize(hostName),
            resume = known != null,
            known = known,
        )
        return PairMessage.Challenge(code)
    }

    /** The user tapped Allow. */
    fun allow(): PairMessage.Accept? {
        val pending = state as? State.AwaitingAllow ?: return null
        val credentials = pending.known ?: Credentials(randomToken(), randomKey())
        state = State.Accepted(
            hostId = pending.hostId,
            hostName = pending.hostName,
            token = credentials.token,
            key = credentials.key,
        )
        return PairMessage.Accept(pending.code, credentials.token, credentials.key)
    }

    /** The user tapped Deny (or the dialog timed out). */
    fun deny(reason: String): PairMessage.Reject? {
        if (state !is State.AwaitingAllow) return null
        val clean = Beacon.sanitize(reason)
        state = State.Rejected(clean)
        return PairMessage.Reject(clean)
    }

    fun reset() {
        state = State.Idle
    }

    private fun randomToken(): Long {
        var token = random.nextLong()
        if (token == 0L) token = 1L
        return token
    }

    private fun randomKey(): ByteArray = ByteArray(Wire.KEY_LEN).also { random.nextBytes(it) }
}
