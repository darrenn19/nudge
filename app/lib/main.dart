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
  await initWindow();
  runApp(NudgeRoot(instance: instance, files: filesFromArgs(args)));
}

class NudgeRoot extends StatefulWidget {
  const NudgeRoot({super.key, this.instance, this.files = const []});

  /// Windows: hands us files from later launches.
  final SingleInstance? instance;

  /// Files to send from the command line ("Send with Nudge").
  final List<String> files;

  @override
  State<NudgeRoot> createState() => _NudgeRootState();
}

class _NudgeRootState extends State<NudgeRoot> {
  late final Future<NudgeController> _starting = _start();

  Future<NudgeController> _start() async {
    await enableMulticast();
    final app = await NudgeApp.start(
      dataDir: await dataDir(),
      downloadDir: await downloadDir(),
      deviceName: await deviceName(),
      platform: platformTag,
    );
    final controller = NudgeController(app);
    TransferNotifier(controller);
    // Android: files shared to Nudge (share sheet, quick settings tile).
    unawaited(listenForShares(controller.offer));
    // Windows: tray icon, Explorer menu, and files from other launches.
    if (Platform.isWindows) {
      await Tray.start(controller);
      unawaited(registerSendToMenu());
      void offerPaths(List<String> paths) {
        unawaited(showWindow());
        controller.offer(paths.map(OutgoingFile.fromPath).toList());
      }

      widget.instance?.listen(onFiles: offerPaths, onShow: () => unawaited(showWindow()));
      if (widget.files.isNotEmpty) offerPaths(widget.files);
    }
    return controller;
  }

  @override
  Widget build(BuildContext context) {
    return MaterialApp(
      title: 'Nudge',
      debugShowCheckedModeBanner: false,
      theme: _theme(Brightness.light),
      darkTheme: _theme(Brightness.dark),
      home: FutureBuilder<NudgeController>(
        future: _starting,
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
