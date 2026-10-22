// System notifications for transfers: one ongoing "sending/receiving" entry
// while files move, and a "received" one per incoming file.

import 'dart:async';

import 'controller.dart';
import 'desktop.dart';
import 'format.dart';
import 'platform.dart';

class TransferNotifier {
  TransferNotifier(this.controller) {
    controller.addListener(_onChange);
  }

  final NudgeController controller;

  /// Incoming transfers already announced.
  final _announced = <int>{};
  Timer? _throttle;
  Timer? _idle;
  bool _showing = false;

  void _onChange() {
    for (final t in controller.transfers) {
      if (t.incoming && t.state == TransferState.done && _announced.add(t.id)) {
        final from = controller.deviceById(t.deviceId)?.name ?? '';
        if (t.path case final path?) {
          unawaited(notifyReceived(path: path, from: from));
          unawaited(showReceivedToast(path: path, from: from));
        }
      }
    }
    // Progress events arrive many times a second; the notification needs one.
    _throttle ??= Timer(const Duration(milliseconds: 800), () {
      _throttle = null;
      _update();
    });
  }

  void _update() {
    final running = controller.transfers.where((t) => t.state == TransferState.running).toList();
    if (running.isEmpty) {
      // Multi-file sends have a short gap between files; don't flicker.
      _idle ??= Timer(const Duration(milliseconds: 1500), () {
        _idle = null;
        if (_showing && !controller.transfers.any((t) => t.state == TransferState.running)) {
          _showing = false;
          unawaited(hideTransferProgress());
        }
      });
      return;
    }
    _idle?.cancel();
    _idle = null;
    _showing = true;

    final size = running.fold<int>(0, (sum, t) => sum + t.size);
    final bytes = running.fold<int>(0, (sum, t) => sum + t.bytes);
    final speed = running.fold<double>(0, (sum, t) => sum + t.bytesPerSec);
    final incoming = running.every((t) => t.incoming);
    final outgoing = running.every((t) => !t.incoming);
    final String title;
    if (running.length == 1) {
      final t = running.single;
      final peer = controller.deviceById(t.deviceId)?.name;
      final verb = t.incoming ? 'Receiving' : 'Sending';
      title = peer == null
          ? '$verb ${t.fileName}'
          : '$verb ${t.fileName} ${t.incoming ? 'from' : 'to'} $peer';
    } else {
      final verb = incoming ? 'Receiving' : (outgoing ? 'Sending' : 'Moving');
      title = '$verb ${running.length} files';
    }
    final left = speed > 0 ? Duration(milliseconds: ((size - bytes) / speed * 1000).round()) : null;
    final text = [
      '${formatSize(bytes)} of ${formatSize(size)}',
      if (speed > 0) formatSpeed(speed),
      if (left != null) '${formatDuration(left)} left',
    ].join(' · ');
    unawaited(
      showTransferProgress(
        title: title,
        text: text,
        progress: size == 0 || bytes == 0 ? -1 : (bytes * 100 ~/ size),
      ),
    );
  }

  void dispose() {
    controller.removeListener(_onChange);
    _throttle?.cancel();
    _idle?.cancel();
  }
}
