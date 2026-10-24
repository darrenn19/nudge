// Windows: one Nudge at a time.
//
// The runner (windows/runner/main.cpp) makes sure of it before Flutter even
// loads: a later launch hands its command line to the running Nudge and
// exits. Explorer's "Send with Nudge" starts one such launch per selected
// file; their files are gathered here so one selection arrives together.

import 'dart:async';
import 'dart:io';

import 'package:flutter/services.dart';

import 'windows_shell.dart';

const _sendFlag = '--send';

const _channel = MethodChannel('nudge/instance');

/// File paths passed on the command line (`--send <path>...`).
List<String> filesFromArgs(List<String> args) {
  final at = args.indexOf(_sendFlag);
  return at < 0 ? const [] : args.sublist(at + 1).where((a) => a.isNotEmpty).toList();
}

/// Text shared from the Share panel (`--clip <file>`): the file's text. The
/// file was made for this, so it's deleted.
String? textFromArgs(List<String> args) {
  final at = args.indexOf('--clip');
  if (at < 0 || at + 1 >= args.length) return null;
  final file = File(args[at + 1]);
  try {
    return file.readAsStringSync();
  } catch (_) {
    return null;
  } finally {
    try {
      file.deleteSync();
    } catch (_) {}
  }
}

/// Calls [onFiles] with files from later launches (batched, since Explorer
/// starts one process per file), [onText] with shared text, and [onShow]
/// when a launch brings neither.
Future<void> listenForLaunches({
  required void Function(List<String> paths) onFiles,
  required void Function(String text) onText,
  required void Function() onShow,
}) async {
  final pending = <String>[];
  Timer? batch;
  void launched(Object? arguments) {
    final args = (arguments as List?)?.whereType<String>().toList() ?? const [];
    if (textFromArgs(args) case final text?) {
      onText(text);
      return;
    }
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

/// Put Nudge in Explorer's "Send to" menu, pointing at this exe. Every
/// selected file comes in one launch.
Future<void> installSendTo() async {
  // A package's writes to the user's folders are kept private to it, so
  // the shortcut wouldn't show; packaged, the right-click menu is there.
  if (await isPackaged()) return;
  try {
    await _channel.invokeMethod<bool>('installSendTo');
  } on PlatformException {
    // Only a shortcut; the right-click menu entry still works.
  }
}
