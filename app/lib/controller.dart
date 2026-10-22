// App state: the device list and transfers, kept current from core events.

import 'dart:async';

import 'package:flutter/foundation.dart';

import 'platform.dart';
import 'src/rust/api/nudge.dart';

enum TransferState { running, done, failed }

class Transfer {
  Transfer({
    required this.id,
    required this.deviceId,
    required this.incoming,
    required this.fileName,
    required this.size,
  });

  final int id;
  final String deviceId;
  final bool incoming;
  final String fileName;
  final int size;

  TransferState state = TransferState.running;
  int bytes = 0;
  double bytesPerSec = 0;
  String? path;
  String? error;
  Duration? took;

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
class FilesToSend {
  const FilesToSend(this.files, {required this.fromOutside});

  final List<OutgoingFile> files;
  final bool fromOutside;
}

class NudgeController extends ChangeNotifier {
  NudgeController(this.app) {
    _subscription = app.events().listen(_onEvent);
    devices = app.devices();
  }

  final NudgeApp app;
  late final StreamSubscription<AppEvent> _subscription;

  List<DeviceInfo> devices = const [];

  /// Newest first.
  final List<Transfer> transfers = [];

  final _pairing = StreamController<AppEvent>.broadcast();

  /// `Paired` and `PairingClosed` events, for the pairing screen.
  Stream<AppEvent> get pairingEvents => _pairing.stream;

  String get deviceName => app.deviceName();

  List<DeviceInfo> get pairedDevices => devices.where((d) => d.paired).toList();

  List<DeviceInfo> get nearbyDevices => devices.where((d) => !d.paired).toList();

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

  void offer(List<OutgoingFile> files, {bool fromOutside = true}) {
    if (files.isEmpty) return;
    final request = FilesToSend(files, fromOutside: fromOutside);
    _toSend.hasListener ? _toSend.add(request) : _unclaimed.add(request);
  }

  void unpair(DeviceInfo device) {
    app.unpair(deviceId: device.id);
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
          Transfer(id: id, deviceId: deviceId, incoming: incoming, fileName: fileName, size: size),
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
          ..bytes = t.size
          ..path = path
          ..took = Duration(microseconds: (seconds * 1e6).round());
        // Speed over this connection's bytes (a resumed file sends fewer).
        if (seconds > 0) t.bytesPerSec = bytes / seconds;
      case AppEvent_TransferFailed(:final id, :final error):
        final t = _transfer(id);
        if (t == null) return;
        t
          ..state = TransferState.failed
          ..error = error;
    }
    notifyListeners();
  }

  @override
  void dispose() {
    _subscription.cancel();
    _pairing.close();
    _toSend.close();
    super.dispose();
  }
}
