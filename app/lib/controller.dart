// App state: the device list and transfers, kept current from core events.

import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:flutter/foundation.dart';

import 'platform.dart';
import 'src/rust/api/nudge.dart';

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
  });

  final int id;
  final String deviceId;
  final bool incoming;
  final String fileName;
  final int size;

  /// The other device's name when the transfer started; a browser tab may
  /// be gone by the time it's shown.
  final String? peerName;

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

  /// Outgoing: delivered, and the receiver is asking its user. A browser
  /// tab asks too, until it accepts.
  bool pending = false;

  /// Outgoing, delivered, and waiting for the receiver's answer. Nothing is
  /// moving, so it isn't shown as busy.
  bool get awaitingAnswer => !incoming && pending && !accepted && state == TransferState.running;

  /// Something is actually happening: bytes moving or about to.
  bool get active => state == TransferState.running && !awaitingAnswer;

  /// What happened, in a word or two, from this side.
  String get outcome {
    if (state == TransferState.failed) {
      return switch (error) {
        'they declined' => 'Declined',
        'no answer' => 'No answer',
        _ => error ?? 'Failed',
      };
    }
    if (incoming) return state == TransferState.done ? 'Received' : 'Receiving';
    if (awaitingAnswer) return 'Pending';
    if (state == TransferState.done) return pending ? 'Accepted · delivered' : 'Delivered';
    return pending ? 'Accepted · sending' : 'Sending';
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
  const FilesToSend(this.files, {required this.fromOutside, this.byQr = false});

  final List<OutgoingFile> files;
  final bool fromOutside;
  final bool byQr;
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

/// What the sender hears back: the receiver's answer.
enum Answer { accepted, declined, noAnswer }

class NudgeController extends ChangeNotifier {
  NudgeController(this.app, {String? dataDir})
    : _orderFile = dataDir == null ? null : File('$dataDir${Platform.pathSeparator}order.json') {
    _subscription = app.events().listen(_onEvent);
    devices = app.devices();
    webShareUrl = app.webShareUrl();
    _loadOrder();
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

  void offer(List<OutgoingFile> files, {bool fromOutside = true, bool byQr = false}) {
    if (files.isEmpty) return;
    if (!byQr) warmUp();
    final request = FilesToSend(files, fromOutside: fromOutside, byQr: byQr);
    _toSend.hasListener ? _toSend.add(request) : _unclaimed.add(request);
  }

  void unpair(DeviceInfo device) {
    app.unpair(deviceId: device.id);
  }

  /// Take one finished transfer off the list.
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
    _offerChanges.close();
    _pairing.close();
    _toSend.close();
    super.dispose();
  }
}
