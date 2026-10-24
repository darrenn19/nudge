import 'dart:async';
import 'dart:io';

import 'package:flutter/cupertino.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import 'controller.dart';
import 'desktop.dart';
import 'notifier.dart';
import 'single_instance.dart';
import 'windows_shell.dart';
import 'platform.dart';
import 'src/rust/api/nudge.dart';
import 'src/rust/frb_generated.dart';
import 'ui/home.dart';
import 'ui/style.dart';
import 'wifi.dart';

Future<void> main(List<String> args) async {
  WidgetsFlutterBinding.ensureInitialized();
  if (Platform.isAndroid) {
    // Draw behind the status and navigation bars, with light icons on the
    // dark background.
    await SystemChrome.setEnabledSystemUIMode(SystemUiMode.edgeToEdge);
    SystemChrome.setSystemUIOverlayStyle(
      const SystemUiOverlayStyle(
        statusBarColor: Colors.transparent,
        systemNavigationBarColor: Colors.transparent,
        statusBarIconBrightness: Brightness.light,
        systemNavigationBarIconBrightness: Brightness.light,
      ),
    );
  }
  await RustLib.init();
  await initWindow(hidden: args.contains(backgroundFlag));
  // Nudge runs before (and without) any screen: Windows starts it hidden at
  // login, and Android starts it at boot or keeps it after the window closes.
  // Either way it must be ready to receive.
  final starting = _start(filesFromArgs(args), Platform.isWindows ? textFromArgs(args) : null);
  // The core starts in well under a second: wait for it, so the first frame
  // is the device list rather than a placeholder that then fades away.
  NudgeController? ready;
  try {
    ready = await starting.timeout(const Duration(milliseconds: 800));
  } catch (_) {
    // Still starting (or failed): the placeholder shows until it's done.
  }
  runApp(NudgeRoot(starting: starting, ready: ready));
}

Future<NudgeController> _start(List<String> files, String? text) async {
  await enableMulticast();
  final dir = await dataDir();
  final app = await NudgeApp.start(
    dataDir: dir,
    downloadDir: await downloadDir(),
    deviceName: await deviceName(),
    platform: platformTag,
  );
  final controller = NudgeController(app, dataDir: dir);
  // Android's background service shows notifications itself (with or
  // without this window); Windows shows its toasts from here.
  if (!Platform.isAndroid) TransferNotifier(controller);
  // Android: files shared to Nudge (share sheet, quick settings tile); text
  // shared to it goes to the other devices' clipboards.
  unawaited(
    listenForShares(
      controller.offer,
      onStarting: controller.warmUp,
      onText: (text, {to}) => unawaited(controller.shareClipboardAndSay(text, leave: true, to: to)),
    ),
  );
  // Android: each paired device in the share sheet's top row.
  if (Platform.isAndroid) {
    var published = '';
    void publish() {
      final devices = controller.pairedDevices;
      final key = [for (final d in devices) '${d.id}=${d.name}'].join('|');
      if (key == published) return;
      published = key;
      unawaited(publishShareTargets([for (final d in devices) (id: d.id, name: d.name)]));
    }

    controller.addListener(publish);
    publish();
  }
  // Clipboard sync. Windows: whatever is copied goes across as it's copied.
  // Android only lets apps read the clipboard on screen: what was copied
  // goes when Nudge opens.
  listenForCopies((text) => unawaited(controller.shareClipboard(text)));
  if (Platform.isAndroid) {
    Future<void> sendCopied() async {
      final text = await newClipboardText();
      if (text != null) await controller.shareClipboardAndSay(text);
    }

    AppLifecycleListener(onResume: () => unawaited(sendCopied()));
    unawaited(sendCopied());
  }
  // A direct link left over from a run that stopped while joined.
  unawaited(forgetHotspot());
  // Windows: tray icon, Explorer menu, start at login, and files from other
  // launches.
  if (Platform.isWindows) {
    unawaited(Tray.start(controller, dataDir: dir));
    void offerPaths(List<String> paths) {
      unawaited(showWindow());
      controller.offer(paths.map(OutgoingFile.fromPath).toList());
    }

    // Text from the Share panel: onto the other devices' clipboards.
    void copyAcross(String text) {
      unawaited(showWindow());
      unawaited(controller.shareClipboardAndSay(text, leave: true));
    }

    unawaited(
      listenForLaunches(
        onFiles: offerPaths,
        onText: copyAcross,
        onShow: () => unawaited(showWindow()),
      ),
    );
    if (files.isNotEmpty) offerPaths(files);
    if (text != null) copyAcross(text);
    // Housekeeping that can wait until Nudge is up and idle.
    unawaited(
      Future<void>.delayed(const Duration(seconds: 3), () async {
        prepareToasts();
        await registerShell(dir);
        await installSendTo();
      }),
    );
  }
  return controller;
}

class NudgeRoot extends StatelessWidget {
  const NudgeRoot({super.key, required this.starting, this.ready});

  final Future<NudgeController> starting;

  /// Already started: shown straight away.
  final NudgeController? ready;

  @override
  Widget build(BuildContext context) {
    // Nothing animates while the window is hidden in the tray or minimized.
    return ValueListenableBuilder<bool>(
      valueListenable: windowShown,
      builder: (context, shown, child) => TickerMode(enabled: shown, child: child!),
      child: _app(),
    );
  }

  Widget _app() {
    return MaterialApp(
      title: 'Nudge',
      debugShowCheckedModeBanner: false,
      // Frame-time graphs, for checking smoothness:
      // flutter run --profile --dart-define=NUDGE_PERF=true
      showPerformanceOverlay: const bool.fromEnvironment('NUDGE_PERF'),
      theme: _theme(),
      // One background behind every screen: moving between screens only
      // moves their content, never repaints the backdrop.
      builder: (context, child) => NudgeBackground(child: child ?? const SizedBox()),
      home: ready != null
          ? HomeScreen(controller: ready!)
          : FutureBuilder<NudgeController>(
              future: starting,
              builder: (context, snapshot) {
                final Widget page;
                if (snapshot.hasError) {
                  page = _StartError(error: '${snapshot.error}');
                } else if (snapshot.data case final controller?) {
                  page = HomeScreen(controller: controller);
                } else {
                  page = const _Starting();
                }
                return AnimatedSwitcher(duration: const Duration(milliseconds: 300), child: page);
              },
            ),
    );
  }
}

/// Always dark, with iOS-style motion on every platform, so the phone and
/// the desktop look and feel the same.
ThemeData _theme() {
  const transitions = CupertinoPageTransitionsBuilder();
  return ThemeData(
    brightness: Brightness.dark,
    colorScheme: ColorScheme.fromSeed(
      seedColor: Palette.accent,
      brightness: Brightness.dark,
      surface: Palette.bottom,
    ),
    scaffoldBackgroundColor: Palette.bottom,
    splashFactory: NoSplash.splashFactory,
    highlightColor: Colors.transparent,
    textSelectionTheme: const TextSelectionThemeData(cursorColor: Palette.accent),
    tooltipTheme: TooltipThemeData(
      decoration: BoxDecoration(
        color: Palette.top.withValues(alpha: 0.95),
        borderRadius: BorderRadius.circular(8),
      ),
      textStyle: const TextStyle(color: Palette.text, fontSize: 12),
    ),
    pageTransitionsTheme: const PageTransitionsTheme(
      builders: {
        TargetPlatform.android: transitions,
        TargetPlatform.windows: transitions,
        TargetPlatform.linux: transitions,
        TargetPlatform.macOS: transitions,
        TargetPlatform.iOS: transitions,
      },
    ),
  );
}

class _Starting extends StatelessWidget {
  const _Starting();

  @override
  Widget build(BuildContext context) {
    return const Scaffold(
      backgroundColor: Colors.transparent,
      body: Center(child: Orb(size: 200)),
    );
  }
}

class _StartError extends StatelessWidget {
  const _StartError({required this.error});

  final String error;

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      backgroundColor: Colors.transparent,
      body: RepaintBoundary(
        child: Center(
          child: Padding(
            padding: const EdgeInsets.all(32),
            child: Column(
              mainAxisSize: MainAxisSize.min,
              children: [
                const Icon(CupertinoIcons.exclamationmark_circle, size: 48, color: Palette.bad),
                const SizedBox(height: 16),
                const Text("Nudge couldn't start", style: Fonts.title),
                const SizedBox(height: 8),
                SelectableText(error, textAlign: TextAlign.center, style: Fonts.body),
              ],
            ),
          ),
        ),
      ),
    );
  }
}
