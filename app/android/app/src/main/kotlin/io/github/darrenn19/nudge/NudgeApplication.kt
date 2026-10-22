package io.github.darrenn19.nudge

import android.app.Application
import io.flutter.embedding.engine.FlutterEngine
import io.flutter.embedding.engine.FlutterEngineCache
import io.flutter.embedding.engine.dart.DartExecutor
import io.flutter.plugin.common.MethodChannel

/**
 * Starts Nudge's Flutter engine with the process, not the window. Transfers
 * keep running (and reporting progress) after the user leaves the app, as
 * long as [TransferService] holds the process in the foreground.
 */
class NudgeApplication : Application() {
    override fun onCreate() {
        super.onCreate()
        Notifications.createChannels(this)
        // Copies left by the last run; nothing can be sending them now.
        SharedFiles.clearCopies(this)
        val engine = FlutterEngine(this)
        MethodChannel(engine.dartExecutor.binaryMessenger, PlatformChannel.NAME)
            .setMethodCallHandler(PlatformChannel(this))
        engine.dartExecutor.executeDartEntrypoint(DartExecutor.DartEntrypoint.createDefault())
        FlutterEngineCache.getInstance().put(ENGINE_ID, engine)
    }

    companion object {
        const val ENGINE_ID = "nudge"
    }
}
