package io.github.darrenn19.nudge

import android.app.Application
import io.flutter.embedding.engine.FlutterEngine
import io.flutter.embedding.engine.FlutterEngineCache
import io.flutter.embedding.engine.dart.DartExecutor
import io.flutter.plugin.common.MethodChannel

/**
 * Starts Nudge's Flutter engine with the process, not the window, so Nudge
 * runs (and receives) with no window open, for as long as [NudgeService]
 * keeps the process alive.
 */
class NudgeApplication : Application() {
    override fun onCreate() {
        super.onCreate()
        Notifications.createChannels(this)
        // Copies left by the last run; nothing can be sending them now.
        SharedFiles.clearCopies(this)
        val engine = FlutterEngine(this)
        platform = MethodChannel(engine.dartExecutor.binaryMessenger, PlatformChannel.NAME)
            .apply { setMethodCallHandler(PlatformChannel(this@NudgeApplication)) }
        engine.dartExecutor.executeDartEntrypoint(DartExecutor.DartEntrypoint.createDefault())
        FlutterEngineCache.getInstance().put(ENGINE_ID, engine)
        NudgeService.start(this)
    }

    companion object {
        const val ENGINE_ID = "nudge"

        /** For telling Dart things that happen outside it (a notification's buttons). */
        var platform: MethodChannel? = null
            private set
    }
}
