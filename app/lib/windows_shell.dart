// Windows: "Send with Nudge" in Explorer's right-click menu.
//
// Registered per user (HKCU, no admin rights) on every start, so it always
// points at the exe that's running. On Windows 11 it appears under
// "Show more options"; the top-level menu needs a packaged app.

import 'dart:io';

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
