import 'dart:async';
import 'dart:io';
import 'dart:math' as math;
import 'dart:ui' show PointerDeviceKind;

import 'package:desktop_drop/desktop_drop.dart';
import 'package:flutter/cupertino.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:open_filex/open_filex.dart';

import '../controller.dart';
import '../format.dart';
import '../platform.dart';
import '../src/rust/api/nudge.dart';
import 'pair.dart';
import 'send_sheet.dart';
import 'style.dart';
import 'web_share.dart';

/// One big orb per device; swipe between them and tap to send. This device
/// sits underneath as a small bubble, with dots flowing between the two
/// while files move. The last page adds a device.
class HomeScreen extends StatefulWidget {
  const HomeScreen({super.key, required this.controller});

  final NudgeController controller;

  @override
  State<HomeScreen> createState() => _HomeScreenState();
}

class _HomeScreenState extends State<HomeScreen> {
  NudgeController get controller => widget.controller;
  late final StreamSubscription<FilesToSend> _incoming;
  late final StreamSubscription<void> _questions;
  late final StreamSubscription<(DeviceInfo?, String, Answer)> _answers;
  late final StreamSubscription<(String, String)> _clips;
  late final StreamSubscription<String> _notes;
  final _pages = PageController(viewportFraction: 0.74);

  /// The device on screen; null on the "add a device" page.
  String? _focusId;
  List<String> _shownIds = const [];

  /// Arrivals already announced, and incoming transfers already followed.
  final _announced = <int>{};
  final _followed = <int>{};
  bool _dragging = false;

  /// Bumped for each arrival: this device's bubble pops.
  int _arrivals = 0;

  @override
  void initState() {
    super.initState();
    _incoming = controller.filesToSend.listen((request) {
      if (mounted) chooseAndSend(context, controller, request);
    });
    // Anything that already happened isn't news.
    _announced.addAll(controller.transfers.map((t) => t.id));
    _followed.addAll(controller.transfers.map((t) => t.id));
    _focusId = _targets.firstOrNull?.id;
    controller.addListener(_onChange);
    _questions = controller.offerChanges.listen((_) => _askNext());
    _answers = controller.answers.listen((a) {
      final (_, name, answer) = a;
      if (!mounted) return;
      switch (answer) {
        case Answer.accepted:
          showToast(context, '$name accepted', icon: CupertinoIcons.checkmark_circle);
        case Answer.declined:
          showToast(context, '$name declined', icon: CupertinoIcons.xmark_circle);
        case Answer.noAnswer:
          showToast(context, "$name didn't answer", icon: CupertinoIcons.clock);
      }
    });
    _clips = controller.clipsReceived.listen((c) {
      if (mounted) showToast(context, 'Copied from ${c.$1}', icon: CupertinoIcons.doc_on_clipboard);
    });
    _notes = controller.notes.listen((note) {
      if (mounted) showToast(context, note, icon: CupertinoIcons.doc_on_clipboard);
    });
    WidgetsBinding.instance.addPostFrameCallback((_) => _askNext());
    // Already paired (say, after an update): the Samsung guide, once, after
    // the screen has settled.
    Future<void>.delayed(const Duration(seconds: 2), _guideKeepAwake);
  }

  /// The question on screen, if any.
  int? _asking;

  /// Show the oldest unanswered question. One at a time; the sheet closes
  /// itself when its question is answered elsewhere or times out.
  Future<void> _askNext() async {
    if (!mounted || _asking != null) return;
    final offer = controller.offers.firstOrNull;
    final pair = controller.pairAsks.firstOrNull;
    if (offer == null && pair == null) return;
    HapticFeedback.mediumImpact();
    // Oldest first.
    if (pair != null && (offer == null || pair.id < offer.id)) {
      _asking = pair.id;
      await showGlassSheet<void>(
        context,
        (_) => _PairQuestionSheet(controller: controller, ask: pair),
      );
    } else {
      _asking = offer!.id;
      _goTo(offer.deviceId);
      await showGlassSheet<void>(
        context,
        (_) => _QuestionSheet(controller: controller, offer: offer),
      );
    }
    _asking = null;
    _askNext();
  }

  @override
  void dispose() {
    _incoming.cancel();
    _questions.cancel();
    _answers.cancel();
    _clips.cancel();
    _notes.cancel();
    controller.removeListener(_onChange);
    _pages.dispose();
    super.dispose();
  }

  /// Paired devices, open browser pages, then unpaired ones nearby.
  List<DeviceInfo> get _targets => [
    ...controller.pairedDevices,
    ...controller.browserDevices,
    ...controller.nearbyDevices,
  ];

  DeviceInfo? get _focus {
    for (final d in _targets) {
      if (d.id == _focusId) return d;
    }
    return null;
  }

  void _onChange() {
    for (final t in controller.transfers) {
      // Something's arriving: show who it's from.
      if (t.incoming && _followed.add(t.id)) _goTo(t.deviceId);
      if (!t.incoming || t.state != TransferState.done || !_announced.add(t.id)) continue;
      setState(() => _arrivals++);
      final path = t.path;
      showToast(
        context,
        t.peerName == null ? t.fileName : '${t.fileName} · from ${t.peerName}',
        icon: CupertinoIcons.arrow_down_circle,
        action: path == null ? null : 'Open',
        onAction: path == null ? null : () => OpenFilex.open(path),
      );
    }
  }

  void _goTo(String deviceId) {
    final i = _targets.indexWhere((d) => d.id == deviceId);
    if (i < 0 || !_pages.hasClients) return;
    _pages.animateToPage(
      i,
      duration: const Duration(milliseconds: 450),
      curve: Curves.easeOutCubic,
    );
  }

  void _step(int by) {
    if (!_pages.hasClients) return;
    final next = ((_pages.page ?? 0).round() + by).clamp(0, _targets.length);
    _pages.animateToPage(
      next,
      duration: const Duration(milliseconds: 380),
      curve: Curves.easeOutCubic,
    );
  }

  Future<void> _openPairing() async {
    final device = await Navigator.of(context)
        .push<DeviceInfo>(nudgeRoute((_) => PairScreen(controller: controller)));
    if (device != null && mounted) {
      showToast(context, 'Paired with ${device.name}');
      _focusId = device.id;
      WidgetsBinding.instance.addPostFrameCallback((_) => _goTo(device.id));
      // Now that there's somewhere to send, make sure Android won't stop
      // Nudge mid-transfer or while waiting to receive, and can start the
      // hotspot for big transfers. Each asked once.
      unawaited(
        askToRunInBackground().then((_) => askForDirectLink()).then((_) => _guideKeepAwake()),
      );
    }
  }

  Future<void> _openWebShare() async {
    // Start sharing first (it takes a moment), so the screen slides in
    // complete instead of changing halfway through.
    try {
      await controller.startWebShare();
    } catch (_) {
      // The screen shows the error and a retry.
    }
    if (!mounted) return;
    await Navigator.of(context).push(nudgeRoute((_) => WebShareScreen(controller: controller)));
  }

  Future<void> _sendTo(DeviceInfo device) async {
    controller.warmUp(device);
    final files = await pickFiles();
    // A file dialog can leave a stray click behind on the window; don't let
    // it open the menu.
    _quietUntil = DateTime.now().add(const Duration(milliseconds: 900));
    if (files.isEmpty) return;
    controller.send(device, files);
  }

  /// A whole folder: it arrives as a folder.
  Future<void> _sendFolderTo(DeviceInfo device) async {
    controller.warmUp(device);
    final files = await pickFolder();
    _quietUntil = DateTime.now().add(const Duration(milliseconds: 900));
    if (files.isEmpty) return;
    controller.send(device, files);
  }

  /// Until then, ignore stray presses (see `_sendTo`).
  DateTime _quietUntil = DateTime(0);

  void _tapDevice(DeviceInfo d) {
    if (!d.paired && !d.isBrowser) {
      _pairNearby(d);
    } else if (d.online) {
      _sendTo(d);
    } else {
      _explainAway(d);
    }
  }

  /// Pair with a device that's nearby: no QR code, just a confirmation on
  /// the other side.
  Future<void> _pairNearby(DeviceInfo d) async {
    final paired = await showGlassSheet<DeviceInfo>(
      context,
      (_) => _PairingSheet(controller: controller, device: d, onUseCode: _openPairing),
    );
    if (paired != null && mounted) {
      showToast(context, 'Paired with ${paired.name}');
      _focusId = paired.id;
      WidgetsBinding.instance.addPostFrameCallback((_) => _goTo(paired.id));
      unawaited(
        askToRunInBackground().then((_) => askForDirectLink()).then((_) => _guideKeepAwake()),
      );
    }
  }

  /// What the "!" means. A phone with its screen off often still receives,
  /// so there's a way to try anyway when there's an address for it.
  void _explainAway(DeviceInfo d) {
    showMenuSheet(
      context,
      title: "${d.name} isn't nearby",
      subtitle:
          "It hasn't been seen on this Wi-Fi just now. It may be asleep, switched off, "
          'or on another network.'
          '${d.reachable ? ' A sleeping phone usually still receives.' : ''}'
          // After a restart, Android keeps Nudge's storage locked (and Nudge
          // stopped) until the phone is unlocked once.
          '${d.platform == 'android' ? ' If it restarted, unlock it once so Nudge can start.' : ''}',
      items: [
        if (d.reachable)
          MenuItem(label: 'Send anyway', icon: CupertinoIcons.paperplane, onTap: () => _sendTo(d)),
        MenuItem(label: 'Look again', icon: CupertinoIcons.refresh, onTap: controller.refresh),
      ],
    );
  }

  /// Samsung, once: put Nudge in "Never sleeping apps", or files stop
  /// arriving while the phone is idle.
  Future<void> _guideKeepAwake() async {
    if (_guiding || controller.pairedDevices.isEmpty || !await needsSleepGuide()) return;
    if (!mounted) return;
    _guiding = true;
    var opened = false;
    await showMenuSheet(
      context,
      title: 'Keep Nudge awake',
      subtitle:
          'Samsung phones put apps to sleep, and a sleeping Nudge misses files. '
          'On the next screen, tap "Never sleeping apps", then +, and add Nudge.',
      items: [
        MenuItem(
          label: 'Open battery settings',
          icon: CupertinoIcons.battery_full,
          onTap: () {
            opened = true;
            unawaited(sleepGuideDone(open: true));
          },
        ),
      ],
    );
    if (!opened) await sleepGuideDone(open: false);
  }

  bool _guiding = false;

  /// Put devices in the order you want them.
  void _arrange() {
    showGlassSheet<void>(context, (_) => _ArrangeSheet(controller: controller));
  }

  /// What this device is called, and where that comes from.
  Future<void> _aboutThisDevice() async {
    final tileWhileLocked = isPhone ? await latestTileWhileLocked() : null;
    if (!mounted) return;
    await showMenuSheet(
      context,
      title: 'This is you',
      subtitle:
          'Your other devices see this one as "${controller.deviceName}". '
          '${isPhone ? 'To change it, go to Settings → About phone → Device name.' : 'To change it, go to Settings → System → About → Rename this PC.'}',
      items: [
        // USB cables and Ethernet are always used when faster; the hotspot
        // takes the PC off its usual Wi-Fi for a while, so it can be refused.
        MenuItem(
          label: isPhone
              ? 'Start a hotspot for big transfers'
              : "Join the phone's hotspot for big transfers",
          icon: CupertinoIcons.bolt,
          toggle: controller.directLink,
          onToggle: controller.setDirectLink,
        ),
        // The quick settings tile sends the newest photo or download; from
        // the lock screen only if allowed here.
        if (tileWhileLocked != null)
          MenuItem(
            label: '"Send latest" tile works while locked',
            icon: CupertinoIcons.device_phone_portrait,
            toggle: tileWhileLocked,
            onToggle: (on) => unawaited(setLatestTileWhileLocked(on)),
          ),
        MenuItem(
          label: 'Files while locked: ${controller.whenLocked.label}',
          icon: CupertinoIcons.lock,
          onTap: _chooseWhenLocked,
        ),
      ],
    );
  }

  /// Who may send while this device is locked.
  void _chooseWhenLocked() {
    showMenuSheet(
      context,
      title: 'Accept files while locked',
      subtitle: isPhone
          ? 'While the phone is locked or its screen is off.'
          : 'While this PC is locked.',
      items: [
        for (final choice in WhenLocked.values)
          MenuItem(
            label: choice.label,
            icon: choice == controller.whenLocked
                ? CupertinoIcons.checkmark_circle_fill
                : CupertinoIcons.circle,
            onTap: () => controller.setWhenLocked(choice),
          ),
      ],
    );
  }

  Future<void> _deviceMenu(DeviceInfo d) async {
    if (!d.paired || DateTime.now().isBefore(_quietUntil)) return;
    await showMenuSheet(
      context,
      title: d.name,
      subtitle: [
        d.online ? 'Nearby' : 'Not nearby',
        if (d.acceptsFromUs == true) 'accepts your files automatically',
        if (d.acceptsFromUs == false) 'asks before accepting your files',
      ].join(' · '),
      items: [
        MenuItem(label: 'Send files', icon: CupertinoIcons.paperplane, onTap: () => _sendTo(d)),
        MenuItem(
          label: 'Send a folder',
          icon: CupertinoIcons.folder,
          onTap: () => unawaited(_sendFolderTo(d)),
        ),
        MenuItem(
          label: 'Always accept files',
          icon: CupertinoIcons.checkmark_shield,
          toggle: d.autoAccept,
          onToggle: (on) => controller.setAutoAccept(d, on),
        ),
        MenuItem(
          label: 'Share clipboard',
          icon: CupertinoIcons.doc_on_clipboard,
          toggle: d.clipboard,
          onToggle: (on) => controller.setClipboardSync(d, on),
        ),
        if (controller.pairedDevices.length > 1)
          MenuItem(
            label: 'Arrange devices',
            icon: CupertinoIcons.arrow_up_arrow_down,
            onTap: _arrange,
          ),
        MenuItem(
          label: 'Forget this device',
          icon: CupertinoIcons.trash,
          destructive: true,
          onTap: () async {
            final ok = await confirm(
              context,
              title: 'Forget ${d.name}?',
              message: "You'll need to pair again to send between these devices.",
              action: 'Forget',
              destructive: true,
            );
            if (ok) controller.unpair(d);
          },
        ),
      ],
    );
  }

  /// Windows: files dropped on the window go to the device on screen, or
  /// to a picker if it can't take them now.
  void _onDrop(DropDoneDetails details) {
    setState(() => _dragging = false);
    final files = [
      for (final item in details.files)
        // Files and folders (a folder goes whole).
        if (FileSystemEntity.typeSync(item.path) != FileSystemEntityType.notFound)
          OutgoingFile.fromPath(item.path),
    ];
    if (files.isEmpty) return;
    final d = _focus;
    if (d != null && (d.paired || d.isBrowser) && d.online) {
      controller.send(d, files);
    } else {
      controller.offer(files, fromOutside: false);
    }
  }

  @override
  Widget build(BuildContext context) {
    return DropTarget(
      enable: Platform.isWindows,
      onDragEntered: (_) {
        setState(() => _dragging = true);
        controller.warmUp();
      },
      onDragExited: (_) => setState(() => _dragging = false),
      onDragDone: _onDrop,
      child: CallbackShortcuts(
        bindings: {
          const SingleActivator(LogicalKeyboardKey.arrowLeft): () => _step(-1),
          const SingleActivator(LogicalKeyboardKey.arrowRight): () => _step(1),
        },
        child: Focus(
          autofocus: true,
          child: Scaffold(
            backgroundColor: Colors.transparent,
            body: RepaintBoundary(
              child: SafeArea(
                child: ListenableBuilder(
                  listenable: controller,
                  builder: (context, _) => _buildContent(),
                ),
              ),
            ),
          ),
        ),
      ),
    );
  }

  Widget _buildContent() {
    final targets = _targets;
    final ids = [for (final d in targets) d.id];
    // The list changed (a device came or went): stay on the same device.
    if (!_sameIds(ids, _shownIds)) {
      _shownIds = ids;
      final keep = _focusId == null ? targets.length : ids.indexOf(_focusId!);
      WidgetsBinding.instance.addPostFrameCallback((_) {
        if (!_pages.hasClients) return;
        final target = keep < 0 ? 0 : keep;
        if ((_pages.page ?? 0).round() != target) _pages.jumpToPage(target);
        if (keep < 0) setState(() => _focusId = targets.firstOrNull?.id);
      });
    }

    // Busy means bytes moving: a send waiting on an answer is delivered and
    // shown as such, not as busy.
    final running = controller.transfers.where((t) => t.active).toList();
    final focus = _focus;
    final linked = [
      for (final t in running)
        if (t.deviceId == focus?.id) t,
    ];
    final incoming = running.where((t) => t.incoming).toList();
    final inSize = incoming.fold<int>(0, (s, t) => s + t.size);
    final inBytes = incoming.fold<int>(0, (s, t) => s + t.bytes);

    return Column(
      children: [
        Padding(
          padding: const EdgeInsets.fromLTRB(16, 8, 16, 0),
          child: Row(
            children: [
              GlassIconButton(
                icon: CupertinoIcons.clock,
                tooltip: 'Activity',
                dot: running.isNotEmpty,
                onTap: () => showActivity(context, controller),
              ),
              const Spacer(),
              GlassIconButton(
                icon: CupertinoIcons.globe,
                tooltip: 'Share without the app',
                dot: controller.webShareUrl != null,
                onTap: _openWebShare,
              ),
              const SizedBox(width: 10),
              GlassIconButton(
                icon: CupertinoIcons.qrcode,
                tooltip: 'Pair a device',
                onTap: _openPairing,
              ),
            ],
          ),
        ),
        Expanded(
          child: LayoutBuilder(
            builder: (context, box) {
              final orb = math
                  .min(box.maxWidth * 0.74 - 24, box.maxHeight - _underOrb - 8)
                  .clamp(140.0, 330.0);
              return Stack(
                alignment: Alignment.center,
                children: [
                  ScrollConfiguration(
                    // Swipe with a mouse too, not just touch.
                    behavior: ScrollConfiguration.of(context)
                        .copyWith(dragDevices: PointerDeviceKind.values.toSet(), scrollbars: false),
                    child: PageView.builder(
                      controller: _pages,
                      itemCount: targets.length + 1,
                      onPageChanged: (i) {
                        tapFeedback();
                        setState(() => _focusId = i < targets.length ? targets[i].id : null);
                      },
                      itemBuilder: (context, i) => _Carousel(
                        pages: _pages,
                        index: i,
                        child: i < targets.length
                            ? _DevicePage(
                                device: targets[i],
                                focused: targets[i].id == _focusId,
                                dragging: _dragging,
                                transfers: controller.transfers,
                                size: orb,
                                onTap: () => _tapDevice(targets[i]),
                                onMenu: () => _deviceMenu(targets[i]),
                                onAway: () => _explainAway(targets[i]),
                              )
                            : _AddPage(size: orb, focused: _focusId == null, onTap: _openPairing),
                      ),
                    ),
                  ),
                  if (!isPhone && targets.isNotEmpty) ...[
                    Positioned(
                      left: 12,
                      child: _Arrow(
                        icon: CupertinoIcons.chevron_left,
                        visible: _focusId != targets.first.id,
                        onTap: () => _step(-1),
                      ),
                    ),
                    Positioned(
                      right: 12,
                      child: _Arrow(
                        icon: CupertinoIcons.chevron_right,
                        visible: _focusId != null,
                        onTap: () => _step(1),
                      ),
                    ),
                  ],
                ],
              );
            },
          ),
        ),
        _PageDots(
          onTap: (i) => _pages.animateToPage(
            i,
            duration: const Duration(milliseconds: 420),
            curve: Curves.easeOutCubic,
          ),
          count: targets.length + 1,
          index: _focusId == null
              ? targets.length
              : math.max(0, targets.indexWhere((d) => d.id == _focusId)),
        ),
        _FlowGap(
          // Sending goes up to the device; receiving comes down to this one.
          direction: linked.isEmpty ? null : (linked.first.incoming ? 1 : -1),
        ),
        _Self(
          onTap: _aboutThisDevice,
          name: controller.deviceName,
          progress: incoming.isEmpty || inSize == 0 ? null : inBytes / inSize,
          busy: incoming.isNotEmpty,
          arrivals: _arrivals,
        ),
        Padding(
          padding: const EdgeInsets.fromLTRB(24, 12, 24, 8),
          child: AnimatedSwitcher(
            duration: const Duration(milliseconds: 300),
            child: Text(
              _status(running, targets),
              key: ValueKey(_status(running, targets)),
              textAlign: TextAlign.center,
              maxLines: 1,
              overflow: TextOverflow.ellipsis,
              style: Fonts.small,
            ),
          ),
        ),
        _ActivityPill(controller: controller),
        const SizedBox(height: 12),
      ],
    );
  }

  static bool _sameIds(List<String> a, List<String> b) {
    if (a.length != b.length) return false;
    for (var i = 0; i < a.length; i++) {
      if (a[i] != b[i]) return false;
    }
    return true;
  }

  String _status(List<Transfer> running, List<DeviceInfo> targets) {
    if (_dragging) {
      final d = _focus;
      return d != null && d.online ? 'Drop to send to ${d.name}' : 'Drop to choose who gets them';
    }
    if (running.isNotEmpty) {
      final speed = running.fold<double>(0, (s, t) => s + t.bytesPerSec);
      final left = running.fold<int>(0, (s, t) => s + t.size - t.bytes);
      if (speed <= 0) return 'Starting…';
      return '${formatSpeed(speed)} · '
          '${formatDuration(Duration(seconds: (left / speed).ceil()))} left';
    }
    if (targets.isEmpty) return 'Pair your first device to start';
    if (_focusId == null) return '${isPhone ? 'Tap' : 'Click'} to pair another device';
    if (!controller.app.discoveryRunning()) return "Can't search this network";
    final more = targets.length > 1 ? (isPhone ? ' · swipe for others' : ' · ← → for others') : '';
    return '${isPhone ? 'Tap' : 'Click'} to send$more';
  }
}

/// "Darren's PC wants to send you 3 files": Accept or Decline, and whether
/// to always accept from that device. Closes itself if the question is
/// answered elsewhere (a notification) or times out.
class _QuestionSheet extends StatefulWidget {
  const _QuestionSheet({required this.controller, required this.offer});

  final NudgeController controller;
  final IncomingOffer offer;

  @override
  State<_QuestionSheet> createState() => _QuestionSheetState();
}

class _QuestionSheetState extends State<_QuestionSheet> {
  bool _always = false;
  bool _done = false;
  late final StreamSubscription<void> _changes;

  @override
  void initState() {
    super.initState();
    _changes = widget.controller.offerChanges.listen((_) {
      final open = widget.controller.offers.any((o) => o.id == widget.offer.id);
      if (!open) _close();
    });
  }

  @override
  void dispose() {
    _changes.cancel();
    super.dispose();
  }

  void _close() {
    if (_done || !mounted) return;
    _done = true;
    Navigator.of(context).pop();
  }

  void _answer(bool accept) {
    if (_done) return;
    widget.controller.answerOffer(widget.offer, accept: accept, always: accept && _always);
    _close();
  }

  @override
  Widget build(BuildContext context) {
    final o = widget.offer;
    return Padding(
      padding: const EdgeInsets.fromLTRB(20, 18, 20, 18),
      child: Column(
        mainAxisSize: MainAxisSize.min,
        children: [
          Container(
            width: 64,
            height: 64,
            decoration: BoxDecoration(
              shape: BoxShape.circle,
              color: Colors.white.withValues(alpha: 0.12),
              border: Border.all(color: Palette.glassBorder),
            ),
            child: const Icon(CupertinoIcons.arrow_down, color: Palette.text, size: 26),
          ),
          const SizedBox(height: 14),
          Text(
            '${o.deviceName} wants to send you ${o.what}',
            textAlign: TextAlign.center,
            style: Fonts.title.copyWith(fontSize: 20),
          ),
          const SizedBox(height: 4),
          Text(
            o.count == 1 ? formatSize(o.size) : '${o.fileName} and more · ${formatSize(o.size)}',
            textAlign: TextAlign.center,
            maxLines: 1,
            overflow: TextOverflow.ellipsis,
            style: Fonts.small,
          ),
          // They're already coming: accepting just keeps them.
          ListenableBuilder(
            listenable: widget.controller,
            builder: (context, _) {
              final arriving = [
                for (final t in widget.controller.transfers)
                  if (t.incoming && t.deviceId == o.deviceId && t.state == TransferState.running) t,
              ];
              if (arriving.isEmpty) return const SizedBox(height: 4);
              final bytes = arriving.fold<int>(0, (s, t) => s + t.bytes);
              final all = arriving.every((t) => t.waitingForYou) && bytes >= o.size;
              return Padding(
                padding: const EdgeInsets.only(top: 6),
                child: Text(
                  all
                      ? 'Already here · accept to keep'
                      : 'Already arriving · ${(100 * bytes / (o.size == 0 ? 1 : o.size)).clamp(0, 100).floor()}%',
                  textAlign: TextAlign.center,
                  style: Fonts.small.copyWith(color: Palette.good),
                ),
              );
            },
          ),
          const SizedBox(height: 18),
          Pressable(
            onTap: () => setState(() => _always = !_always),
            child: Container(
              padding: const EdgeInsets.fromLTRB(16, 6, 8, 6),
              decoration: BoxDecoration(
                color: Palette.glass,
                borderRadius: BorderRadius.circular(18),
                border: Border.all(color: Palette.glassBorder),
              ),
              child: Row(
                children: [
                  Expanded(
                    child: Text(
                      'Always accept from ${o.deviceName}',
                      style: const TextStyle(color: Palette.text, fontSize: 15),
                    ),
                  ),
                  IgnorePointer(
                    child: CupertinoSwitch(
                      value: _always,
                      activeTrackColor: Palette.good,
                      onChanged: (_) {},
                    ),
                  ),
                ],
              ),
            ),
          ),
          const SizedBox(height: 14),
          Row(
            children: [
              Expanded(
                child: SheetButton(label: 'Decline', onTap: () => _answer(false)),
              ),
              const SizedBox(width: 10),
              Expanded(
                child: SheetButton(label: 'Accept', filled: true, onTap: () => _answer(true)),
              ),
            ],
          ),
        ],
      ),
    );
  }
}

/// Pairing with a nearby device: shows the code, waits for the other side
/// to accept. Pops with the paired device.
class _PairingSheet extends StatefulWidget {
  const _PairingSheet({required this.controller, required this.device, required this.onUseCode});

  final NudgeController controller;
  final DeviceInfo device;
  final VoidCallback onUseCode;

  @override
  State<_PairingSheet> createState() => _PairingSheetState();
}

class _PairingSheetState extends State<_PairingSheet> {
  late final String _code = widget.controller.pairCode(widget.device);
  bool _failed = false;

  @override
  void initState() {
    super.initState();
    _ask();
  }

  Future<void> _ask() async {
    setState(() => _failed = false);
    try {
      final paired = await widget.controller.pairNearby(widget.device);
      if (mounted) Navigator.of(context).pop(paired);
    } catch (_) {
      if (mounted) setState(() => _failed = true);
    }
  }

  @override
  Widget build(BuildContext context) {
    final name = widget.device.name;
    return Padding(
      padding: const EdgeInsets.fromLTRB(20, 18, 20, 18),
      child: Column(
        mainAxisSize: MainAxisSize.min,
        children: [
          Text('Pair with $name', style: Fonts.title.copyWith(fontSize: 20)),
          const SizedBox(height: 18),
          _Code(_code),
          const SizedBox(height: 14),
          AnimatedSwitcher(
            duration: const Duration(milliseconds: 250),
            child: _failed
                ? Text(
                    "$name didn't accept. Make sure Nudge is open on it, then try again.",
                    key: const ValueKey('failed'),
                    textAlign: TextAlign.center,
                    style: Fonts.body.copyWith(color: Palette.bad),
                  )
                : Row(
                    key: const ValueKey('waiting'),
                    mainAxisSize: MainAxisSize.min,
                    children: [
                      const CupertinoActivityIndicator(color: Palette.muted, radius: 8),
                      const SizedBox(width: 10),
                      Flexible(
                        child: Text(
                          'Accept on $name, and check it shows this code',
                          style: Fonts.body,
                        ),
                      ),
                    ],
                  ),
          ),
          const SizedBox(height: 18),
          Row(
            children: [
              Expanded(
                child: SheetButton(
                  label: 'Use a QR code',
                  onTap: () {
                    Navigator.of(context).pop();
                    widget.onUseCode();
                  },
                ),
              ),
              if (_failed) ...[
                const SizedBox(width: 10),
                Expanded(
                  child: SheetButton(label: 'Try again', filled: true, onTap: _ask),
                ),
              ],
            ],
          ),
        ],
      ),
    );
  }
}

/// "Darren's S24 wants to pair": check the code, then pair or decline.
class _PairQuestionSheet extends StatefulWidget {
  const _PairQuestionSheet({required this.controller, required this.ask});

  final NudgeController controller;
  final PairAsk ask;

  @override
  State<_PairQuestionSheet> createState() => _PairQuestionSheetState();
}

class _PairQuestionSheetState extends State<_PairQuestionSheet> {
  bool _done = false;
  late final StreamSubscription<void> _changes;

  @override
  void initState() {
    super.initState();
    _changes = widget.controller.offerChanges.listen((_) {
      if (!widget.controller.pairAsks.any((a) => a.id == widget.ask.id)) _close();
    });
  }

  @override
  void dispose() {
    _changes.cancel();
    super.dispose();
  }

  void _close() {
    if (_done || !mounted) return;
    _done = true;
    Navigator.of(context).pop();
  }

  void _answer(bool accept) {
    if (_done) return;
    widget.controller.answerPair(widget.ask, accept: accept);
    _close();
  }

  @override
  Widget build(BuildContext context) {
    final a = widget.ask;
    return Padding(
      padding: const EdgeInsets.fromLTRB(20, 18, 20, 18),
      child: Column(
        mainAxisSize: MainAxisSize.min,
        children: [
          Text(
            '${a.deviceName} wants to pair',
            textAlign: TextAlign.center,
            style: Fonts.title.copyWith(fontSize: 20),
          ),
          const SizedBox(height: 18),
          _Code(a.code),
          const SizedBox(height: 14),
          Text(
            'Pair only if ${a.deviceName} shows the same code.',
            textAlign: TextAlign.center,
            style: Fonts.body,
          ),
          const SizedBox(height: 18),
          Row(
            children: [
              Expanded(
                child: SheetButton(label: 'Decline', onTap: () => _answer(false)),
              ),
              const SizedBox(width: 10),
              Expanded(
                child: SheetButton(label: 'Pair', filled: true, onTap: () => _answer(true)),
              ),
            ],
          ),
        ],
      ),
    );
  }
}

/// A pairing code, big and spaced, to compare at a glance.
class _Code extends StatelessWidget {
  const _Code(this.code);

  final String code;

  @override
  Widget build(BuildContext context) {
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 22, vertical: 12),
      decoration: BoxDecoration(
        color: Palette.glass,
        borderRadius: BorderRadius.circular(18),
        border: Border.all(color: Palette.glassBorder),
      ),
      child: Text(
        code,
        style: const TextStyle(
          color: Palette.text,
          fontSize: 34,
          fontWeight: FontWeight.w700,
          letterSpacing: 4,
          fontFeatures: [FontFeature.tabularFigures()],
        ),
      ),
    );
  }
}

/// Drag your devices into the order you want them.
class _ArrangeSheet extends StatefulWidget {
  const _ArrangeSheet({required this.controller});

  final NudgeController controller;

  @override
  State<_ArrangeSheet> createState() => _ArrangeSheetState();
}

class _ArrangeSheetState extends State<_ArrangeSheet> {
  late final List<DeviceInfo> _devices = List.of(widget.controller.pairedDevices);

  @override
  Widget build(BuildContext context) {
    return Padding(
      padding: const EdgeInsets.fromLTRB(16, 14, 16, 16),
      child: Column(
        mainAxisSize: MainAxisSize.min,
        children: [
          Text('Arrange devices', style: Fonts.title.copyWith(fontSize: 19)),
          const SizedBox(height: 4),
          const Text('Drag to change the order you swipe through', style: Fonts.small),
          const SizedBox(height: 14),
          Flexible(
            child: ReorderableListView(
              shrinkWrap: true,
              buildDefaultDragHandles: false,
              proxyDecorator: (child, _, _) =>
                  Material(type: MaterialType.transparency, child: child),
              onReorderItem: (from, to) {
                setState(() => _devices.insert(to, _devices.removeAt(from)));
                widget.controller.setOrder([for (final d in _devices) d.id]);
              },
              children: [
                for (final (i, d) in _devices.indexed)
                  ReorderableDragStartListener(
                    key: ValueKey(d.id),
                    index: i,
                    child: Container(
                      margin: const EdgeInsets.symmetric(vertical: 4),
                      padding: const EdgeInsets.symmetric(horizontal: 14, vertical: 12),
                      decoration: BoxDecoration(
                        color: Palette.glass,
                        borderRadius: BorderRadius.circular(18),
                        border: Border.all(color: Palette.glassBorder),
                      ),
                      child: Row(
                        children: [
                          Icon(deviceIcon(d), color: Palette.text, size: 20),
                          const SizedBox(width: 14),
                          Expanded(
                            child: Text(
                              d.name,
                              style: const TextStyle(color: Palette.text, fontSize: 16),
                            ),
                          ),
                          const MouseRegion(
                            cursor: SystemMouseCursors.grab,
                            child: Icon(CupertinoIcons.line_horizontal_3, color: Palette.muted),
                          ),
                        ],
                      ),
                    ),
                  ),
              ],
            ),
          ),
          const SizedBox(height: 10),
          SheetButton(label: 'Done', filled: true, onTap: () => Navigator.of(context).pop()),
        ],
      ),
    );
  }
}

/// Height of what sits under each device's orb: gap, menu, setting line.
const _underOrb = 10.0 + 34 + 22;

/// Scales and fades pages by how far they are from the middle, so the
/// neighbours peek in, smaller.
class _Carousel extends StatelessWidget {
  const _Carousel({required this.pages, required this.index, required this.child});

  final PageController pages;
  final int index;
  final Widget child;

  @override
  Widget build(BuildContext context) {
    return AnimatedBuilder(
      animation: pages,
      builder: (context, child) {
        final page = pages.hasClients && pages.position.haveDimensions
            ? pages.page ?? 0
            : pages.initialPage.toDouble();
        final away = (page - index).abs().clamp(0.0, 1.0);
        return Opacity(
          opacity: 1 - 0.65 * away,
          child: Transform.scale(scale: 1 - 0.3 * away, child: child),
        );
      },
      child: Center(child: child),
    );
  }
}

/// One device as a big orb, with what's happening with it.
class _DevicePage extends StatefulWidget {
  const _DevicePage({
    required this.device,
    required this.focused,
    required this.dragging,
    required this.transfers,
    required this.size,
    required this.onTap,
    required this.onMenu,
    required this.onAway,
  });

  final DeviceInfo device;
  final bool focused;
  final bool dragging;
  final List<Transfer> transfers;
  final double size;
  final VoidCallback onTap;
  final VoidCallback onMenu;

  /// The "!" was tapped: say what it means.
  final VoidCallback onAway;

  @override
  State<_DevicePage> createState() => _DevicePageState();
}

class _DevicePageState extends State<_DevicePage> with TickerProviderStateMixin {
  late final _pop = AnimationController(vsync: this, duration: const Duration(milliseconds: 450));
  late final _shake = AnimationController(vsync: this, duration: const Duration(milliseconds: 500));

  /// The last finished transfer already shown; older results aren't news.
  late int? _seen = _last?.id;

  /// Sends already shown as delivered-and-pending.
  late final Set<int> _seenPending = {
    for (final t in widget.transfers)
      if (t.awaitingAnswer) t.id,
  };

  /// A short message after something happens ("Sent", "Declined"), its
  /// colour, and whether it's bad news.
  /// Text, its colour, and the orb's tint (none for neutral news).
  (String, Color, Color?)? _flash;
  Timer? _clear;

  void _show(String text, Color color, {Color? tint, bool bad = false}) {
    _flash = (text, color, tint);
    if (bad) {
      _shake.forward(from: 0);
      HapticFeedback.heavyImpact();
    } else {
      _pop.forward(from: 0);
      tapFeedback();
    }
    _clear?.cancel();
    _clear = Timer(const Duration(milliseconds: 2600), () {
      if (mounted) setState(() => _flash = null);
    });
  }

  Transfer? get _last {
    Transfer? last;
    for (final t in widget.transfers) {
      if (t.deviceId != widget.device.id || t.finishedAt == null) continue;
      if (last == null || t.finishedAt!.isAfter(last.finishedAt!)) last = t;
    }
    return last;
  }

  @override
  void didUpdateWidget(_DevicePage old) {
    super.didUpdateWidget(old);
    // All there, and they're being asked: that's "sent" from here.
    for (final t in widget.transfers) {
      if (t.deviceId == widget.device.id && t.awaitingAnswer && _seenPending.add(t.id)) {
        // Neutral: it's with them, but they haven't said yes.
        _show('Sent · waiting for an answer', Palette.text);
        return;
      }
    }
    final last = _last;
    if (last == null || last.id == _seen) return;
    _seen = last.id;
    if (last.state == TransferState.failed) {
      _show(last.outcome, Palette.bad, tint: Palette.bad, bad: true);
    } else if (last.incoming) {
      _show('Received', Palette.good, tint: Palette.good);
    } else {
      _show(last.pending ? 'Accepted' : 'Sent', Palette.good, tint: Palette.good);
    }
  }

  @override
  void dispose() {
    _clear?.cancel();
    _pop.dispose();
    _shake.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final d = widget.device;
    final running = [
      for (final t in widget.transfers)
        if (t.deviceId == d.id && t.active) t,
    ];
    final waiting = widget.transfers.any((t) => t.deviceId == d.id && t.awaitingAnswer);
    final total = running.fold<int>(0, (s, t) => s + t.size);
    final done = running.fold<int>(0, (s, t) => s + t.bytes);
    final progress = running.isEmpty || done == 0 ? null : done / math.max(total, 1);
    final away = (d.paired || d.isBrowser) && !d.online;
    final flash = running.isEmpty ? _flash : null;

    // Going over something faster than Wi-Fi: say so.
    final via = running
        .map((t) => t.via)
        .whereType<String>()
        .where((v) => v != 'Wi-Fi')
        .firstOrNull;
    final (String sub, Color? subColor) = switch (()) {
      _ when running.isNotEmpty => (
        '${running.every((t) => t.incoming) ? 'Receiving' : 'Sending'}'
            '${progress == null ? '…' : ' ${(progress * 100).floor()}%'}'
            '${via == null ? '' : ' · $via'}',
        null,
      ),
      _ when flash != null => (flash.$1, flash.$2),
      _ when waiting => ('Sent · waiting for an answer', Palette.muted),
      _ when !d.paired && !d.isBrowser => ('Tap to pair', Palette.accent),
      _ when away => ('', null),
      _ when widget.dragging && widget.focused => ('Drop to send', Palette.text),
      _ => (d.isBrowser ? 'In a browser' : 'Tap to send', null),
    };

    final orb = Orb(
      size: widget.size,
      icon: deviceIcon(d),
      label: d.name,
      sublabel: sub.isEmpty ? null : sub,
      sublabelColor: subColor,
      // Not nearby: a "!" instead of words; tap it to find out more.
      badge: away && running.isEmpty && flash == null && !waiting
          ? Tooltip(
              message: "Not nearby: tap to find out more",
              child: Pressable(
                onTap: widget.onAway,
                child: Container(
                  width: 30,
                  height: 30,
                  decoration: const BoxDecoration(color: Palette.warn, shape: BoxShape.circle),
                  child: const Icon(
                    CupertinoIcons.exclamationmark,
                    size: 17,
                    color: Palette.bottom,
                  ),
                ),
              ),
            )
          : null,
      progress: progress,
      pulse: running.isNotEmpty || (widget.dragging && widget.focused),
      breathe: widget.focused,
      dim: away && running.isEmpty && flash == null && !waiting,
      tint: flash?.$3,
      onTap: widget.onTap,
      onLongPress: d.paired ? widget.onMenu : null,
    );

    return Column(
      mainAxisSize: MainAxisSize.min,
      children: [
        AnimatedBuilder(
          animation: Listenable.merge([_pop, _shake]),
          builder: (context, child) {
            final pop = 1 + 0.08 * math.sin(math.pi * _pop.value);
            final shake = math.sin(_shake.value * math.pi * 6) * 10 * (1 - _shake.value);
            return Transform.translate(
              offset: Offset(shake, 0),
              child: Transform.scale(scale: pop, child: child),
            );
          },
          child: orb,
        ),
        const SizedBox(height: 10),
        // The menu for a paired device, centred under it, and whether it
        // takes our files without asking. Same height either way, so
        // nothing jumps.
        SizedBox(
          height: 34,
          child: AnimatedOpacity(
            opacity: widget.focused && d.paired ? 1 : 0,
            duration: const Duration(milliseconds: 250),
            child: d.paired
                ? Tooltip(
                    message: 'More',
                    child: Pressable(
                      onTap: widget.onMenu,
                      child: Container(
                        width: 44,
                        height: 34,
                        decoration: BoxDecoration(
                          borderRadius: BorderRadius.circular(17),
                          color: Palette.glass,
                          border: Border.all(color: Palette.glassBorder),
                        ),
                        child: const Icon(CupertinoIcons.ellipsis, size: 16, color: Palette.text),
                      ),
                    ),
                  )
                : null,
          ),
        ),
        SizedBox(
          height: 22,
          child: AnimatedOpacity(
            opacity: widget.focused && d.paired && d.acceptsFromUs != null ? 1 : 0,
            duration: const Duration(milliseconds: 250),
            child: Padding(
              padding: const EdgeInsets.only(top: 6),
              child: Row(
                mainAxisSize: MainAxisSize.min,
                children: [
                  Icon(
                    d.acceptsFromUs == true
                        ? CupertinoIcons.checkmark_shield
                        : CupertinoIcons.hand_raised,
                    size: 13,
                    color: Palette.muted,
                  ),
                  const SizedBox(width: 5),
                  Text(
                    d.acceptsFromUs == true
                        ? 'Accepts your files automatically'
                        : 'Asks before accepting your files',
                    style: Fonts.small.copyWith(fontSize: 12),
                  ),
                ],
              ),
            ),
          ),
        ),
      ],
    );
  }
}

/// The last page: pair another device.
class _AddPage extends StatelessWidget {
  const _AddPage({required this.size, required this.focused, required this.onTap});

  final double size;
  final bool focused;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    return Column(
      mainAxisSize: MainAxisSize.min,
      children: [
        Orb(
          size: size,
          icon: CupertinoIcons.add,
          label: 'Add device',
          sublabel: 'Pair with a code',
          breathe: focused,
          onTap: onTap,
        ),
        const SizedBox(height: 44),
      ],
    );
  }
}

class _Arrow extends StatelessWidget {
  const _Arrow({required this.icon, required this.visible, required this.onTap});

  final IconData icon;
  final bool visible;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    return IgnorePointer(
      ignoring: !visible,
      child: AnimatedOpacity(
        opacity: visible ? 1 : 0,
        duration: const Duration(milliseconds: 200),
        child: GlassIconButton(icon: icon, tooltip: '', onTap: onTap),
      ),
    );
  }
}

/// Which page you're on, iOS style: the current one is a short bar.
class _PageDots extends StatelessWidget {
  const _PageDots({required this.count, required this.index, required this.onTap});

  final int count;
  final int index;
  final ValueChanged<int> onTap;

  @override
  Widget build(BuildContext context) {
    if (count <= 1) return const SizedBox(height: 24);
    return SizedBox(
      height: 24,
      child: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          for (var i = 0; i < count; i++)
            // A bigger target than the dot itself, so it's easy to hit.
            MouseRegion(
              cursor: SystemMouseCursors.click,
              child: GestureDetector(
                behavior: HitTestBehavior.opaque,
                onTap: () {
                  tapFeedback();
                  onTap(i);
                },
                child: Padding(
                  padding: const EdgeInsets.symmetric(horizontal: 4, vertical: 9),
                  child: AnimatedContainer(
                    duration: const Duration(milliseconds: 300),
                    curve: Curves.easeOutCubic,
                    width: i == index ? 18 : 6,
                    height: 6,
                    decoration: BoxDecoration(
                      color: i == index ? Palette.text : Palette.faint,
                      borderRadius: BorderRadius.circular(3),
                    ),
                  ),
                ),
              ),
            ),
        ],
      ),
    );
  }
}

/// The gap between the device on screen and this one. Dots flow up while
/// sending ([direction] -1) and down while receiving (1).
class _FlowGap extends StatefulWidget {
  const _FlowGap({required this.direction});

  final int? direction;

  @override
  State<_FlowGap> createState() => _FlowGapState();
}

class _FlowGapState extends State<_FlowGap> with SingleTickerProviderStateMixin {
  late final _t = AnimationController(vsync: this, duration: const Duration(milliseconds: 1100));

  @override
  void initState() {
    super.initState();
    _sync();
  }

  @override
  void didUpdateWidget(_FlowGap old) {
    super.didUpdateWidget(old);
    _sync();
  }

  /// Only tick while something's moving.
  void _sync() {
    if (widget.direction == null) {
      _t.stop();
    } else if (!_t.isAnimating) {
      _t.repeat();
    }
  }

  @override
  void dispose() {
    _t.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    return SizedBox(
      height: 44,
      width: 24,
      child: widget.direction == null
          ? null
          : RepaintBoundary(child: CustomPaint(painter: _FlowPainter(_t, widget.direction!))),
    );
  }
}

class _FlowPainter extends CustomPainter {
  _FlowPainter(this.t, this.direction) : super(repaint: t);

  final Animation<double> t;
  final int direction;

  static const _dots = 3;

  @override
  void paint(Canvas canvas, Size size) {
    final paint = Paint();
    final top = Offset(size.width / 2, 2);
    final bottom = Offset(size.width / 2, size.height - 2);
    final (from, to) = direction > 0 ? (top, bottom) : (bottom, top);
    for (var k = 0; k < _dots; k++) {
      final p = (t.value + k / _dots) % 1;
      // Fade in at the start and out at the end.
      final alpha = math.sin(math.pi * p);
      paint.color = Palette.text.withValues(alpha: 0.8 * alpha);
      canvas.drawCircle(Offset.lerp(from, to, p)!, 2 + 1.5 * alpha, paint);
    }
  }

  @override
  bool shouldRepaint(_FlowPainter old) => old.direction != direction;
}

/// This device, small, under the big orb. Rings while receiving; pops
/// green when something arrives.
class _Self extends StatefulWidget {
  const _Self({
    required this.onTap,
    required this.name,
    required this.progress,
    required this.busy,
    required this.arrivals,
  });

  final VoidCallback onTap;
  final String name;
  final double? progress;
  final bool busy;
  final int arrivals;

  @override
  State<_Self> createState() => _SelfState();
}

class _SelfState extends State<_Self> with SingleTickerProviderStateMixin {
  late final _pop = AnimationController(vsync: this, duration: const Duration(milliseconds: 700));

  @override
  void didUpdateWidget(_Self old) {
    super.didUpdateWidget(old);
    if (old.arrivals != widget.arrivals) _pop.forward(from: 0);
  }

  @override
  void dispose() {
    _pop.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    const size = 56.0;
    return Pressable(
      onTap: widget.onTap,
      child: Column(
        mainAxisSize: MainAxisSize.min,
        children: [
          AnimatedBuilder(
            animation: _pop,
            builder: (context, child) {
              final v = _pop.value;
              final green = _pop.isAnimating ? math.sin(math.pi * v) : 0.0;
              return Transform.scale(
                scale: 1 + 0.18 * math.sin(math.pi * v),
                child: Stack(
                  alignment: Alignment.center,
                  children: [
                    Container(
                      width: size,
                      height: size,
                      decoration: BoxDecoration(
                        shape: BoxShape.circle,
                        color: Color.lerp(
                          Colors.white.withValues(alpha: 0.12),
                          Palette.good.withValues(alpha: 0.35),
                          green,
                        ),
                        border: Border.all(color: Palette.glassBorder),
                      ),
                      child: child,
                    ),
                    if (widget.busy) ProgressRing(value: widget.progress, size: size + 8),
                  ],
                ),
              );
            },
            child: Icon(
              isPhone ? CupertinoIcons.device_phone_portrait : CupertinoIcons.device_laptop,
              color: Palette.text,
              size: 22,
            ),
          ),
          const SizedBox(height: 6),
          const Text(
            'THIS DEVICE',
            style: TextStyle(
              color: Palette.faint,
              fontSize: 10,
              fontWeight: FontWeight.w600,
              letterSpacing: 1.2,
            ),
          ),
          const SizedBox(height: 2),
          Text(
            'Seen by others as "${widget.name}"',
            maxLines: 1,
            overflow: TextOverflow.ellipsis,
            style: Fonts.small,
          ),
        ],
      ),
    );
  }
}

IconData deviceIcon(DeviceInfo d) => switch (d.platform) {
  'android' || 'ios' => CupertinoIcons.device_phone_portrait,
  'windows' || 'linux' || 'macos' => CupertinoIcons.device_laptop,
  'browser' => CupertinoIcons.globe,
  // Not seen yet this session, so the kind of device isn't known.
  _ => CupertinoIcons.dot_radiowaves_left_right,
};

/// The latest transfer, as a small pill. Tap for all of them.
/// Once nothing is moving (finished, or waiting on someone's answer), the
/// pill fades away after a few seconds; the transfer stays in the Activity
/// sheet. A later change (say, accepted) shows it briefly again.
class _ActivityPill extends StatefulWidget {
  const _ActivityPill({required this.controller});

  final NudgeController controller;

  @override
  State<_ActivityPill> createState() => _ActivityPillState();
}

class _ActivityPillState extends State<_ActivityPill> {
  static const _linger = Duration(seconds: 4);

  Timer? _timer;
  String? _timerFor;
  String? _hidden;

  NudgeController get controller => widget.controller;

  @override
  void dispose() {
    _timer?.cancel();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    var t = controller.transfers.firstOrNull;
    final key = t == null ? null : '${t.id}-${t.state}-${t.outcome}';
    // Shown while bytes move. Once they stop (done, failed, or waiting for
    // someone to accept), it lingers briefly; the Activity sheet keeps it.
    final settled = t != null && !t.active;
    if (!settled) {
      _timer?.cancel();
      _timerFor = null;
    } else if (_timerFor != key) {
      _timer?.cancel();
      _timerFor = key;
      _timer = Timer(_linger, () {
        if (mounted) setState(() => _hidden = key);
      });
    }
    if (key == _hidden) t = null;
    return AnimatedSwitcher(
      duration: const Duration(milliseconds: 350),
      transitionBuilder: (child, a) => FadeTransition(
        opacity: a,
        child: SlideTransition(
          position: Tween(begin: const Offset(0, 0.4), end: Offset.zero).animate(a),
          child: child,
        ),
      ),
      child: t == null
          ? const SizedBox(height: 52, key: ValueKey('none'))
          : Pressable(
              key: ValueKey(key),
              onTap: () => showActivity(context, controller),
              child: ConstrainedBox(
                constraints: const BoxConstraints(maxWidth: 360),
                child: Glass(
                  radius: 26,
                  padding: const EdgeInsets.fromLTRB(8, 8, 18, 8),
                  child: Row(
                    mainAxisSize: MainAxisSize.min,
                    children: [
                      _TransferIcon(transfer: t, size: 36),
                      const SizedBox(width: 10),
                      Flexible(
                        child: Text(
                          t.fileName,
                          maxLines: 1,
                          overflow: TextOverflow.ellipsis,
                          style: Fonts.label,
                        ),
                      ),
                    ],
                  ),
                ),
              ),
            ),
    );
  }
}

class _TransferIcon extends StatelessWidget {
  const _TransferIcon({required this.transfer, required this.size});

  final Transfer transfer;
  final double size;

  @override
  Widget build(BuildContext context) {
    final t = transfer;
    final (icon, color) = switch (t.state) {
      _ when t.awaitingAnswer || t.waitingForYou => (CupertinoIcons.hourglass, Palette.warn),
      TransferState.done => (CupertinoIcons.checkmark_alt, Palette.good),
      TransferState.failed => (
        t.error == 'they declined' ? CupertinoIcons.xmark : CupertinoIcons.exclamationmark,
        Palette.bad,
      ),
      TransferState.running => (
        t.incoming ? CupertinoIcons.arrow_down : CupertinoIcons.arrow_up,
        Palette.text,
      ),
    };
    return SizedBox.square(
      dimension: size,
      child: Stack(
        alignment: Alignment.center,
        children: [
          Container(
            width: size,
            height: size,
            decoration: BoxDecoration(shape: BoxShape.circle, color: color.withValues(alpha: 0.14)),
            child: Icon(icon, size: size * 0.45, color: color),
          ),
          if (t.active) ProgressRing(value: t.bytes == 0 ? null : t.fraction, size: size),
        ],
      ),
    );
  }
}

/// Every recent transfer, in a sheet.
Future<void> showActivity(BuildContext context, NudgeController controller) {
  return showGlassSheet<void>(
    context,
    (context) => ListenableBuilder(
      listenable: controller,
      builder: (context, _) {
        final transfers = controller.transfers.take(30).toList();
        return Padding(
          padding: const EdgeInsets.fromLTRB(20, 12, 12, 16),
          child: Column(
            mainAxisSize: MainAxisSize.min,
            children: [
              Row(
                children: [
                  const Expanded(child: Text('Activity', style: Fonts.title)),
                  if (transfers.any((t) => t.state != TransferState.running))
                    CupertinoButton(
                      onPressed: controller.clearFinished,
                      child: const Text('Clear', style: TextStyle(color: Palette.accent)),
                    ),
                ],
              ),
              const SizedBox(height: 8),
              if (transfers.isEmpty)
                const Padding(
                  padding: EdgeInsets.symmetric(vertical: 40),
                  child: Text('Nothing yet', style: Fonts.body),
                )
              else
                Flexible(
                  child: ListView(
                    shrinkWrap: true,
                    physics: const BouncingScrollPhysics(),
                    children: [
                      for (final t in transfers)
                        // Swipe a finished one away (or tap its ×).
                        t.state == TransferState.running
                            ? _TransferRow(transfer: t, controller: controller)
                            : Dismissible(
                                key: ValueKey('transfer-${t.id}'),
                                direction: DismissDirection.endToStart,
                                onDismissed: (_) => controller.removeTransfer(t),
                                background: Container(
                                  alignment: Alignment.centerRight,
                                  padding: const EdgeInsets.only(right: 16),
                                  child: const Icon(
                                    CupertinoIcons.trash,
                                    color: Palette.bad,
                                    size: 20,
                                  ),
                                ),
                                child: _TransferRow(transfer: t, controller: controller),
                              ),
                    ],
                  ),
                ),
            ],
          ),
        );
      },
    ),
  );
}

class _TransferRow extends StatelessWidget {
  const _TransferRow({required this.transfer, required this.controller});

  final Transfer transfer;
  final NudgeController controller;

  @override
  Widget build(BuildContext context) {
    final t = transfer;
    final device = controller.deviceById(t.deviceId);
    final peer = t.peerName ?? device?.name ?? 'device';
    final detail = switch (t.state) {
      _ when t.awaitingAnswer => 'Sent · waiting for $peer to accept or decline',
      _ when t.waitingForYou => 'Here · accept to keep it',
      TransferState.running => [
        if (t.accepted && t.pending) 'Accepted',
        '${t.incoming ? 'From' : 'To'} $peer',
        if (t.bytesPerSec > 0) formatSpeed(t.bytesPerSec),
        if (t.remaining case final left?) '${formatDuration(left)} left',
        if (t.via case final via?) 'via $via',
      ].join(' · '),
      TransferState.done =>
        '${t.outcome} · ${t.incoming ? 'from' : 'to'} $peer · ${formatSize(t.size)}',
      TransferState.failed => '${t.outcome} · ${t.incoming ? 'from' : 'to'} $peer',
    };
    final path = t.path;
    return Padding(
      padding: const EdgeInsets.symmetric(vertical: 6),
      child: Row(
        children: [
          _TransferIcon(transfer: t, size: 40),
          const SizedBox(width: 12),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(
                  t.fileName,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: const TextStyle(color: Palette.text, fontSize: 15),
                ),
                const SizedBox(height: 2),
                Text(
                  detail,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: Fonts.small.copyWith(
                    color: t.state == TransferState.failed
                        ? Palette.bad
                        : t.awaitingAnswer || t.waitingForYou
                        ? Palette.warn
                        : Palette.muted,
                  ),
                ),
              ],
            ),
          ),
          if (t.state == TransferState.done && t.incoming && path != null)
            CupertinoButton(
              padding: const EdgeInsets.symmetric(horizontal: 12),
              onPressed: () => OpenFilex.open(path),
              child: const Text('Open', style: TextStyle(color: Palette.accent, fontSize: 15)),
            ),
          if (t.state != TransferState.running)
            Tooltip(
              message: 'Remove',
              child: CupertinoButton(
                padding: const EdgeInsets.symmetric(horizontal: 8),
                minimumSize: const Size(32, 32),
                onPressed: () => controller.removeTransfer(t),
                child: const Icon(CupertinoIcons.xmark, size: 16, color: Palette.muted),
              ),
            ),
        ],
      ),
    );
  }
}
