// Windows: one Nudge at a time.
//
// The runner (windows/runner/main.cpp) makes sure of it before Flutter even
// loads: a later launch hands its command line to the running Nudge and
// exits. Explorer's "Send with Nudge" starts one such launch per selected
// file; their files are gathered here so one selection arrives together.

import 'dart:async';

import 'package:flutter/services.dart';

const _sendFlag = '--send';

const _channel = MethodChannel('nudge/instance');

/// File paths passed on the command line (`--send <path>...`).
List<String> filesFromArgs(List<String> args) {
  final at = args.indexOf(_sendFlag);
  return at < 0 ? const [] : args.sublist(at + 1).where((a) => a.isNotEmpty).toList();
}

/// Calls [onFiles] with files from later launches (batched, since Explorer
/// starts one process per file), and [onShow] when a launch has no files.
Future<void> listenForLaunches({
  required void Function(List<String> paths) onFiles,
  required void Function() onShow,
}) async {
  final pending = <String>[];
  Timer? batch;
  void launched(Object? arguments) {
    final args = (arguments as List?)?.whereType<String>().toList() ?? const [];
    final files = filesFromArgs(args);
    if (files.isEmpty) {
      onShow();
      return;
    }
    pending.addAll(files);
    batch?.cancel();
    batch = Timer(const Duration(milliseconds: 400), () {
      onFiles(List.of(pending));
      pending.clear();
    });
  }

  _channel.setMethodCallHandler((call) async {
    if (call.method == 'launched') launched(call.arguments);
  });
  // Launches that came while this one was starting.
  final early = await _channel.invokeListMethod<Object?>('listen');
  early?.forEach(launched);
}
