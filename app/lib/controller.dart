// App state: the device list and transfers, kept current from core events.

import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';

import 'platform.dart';
import 'src/rust/api/nudge.dart';
import 'wifi.dart';

enum TransferState { running, done, failed }

extension DeviceKind on DeviceInfo {
  /// Someone using the web page (see `NudgeApp.startWebShare`), not Nudge.
  bool get isBrowser => platform == 'browser';
}

class Transfer {
  Transfer({
    required this.id,
    required this.deviceId,
    required this.incoming,
    required this.fileName,
    required this.size,
    this.peerName,
    this.toBrowser = false,
  });

  final int id;
  final String deviceId;
  final bool incoming;
  final String fileName;
  final int size;

  /// The other device's name when the transfer started; a browser tab may
  /// be gone by the time it's shown.
  final String? peerName;

  /// Sent to a web page. Pages still download only after accepting.
  final bool toBrowser;

  TransferState state = TransferState.running;
  int bytes = 0;
  double bytesPerSec = 0;
  String? path;
  String? error;
  Duration? took;

  /// When it finished (done or failed), for brief "sent" / "failed" effects.
  DateTime? finishedAt;

  /// Outgoing: the receiver said yes.
  bool accepted = false;

  /// Outgoing: the receiver is asking its user. The bytes go meanwhile
  /// (a browser tab waits for its answer first).
  bool pending = false;

  /// Every byte is there, held until the receiver's user answers.
  bool delivered = false;

  /// The network it's going over: "usb", "wired", "hotspot", "wifi".
  String? route;

  /// Outgoing, all there, waiting for the receiver's answer. Nothing is
  /// moving, so it isn't shown as busy.
  bool get awaitingAnswer =>
      !incoming &&
      pending &&
      !accepted &&
      state == TransferState.running &&
      (delivered || toBrowser);

  /// Incoming, all here, waiting for this device's user to accept.
  bool get waitingForYou => incoming && delivered && state == TransferState.running;

  /// Something is actually happening: bytes moving or about to.
  bool get active => state == TransferState.running && !awaitingAnswer && !waitingForYou;

  /// It can be stopped: on its way, or (to a web page) offered and not yet
  /// accepted. Files waiting for an answer are settled by the answer.
  bool get cancellable => active || (toBrowser && awaitingAnswer);

  /// Stopped by this side's user ("cancelled") or the other's.
  bool get cancelled => error == 'cancelled' || error == 'they cancelled';

  /// How it's travelling, for people: "USB cable", "direct Wi-Fi"...
  String? get via => switch (route) {
    'usb' => 'USB cable',
    'wired' => 'cable',
    'hotspot' => 'direct Wi-Fi',
    'wifi' => 'Wi-Fi',
    _ => null,
  };

  /// What happened, in a word or two, from this side.
  String get outcome {
    if (state == TransferState.failed) {
      return switch (error) {
        'they declined' || 'you declined' => 'Declined',
        'no answer' => 'No answer',
        'cancelled' => 'Cancelled',
        'they cancelled' => 'They cancelled',
        'sharing was stopped' => 'Sharing stopped',
        'the browser stopped downloading' => 'Download stopped',
        _ => error ?? 'Failed',
      };
    }
    if (incoming) {
      if (waitingForYou) return 'Waiting for you';
      return state == TransferState.done ? 'Received' : 'Receiving';
    }
    if (awaitingAnswer) return 'Sent · waiting';
    if (state == TransferState.done) return pending ? 'Accepted · delivered' : 'Delivered';
    return accepted && pending ? 'Accepted · sending' : 'Sending';
  }

  double get fraction => size == 0 ? 1 : (bytes / size).clamp(0, 1);

  /// Time left at the recent speed, or null while there's no speed yet.
  Duration? get remaining {
    if (bytesPerSec <= 0) return null;
    final secs = (size - bytes) / bytesPerSec;
    return Duration(milliseconds: (secs * 1000).round());
  }
}

/// Files waiting for the user to pick a device. [fromOutside] is true when
/// they came from another app, which Nudge then returns to once sending starts.
/// [byQr] means they're for someone without Nudge: show the QR code.
class FilesToSend {
  const FilesToSend(this.files, {required this.fromOutside, this.byQr = false, this.to});

  final List<OutgoingFile> files;
  final bool fromOutside;
  final bool byQr;

  /// The device chosen already (a Direct Share target): no need to ask.
  final String? to;
}

/// A paired device asking to send files; answer with
/// [NudgeController.answerOffer].
class IncomingOffer {
  const IncomingOffer({
    required this.id,
    required this.deviceId,
    required this.deviceName,
    required this.fileName,
    required this.count,
    required this.size,
  });

  final int id;
  final String deviceId;
  final String deviceName;
  final String fileName;
  final int count;
  final int size;

  /// "photo.jpg" or "3 files".
  String get what => count == 1 ? fileName : '$count files';
}

/// A nearby device asking to pair; answer with [NudgeController.answerPair].
class PairAsk {
  const PairAsk({
    required this.id,
    required this.deviceId,
    required this.deviceName,
    required this.code,
  });

  final int id;
  final String deviceId;
  final String deviceName;
  final String code;
}

/// What happens to files sent while this device is locked.
enum WhenLocked {
  /// As when unlocked: devices set to "always accept" send without a
  /// question, others are asked (only paired devices can send at all).
  paired('Paired devices', 'usual'),

  /// Ask about every send, even from devices set to "always accept".
  ask('Ask me', 'ask'),

  /// Decline every send until unlocked.
  never('Never', 'refuse');

  const WhenLocked(this.label, this._mode);

  final String label;
  final String _mode;
}

/// What the sender hears back: the receiver's answer.
enum Answer { accepted, declined, noAnswer }

class NudgeController extends ChangeNotifier {
  NudgeController(this.app, {String? dataDir})
    : _orderFile = dataDir == null ? null : File('$dataDir${Platform.pathSeparator}order.json'),
      _lockedFile = dataDir == null
          ? null
          : File('$dataDir${Platform.pathSeparator}when_locked.txt'),
      _targetFile = dataDir == null
          ? null
          : File('$dataDir${Platform.pathSeparator}send_target.json') {
    _subscription = app.events().listen(_onEvent);
    devices = app.devices();
    webShareUrl = app.webShareUrl();
    _loadOrder();
    _loadTarget();
    _loadWhenLocked();
    // Android's background service applies it as the phone locks.
    if (!Platform.isAndroid) unawaited(listenForLock(_setLocked));
    unawaited(watchUsbCable());
  }

  // Files while locked. The choice is kept beside the identity; the core is
  // told what applies right now each time the lock or the choice changes.
  final File? _lockedFile;
  WhenLocked whenLocked = WhenLocked.paired;
  bool _locked = false;

  void _loadWhenLocked() {
    try {
      final saved = _lockedFile?.readAsStringSync().trim();
      whenLocked = WhenLocked.values.firstWhere((w) => w.name == saved);
    } catch (_) {
      whenLocked = WhenLocked.paired;
    }
    _applyReceiving();
  }

  void setWhenLocked(WhenLocked choice) {
    whenLocked = choice;
    try {
      _lockedFile?.writeAsStringSync(choice.name);
    } catch (_) {
      // Applies for now; it just won't stick.
    }
    _applyReceiving();
    unawaited(whenLockedChanged());
    notifyListeners();
  }

  void _setLocked(bool locked) {
    _locked = locked;
    // Android's service tells the core itself (LockState).
    if (!Platform.isAndroid) app.setScreenLocked(locked: locked);
    _applyReceiving();
  }

  void _applyReceiving() {
    if (Platform.isAndroid) return; // see listenForLock above
    app.setReceiving(mode: _locked ? whenLocked._mode : WhenLocked.paired._mode);
  }

  final NudgeApp app;

  /// Questions waiting for the user, oldest first.
  final List<IncomingOffer> offers = [];

  /// Pairing requests waiting for the user, oldest first.
  final List<PairAsk> pairAsks = [];

  void answerPair(PairAsk ask, {required bool accept}) {
    app.answerPair(id: ask.id, accept: accept);
    _closeOffer(ask.id);
    notifyListeners();
  }

  /// The code both screens show while pairing with [device].
  String pairCode(DeviceInfo device) => app.pairCode(deviceId: device.id);

  /// Pair with a nearby device without a QR code; finishes when its user
  /// answers. Throws if they decline or don't answer.
  Future<DeviceInfo> pairNearby(DeviceInfo device) async {
    final paired = await app.pairNearby(deviceId: device.id);
    devices = app.devices();
    notifyListeners();
    return paired;
  }

  final _answers = StreamController<(DeviceInfo?, String, Answer)>.broadcast();

  /// Answers to this device's sends: (device, its name, answer). Once per
  /// send, not per file.
  Stream<(DeviceInfo?, String, Answer)> get answers => _answers.stream;

  final _offerChanges = StreamController<void>.broadcast();

  /// Fires whenever [offers] changes.
  Stream<void> get offerChanges => _offerChanges.stream;

  void answerOffer(IncomingOffer offer, {required bool accept, bool always = false}) {
    app.answerOffer(id: offer.id, accept: accept, always: always);
    _closeOffer(offer.id);
    if (always) devices = app.devices();
    notifyListeners();
  }

  void setAutoAccept(DeviceInfo device, bool on) {
    try {
      app.setAutoAccept(deviceId: device.id, on_: on);
    } finally {
      // Whatever happened, show what's actually set.
      devices = app.devices();
      notifyListeners();
    }
  }

  void _closeOffer(int id) {
    final before = offers.length + pairAsks.length;
    offers.removeWhere((o) => o.id == id);
    pairAsks.removeWhere((a) => a.id == id);
    if (offers.length + pairAsks.length != before) _offerChanges.add(null);
  }

  // The order the user put their devices in (device IDs), kept beside the
  // identity. Devices not in it follow, in the core's order.
  final File? _orderFile;
  List<String> order = [];

  void _loadOrder() {
    try {
      final saved = jsonDecode(_orderFile?.readAsStringSync() ?? '[]');
      order = [for (final id in saved as List) id as String];
    } catch (_) {
      order = [];
    }
  }

  void setOrder(List<String> ids) {
    order = List.of(ids);
    try {
      _orderFile?.writeAsStringSync(jsonEncode(order));
    } catch (_) {
      // Only an arrangement; it just won't stick.
    }
    notifyListeners();
  }

  /// [list] in the user's order.
  List<DeviceInfo> ordered(List<DeviceInfo> list) {
    int rank(DeviceInfo d) {
      final i = order.indexOf(d.id);
      return i < 0 ? order.length : i;
    }

    final indexed = list.indexed.toList()
      ..sort((a, b) {
        final byRank = rank(a.$2).compareTo(rank(b.$2));
        return byRank != 0 ? byRank : a.$1.compareTo(b.$1);
      });
    return [for (final (_, d) in indexed) d];
  }

  // Where "Send latest" goes when more than one device is around: the
  // default device if the user picked one, else the current device (the
  // one last swiped to). Kept beside the identity; Android's tile reads
  // the same file (Targets.kt).
  final File? _targetFile;
  String? _currentId;
  String? _defaultId;

  void _loadTarget() {
    try {
      final saved = jsonDecode(_targetFile?.readAsStringSync() ?? '{}') as Map;
      _currentId = saved['current'] as String?;
      _defaultId = saved['default'] as String?;
    } catch (_) {
      // Nothing chosen yet.
    }
  }

  void _saveTarget() {
    try {
      _targetFile?.writeAsStringSync(jsonEncode({'current': _currentId, 'default': _defaultId}));
    } catch (_) {
      // Applies for now; it just won't stick.
    }
  }

  DeviceInfo? _pairedById(String? id) {
    for (final d in devices) {
      if (d.id == id && d.paired) return d;
    }
    return null;
  }

  /// The device the user set as default, if it's still paired.
  DeviceInfo? get defaultDevice => _pairedById(_defaultId);

  /// Where "Send latest" goes: the default device, else the current one.
  DeviceInfo? get sendTarget => defaultDevice ?? _pairedById(_currentId);

  /// The user swiped to [device]: it's the current device now.
  void setCurrent(DeviceInfo device) {
    if (!device.paired || device.id == _currentId) return;
    _currentId = device.id;
    _saveTarget();
    notifyListeners();
  }

  /// Always use [device] (null: go back to the current device).
  void setDefault(DeviceInfo? device) {
    _defaultId = device?.id;
    _saveTarget();
    notifyListeners();
  }

  /// The link people without Nudge open, while sharing with browsers.
  String? webShareUrl;
  late final StreamSubscription<AppEvent> _subscription;

  List<DeviceInfo> devices = const [];

  /// Newest first.
  final List<Transfer> transfers = [];

  final _pairing = StreamController<AppEvent>.broadcast();

  /// `Paired` and `PairingClosed` events, for the pairing screen.
  Stream<AppEvent> get pairingEvents => _pairing.stream;

  String get deviceName => app.deviceName();

  List<DeviceInfo> get pairedDevices => ordered(devices.where((d) => d.paired).toList());

  List<DeviceInfo> get nearbyDevices => devices.where((d) => !d.paired && !d.isBrowser).toList();

  /// Open web pages: people without Nudge.
  List<DeviceInfo> get browserDevices => devices.where((d) => d.isBrowser).toList();

  /// Start sharing with browsers (or keep sharing): returns the link.
  Future<String> startWebShare() async {
    final url = await app.startWebShare();
    webShareUrl = url;
    notifyListeners();
    return url;
  }

  void stopWebShare() {
    app.stopWebShare();
    webShareUrl = null;
    devices = app.devices();
    notifyListeners();
  }

  DeviceInfo? deviceById(String id) {
    for (final d in devices) {
      if (d.id == id) return d;
    }
    return null;
  }

  void refresh() {
    app.refresh();
    devices = app.devices();
    notifyListeners();
  }

  void send(DeviceInfo device, List<OutgoingFile> files) {
    final paths = [for (final f in files) ?f.path];
    final open = [
      for (final f in files)
        if (f.fd case final fd?) OpenFile(fd: fd, name: f.name),
    ];
    if (paths.isNotEmpty) app.sendFiles(deviceId: device.id, paths: paths);
    if (open.isNotEmpty) app.sendOpenFiles(deviceId: device.id, files: open);
  }

  // Offers made before the home screen is listening wait here.
  final _unclaimed = <FilesToSend>[];
  late final StreamController<FilesToSend> _toSend = StreamController.broadcast(
    onListen: () => scheduleMicrotask(() {
      _unclaimed.forEach(_toSend.add);
      _unclaimed.clear();
    }),
  );

  /// Files handed to Nudge from outside (share sheet, tile, Explorer, a drop)
  /// that need a device chosen. The home screen listens.
  Stream<FilesToSend> get filesToSend => _toSend.stream;

  /// A send is likely soon: open connections now, to [device] or to every
  /// nearby paired device, so it starts without a handshake. Cheap to call;
  /// unused connections close on their own.
  void warmUp([DeviceInfo? device]) {
    // Includes devices that went quiet: connecting early also wakes a phone
    // with its screen off before the files go.
    final targets = device != null ? [device] : pairedDevices.where((d) => d.reachable);
    for (final d in targets) {
      try {
        app.warm(deviceId: d.id);
      } catch (_) {
        // Only ever a head start; the send connects anyway.
      }
    }
  }

  void offer(List<OutgoingFile> files, {bool fromOutside = true, bool byQr = false, String? to}) {
    if (files.isEmpty) return;
    if (!byQr) warmUp(to == null ? null : deviceById(to));
    final request = FilesToSend(files, fromOutside: fromOutside, byQr: byQr, to: to);
    _toSend.hasListener ? _toSend.add(request) : _unclaimed.add(request);
  }

  void unpair(DeviceInfo device) {
    app.unpair(deviceId: device.id);
  }

  /// Take one finished transfer off the list.
  /// Stop [t], and the rest of the send it's part of.
  void cancel(Transfer t) {
    if (t.cancellable) app.cancelTransfer(id: t.id);
  }

  /// Stop everything going to or coming from [device].
  void cancelWith(DeviceInfo device) {
    for (final t in transfers) {
      if (t.deviceId == device.id && t.cancellable) app.cancelTransfer(id: t.id);
    }
  }

  void removeTransfer(Transfer t) {
    if (t.state == TransferState.running) return;
    transfers.remove(t);
    notifyListeners();
  }

  void clearFinished() {
    transfers.removeWhere((t) => t.state != TransferState.running);
    notifyListeners();
  }

  Transfer? _transfer(int id) {
    for (final t in transfers) {
      if (t.id == id) return t;
    }
    return null;
  }

  void _onEvent(AppEvent event) {
    switch (event) {
      case AppEvent_DevicesChanged():
        devices = app.devices();
      case AppEvent_Paired():
        devices = app.devices();
        _pairing.add(event);
      case AppEvent_PairingClosed():
        _pairing.add(event);
        return;
      case AppEvent_TransferStarted(
        :final id,
        :final deviceId,
        :final incoming,
        :final fileName,
        :final size,
      ):
        transfers.insert(
          0,
          Transfer(
            id: id,
            deviceId: deviceId,
            incoming: incoming,
            fileName: fileName,
            size: size,
            peerName: deviceById(deviceId)?.name,
            toBrowser: deviceById(deviceId)?.isBrowser ?? false,
          ),
        );
        if (transfers.length > 50) transfers.removeLast();
      case AppEvent_TransferProgress(:final id, :final bytes, :final bytesPerSec):
        final t = _transfer(id);
        if (t == null) return;
        t.bytes = bytes;
        // Smooth the speed a little so the ETA doesn't jump around.
        t.bytesPerSec = t.bytesPerSec == 0 ? bytesPerSec : t.bytesPerSec * 0.6 + bytesPerSec * 0.4;
      case AppEvent_TransferDone(:final id, :final path, :final bytes, :final seconds):
        final t = _transfer(id);
        if (t == null) return;
        t
          ..state = TransferState.done
          ..finishedAt = DateTime.now()
          ..bytes = t.size
          ..path = path
          ..took = Duration(microseconds: (seconds * 1e6).round());
        // Speed over this connection's bytes (a resumed file sends fewer).
        if (seconds > 0) t.bytesPerSec = bytes / seconds;
      case AppEvent_WebShareStopped():
        webShareUrl = null;
        devices = app.devices();
      case AppEvent_OfferReceived(
        :final id,
        :final deviceId,
        :final fileName,
        :final count,
        :final size,
      ):
        offers.add(
          IncomingOffer(
            id: id,
            deviceId: deviceId,
            deviceName: deviceById(deviceId)?.name ?? 'A paired device',
            fileName: fileName,
            count: count,
            size: size,
          ),
        );
        _offerChanges.add(null);
      case AppEvent_OfferClosed(:final id):
        _closeOffer(id);
      case AppEvent_TransferPending(:final id):
        final t = _transfer(id);
        if (t == null) return;
        t.pending = true;
      case AppEvent_PairAsked(:final id, :final deviceId, :final deviceName, :final code):
        pairAsks.add(PairAsk(id: id, deviceId: deviceId, deviceName: deviceName, code: code));
        _offerChanges.add(null);
      case AppEvent_TransferDelivered(:final id):
        final t = _transfer(id);
        if (t == null) return;
        t
          ..delivered = true
          ..bytes = t.size;
      case AppEvent_TransferRoute(:final id, :final route):
        final t = _transfer(id);
        if (t == null) return;
        t.route = route;
      case AppEvent_ClipboardReceived(:final deviceId, :final text):
        _clipboardArrived(deviceId, text);
        return;
      case AppEvent_ClipboardImageReceived(:final deviceId, :final path):
        unawaited(_clipboardImageArrived(deviceId, path));
        return;
      // Android's background service runs the hotspot.
      case AppEvent_HotspotWanted(:final id):
        if (!Platform.isAndroid) unawaited(_startHotspot(id));
        return;
      case AppEvent_HotspotDone():
        if (!Platform.isAndroid) unawaited(stopHotspot());
        return;
      case AppEvent_JoinWanted(:final id, :final ssid, :final passphrase, :final wpa3):
        unawaited(_joinHotspot(id, ssid, passphrase, wpa3));
        return;
      case AppEvent_LeaveWanted():
        unawaited(leaveHotspot());
        return;
      // Screen mirroring: on Android the background service asks and shows
      // "being shared" (a notification, even with the window open); the
      // Windows core declines requests to share this computer's screen.
      case AppEvent_MirrorAsked():
      case AppEvent_MirrorStarted():
      case AppEvent_MirrorEnded():
        return;
      case AppEvent_TransferAccepted(:final id):
        final t = _transfer(id);
        if (t == null) return;
        t.accepted = true;
        _tellAnswer(t, Answer.accepted);
      case AppEvent_TransferFailed(:final id, :final error):
        final t = _transfer(id);
        if (t == null) return;
        t
          ..state = TransferState.failed
          ..finishedAt = DateTime.now()
          ..error = error;
        if (!t.incoming && !t.accepted) {
          if (error == 'they declined') _tellAnswer(t, Answer.declined);
          if (error == 'no answer') _tellAnswer(t, Answer.noAnswer);
        }
    }
    notifyListeners();
  }

  // Faster paths. USB cables and Ethernet are used by themselves; the
  // phone's hotspot needs each app to do its part.

  /// Big transfers may move to the phone's hotspot (the user can say no).
  bool get directLink => app.directLink();

  void setDirectLink(bool on) {
    app.setDirectLink(on_: on);
    notifyListeners();
  }

  Future<void> _startHotspot(int id) async {
    final hotspot = await startHotspot();
    app.hotspotReady(
      id: id,
      ssid: hotspot?.ssid,
      passphrase: hotspot?.passphrase ?? '',
      wpa3: hotspot?.wpa3 ?? false,
    );
  }

  Future<void> _joinHotspot(int id, String ssid, String passphrase, bool wpa3) async {
    final ok = await joinHotspot(ssid: ssid, passphrase: passphrase, wpa3: wpa3);
    app.joined(id: id, ok: ok);
  }

  // Clipboard: text copied on one device lands on the others' clipboards,
  // with no question. Each paired device can be left out.

  final _clips = StreamController<(String, String)>.broadcast();

  /// Clipboard text (or an image: empty text) that arrived: (from device
  /// name, text).
  Stream<(String, String)> get clipsReceived => _clips.stream;

  /// The text on the clipboard as last sent or received, so text that
  /// arrived isn't sent straight back.
  String? _lastClip;

  void setClipboardSync(DeviceInfo device, bool on) {
    try {
      app.setClipboardSync(deviceId: device.id, on_: on);
    } finally {
      devices = app.devices();
      notifyListeners();
    }
  }

  /// Windows: see [d]'s screen in a window of its own. Its user is asked
  /// first. Returns null once it's showing, or why not.
  Future<String?> mirror(DeviceInfo d) async {
    try {
      await app.mirrorDevice(deviceId: d.id);
      return null;
    } catch (e) {
      return '$e'.replaceFirst(RegExp(r'^[^:]*Exception: ?'), '');
    }
  }

  /// Send clipboard text to every paired device that syncs it. Returns the
  /// names of the devices it reached. Text that just arrived from one of
  /// them isn't sent back.
  /// With [again], it goes even if it's what was sent or received last
  /// (the user shared it on purpose).
  /// With [to], it goes to that device only, whether or not it syncs the
  /// clipboard (the user picked it).
  Future<List<String>> shareClipboard(String text, {bool again = false, String? to}) async {
    if (text.isEmpty) return const [];
    // What just arrived (and so is on the clipboard now) isn't news.
    if (text == _lastClip && !again) return const [];
    _lastClip = text;
    final targets = to != null
        ? devices.where((d) => d.id == to && (d.paired || d.isBrowser)).toList()
        : pairedDevices.where((d) => d.clipboard && d.reachable).toList();
    final reached = <String>[];
    await Future.wait([
      for (final d in targets)
        app
            .sendClipboard(deviceId: d.id, text: text)
            .then((_) => reached.add(d.name))
            .catchError((_) {}),
    ]);
    return reached;
  }

  final _notes = StreamController<String>.broadcast();

  /// Short messages for the user ("Copied to Laptop").
  Stream<String> get notes => _notes.stream;

  /// [shareClipboard], and say where it went. With [leave], go back to the
  /// app it was shared from.
  Future<void> shareClipboardAndSay(String text, {bool leave = false, String? to}) async {
    final reached = await shareClipboard(text, again: leave, to: to);
    if (reached.isNotEmpty) {
      _notes.add('Copied to ${reached.join(', ')}');
      if (leave) await leaveApp();
    } else if (leave) {
      _notes.add("Couldn't reach a device to copy to");
    }
  }

  /// A copied image to every paired device that syncs the clipboard and is
  /// around. Returns the names of the devices it reached.
  Future<List<String>> shareClipboardImage(Uint8List data, String mime) async {
    if (data.isEmpty) return const [];
    final targets = pairedDevices.where((d) => d.clipboard && d.reachable).toList();
    final reached = <String>[];
    await Future.wait([
      for (final d in targets)
        app
            .sendClipboardImage(deviceId: d.id, mime: mime, data: data)
            .then((_) => reached.add(d.name))
            .catchError((_) {}),
    ]);
    return reached;
  }

  /// A copied image arrived (saved at [path]): onto the clipboard. Android's
  /// background service already put it there.
  Future<void> _clipboardImageArrived(String deviceId, String path) async {
    if (!Platform.isAndroid) {
      try {
        await setClipboardImage(await File(path).readAsBytes());
      } catch (_) {
        return;
      }
    }
    _clips.add((deviceById(deviceId)?.name ?? 'A paired device', ''));
  }

  /// What's on this device's clipboard, to a web page [device], which
  /// shows it with a Copy button. Says how it went.
  Future<void> sendClipboardTo(DeviceInfo device) async {
    final text = (await Clipboard.getData(Clipboard.kTextPlain))?.text ?? '';
    if (text.isEmpty) {
      _notes.add('Nothing to send: the clipboard is empty');
      return;
    }
    final reached = await shareClipboard(text, again: true, to: device.id);
    _notes.add(
      reached.isEmpty
          ? "Couldn't reach ${device.name}"
          : device.isBrowser
          ? 'Sent to ${device.name}: they tap Copy'
          : 'Copied to ${device.name}',
    );
  }

  Future<void> _clipboardArrived(String deviceId, String text) async {
    _lastClip = text;
    // Android's background service already put it there.
    if (!Platform.isAndroid) await setClipboardText(text);
    _clips.add((deviceById(deviceId)?.name ?? 'A paired device', text));
  }

  /// Devices (and when) an answer was last passed on, so a batch of files
  /// gives one alert, not one per file.
  final _told = <String, DateTime>{};

  void _tellAnswer(Transfer t, Answer answer) {
    final key = '${t.deviceId}/${answer.name}';
    final last = _told[key];
    final now = DateTime.now();
    _told[key] = now;
    if (last != null && now.difference(last) < const Duration(seconds: 5)) return;
    _answers.add((deviceById(t.deviceId), t.peerName ?? 'They', answer));
  }

  @override
  void dispose() {
    _subscription.cancel();
    _answers.close();
    _clips.close();
    _notes.close();
    _offerChanges.close();
    _pairing.close();
    _toSend.close();
    super.dispose();
  }
}
