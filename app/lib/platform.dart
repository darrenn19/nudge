// Per-platform details: names, folders, picking and sharing files, and
// Android's notifications and multicast lock.

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

/// A file waiting to be sent: a path, or (Android) a descriptor the system
/// opened for us. Descriptors are owned until sent or [closeUnsent].
class OutgoingFile {
  const OutgoingFile.path(String this.path, {required this.name, required this.size}) : fd = null;
  const OutgoingFile.fd(int this.fd, {required this.name, required this.size}) : path = null;

  final String? path;
  final int? fd;
  final String name;
  final int size;

  static OutgoingFile fromMap(Map<Object?, Object?> m) {
    final name = m['name'] as String? ?? 'file';
    final size = (m['size'] as num?)?.toInt() ?? 0;
    final fd = m['fd'] as int?;
    return fd != null
        ? OutgoingFile.fd(fd, name: name, size: size)
        : OutgoingFile.path(m['path'] as String, name: name, size: size);
  }

  static OutgoingFile fromPath(String path) {
    final file = File(path);
    return OutgoingFile.path(
      path,
      name: path.split(RegExp(r'[\/]')).last,
      size: file.existsSync() ? file.lengthSync() : 0,
    );
  }
}

/// Let the user choose files to send.
///
/// On Android these come back as open descriptors, read in place: no copy,
/// however big the file.
Future<List<OutgoingFile>> pickFiles() async {
  if (Platform.isAndroid) {
    final picked = await _activity.invokeListMethod<Map<Object?, Object?>>('pickFiles');
    return [for (final m in picked ?? const <Map<Object?, Object?>>[]) OutgoingFile.fromMap(m)];
  }
  final picked = await FilePicker.pickFiles(dialogTitle: 'Choose files to send');
  return [
    for (final file in picked)
      if (file.path != null) OutgoingFile.fromPath(file.path!),
  ];
}

/// Release files the user decided not to send.
Future<void> closeUnsent(List<OutgoingFile> files) async {
  final fds = [for (final f in files) ?f.fd];
  if (fds.isNotEmpty) await _channel.invokeMethod('closeFiles', {'fds': fds});
}

// Android: files shared with Nudge, and the notifications.

const _activity = MethodChannel('nudge/activity');

/// Calls [onFiles] with files shared to Nudge (share sheet, quick settings
/// tile), now and whenever more arrive; `byQr` is set for files shared with
/// "Share by QR code", meant for someone without Nudge. [onStarting] runs
/// the moment a share or pick begins, before the files are ready.
Future<void> listenForShares(
  void Function(List<OutgoingFile> files, {bool byQr}) onFiles, {
  void Function()? onStarting,
}) async {
  if (!Platform.isAndroid) return;
  Future<void> take() async {
    try {
      final shared = await _activity.invokeListMethod<Map<Object?, Object?>>('takeShared');
      if (shared == null) return;
      bool byQr(Map<Object?, Object?> m) => m['byQr'] == true;
      final normal = [
        for (final m in shared)
          if (!byQr(m)) OutgoingFile.fromMap(m),
      ];
      final qr = [
        for (final m in shared)
          if (byQr(m)) OutgoingFile.fromMap(m),
      ];
      if (normal.isNotEmpty) onFiles(normal);
      if (qr.isNotEmpty) onFiles(qr, byQr: true);
    } on MissingPluginException {
      // No window yet; files wait in the activity until it asks again.
    }
  }

  _activity.setMethodCallHandler((call) async {
    switch (call.method) {
      case 'shareStarting':
        onStarting?.call();
      case 'sharedAvailable':
        await take();
    }
  });
  await take();
}

/// Android: ask once to be left out of battery optimization, so the system
/// doesn't stop Nudge mid-transfer or while it waits for files. Does nothing
/// if already allowed or asked before.
Future<void> askToRunInBackground() async {
  if (!Platform.isAndroid) return;
  try {
    await _activity.invokeMethod('askToRunInBackground');
  } on MissingPluginException {
    // No window; ask another time.
  }
}

/// Go back to the app the user shared from. Transfers carry on.
Future<void> leaveApp() async {
  if (!Platform.isAndroid) return;
  try {
    await _activity.invokeMethod('moveToBack');
  } on MissingPluginException {
    // Window already gone.
  }
}

/// Show (or update) the ongoing transfer notification. [progress] is 0–100,
/// or -1 when unknown. Also keeps Android from stopping the app mid-transfer.
Future<void> showTransferProgress({
  required String title,
  required String text,
  required int progress,
}) async {
  if (!Platform.isAndroid) return;
  await _channel.invokeMethod('transfersActive', {
    'title': title,
    'text': text,
    'progress': progress,
  });
}

Future<void> hideTransferProgress() async {
  if (Platform.isAndroid) await _channel.invokeMethod('transfersIdle');
}

/// "Darren's PC wants to send you 3 files", with Accept and Decline, for
/// when Nudge isn't on screen.
Future<void> notifyOffer({required int id, required String title, required String text}) async {
  if (!Platform.isAndroid) return;
  await _channel.invokeMethod('notifyOffer', {'id': id, 'title': title, 'text': text});
}

Future<void> cancelOfferNotification(int id) async {
  if (Platform.isAndroid) await _channel.invokeMethod('cancelOffer', {'id': id});
}

/// Calls [onAnswer] when Accept or Decline is tapped on an offer
/// notification.
void listenForOfferAnswers(void Function(int id, bool accept) onAnswer) {
  if (!Platform.isAndroid) return;
  _channel.setMethodCallHandler((call) async {
    if (call.method != 'offerAnswer') return;
    final args = (call.arguments as Map).cast<String, Object?>();
    final id = (args['id'] as num?)?.toInt();
    if (id != null) onAnswer(id, args['accept'] == true);
  });
}

/// "Received photo.jpg from Laptop", with an Open action. Also adds the file
/// to Android's media index so it shows up in Files and Gallery.
/// Add a received file to Android's media index (Files, Gallery) without a
/// notification: for when Nudge is on screen and says so itself.
Future<void> indexReceived(String path) async {
  if (Platform.isAndroid) await _channel.invokeMethod('scanFile', {'path': path});
}

Future<void> notifyReceived({required String path, required String from}) async {
  if (Platform.isAndroid) {
    await _channel.invokeMethod('notifyReceived', {'path': path, 'from': from});
  }
}
