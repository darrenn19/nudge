// Windows: tray icon, close-to-tray, and "received" toasts.
//
// Closing the window hides Nudge to the tray, where it keeps receiving.
// "Quit Nudge" in the tray menu really exits.

import 'dart:async';
import 'dart:io';

import 'package:flutter/material.dart';
import 'package:local_notifier/local_notifier.dart';
import 'package:open_filex/open_filex.dart';
import 'package:tray_manager/tray_manager.dart' as tray;
import 'package:window_manager/window_manager.dart';

import 'controller.dart';
import 'platform.dart';

bool get _desktop => Platform.isWindows;

/// Before `runApp`: take over the window's close button.
Future<void> initWindow() async {
  if (!_desktop) return;
  await windowManager.ensureInitialized();
  await windowManager.waitUntilReadyToShow(
    const WindowOptions(
      title: 'Nudge',
      size: Size(480, 720),
      minimumSize: Size(360, 480),
      center: true,
    ),
    () async {
      await windowManager.show();
      await windowManager.focus();
    },
  );
  await windowManager.setPreventClose(true);
  await localNotifier.setup(appName: 'Nudge', shortcutPolicy: ShortcutPolicy.requireCreate);
}

/// Bring the window back from the tray.
Future<void> showWindow() async {
  if (!_desktop) return;
  await windowManager.show();
  await windowManager.focus();
}

/// "Received photo.jpg from Phone"; clicking opens the file.
Future<void> showReceivedToast({required String path, required String from}) async {
  if (!_desktop) return;
  final name = path.split(RegExp(r'[\\/]')).last;
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
    await toast.show();
  } catch (_) {
    // Toasts are a nicety; the file is saved either way.
  }
}

/// The tray icon and its menu, and hide-on-close.
class Tray with WindowListener {
  Tray._(this.controller, this._icon);

  final NudgeController controller;

  // Kept for the life of the app: if it's garbage-collected, the icon goes.
  final tray.TrayIcon _icon;
  static Tray? _instance;

  static Future<void> start(NudgeController controller) async {
    if (!_desktop || _instance != null) return;
    final icon = tray.TrayIcon.create();
    if (icon == null) return;
    final t = _instance = Tray._(controller, icon);
    windowManager.addListener(t);
    icon
      ..icon = tray.ImageAsset.fromAsset('assets/tray.ico')
      ..setTooltip('Nudge')
      ..setContextMenu(t._menu())
      ..setContextMenuTrigger(tray.ContextMenuTrigger.rightClicked)
      ..addListener((event) {
        if (event is tray.TrayIconClickedEvent) unawaited(showWindow());
      })
      ..setVisible(true);
  }

  tray.Menu _menu() {
    final menu = tray.Menu.create()!;
    void add(String label, void Function() onClick) {
      final item = tray.MenuItem.createWithLabelAndType(label, tray.MenuItemType.normal)!;
      item.addListener((event) {
        if (event is tray.MenuItemClickedEvent) onClick();
      });
      menu.addItem(item);
    }

    add('Open Nudge', () => unawaited(showWindow()));
    add('Send files…', () => unawaited(_pickAndSend()));
    menu.addSeparator();
    add('Quit Nudge', () => unawaited(_quit()));
    return menu;
  }

  Future<void> _pickAndSend() async {
    await showWindow();
    controller.offer(await pickFiles(), fromOutside: false);
  }

  Future<void> _quit() async {
    _icon.dispose();
    await windowManager.setPreventClose(false);
    await windowManager.destroy();
  }

  /// The close button hides to the tray; files keep arriving.
  @override
  void onWindowClose() => unawaited(windowManager.hide());
}
