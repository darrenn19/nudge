// Per-platform details: names, folders, picking and sharing files, and
// Android's notifications and multicast lock.

import 'dart:io';

import 'package:file_picker/file_picker.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:path_provider/path_provider.dart';

const _channel = MethodChannel('nudge/platform');

/// What Android says through [_channel], by method. One handler serves all.
final _fromAndroid = <String, void Function(Object? arguments)>{};

void _onAndroid(String method, void Function(Object? arguments) handle) {
  _fromAndroid[method] = handle;
  _channel.setMethodCallHandler((call) async => _fromAndroid[call.method]?.call(call.arguments));
}

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
    final path = m['path'] as String?;
    // An empty folder from a picked folder ("Trip/empty/"): no file at all.
    if (fd == null && path == null) return OutgoingFile.fd(-1, name: name, size: 0);
    return fd != null
        ? OutgoingFile.fd(fd, name: name, size: size)
        : OutgoingFile.path(path!, name: name, size: size);
  }

  /// A file, or a folder (everything in it goes; its size is the total).
  static OutgoingFile fromPath(String path) {
    final file = File(path);
    final folder = Directory(path);
    final int size;
    if (file.existsSync()) {
      size = file.lengthSync();
    } else if (folder.existsSync()) {
      size = _folderSize(folder);
    } else {
      size = 0;
    }
    return OutgoingFile.path(path, name: path.split(RegExp(r'[\\/]')).last, size: size);
  }

  static int _folderSize(Directory folder) {
    var total = 0;
    try {
      for (final entry in folder.listSync(recursive: true, followLinks: false)) {
        if (entry is File) total += entry.lengthSync();
      }
    } on FileSystemException {
      // Unreadable parts aren't sent either.
    }
    return total;
  }
}

/// Let the user choose files to send.
///
/// On Android these come back as open descriptors, read in place: no copy,
/// however big the file.
/// Desktop: let the user choose a folder to send (everything in it goes).
///
/// Android hands back the folder's files, each named by its path in the
/// folder ("Trip/day 2/plan.jpg"); desktop hands back the folder itself.
/// Either way it arrives as a folder. Empty when nothing was picked.
Future<List<OutgoingFile>> pickFolder() async {
  if (Platform.isAndroid) {
    final picked = await _activity.invokeListMethod<Map<Object?, Object?>>('pickFolder');
    return [for (final m in picked ?? const <Map<Object?, Object?>>[]) OutgoingFile.fromMap(m)];
  }
  final path = await FilePicker.getDirectoryPath(dialogTitle: 'Choose a folder to send');
  return path == null ? const [] : [OutgoingFile.fromPath(path)];
}

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
  final fds = [
    for (final f in files)
      if (f.fd case final fd? when fd >= 0) fd,
  ];
  if (fds.isNotEmpty) await _channel.invokeMethod('closeFiles', {'fds': fds});
}

// Android: files shared with Nudge, and the notifications.

const _activity = MethodChannel('nudge/activity');

/// Calls [onFiles] with files shared to Nudge (share sheet, quick settings
/// tile), now and whenever more arrive; `byQr` is set for files shared with
/// "Share by QR code", meant for someone without Nudge; `to` is the device
/// picked in the share sheet's top row (Direct Share), if one was.
/// [onStarting] runs the moment a share or pick begins, before the files
/// are ready. [onText] gets shared or selected text.
Future<void> listenForShares(
  void Function(List<OutgoingFile> files, {bool byQr, String? to}) onFiles, {
  void Function()? onStarting,
  void Function(String text, {String? to})? onText,
}) async {
  if (!Platform.isAndroid) return;
  Future<void> take() async {
    try {
      final shared = await _activity.invokeListMethod<Map<Object?, Object?>>('takeShared');
      if (shared == null) return;
      bool byQr(Map<Object?, Object?> m) => m['byQr'] == true;
      // Grouped by the device they're for (null: ask).
      final normal = <String?, List<OutgoingFile>>{};
      for (final m in shared) {
        if (!byQr(m)) (normal[m['to'] as String?] ??= []).add(OutgoingFile.fromMap(m));
      }
      final qr = [
        for (final m in shared)
          if (byQr(m)) OutgoingFile.fromMap(m),
      ];
      normal.forEach((to, files) => onFiles(files, to: to));
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
      case 'sharedText':
        final args = (call.arguments as Map?)?.cast<String, Object?>() ?? const {};
        if (args['text'] case final String text) onText?.call(text, to: args['to'] as String?);
    }
  });
  await take();
}

/// Android: put each paired device in the share sheet's top row (Direct
/// Share), in the user's order. Sharing to one sends straight to it.
Future<void> publishShareTargets(List<({String id, String name})> devices) async {
  if (!Platform.isAndroid) return;
  await _channel.invokeMethod('setShareTargets', {
    'devices': [
      for (final d in devices) {'id': d.id, 'name': d.name},
    ],
  });
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

/// Android: whether the "Send latest" tile may send while the phone is
/// locked (off unless turned on; then the phone asks to unlock first).
Future<bool> latestTileWhileLocked() async {
  if (!Platform.isAndroid) return false;
  return await _channel.invokeMethod<bool>('latestTileWhileLocked') ?? false;
}

Future<void> setLatestTileWhileLocked(bool on) async {
  if (Platform.isAndroid) await _channel.invokeMethod('setLatestTileWhileLocked', {'on': on});
}

/// Android: what instant clipboard (phone to other devices, with Nudge in
/// the background) still needs. See ClipWatch.kt.
typedef InstantClipboard = ({bool overlay, bool logs, bool running});

Future<InstantClipboard?> instantClipboard() async {
  if (!Platform.isAndroid) return null;
  final m = await _channel.invokeMapMethod<String, bool>('instantClipboard') ?? const {};
  return (overlay: m['overlay'] ?? false, logs: m['logs'] ?? false, running: m['running'] ?? false);
}

/// Android: the "Appear on top" setting for Nudge.
Future<void> openOverlaySettings() async {
  if (Platform.isAndroid) await _channel.invokeMethod('openOverlaySettings');
}

/// Android: start watching the clipboard, once both permissions are there.
Future<void> startInstantClipboard() async {
  if (Platform.isAndroid) await _channel.invokeMethod('startInstantClipboard');
}

/// The one-time command that lets Nudge notice copies (run from a computer).
const instantClipboardCommand =
    'adb shell pm grant io.github.darrenn19.nudge android.permission.READ_LOGS';

/// Samsung: whether to show the one-time "keep Nudge awake" guide (Samsung
/// puts apps to sleep despite the battery exemption; "Never sleeping apps"
/// stops it).
Future<bool> needsSleepGuide() async {
  if (!Platform.isAndroid) return false;
  try {
    return await _activity.invokeMethod<bool>('needsSleepGuide') ?? false;
  } on MissingPluginException {
    return false;
  }
}

/// Open the settings page with "Never sleeping apps" (or, with [open]
/// false, just don't ask again).
Future<void> sleepGuideDone({required bool open}) async {
  if (!Platform.isAndroid) return;
  await _activity.invokeMethod(open ? 'openSleepSettings' : 'sleepGuideDismissed');
}

/// Android: ask once for what the hotspot (the direct link for big
/// transfers) needs: "nearby devices", or location on older versions.
Future<void> askForDirectLink() async {
  if (!Platform.isAndroid) return;
  try {
    await _activity.invokeMethod('askForDirectLink');
  } on MissingPluginException {
    // No window; ask another time.
  }
}

/// Android: keep the CPU awake and Wi-Fi out of power saving while files
/// move, on screen or not. Call again every few minutes to keep it.
Future<void> holdNetwork(bool on) async {
  if (Platform.isAndroid) await _channel.invokeMethod('holdNetwork', {'on': on});
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
/// or -1 when unknown; its Cancel stops [ids]. Also keeps Android from
/// stopping the app mid-transfer.
Future<void> showTransferProgress({
  required String title,
  required String text,
  required int progress,
  List<int> ids = const [],
}) async {
  if (!Platform.isAndroid) return;
  await _channel.invokeMethod('transfersActive', {
    'title': title,
    'text': text,
    'progress': progress,
    'ids': ids,
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
  _onAndroid('offerAnswer', (arguments) {
    final args = (arguments as Map).cast<String, Object?>();
    final id = (args['id'] as num?)?.toInt();
    if (id != null) onAnswer(id, args['accept'] == true);
  });
}

// Locking.

/// Android: the "Accept files while locked" choice changed; apply it now.
Future<void> whenLockedChanged() async {
  if (Platform.isAndroid) await _channel.invokeMethod('whenLockedChanged');
}

const _session = MethodChannel('nudge/session');

/// Calls [onChange] with whether the device is locked: now, and whenever
/// it changes. On Android the screen being off counts as locked.
Future<void> listenForLock(void Function(bool locked) onChange) async {
  if (Platform.isAndroid) {
    _onAndroid('locked', (locked) => onChange(locked == true));
    onChange(await _channel.invokeMethod<bool>('isLocked') ?? false);
  } else if (Platform.isWindows) {
    // Nudge starts (or is opened) unlocked.
    _session.setMethodCallHandler((call) async {
      if (call.method == 'locked') onChange(call.arguments == true);
    });
  }
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

// Clipboard.

/// Put clipboard text that came from another device on this one's
/// clipboard. Android does it natively: it works with no window open.
Future<void> setClipboardText(String text) async {
  if (Platform.isAndroid) {
    await _channel.invokeMethod('setClipboard', {'text': text});
  } else {
    await Clipboard.setData(ClipboardData(text: text));
  }
}

/// Android: calls [handle] each time the window gets focus, the first moment
/// Android lets it read the clipboard (on resume is too early).
void onWindowFocused(void Function() handle) {
  if (Platform.isAndroid) _onAndroid('windowFocused', (_) => handle());
}

/// Android: the clipboard's text if it changed since last asked (and wasn't
/// put there by Nudge). Android only lets an app read it while its window
/// has focus.
Future<String?> newClipboardText() async {
  if (!Platform.isAndroid) return null;
  try {
    return await _channel.invokeMethod<String>('clipboardIfNew');
  } on PlatformException {
    return null;
  }
}

const _clipboard = MethodChannel('nudge/clipboard');

/// Windows: calls [onCopy] with the text whenever text is copied, and
/// [onImage] with an image (as PNG) whenever one is, except what password
/// managers mark as private.
void listenForCopies(void Function(String text) onCopy, {void Function(Uint8List png)? onImage}) {
  if (!Platform.isWindows) return;
  _clipboard.setMethodCallHandler((call) async {
    switch (call.method) {
      case 'copied' when call.arguments is String:
        onCopy(call.arguments as String);
      case 'copiedImage' when call.arguments is Uint8List:
        onImage?.call(call.arguments as Uint8List);
    }
  });
}

/// Windows: put an image (PNG, JPEG...) from another device on the
/// clipboard. Android does it natively (ClipSync.setImage).
Future<bool> setClipboardImage(Uint8List image) async {
  if (!Platform.isWindows) return false;
  return await _clipboard.invokeMethod<bool>('setImage', image) ?? false;
}

/// A cable from this phone to a computer, as Android sees it.
class UsbCable {
  const UsbCable({
    this.data = false,
    this.tethering = false,
    this.chargeOnly = false,
    this.asked = false,
    this.developerOptions = false,
  });

  /// A computer at the other end, talking USB (not just power).
  final bool data;

  /// The phone shares a network over the cable (USB tethering), so files go
  /// over it. Any cable that carries data works: USB-C, micro-USB, USB-A.
  final bool tethering;

  /// Power from a computer but no data: a cable that only charges.
  final bool chargeOnly;

  /// The person was shown how to use the cable once; not asked again.
  final bool asked;

  /// Developer options is on (needed to make tethering come on by itself).
  final bool developerOptions;

  /// Plugged in, files still go over Wi-Fi, and it's worth saying so: only
  /// until the person has been shown once.
  bool get couldUse => data && !tethering && !asked;
}

/// The cable right now; Android only (always none elsewhere).
final usbCable = ValueNotifier<UsbCable>(const UsbCable());

/// Keep [usbCable] up to date.
Future<void> watchUsbCable() async {
  if (!Platform.isAndroid) return;
  UsbCable read(Object? state) {
    final m = (state as Map?) ?? const {};
    return UsbCable(
      data: m['data'] == true,
      tethering: m['tethering'] == true,
      chargeOnly: m['chargeOnly'] == true,
      asked: m['asked'] == true,
      developerOptions: m['developerOptions'] == true,
    );
  }

  _onAndroid('usbCable', (state) => usbCable.value = read(state));
  _readUsbCable = () async => usbCable.value = read(await _channel.invokeMethod<Object?>('usbCable'));
  await _readUsbCable!();
}

Future<void> Function()? _readUsbCable;

/// Android's screen with the USB tethering switch (for this time only).
Future<void> openTetherSettings() async {
  if (!Platform.isAndroid) return;
  await _channel.invokeMethod('openTetherSettings');
  await _readUsbCable?.call();
}

/// Where to make USB tethering come on by itself whenever the phone is
/// plugged into a computer (Developer options → Default USB configuration).
Future<void> openAutoTethering() async {
  if (!Platform.isAndroid) return;
  await _channel.invokeMethod('openAutoTethering');
  await _readUsbCable?.call();
}
