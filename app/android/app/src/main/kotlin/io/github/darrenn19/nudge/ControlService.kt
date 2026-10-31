package io.github.darrenn19.nudge

import android.accessibilityservice.AccessibilityService
import android.accessibilityservice.GestureDescription
import android.content.ComponentName
import android.content.Context
import android.content.Intent
import android.graphics.Path
import android.graphics.PointF
import android.os.Build
import android.os.Bundle
import android.os.Handler
import android.os.HandlerThread
import android.os.SystemClock
import android.provider.Settings
import android.util.Log
import android.accessibilityservice.InputMethod
import android.view.KeyEvent
import android.view.WindowManager
import android.view.accessibility.AccessibilityEvent
import android.view.accessibility.AccessibilityNodeInfo
import java.text.BreakIterator

/**
 * Remote control during screen mirroring: the PC's taps, drags, scrolling,
 * keys and typing, carried out on this phone.
 *
 * Android only lets an Accessibility Service do this: gestures through
 * [dispatchGesture], system buttons through [performGlobalAction], typing by
 * editing the focused text field ([AccessibilityNodeInfo.ACTION_SET_TEXT]).
 * The user turns it on in Settings → Accessibility (asked the first time the
 * PC tries to control the phone: see [RemoteControl]). It does nothing by
 * itself: it only acts on input from a mirroring session this phone's user
 * agreed to, which the core ends when the phone locks.
 *
 * Touch: each of the PC's pointers (the mouse; in game mode also the
 * keyboard's joystick) becomes a continuing stroke, all in one gesture
 * (each new stretch is dispatched once the previous one has played out,
 * timed like the mouse moved), so drags, long presses and two fingers at
 * once work, not just taps. Scrolling
 * becomes a finger dragging the content, lifted after holding still (no
 * fling), so wheel notches move the page by a steady amount.
 *
 * Everything runs on one thread of its own ([handler]).
 */
class ControlService : AccessibilityService() {
    companion object {
        private const val TAG = "Nudge"

        /** Connected and ready, or null (switched off in Settings). */
        @Volatile
        var instance: ControlService? = null
            private set

        /**
         * A moving stroke's stretch takes as long as the movement took to
         * come in from the PC, less how long its newest point has already
         * waited, so the finger moves at the mouse's speed instead of in
         * bursts (a fixed 8 ms played 10-15 ms of movement at once, then
         * stood still). Within these bounds, ms: the longest is how far the
         * finger may trail the mouse. A finger that doesn't move (a tap's
         * lift, a held finger) takes 1 ms.
         */
        private const val MIN_SEGMENT_MS = 4L
        private const val MAX_SEGMENT_MS = 12L

        /** A new touch's first stretch (its moves already waiting), ms. */
        private const val FIRST_SEGMENT_MS = 8L

        /**
         * A finger the PC moved or repeated within this long is one it's
         * working now (it repeats a held one every 200 ms): put back after
         * Android cancels a gesture. Others give way to a touch on the phone.
         */
        private const val ACTIVE_MS = 300L

        /** A touchpad swipe starts at least this share of the screen from its edges (room to move). */
        private const val PAN_START_MARGIN = 0.2f

        /** Without word from the PC that the fingers left the touchpad, a swipe ends after this long without input. */
        private const val PAN_IDLE_MS = 400L

        /** A stretch ending in a flick (unused by the PC now; kept for [Op.Up.flick]). */
        private const val FLICK_MS = 70L

        /** One wheel notch (120 units) moves the content this far. */
        private const val NOTCH_DP = 64f

        /** Scrolling stops (the finger lifts) after this long without wheel input. */
        private const val SCROLL_IDLE_MS = 150L

        /** Key modifier bits (core/src/input.rs). */
        private const val SHIFT = 1
        private const val CTRL = 2

        /** The scrolling finger's number (the PC's are 0-255). */
        private const val SCROLL_FINGER = 1000

        /** A scrolling finger stays this far from the screen's edges. */
        private const val EDGE_FRACTION = 0.03f

        /**
         * Accessibility settings, with Nudge's entry highlighted where Settings
         * supports it. Not the app's own page (ACCESSIBILITY_DETAILS_SETTINGS):
         * it resolves, but opening it needs a system-only permission, so it
         * fails silently from a notification.
         */
        fun settingsIntent(context: Context): Intent {
            val component = ComponentName(context, ControlService::class.java).flattenToString()
            return Intent(Settings.ACTION_ACCESSIBILITY_SETTINGS)
                .putExtra(":settings:fragment_args_key", component)
                .putExtra(":settings:show_fragment_args", Bundle().apply { putString(":settings:fragment_args_key", component) })
        }
    }

    private var thread: HandlerThread? = null
    private lateinit var handler: Handler

    override fun onServiceConnected() {
        // Input waits behind nothing: the screen encoder runs at the same time.
        val t = HandlerThread("nudge-control", android.os.Process.THREAD_PRIORITY_URGENT_DISPLAY).apply { start() }
        thread = t
        handler = Handler(t.looper)
        instance = this
        Log.i(TAG, "control: on")
    }

    override fun onUnbind(intent: Intent?): Boolean {
        stop()
        return super.onUnbind(intent)
    }

    override fun onDestroy() {
        stop()
        super.onDestroy()
    }

    private fun stop() {
        if (instance === this) instance = null
        thread?.quitSafely()
        thread = null
    }

    override fun onAccessibilityEvent(event: AccessibilityEvent?) {}

    override fun onInterrupt() {}

    // ---- Input from the PC (any thread) ----

    /**
     * A touch by finger [pointer] (0 the mouse, 1 the game-mode joystick):
     * [action] 0 down, 1 move, 2 up, 3 cancel; [x], [y] 0..1 across the screen.
     */
    fun touch(pointer: Int, action: Int, x: Float, y: Float) = act { onTouch(pointer, action, x, y) }

    /** Wheel scrolling at ([x], [y]); [dx], [dy] in 1/120 notch, positive dy = wheel away from the user. */
    fun scroll(x: Float, y: Float, dx: Int, dy: Int) = act { onScroll(x, y, dx, dy) }

    /** Touchpad scrolling at ([x], [y]): the content dragged by [fx], [fy] of the screen's width and height. */
    fun pan(x: Float, y: Float, fx: Float, fy: Float) = act { onPan(x, y, fx, fy) }

    /**
     * Touchpad swipe measured on the touchpad: its fingers moved [dxMm],
     * [dyMm]. The finger here moves the same distance on this screen, so it
     * feels as swiping on the phone. 0, 0: the fingers rest on the pad.
     */
    fun swipe(x: Float, y: Float, dxMm: Float, dyMm: Float) = act { onSwipe(x, y, dxMm, dyMm) }

    /** The PC's fingers left the touchpad. */
    fun panEnd() = act { onPanEnd() }

    /** A key, with [mods]: 1 Shift, 2 Ctrl. */
    fun key(key: Int, mods: Int) = act { onKey(key, mods) }

    fun text(text: String) = enqueue(Job.Typed(text))

    /** The session ended: let go of anything held. */
    fun release() = act {
        for ((id, f) in fingers) {
            if (id != SCROLL_FINGER && f.down) lift(f)
        }
        if (scrolling) endScroll()
    }

    // Everything from the PC goes through one queue, in order, on [handler].
    // Text that piled up meanwhile (fast typing while the app was slow to
    // answer) goes into the field in one go instead of a character at a time.

    private sealed interface Job {
        class Typed(val text: String) : Job
        class Run(val block: () -> Unit) : Job
    }

    private val jobs = java.util.concurrent.ConcurrentLinkedQueue<Job>()
    private val draining = java.util.concurrent.atomic.AtomicBoolean(false)

    private fun act(block: () -> Unit) = enqueue(Job.Run(block))

    private fun enqueue(job: Job) {
        jobs.add(job)
        if (draining.compareAndSet(false, true)) handler.post(drain)
    }

    private val drain = Runnable {
        // Cleared first: a job added from now on posts another drain (which may find nothing).
        draining.set(false)
        while (true) {
            when (val job = jobs.poll() ?: break) {
                is Job.Run -> job.block()
                is Job.Typed -> {
                    val all = StringBuilder(job.text)
                    while (jobs.peek() is Job.Typed) all.append((jobs.poll() as Job.Typed).text)
                    insert(all.toString())
                }
            }
        }
    }

    /** Lifts fingers whose "up" never came ([RemoteControl] checks). */
    fun liftIfQuiet(quietMs: Long) = act {
        val now = SystemClock.uptimeMillis()
        for ((id, f) in fingers) {
            if (id != SCROLL_FINGER && f.down && now - f.heardAt >= quietMs) {
                Log.i(TAG, "control: no word from the PC about finger $id, lifting")
                lift(f)
            }
        }
    }

    // ---- Touch and scrolling: fingers, each with a queue of what it should do ----
    //
    // Android plays one gesture at a time, and a new one cancels any finger
    // still held from the last that it doesn't continue. So every gesture
    // carries every held finger: the ones with something to do move, the
    // rest stay where they are.

    private sealed class Op {
        abstract val at: PointF

        /** When it came in, for the timings log. */
        val queued = SystemClock.uptimeMillis()
        class Down(override val at: PointF) : Op()
        class Move(override val at: PointF) : Op()

        /** [flick]: the last stretch is a quick flick (a touchpad swipe's end). */
        class Up(override val at: PointF, val flick: Boolean = false) : Op()
    }

    private class Finger {
        val ops = ArrayDeque<Op>()

        /** The stroke on the screen now (the finger is down), to continue. */
        var stroke: GestureDescription.StrokeDescription? = null

        /** Where the stroke ends now. */
        var last = PointF()

        /** Down, as far as the PC has told us. */
        var down = false
        var heardAt = 0L

        /** When the input the stroke has reached so far came in. */
        var reachedAt = 0L
    }

    private val fingers = HashMap<Int, Finger>()
    private var inFlight = false

    /** The scrolling finger: down, where it is, where it started. */
    private var scrolling = false
    private var scrollAt = PointF()
    private var scrollFrom = PointF()
    private var lastScrollAt = 0L

    /** The screen's size as it's turned now, px. Read when a touch or scroll starts. */
    private var width = 1f
    private var height = 1f

    private fun measure() {
        val wm = getSystemService(WindowManager::class.java)
        if (Build.VERSION.SDK_INT >= 30) {
            val b = wm.maximumWindowMetrics.bounds
            width = b.width().toFloat()
            height = b.height().toFloat()
        } else {
            val p = android.graphics.Point()
            @Suppress("DEPRECATION")
            wm.defaultDisplay.getRealSize(p)
            width = p.x.toFloat()
            height = p.y.toFloat()
        }
    }

    /**
     * A point on the screen, on whole pixels. Android only continues a held
     * finger from exactly where it stopped; whole pixels keep the ends of
     * each stretch exact (fractions drifted by rounding now and then, and
     * Android cancelled every finger, the game joystick included).
     */
    private fun at(x: Float, y: Float) = PointF(
        Math.round(x.coerceIn(0f, width - 1)).toFloat(),
        Math.round(y.coerceIn(0f, height - 1)).toFloat(),
    )

    private fun px(x: Float, y: Float) = at(x * (width - 1), y * (height - 1))

    private fun finger(id: Int) = fingers.getOrPut(id) { Finger() }

    private fun pcFingerDown() = fingers.any { (id, f) -> id != SCROLL_FINGER && f.down }

    private fun lift(f: Finger) {
        f.down = false
        f.ops.addLast(Op.Up(PointF(f.last.x, f.last.y)))
        pump()
    }

    private fun onTouch(pointer: Int, action: Int, x: Float, y: Float) {
        val f = finger(pointer)
        f.heardAt = SystemClock.uptimeMillis()
        when (action) {
            0 -> {
                if (scrolling) endScroll()
                if (f.down) f.ops.addLast(Op.Up(f.last)) // its "up" was lost
                if (!pcFingerDown()) measure()
                f.down = true
                f.ops.addLast(Op.Down(px(x, y)))
            }
            1 -> if (f.down) f.ops.addLast(Op.Move(px(x, y)))
            // Android can't be told "cancel": lifting where it is comes closest.
            2, 3 -> if (f.down) {
                f.down = false
                f.ops.addLast(Op.Up(if (action == 2) px(x, y) else PointF(f.last.x, f.last.y)))
            }
        }
        pump()
    }

    /** Mouse wheel: a finger drags the content a fixed step per notch. */
    private fun onScroll(x: Float, y: Float, dx: Int, dy: Int) {
        if (pcFingerDown()) return // mid-drag: scrolling means nothing to a finger
        if (scrolling && panning) endScroll() // wheel after touchpad: a new drag
        val f = finger(SCROLL_FINGER)
        val density = resources.displayMetrics.density
        // The finger drags the content: wheel away from the user = content down.
        val mx = -dx / 120f * NOTCH_DP * density
        val my = dy / 120f * NOTCH_DP * density
        if (!scrolling) {
            measure()
            scrollFrom = px(x, y)
            scrollAt = scrollFrom
            scrolling = true
            panning = false
            f.ops.addLast(Op.Down(scrollFrom))
        }
        var to = at(scrollAt.x + mx, scrollAt.y + my)
        if (!inside(to)) {
            // Out of room: lift, and take hold again where scrolling started.
            f.ops.addLast(Op.Up(scrollAt))
            f.ops.addLast(Op.Down(scrollFrom))
            to = at(
                (scrollFrom.x + mx).coerceIn(edgeX(), width - edgeX()),
                (scrollFrom.y + my).coerceIn(edgeY(), height - edgeY()),
            )
        }
        scrollAt = to
        f.ops.addLast(Op.Move(to))
        pump()
        scrolled(SCROLL_IDLE_MS)
    }

    /** The scrolling finger is a touchpad swipe ([onPan]), not wheel notches. */
    private var panning = false

    /**
     * Touchpad: like a finger on the phone. The finger follows the touchpad
     * fingers as they move (as far as the PC would scroll), stays down while
     * they rest on the pad, and lets go the moment they leave it ([onPanEnd],
     * from the PC): at the speed they were moving, so a quick swipe flings
     * (one page in a pager) and a slow or half swipe stops or snaps back as
     * on the phone. Without word from the PC (an older one), it lets go after
     * [PAN_IDLE_MS] of quiet.
     */
    private fun onSwipe(x: Float, y: Float, dxMm: Float, dyMm: Float) {
        if (dxMm == 0f && dyMm == 0f) {
            // Still touching: keep holding (no new touch for resting fingers).
            if (scrolling && panning) scrolled(PAN_IDLE_MS)
            return
        }
        if (!scrolling) measure()
        val dm = resources.displayMetrics
        // Pixels per mm on this screen (its real density; the rounded one if that's implausible).
        val dpi = ((dm.xdpi + dm.ydpi) / 2).takeIf { it in 100f..1000f } ?: dm.densityDpi.toFloat()
        val perMm = dpi / 25.4f
        onPan(x, y, dxMm * perMm / width, dyMm * perMm / height, exact = true)
    }

    /**
     * [exact]: the movement is the finger's own (a measured swipe): no jump
     * over the touch slop, the app sees a finger exactly as on the phone.
     */
    private fun onPan(x: Float, y: Float, fx: Float, fy: Float, exact: Boolean = false) {
        if (pcFingerDown()) return
        if (scrolling && !panning) endScroll() // touchpad after wheel: a new drag
        val f = finger(SCROLL_FINGER)
        val now = SystemClock.uptimeMillis()
        if (!scrolling) {
            measure()
            // Where the pointer is, but with room to move: a swipe started
            // by an edge would reach it at once and stop there.
            scrollFrom = at(
                (x * (width - 1)).coerceIn(width * PAN_START_MARGIN, width * (1 - PAN_START_MARGIN)),
                (y * (height - 1)).coerceIn(height * PAN_START_MARGIN, height * (1 - PAN_START_MARGIN)),
            )
            scrollAt = scrollFrom
            scrolling = true
            panning = true
            panTravel = PointF()
            slop = null
            swipe = Swipe(now)
            f.ops.addLast(Op.Down(scrollFrom))
        }
        panTravel = PointF(panTravel.x + fx * width, panTravel.y + fy * height)
        // Android ignores a touch's first few mm (touch slop) before it
        // scrolls: the finger jumps them at the first movement, so the
        // content follows from the first mm instead of lagging, then lurching.
        val s = if (exact) PointF() else slop ?: run {
            val len = Math.hypot(panTravel.x.toDouble(), panTravel.y.toDouble()).toFloat()
            if (len < 0.5f) return@run null
            val reach = android.view.ViewConfiguration.get(this).scaledTouchSlop + 2f
            PointF(panTravel.x / len * reach, panTravel.y / len * reach).also { slop = it }
        } ?: PointF()
        // At the screen's edge it stops, as a finger would.
        val to = at(
            (scrollFrom.x + s.x + panTravel.x).coerceIn(1f, width - 2),
            (scrollFrom.y + s.y + panTravel.y).coerceIn(1f, height - 2),
        )
        swipe?.moved(now)
        if (to != scrollAt) {
            scrollAt = to
            f.ops.addLast(Op.Move(to))
            pump()
        }
        scrolled(PAN_IDLE_MS)
    }

    /** How far the touchpad fingers have moved this swipe (px on the phone), and the touch slop jumped. */
    private var panTravel = PointF()
    private var slop: PointF? = null

    /** For the log: one line per touchpad swipe. */
    private class Swipe(val start: Long) {
        var events = 0
        var last = start
        fun moved(now: Long) {
            events++
            last = now
        }
    }

    private var swipe: Swipe? = null

    /** The PC's fingers left the touchpad: let go now. */
    private fun onPanEnd() {
        if (scrolling && panning) endScroll(byLift = true)
    }

    /** Scrolling input just came: the finger lifts after [idle] ms without more. */
    private fun scrolled(idle: Long) {
        lastScrollAt = SystemClock.uptimeMillis()
        handler.postDelayed({
            if (scrolling && SystemClock.uptimeMillis() - lastScrollAt >= idle) endScroll()
        }, idle)
    }

    private fun edgeX() = width * EDGE_FRACTION
    private fun edgeY() = height * EDGE_FRACTION
    private fun inside(p: PointF) = p.x in edgeX()..(width - edgeX()) && p.y in edgeY()..(height - edgeY())

    /**
     * Lifts the scrolling finger where it is. Wheel steps: it has held still
     * since its last move, so nothing flings. A touchpad swipe: at once, with
     * the speed it had.
     */
    private fun endScroll(byLift: Boolean = false) {
        scrolling = false
        if (panning) swipe?.let { w ->
            val now = SystemClock.uptimeMillis()
            Log.i(TAG, "control: swipe ${w.last - w.start} ms, ${w.events} moves, " +
                "travel ${panTravel.x.toInt()},${panTravel.y.toInt()} px, " +
                "ended by ${if (byLift) "fingers lifted (${now - w.last} ms after the last move)" else "a pause (no lift from the PC)"}")
        }
        swipe = null
        finger(SCROLL_FINGER).ops.addLast(Op.Up(scrollAt))
        pump()
    }

    /**
     * Touch timings on the phone, logged every 10 s: how long input waited
     * for the gesture before to finish, and how long Android took to play
     * each gesture and say so. With the PC's numbers ("mirror input:" in the
     * app's log), the whole way from mouse to screen.
     */
    private val timings = object {
        val wait = ArrayList<Long>()
        val play = ArrayList<Long>()
        var cancelled = 0
        var since = SystemClock.uptimeMillis()

        fun waited(ms: Long) {
            wait += ms
        }

        fun played(ms: Long, cancelled: Boolean) {
            play += ms
            if (cancelled) this.cancelled++
            val now = SystemClock.uptimeMillis()
            if (now - since < 10_000) return
            fun p(v: List<Long>, q: Double) = v.sorted().let { if (it.isEmpty()) 0 else it[((it.size - 1) * q).toInt()] }
            Log.i(TAG, "control: ${play.size} gestures; input waited p50 ${p(wait, .5)} ms, p95 ${p(wait, .95)}; " +
                "played p50 ${p(play, .5)} ms, p95 ${p(play, .95)}, max ${p(play, 1.0)}; ${this.cancelled} cancelled")
            wait.clear()
            play.clear()
            this.cancelled = 0
            since = now
        }
    }

    /**
     * A stretch from [from] through [points] (the positions the PC sent, in
     * order, so a curved drag stays curved). Never zero length: Android can't
     * place a finger on an empty path and cancels the whole gesture (a held
     * key repeating its position cancelled the joystick every 200 ms). A
     * finger that stays put goes one pixel out and back.
     */
    private fun stretch(from: PointF, points: List<PointF>) = Path().apply {
        moveTo(from.x, from.y)
        var prev = from
        for (p in points) {
            if (p == prev) continue
            lineTo(p.x, p.y)
            prev = p
        }
        if (points.all { it == from }) {
            val nudge = if (from.x >= width - 2) -1f else 1f
            lineTo(from.x + nudge, from.y)
            lineTo(from.x, from.y)
        }
    }

    /** One finger's part in the next gesture, and whether it stays down after. */
    private class Part(
        val finger: Finger,
        val stroke: GestureDescription.StrokeDescription,
        val held: Boolean,
        val moved: Boolean,
    )

    /**
     * Dispatches the next gesture once the one on the screen has played out:
     * for each finger, the moves queued since, ending with a lift if one is
     * queued (or a new touch waits behind them); new touches start; fingers
     * with nothing to do hold still. A gesture while one plays would cancel it.
     */
    private fun pump() {
        if (inFlight) return
        val now = SystemClock.uptimeMillis()
        val parts = mutableListOf<Part>()
        // The longest any input in this gesture waited, for the timings log.
        var oldest = now
        for (f in fingers.values) {
            val current = f.stroke
            if (current == null) {
                // Nothing held: moves and lifts left over belong to a touch that's gone.
                while (f.ops.isNotEmpty() && f.ops.first() !is Op.Down) f.ops.removeFirst()
                val down = f.ops.removeFirstOrNull() ?: continue
                oldest = minOf(oldest, down.queued)
                f.reachedAt = down.queued
                // Moves already waiting go in the same stroke: a touch starts
                // moving at once (the game joystick: down and pushed in one go),
                // and a whole tap or swipe that's already in goes as one.
                val points = mutableListOf<PointF>()
                var lift = false
                var flick = false
                while (f.ops.isNotEmpty()) {
                    when (val op = f.ops.first()) {
                        is Op.Move -> { points += op.at; f.reachedAt = op.queued; f.ops.removeFirst() }
                        is Op.Up -> { points += op.at; f.reachedAt = op.queued; f.ops.removeFirst(); lift = true; flick = op.flick; break }
                        is Op.Down -> { lift = true; break }
                    }
                }
                // Its end is exact (whole pixels), so the next stretch can continue it.
                val path = stretch(down.at, points)
                f.last = points.lastOrNull() ?: down.at
                val still = points.all { it == down.at }
                val time = when {
                    flick -> FLICK_MS
                    still -> 1L
                    else -> FIRST_SEGMENT_MS
                }
                parts += Part(f, GestureDescription.StrokeDescription(path, 0, time, !lift), held = !lift, moved = true)
                continue
            }
            val start = f.last
            val points = mutableListOf<PointF>()
            var lift = false
            var flick = false
            var newest = 0L
            while (f.ops.isNotEmpty()) {
                when (val op = f.ops.first()) {
                    is Op.Move -> { points += op.at; newest = op.queued; oldest = minOf(oldest, op.queued); f.ops.removeFirst() }
                    is Op.Up -> {
                        points += op.at
                        newest = op.queued
                        oldest = minOf(oldest, op.queued)
                        f.ops.removeFirst()
                        lift = true
                        flick = op.flick
                        break
                    }
                    is Op.Down -> { lift = true; break } // lift here first
                }
            }
            // From where it is through the points since: exact ends, see `at`.
            val path = stretch(f.last, points)
            // As long as the movement took to come in, less how far behind the
            // newest point already is: the mouse's speed, without falling behind.
            val duration = if (newest == 0L) MIN_SEGMENT_MS
            else ((newest - f.reachedAt) - (now - newest)).coerceIn(MIN_SEGMENT_MS, MAX_SEGMENT_MS)
            if (newest != 0L) f.reachedAt = newest
            val next = try {
                // Not moving: no reason to take time (a tap lifts at once).
                val still = points.all { it == f.last }
                current.continueStroke(path, 0, if (flick) FLICK_MS else if (still) 1 else duration, !lift)
            } catch (e: RuntimeException) {
                Log.w(TAG, "control: couldn't continue a stroke", e)
                f.stroke = null
                continue
            }
            points.lastOrNull()?.let { f.last = it }
            // A repeat of where it already is (a key held down) needs no gesture.
            parts += Part(f, next, held = !lift, moved = lift || points.any { it != start })
        }
        // Only held fingers standing still: nothing to do.
        if (parts.none { it.moved }) return
        val builder = GestureDescription.Builder()
        parts.take(GestureDescription.getMaxStrokeCount()).forEach { builder.addStroke(it.stroke) }
        inFlight = true
        timings.waited(now - oldest)
        val ok = dispatchGesture(builder.build(), object : GestureResultCallback() {
            override fun onCompleted(g: GestureDescription?) {
                timings.played(SystemClock.uptimeMillis() - now, cancelled = false)
                inFlight = false
                parts.forEach { it.finger.stroke = if (it.held) it.stroke else null }
                pump()
            }

            override fun onCancelled(g: GestureDescription?) {
                timings.played(SystemClock.uptimeMillis() - now, cancelled = true)
                // Usually someone touched the phone itself (or Android cut it
                // short): every finger is off the screen now. Their touch
                // wins: putting a finger back down would cancel it (a swipe
                // on a notification never got through while two fingers
                // rested on the PC's touchpad). Only a finger the PC is
                // working right now goes back (the game joystick held by a
                // key, a drag under way), or it'd stay dead until let go.
                inFlight = false
                val now = SystemClock.uptimeMillis()
                var back = 0
                for ((id, f) in fingers) {
                    val held = f.stroke != null && f.down && id != SCROLL_FINGER && now - f.heardAt < ACTIVE_MS
                    f.stroke = null
                    if (held && f.ops.firstOrNull() !is Op.Down) {
                        f.ops.addFirst(Op.Down(f.last))
                        back++
                    } else if (!held) {
                        f.ops.clear()
                    }
                }
                // A touchpad scroll or wheel steps: over (new ones start afresh).
                if (scrolling) {
                    scrolling = false
                    panning = false
                    swipe = null
                }
                Log.i(TAG, "control: a touch on the phone (or Android) cancelled the PC's gesture; $back finger(s) put back")
                pump()
            }
        }, handler)
        if (!ok) {
            inFlight = false
            fingers.values.forEach { it.stroke = null }
        }
    }

    // ---- Keys ----

    private fun onKey(key: Int, mods: Int) {
        val shift = mods and SHIFT != 0
        val ctrl = mods and CTRL != 0
        if (key in 11..18 && pressKey(key, shift, ctrl)) return
        when (key) {
            1 -> performGlobalAction(GLOBAL_ACTION_BACK)
            2 -> performGlobalAction(GLOBAL_ACTION_HOME)
            3 -> performGlobalAction(GLOBAL_ACTION_RECENTS)
            4 -> performGlobalAction(GLOBAL_ACTION_NOTIFICATIONS)
            5 -> performGlobalAction(GLOBAL_ACTION_QUICK_SETTINGS)
            10 -> if (shift) editor()?.let { it.replace(it.start, it.end, "\n") } else enter()
            11 -> delete(forward = false, word = ctrl)
            12 -> delete(forward = true, word = ctrl)
            13 -> moveCursor(forward = false, word = ctrl, extend = shift)
            14 -> moveCursor(forward = true, word = ctrl, extend = shift)
            15 -> editor()?.moveLine(forward = false, extend = shift)
            16 -> editor()?.moveLine(forward = true, extend = shift)
            17 -> moveToEdge(end = false, wholeText = ctrl, extend = shift)
            18 -> moveToEdge(end = true, wholeText = ctrl, extend = shift)
            20 -> editor()?.menu(android.R.id.selectAll, AccessibilityNodeInfo.ACTION_SET_SELECTION)
            21 -> editor()?.menu(android.R.id.copy, AccessibilityNodeInfo.ACTION_COPY)
            22 -> editor()?.menu(android.R.id.cut, AccessibilityNodeInfo.ACTION_CUT)
            23 -> editor()?.menu(android.R.id.paste, AccessibilityNodeInfo.ACTION_PASTE)
        }
    }

    /**
     * Editing keys (Backspace, Delete, arrows, Home/End) as key presses, the
     * way a hardware keyboard sends them: the app does what it does for a
     * keyboard (Ctrl deletes or jumps a word, Shift selects), and nothing has
     * to be read from the field first. Reading it is a call into the app
     * that waits for its answer, which made fast typing lag. False without a
     * keyboard connection (before Android 13, or none open).
     */
    private fun pressKey(key: Int, shift: Boolean, ctrl: Boolean): Boolean {
        if (Build.VERSION.SDK_INT < 33) return false
        val ic = inputMethod?.currentInputConnection ?: return false
        val code = when (key) {
            11 -> KeyEvent.KEYCODE_DEL
            12 -> KeyEvent.KEYCODE_FORWARD_DEL
            13 -> KeyEvent.KEYCODE_DPAD_LEFT
            14 -> KeyEvent.KEYCODE_DPAD_RIGHT
            15 -> KeyEvent.KEYCODE_DPAD_UP
            16 -> KeyEvent.KEYCODE_DPAD_DOWN
            17 -> KeyEvent.KEYCODE_MOVE_HOME
            18 -> KeyEvent.KEYCODE_MOVE_END
            else -> return false
        }
        var meta = 0
        if (shift) meta = meta or KeyEvent.META_SHIFT_ON or KeyEvent.META_SHIFT_LEFT_ON
        if (ctrl) meta = meta or KeyEvent.META_CTRL_ON or KeyEvent.META_CTRL_LEFT_ON
        anchor = null
        val t = SystemClock.uptimeMillis()
        ic.sendKeyEvent(KeyEvent(t, t, KeyEvent.ACTION_DOWN, code, 0, meta))
        ic.sendKeyEvent(KeyEvent(t, t, KeyEvent.ACTION_UP, code, 0, meta))
        return true
    }

    // ---- Typing ----
    //
    // Two ways into the focused text field. Android 13+: as a keyboard
    // (this service's InputMethod, `flagInputMethodEditor`), which reaches
    // every field a keyboard can type in: browsers, Flutter and Compose
    // apps, games. Otherwise, or if no keyboard connection is open: the
    // field's accessibility node (ACTION_SET_TEXT), for standard fields.

    /**
     * The focused text field: some of its text around the cursor, the
     * selection in it (start <= end), and how to change it.
     */
    private abstract inner class Editor {
        abstract val text: String
        abstract val start: Int
        abstract val end: Int
        abstract val multiLine: Boolean

        /** Replaces [from]..[to] of [text] with [with], cursor after it. */
        abstract fun replace(from: Int, to: Int, with: String)
        abstract fun selectRaw(a: Int, b: Int)
        abstract fun moveLine(forward: Boolean, extend: Boolean)

        /** Select all, copy, cut, paste. */
        abstract fun menu(id: Int, action: Int)

        /** The keyboard's action key (Go, Search, Send). */
        abstract fun imeAction()

        /** If the field's keyboard action is Send: does it, true. */
        open fun sendAction(): Boolean = false

        /** If the field's keyboard action is Search or Go (search and address bars): does it, true. */
        open fun searchAction(): Boolean = false

        /** The field's node, if known (to find a Send button near it). */
        abstract val node: AccessibilityNodeInfo?

        fun select(a: Int, b: Int) {
            anchor = null
            selectRaw(a, b)
        }
    }

    /** As a keyboard (Android 13+). */
    @androidx.annotation.RequiresApi(33)
    private inner class KeyboardEditor(
        private val ic: InputMethod.AccessibilityInputConnection,
        private val info: android.view.inputmethod.EditorInfo?,
        surrounding: android.view.inputmethod.SurroundingText,
    ) : Editor() {
        override val text = surrounding.text.toString()
        private val offset = surrounding.offset
        override val start = minOf(surrounding.selectionStart, surrounding.selectionEnd).coerceIn(0, text.length)
        override val end = maxOf(surrounding.selectionStart, surrounding.selectionEnd).coerceIn(0, text.length)
        override val multiLine = info != null &&
            info.inputType and android.text.InputType.TYPE_TEXT_FLAG_MULTI_LINE != 0
        override val node: AccessibilityNodeInfo? get() = findFocus(AccessibilityNodeInfo.FOCUS_INPUT)

        override fun replace(from: Int, to: Int, with: String) {
            anchor = null
            ic.setSelection(offset + from, offset + to)
            ic.commitText(with, 1, null)
        }

        override fun selectRaw(a: Int, b: Int) {
            ic.setSelection(offset + a, offset + b)
        }

        override fun moveLine(forward: Boolean, extend: Boolean) {
            anchor = null
            key(if (forward) KeyEvent.KEYCODE_DPAD_DOWN else KeyEvent.KEYCODE_DPAD_UP, if (extend) KeyEvent.META_SHIFT_ON or KeyEvent.META_SHIFT_LEFT_ON else 0)
        }

        override fun menu(id: Int, action: Int) {
            ic.performContextMenuAction(id)
        }

        override fun sendAction(): Boolean {
            val action = (info?.imeOptions ?: 0) and android.view.inputmethod.EditorInfo.IME_MASK_ACTION
            if (action != android.view.inputmethod.EditorInfo.IME_ACTION_SEND) return false
            ic.performEditorAction(action)
            return true
        }

        override fun searchAction(): Boolean {
            val action = (info?.imeOptions ?: 0) and android.view.inputmethod.EditorInfo.IME_MASK_ACTION
            if (action != android.view.inputmethod.EditorInfo.IME_ACTION_SEARCH &&
                action != android.view.inputmethod.EditorInfo.IME_ACTION_GO
            ) return false
            ic.performEditorAction(action)
            return true
        }

        override fun imeAction() {
            val action = (info?.imeOptions ?: 0) and android.view.inputmethod.EditorInfo.IME_MASK_ACTION
            if (action != android.view.inputmethod.EditorInfo.IME_ACTION_NONE &&
                action != android.view.inputmethod.EditorInfo.IME_ACTION_UNSPECIFIED
            ) {
                ic.performEditorAction(action)
            } else {
                key(KeyEvent.KEYCODE_ENTER, 0)
            }
        }

        private fun key(code: Int, meta: Int) {
            val t = SystemClock.uptimeMillis()
            ic.sendKeyEvent(KeyEvent(t, t, KeyEvent.ACTION_DOWN, code, 0, meta))
            ic.sendKeyEvent(KeyEvent(t, t, KeyEvent.ACTION_UP, code, 0, meta))
        }
    }

    /** Through the field's accessibility node. */
    private inner class NodeEditor(
        override val node: AccessibilityNodeInfo,
        override val text: String,
        override val start: Int,
        override val end: Int,
    ) : Editor() {
        override val multiLine get() = node.isMultiLine

        override fun replace(from: Int, to: Int, with: String) {
            val all = text.substring(0, from) + with + text.substring(to)
            val args = Bundle().apply {
                putCharSequence(AccessibilityNodeInfo.ACTION_ARGUMENT_SET_TEXT_CHARSEQUENCE, all)
            }
            if (!node.performAction(AccessibilityNodeInfo.ACTION_SET_TEXT, args)) return
            if (node.isPassword) password = all
            select(from + with.length, from + with.length)
        }

        override fun selectRaw(a: Int, b: Int) {
            val args = Bundle().apply {
                putInt(AccessibilityNodeInfo.ACTION_ARGUMENT_SELECTION_START_INT, a)
                putInt(AccessibilityNodeInfo.ACTION_ARGUMENT_SELECTION_END_INT, b)
            }
            node.performAction(AccessibilityNodeInfo.ACTION_SET_SELECTION, args)
        }

        override fun moveLine(forward: Boolean, extend: Boolean) {
            anchor = null
            val args = Bundle().apply {
                putInt(
                    AccessibilityNodeInfo.ACTION_ARGUMENT_MOVEMENT_GRANULARITY_INT,
                    AccessibilityNodeInfo.MOVEMENT_GRANULARITY_LINE,
                )
                putBoolean(AccessibilityNodeInfo.ACTION_ARGUMENT_EXTEND_SELECTION_BOOLEAN, extend)
            }
            val action = if (forward) AccessibilityNodeInfo.ACTION_NEXT_AT_MOVEMENT_GRANULARITY
            else AccessibilityNodeInfo.ACTION_PREVIOUS_AT_MOVEMENT_GRANULARITY
            node.performAction(action, args)
        }

        override fun menu(id: Int, action: Int) {
            if (action == AccessibilityNodeInfo.ACTION_SET_SELECTION) select(0, text.length)
            else node.performAction(action)
        }

        override fun imeAction() {
            if (Build.VERSION.SDK_INT >= 30) {
                node.performAction(AccessibilityNodeInfo.AccessibilityAction.ACTION_IME_ENTER.id)
            }
        }
    }

    /**
     * What a password field holds, as typed from here: Android shows it to
     * accessibility services only as dots. Trusted while the length matches.
     */
    private var password = ""

    /**
     * The fixed end of a selection made with Shift (the other end moves).
     * Android reports a selection's two ends but not which one is fixed.
     */
    private var anchor: Int? = null

    /** How much text around the cursor the keyboard way reads. */
    private val AROUND = 2000

    /** The focused text field: as a keyboard where possible, else its node. */
    private fun editor(): Editor? {
        if (Build.VERSION.SDK_INT >= 33) {
            val im = inputMethod
            val ic = im?.currentInputConnection
            val around = runCatching { ic?.getSurroundingText(AROUND, AROUND, 0) }.getOrNull()
            if (ic != null && around != null) return KeyboardEditor(ic, im.currentInputEditorInfo, around)
        }
        return nodeEditor()
    }

    private fun nodeEditor(): Editor? {
        val node = findFocus(AccessibilityNodeInfo.FOCUS_INPUT) ?: return null
        // The cached copy can be out of date: this service doesn't hear about
        // text changes, so after typing it still had the old text, and the
        // next character replaced the ones typed before it.
        node.refresh()
        if (!node.isEditable) return null
        var text = when {
            Build.VERSION.SDK_INT >= 26 && node.isShowingHintText -> ""
            else -> node.text?.toString() ?: ""
        }
        if (node.isPassword) {
            text = when {
                text.isEmpty() -> ""
                password.length == text.length -> password
                else -> {
                    Log.i(TAG, "control: a password field changed elsewhere; can't edit it from here")
                    return null
                }
            }
        }
        var s = node.textSelectionStart
        var e = node.textSelectionEnd
        if (s < 0 || e < 0) { s = text.length; e = text.length }
        if (s > e) s = e.also { e = s }
        return NodeEditor(node, text, s.coerceIn(0, text.length), e.coerceIn(0, text.length))
    }

    private fun insert(typed: String) {
        if (Build.VERSION.SDK_INT >= 33) {
            // Straight in as a keyboard would, without reading the field.
            val ic = inputMethod?.currentInputConnection
            if (ic != null) {
                anchor = null
                ic.commitText(typed, 1, null)
                return
            }
        }
        val e = editor() ?: run {
            Log.i(TAG, "control: typed, but no text field has focus")
            return
        }
        e.replace(e.start, e.end, typed)
    }

    /**
     * Puts the cursor at [to]; with [extend], selects from the selection's
     * fixed end to there, as Shift does on a PC.
     */
    private fun moveTo(e: Editor, to: Int, extend: Boolean, forward: Boolean) {
        if (!extend) {
            e.select(to, to)
            return
        }
        // Keep the anchor while it's still one end of the selection.
        val fixed = anchor?.takeIf { it == e.start || it == e.end }
            ?: if (forward) e.start else e.end
        e.select(minOf(fixed, to), maxOf(fixed, to))
        anchor = fixed
    }

    /** Where the moving end of the selection is (the cursor, if none). */
    private fun moving(e: Editor, forward: Boolean): Int {
        val a = anchor?.takeIf { it == e.start || it == e.end }
        return when {
            a == e.start -> e.end
            a == e.end -> e.start
            forward -> e.end
            else -> e.start
        }
    }

    /** One character (as seen on screen: an emoji counts as one) before or after [at]. */
    private fun neighbour(text: String, at: Int, forward: Boolean): Int {
        val it = BreakIterator.getCharacterInstance().apply { setText(text) }
        val n = if (forward) it.following(at) else it.preceding(at)
        return if (n == BreakIterator.DONE) at else n
    }

    /**
     * The next word boundary from [at], as Ctrl+arrow on a PC: forward, past
     * the end of the next word; back, to the start of the previous one
     * (spaces and punctuation in between are skipped).
     */
    private fun wordEdge(text: String, at: Int, forward: Boolean): Int {
        val it = BreakIterator.getWordInstance().apply { setText(text) }
        var pos = at
        while (true) {
            val next = if (forward) it.following(pos) else it.preceding(pos)
            if (next == BreakIterator.DONE) return if (forward) text.length else 0
            val word = if (forward) text.substring(pos, next) else text.substring(next, pos)
            pos = next
            if (word.any { c -> c.isLetterOrDigit() }) return pos
        }
    }

    /** Backspace or Delete; with [word], a whole word (Ctrl+Backspace). */
    private fun delete(forward: Boolean, word: Boolean) {
        val e = editor() ?: return
        when {
            e.start != e.end -> e.replace(e.start, e.end, "")
            forward && e.end < e.text.length -> {
                val to = if (word) wordEdge(e.text, e.end, true) else neighbour(e.text, e.end, true)
                e.replace(e.start, to, "")
            }
            !forward && e.start > 0 -> {
                val from = if (word) wordEdge(e.text, e.start, false) else neighbour(e.text, e.start, false)
                e.replace(from, e.end, "")
            }
        }
    }

    private fun moveCursor(forward: Boolean, word: Boolean, extend: Boolean) {
        val e = editor() ?: return
        // Without Shift, an arrow first collapses a selection to its edge.
        if (!extend && e.start != e.end && !word) {
            moveTo(e, if (forward) e.end else e.start, extend = false, forward)
            return
        }
        val from = if (extend) moving(e, forward) else if (forward) e.end else e.start
        val to = if (word) wordEdge(e.text, from, forward) else neighbour(e.text, from, forward)
        moveTo(e, to, extend, forward)
    }

    /** Home / End: the line's start or end; with [wholeText], the text's. */
    private fun moveToEdge(end: Boolean, wholeText: Boolean, extend: Boolean) {
        val e = editor() ?: return
        val from = if (extend) moving(e, end) else if (end) e.end else e.start
        val to = when {
            wholeText -> if (end) e.text.length else 0
            end -> e.text.indexOf('\n', from).let { if (it < 0) e.text.length else it }
            else -> e.text.lastIndexOf('\n', (from - 1).coerceAtLeast(0)).let {
                if (it < 0 || from == 0) 0 else it + 1
            }
        }
        moveTo(e, to, extend, end)
    }

    /**
     * Enter, as in a chat on a PC: sends. In order:
     * 1. The field says its keyboard action is Send (Snapchat, many chats):
     *    that action.
     * 2. A Send button by the field, found by its label or ID in the common
     *    apps' words (WhatsApp, Messages, Samsung Messages, Messenger,
     *    Instagram, TikTok, Telegram, Discord, Signal, X, Slack, Teams...),
     *    or the button holding such a label.
     * 3. A chat box (a short multi-line field) with text in it: the button
     *    just right of it on the same row, which in every chat app is Send
     *    once there's text (TikTok and others label it nothing).
     * 4. Otherwise a multi-line field (notes) gets a new line, and a one-line
     *    field (search, address) the keyboard's action.
     */
    private fun enter() {
        val e = editor() ?: return
        if (e.sendAction()) {
            Log.i(TAG, "control: Enter: the field's Send action")
            return
        }
        // Search and address bars search: Google's box is multi-line, so it
        // went looking for a Send button and pressed its Clear button.
        if (e.searchAction()) {
            Log.i(TAG, "control: Enter: the field's Search / Go action")
            return
        }
        val field = e.node
        if (field != null && searchBox(field)) {
            Log.i(TAG, "control: Enter: a search box; its keyboard action")
            e.imeAction()
            return
        }
        // The field's surroundings (the composer row) first: walking the
        // whole window is a call into the app per node, slow on big screens,
        // and typing waits behind it.
        val around = field?.let { around(it) } ?: emptyList()
        val all by lazy { nodes() }
        val named = field?.let { sendButton(it, around) ?: sendButton(it, all) }
        if (named != null) {
            Log.i(TAG, "control: Enter: pressed ${describe(named)}")
            named.performAction(AccessibilityNodeInfo.ACTION_CLICK)
            return
        }
        val unnamed = if (field != null && e.multiLine && e.text.isNotBlank()) {
            buttonRightOf(field, around) ?: buttonRightOf(field, all)
        } else {
            null
        }
        if (unnamed != null) {
            Log.i(TAG, "control: Enter: pressed the unlabelled button by the chat box (${describe(unnamed)})")
            unnamed.performAction(AccessibilityNodeInfo.ACTION_CLICK)
            return
        }
        Log.i(TAG, "control: Enter: no Send found; ${if (e.multiLine) "new line" else "keyboard action"}")
        if (e.multiLine) e.replace(e.start, e.end, "\n") else e.imeAction()
    }

    /** Search and address bars, by their hint, ID or label. */
    private val searchWords = Regex(
        "search|query|omnibox|url_bar|address|location_bar|find|suche|recherch|buscar|cerca|zoek|поиск|搜索|検索|검색",
        RegexOption.IGNORE_CASE,
    )

    private fun searchBox(n: AccessibilityNodeInfo): Boolean {
        val words = listOfNotNull(
            if (Build.VERSION.SDK_INT >= 26) n.hintText?.toString() else null,
            n.viewIdResourceName,
            n.contentDescription?.toString(),
            if (Build.VERSION.SDK_INT >= 28) n.paneTitle?.toString() else null,
        ).joinToString(" ")
        return searchWords.containsMatchIn(words)
    }

    private fun describe(n: AccessibilityNodeInfo) =
        listOfNotNull(n.className?.toString()?.substringAfterLast('.'), n.viewIdResourceName, n.contentDescription, n.text)
            .joinToString(" ")

    /**
     * Words that mark a Send button: its description, text or ID. "Send" in
     * many languages, plus the IDs and names apps use ("Post" for comments).
     */
    private val sendWords = Regex(
        listOf(
            """(^|[^a-z])send([^a-z]|$)""", """(^|[^a-z])post([^a-z]|$)""", "submit", "sendbutton", "send_button", "btn_send", "button_send",
            "compose:draft:send", "envoyer", "enviar", "senden", "invia", "verzend", "wyślij", "gönder", "kirim", "gửi", "ส่ง",
            "отправ", "надісл", "发送", "傳送", "送出", "送信", "전송", "보내기", "إرسال", "שלח", "भेज",
        ).joinToString("|"),
        RegexOption.IGNORE_CASE,
    )

    /** A Send button further than this from the field (px) belongs to something else. */
    private fun sendReach() = resources.displayMetrics.density * 200

    private fun bounds(n: AccessibilityNodeInfo) = android.graphics.Rect().also { n.getBoundsInScreen(it) }

    /** Gap between two rectangles, px (0 if they touch or overlap). */
    private fun gap(a: android.graphics.Rect, b: android.graphics.Rect): Float {
        val dx = maxOf(0, maxOf(a.left - b.right, b.left - a.right))
        val dy = maxOf(0, maxOf(a.top - b.bottom, b.top - a.bottom))
        return (dx + dy).toFloat()
    }

    /** Every node in the active window (bounded: huge screens are cut short). */
    private fun nodes(): List<AccessibilityNodeInfo> = subtree(rootInActiveWindow, 3000)

    /** The nodes near [field]: everything under its ancestor three levels up. */
    private fun around(field: AccessibilityNodeInfo): List<AccessibilityNodeInfo> {
        var top = field
        for (i in 0 until 3) top = top.parent ?: break
        return subtree(top, 300)
    }

    private fun subtree(root: AccessibilityNodeInfo?, limit: Int): List<AccessibilityNodeInfo> {
        root ?: return emptyList()
        val out = ArrayList<AccessibilityNodeInfo>()
        val queue = ArrayDeque<AccessibilityNodeInfo>().apply { add(root) }
        while (queue.isNotEmpty() && out.size < limit) {
            val n = queue.removeFirst()
            out += n
            for (i in 0 until n.childCount) n.getChild(i)?.let { queue.add(it) }
        }
        return out
    }

    /** The control that takes a tap on [n]: it, or the nearest clickable one around it. */
    private fun clickable(n: AccessibilityNodeInfo): AccessibilityNodeInfo? {
        var t: AccessibilityNodeInfo? = n
        var up = 0
        while (t != null && !t.isClickable && up < 4) {
            t = t.parent
            up++
        }
        return t?.takeIf { it.isClickable && it.isEnabled && !it.isEditable }
    }

    /** The chat's Send button, by name, nearest the field. */
    private fun sendButton(field: AccessibilityNodeInfo, among: List<AccessibilityNodeInfo>): AccessibilityNodeInfo? {
        val fb = bounds(field)
        var best: AccessibilityNodeInfo? = null
        var bestGap = Float.MAX_VALUE
        for (n in among) {
            if (!n.isVisibleToUser || n.isEditable) continue
            val name = listOfNotNull(
                n.contentDescription?.toString(),
                n.text?.toString(),
                n.viewIdResourceName?.substringAfter(":id/"),
                if (Build.VERSION.SDK_INT >= 30) n.stateDescription?.toString() else null,
            ).joinToString(" ").replace('_', ' ')
            if (!sendWords.containsMatchIn(name) && !sendWords.containsMatchIn(n.viewIdResourceName ?: "")) continue
            val target = clickable(n) ?: continue
            val g = gap(fb, bounds(target))
            if (g < bestGap && g <= sendReach()) {
                best = target
                bestGap = g
            }
        }
        return best
    }

    /**
     * The clickable control just right of a chat box, on its row: Send, in
     * chat apps, once there's text. Only for a short field (a composer, not
     * a page-sized editor).
     */
    private fun buttonRightOf(field: AccessibilityNodeInfo, among: List<AccessibilityNodeInfo>): AccessibilityNodeInfo? {
        val fb = bounds(field)
        if (fb.height() > resources.displayMetrics.heightPixels / 4) return null
        var best: AccessibilityNodeInfo? = null
        var bestGap = Float.MAX_VALUE
        for (n in among) {
            if (!n.isVisibleToUser || !n.isClickable || !n.isEnabled || n.isEditable) continue
            // Only buttons with no name: a named one isn't Send (that's
            // found by name), it's Clear, the mic, emoji... (Enter pressed
            // a search box's "Clear" and wiped the text.)
            if (!n.contentDescription.isNullOrBlank() || !n.text.isNullOrBlank()) continue
            val b = bounds(n)
            // Right of the field (or at its right end, inside its row), on its row.
            val sameRow = b.centerY() in (fb.top - fb.height() / 2)..(fb.bottom + fb.height() / 2)
            val rightSide = b.centerX() > fb.right - b.width() && b.left >= fb.centerX()
            val small = b.width() < resources.displayMetrics.widthPixels / 3
            if (!sameRow || !rightSide || !small) continue
            // Containers that hold the field aren't buttons.
            if (b.contains(fb)) continue
            val g = Math.abs(b.centerX() - fb.right).toFloat()
            if (g < bestGap && g <= sendReach()) {
                best = n
                bestGap = g
            }
        }
        return best
    }
}
