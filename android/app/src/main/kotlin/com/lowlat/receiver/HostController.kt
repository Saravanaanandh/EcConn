package com.lowlat.receiver

import android.content.Context
import android.os.Handler
import android.os.Looper
import android.util.Log
import com.lowlat.protocol.PairMessage
import com.lowlat.protocol.PairedHost
import com.lowlat.protocol.PairedHostStore
import java.io.File
import java.net.DatagramPacket
import java.net.DatagramSocket
import java.nio.charset.StandardCharsets
import java.util.concurrent.atomic.AtomicBoolean

/**
 * Initiates pairing from Android (Host role) toward a discovered Receiver
 * (Windows, Linux, or another Android device).
 */
class HostController(private val context: Context) {

    companion object {
        private const val TAG = "HostController"
        private const val STORE_FILE_NAME = "paired_receivers.txt"
    }

    private val mainHandler = Handler(Looper.getMainLooper())
    private val pairingActive = AtomicBoolean(false)
    private var store: PairedHostStore = loadStore()

    var onPairingCode: ((String) -> Unit)? = null
    var onPaired: ((LanBrowser.DiscoveredReceiver, Long) -> Unit)? = null
    var onPairingFailed: ((String) -> Unit)? = null

    fun startPairing(
        receiver: LanBrowser.DiscoveredReceiver,
        hostId: String,
        hostName: String,
    ) {
        if (pairingActive.getAndSet(true)) return

        Thread({
            var sock: DatagramSocket? = null
            try {
                sock = DatagramSocket().apply { soTimeout = 500 }
                val known = store.credentials(receiver.beacon.id)
                val req = if (known != null) {
                    PairMessage.Resume(hostId, hostName)
                } else {
                    PairMessage.Request(hostId, hostName)
                }
                val reqBytes = (req.encode() + "\n").toByteArray(StandardCharsets.UTF_8)
                val targetPort = receiver.beacon.pairPort
                sock.send(DatagramPacket(reqBytes, reqBytes.size, receiver.address, targetPort))

                val deadline = System.currentTimeMillis() + 45_000L
                var lastResend = System.currentTimeMillis()
                val buf = ByteArray(1024)
                val pkt = DatagramPacket(buf, buf.size)

                while (pairingActive.get() && System.currentTimeMillis() < deadline) {
                    val now = System.currentTimeMillis()
                    if (now - lastResend >= 3000L) {
                        sock.send(DatagramPacket(reqBytes, reqBytes.size, receiver.address, targetPort))
                        lastResend = now
                    }
                    try {
                        pkt.length = buf.size
                        sock.receive(pkt)
                        val text = String(pkt.data, pkt.offset, pkt.length, StandardCharsets.UTF_8)
                        when (val msg = PairMessage.decode(text)) {
                            is PairMessage.Challenge -> {
                                mainHandler.post { onPairingCode?.invoke(msg.code) }
                            }
                            is PairMessage.Accept -> {
                                store.upsert(
                                    PairedHost(
                                        receiver.beacon.id,
                                        receiver.beacon.name,
                                        msg.token,
                                        msg.key,
                                    )
                                )
                                saveStore()
                                pairingActive.set(false)
                                mainHandler.post { onPaired?.invoke(receiver, msg.token) }
                                return@Thread
                            }
                            is PairMessage.Reject -> {
                                pairingActive.set(false)
                                mainHandler.post { onPairingFailed?.invoke("Rejected: ${msg.reason}") }
                                return@Thread
                            }
                            else -> {}
                        }
                    } catch (_: java.net.SocketTimeoutException) {
                        // continue waiting
                    }
                }
                if (pairingActive.getAndSet(false)) {
                    mainHandler.post { onPairingFailed?.invoke("Pairing timed out (45s)") }
                }
            } catch (e: Exception) {
                Log.e(TAG, "Pairing failed", e)
                pairingActive.set(false)
                mainHandler.post { onPairingFailed?.invoke(e.message ?: "Pairing error") }
            } finally {
                try { sock?.close() } catch (_: Throwable) {}
            }
        }, "EcConnHostPair").start()
    }

    fun cancelPairing() {
        pairingActive.set(false)
    }

    private fun loadStore(): PairedHostStore {
        return try {
            val file = File(context.filesDir, STORE_FILE_NAME)
            if (file.exists()) {
                PairedHostStore.fromText(file.readText(StandardCharsets.UTF_8))
            } else {
                PairedHostStore()
            }
        } catch (_: Exception) {
            PairedHostStore()
        }
    }

    private fun saveStore() {
        try {
            val file = File(context.filesDir, STORE_FILE_NAME)
            file.writeText(store.toText(), StandardCharsets.UTF_8)
        } catch (e: Exception) {
            Log.w(TAG, "Failed to save paired receivers: ${e.message}")
        }
    }
}
