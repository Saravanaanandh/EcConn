package com.lowlat.receiver

import android.app.Activity
import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.hardware.display.DisplayManager
import android.hardware.display.VirtualDisplay
import android.media.MediaCodec
import android.media.MediaCodecInfo
import android.media.MediaFormat
import android.media.projection.MediaProjection
import android.media.projection.MediaProjectionManager
import android.os.Build
import android.os.Bundle
import android.os.IBinder
import android.util.Log
import com.lowlat.protocol.FrameHeader
import com.lowlat.protocol.InputType
import com.lowlat.protocol.Wire
import java.net.DatagramPacket
import java.net.DatagramSocket
import java.net.InetAddress
import java.util.concurrent.atomic.AtomicBoolean

/**
 * Foreground service that captures the Android screen via [MediaProjection],
 * hardware-encodes it to H.264 Annex-B via [MediaCodec], and streams UDP
 * packets to the paired EcConn Receiver on port 9100.
 */
class ScreenShareService : Service() {

    companion object {
        private const val TAG = "ScreenShareService"
        private const val CHANNEL_ID = "ecconn_screenshare"
        private const val NOTIFICATION_ID = 4001

        const val ACTION_START = "com.lowlat.receiver.ACTION_SHARE_START"
        const val ACTION_STOP = "com.lowlat.receiver.ACTION_SHARE_STOP"

        const val EXTRA_RESULT_CODE = "result_code"
        const val EXTRA_DATA = "data"
        const val EXTRA_DEST_IP = "dest_ip"
        const val EXTRA_DEST_PORT = "dest_port"
        const val EXTRA_WIDTH = "width"
        const val EXTRA_HEIGHT = "height"

        @Volatile
        var isStreaming: Boolean = false
            private set

        @Volatile
        var onStatsUpdate: ((frames: Long, kbps: Double) -> Unit)? = null

        @Volatile
        var onStopped: ((String) -> Unit)? = null
    }

    private val running = AtomicBoolean(false)
    private var mediaProjection: MediaProjection? = null
    private var virtualDisplay: VirtualDisplay? = null
    private var encoder: MediaCodec? = null
    private var videoSocket: DatagramSocket? = null
    private var controlSocket: DatagramSocket? = null
    private var streamThread: Thread? = null
    private var controlThread: Thread? = null

    @Volatile
    private var destAddr: InetAddress? = null
    private var destPort: Int = Wire.VIDEO_PORT
    private var encodeW: Int = 720
    private var encodeH: Int = 1280
    private val startNs = System.nanoTime()

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        if (intent?.action == ACTION_STOP) {
            stopSharing("Stopped by user")
            stopSelf()
            return START_NOT_STICKY
        }

        if (intent?.action == ACTION_START) {
            createNotificationChannel()
            val notification = buildNotification()
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
                startForeground(
                    NOTIFICATION_ID,
                    notification,
                    ServiceInfo.FOREGROUND_SERVICE_TYPE_MEDIA_PROJECTION,
                )
            } else {
                startForeground(NOTIFICATION_ID, notification)
            }

            val resultCode = intent.getIntExtra(EXTRA_RESULT_CODE, Activity.RESULT_CANCELED)
            @Suppress("DEPRECATION")
            val data: Intent? = intent.getParcelableExtra(EXTRA_DATA)
            val destIp = intent.getStringExtra(EXTRA_DEST_IP) ?: ""
            destPort = intent.getIntExtra(EXTRA_DEST_PORT, Wire.VIDEO_PORT)
            val rawW = intent.getIntExtra(EXTRA_WIDTH, 1080)
            val rawH = intent.getIntExtra(EXTRA_HEIGHT, 2460)

            if (resultCode != Activity.RESULT_OK || data == null || destIp.isBlank()) {
                stopSharing("Invalid screen capture permission or target")
                stopSelf()
                return START_NOT_STICKY
            }

            startSharing(resultCode, data, destIp, rawW, rawH)
        }

        return START_NOT_STICKY
    }

    private fun startSharing(
        resultCode: Int,
        data: Intent,
        destIp: String,
        rawW: Int,
        rawH: Int,
    ) {
        stopSharing(null)

        try {
            destAddr = InetAddress.getByName(destIp)
            val (w, h) = computeEncodeSize(rawW, rawH)
            encodeW = w
            encodeH = h

            val mgr = getSystemService(Context.MEDIA_PROJECTION_SERVICE) as MediaProjectionManager
            val projection = mgr.getMediaProjection(resultCode, data)
                ?: throw IllegalStateException("MediaProjection returned null")
            mediaProjection = projection

            projection.registerCallback(object : MediaProjection.Callback() {
                override fun onStop() {
                    if (running.get()) {
                        stopSharing("Screen capture ended by system")
                        stopSelf()
                    }
                }
            }, null)

            val format = MediaFormat.createVideoFormat("video/avc", encodeW, encodeH).apply {
                setInteger(
                    MediaFormat.KEY_COLOR_FORMAT,
                    MediaCodecInfo.CodecCapabilities.COLOR_FormatSurface,
                )
                setInteger(MediaFormat.KEY_BIT_RATE, 4_000_000)
                setInteger(MediaFormat.KEY_FRAME_RATE, 30)
                setInteger(MediaFormat.KEY_I_FRAME_INTERVAL, 1)
                setLong(MediaFormat.KEY_REPEAT_PREVIOUS_FRAME_AFTER, 100_000L)
            }

            val codec = MediaCodec.createEncoderByType("video/avc")
            codec.configure(format, null, null, MediaCodec.CONFIGURE_FLAG_ENCODE)
            val inputSurface = codec.createInputSurface()
            codec.start()
            encoder = codec

            val dpi = resources.displayMetrics.densityDpi
            virtualDisplay = projection.createVirtualDisplay(
                "EcConnShare",
                encodeW,
                encodeH,
                dpi,
                DisplayManager.VIRTUAL_DISPLAY_FLAG_AUTO_MIRROR,
                inputSurface,
                null,
                null,
            )

            videoSocket = DatagramSocket()
            controlSocket = DatagramSocket(Wire.CONTROL_PORT).apply {
                reuseAddress = true
                soTimeout = 500
            }

            running.set(true)
            isStreaming = true

            startControlListener()
            startEncoderDrainLoop()
            Log.i(TAG, "ScreenShareService started ${encodeW}x${encodeH} -> $destIp:$destPort")
        } catch (e: Exception) {
            Log.e(TAG, "Failed to start screen share", e)
            stopSharing("Error: ${e.message}")
            stopSelf()
        }
    }

    private fun computeEncodeSize(rawW: Int, rawH: Int): Pair<Int, Int> {
        val maxLong = 1280.0
        val longSide = maxOf(rawW, rawH).toDouble().coerceAtLeast(1.0)
        val scale = if (longSide > maxLong) maxLong / longSide else 1.0
        val w = ((rawW * scale).toInt() / 16) * 16
        val h = ((rawH * scale).toInt() / 16) * 16
        return w.coerceAtLeast(320) to h.coerceAtLeast(320)
    }

    private fun startControlListener() {
        val thread = Thread({
            val buf = ByteArray(256)
            val pkt = DatagramPacket(buf, buf.size)
            while (running.get()) {
                val sock = controlSocket ?: break
                try {
                    pkt.length = buf.size
                    sock.receive(pkt)
                    if (pkt.length == 10 && String(pkt.data, pkt.offset, pkt.length, Charsets.UTF_8) == "ECCONN_BYE") {
                        Log.i(TAG, "Client disconnected (ECCONN_BYE)")
                        stopSharing("Client disconnected")
                        stopSelf()
                        break
                    }
                    val hdr = FrameHeader.decode(pkt.data, pkt.offset, pkt.length) ?: continue
                    if (hdr.isKeyframeRequest()) {
                        requestSyncFrame()
                        sendDisplayConfig()
                    }
                } catch (_: java.net.SocketTimeoutException) {
                    // loop
                } catch (e: Exception) {
                    if (running.get()) Log.w(TAG, "Control recv error: ${e.message}")
                }
            }
        }, "EcConnShareControl")
        controlThread = thread
        thread.start()
    }

    private fun requestSyncFrame() {
        try {
            val bundle = Bundle().apply {
                putInt(MediaCodec.PARAMETER_KEY_REQUEST_SYNC_FRAME, 0)
            }
            encoder?.setParameters(bundle)
        } catch (_: Throwable) {}
    }

    private fun startEncoderDrainLoop() {
        val thread = Thread({
            val info = MediaCodec.BufferInfo()
            var csdBytes: ByteArray? = null
            var frameId = 1L
            var totalFrames = 0L
            var bytesInWindow = 0L
            var windowStartMs = System.currentTimeMillis()

            sendDisplayConfig()

            while (running.get()) {
                val codec = encoder ?: break
                try {
                    val outIdx = codec.dequeueOutputBuffer(info, 20_000L)
                    if (outIdx >= 0) {
                        val outBuf = codec.getOutputBuffer(outIdx)
                        if (outBuf != null && info.size > 0) {
                            val bytes = ByteArray(info.size)
                            outBuf.position(info.offset)
                            outBuf.limit(info.offset + info.size)
                            outBuf.get(bytes)

                            if ((info.flags and MediaCodec.BUFFER_FLAG_CODEC_CONFIG) != 0) {
                                csdBytes = bytes
                            } else {
                                val isKey = (info.flags and MediaCodec.BUFFER_FLAG_KEY_FRAME) != 0
                                val payload = if (isKey && csdBytes != null) {
                                    csdBytes!! + bytes
                                } else {
                                    bytes
                                }
                                sendVideoFrame(frameId++, payload, isKey)
                                totalFrames++
                                bytesInWindow += payload.size
                            }
                        }
                        codec.releaseOutputBuffer(outIdx, false)

                        val nowMs = System.currentTimeMillis()
                        val elapsed = nowMs - windowStartMs
                        if (elapsed >= 1000L) {
                            val kbps = (bytesInWindow * 8.0) / elapsed.toDouble()
                            onStatsUpdate?.invoke(totalFrames, kbps)
                            bytesInWindow = 0L
                            windowStartMs = nowMs
                        }
                    }
                } catch (e: Exception) {
                    if (running.get()) Log.w(TAG, "Encoder loop error: ${e.message}")
                    break
                }
            }
        }, "EcConnShareEncoder")
        streamThread = thread
        thread.start()
    }

    private fun sendDisplayConfig() {
        val addr = destAddr ?: return
        val sock = videoSocket ?: return
        try {
            val out = ByteArray(Wire.HEADER_LEN + 24)
            FrameHeader(
                flags = 0,
                frameId = 0L,
                packetIndex = 0,
                packetCount = 1,
                timestampUs = (System.nanoTime() - startNs) / 1000L,
                payloadLen = 24,
            ).encode(out, 0)
            val base = Wire.HEADER_LEN
            out[base] = InputType.DisplayConfig.code.toByte()
            putBe32(out, base + 13, encodeW.toLong())
            putBe32(out, base + 17, encodeH.toLong())
            putBe16(out, base + 21, 0)
            out[base + 23] = 0
            sock.send(DatagramPacket(out, out.size, addr, destPort))
            sock.send(DatagramPacket(out, out.size, addr, destPort))
        } catch (e: Exception) {
            Log.w(TAG, "Failed to send DisplayConfig: ${e.message}")
        }
    }

    private fun sendVideoFrame(frameId: Long, data: ByteArray, isKeyframe: Boolean) {
        val addr = destAddr ?: return
        val sock = videoSocket ?: return
        val count = ((data.size + Wire.MAX_PAYLOAD - 1) / Wire.MAX_PAYLOAD).coerceAtLeast(1)
        val tsUs = (System.nanoTime() - startNs) / 1000L
        val flags = if (isKeyframe) Wire.FLAG_KEYFRAME else 0

        for (index in 0 until count) {
            val offset = index * Wire.MAX_PAYLOAD
            val len = minOf(Wire.MAX_PAYLOAD, data.size - offset)
            val datagram = ByteArray(Wire.HEADER_LEN + len)
            FrameHeader(
                flags = flags,
                frameId = frameId,
                packetIndex = index,
                packetCount = count,
                timestampUs = tsUs,
                payloadLen = len,
            ).encode(datagram, 0)
            data.copyInto(datagram, Wire.HEADER_LEN, offset, offset + len)
            sock.send(DatagramPacket(datagram, datagram.size, addr, destPort))
        }
    }

    private fun putBe16(buf: ByteArray, offset: Int, value: Int) {
        buf[offset] = ((value ushr 8) and 0xFF).toByte()
        buf[offset + 1] = (value and 0xFF).toByte()
    }

    private fun putBe32(buf: ByteArray, offset: Int, value: Long) {
        buf[offset] = ((value ushr 24) and 0xFF).toByte()
        buf[offset + 1] = ((value ushr 16) and 0xFF).toByte()
        buf[offset + 2] = ((value ushr 8) and 0xFF).toByte()
        buf[offset + 3] = (value and 0xFF).toByte()
    }

    private fun stopSharing(reason: String?) {
        val wasRunning = running.getAndSet(false)
        isStreaming = false
        val addr = destAddr
        val sock = videoSocket
        if (wasRunning && addr != null && sock != null) {
            try {
                val bye = "ECCONN_BYE".toByteArray(Charsets.UTF_8)
                val pkt = DatagramPacket(bye, bye.size, addr, destPort)
                for (i in 0 until 3) sock.send(pkt)
            } catch (_: Throwable) {}
        }
        try { virtualDisplay?.release() } catch (_: Throwable) {}
        virtualDisplay = null
        try { encoder?.stop() } catch (_: Throwable) {}
        try { encoder?.release() } catch (_: Throwable) {}
        encoder = null
        try { mediaProjection?.stop() } catch (_: Throwable) {}
        mediaProjection = null
        try { videoSocket?.close() } catch (_: Throwable) {}
        videoSocket = null
        try { controlSocket?.close() } catch (_: Throwable) {}
        controlSocket = null
        if (wasRunning && reason != null) {
            onStopped?.invoke(reason)
        }
    }

    override fun onDestroy() {
        stopSharing("Service destroyed")
        super.onDestroy()
    }

    private fun createNotificationChannel() {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            val channel = NotificationChannel(
                CHANNEL_ID,
                "EcConn Screen Sharing",
                NotificationManager.IMPORTANCE_LOW,
            )
            val mgr = getSystemService(NotificationManager::class.java)
            mgr?.createNotificationChannel(channel)
        }
    }

    private fun buildNotification(): Notification {
        val builder = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            Notification.Builder(this, CHANNEL_ID)
        } else {
            @Suppress("DEPRECATION")
            Notification.Builder(this)
        }
        return builder
            .setContentTitle("EcConn Host Active")
            .setContentText("Sharing your phone screen wirelessly")
            .setSmallIcon(android.R.drawable.ic_menu_share)
            .build()
    }
}
