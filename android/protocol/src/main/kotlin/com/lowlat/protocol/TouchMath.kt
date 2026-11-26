package com.lowlat.protocol

import kotlin.math.max
import kotlin.math.min
import kotlin.math.roundToInt

/** How the video is scaled inside the surface. */
enum class FitMode { Fit, Fill }

/** The area the video occupies, in view pixels (may be negative for Fill). */
data class VideoRect(
    val left: Float,
    val top: Float,
    val width: Float,
    val height: Float,
)

/**
 * Maps a touch position to the normalized 0..65535 coordinates the server
 * expects, taking letterboxing (Fit) or cropping (Fill) into account.
 */
object TouchMath {
    fun rect(
        containerWidth: Float,
        containerHeight: Float,
        videoWidth: Int,
        videoHeight: Int,
        mode: FitMode,
    ): VideoRect {
        if (containerWidth <= 0f || containerHeight <= 0f || videoWidth <= 0 || videoHeight <= 0) {
            return VideoRect(0f, 0f, containerWidth, containerHeight)
        }
        val scale = if (mode == FitMode.Fit) {
            min(containerWidth / videoWidth, containerHeight / videoHeight)
        } else {
            max(containerWidth / videoWidth, containerHeight / videoHeight)
        }
        val width = videoWidth * scale
        val height = videoHeight * scale
        return VideoRect(
            left = (containerWidth - width) / 2f,
            top = (containerHeight - height) / 2f,
            width = width,
            height = height,
        )
    }

    fun normalize(x: Float, y: Float, rect: VideoRect): Pair<Int, Int> {
        if (rect.width <= 0f || rect.height <= 0f) return 0 to 0
        val nx = ((x - rect.left) / rect.width).coerceIn(0f, 1f)
        val ny = ((y - rect.top) / rect.height).coerceIn(0f, 1f)
        return (nx * 65535f).roundToInt() to (ny * 65535f).roundToInt()
    }
}
