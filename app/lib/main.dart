import 'dart:async';
import 'dart:io';

import 'package:flutter/material.dart';

import 'controller.dart';
import 'desktop.dart';
import 'notifier.dart';
import 'single_instance.dart';
import 'windows_shell.dart';
import 'platform.dart';
import 'src/rust/api/nudge.dart';
import 'src/rust/frb_generated.dart';
import 'ui/home.dart';

Future<void> main(List<String> args) async {
  WidgetsFlutterBinding.ensureInitialized();
  SingleInstance? instance;
  if (Platform.isWindows) {
    // Another Nudge is running: give it our files and exit.
    instance = await SingleInstance.claim(await dataDir(), args);
    if (instance == null) exit(0);
  }
  await RustLib.init();
  await initWindow(hidden: args.contains(backgroundFlag));
  // Nudge runs before (and without) any screen: Windows starts it hidden at
  // login, and Android starts it at boot or keeps it after the window closes.
  // Either way it must be ready to receive.
  final starting = _start(instance, filesFromArgs(args));
  runApp(NudgeRoot(starting: starting));
}

Future<NudgeController> _start(SingleInstance? instance, List<String> files) async {
  await enableMulticast();
  final dir = await dataDir();
  final app = await NudgeApp.start(
    dataDir: dir,
    downloadDir: await downloadDir(),
    deviceName: await deviceName(),
    platform: platformTag,
  );
  final controller = NudgeController(app);
  TransferNotifier(controller);
  // Android: files shared to Nudge (share sheet, quick settings tile).
  unawaited(listenForShares(controller.offer, onStarting: controller.warmUp));
  // Windows: tray icon, Explorer menu, start at login, and files from other
  // launches.
  if (Platform.isWindows) {
    await Tray.start(controller, dataDir: dir);
    unawaited(registerSendToMenu());
    unawaited(syncStartWithWindows(dir));
    void offerPaths(List<String> paths) {
      unawaited(showWindow());
      controller.offer(paths.map(OutgoingFile.fromPath).toList());
    }

    instance?.listen(onFiles: offerPaths, onShow: () => unawaited(showWindow()));
    if (files.isNotEmpty) offerPaths(files);
  }
  return controller;
}

class NudgeRoot extends StatelessWidget {
  const NudgeRoot({super.key, required this.starting});

  final Future<NudgeController> starting;

  @override
  Widget build(BuildContext context) {
    return MaterialApp(
      title: 'Nudge',
      debugShowCheckedModeBanner: false,
      theme: _theme(Brightness.light),
      darkTheme: _theme(Brightness.dark),
      home: FutureBuilder<NudgeController>(
        future: starting,
        builder: (context, snapshot) {
          if (snapshot.hasError) return _StartError(error: '${snapshot.error}');
          final controller = snapshot.data;
          if (controller == null) {
            return const Scaffold(body: Center(child: CircularProgressIndicator()));
          }
          return HomeScreen(controller: controller);
        },
      ),
    );
  }
}

ThemeData _theme(Brightness brightness) {
  final scheme = ColorScheme.fromSeed(seedColor: const Color(0xFF3D5AFE), brightness: brightness);
  return ThemeData(
    colorScheme: scheme,
    scaffoldBackgroundColor: scheme.surface,
    appBarTheme: AppBarTheme(
      backgroundColor: scheme.surface,
      surfaceTintColor: Colors.transparent,
      titleTextStyle: TextStyle(fontSize: 22, fontWeight: FontWeight.w600, color: scheme.onSurface),
    ),
    cardTheme: CardThemeData(
      elevation: 0,
      color: scheme.surfaceContainerLow,
      shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(16)),
      clipBehavior: Clip.antiAlias,
    ),
    snackBarTheme: const SnackBarThemeData(behavior: SnackBarBehavior.floating),
  );
}

class _StartError extends StatelessWidget {
  const _StartError({required this.error});

  final String error;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return Scaffold(
      body: Center(
        child: Padding(
          padding: const EdgeInsets.all(32),
          child: Column(
            mainAxisSize: MainAxisSize.min,
            children: [
              Icon(Icons.error_outline_rounded, size: 48, color: theme.colorScheme.error),
              const SizedBox(height: 16),
              Text("Nudge couldn't start", style: theme.textTheme.titleLarge),
              const SizedBox(height: 8),
              SelectableText(error, textAlign: TextAlign.center),
            ],
          ),
        ),
      ),
    );
  }
}
