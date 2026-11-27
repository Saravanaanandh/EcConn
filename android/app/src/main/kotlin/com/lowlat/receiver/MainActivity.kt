package com.lowlat.receiver

import android.Manifest
import android.app.Activity
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.content.res.Configuration
import android.graphics.Color
import android.graphics.Typeface
import android.media.projection.MediaProjectionManager
import android.net.wifi.WifiManager
import android.os.Build
import android.os.Bundle
import android.util.Log
import android.util.TypedValue
import android.view.*
import android.widget.Button
import android.widget.FrameLayout
import android.widget.LinearLayout
import android.widget.ScrollView
import android.widget.TextView
import com.lowlat.protocol.FitMode
import com.lowlat.protocol.TouchMath
import com.lowlat.protocol.Wire

/**
 * Main Activity entry point for the EcConn Android app (unified Client + Host).
 *
 * Implements immersive fullscreen, screen-always-on, MediaCodec video rendering
 * onto a SurfaceView, Wi-Fi multicast lock, LAN advertising/browsing, touch interaction,
 * and the light sky-blue EcConn UI matching `UI_template`.
 */
class MainActivity : Activity(), SurfaceHolder.Callback {

    companion object {
        private const val TAG = "MainActivity"
        private const val REQ_MEDIA_PROJECTION = 1001
        private const val REQ_NOTIFICATIONS = 1002
    }

    enum class AppMode { WELCOME, RECEIVER, HOST }

    private var appMode = AppMode.WELCOME

    private lateinit var decoder: H264Decoder
    private lateinit var advertiser: Advertiser
    private lateinit var controller: ReceiverController
    private lateinit var lanBrowser: LanBrowser
    private lateinit var hostController: HostController

    private var multicastLock: WifiManager.MulticastLock? = null
    private var discoveredReceivers: List<LanBrowser.DiscoveredReceiver> = emptyList()
    private var selectedReceiver: LanBrowser.DiscoveredReceiver? = null
    private var pendingShareTarget: LanBrowser.DiscoveredReceiver? = null
    private var activeShareTargetName: String = "Client Device"
    private var hostPairingCode: String? = null
    private var hostStatusNote: String? = null
    private var showHostConnectionInfo: Boolean = false
    private var showClientConnectionInfo: Boolean = false
    private var wasClientConnected: Boolean = false
    private var lastHostStatsText: String = "Live stream"
    private var lastVideoSizeText: String = ""

    // Root & streaming views
    private lateinit var rootLayout: FrameLayout
    private lateinit var surfaceView: SurfaceView
    private lateinit var screenScroll: ScrollView
    private lateinit var screenContainer: LinearLayout

    // Screen containers
    private lateinit var welcomeScreen: LinearLayout
    private lateinit var clientWaitingScreen: LinearLayout
    private lateinit var clientWaitingStatusText: TextView
    private lateinit var hostFindScreen: LinearLayout
    private lateinit var hostDevicesContainer: LinearLayout
    private lateinit var hostStatusBanner: TextView
    private lateinit var hostConnectedScreen: LinearLayout
    private lateinit var hostConnectedPeerText: TextView
    private lateinit var hostTileFpsText: TextView
    private lateinit var hostTileResText: TextView
    private lateinit var hostTileOrientText: TextView
    private lateinit var hostConnInfoCard: LinearLayout
    private lateinit var hostConnInfoDetail: TextView
    private lateinit var confirmScreen: LinearLayout
    private lateinit var confirmSubtitleText: TextView
    private lateinit var confirmCodeText: TextView
    private lateinit var confirmWaitingNote: TextView
    private lateinit var confirmAllowBtn: Button
    private lateinit var confirmCancelBtn: Button

    // Floating streaming controls (Client Connected)
    private lateinit var streamingBar: LinearLayout
    private lateinit var streamingInfoText: TextView

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)

        window.addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)

        decoder = H264Decoder()
        advertiser = Advertiser(this)
        controller = ReceiverController(this, decoder)
        decoder.onRequestKeyframe = { controller.requestKeyframe() }
        lanBrowser = LanBrowser(this, DisplayInfo.getDeviceId(this))
        hostController = HostController(this)

        requestNotificationPermissionIfNeeded()
        buildUi()
        setupListeners()
        updateVisibleScreen()
    }

    private fun buildUi() {
        window.decorView.setBackgroundColor(EcConnColors.BG_CANVAS)

        rootLayout = FrameLayout(this).apply {
            setBackgroundColor(EcConnColors.BG_CANVAS)
            layoutParams = ViewGroup.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT,
                ViewGroup.LayoutParams.MATCH_PARENT
            )
        }

        // 1. SurfaceView for video rendering (always attached so SurfaceHolder stays valid)
        surfaceView = SurfaceView(this).apply {
            layoutParams = FrameLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT,
                ViewGroup.LayoutParams.MATCH_PARENT
            )
        }
        surfaceView.holder.addCallback(this)
        rootLayout.addView(surfaceView)

        // 2. Light Sky-Blue Scrollable UI Overlay
        screenScroll = ScrollView(this).apply {
            setBackgroundColor(EcConnColors.BG_CANVAS)
            isFillViewport = true
            layoutParams = FrameLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT,
                ViewGroup.LayoutParams.MATCH_PARENT
            )
        }

        screenContainer = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            setPadding(dp(22), dp(24), dp(22), dp(24))
            layoutParams = FrameLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT,
                ViewGroup.LayoutParams.WRAP_CONTENT
            )
        }

        buildWelcomeScreen()
        buildClientWaitingScreen()
        buildHostFindScreen()
        buildHostConnectedScreen()
        buildConfirmScreen()

        screenContainer.addView(welcomeScreen)
        screenContainer.addView(clientWaitingScreen)
        screenContainer.addView(hostFindScreen)
        screenContainer.addView(hostConnectedScreen)
        screenContainer.addView(confirmScreen)

        screenScroll.addView(screenContainer)
        rootLayout.addView(screenScroll)

        // 3. Floating Client Connected Bar (matches Client Mode.png)
        streamingBar = LinearLayout(this).apply {
            orientation = LinearLayout.HORIZONTAL
            gravity = Gravity.CENTER_VERTICAL
            background = makeRoundedDrawable(
                fillColor = Color.argb(235, 235, 243, 252),
                strokeColor = EcConnColors.CARD_BORDER,
                strokeWidthPx = dp(1),
                radiusPx = dpF(24f)
            )
            setPadding(dp(16), dp(8), dp(10), dp(8))
            visibility = View.GONE
            layoutParams = FrameLayout.LayoutParams(
                ViewGroup.LayoutParams.WRAP_CONTENT,
                ViewGroup.LayoutParams.WRAP_CONTENT
            ).apply {
                gravity = Gravity.TOP or Gravity.END
                setMargins(dp(16), dp(16), dp(16), 0)
            }
        }

        streamingInfoText = TextView(this).apply {
            text = "Connection info"
            setTextColor(EcConnColors.BRAND_BLUE)
            setTextSize(TypedValue.COMPLEX_UNIT_SP, 13f)
            typeface = Typeface.create("sans-serif-medium", Typeface.BOLD)
            setPadding(0, 0, dp(12), 0)
            setOnClickListener {
                showClientConnectionInfo = !showClientConnectionInfo
                text = if (showClientConnectionInfo) {
                    val res = if (lastVideoSizeText.isNotEmpty()) " · $lastVideoSizeText" else ""
                    "${controller.currentHostName}$res"
                } else {
                    "Connection info"
                }
            }
        }
        streamingBar.addView(streamingInfoText)

        val btnClientDisconnect = makeDangerPillButton(this, "Disconnect").apply {
            textSize = 12.5f
            setPadding(dp(14), dp(5), dp(14), dp(5))
            setOnClickListener { controller.disconnect() }
        }
        streamingBar.addView(btnClientDisconnect, LinearLayout.LayoutParams(
            ViewGroup.LayoutParams.WRAP_CONTENT,
            dp(34)
        ))
        rootLayout.addView(streamingBar)

        setContentView(rootLayout)
    }

    private fun buildWelcomeScreen() {
        welcomeScreen = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            layoutParams = LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT,
                ViewGroup.LayoutParams.WRAP_CONTENT
            )
        }

        welcomeScreen.addView(makeEcConnHeader(this, "EcConn"))

        welcomeScreen.addView(makeEcConnHeroImageView(this), LinearLayout.LayoutParams(
            ViewGroup.LayoutParams.MATCH_PARENT,
            dp(210)
        ).apply {
            topMargin = dp(12)
            bottomMargin = dp(12)
        })

        welcomeScreen.addView(TextView(this).apply {
            text = "Connect and extend\nanything easily."
            setTextColor(EcConnColors.NAVY)
            setTextSize(TypedValue.COMPLEX_UNIT_SP, 22f)
            typeface = Typeface.create("sans-serif-medium", Typeface.BOLD)
            setPadding(0, 0, 0, dp(22))
        })

        val buttonsRow = LinearLayout(this).apply {
            orientation = LinearLayout.HORIZONTAL
            layoutParams = LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT,
                ViewGroup.LayoutParams.WRAP_CONTENT
            ).apply {
                bottomMargin = dp(24)
            }
        }

        val btnClient = makePrimaryPillButton(this, "Client").apply {
            setOnClickListener { switchMode(AppMode.RECEIVER) }
        }
        buttonsRow.addView(btnClient, LinearLayout.LayoutParams(0, dp(48), 1f).apply {
            rightMargin = dp(8)
        })

        val btnHost = makeOutlinePillButton(this, "Host").apply {
            setOnClickListener { switchMode(AppMode.HOST) }
        }
        buttonsRow.addView(btnHost, LinearLayout.LayoutParams(0, dp(48), 1f).apply {
            leftMargin = dp(8)
        })

        welcomeScreen.addView(buttonsRow)
        welcomeScreen.addView(makeWifiBanner(this))
    }

    private fun buildClientWaitingScreen() {
        clientWaitingScreen = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            layoutParams = LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT,
                ViewGroup.LayoutParams.WRAP_CONTENT
            )
        }

        val backBtn = makeOutlinePillButton(this, "← Back").apply {
            textSize = 13f
            setPadding(dp(14), dp(4), dp(14), dp(4))
            setOnClickListener { switchMode(AppMode.WELCOME) }
        }
        clientWaitingScreen.addView(backBtn, LinearLayout.LayoutParams(
            ViewGroup.LayoutParams.WRAP_CONTENT,
            dp(36)
        ).apply {
            bottomMargin = dp(14)
        })

        clientWaitingScreen.addView(makeEcConnHeader(this, "Client"))

        clientWaitingScreen.addView(TextView(this).apply {
            text = "Waiting for Host to Connect"
            setTextColor(EcConnColors.NAVY)
            setTextSize(TypedValue.COMPLEX_UNIT_SP, 20f)
            typeface = Typeface.create("sans-serif-medium", Typeface.BOLD)
            setPadding(0, dp(16), 0, dp(6))
        })

        clientWaitingScreen.addView(TextView(this).apply {
            text = "Device Name: ${DisplayInfo.getDeviceName()}"
            setTextColor(EcConnColors.SLATE)
            setTextSize(TypedValue.COMPLEX_UNIT_SP, 14f)
            setPadding(0, 0, 0, dp(8))
        })

        clientWaitingStatusText = TextView(this).apply {
            text = "Loading…"
            setTextColor(EcConnColors.BRAND_BLUE)
            setTextSize(TypedValue.COMPLEX_UNIT_SP, 14f)
            typeface = Typeface.create("sans-serif-medium", Typeface.BOLD)
            setPadding(0, 0, 0, dp(14))
        }
        clientWaitingScreen.addView(clientWaitingStatusText)

        clientWaitingScreen.addView(makeEcConnHeroImageView(this), LinearLayout.LayoutParams(
            ViewGroup.LayoutParams.MATCH_PARENT,
            dp(190)
        ).apply {
            bottomMargin = dp(18)
        })

        clientWaitingScreen.addView(makeWifiBanner(this))
    }

    private fun buildHostFindScreen() {
        hostFindScreen = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            layoutParams = LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT,
                ViewGroup.LayoutParams.WRAP_CONTENT
            )
        }

        val backBtn = makeOutlinePillButton(this, "← Back").apply {
            textSize = 13f
            setPadding(dp(14), dp(4), dp(14), dp(4))
            setOnClickListener { switchMode(AppMode.WELCOME) }
        }
        hostFindScreen.addView(backBtn, LinearLayout.LayoutParams(
            ViewGroup.LayoutParams.WRAP_CONTENT,
            dp(36)
        ).apply {
            bottomMargin = dp(14)
        })

        hostFindScreen.addView(makeEcConnHeader(this, "Host"))

        hostFindScreen.addView(TextView(this).apply {
            text = "Searching for Client Devices"
            setTextColor(EcConnColors.NAVY)
            setTextSize(TypedValue.COMPLEX_UNIT_SP, 20f)
            typeface = Typeface.create("sans-serif-medium", Typeface.BOLD)
            setPadding(0, dp(14), 0, dp(4))
        })

        hostFindScreen.addView(TextView(this).apply {
            text = "Host Device : ${DisplayInfo.getDeviceName()}"
            setTextColor(EcConnColors.SLATE)
            setTextSize(TypedValue.COMPLEX_UNIT_SP, 13.5f)
            setPadding(0, 0, 0, dp(16))
        })

        val listHeaderRow = LinearLayout(this).apply {
            orientation = LinearLayout.HORIZONTAL
            gravity = Gravity.CENTER_VERTICAL
            layoutParams = LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT,
                ViewGroup.LayoutParams.WRAP_CONTENT
            ).apply {
                bottomMargin = dp(10)
            }
        }
        listHeaderRow.addView(TextView(this).apply {
            text = "Available Devices"
            setTextColor(EcConnColors.NAVY)
            setTextSize(TypedValue.COMPLEX_UNIT_SP, 16f)
            typeface = Typeface.create("sans-serif-medium", Typeface.BOLD)
            layoutParams = LinearLayout.LayoutParams(0, ViewGroup.LayoutParams.WRAP_CONTENT, 1f)
        })
        val refreshBtn = makePrimaryPillButton(this, "Refresh").apply {
            textSize = 13f
            setPadding(dp(16), dp(4), dp(16), dp(4))
            setOnClickListener {
                lanBrowser.stop()
                lanBrowser.start()
                renderHostDevices()
            }
        }
        listHeaderRow.addView(refreshBtn, LinearLayout.LayoutParams(
            ViewGroup.LayoutParams.WRAP_CONTENT,
            dp(34)
        ))
        hostFindScreen.addView(listHeaderRow)

        hostDevicesContainer = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            layoutParams = LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT,
                ViewGroup.LayoutParams.WRAP_CONTENT
            ).apply {
                bottomMargin = dp(10)
            }
        }
        hostFindScreen.addView(hostDevicesContainer)

        hostStatusBanner = TextView(this).apply {
            setTextColor(EcConnColors.DANGER_RED)
            setTextSize(TypedValue.COMPLEX_UNIT_SP, 13f)
            visibility = View.GONE
            setPadding(0, 0, 0, dp(12))
        }
        hostFindScreen.addView(hostStatusBanner)

        hostFindScreen.addView(makeWifiBanner(this))
    }

    private fun buildHostConnectedScreen() {
        hostConnectedScreen = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            layoutParams = LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT,
                ViewGroup.LayoutParams.WRAP_CONTENT
            )
        }

        hostConnectedScreen.addView(makeEcConnHeader(this, "Host"))

        hostConnectedScreen.addView(TextView(this).apply {
            text = "Connected Successfully"
            setTextColor(EcConnColors.NAVY)
            setTextSize(TypedValue.COMPLEX_UNIT_SP, 21f)
            typeface = Typeface.create("sans-serif-medium", Typeface.BOLD)
            setPadding(0, dp(14), 0, dp(4))
        })

        val connInfoLink = TextView(this).apply {
            text = "Connection Info →"
            setTextColor(EcConnColors.BRAND_BLUE)
            setTextSize(TypedValue.COMPLEX_UNIT_SP, 14f)
            typeface = Typeface.create("sans-serif-medium", Typeface.BOLD)
            setPadding(0, 0, 0, dp(14))
            setOnClickListener {
                showHostConnectionInfo = !showHostConnectionInfo
                hostConnInfoCard.visibility = if (showHostConnectionInfo) View.VISIBLE else View.GONE
            }
        }
        hostConnectedScreen.addView(connInfoLink)

        // Soft blue Stream settings card
        val streamCard = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            background = makeRoundedDrawable(
                fillColor = EcConnColors.CARD_BLUE,
                strokeColor = EcConnColors.CARD_BORDER,
                strokeWidthPx = dp(1),
                radiusPx = dpF(16f)
            )
            setPadding(dp(16), dp(16), dp(16), dp(16))
            layoutParams = LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT,
                ViewGroup.LayoutParams.WRAP_CONTENT
            ).apply {
                bottomMargin = dp(14)
            }
        }

        streamCard.addView(TextView(this).apply {
            text = "Stream settings"
            setTextColor(EcConnColors.NAVY)
            setTextSize(TypedValue.COMPLEX_UNIT_SP, 16f)
            typeface = Typeface.create("sans-serif-medium", Typeface.BOLD)
        })

        hostConnectedPeerText = TextView(this).apply {
            text = "Connected to Client Device"
            setTextColor(EcConnColors.SLATE)
            setTextSize(TypedValue.COMPLEX_UNIT_SP, 13f)
            setPadding(0, dp(2), 0, dp(12))
        }
        streamCard.addView(hostConnectedPeerText)

        val tilesRow = LinearLayout(this).apply {
            orientation = LinearLayout.HORIZONTAL
            layoutParams = LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT,
                ViewGroup.LayoutParams.WRAP_CONTENT
            )
        }

        fun makeTile(initialText: String): Pair<LinearLayout, TextView> {
            val tv = TextView(this).apply {
                text = initialText
                setTextColor(EcConnColors.NAVY)
                setTextSize(TypedValue.COMPLEX_UNIT_SP, 13f)
                typeface = Typeface.create("sans-serif-medium", Typeface.BOLD)
                gravity = Gravity.CENTER
            }
            val box = LinearLayout(this).apply {
                gravity = Gravity.CENTER
                background = makeRoundedDrawable(
                    fillColor = EcConnColors.CARD_WHITE,
                    strokeColor = EcConnColors.CARD_BORDER,
                    strokeWidthPx = dp(1),
                    radiusPx = dpF(12f)
                )
                setPadding(dp(10), dp(10), dp(10), dp(10))
                addView(tv)
            }
            return box to tv
        }

        val (tileFps, tvFps) = makeTile("FPS 30")
        val (tileRes, tvRes) = makeTile("Mirror")
        val (tileOrient, tvOrient) = makeTile("0°")
        hostTileFpsText = tvFps
        hostTileResText = tvRes
        hostTileOrientText = tvOrient

        tilesRow.addView(tileFps, LinearLayout.LayoutParams(0, ViewGroup.LayoutParams.WRAP_CONTENT, 1f).apply {
            rightMargin = dp(6)
        })
        tilesRow.addView(tileRes, LinearLayout.LayoutParams(0, ViewGroup.LayoutParams.WRAP_CONTENT, 1f).apply {
            rightMargin = dp(6)
        })
        tilesRow.addView(tileOrient, LinearLayout.LayoutParams(0, ViewGroup.LayoutParams.WRAP_CONTENT, 1f))
        streamCard.addView(tilesRow)
        hostConnectedScreen.addView(streamCard)

        // Expandable Connection Info card
        hostConnInfoCard = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            background = makeRoundedDrawable(
                fillColor = EcConnColors.CARD_WHITE,
                strokeColor = EcConnColors.CARD_BORDER,
                strokeWidthPx = dp(1),
                radiusPx = dpF(12f)
            )
            setPadding(dp(14), dp(12), dp(14), dp(12))
            visibility = View.GONE
            layoutParams = LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT,
                ViewGroup.LayoutParams.WRAP_CONTENT
            ).apply {
                bottomMargin = dp(14)
            }
        }
        hostConnInfoDetail = TextView(this).apply {
            text = lastHostStatsText
            setTextColor(EcConnColors.SLATE)
            setTextSize(TypedValue.COMPLEX_UNIT_SP, 12.5f)
        }
        hostConnInfoCard.addView(hostConnInfoDetail)
        hostConnectedScreen.addView(hostConnInfoCard)

        val btnDisconnectHost = makeDangerPillButton(this, "Disconnect").apply {
            setOnClickListener { stopPhoneScreenShare() }
        }
        hostConnectedScreen.addView(btnDisconnectHost, LinearLayout.LayoutParams(
            dp(170),
            dp(46)
        ).apply {
            bottomMargin = dp(16)
        })

        hostConnectedScreen.addView(makeEcConnHeroImageView(this), LinearLayout.LayoutParams(
            ViewGroup.LayoutParams.MATCH_PARENT,
            dp(165)
        ))
    }

    private fun buildConfirmScreen() {
        confirmScreen = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            layoutParams = LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT,
                ViewGroup.LayoutParams.WRAP_CONTENT
            )
        }

        confirmScreen.addView(makeEcConnHeader(this, "EcConn"))

        confirmScreen.addView(TextView(this).apply {
            text = "Confirm connection"
            setTextColor(EcConnColors.NAVY)
            setTextSize(TypedValue.COMPLEX_UNIT_SP, 22f)
            typeface = Typeface.create("sans-serif-medium", Typeface.BOLD)
            setPadding(0, dp(18), 0, dp(8))
        })

        confirmSubtitleText = TextView(this).apply {
            text = "Verify the secret code shown on the other device. The code must match on both devices."
            setTextColor(EcConnColors.SLATE)
            setTextSize(TypedValue.COMPLEX_UNIT_SP, 14f)
            setPadding(0, 0, 0, dp(22))
        }
        confirmScreen.addView(confirmSubtitleText)

        confirmCodeText = TextView(this).apply {
            text = "000 000"
            setTextColor(EcConnColors.BRAND_BLUE)
            setTextSize(TypedValue.COMPLEX_UNIT_SP, 34f)
            typeface = Typeface.MONOSPACE
            setPadding(0, dp(8), 0, dp(6))
        }
        confirmScreen.addView(confirmCodeText)

        // Blue underline under the code (matches Confirm Connection.png)
        confirmScreen.addView(View(this).apply {
            setBackgroundColor(EcConnColors.BRAND_BLUE)
            layoutParams = LinearLayout.LayoutParams(dp(160), dp(2)).apply {
                bottomMargin = dp(10)
            }
        })

        confirmWaitingNote = TextView(this).apply {
            text = ""
            setTextColor(EcConnColors.SLATE)
            setTextSize(TypedValue.COMPLEX_UNIT_SP, 13f)
            visibility = View.GONE
            setPadding(0, 0, 0, dp(16))
        }
        confirmScreen.addView(confirmWaitingNote)

        val row = LinearLayout(this).apply {
            orientation = LinearLayout.HORIZONTAL
            layoutParams = LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT,
                ViewGroup.LayoutParams.WRAP_CONTENT
            ).apply {
                topMargin = dp(12)
                bottomMargin = dp(24)
            }
        }

        confirmAllowBtn = makePrimaryPillButton(this, "Allow").apply {
            setOnClickListener {
                controller.allowPairing()
                updateVisibleScreen()
            }
        }
        row.addView(confirmAllowBtn, LinearLayout.LayoutParams(0, dp(46), 1f).apply {
            rightMargin = dp(8)
        })

        confirmCancelBtn = makeOutlinePillButton(this, "Cancel").apply {
            setOnClickListener {
                if (appMode == AppMode.RECEIVER) {
                    controller.denyPairing()
                } else {
                    hostController.cancelPairing()
                    hostPairingCode = null
                }
                updateVisibleScreen()
            }
        }
        row.addView(confirmCancelBtn, LinearLayout.LayoutParams(0, dp(46), 1f).apply {
            leftMargin = dp(8)
        })

        confirmScreen.addView(row)
        confirmScreen.addView(makeEcConnHeroImageView(this), LinearLayout.LayoutParams(
            ViewGroup.LayoutParams.MATCH_PARENT,
            dp(160)
        ))
    }

    private fun updateVisibleScreen() {
        val isClientStreaming = appMode == AppMode.RECEIVER && controller.state == ReceiverController.State.CONNECTED
        val isClientPairing = appMode == AppMode.RECEIVER && controller.state == ReceiverController.State.PAIRING_CHALLENGE
        val isHostPairing = appMode == AppMode.HOST && hostPairingCode != null
        val isHostStreaming = appMode == AppMode.HOST && ScreenShareService.isStreaming

        if (isClientStreaming) {
            screenScroll.visibility = View.GONE
            streamingInfoText.text = if (showClientConnectionInfo) {
                val res = if (lastVideoSizeText.isNotEmpty()) " · $lastVideoSizeText" else ""
                "${controller.currentHostName}$res"
            } else {
                "Connection info"
            }
            streamingBar.visibility = View.VISIBLE
            return
        }

        screenScroll.visibility = View.VISIBLE
        streamingBar.visibility = View.GONE

        welcomeScreen.visibility = if (appMode == AppMode.WELCOME) View.VISIBLE else View.GONE
        clientWaitingScreen.visibility =
            if (appMode == AppMode.RECEIVER && !isClientPairing) View.VISIBLE else View.GONE
        hostFindScreen.visibility =
            if (appMode == AppMode.HOST && !isHostPairing && !isHostStreaming) View.VISIBLE else View.GONE
        hostConnectedScreen.visibility =
            if (isHostStreaming) View.VISIBLE else View.GONE
        confirmScreen.visibility =
            if (isClientPairing || isHostPairing) View.VISIBLE else View.GONE

        if (isHostPairing) {
            confirmAllowBtn.visibility = View.GONE
            confirmWaitingNote.text = "Waiting for Allow on client device…"
            confirmWaitingNote.visibility = View.VISIBLE
            confirmCodeText.text = hostPairingCode ?: ""
        } else if (isClientPairing) {
            confirmAllowBtn.visibility = View.VISIBLE
            confirmWaitingNote.visibility = View.GONE
        }

        if (isHostStreaming) {
            val (w, h) = DisplayInfo.getScreenDimensions(this)
            val orient = DisplayInfo.getOrientationDegrees(this)
            hostConnectedPeerText.text = "Connected to $activeShareTargetName"
            hostTileResText.text = "${w}×${h}"
            hostTileOrientText.text = "${orient}°"
            hostConnInfoDetail.text = "$activeShareTargetName · $lastHostStatsText"
        }
    }

    private fun setupListeners() {
        controller.onStateChanged = { state, detail ->
            runOnUiThread {
                if (state == ReceiverController.State.CONNECTED) {
                    wasClientConnected = true
                    showClientConnectionInfo = false
                } else if (wasClientConnected &&
                    (state == ReceiverController.State.IDLE || state == ReceiverController.State.DISCONNECTED)
                ) {
                    wasClientConnected = false
                    switchMode(AppMode.WELCOME)
                    return@runOnUiThread
                }
                clientWaitingStatusText.text = when (state) {
                    ReceiverController.State.IDLE -> detail ?: "Loading…"
                    ReceiverController.State.PAIRING_CHALLENGE -> "Confirm pairing…"
                    ReceiverController.State.CONNECTED -> "Connected"
                    ReceiverController.State.DISCONNECTED -> detail ?: "Disconnected"
                }
                updateVisibleScreen()
            }
        }

        controller.onPairingChallenge = { code, hostName ->
            runOnUiThread {
                val formatted = if (code.length == 6) {
                    "${code.substring(0, 3)} ${code.substring(3)}"
                } else code
                confirmCodeText.text = formatted
                confirmSubtitleText.text =
                    "Host '$hostName' wants to connect. The code must match on both devices."
                updateVisibleScreen()
            }
        }

        controller.onVideoSizeChanged = { w, h ->
            Log.i(TAG, "Video stream resolution changed to ${w}x${h}")
            runOnUiThread {
                lastVideoSizeText = "${w}×${h}"
                streamingInfoText.text = "${controller.currentHostName} · $lastVideoSizeText"
            }
        }

        surfaceView.setOnTouchListener { v, event ->
            handleTouchEvent(v, event)
            true
        }

        rootLayout.setOnClickListener {
            if (appMode == AppMode.RECEIVER && controller.state == ReceiverController.State.CONNECTED) {
                streamingBar.visibility = View.VISIBLE
                streamingBar.postDelayed({
                    if (controller.state == ReceiverController.State.CONNECTED) {
                        streamingBar.visibility = View.GONE
                    }
                }, 3500L)
            }
        }

        lanBrowser.onDevicesChanged = { list ->
            discoveredReceivers = list
            if (selectedReceiver == null || list.none { it.beacon.id == selectedReceiver?.beacon?.id }) {
                selectedReceiver = list.firstOrNull()
            }
            if (appMode == AppMode.HOST) {
                renderHostDevices()
            }
        }

        hostController.onPairingCode = { code ->
            hostPairingCode = if (code.length == 6) "${code.substring(0, 3)} ${code.substring(3)}" else code
            confirmSubtitleText.text =
                "Verify the secret code shown on the client device. The code must match on both devices."
            updateVisibleScreen()
        }

        hostController.onPaired = { receiver, _ ->
            hostPairingCode = null
            activeShareTargetName = receiver.beacon.name
            pendingShareTarget = receiver
            val mgr = getSystemService(Context.MEDIA_PROJECTION_SERVICE) as MediaProjectionManager
            @Suppress("DEPRECATION")
            startActivityForResult(mgr.createScreenCaptureIntent(), REQ_MEDIA_PROJECTION)
        }

        hostController.onPairingFailed = { reason ->
            hostPairingCode = null
            hostStatusNote = reason
            hostStatusBanner.text = reason
            hostStatusBanner.visibility = View.VISIBLE
            updateVisibleScreen()
        }

        ScreenShareService.onStatsUpdate = { frames, kbps ->
            runOnUiThread {
                lastHostStatsText = "$frames frames · ${kbps.toInt()} kbps"
                if (appMode == AppMode.HOST && ScreenShareService.isStreaming) {
                    hostTileFpsText.text = "${kbps.toInt()}k"
                    hostConnInfoDetail.text = "$activeShareTargetName · $lastHostStatsText"
                }
            }
        }

        ScreenShareService.onStopped = { reason ->
            runOnUiThread {
                hostStatusNote = reason
                if (appMode == AppMode.HOST) {
                    switchMode(AppMode.WELCOME)
                }
            }
        }
    }

    private fun switchMode(target: AppMode) {
        if (appMode == target) return
        appMode = target

        when (target) {
            AppMode.WELCOME -> {
                controller.stop()
                advertiser.stop()
                stopPhoneScreenShare()
                hostController.cancelPairing()
                lanBrowser.stop()
                hostPairingCode = null
            }
            AppMode.HOST -> {
                controller.stop()
                advertiser.stop()
                hostPairingCode = null
                hostStatusBanner.visibility = View.GONE
                lanBrowser.start()
                renderHostDevices()
            }
            AppMode.RECEIVER -> {
                stopPhoneScreenShare()
                hostController.cancelPairing()
                lanBrowser.stop()
                hostPairingCode = null

                val (w, h) = DisplayInfo.getScreenDimensions(this)
                val orient = DisplayInfo.getOrientationDegrees(this)
                advertiser.start(w, h, orient)
                controller.start()
            }
        }
        updateVisibleScreen()
    }

    private fun renderHostDevices() {
        hostDevicesContainer.removeAllViews()
        if (discoveredReceivers.isEmpty()) {
            val emptyCard = LinearLayout(this).apply {
                orientation = LinearLayout.VERTICAL
                background = makeRoundedDrawable(
                    fillColor = EcConnColors.CARD_BLUE,
                    strokeColor = EcConnColors.CARD_BORDER,
                    strokeWidthPx = dp(1),
                    radiusPx = dpF(14f)
                )
                setPadding(dp(16), dp(18), dp(16), dp(18))
                addView(TextView(this@MainActivity).apply {
                    text = "Scanning LAN for EcConn clients…"
                    setTextColor(EcConnColors.SLATE)
                    setTextSize(TypedValue.COMPLEX_UNIT_SP, 13.5f)
                    gravity = Gravity.CENTER
                })
            }
            hostDevicesContainer.addView(emptyCard)
            return
        }

        for (dev in discoveredReceivers) {
            val row = LinearLayout(this).apply {
                orientation = LinearLayout.HORIZONTAL
                gravity = Gravity.CENTER_VERTICAL
                background = makeRoundedDrawable(
                    fillColor = EcConnColors.CARD_WHITE,
                    strokeColor = EcConnColors.CARD_BORDER,
                    strokeWidthPx = dp(1),
                    radiusPx = dpF(14f)
                )
                setPadding(dp(14), dp(10), dp(12), dp(10))
                layoutParams = LinearLayout.LayoutParams(
                    ViewGroup.LayoutParams.MATCH_PARENT,
                    ViewGroup.LayoutParams.WRAP_CONTENT
                ).apply {
                    bottomMargin = dp(8)
                }
            }

            val textCol = LinearLayout(this).apply {
                orientation = LinearLayout.VERTICAL
                layoutParams = LinearLayout.LayoutParams(0, ViewGroup.LayoutParams.WRAP_CONTENT, 1f)
            }
            textCol.addView(TextView(this).apply {
                text = dev.beacon.name
                setTextColor(EcConnColors.NAVY)
                setTextSize(TypedValue.COMPLEX_UNIT_SP, 15f)
                typeface = Typeface.create("sans-serif-medium", Typeface.BOLD)
            })
            textCol.addView(TextView(this).apply {
                text = "${dev.address.hostAddress} · ${dev.beacon.width}×${dev.beacon.height} · ${dev.beacon.platform}"
                setTextColor(EcConnColors.SLATE)
                setTextSize(TypedValue.COMPLEX_UNIT_SP, 11.5f)
            })
            row.addView(textCol)

            val connectBtn = makeOutlinePillButton(this, "connect").apply {
                textSize = 13f
                setPadding(dp(16), dp(4), dp(16), dp(4))
                setOnClickListener {
                    selectedReceiver = dev
                    startPairingForTarget(dev)
                }
            }
            row.addView(connectBtn, LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.WRAP_CONTENT,
                dp(34)
            ))

            hostDevicesContainer.addView(row)
        }
    }

    private fun startPairingForTarget(target: LanBrowser.DiscoveredReceiver) {
        hostStatusBanner.visibility = View.GONE
        hostPairingCode = "··· ···"
        confirmSubtitleText.text = "Connecting to ${target.beacon.name}…"
        updateVisibleScreen()
        hostController.startPairing(target, DisplayInfo.getDeviceId(this), DisplayInfo.getDeviceName())
    }

    private fun stopPhoneScreenShare() {
        val wasStreaming = ScreenShareService.isStreaming
        val stopIntent = Intent(this, ScreenShareService::class.java).apply {
            action = ScreenShareService.ACTION_STOP
        }
        startService(stopIntent)
        if (wasStreaming && appMode == AppMode.HOST) {
            switchMode(AppMode.WELCOME)
        } else {
            updateVisibleScreen()
        }
    }

    @Deprecated("Deprecated in Java")
    override fun onActivityResult(requestCode: Int, resultCode: Int, data: Intent?) {
        @Suppress("DEPRECATION")
        super.onActivityResult(requestCode, resultCode, data)
        if (requestCode == REQ_MEDIA_PROJECTION) {
            val target = pendingShareTarget
            pendingShareTarget = null
            if (resultCode == RESULT_OK && data != null && target != null) {
                val (w, h) = DisplayInfo.getScreenDimensions(this)
                val svcIntent = Intent(this, ScreenShareService::class.java).apply {
                    action = ScreenShareService.ACTION_START
                    putExtra(ScreenShareService.EXTRA_RESULT_CODE, resultCode)
                    putExtra(ScreenShareService.EXTRA_DATA, data)
                    putExtra(ScreenShareService.EXTRA_DEST_IP, target.address.hostAddress)
                    putExtra(ScreenShareService.EXTRA_DEST_PORT, target.beacon.videoPort)
                    putExtra(ScreenShareService.EXTRA_WIDTH, w)
                    putExtra(ScreenShareService.EXTRA_HEIGHT, h)
                }
                if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
                    startForegroundService(svcIntent)
                } else {
                    startService(svcIntent)
                }
                hostTileFpsText.text = "FPS 30"
                updateVisibleScreen()
            } else {
                hostStatusBanner.text = "Screen capture cancelled"
                hostStatusBanner.visibility = View.VISIBLE
                updateVisibleScreen()
            }
        }
    }

    private fun requestNotificationPermissionIfNeeded() {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            if (checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS) != PackageManager.PERMISSION_GRANTED) {
                requestPermissions(arrayOf(Manifest.permission.POST_NOTIFICATIONS), REQ_NOTIFICATIONS)
            }
        }
    }

    private fun handleTouchEvent(view: View, event: MotionEvent) {
        val vw = view.width.toFloat()
        val vh = view.height.toFloat()
        val vidW = controller.videoWidth
        val vidH = controller.videoHeight

        val rect = TouchMath.rect(vw, vh, vidW, vidH, FitMode.Fit)
        val (normX, normY) = TouchMath.normalize(event.x, event.y, rect)

        when (event.actionMasked) {
            MotionEvent.ACTION_DOWN -> controller.sendTouchDown(normX, normY)
            MotionEvent.ACTION_MOVE -> controller.sendTouchMove(normX, normY)
            MotionEvent.ACTION_UP, MotionEvent.ACTION_CANCEL -> controller.sendTouchUp(normX, normY)
        }
    }

    override fun onStart() {
        super.onStart()
        acquireMulticastLock()

        if (appMode == AppMode.RECEIVER) {
            val (w, h) = DisplayInfo.getScreenDimensions(this)
            val orient = DisplayInfo.getOrientationDegrees(this)
            advertiser.start(w, h, orient)
            controller.start()
        } else if (appMode == AppMode.HOST) {
            lanBrowser.start()
        }
    }

    override fun onResume() {
        super.onResume()
        applyImmersiveMode()
    }

    override fun onStop() {
        super.onStop()
        if (appMode == AppMode.RECEIVER) {
            controller.stop()
            advertiser.stop()
        } else if (appMode == AppMode.HOST) {
            lanBrowser.stop()
        }
        releaseMulticastLock()
    }

    override fun onDestroy() {
        super.onDestroy()
        decoder.stop()
    }

    override fun onConfigurationChanged(newConfig: Configuration) {
        super.onConfigurationChanged(newConfig)
        val (w, h) = DisplayInfo.getScreenDimensions(this)
        val orient = DisplayInfo.getOrientationDegrees(this)
        advertiser.updateDisplay(w, h, orient)

        if (controller.state == ReceiverController.State.CONNECTED) {
            controller.sendOrientation(orient)
        }
        if (appMode == AppMode.HOST && ScreenShareService.isStreaming) {
            updateVisibleScreen()
        }
    }

    override fun surfaceCreated(holder: SurfaceHolder) {
        decoder.setSurface(holder.surface)
    }

    override fun surfaceChanged(holder: SurfaceHolder, format: Int, width: Int, height: Int) {
        decoder.setSurface(holder.surface)
    }

    override fun surfaceDestroyed(holder: SurfaceHolder) {
        decoder.setSurface(null)
    }

    private fun acquireMulticastLock() {
        try {
            val wifi = applicationContext.getSystemService(Context.WIFI_SERVICE) as? WifiManager
            val lock = wifi?.createMulticastLock("lowlat_multicast_lock")
            lock?.setReferenceCounted(true)
            lock?.acquire()
            multicastLock = lock
        } catch (_: Exception) {
        }
    }

    private fun releaseMulticastLock() {
        try {
            if (multicastLock?.isHeld == true) {
                multicastLock?.release()
            }
        } catch (_: Exception) {
        }
        multicastLock = null
    }

    private fun applyImmersiveMode() {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
            window.insetsController?.let { insetsController ->
                insetsController.hide(WindowInsets.Type.statusBars() or WindowInsets.Type.navigationBars())
                insetsController.systemBarsBehavior =
                    WindowInsetsController.BEHAVIOR_SHOW_TRANSIENT_BARS_BY_SWIPE
            }
        } else {
            @Suppress("DEPRECATION")
            window.decorView.systemUiVisibility = (
                View.SYSTEM_UI_FLAG_IMMERSIVE_STICKY
                    or View.SYSTEM_UI_FLAG_FULLSCREEN
                    or View.SYSTEM_UI_FLAG_HIDE_NAVIGATION
                    or View.SYSTEM_UI_FLAG_LAYOUT_FULLSCREEN
                    or View.SYSTEM_UI_FLAG_LAYOUT_HIDE_NAVIGATION
                    or View.SYSTEM_UI_FLAG_LAYOUT_STABLE
            )
        }
    }

    private fun dp(value: Int): Int =
        (value * resources.displayMetrics.density + 0.5f).toInt()
}
