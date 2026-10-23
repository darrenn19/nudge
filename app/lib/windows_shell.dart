// Windows: "Send with Nudge" in Explorer's right-click menu, and starting
// with Windows.
//
// Both are registered per user (HKCU, no admin rights) on every start, so
// they always point at the exe that's running: in one `reg import`, a few
// seconds after startup. On Windows 11 the menu entry appears under "Show
// more options"; the top-level menu needs a packaged app.

import 'dart:convert';
import 'dart:io';

import 'package:flutter/foundation.dart';

const _menuKey = r'HKEY_CURRENT_USER\Software\Classes\*\shell\Nudge';

/// The Explorer menu entry and the login entry, written together.
Future<void> registerShell(String dataDir) async {
  if (!Platform.isWindows) return;
  final exe = Platform.resolvedExecutable;
  final lines = [
    'Windows Registry Editor Version 5.00',
    '',
    '[$_menuKey]',
    '@=${_regString('Send with Nudge')}',
    '"Icon"=${_regString('"$exe",0')}',
    // Any number of selected files (Explorer otherwise hides the verb past 15).
    '"MultiSelectModel"=${_regString('Player')}',
    '',
    '[$_menuKey\\command]',
    '@=${_regString('"$exe" --send "%1"')}',
    '',
    if (canStartWithWindows) ...[
      '[$_runKeyLong]',
      await startsWithWindows(dataDir) ? '"Nudge"=${_regString(_runCommand)}' : '"Nudge"=-',
      '',
    ],
  ];
  try {
    final file = File('$dataDir${Platform.pathSeparator}shell.reg');
    await file.writeAsBytes(_utf16('${lines.join('\r\n')}\r\n'), flush: true);
    await Process.run('reg', ['import', file.path]);
  } on Exception {
    // reg.exe missing or blocked: no menu entry, no start at login.
  }
}

/// A string value as a .reg file writes it.
String _regString(String s) => '"${s.replaceAll(r'\', r'\\').replaceAll('"', r'\"')}"';

/// UTF-16 with a byte order mark, which is what `reg import` reads.
Uint8List _utf16(String s) {
  final units = [0xFEFF, ...s.codeUnits];
  final bytes = Uint8List(units.length * 2);
  for (var i = 0; i < units.length; i++) {
    bytes[2 * i] = units[i] & 0xFF;
    bytes[2 * i + 1] = units[i] >> 8;
  }
  return bytes;
}

// Windows: start with Windows, hidden in the tray, so files arrive (as
// toasts) without the user opening Nudge.

/// Passed by the login entry: start in the tray with no window.
const backgroundFlag = '--background';

const _runKey = r'HKCU\Software\Microsoft\Windows\CurrentVersion\Run';
const _runKeyLong = r'HKEY_CURRENT_USER\Software\Microsoft\Windows\CurrentVersion\Run';

String get _runCommand => '"${Platform.resolvedExecutable}" $backgroundFlag';

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

/// Make the login entry match the choice (startup does this in
/// [registerShell], so the entry follows the exe if it moves).
Future<void> syncStartWithWindows(String dataDir) async {
  if (!canStartWithWindows) return;
  final args = await startsWithWindows(dataDir)
      ? ['add', _runKey, '/v', 'Nudge', '/d', _runCommand, '/f']
      : ['delete', _runKey, '/v', 'Nudge', '/f'];
  try {
    await Process.run('reg', args);
  } on ProcessException {
    // reg.exe missing or blocked; Nudge just won't start at login.
  }
}
