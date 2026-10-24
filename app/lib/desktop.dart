// Windows: tray icon, close-to-tray, and "received" toasts.
//
// Closing the window hides Nudge to the tray, where it keeps receiving.
// "Quit Nudge" in the tray menu really exits, as does closing a debug build.

import 'dart:async';
import 'dart:io';

import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';
import 'package:local_notifier/local_notifier.dart';
import 'package:open_filex/open_filex.dart';
import 'package:tray_manager/tray_manager.dart' as tray;
import 'package:window_manager/window_manager.dart';

import 'controller.dart';
import 'platform.dart';
import 'windows_shell.dart';

bool get _desktop => Platform.isWindows;

/// Whether the window is on screen (not hidden in the tray or minimized).
/// Animations stop while it isn't, so Nudge in the tray costs nothing.
final windowShown = ValueNotifier<bool>(true);

/// Before `runApp`: take over the window's close button. With [hidden]
/// (started at login), Nudge starts in the tray with no window.
///
/// The runner already made the window at its size and place, and shows it
/// with the first frame (windows/runner), so nothing here waits on it.
Future<void> initWindow({bool hidden = false}) async {
  if (!_desktop) return;
  windowShown.value = !hidden;
  await windowManager.ensureInitialized();
  windowManager.addListener(_Shown());
  // The title bar is made dark in the runner (win32_window.cpp): this
  // plugin's setBrightness only goes dark when Windows itself is dark.
  await Future.wait([
    windowManager.setMinimumSize(const Size(360, 480)),
    windowManager.setPreventClose(true),
  ]);
}

class _Shown with WindowListener {
  @override
  void onWindowEvent(String eventName) {
    switch (eventName) {
      case 'show' || 'restore':
        windowShown.value = true;
      case 'hide' || 'minimize':
        windowShown.value = false;
    }
  }
}

/// Toasts are set up once, off the startup path (it can create a Start
/// menu shortcut, which is slow).
Future<void>? _toastsReady;

Future<void> _toasts() => _toastsReady ??= localNotifier.setup(
  appName: 'Nudge',
  shortcutPolicy: ShortcutPolicy.requireCreate,
);

/// Get toasts ready ahead of the first one.
void prepareToasts() {
  if (_desktop) unawaited(_toasts().catchError((_) {}));
}

/// Bring the window back from the tray.
Future<void> showWindow() async {
  if (!_desktop) return;
  windowShown.value = true;
  await windowManager.show();
  await windowManager.focus();
}

/// "Received photo.jpg from Phone"; clicking opens the file.
/// [name] is what came: a file's name, or for several files sent together
/// "Trip" or "37 files" ([path] is then the folder they went in).
Future<void> showReceivedToast({
  required String path,
  required String from,
  required String name,
}) async {
  if (!_desktop) return;
  final toast = LocalNotification(
    title: 'Received $name',
    body: from.isEmpty ? 'Saved to Downloads' : 'From $from',
    actions: [LocalNotificationAction(text: 'Open')],
  );
  void open() => unawaited(OpenFilex.open(path));
  toast
    ..onClick = open
    ..onClickAction = (_) => open();
  try {
    await _toasts();
    await toast.show();
  } catch (_) {
    // Toasts are a nicety; the file is saved either way.
  }
}

/// Whether Nudge's window is in front of the user (so the in-app question
/// is enough).
Future<bool> windowInFront() async {
  if (!_desktop) return false;
  try {
    return await windowManager.isVisible() && await windowManager.isFocused();
  } catch (_) {
    return false;
  }
}

/// "Darren's S24 wants to send you 3 files", with Accept and Decline, for
/// when the window isn't in front. Returns a way to take it down.
Future<void Function()> showOfferToast({
  required String title,
  required String body,
  required void Function(bool accept) onAnswer,
}) async {
  if (!_desktop) return () {};
  final toast = LocalNotification(
    title: title,
    body: body,
    actions: [
      LocalNotificationAction(text: 'Accept'),
      LocalNotificationAction(text: 'Decline'),
    ],
  );
  toast.onClick = () => unawaited(showWindow());
  toast.onClickAction = (i) => onAnswer(i == 0);
  try {
    await _toasts();
    await toast.show();
  } catch (_) {
    // The in-app question is still there.
  }
  return () => unawaited(toast.close());
}

/// The tray icon and its menu, and hide-on-close.
class Tray with WindowListener {
  Tray._(this.controller, this._icon, this._dataDir);

  final NudgeController controller;
  final String _dataDir;

  // Kept for the life of the app: if it's garbage-collected, the icon goes.
  final tray.TrayIcon _icon;
  static Tray? _instance;

  static Future<void> start(NudgeController controller, {required String dataDir}) async {
    if (!_desktop || _instance != null) return;
    final icon = tray.TrayIcon.create();
    if (icon == null) return;
    final t = _instance = Tray._(controller, icon, dataDir);
    windowManager.addListener(t);
    final startsAtLogin = canStartWithWindows ? await startsWithWindows(dataDir) : null;
    icon
      ..icon = tray.ImageAsset.fromAsset('assets/tray.ico')
      ..setTooltip('Nudge')
      ..setContextMenu(t._menu(startsAtLogin))
      ..setContextMenuTrigger(tray.ContextMenuTrigger.rightClicked)
      ..addListener((event) {
        if (event is tray.TrayIconClickedEvent) unawaited(showWindow());
      })
      ..setVisible(true);
  }

  /// [startsAtLogin] is null when the option doesn't apply (debug builds).
  tray.Menu _menu(bool? startsAtLogin) {
    final menu = tray.Menu.create()!;
    tray.MenuItem add(
      String label,
      void Function(tray.MenuItem item) onClick, {
      tray.MenuItemType type = tray.MenuItemType.normal,
    }) {
      final item = tray.MenuItem.createWithLabelAndType(label, type)!;
      item.addListener((event) {
        if (event is tray.MenuItemClickedEvent) onClick(item);
      });
      menu.addItem(item);
      return item;
    }

    add('Open Nudge', (_) => unawaited(showWindow()));
    add('Send files…', (_) => unawaited(_pickAndSend()));
    add('Send a folder…', (_) => unawaited(_pickFolderAndSend()));
    menu.addSeparator();
    if (startsAtLogin != null) {
      var on = startsAtLogin;
      tray.MenuItemState state(bool on) =>
          on ? tray.MenuItemState.checked : tray.MenuItemState.unchecked;
      add('Start with Windows', (item) {
        on = !on;
        item.state = state(on);
        unawaited(setStartWithWindows(_dataDir, on));
      }, type: tray.MenuItemType.checkbox).state = state(
        on,
      );
    }
    add('Quit Nudge', (_) => unawaited(_quit()));
    return menu;
  }

  Future<void> _pickAndSend() async {
    controller.warmUp();
    await showWindow();
    controller.offer(await pickFiles(), fromOutside: false);
  }

  /// A folder goes whole, in its own folder on the other side.
  Future<void> _pickFolderAndSend() async {
    controller.warmUp();
    await showWindow();
    controller.offer(await pickFolder(), fromOutside: false);
  }

  Future<void> _quit() async {
    _icon.dispose();
    await windowManager.setPreventClose(false);
    await windowManager.destroy();
  }

  /// The close button hides to the tray; files keep arriving. Debug builds
  /// quit instead: a copy left in the tray locks the exe and breaks the
  /// next build.
  @override
  void onWindowClose() => unawaited(kDebugMode ? _quit() : windowManager.hide());
}
