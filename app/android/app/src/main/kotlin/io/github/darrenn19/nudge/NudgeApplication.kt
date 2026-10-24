package io.github.darrenn19.nudge

import android.app.Application
import android.util.Log

/**
 * Starts Nudge's core with the process, not the window: the core runs in
 * the process itself (see [Core]), with no Flutter engine, so files arrive
 * with no window open for as long as [NudgeService] keeps the process
 * alive. The window attaches to the same core when it opens.
 */
class NudgeApplication : Application() {
    override fun onCreate() {
        super.onCreate()
        Notifications.createChannels(this)
        // Copies left by the last run; nothing can be sending them now.
        SharedFiles.clearCopies(this)
        Background.start(this)
        NudgeService.start(this)
        // Off the main thread: it reads the identity and checks folders.
        Thread({
            Device.holdMulticast(this)
            val error = Core.start(
                Device.dataDir(this).path,
                Device.downloads(this).path,
                Device.name(this),
                Background,
            )
            if (error != null) Log.e("Nudge", "Couldn't start: $error")
            LockState.watch(this)
        }, "nudge-start").start()
    }
}
