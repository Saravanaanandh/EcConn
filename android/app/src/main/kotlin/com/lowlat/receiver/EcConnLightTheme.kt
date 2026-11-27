package com.lowlat.receiver

import android.content.Context
import android.graphics.Canvas
import android.graphics.Color
import android.graphics.Paint
import android.graphics.Path
import android.graphics.RectF
import android.graphics.Typeface
import android.graphics.drawable.GradientDrawable
import android.util.TypedValue
import android.view.Gravity
import android.view.View
import android.widget.Button
import android.widget.ImageView
import android.widget.LinearLayout
import android.widget.TextView

object EcConnColors {
    val BG_CANVAS = Color.parseColor("#F5F9FD")
    val CARD_WHITE = Color.WHITE
    val CARD_BLUE = Color.parseColor("#EBF3FC")
    val CARD_BORDER = Color.parseColor("#D6E7F9")
    val SKY_CIRCLE = Color.parseColor("#CBE4FA")
    val WOOD_DESK = Color.parseColor("#E8CCB2")

    val BRAND_BLUE = Color.parseColor("#1565C0")
    val BRAND_BLUE_LIGHT = Color.parseColor("#2196F3")
    val NAVY = Color.parseColor("#0D253F")
    val SLATE = Color.parseColor("#546E7A")
    val MUTED = Color.parseColor("#90A4AE")
    val DANGER_RED = Color.parseColor("#DC4C4C")
    val DANGER_BG = Color.parseColor("#FFF7F7")
}

fun Context.dp(value: Int): Int =
    TypedValue.applyDimension(
        TypedValue.COMPLEX_UNIT_DIP,
        value.toFloat(),
        resources.displayMetrics
    ).toInt()

fun Context.dpF(value: Float): Float =
    TypedValue.applyDimension(
        TypedValue.COMPLEX_UNIT_DIP,
        value,
        resources.displayMetrics
    )

fun makeRoundedDrawable(
    fillColor: Int,
    strokeColor: Int = Color.TRANSPARENT,
    strokeWidthPx: Int = 0,
    radiusPx: Float
): GradientDrawable = GradientDrawable().apply {
    shape = GradientDrawable.RECTANGLE
    setColor(fillColor)
    if (strokeWidthPx > 0 && strokeColor != Color.TRANSPARENT) {
        setStroke(strokeWidthPx, strokeColor)
    }
    cornerRadius = radiusPx
}

fun makePrimaryPillButton(context: Context, text: String): Button = Button(context).apply {
    this.text = text
    isAllCaps = false
    setTextColor(Color.WHITE)
    textSize = 15f
    typeface = Typeface.create("sans-serif-medium", Typeface.BOLD)
    background = makeRoundedDrawable(
        fillColor = EcConnColors.BRAND_BLUE,
        radiusPx = context.dpF(24f)
    )
    elevation = 0f
    stateListAnimator = null
    setPadding(context.dp(24), context.dp(10), context.dp(24), context.dp(10))
}

fun makeOutlinePillButton(context: Context, text: String): Button = Button(context).apply {
    this.text = text
    isAllCaps = false
    setTextColor(EcConnColors.BRAND_BLUE)
    textSize = 15f
    typeface = Typeface.create("sans-serif-medium", Typeface.BOLD)
    background = makeRoundedDrawable(
        fillColor = EcConnColors.CARD_WHITE,
        strokeColor = EcConnColors.BRAND_BLUE,
        strokeWidthPx = context.dp(2),
        radiusPx = context.dpF(24f)
    )
    elevation = 0f
    stateListAnimator = null
    setPadding(context.dp(24), context.dp(10), context.dp(24), context.dp(10))
}

fun makeDangerPillButton(context: Context, text: String): Button = Button(context).apply {
    this.text = text
    isAllCaps = false
    setTextColor(EcConnColors.DANGER_RED)
    textSize = 15f
    typeface = Typeface.create("sans-serif-medium", Typeface.BOLD)
    background = makeRoundedDrawable(
        fillColor = EcConnColors.DANGER_BG,
        strokeColor = EcConnColors.DANGER_RED,
        strokeWidthPx = context.dp(2),
        radiusPx = context.dpF(24f)
    )
    elevation = 0f
    stateListAnimator = null
    setPadding(context.dp(24), context.dp(10), context.dp(24), context.dp(10))
}

class EcConnWifiIconView(context: Context, private val iconColor: Int = EcConnColors.BRAND_BLUE) :
    View(context) {
    private val paint = Paint(Paint.ANTI_ALIAS_FLAG).apply {
        color = iconColor
        style = Paint.Style.STROKE
        strokeCap = Paint.Cap.ROUND
    }
    private val dotPaint = Paint(Paint.ANTI_ALIAS_FLAG).apply {
        color = iconColor
        style = Paint.Style.FILL
    }

    override fun onDraw(canvas: Canvas) {
        super.onDraw(canvas)
        val w = width.toFloat()
        val h = height.toFloat()
        val size = minOf(w, h)
        val cx = w / 2f
        val cy = h * 0.78f

        canvas.drawCircle(cx, cy, size * 0.085f, dotPaint)
        paint.strokeWidth = size * 0.10f

        val radii = floatArrayOf(size * 0.28f, size * 0.48f, size * 0.68f)
        val sweeps = floatArrayOf(84f, 92f, 100f)
        for (i in radii.indices) {
            val r = radii[i]
            val rect = RectF(cx - r, cy - r, cx + r, cy + r)
            canvas.drawArc(rect, -90f - sweeps[i] / 2f, sweeps[i], false, paint)
        }
    }
}

fun makeEcConnHeader(context: Context, title: String): LinearLayout = LinearLayout(context).apply {
    orientation = LinearLayout.HORIZONTAL
    gravity = Gravity.CENTER_VERTICAL
    addView(ImageView(context).apply {
        setImageResource(R.drawable.ecconn_logo)
        scaleType = ImageView.ScaleType.FIT_CENTER
    }, LinearLayout.LayoutParams(context.dp(46), context.dp(46)))
    addView(TextView(context).apply {
        text = title
        textSize = 28f
        setTextColor(EcConnColors.NAVY)
        typeface = Typeface.create("sans-serif-medium", Typeface.BOLD)
        setPadding(context.dp(10), 0, 0, 0)
    })
}

fun makeEcConnHeroImageView(context: Context): ImageView = ImageView(context).apply {
    setImageResource(R.drawable.ecconn_hero)
    scaleType = ImageView.ScaleType.FIT_CENTER
}

fun makeWifiBanner(context: Context): LinearLayout = LinearLayout(context).apply {
    orientation = LinearLayout.HORIZONTAL
    gravity = Gravity.CENTER_VERTICAL
    background = makeRoundedDrawable(
        fillColor = EcConnColors.CARD_BLUE,
        strokeColor = EcConnColors.CARD_BORDER,
        strokeWidthPx = context.dp(1),
        radiusPx = context.dpF(14f)
    )
    setPadding(context.dp(14), context.dp(12), context.dp(14), context.dp(12))

    addView(
        EcConnWifiIconView(context, EcConnColors.BRAND_BLUE),
        LinearLayout.LayoutParams(context.dp(28), context.dp(24)).apply {
            rightMargin = context.dp(12)
        }
    )
    addView(LinearLayout(context).apply {
        orientation = LinearLayout.VERTICAL
        addView(TextView(context).apply {
            text = "Check your Wi-Fi connection"
            textSize = 13.5f
            setTextColor(EcConnColors.NAVY)
            typeface = Typeface.create("sans-serif-medium", Typeface.BOLD)
        })
        addView(TextView(context).apply {
            text = "Both devices must use the same Wi-Fi."
            textSize = 12f
            setTextColor(EcConnColors.SLATE)
        })
    })
}

class EcConnHeroView(context: Context, var showWifiBadge: Boolean = true) : View(context) {
    private val skyPaint = Paint(Paint.ANTI_ALIAS_FLAG).apply {
        color = EcConnColors.SKY_CIRCLE
        style = Paint.Style.FILL
    }
    private val deskPaint = Paint(Paint.ANTI_ALIAS_FLAG).apply {
        color = EcConnColors.WOOD_DESK
        style = Paint.Style.FILL
    }
    private val navyFill = Paint(Paint.ANTI_ALIAS_FLAG).apply {
        color = EcConnColors.NAVY
        style = Paint.Style.FILL
    }
    private val whiteFill = Paint(Paint.ANTI_ALIAS_FLAG).apply {
        color = Color.WHITE
        style = Paint.Style.FILL
    }
    private val blueScreenFill = Paint(Paint.ANTI_ALIAS_FLAG).apply {
        color = Color.parseColor("#E3F0FC")
        style = Paint.Style.FILL
    }
    private val blueAccent = Paint(Paint.ANTI_ALIAS_FLAG).apply {
        color = EcConnColors.BRAND_BLUE_LIGHT
        style = Paint.Style.FILL
    }
    private val slateLine = Paint(Paint.ANTI_ALIAS_FLAG).apply {
        color = EcConnColors.SLATE
        style = Paint.Style.STROKE
        strokeCap = Paint.Cap.ROUND
    }
    private val wifiArcPaint = Paint(Paint.ANTI_ALIAS_FLAG).apply {
        color = EcConnColors.BRAND_BLUE
        style = Paint.Style.STROKE
        strokeCap = Paint.Cap.ROUND
    }
    private val wifiDotPaint = Paint(Paint.ANTI_ALIAS_FLAG).apply {
        color = EcConnColors.BRAND_BLUE
        style = Paint.Style.FILL
    }

    override fun onDraw(canvas: Canvas) {
        super.onDraw(canvas)
        val w = width.toFloat()
        val h = height.toFloat()
        val cx = w / 2f
        val cy = h * 0.56f
        val r = minOf(w, h) * 0.44f

        // Soft sky-blue backdrop circle
        canvas.drawCircle(cx, cy, r, skyPaint)

        // Desk shelf
        val deskY = cy + r * 0.52f
        val deskRect = RectF(cx - r * 0.96f, deskY, cx + r * 0.96f, deskY + r * 0.14f)
        canvas.drawRoundRect(deskRect, r * 0.07f, r * 0.07f, deskPaint)

        // Laptop on left
        val lapL = cx - r * 0.72f
        val lapR = cx + r * 0.16f
        val lapT = deskY - r * 0.62f
        val lapRect = RectF(lapL, lapT, lapR, deskY - r * 0.03f)
        canvas.drawRoundRect(lapRect, r * 0.06f, r * 0.06f, navyFill)

        val lapScreen = RectF(lapL + r * 0.05f, lapT + r * 0.05f, lapR - r * 0.05f, deskY - r * 0.08f)
        canvas.drawRoundRect(lapScreen, r * 0.03f, r * 0.03f, whiteFill)
        canvas.drawRoundRect(
            RectF(lapScreen.left + r * 0.03f, lapScreen.top + r * 0.03f, lapScreen.right - r * 0.03f, lapScreen.bottom - r * 0.03f),
            r * 0.02f,
            r * 0.02f,
            blueScreenFill
        )
        // Laptop base
        canvas.drawRoundRect(
            RectF(lapL - r * 0.08f, deskY - r * 0.04f, lapR + r * 0.08f, deskY + r * 0.01f),
            r * 0.02f,
            r * 0.02f,
            navyFill
        )

        // Phone / Tablet on right
        val phL = cx + r * 0.24f
        val phR = cx + r * 0.74f
        val phT = deskY - r * 0.50f
        canvas.drawRoundRect(RectF(phL, phT, phR, deskY + r * 0.01f), r * 0.08f, r * 0.08f, navyFill)
        val phScreen = RectF(phL + r * 0.05f, phT + r * 0.06f, phR - r * 0.05f, deskY - r * 0.06f)
        canvas.drawRoundRect(phScreen, r * 0.04f, r * 0.04f, whiteFill)

        // Small UI tile inside phone screen
        canvas.drawRoundRect(
            RectF(phScreen.left + r * 0.04f, phScreen.top + r * 0.06f, phScreen.right - r * 0.04f, phScreen.top + r * 0.19f),
            r * 0.03f,
            r * 0.03f,
            blueAccent
        )

        // Floating Wi-Fi icon above devices
        if (showWifiBadge) {
            val wx = cx + r * 0.14f
            val wy = cy - r * 0.42f
            val ws = r * 0.22f
            canvas.drawCircle(wx, wy + ws * 0.3f, ws * 0.10f, wifiDotPaint)
            wifiArcPaint.strokeWidth = ws * 0.12f
            for (pair in arrayOf(0.35f to 84f, 0.58f to 92f, 0.82f to 100f)) {
                val ar = ws * pair.first
                val rect = RectF(wx - ar, wy + ws * 0.3f - ar, wx + ar, wy + ws * 0.3f + ar)
                canvas.drawArc(rect, -90f - pair.second / 2f, pair.second, false, wifiArcPaint)
            }
        }
    }
}
