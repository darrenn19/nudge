// System notifications for transfers: one ongoing "sending/receiving" entry
// while files move, a "received" one per incoming file, and a question with
// Accept / Decline when a device wants to send and Nudge isn't on screen.

import 'dart:async';
import 'dart:io';

import 'package:flutter/widgets.dart';

import 'controller.dart';
import 'desktop.dart';
import 'format.dart';
import 'platform.dart';

class TransferNotifier {
  TransferNotifier(this.controller) {
    controller.addListener(_onChange);
    controller.offerChanges.listen((_) => _syncQuestions());
    listenForOfferAnswers(_answer);
  }

  /// Accept or Decline from a notification or toast: files or pairing.
  void _answer(int id, bool accept) {
    for (final o in controller.offers.toList()) {
      if (o.id == id) controller.answerOffer(o, accept: accept);
    }
    for (final a in controller.pairAsks.toList()) {
      if (a.id == id) controller.answerPair(a, accept: accept);
    }
  }

  /// Questions shown outside the app, and how to take each down.
  final _asked = <int, void Function()>{};

  Future<void> _syncQuestions() async {
    final open = {
      for (final o in controller.offers) o.id,
      for (final a in controller.pairAsks) a.id,
    };
    for (final id in _asked.keys.where((id) => !open.contains(id)).toList()) {
      _asked.remove(id)!();
    }
    final questions = [
      for (final o in controller.offers)
        (
          o.id,
          '${o.deviceName} wants to send you ${o.what}',
          o.count == 1 ? formatSize(o.size) : '${o.fileName} and more · ${formatSize(o.size)}',
        ),
      for (final a in controller.pairAsks)
        (a.id, '${a.deviceName} wants to pair', 'Check it shows ${a.code}'),
    ];
    for (final (id, title, text) in questions) {
      if (_asked.containsKey(id) || await _onScreen()) continue;
      if (Platform.isAndroid) {
        _asked[id] = () => unawaited(cancelOfferNotification(id));
        await notifyOffer(id: id, title: title, text: text);
      } else {
        _asked[id] = () {};
        _asked[id] = await showOfferToast(
          title: title,
          body: text,
          onAnswer: (accept) => _answer(id, accept),
        );
      }
    }
  }

  /// The app is in front of the user, so its own question is enough.
  Future<bool> _onScreen() async {
    if (Platform.isAndroid) {
      return WidgetsBinding.instance.lifecycleState == AppLifecycleState.resumed;
    }
    return windowInFront();
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
        if (t.path case final path?) unawaited(_announce(path, from));
      }
    }
    // Progress events arrive many times a second; the notification needs one.
    _throttle ??= Timer(const Duration(milliseconds: 800), () {
      _throttle = null;
      _update();
    });
  }

  /// "Received photo.jpg": a notification or toast, unless Nudge is on
  /// screen and shows it itself.
  Future<void> _announce(String path, String from) async {
    if (await _onScreen()) {
      await indexReceived(path);
      return;
    }
    await notifyReceived(path: path, from: from);
    await showReceivedToast(path: path, from: from);
  }

  void _update() {
    // Moving files only: a send waiting on an answer isn't "in progress".
    // On screen, the app shows progress itself, so the notification goes
    // back to its quiet "ready" state (it can't go away: it keeps Nudge
    // running).
    final onScreen =
        Platform.isAndroid && WidgetsBinding.instance.lifecycleState == AppLifecycleState.resumed;
    final running = onScreen ? <Transfer>[] : controller.transfers.where((t) => t.active).toList();
    if (running.isEmpty) {
      // Multi-file sends have a short gap between files; don't flicker.
      _idle ??= Timer(const Duration(milliseconds: 1500), () {
        _idle = null;
        if (_showing && !controller.transfers.any((t) => t.active)) {
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
