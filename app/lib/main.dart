import 'dart:io';

import 'package:flutter/material.dart';

import 'controller.dart';
import 'platform.dart';
import 'src/rust/api/nudge.dart';
import 'src/rust/frb_generated.dart';
import 'ui/home.dart';

Future<void> main() async {
  WidgetsFlutterBinding.ensureInitialized();
  await RustLib.init();
  runApp(const NudgeRoot());
}

class NudgeRoot extends StatefulWidget {
  const NudgeRoot({super.key});

  @override
  State<NudgeRoot> createState() => _NudgeRootState();
}

class _NudgeRootState extends State<NudgeRoot> {
  late final Future<NudgeController> _starting = _start();

  Future<NudgeController> _start() async {
    await enableMulticast();
    if (Platform.isAndroid) await clearPickedCopies();
    final app = await NudgeApp.start(
      dataDir: await dataDir(),
      downloadDir: await downloadDir(),
      deviceName: await deviceName(),
      platform: platformTag,
    );
    return NudgeController(app);
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
