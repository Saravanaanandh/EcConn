package com.lowlat.protocol

/**
 * One receiver as advertised on the LAN, mirroring `common::discovery::Beacon`.
 *
 * Wire format (single UDP line):
 *
 * ```
 * LOWLAT1|id|name|platform|width|height|orientation|video_port|pair_port
 * ```
 */
data class Beacon(
    val id: String,
    val name: String,
    val platform: String,
    val width: Int,
    val height: Int,
    val orientation: Int,
    val videoPort: Int,
    val pairPort: Int,
) {
    fun encode(): String = buildString {
        append(Wire.BEACON_MAGIC)
        append('|').append(sanitize(id))
        append('|').append(sanitize(name))
        append('|').append(sanitize(platform))
        append('|').append(width)
        append('|').append(height)
        append('|').append(orientation)
        append('|').append(videoPort)
        append('|').append(pairPort)
    }

    companion object {
        fun parse(text: String): Beacon? {
            val parts = Wire.splitFields(text.trim())
            if (parts.size != 9) return null
            if (parts[0] != Wire.BEACON_MAGIC) return null
            if (parts[1].isEmpty() || parts[2].isEmpty()) return null
            val width = parts[4].toIntOrNull() ?: return null
            val height = parts[5].toIntOrNull() ?: return null
            val orientation = parts[6].toIntOrNull() ?: return null
            val videoPort = parts[7].toIntOrNull() ?: return null
            val pairPort = parts[8].toIntOrNull() ?: return null
            return Beacon(
                id = parts[1],
                name = parts[2],
                platform = parts[3],
                width = width,
                height = height,
                orientation = orientation,
                videoPort = videoPort,
                pairPort = pairPort,
            )
        }

        /** Keep `|` and control characters out of a field. */
        internal fun sanitize(value: String): String = buildString(value.length) {
            for (c in value) {
                append(if (c == '|' || c.isISOControl()) '_' else c)
            }
        }
    }
}
