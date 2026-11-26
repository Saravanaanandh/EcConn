package com.lowlat.receiver

import android.content.Context
import android.graphics.*
import android.graphics.drawable.GradientDrawable
import android.util.AttributeSet
import android.view.View

/**
 * EcConn Obsidian Dark Neumorphic palette and drawable helpers,
 * matching the dark obsidian + electric violet + ice cyan aesthetic.
 */
object EcConnTheme {
    val BG_OBSIDIAN = Color.parseColor("#111219")
    val CARD_SURFACE = Color.parseColor("#1B1D28")
    val CARD_BORDER = Color.parseColor("#2B2E3F")
    val INSET_WELL = Color.parseColor("#13141C")
    val INSET_BORDER = Color.parseColor("#232635")

    val VIOLET = Color.parseColor("#8B5CF6")
    val VIOLET_LIGHT = Color.parseColor("#A78BFA")
    val VIOLET_DIM = Color.parseColor("#3B2D66")
    val CYAN = Color.parseColor("#38BDF8")
    val MINT = Color.parseColor("#34D399")
    val AMBER = Color.parseColor("#FBBF24")
    val DANGER_BG = Color.parseColor("#3B1922")
    val DANGER_FG = Color.parseColor("#F87171")

    val TEXT_PRIMARY = Color.parseColor("#F3F4F6")
    val TEXT_SECONDARY = Color.parseColor("#8E95A8")
    val TEXT_MUTED = Color.parseColor("#5C6378")

    fun cardDrawable(cornerDp: Float, density: Float, strokeColor: Int = CARD_BORDER, bgColor: Int = CARD_SURFACE): GradientDrawable =
        GradientDrawable().apply {
            setColor(bgColor)
            cornerRadius = cornerDp * density
            setStroke((1f * density).toInt().coerceAtLeast(1), strokeColor)
        }

    fun insetDrawable(cornerDp: Float, density: Float, bgColor: Int = INSET_WELL): GradientDrawable =
        GradientDrawable().apply {
            setColor(bgColor)
            cornerRadius = cornerDp * density
            setStroke((1f * density).toInt().coerceAtLeast(1), INSET_BORDER)
        }

    fun pillButtonDrawable(color: Int, cornerDp: Float, density: Float): GradientDrawable =
        GradientDrawable().apply {
            setColor(color)
            cornerRadius = cornerDp * density
        }
}

/**
 * Custom circular Obsidian dial widget displaying EcConn receiver state,
 * glowing violet-to-cyan arc, and central status readout.
 */
class StatusRingView @JvmOverloads constructor(
    context: Context,
    attrs: AttributeSet? = null,
) : View(context, attrs) {

    var centerTitle: String = "READY"
        set(value) { field = value; invalidate() }

    var centerSubtitle: String = "Listening on LAN"
        set(value) { field = value; invalidate() }

    var sweepFraction: Float = 0.68f
        set(value) { field = value; invalidate() }

    var accentColor: Int = EcConnTheme.VIOLET_LIGHT
        set(value) { field = value; invalidate() }

    private val trackPaint = Paint(Paint.ANTI_ALIAS_FLAG).apply {
        style = Paint.Style.STROKE
        color = Color.parseColor("#232638")
    }

    private val wellPaint = Paint(Paint.ANTI_ALIAS_FLAG).apply {
        style = Paint.Style.FILL
        color = EcConnTheme.INSET_WELL
    }

    private val innerRimPaint = Paint(Paint.ANTI_ALIAS_FLAG).apply {
        style = Paint.Style.STROKE
        color = Color.parseColor("#2D3145")
    }

    private val arcPaint = Paint(Paint.ANTI_ALIAS_FLAG).apply {
        style = Paint.Style.STROKE
        strokeCap = Paint.Cap.ROUND
    }

    private val glowPaint = Paint(Paint.ANTI_ALIAS_FLAG).apply {
        style = Paint.Style.STROKE
        strokeCap = Paint.Cap.ROUND
    }

    private val titlePaint = Paint(Paint.ANTI_ALIAS_FLAG).apply {
        color = EcConnTheme.TEXT_PRIMARY
        textAlign = Paint.Align.CENTER
        typeface = Typeface.create(Typeface.DEFAULT, Typeface.BOLD)
    }

    private val subPaint = Paint(Paint.ANTI_ALIAS_FLAG).apply {
        color = EcConnTheme.TEXT_SECONDARY
        textAlign = Paint.Align.CENTER
    }

    private val dotPaint = Paint(Paint.ANTI_ALIAS_FLAG).apply {
        style = Paint.Style.FILL
    }

    private val arcRect = RectF()

    override fun onDraw(canvas: Canvas) {
        super.onDraw(canvas)
        val w = width.toFloat()
        val h = height.toFloat()
        val size = minOf(w, h)
        if (size <= 0f) return

        val cx = w / 2f
        val cy = h / 2f
        val density = resources.displayMetrics.density
        val strokeW = 7f * density
        val radius = (size / 2f) - strokeW - 10f * density

        // Inner recessed dial well
        canvas.drawCircle(cx, cy, radius - strokeW * 1.4f, wellPaint)
        innerRimPaint.strokeWidth = 1.2f * density
        canvas.drawCircle(cx, cy, radius - strokeW * 1.4f, innerRimPaint)

        // Outer track ring
        trackPaint.strokeWidth = strokeW
        canvas.drawCircle(cx, cy, radius, trackPaint)

        // Glowing sweep arc
        arcRect.set(cx - radius, cy - radius, cx + radius, cy + radius)
        val sweep = (sweepFraction.coerceIn(0.05f, 1f)) * 360f

        glowPaint.strokeWidth = strokeW * 2.2f
        glowPaint.color = Color.argb(65, Color.red(accentColor), Color.green(accentColor), Color.blue(accentColor))
        canvas.drawArc(arcRect, -90f, sweep, false, glowPaint)

        arcPaint.strokeWidth = strokeW
        arcPaint.shader = SweepGradient(
            cx, cy,
            intArrayOf(EcConnTheme.VIOLET, EcConnTheme.CYAN, EcConnTheme.VIOLET_LIGHT, EcConnTheme.VIOLET),
            floatArrayOf(0f, 0.45f, 0.8f, 1f)
        )
        canvas.drawArc(arcRect, -90f, sweep, false, arcPaint)

        // Endpoint knob
        val endAngleRad = Math.toRadians((-90f + sweep).toDouble())
        val kx = cx + radius * Math.cos(endAngleRad).toFloat()
        val ky = cy + radius * Math.sin(endAngleRad).toFloat()
        dotPaint.color = Color.WHITE
        canvas.drawCircle(kx, ky, strokeW * 0.85f, dotPaint)
        dotPaint.color = EcConnTheme.VIOLET
        canvas.drawCircle(kx, ky, strokeW * 0.42f, dotPaint)

        // Center text
        titlePaint.textSize = size * 0.17f
        subPaint.textSize = size * 0.075f
        canvas.drawText(centerTitle, cx, cy + titlePaint.textSize * 0.2f, titlePaint)
        canvas.drawText(centerSubtitle, cx, cy + titlePaint.textSize * 0.72f, subPaint)
    }
}
