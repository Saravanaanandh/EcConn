package com.lowlat.receiver

import android.app.Activity
import android.content.Context
import android.content.res.Configuration
import android.graphics.Color
import android.graphics.Typeface
import android.graphics.drawable.GradientDrawable
import android.net.wifi.WifiManager
import android.os.Build
import android.os.Bundle
import android.text.SpannableString
import android.text.Spanned
import android.text.style.ForegroundColorSpan
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
 * Main Activity entry point for the EcConn Android Receiver.
 *
 * Implements immersive fullscreen, screen-always-on, MediaCodec video rendering
 * onto a SurfaceView, Wi-Fi multicast lock, LAN advertising, touch interaction,
 * and the EcConn Obsidian dashboard UI when idle or pairing.
 */
class MainActivity : Activity(), SurfaceHolder.Callback {

    companion object {
        private const val TAG = "MainActivity"
    }

    private lateinit var decoder: H264Decoder
    private lateinit var advertiser: Advertiser
    private lateinit var controller: ReceiverController

    private var multicastLock: WifiManager.MulticastLock? = null

    // UI elements
    private lateinit var rootLayout: FrameLayout
    private lateinit var surfaceView: SurfaceView
    private lateinit var dashboardOverlay: ScrollView
    private lateinit var statusRing: StatusRingView
    private lateinit var headerStatusDot: View
    private lateinit var cardDisplaySub: TextView
    private lateinit var cardNetworkSub: TextView
    private lateinit var statusText: TextView
    private lateinit var pairingCard: LinearLayout
    private lateinit var pairingTitle: TextView
    private lateinit var pairingSubtitle: TextView
    private lateinit var pairingCodeText: TextView
    private lateinit var btnAllow: Button
    private lateinit var btnDeny: Button
    private lateinit var btnDisconnect: Button

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)

        // Keep screen awake
        window.addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)

        // Initialize core components
        decoder = H264Decoder()
        advertiser = Advertiser(this)
        controller = ReceiverController(this, decoder)
        decoder.onRequestKeyframe = { controller.requestKeyframe() }

        buildUi()
        setupListeners()
    }

    private fun buildUi() {
        val d = resources.displayMetrics.density
        window.decorView.setBackgroundColor(EcConnTheme.BG_OBSIDIAN)

        rootLayout = FrameLayout(this).apply {
            setBackgroundColor(EcConnTheme.BG_OBSIDIAN)
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

        // 2. Obsidian Idle / Ready Dashboard Overlay
        dashboardOverlay = ScrollView(this).apply {
            setBackgroundColor(EcConnTheme.BG_OBSIDIAN)
            isFillViewport = true
            layoutParams = FrameLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT,
                ViewGroup.LayoutParams.MATCH_PARENT
            )
        }

        val dashContent = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            setPadding(dp(20), dp(24), dp(20), dp(24))
            layoutParams = FrameLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT,
                ViewGroup.LayoutParams.WRAP_CONTENT
            )
        }

        // Top Header Row
        val headerRow = LinearLayout(this).apply {
            orientation = LinearLayout.HORIZONTAL
            gravity = Gravity.CENTER_VERTICAL
            setPadding(0, 0, 0, dp(18))
        }

        val headerTextCol = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            layoutParams = LinearLayout.LayoutParams(0, ViewGroup.LayoutParams.WRAP_CONTENT, 1f)
        }

        val eyebrow = TextView(this).apply {
            val raw = "—  ECCONN"
            val span = SpannableString(raw)
            span.setSpan(ForegroundColorSpan(EcConnTheme.VIOLET_LIGHT), 0, 1, Spanned.SPAN_EXCLUSIVE_EXCLUSIVE)
            text = span
            setTextColor(EcConnTheme.TEXT_SECONDARY)
            setTextSize(TypedValue.COMPLEX_UNIT_SP, 11f)
            letterSpacing = 0.16f
            typeface = Typeface.DEFAULT_BOLD
        }
        headerTextCol.addView(eyebrow)

        val titleView = TextView(this).apply {
            val raw = "EcConn Display"
            val span = SpannableString(raw)
            span.setSpan(ForegroundColorSpan(EcConnTheme.VIOLET_LIGHT), 7, raw.length, Spanned.SPAN_EXCLUSIVE_EXCLUSIVE)
            text = span
            setTextColor(EcConnTheme.TEXT_PRIMARY)
            setTextSize(TypedValue.COMPLEX_UNIT_SP, 26f)
            typeface = Typeface.create(Typeface.DEFAULT, Typeface.BOLD)
            setPadding(0, dp(2), 0, dp(2))
        }
        headerTextCol.addView(titleView)

        val deviceLabel = Build.MODEL?.takeIf { it.isNotBlank() } ?: "Android Receiver"
        val subtitleView = TextView(this).apply {
            text = "Wireless extended monitor · $deviceLabel"
            setTextColor(EcConnTheme.TEXT_SECONDARY)
            setTextSize(TypedValue.COMPLEX_UNIT_SP, 13f)
        }
        headerTextCol.addView(subtitleView)
        headerRow.addView(headerTextCol)

        // Top-right circular status indicator
        val statusCircle = FrameLayout(this).apply {
            background = EcConnTheme.cardDrawable(24f, d)
            layoutParams = LinearLayout.LayoutParams(dp(46), dp(46))
        }
        headerStatusDot = View(this).apply {
            background = EcConnTheme.pillButtonDrawable(EcConnTheme.CYAN, 6f, d)
            layoutParams = FrameLayout.LayoutParams(dp(12), dp(12)).apply {
                gravity = Gravity.CENTER
            }
        }
        statusCircle.addView(headerStatusDot)
        headerRow.addView(statusCircle)
        dashContent.addView(headerRow)

        // Hero Status Dial Card
        val heroCard = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            gravity = Gravity.CENTER_HORIZONTAL
            background = EcConnTheme.cardDrawable(24f, d)
            setPadding(dp(20), dp(20), dp(20), dp(18))
            layoutParams = LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT,
                ViewGroup.LayoutParams.WRAP_CONTENT
            ).apply {
                setMargins(0, 0, 0, dp(14))
            }
        }

        statusRing = StatusRingView(this).apply {
            centerTitle = "READY"
            centerSubtitle = "Listening on LAN"
            sweepFraction = 0.72f
            accentColor = EcConnTheme.VIOLET_LIGHT
            layoutParams = LinearLayout.LayoutParams(dp(190), dp(190))
        }
        heroCard.addView(statusRing)

        val heroFooter = TextView(this).apply {
            text = "●  Open EcConn Host on your PC and click Pair & Connect"
            setTextColor(EcConnTheme.CYAN)
            setTextSize(TypedValue.COMPLEX_UNIT_SP, 12f)
            gravity = Gravity.CENTER
            setPadding(0, dp(8), 0, 0)
        }
        heroCard.addView(heroFooter)
        dashContent.addView(heroCard)

        // 2-Column Info Grid (Row 1)
        val (screenW, screenH) = DisplayInfo.getScreenDimensions(this)
        val gridRow1 = LinearLayout(this).apply {
            orientation = LinearLayout.HORIZONTAL
            layoutParams = LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT,
                ViewGroup.LayoutParams.WRAP_CONTENT
            ).apply {
                setMargins(0, 0, 0, dp(12))
            }
        }

        val displayCard = buildInfoCard(
            d = d,
            badgeText = "HD",
            badgeColor = EcConnTheme.CYAN,
            pillText = "H.264",
            pillColor = EcConnTheme.VIOLET,
            title = "Display Link",
            subtitle = "${screenW}×${screenH} · Hardware"
        ).also {
            cardDisplaySub = it.second
            it.first.layoutParams = LinearLayout.LayoutParams(0, ViewGroup.LayoutParams.WRAP_CONTENT, 1f).apply {
                setMargins(0, 0, dp(6), 0)
            }
        }
        gridRow1.addView(displayCard.first)

        val touchCard = buildInfoCard(
            d = d,
            badgeText = "✋",
            badgeColor = EcConnTheme.VIOLET_LIGHT,
            pillText = "ON",
            pillColor = EcConnTheme.VIOLET,
            title = "Touch Input",
            subtitle = "Low-latency UDP"
        ).also {
            it.first.layoutParams = LinearLayout.LayoutParams(0, ViewGroup.LayoutParams.WRAP_CONTENT, 1f).apply {
                setMargins(dp(6), 0, 0, 0)
            }
        }
        gridRow1.addView(touchCard.first)
        dashContent.addView(gridRow1)

        // 2-Column Info Grid (Row 2)
        val gridRow2 = LinearLayout(this).apply {
            orientation = LinearLayout.HORIZONTAL
            layoutParams = LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT,
                ViewGroup.LayoutParams.WRAP_CONTENT
            ).apply {
                setMargins(0, 0, 0, dp(14))
            }
        }

        val netCard = buildInfoCard(
            d = d,
            badgeText = "LAN",
            badgeColor = EcConnTheme.MINT,
            pillText = "mDNS",
            pillColor = EcConnTheme.INSET_BORDER,
            title = "Network",
            subtitle = "Video :${Wire.VIDEO_PORT} · Pair :${Wire.PAIR_PORT}"
        ).also {
            cardNetworkSub = it.second
            it.first.layoutParams = LinearLayout.LayoutParams(0, ViewGroup.LayoutParams.WRAP_CONTENT, 1f).apply {
                setMargins(0, 0, dp(6), 0)
            }
        }
        gridRow2.addView(netCard.first)

        val modeCard = buildInfoCard(
            d = d,
            badgeText = "⟳",
            badgeColor = EcConnTheme.CYAN,
            pillText = "AUTO",
            pillColor = EcConnTheme.VIOLET,
            title = "Screen Mode",
            subtitle = "Extend · Auto-Rotate"
        ).also {
            it.first.layoutParams = LinearLayout.LayoutParams(0, ViewGroup.LayoutParams.WRAP_CONTENT, 1f).apply {
                setMargins(dp(6), 0, 0, 0)
            }
        }
        gridRow2.addView(modeCard.first)
        dashContent.addView(gridRow2)

        // Bottom Floating Dock Pill
        val bottomDock = LinearLayout(this).apply {
            orientation = LinearLayout.HORIZONTAL
            gravity = Gravity.CENTER
            background = EcConnTheme.cardDrawable(28f, d)
            setPadding(dp(16), dp(10), dp(16), dp(10))
            layoutParams = LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT,
                ViewGroup.LayoutParams.WRAP_CONTENT
            )
        }
        val dockItems = listOf(
            "Receiver" to true,
            "Display" to false,
            "Network" to false,
            "Ready" to false
        )
        for ((label, active) in dockItems) {
            val item = TextView(this).apply {
                text = label
                gravity = Gravity.CENTER
                setTextSize(TypedValue.COMPLEX_UNIT_SP, 12f)
                setPadding(dp(12), dp(6), dp(12), dp(6))
                if (active) {
                    setTextColor(EcConnTheme.TEXT_PRIMARY)
                    typeface = Typeface.DEFAULT_BOLD
                    background = EcConnTheme.pillButtonDrawable(EcConnTheme.VIOLET_DIM, 16f, d)
                } else {
                    setTextColor(EcConnTheme.TEXT_SECONDARY)
                }
                layoutParams = LinearLayout.LayoutParams(0, ViewGroup.LayoutParams.WRAP_CONTENT, 1f)
            }
            bottomDock.addView(item)
        }
        dashContent.addView(bottomDock)

        dashboardOverlay.addView(dashContent)
        rootLayout.addView(dashboardOverlay)

        // 3. Floating Streaming Status Badge (top-left, visible when streaming)
        statusText = TextView(this).apply {
            text = "EcConn · Ready"
            setTextColor(EcConnTheme.TEXT_PRIMARY)
            setTextSize(TypedValue.COMPLEX_UNIT_SP, 12f)
            typeface = Typeface.DEFAULT_BOLD
            setPadding(dp(14), dp(7), dp(14), dp(7))
            background = EcConnTheme.cardDrawable(18f, d, EcConnTheme.VIOLET_DIM, Color.argb(210, 27, 29, 40))
            visibility = View.GONE
            layoutParams = FrameLayout.LayoutParams(
                ViewGroup.LayoutParams.WRAP_CONTENT,
                ViewGroup.LayoutParams.WRAP_CONTENT
            ).apply {
                gravity = Gravity.TOP or Gravity.START
                setMargins(dp(16), dp(16), 0, 0)
            }
        }
        rootLayout.addView(statusText)

        // 4. Floating Disconnect Button (top-right, visible when streaming)
        btnDisconnect = Button(this).apply {
            text = "Disconnect"
            isAllCaps = false
            setTextColor(EcConnTheme.DANGER_FG)
            setTextSize(TypedValue.COMPLEX_UNIT_SP, 12f)
            typeface = Typeface.DEFAULT_BOLD
            setPadding(dp(14), dp(4), dp(14), dp(4))
            background = EcConnTheme.cardDrawable(16f, d, Color.parseColor("#6B2230"), EcConnTheme.DANGER_BG)
            visibility = View.GONE
            layoutParams = FrameLayout.LayoutParams(
                ViewGroup.LayoutParams.WRAP_CONTENT,
                ViewGroup.LayoutParams.WRAP_CONTENT
            ).apply {
                gravity = Gravity.TOP or Gravity.END
                setMargins(0, dp(16), dp(16), 0)
            }
            setOnClickListener {
                controller.disconnect()
            }
        }
        rootLayout.addView(btnDisconnect)

        // 5. Obsidian Pairing Challenge Card (centered, initially hidden)
        pairingCard = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            gravity = Gravity.CENTER_HORIZONTAL
            setPadding(dp(24), dp(24), dp(24), dp(24))
            background = EcConnTheme.cardDrawable(24f, d, EcConnTheme.VIOLET, EcConnTheme.CARD_SURFACE)
            visibility = View.GONE
            layoutParams = FrameLayout.LayoutParams(
                dp(340),
                ViewGroup.LayoutParams.WRAP_CONTENT
            ).apply {
                gravity = Gravity.CENTER
            }
        }

        pairingTitle = TextView(this).apply {
            text = "EcConn Pairing Request"
            setTextColor(EcConnTheme.TEXT_PRIMARY)
            setTextSize(TypedValue.COMPLEX_UNIT_SP, 19f)
            typeface = Typeface.DEFAULT_BOLD
            gravity = Gravity.CENTER
        }
        pairingCard.addView(pairingTitle)

        pairingSubtitle = TextView(this).apply {
            text = "A host PC wants to connect"
            setTextColor(EcConnTheme.TEXT_SECONDARY)
            setTextSize(TypedValue.COMPLEX_UNIT_SP, 13f)
            gravity = Gravity.CENTER
            setPadding(0, dp(6), 0, dp(16))
        }
        pairingCard.addView(pairingSubtitle)

        pairingCodeText = TextView(this).apply {
            text = "000 000"
            setTextColor(EcConnTheme.VIOLET_LIGHT)
            setTextSize(TypedValue.COMPLEX_UNIT_SP, 34f)
            typeface = Typeface.MONOSPACE
            gravity = Gravity.CENTER
            setPadding(dp(20), dp(12), dp(20), dp(12))
            background = EcConnTheme.insetDrawable(14f, d)
        }
        pairingCard.addView(pairingCodeText)

        val buttonsRow = LinearLayout(this).apply {
            orientation = LinearLayout.HORIZONTAL
            gravity = Gravity.CENTER
            setPadding(0, dp(20), 0, 0)
            layoutParams = LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT,
                ViewGroup.LayoutParams.WRAP_CONTENT
            )
        }

        btnDeny = Button(this).apply {
            text = "Deny"
            isAllCaps = false
            setTextColor(EcConnTheme.TEXT_SECONDARY)
            typeface = Typeface.DEFAULT_BOLD
            background = EcConnTheme.insetDrawable(14f, d)
            layoutParams = LinearLayout.LayoutParams(0, dp(46), 1f).apply {
                setMargins(0, 0, dp(8), 0)
            }
            setOnClickListener {
                controller.denyPairing()
                pairingCard.visibility = View.GONE
            }
        }
        buttonsRow.addView(btnDeny)

        btnAllow = Button(this).apply {
            text = "Allow"
            isAllCaps = false
            setTextColor(Color.WHITE)
            typeface = Typeface.DEFAULT_BOLD
            background = EcConnTheme.pillButtonDrawable(EcConnTheme.VIOLET, 14f, d)
            layoutParams = LinearLayout.LayoutParams(0, dp(46), 1f).apply {
                setMargins(dp(8), 0, 0, 0)
            }
            setOnClickListener {
                controller.allowPairing()
                pairingCard.visibility = View.GONE
            }
        }
        buttonsRow.addView(btnAllow)

        pairingCard.addView(buttonsRow)
        rootLayout.addView(pairingCard)

        setContentView(rootLayout)
    }

    private fun buildInfoCard(
        d: Float,
        badgeText: String,
        badgeColor: Int,
        pillText: String,
        pillColor: Int,
        title: String,
        subtitle: String
    ): Pair<LinearLayout, TextView> {
        val card = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            background = EcConnTheme.cardDrawable(20f, d)
            setPadding(dp(14), dp(14), dp(14), dp(14))
        }

        val topRow = LinearLayout(this).apply {
            orientation = LinearLayout.HORIZONTAL
            gravity = Gravity.CENTER_VERTICAL
            setPadding(0, 0, 0, dp(10))
        }

        val iconCircle = TextView(this).apply {
            text = badgeText
            gravity = Gravity.CENTER
            setTextColor(badgeColor)
            setTextSize(TypedValue.COMPLEX_UNIT_SP, 12f)
            typeface = Typeface.DEFAULT_BOLD
            background = EcConnTheme.insetDrawable(18f, d)
            layoutParams = LinearLayout.LayoutParams(dp(36), dp(36))
        }
        topRow.addView(iconCircle)

        val spacer = View(this).apply {
            layoutParams = LinearLayout.LayoutParams(0, 1, 1f)
        }
        topRow.addView(spacer)

        val pill = TextView(this).apply {
            text = pillText
            setTextColor(EcConnTheme.TEXT_PRIMARY)
            setTextSize(TypedValue.COMPLEX_UNIT_SP, 10f)
            typeface = Typeface.DEFAULT_BOLD
            setPadding(dp(9), dp(3), dp(9), dp(3))
            background = EcConnTheme.pillButtonDrawable(pillColor, 10f, d)
        }
        topRow.addView(pill)
        card.addView(topRow)

        val titleView = TextView(this).apply {
            text = title
            setTextColor(EcConnTheme.TEXT_PRIMARY)
            setTextSize(TypedValue.COMPLEX_UNIT_SP, 14f)
            typeface = Typeface.DEFAULT_BOLD
        }
        card.addView(titleView)

        val subView = TextView(this).apply {
            text = subtitle
            setTextColor(EcConnTheme.TEXT_SECONDARY)
            setTextSize(TypedValue.COMPLEX_UNIT_SP, 11f)
            setPadding(0, dp(2), 0, 0)
        }
        card.addView(subView)

        return card to subView
    }

    private fun setupListeners() {
        val d = resources.displayMetrics.density

        // Controller callbacks
        controller.onStateChanged = { state, detail ->
            runOnUiThread {
                val detailStr = detail ?: "Listening on LAN"
                when (state) {
                    ReceiverController.State.IDLE -> {
                        dashboardOverlay.visibility = View.VISIBLE
                        statusRing.centerTitle = "READY"
                        statusRing.centerSubtitle = detailStr
                        statusRing.sweepFraction = 0.72f
                        statusRing.accentColor = EcConnTheme.VIOLET_LIGHT
                        headerStatusDot.background = EcConnTheme.pillButtonDrawable(EcConnTheme.CYAN, 6f, d)
                        statusText.visibility = View.GONE
                        btnDisconnect.visibility = View.GONE
                        pairingCard.visibility = View.GONE
                    }
                    ReceiverController.State.PAIRING_CHALLENGE -> {
                        dashboardOverlay.visibility = View.VISIBLE
                        statusRing.centerTitle = "PAIRING"
                        statusRing.centerSubtitle = detailStr
                        statusRing.sweepFraction = 0.90f
                        statusRing.accentColor = EcConnTheme.AMBER
                        headerStatusDot.background = EcConnTheme.pillButtonDrawable(EcConnTheme.AMBER, 6f, d)
                        statusText.visibility = View.GONE
                    }
                    ReceiverController.State.CONNECTED -> {
                        dashboardOverlay.visibility = View.GONE
                        statusText.text = "EcConn · ${controller.currentHostName}"
                        statusText.visibility = View.VISIBLE
                        statusText.postDelayed({
                            if (controller.state == ReceiverController.State.CONNECTED) {
                                statusText.visibility = View.GONE
                            }
                        }, 3000L)
                        btnDisconnect.visibility = View.VISIBLE
                        pairingCard.visibility = View.GONE
                    }
                    ReceiverController.State.DISCONNECTED -> {
                        dashboardOverlay.visibility = View.VISIBLE
                        statusRing.centerTitle = "OFFLINE"
                        statusRing.centerSubtitle = detailStr
                        statusRing.sweepFraction = 0.35f
                        statusRing.accentColor = EcConnTheme.DANGER_FG
                        headerStatusDot.background = EcConnTheme.pillButtonDrawable(EcConnTheme.DANGER_FG, 6f, d)
                        statusText.visibility = View.GONE
                        btnDisconnect.visibility = View.GONE
                        pairingCard.visibility = View.GONE
                    }
                }
            }
        }

        controller.onPairingChallenge = { code, hostName ->
            runOnUiThread {
                val formatted = if (code.length == 6) {
                    "${code.substring(0, 3)} ${code.substring(3)}"
                } else code
                pairingCodeText.text = formatted
                pairingSubtitle.text = "Host '$hostName' wants to connect"
                pairingCard.visibility = View.VISIBLE
            }
        }

        controller.onVideoSizeChanged = { w, h ->
            Log.i(TAG, "Video stream resolution changed to ${w}x${h}")
            runOnUiThread {
                cardDisplaySub.text = "${w}×${h} · Hardware"
            }
        }

        // Touch handling on SurfaceView
        surfaceView.setOnTouchListener { v, event ->
            handleTouchEvent(v, event)
            true
        }

        // Tap root background to show status / disconnect button briefly when connected
        rootLayout.setOnClickListener {
            if (controller.state == ReceiverController.State.CONNECTED) {
                statusText.visibility = View.VISIBLE
                btnDisconnect.visibility = View.VISIBLE
                statusText.postDelayed({
                    if (controller.state == ReceiverController.State.CONNECTED) {
                        statusText.visibility = View.GONE
                        btnDisconnect.visibility = View.GONE
                    }
                }, 3500L)
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
            MotionEvent.ACTION_DOWN -> {
                controller.sendTouchDown(normX, normY)
            }
            MotionEvent.ACTION_MOVE -> {
                controller.sendTouchMove(normX, normY)
            }
            MotionEvent.ACTION_UP, MotionEvent.ACTION_CANCEL -> {
                controller.sendTouchUp(normX, normY)
            }
        }
    }

    override fun onStart() {
        super.onStart()
        acquireMulticastLock()

        val (w, h) = DisplayInfo.getScreenDimensions(this)
        val orient = DisplayInfo.getOrientationDegrees(this)

        advertiser.start(w, h, orient)
        controller.start()
    }

    override fun onResume() {
        super.onResume()
        applyImmersiveMode()
    }

    override fun onStop() {
        super.onStop()
        controller.stop()
        advertiser.stop()
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
        cardDisplaySub.text = "${w}×${h} · Hardware"

        if (controller.state == ReceiverController.State.CONNECTED) {
            controller.sendOrientation(orient)
        }
    }

    // SurfaceHolder.Callback
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
        } catch (e: Exception) {
            // Ignore if multicast lock cannot be acquired
        }
    }

    private fun releaseMulticastLock() {
        try {
            if (multicastLock?.isHeld == true) {
                multicastLock?.release()
            }
        } catch (e: Exception) {
            // Ignore
        }
        multicastLock = null
    }

    private fun applyImmersiveMode() {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
            window.insetsController?.let { controller ->
                controller.hide(WindowInsets.Type.statusBars() or WindowInsets.Type.navigationBars())
                controller.systemBarsBehavior = WindowInsetsController.BEHAVIOR_SHOW_TRANSIENT_BARS_BY_SWIPE
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
