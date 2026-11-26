package com.lowlat.protocol

/** One host the user allowed before. */
data class PairedHost(
    val hostId: String,
    val hostName: String,
    val token: Long,
    val key: ByteArray,
)

/**
 * Text store identical to `common/src/store.rs`:
 *
 * ```
 * # lowlat paired hosts v1
 * host-id|host name|00000000deadbeef|<64 hex chars>
 * ```
 *
 * The app writes it to `filesDir/paired_hosts.txt`; keeping the same format
 * makes the file easy to inspect and to move between implementations.
 */
class PairedHostStore(
    val hosts: MutableList<PairedHost> = mutableListOf(),
) {
    fun find(hostId: String): PairedHost? = hosts.firstOrNull { it.hostId == hostId }

    fun credentials(hostId: String): Credentials? =
        find(hostId)?.let { Credentials(it.token, it.key) }

    fun upsert(host: PairedHost) {
        val index = hosts.indexOfFirst { it.hostId == host.hostId }
        if (index >= 0) hosts[index] = host else hosts.add(host)
    }

    fun toText(): String = buildString {
        append(HEADER).append('\n')
        for (host in hosts) {
            append(Beacon.sanitize(host.hostId)).append('|')
            append(Beacon.sanitize(host.hostName)).append('|')
            append(java.lang.Long.toUnsignedString(host.token, 16).padStart(16, '0')).append('|')
            append(toHex(host.key)).append('\n')
        }
    }

    companion object {
        const val HEADER: String = "# lowlat paired hosts v1"

        fun fromText(text: String): PairedHostStore {
            val store = PairedHostStore()
            for (raw in text.lineSequence()) {
                val line = raw.trim()
                if (line.isEmpty() || line.startsWith("#")) continue
                val parts = Wire.splitFields(line)
                if (parts.size != 4 || parts[0].isEmpty()) continue
                val token = try {
                    java.lang.Long.parseUnsignedLong(parts[2], 16)
                } catch (_: NumberFormatException) {
                    continue
                }
                val key = fromHex(parts[3]) ?: continue
                if (key.size != Wire.KEY_LEN) continue
                store.hosts.add(PairedHost(parts[0], parts[1], token, key))
            }
            return store
        }
    }
}
