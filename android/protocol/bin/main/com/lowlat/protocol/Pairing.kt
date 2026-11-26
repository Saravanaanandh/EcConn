package com.lowlat.protocol

/**
 * Pairing protocol, byte-compatible with `common::pairing::PairMsg`:
 *
 * ```
 * PAIR1|REQ|host_id|host_name          host -> receiver, first contact
 * PAIR1|RESUME|host_id|host_name       host -> receiver, previously paired
 * PAIR1|CHALLENGE|code                 receiver -> host, 6-digit code
 * PAIR1|ACCEPT|code|token_hex|key_hex  receiver -> host, after the user allows
 * PAIR1|REJECT|reason                  receiver -> host
 * ```
 */
sealed class PairMessage {
    abstract fun encode(): String

    data class Request(val hostId: String, val hostName: String) : PairMessage() {
        override fun encode(): String =
            "${Wire.PAIR_MAGIC}|REQ|${Beacon.sanitize(hostId)}|${Beacon.sanitize(hostName)}"
    }

    data class Resume(val hostId: String, val hostName: String) : PairMessage() {
        override fun encode(): String =
            "${Wire.PAIR_MAGIC}|RESUME|${Beacon.sanitize(hostId)}|${Beacon.sanitize(hostName)}"
    }

    data class Challenge(val code: String) : PairMessage() {
        override fun encode(): String = "${Wire.PAIR_MAGIC}|CHALLENGE|${Beacon.sanitize(code)}"
    }

    class Accept(
        val code: String,
        val token: Long,
        val key: ByteArray,
    ) : PairMessage() {
        override fun encode(): String = buildString {
            append(Wire.PAIR_MAGIC).append("|ACCEPT|")
            append(Beacon.sanitize(code)).append('|')
            append(java.lang.Long.toUnsignedString(token, 16).padStart(16, '0')).append('|')
            append(toHex(key))
        }

        override fun equals(other: Any?): Boolean =
            other is Accept && code == other.code && token == other.token && key.contentEquals(other.key)

        override fun hashCode(): Int =
            31 * (31 * code.hashCode() + token.hashCode()) + key.contentHashCode()

        override fun toString(): String =
            "Accept(code=$code, token=${java.lang.Long.toUnsignedString(token, 16)}, key=${toHex(key)})"
    }

    data class Reject(val reason: String) : PairMessage() {
        override fun encode(): String = "${Wire.PAIR_MAGIC}|REJECT|${Beacon.sanitize(reason)}"
    }

    companion object {
        fun decode(text: String): PairMessage? {
            val parts = Wire.splitFields(text.trim())
            if (parts.size < 2 || parts[0] != Wire.PAIR_MAGIC) return null
            return when (parts[1]) {
                "REQ", "RESUME" -> {
                    if (parts.size != 4 || parts[2].isEmpty()) return null
                    if (parts[1] == "REQ") Request(parts[2], parts[3]) else Resume(parts[2], parts[3])
                }
                "CHALLENGE" -> {
                    if (parts.size != 3 || parts[2].isEmpty()) return null
                    Challenge(parts[2])
                }
                "ACCEPT" -> {
                    if (parts.size != 5) return null
                    val token = try {
                        java.lang.Long.parseUnsignedLong(parts[3], 16)
                    } catch (_: NumberFormatException) {
                        return null
                    }
                    val key = fromHex(parts[4]) ?: return null
                    if (key.size != Wire.KEY_LEN) return null
                    Accept(parts[2], token, key)
                }
                "REJECT" -> {
                    if (parts.size != 3) return null
                    Reject(parts[2])
                }
                else -> null
            }
        }
    }
}

/** Lowercase hex, no separators. */
fun toHex(bytes: ByteArray): String {
    val hex = "0123456789abcdef"
    val out = StringBuilder(bytes.size * 2)
    for (byte in bytes) {
        val value = byte.toInt() and 0xFF
        out.append(hex[value ushr 4]).append(hex[value and 0x0F])
    }
    return out.toString()
}

/** Decode hex; `null` on odd length or a bad digit. */
fun fromHex(text: String): ByteArray? {
    if (text.length % 2 != 0) return null
    val out = ByteArray(text.length / 2)
    for (index in out.indices) {
        val hi = hexDigit(text[index * 2]) ?: return null
        val lo = hexDigit(text[index * 2 + 1]) ?: return null
        out[index] = ((hi shl 4) or lo).toByte()
    }
    return out
}

private fun hexDigit(c: Char): Int? = when (c) {
    in '0'..'9' -> c - '0'
    in 'a'..'f' -> c - 'a' + 10
    in 'A'..'F' -> c - 'A' + 10
    else -> null
}
