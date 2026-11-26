package com.lowlat.receiver

import android.content.Context
import android.os.Handler
import android.os.Looper
import android.util.Log
import com.lowlat.protocol.*
import java.io.File
import java.net.DatagramPacket
import java.net.DatagramSocket
import java.net.InetAddress
import java.net.SocketAddress
import java.nio.charset.StandardCharsets
import java.util.concurrent.Executors
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicInteger

/**
 * Orchestrates UDP video receiving (port 9100), pairing handshake (port 9201),
 * frame assembly, H.264 decoding, and input forwarding (port 9000).
 */
class ReceiverController(
    private val context: Context,
    private val decoder: H264Decoder,
) {

    enum class State {
        IDLE,
        PAIRING_CHALLENGE,
        CONNECTED,
        DISCONNECTED,
    }

    companion object {
        private const val TAG = "ReceiverController"
        private const val VIDEO_BUF_SIZE = 1500   // matches 1200-byte payload + 24-byte header; each UDP datagram is exactly one packet
        private const val PAIR_BUF_SIZE = 1024
        private const val STORE_FILE_NAME = "paired_hosts.txt"
    }

    private val mainHandler = Handler(Looper.getMainLooper())
    private val running = AtomicBoolean(false)
    private val ioExecutor = Executors.newSingleThreadExecutor()

    // State
    var state: State = State.IDLE
        private set

    var currentHostAddress: InetAddress? = null
        private set
    var currentHostName: String = ""
        private set
    var sessionToken: Long = 0L
        private set

    private var lastAcceptedCode: String? = null
    private var lastAcceptedKey: ByteArray? = null

    // Video dimensions
    var videoWidth = 1920
        private set
    var videoHeight = 1080
        private set

    // Callbacks
    var onStateChanged: ((State, String?) -> Unit)? = null
    var onPairingChallenge: ((code: String, hostName: String) -> Unit)? = null
    var onVideoSizeChanged: ((width: Int, height: Int) -> Unit)? = null

    // Network & Protocol objects
    private var videoSocket: DatagramSocket? = null
    private var pairSocket: DatagramSocket? = null
    private var controlSocket: DatagramSocket? = null

    private var videoThread: Thread? = null
    private var pairThread: Thread? = null

    private val frameAssembler = FrameAssembler()
    private val pairingMachine = PairingMachine()
    private var hostStore: PairedHostStore = PairedHostStore()

    private var lastPairSender: SocketAddress? = null
    private val seqCounter = AtomicInteger(1)
    private var lastKeyframeRequestAtMs = 0L

    init {
        loadHostStore()
    }

    @Synchronized
    fun start() {
        if (running.get()) return
        running.set(true)

        try {
            val vSock = DatagramSocket(Wire.VIDEO_PORT)
            vSock.receiveBufferSize = 2 * 1024 * 1024 // 2MB receive buffer for low drop
            videoSocket = vSock
        } catch (e: Exception) {
            Log.e(TAG, "Failed to bind video socket on port ${Wire.VIDEO_PORT}", e)
        }

        try {
            val pSock = DatagramSocket(Wire.PAIR_PORT)
            pairSocket = pSock
        } catch (e: Exception) {
            Log.e(TAG, "Failed to bind pair socket on port ${Wire.PAIR_PORT}", e)
        }

        try {
            controlSocket = DatagramSocket()
        } catch (e: Exception) {
            Log.e(TAG, "Failed to create control socket", e)
        }

        startPairThread()
        startVideoThread()
        updateState(State.IDLE, "Listening for hosts...")
    }

    private fun loadHostStore() {
        try {
            val file = File(context.filesDir, STORE_FILE_NAME)
            if (file.exists()) {
                val text = file.readText(StandardCharsets.UTF_8)
                hostStore = PairedHostStore.fromText(text)
                Log.i(TAG, "Loaded ${hostStore.hosts.size} paired hosts")
            }
        } catch (e: Exception) {
            Log.w(TAG, "Failed to load paired hosts: ${e.message}")
        }
    }

    private fun saveHostStore() {
        try {
            val file = File(context.filesDir, STORE_FILE_NAME)
            file.writeText(hostStore.toText(), StandardCharsets.UTF_8)
            Log.i(TAG, "Saved ${hostStore.hosts.size} paired hosts to file")
        } catch (e: Exception) {
            Log.e(TAG, "Failed to save paired hosts", e)
        }
    }

    private fun startPairThread() {
        val thread = Thread({
            val buf = ByteArray(PAIR_BUF_SIZE)
            val packet = DatagramPacket(buf, buf.size)

            while (running.get()) {
                val socket = pairSocket ?: break
                try {
                    packet.length = buf.size
                    socket.receive(packet)

                    val text = String(packet.data, packet.offset, packet.length, StandardCharsets.UTF_8).trim()
                    val sender = packet.socketAddress
                    val senderAddress = packet.address
                    handlePairMessage(text, sender, senderAddress)
                } catch (e: Exception) {
                    if (!running.get()) break
                    Log.w(TAG, "Pair receive error: ${e.message}")
                }
            }
        }, "LowlatPairThread")
        pairThread = thread
        thread.start()
    }

    private fun handlePairMessage(text: String, sender: SocketAddress, senderAddress: InetAddress) {
        val msg = PairMessage.decode(text) ?: return
        lastPairSender = sender
        val cleanHostId = sanitize(when (msg) {
            is PairMessage.Request -> msg.hostId
            is PairMessage.Resume -> msg.hostId
            else -> ""
        })

        when (msg) {
            is PairMessage.Request -> {
                // 1. If already connected to this host, re-send the ACCEPT message
                val acceptedKey = lastAcceptedKey
                if (state == State.CONNECTED && sessionToken != 0L && acceptedKey != null) {
                    val accept = PairMessage.Accept(lastAcceptedCode ?: "000000", sessionToken, acceptedKey)
                    sendPairDatagram(accept.encode(), sender)
                    return
                }

                // 2. If already awaiting allow for this exact host, do NOT re-generate code!
                val currentPending = pairingMachine.pending()
                if (currentPending != null && currentPending.hostId == cleanHostId) {
                    sendPairDatagram(PairMessage.Challenge(currentPending.code).encode(), sender)
                    return
                }

                // 3. New request
                val known = hostStore.credentials(msg.hostId)
                val challenge = pairingMachine.onRequest(msg.hostId, msg.hostName, known)
                sendPairDatagram(challenge.encode(), sender)
                currentHostName = msg.hostName
                currentHostAddress = senderAddress

                mainHandler.post {
                    updateState(State.PAIRING_CHALLENGE, "Pairing with ${msg.hostName}")
                    onPairingChallenge?.invoke(challenge.code, msg.hostName)
                }
            }
            is PairMessage.Resume -> {
                val known = hostStore.credentials(msg.hostId)
                if (known != null) {
                    currentHostName = msg.hostName
                    currentHostAddress = senderAddress
                    val challenge = pairingMachine.onRequest(msg.hostId, msg.hostName, known)
                    val accept = pairingMachine.allow()
                    if (accept != null) {
                        sessionToken = accept.token
                        lastAcceptedCode = accept.code
                        lastAcceptedKey = accept.key
                        sendPairDatagram(accept.encode(), sender)
                        sendPairDatagram(accept.encode(), sender)
                        mainHandler.post {
                            updateState(State.CONNECTED, "Connected to $currentHostName")
                        }
                        // Reset rate-limiter so the first keyframe request goes immediately
                        lastKeyframeRequestAtMs = 0L
                        requestKeyframe()
                        return
                    }
                }

                val currentPending = pairingMachine.pending()
                if (currentPending != null && currentPending.hostId == cleanHostId) {
                    sendPairDatagram(PairMessage.Challenge(currentPending.code).encode(), sender)
                    return
                }

                val challenge = pairingMachine.onRequest(msg.hostId, msg.hostName, null)
                sendPairDatagram(challenge.encode(), sender)
                currentHostName = msg.hostName
                currentHostAddress = senderAddress

                mainHandler.post {
                    updateState(State.PAIRING_CHALLENGE, "Pairing with ${msg.hostName}")
                    onPairingChallenge?.invoke(challenge.code, msg.hostName)
                }
            }
            is PairMessage.Accept -> {
                // Host sent accept (unexpected in receiver role)
            }
            is PairMessage.Reject -> {
                mainHandler.post {
                    updateState(State.DISCONNECTED, "Host rejected pairing: ${msg.reason}")
                }
            }
            is PairMessage.Challenge -> {
                // Host sent challenge (unexpected)
            }
        }
    }

    fun allowPairing() {
        ioExecutor.execute {
            val sender = lastPairSender ?: return@execute
            val accept = pairingMachine.allow() ?: return@execute
            lastAcceptedCode = accept.code
            lastAcceptedKey = accept.key

            // Send twice for UDP reliability
            sendPairDatagram(accept.encode(), sender)
            try { Thread.sleep(20) } catch (ignored: Throwable) {}
            sendPairDatagram(accept.encode(), sender)

            val pending = pairingMachine.state as? PairingMachine.State.Accepted
            if (pending != null) {
                sessionToken = pending.token
                hostStore.upsert(PairedHost(pending.hostId, pending.hostName, pending.token, pending.key))
                saveHostStore()
            }

            mainHandler.post {
                updateState(State.CONNECTED, "Connected to $currentHostName")
            }

            // Reset keyframe rate-limiter so the first request goes immediately
            lastKeyframeRequestAtMs = 0L
            // Request keyframe 3 times with short delays for reliability
            for (i in 0 until 3) {
                try { Thread.sleep(200) } catch (ignored: Throwable) {}
                lastKeyframeRequestAtMs = 0L
                requestKeyframe()
            }
        }
    }

    fun denyPairing(reason: String = "User denied") {
        ioExecutor.execute {
            val sender = lastPairSender
            val reject = pairingMachine.deny(reason)
            if (sender != null && reject != null) {
                sendPairDatagram(reject.encode(), sender)
            }
            mainHandler.post {
                updateState(State.IDLE, "Pairing denied")
            }
        }
    }

    private fun sendPairDatagram(message: String, destination: SocketAddress) {
        ioExecutor.execute {
            try {
                val bytes = (message + "\n").toByteArray(StandardCharsets.UTF_8)
                val packet = DatagramPacket(bytes, bytes.size, destination)
                pairSocket?.send(packet)
            } catch (e: Exception) {
                Log.e(TAG, "Failed to send pairing datagram: ${e.message}")
            }
        }
    }

    private fun startVideoThread() {
        val thread = Thread({
            val buf = ByteArray(VIDEO_BUF_SIZE)
            val packet = DatagramPacket(buf, buf.size)

            while (running.get()) {
                val socket = videoSocket ?: break
                try {
                    packet.length = buf.size
                    socket.receive(packet)

                    if (currentHostAddress == null) {
                        currentHostAddress = packet.address
                    }

                    val len = packet.length
                    if (len < Wire.HEADER_LEN) continue

                    // Check for DisplayConfig message (frame_id 0, packet_count 1, msg_type 0x07)
                    val displayConfig = InputMessages.decodeDisplayConfig(buf.copyOfRange(0, len))
                    if (displayConfig != null) {
                        handleDisplayConfig(displayConfig)
                        continue
                    }

                    val header = FrameHeader.decode(buf, 0, len) ?: continue
                    val payload = buf.copyOfRange(Wire.HEADER_LEN, Wire.HEADER_LEN + header.payloadLen)

                    val nowMs = System.currentTimeMillis()
                    val frame = frameAssembler.onPacket(header, payload, nowMs)
                    if (frame != null) {
                        decoder.feedFrame(frame.data, frame.timestampUs, frame.keyframe)
                        frameAssembler.noteDecoded()
                    }

                    // Check for stale drops periodically
                    val drop = frameAssembler.dropStale(nowMs)
                    if (drop.lostCurrent) {
                        requestKeyframe()
                    }
                } catch (e: Exception) {
                    if (!running.get()) break
                    Log.w(TAG, "Video receive error: ${e.message}")
                }
            }
        }, "LowlatVideoThread")
        videoThread = thread
        thread.start()
    }

    private fun handleDisplayConfig(config: InputMessages.DisplayConfig) {
        if (config.width != videoWidth || config.height != videoHeight) {
            videoWidth = config.width
            videoHeight = config.height
            Log.i(TAG, "Received DisplayConfig: ${videoWidth}x$videoHeight, orient=${config.orientation}")
            decoder.configure(videoWidth, videoHeight)
            mainHandler.post {
                onVideoSizeChanged?.invoke(videoWidth, videoHeight)
            }
            requestKeyframe()
        }
    }

    fun requestKeyframe() {
        val host = currentHostAddress ?: return
        val now = System.currentTimeMillis()
        // Rate-limit: at most once per 300ms (down from 1000ms) so decoder can recover faster
        if (now - lastKeyframeRequestAtMs < 300) return
        lastKeyframeRequestAtMs = now

        ioExecutor.execute {
            try {
                val header = FrameHeader(
                    flags = Wire.FLAG_KEYFRAME_REQUEST,
                    frameId = 0L,
                    packetIndex = 0,
                    packetCount = 0,
                    timestampUs = 0L,
                    payloadLen = 0,
                )
                val datagram = header.encode()
                // Send twice for UDP reliability
                val packet = DatagramPacket(datagram, datagram.size, host, Wire.CONTROL_PORT)
                controlSocket?.send(packet)
                controlSocket?.send(packet)
                Log.d(TAG, "keyframe request sent to $host:${Wire.CONTROL_PORT}")
            } catch (e: Exception) {
                Log.w(TAG, "Failed to send keyframe request: ${e.message}")
            }
        }
    }

    // Input sending methods
    fun sendTouchDown(normX: Int, normY: Int) {
        sendMouseMove(normX, normY)
        sendMouseButton(1) // Left click down
    }

    fun sendTouchMove(normX: Int, normY: Int) {
        sendMouseMove(normX, normY)
    }

    fun sendTouchUp(normX: Int, normY: Int) {
        sendMouseMove(normX, normY)
        sendMouseButton(0) // Left click up
    }

    fun sendMouseMove(x: Int, y: Int) {
        val seq = seqCounter.getAndIncrement() and 0xFFFF
        val data = InputMessages.mouseMove(seq, sessionToken, x, y)
        sendControlPacket(data)
    }

    fun sendMouseButton(mask: Int) {
        val seq = seqCounter.getAndIncrement() and 0xFFFF
        val data = InputMessages.mouseButton(seq, sessionToken, mask)
        sendControlPacket(data)
    }

    fun sendWheel(delta: Int) {
        val seq = seqCounter.getAndIncrement() and 0xFFFF
        val data = InputMessages.wheel(seq, sessionToken, delta)
        sendControlPacket(data)
    }

    fun sendOrientation(degrees: Int) {
        val seq = seqCounter.getAndIncrement() and 0xFFFF
        val data = InputMessages.setOrientation(seq, sessionToken, degrees)
        sendControlPacket(data)
    }

    fun sendReleaseAll() {
        val seq = seqCounter.getAndIncrement() and 0xFFFF
        val data = InputMessages.releaseAll(seq, sessionToken)
        sendControlPacket(data)
    }

    private fun sendControlPacket(data: ByteArray) {
        val host = currentHostAddress ?: return
        ioExecutor.execute {
            try {
                val packet = DatagramPacket(data, data.size, host, Wire.CONTROL_PORT)
                controlSocket?.send(packet)
            } catch (e: Exception) {
                Log.w(TAG, "Failed to send control datagram: ${e.message}")
            }
        }
    }

    private fun updateState(newState: State, detail: String? = null) {
        state = newState
        onStateChanged?.invoke(newState, detail)
    }

    private fun sanitize(value: String): String = buildString(value.length) {
        for (c in value) {
            append(if (c == '|' || c.isISOControl()) '_' else c)
        }
    }

    fun disconnect() {
        sendReleaseAll()
        currentHostAddress = null
        currentHostName = ""
        sessionToken = 0L
        lastAcceptedCode = null
        lastAcceptedKey = null
        pairingMachine.reset()
        updateState(State.IDLE, "Disconnected")
    }

    @Synchronized
    fun stop() {
        running.set(false)
        try {
            videoSocket?.close()
        } catch (ignored: Throwable) {}
        videoSocket = null

        try {
            pairSocket?.close()
        } catch (ignored: Throwable) {}
        pairSocket = null

        try {
            controlSocket?.close()
        } catch (ignored: Throwable) {}
        controlSocket = null

        videoThread = null
        pairThread = null
        decoder.stop()
        updateState(State.IDLE, "Stopped")
    }
}
