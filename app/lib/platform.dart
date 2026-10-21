// Per-platform details: names, folders, and Android's multicast lock.

import 'dart:io';

import 'package:file_picker/file_picker.dart';
import 'package:flutter/services.dart';
import 'package:path_provider/path_provider.dart';

const _channel = MethodChannel('nudge/platform');

String get platformTag => Platform.isAndroid
    ? 'android'
    : Platform.isWindows
    ? 'windows'
    : Platform.operatingSystem;

bool get isPhone => Platform.isAndroid || Platform.isIOS;

/// The name other devices see.
Future<String> deviceName() async {
  if (Platform.isAndroid) {
    final name = await _channel.invokeMethod<String>('deviceName');
    if (name != null && name.isNotEmpty) return name;
  }
  return Platform.localHostname;
}

/// Where this device keeps its identity and paired devices.
/// On Windows this matches the `nudge` command-line tool (%APPDATA%\Nudge),
/// so the two share one identity.
Future<String> dataDir() async {
  if (Platform.isWindows) {
    final appData = Platform.environment['APPDATA'];
    if (appData != null) return '$appData\\Nudge';
  }
  return (await getApplicationSupportDirectory()).path;
}

/// Where received files go: Downloads on Windows, Download/Nudge on Android.
/// Falls back to the app's own folder if the shared one isn't writable
/// (Android 10 without storage permission).
Future<String> downloadDir() async {
  String? wanted;
  if (Platform.isAndroid) {
    wanted = await _channel.invokeMethod<String>('downloadsDir');
  } else {
    wanted = (await getDownloadsDirectory())?.path;
  }
  if (wanted != null && await _writable(wanted)) return wanted;
  final fallback = Platform.isAndroid
      ? await getExternalStorageDirectory()
      : await getApplicationDocumentsDirectory();
  return '${(fallback ?? await getApplicationSupportDirectory()).path}${Platform.pathSeparator}Received';
}

Future<bool> _writable(String dir) async {
  try {
    await Directory(dir).create(recursive: true);
    final probe = File('$dir${Platform.pathSeparator}.nudge-write-test');
    await probe.writeAsString('');
    await probe.delete();
    return true;
  } catch (_) {
    return false;
  }
}

/// Android filters multicast on Wi-Fi unless an app holds this lock; without
/// it, mDNS discovery hears nothing.
Future<void> enableMulticast() async {
  if (Platform.isAndroid) await _channel.invokeMethod('acquireMulticastLock');
}

/// Tell Android's media index about a received file so it shows up in
/// Files and Gallery.
Future<void> announceReceivedFile(String path) async {
  if (Platform.isAndroid) {
    await _channel.invokeMethod('scanFile', {'path': path});
  }
}

/// Let the user choose files to send. Returns local paths.
///
/// Android may hand back content URIs instead of paths; those are copied to
/// the cache first. That copy is slow for big videos and goes away when the
/// share-sheet work (Phase 3) streams straight from file descriptors.
Future<List<String>> pickFiles() async {
  final picked = await FilePicker.pickFiles(dialogTitle: 'Choose files to send');
  final paths = <String>[];
  for (final file in picked) {
    final path = file.path;
    if (path != null) {
      paths.add(path);
      continue;
    }
    final tmp = await getTemporaryDirectory();
    final dir = await Directory(
      '${tmp.path}${Platform.pathSeparator}outgoing${Platform.pathSeparator}'
      '${DateTime.now().microsecondsSinceEpoch}',
    ).create(recursive: true);
    final copy = '${dir.path}${Platform.pathSeparator}${file.name}';
    await file.xFile.saveTo(copy);
    paths.add(copy);
  }
  return paths;
}

/// Remove copies made by [pickFiles] in earlier sessions.
Future<void> clearPickedCopies() async {
  try {
    await FilePicker.clearTemporaryFiles();
    final outgoing = Directory(
      '${(await getTemporaryDirectory()).path}${Platform.pathSeparator}outgoing',
    );
    if (await outgoing.exists()) await outgoing.delete(recursive: true);
  } catch (_) {
    // Best effort.
  }
}
