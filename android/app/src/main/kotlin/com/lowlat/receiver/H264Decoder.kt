package com.lowlat.receiver

import android.media.MediaCodec
import android.media.MediaFormat
import android.util.Log
import android.view.Surface
import com.lowlat.protocol.Avc
import java.nio.ByteBuffer

/**
 * Low-latency H.264 video decoder wrapping Android's [MediaCodec].
 *
 * Thread-safe: all public methods are synchronized on [lock].
 * Qualcomm (c2.qti.avc.decoder) specific requirements:
 * - First frame MUST be an IDR (NAL type 5); P-frames before IDR cause 0xffffffff error.
 * - Do NOT pass BUFFER_FLAG_KEY_FRAME to queueInputBuffer; use flags=0 always.
 * - Do NOT set KEY_LOW_LATENCY or KEY_PRIORITY on Snapdragon - causes BAD_INDEX.
 */
class H264Decoder {

    companion object {
        private const val TAG = "H264Decoder"
        private const val MIME_TYPE = "video/avc"
        private const val TIMEOUT_US = 8000L   // slightly longer timeout for Qualcomm
    }

    private val lock = Any()
    private var codec: MediaCodec? = null
    private var configuredSurface: Surface? = null
    private var currentWidth = 1920
    private var currentHeight = 1080
    private var hasConfigured = false
    private var waitingForFirstKeyframe = true
    private val bufferInfo = MediaCodec.BufferInfo()

    private var cachedCsd0: ByteArray? = null
    private var cachedCsd1: ByteArray? = null

    var onRequestKeyframe: (() -> Unit)? = null

    fun setSurface(surface: Surface?) {
        synchronized(lock) {
            Log.i(TAG, "setSurface: surface=${surface?.hashCode()}, prev=${configuredSurface?.hashCode()}, hasCodec=${codec != null}")
            if (configuredSurface == surface) return
            configuredSurface = surface

            if (surface == null) {
                // Surface destroyed — stop codec so we re-start cleanly on next surface
                stopCodecLocked()
                return
            }

            val activeCodec = codec
            if (activeCodec != null) {
                try {
                    activeCodec.setOutputSurface(surface)
                    Log.i(TAG, "setOutputSurface re-attached OK")
                    return
                } catch (e: Exception) {
                    Log.w(TAG, "setOutputSurface failed (${e.message}), recreating codec")
                    stopCodecLocked()
                }
            }

            // No codec yet or setOutputSurface failed — start fresh with known CSD if available
            if (cachedCsd0 != null && cachedCsd1 != null) {
                configureLocked(currentWidth, currentHeight, cachedCsd0, cachedCsd1)
            } else if (hasConfigured) {
                // Surface came back but we have no SPS/PPS yet — wait for first keyframe
                waitingForFirstKeyframe = true
                onRequestKeyframe?.invoke()
            }
        }
    }

    fun configure(width: Int, height: Int, csd0: ByteArray? = null, csd1: ByteArray? = null) {
        synchronized(lock) {
            configureLocked(width, height, csd0, csd1)
        }
    }

    private fun configureLocked(width: Int, height: Int, csd0: ByteArray? = null, csd1: ByteArray? = null) {
        currentWidth = if (width > 0) width else 1920
        currentHeight = if (height > 0) height else 1080

        if (csd0 != null) cachedCsd0 = csd0
        if (csd1 != null) cachedCsd1 = csd1

        val surface = configuredSurface
        if (surface == null) {
            Log.i(TAG, "configure($currentWidth x $currentHeight) deferred: no surface yet")
            return
        }

        stopCodecLocked()

        try {
            val format = MediaFormat.createVideoFormat(MIME_TYPE, currentWidth, currentHeight)
            val useCsd0 = csd0 ?: cachedCsd0
            val useCsd1 = csd1 ?: cachedCsd1
            if (useCsd0 != null && useCsd1 != null) {
                format.setByteBuffer("csd-0", ByteBuffer.wrap(useCsd0))
                format.setByteBuffer("csd-1", ByteBuffer.wrap(useCsd1))
            }
            // NOTE: Do NOT set KEY_LOW_LATENCY or KEY_PRIORITY on Qualcomm/Snapdragon —
            // causes "setOutputSurface BAD_INDEX" and codec crashes.

            val newCodec = MediaCodec.createDecoderByType(MIME_TYPE)
            newCodec.configure(format, surface, null, 0)
            newCodec.start()

            codec = newCodec
            hasConfigured = true
            waitingForFirstKeyframe = true
            Log.i(TAG, "MediaCodec STARTED ${currentWidth}x${currentHeight} — waiting for first IDR keyframe")
            onRequestKeyframe?.invoke()
        } catch (e: Exception) {
            Log.e(TAG, "Failed to start MediaCodec", e)
            stopCodecLocked()
        }
    }

    fun feedFrame(data: ByteArray, timestampUs: Long, isKeyframe: Boolean) {
        synchronized(lock) {
            val surface = configuredSurface ?: return

            // Extract SPS/PPS (csd-0/csd-1) from any frame that carries them
            val csd = Avc.csd(data)
            if (csd != null) {
                val spsChanged = !csd.first.contentEquals(cachedCsd0 ?: ByteArray(0))
                cachedCsd0 = csd.first
                cachedCsd1 = csd.second

                // If codec is running and SPS changed (resolution change), restart it
                if (hasConfigured && codec != null && spsChanged) {
                    Log.i(TAG, "SPS changed — reconfiguring decoder")
                    configureLocked(currentWidth, currentHeight, csd.first, csd.second)
                }
            }

            val hasIdr = Avc.hasIdr(data) || isKeyframe

            // Auto-configure from first SPS/PPS or keyframe if not yet running
            if (!hasConfigured || codec == null) {
                if (cachedCsd0 != null && cachedCsd1 != null) {
                    configureLocked(currentWidth, currentHeight, cachedCsd0, cachedCsd1)
                } else if (hasIdr) {
                    configureLocked(currentWidth, currentHeight)
                } else {
                    onRequestKeyframe?.invoke()
                    return
                }
            }

            // ───────────────────────────────────────────────────────────────
            // QUALCOMM REQUIREMENT: First frame fed after codec.start() MUST
            // be an IDR NAL (type 5). Feeding a P-frame first causes
            // c2.qti.avc.decoder to return error 0xffffffff and self-destruct.
            // ───────────────────────────────────────────────────────────────
            if (waitingForFirstKeyframe) {
                if (!hasIdr) {
                    // Silently discard; onRequestKeyframe already called at configure time
                    return
                }
                waitingForFirstKeyframe = false
                Log.i(TAG, "First IDR received — stream playback begins")
            }

            val activeCodec = codec ?: return

            try {
                val inputIndex = activeCodec.dequeueInputBuffer(TIMEOUT_US)
                if (inputIndex >= 0) {
                    val inputBuffer = activeCodec.getInputBuffer(inputIndex) ?: return
                    inputBuffer.clear()
                    if (data.size <= inputBuffer.remaining()) {
                        inputBuffer.put(data)
                        // flags=0 always for decoder input; BUFFER_FLAG_KEY_FRAME is for encoder output
                        activeCodec.queueInputBuffer(inputIndex, 0, data.size, timestampUs, 0)
                    } else {
                        Log.w(TAG, "Frame too large (${data.size} > ${inputBuffer.remaining()}), dropping")
                        activeCodec.queueInputBuffer(inputIndex, 0, 0, 0, 0)
                    }
                }
                drainOutputLocked()
            } catch (e: Exception) {
                Log.e(TAG, "feedFrame error (${e.message}), restarting decoder")
                stopCodecLocked()
                // Restart cleanly on next keyframe
                if (cachedCsd0 != null && cachedCsd1 != null) {
                    configureLocked(currentWidth, currentHeight, cachedCsd0, cachedCsd1)
                }
            }
        }
    }

    private fun drainOutputLocked() {
        val activeCodec = codec ?: return
        while (true) {
            try {
                val outIndex = activeCodec.dequeueOutputBuffer(bufferInfo, 0)
                when {
                    outIndex >= 0 -> activeCodec.releaseOutputBuffer(outIndex, true)
                    outIndex == MediaCodec.INFO_OUTPUT_FORMAT_CHANGED ->
                        Log.i(TAG, "Output format changed: ${activeCodec.outputFormat}")
                    else -> break
                }
            } catch (e: Exception) {
                Log.w(TAG, "drainOutput skipped: ${e.message}")
                break
            }
        }
    }

    private fun stopCodecLocked() {
        try { codec?.stop() } catch (ignored: Throwable) {}
        try { codec?.release() } catch (ignored: Throwable) {}
        codec = null
        hasConfigured = false
        waitingForFirstKeyframe = true
    }

    fun stop() {
        synchronized(lock) {
            stopCodecLocked()
            configuredSurface = null
        }
    }
}
