// Windows: "Send with Nudge" in Explorer's right-click menu, and starting
// with Windows.
//
// Both are registered per user (HKCU, no admin rights) on every start, so
// they always point at the exe that's running. On Windows 11 the menu entry
// appears under "Show more options"; the top-level menu needs a packaged app.

import 'dart:convert';
import 'dart:io';

import 'package:flutter/foundation.dart';

const _key = r'HKCU\Software\Classes\*\shell\Nudge';

Future<void> registerSendToMenu() async {
  if (!Platform.isWindows) return;
  final exe = Platform.resolvedExecutable;
  final values = <List<String>>[
    [_key, '/ve', '/d', 'Send with Nudge'],
    [_key, '/v', 'Icon', '/d', '"$exe",0'],
    // Any number of selected files (Explorer otherwise hides the verb past 15).
    [_key, '/v', 'MultiSelectModel', '/d', 'Player'],
    ['$_key\\command', '/ve', '/d', '"$exe" --send "%1"'],
  ];
  for (final v in values) {
    try {
      await Process.run('reg', ['add', ...v, '/f']);
    } on ProcessException {
      return; // reg.exe missing or blocked; the menu just won't appear.
    }
  }
}

// Windows: start with Windows, hidden in the tray, so files arrive (as
// toasts) without the user opening Nudge.

/// Passed by the login entry: start in the tray with no window.
const backgroundFlag = '--background';

const _runKey = r'HKCU\Software\Microsoft\Windows\CurrentVersion\Run';

/// Debug builds never register: the entry would point at a debug exe that
/// quits when its window closes.
bool get canStartWithWindows => Platform.isWindows && !kDebugMode;

String _settingsPath(String dataDir) => '$dataDir${Platform.pathSeparator}windows.json';

/// The user's choice, kept beside the identity. On unless turned off.
Future<bool> startsWithWindows(String dataDir) async {
  try {
    final settings = jsonDecode(await File(_settingsPath(dataDir)).readAsString());
    return (settings as Map<String, Object?>)['startWithWindows'] != false;
  } catch (_) {
    return true;
  }
}

/// Save the choice and apply it.
Future<void> setStartWithWindows(String dataDir, bool on) async {
  try {
    await File(_settingsPath(dataDir)).writeAsString(jsonEncode({'startWithWindows': on}));
  } on FileSystemException {
    // Still applied below; it just won't stick if this file can't be written.
  }
  await syncStartWithWindows(dataDir);
}

/// Make the login entry match the choice. Run on every start, so the entry
/// follows the exe if it moves.
Future<void> syncStartWithWindows(String dataDir) async {
  if (!canStartWithWindows) return;
  final args = await startsWithWindows(dataDir)
      ? [
          'add',
          _runKey,
          '/v',
          'Nudge',
          '/d',
          '"${Platform.resolvedExecutable}" $backgroundFlag',
          '/f',
        ]
      : ['delete', _runKey, '/v', 'Nudge', '/f'];
  try {
    await Process.run('reg', args);
  } on ProcessException {
    // reg.exe missing or blocked; Nudge just won't start at login.
  }
}
