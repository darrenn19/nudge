import 'dart:async';
import 'dart:io';
import 'dart:math' as math;
import 'dart:ui' show PointerDeviceKind;

import 'package:desktop_drop/desktop_drop.dart';
import 'package:flutter/cupertino.dart';
import 'package:flutter/foundation.dart' show ValueListenable;
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

  /// For the transfer stream: the device on screen, this device, and what
  /// lies between them.
  final _orbKey = GlobalKey();
  final _selfKey = GlobalKey();
  final _occluders = <GlobalKey>{};

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
    // Open on the device "Send latest" goes to.
    _focusId = controller.sendTarget?.id ?? _targets.firstOrNull?.id;
    WidgetsBinding.instance.addPostFrameCallback((_) => _adoptFocus());
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

  /// Paired devices, then open browser pages. Unpaired devices nearby
  /// aren't here (any Nudge on the network would show up): they're on the
  /// pairing screen.
  List<DeviceInfo> get _targets => [
    ...controller.pairedDevices,
    ...controller.browserDevices,
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

  /// Nudge is moving the pages itself (say, to show who's sending): that
  /// doesn't change the current device. Only the user's own moves do.
  bool _moving = false;

  /// Show page [i]. With [byUser], the device there becomes current.
  Future<void> _moveTo(int i, {required bool byUser, int ms = 420}) async {
    if (!_pages.hasClients) return;
    _moving = true;
    try {
      await _pages.animateToPage(
        i,
        duration: Duration(milliseconds: ms),
        curve: Curves.easeOutCubic,
      );
    } finally {
      _moving = false;
    }
    if (byUser) _makeCurrent(i);
  }

  /// The user chose page [i] (a swipe, arrow or dot).
  void _makeCurrent(int i) {
    final targets = _targets;
    if (i >= 0 && i < targets.length) controller.setCurrent(targets[i]);
  }

  /// No current device yet (first run, or it was forgotten): the one on
  /// screen is it.
  void _adoptFocus() {
    final d = _focus;
    if (mounted && controller.sendTarget == null && d != null) controller.setCurrent(d);
  }

  void _goTo(String deviceId) {
    final i = _targets.indexWhere((d) => d.id == deviceId);
    if (i >= 0) _moveTo(i, byUser: false, ms: 450);
  }

  void _step(int by) {
    if (!_pages.hasClients) return;
    final next = ((_pages.page ?? 0).round() + by).clamp(0, _targets.length);
    _moveTo(next, byUser: true, ms: 380);
  }

  Future<void> _openPairing() async {
    final device = await Navigator.of(context).push<DeviceInfo>(
      nudgeRoute((_) => PairScreen(controller: controller, pairNearby: _askNearby)),
    );
    if (device != null && mounted) {
      showToast(context, 'Paired with ${device.name}');
      _focusId = device.id;
      controller.setCurrent(device);
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

  /// From the pairing screen: pair with a device nearby there (the sheet
  /// opens over it; a code is already on screen behind).
  Future<DeviceInfo?> _askNearby(BuildContext context, DeviceInfo d) => showGlassSheet<DeviceInfo>(
    context,
    (_) => _PairingSheet(controller: controller, device: d, onUseCode: () {}),
  );

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
      controller.setCurrent(paired);
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
    final instant = await instantClipboard();
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
        if (instant != null)
          MenuItem(
            label:
                'Instant clipboard: ${instant.overlay && instant.logs && instant.running ? 'On' : 'Set up'}',
            icon: CupertinoIcons.doc_on_clipboard,
            onTap: () => unawaited(_setUpInstantClipboard()),
          ),
        if (controller.pairedDevices.length > 1)
          MenuItem(
            label: 'Default device: ${controller.defaultDevice?.name ?? 'None'}',
            icon: CupertinoIcons.pin,
            onTap: _chooseDefault,
          ),
      ],
    );
  }

  /// What the current or default device is for, on this platform.
  String _targetMeaning(String name, {required bool isDefault}) {
    if (isPhone) {
      return isDefault
          ? '"Send latest" always sends to $name, and Nudge opens on it.'
          : '"Send latest" sends to $name when more than one device is nearby. '
                'It\'s the device you last swiped to.';
    }
    return isDefault
        ? 'Nudge always opens on $name.'
        : 'Nudge opens on $name, the device you last chose.';
  }

  /// Tapped the "Current device" or "Default device" mark.
  void _aboutTarget(DeviceInfo d) {
    final isDefault = controller.defaultDevice?.id == d.id;
    showMenuSheet(
      context,
      title: isDefault ? 'Default device' : 'Current device',
      subtitle: _targetMeaning(d.name, isDefault: isDefault),
      items: [
        if (isDefault)
          MenuItem(
            label: 'Stop using as default',
            icon: CupertinoIcons.pin_slash,
            onTap: () => controller.setDefault(null),
          )
        else
          MenuItem(
            label: 'Always use ${d.name}',
            icon: CupertinoIcons.pin,
            onTap: () => controller.setDefault(d),
          ),
      ],
    );
  }

  /// Someone on the web page: what they can be sent, and that it's only for
  /// now. No "always accept" or clipboard sync: they're never paired.
  void _browserMenu(DeviceInfo d) {
    if (DateTime.now().isBefore(_quietUntil)) return;
    showMenuSheet(
      context,
      title: '${d.name} · temporary',
      subtitle:
          'Connected through the web page, not paired. It disappears soon after '
          'they close the page. Anything they send asks you first.',
      items: [
        if (d.online) ...[
          MenuItem(label: 'Send files', icon: CupertinoIcons.paperplane, onTap: () => _sendTo(d)),
          MenuItem(
            label: 'Send clipboard',
            icon: CupertinoIcons.doc_on_clipboard,
            onTap: () => unawaited(controller.sendClipboardTo(d)),
          ),
        ],
        MenuItem(
          label: 'Stop sharing without the app',
          icon: CupertinoIcons.stop_circle,
          destructive: true,
          onTap: controller.stopWebShare,
        ),
      ],
    );
  }

  /// Pick the default device, or none (then the current device is used).
  void _chooseDefault() {
    final chosen = controller.defaultDevice;
    IconData mark(bool on) => on ? CupertinoIcons.checkmark_circle_fill : CupertinoIcons.circle;
    showMenuSheet(
      context,
      title: 'Default device',
      subtitle: isPhone
          ? '"Send latest" always sends here. With none, it sends to the current device: '
                'the one you last swiped to.'
          : 'Nudge opens on it. With none, it opens on the device you last chose.',
      items: [
        MenuItem(
          label: 'None (use the current device)',
          icon: mark(chosen == null),
          onTap: () => controller.setDefault(null),
        ),
        for (final d in controller.pairedDevices)
          MenuItem(
            label: d.name,
            icon: mark(chosen?.id == d.id),
            onTap: () => controller.setDefault(d),
          ),
      ],
    );
  }

  /// Android: what copying on the phone sending at once (Nudge in the
  /// background) needs. Android doesn't allow it normally, so two
  /// permissions: one a switch, one only grantable from a computer, once.
  Future<void> _setUpInstantClipboard() async {
    final s = await instantClipboard();
    if (s == null || !mounted) return;
    final on = s.overlay && s.logs && s.running;
    IconData mark(bool done) => done ? CupertinoIcons.checkmark_circle_fill : CupertinoIcons.circle;
    await showMenuSheet(
      context,
      title: 'Instant clipboard',
      subtitle: on
          ? 'On: whatever you copy goes straight to your devices that share the clipboard, '
                'with Nudge in the background. Text only; images and private text '
                '(like passwords) stay here.'
          : 'Android only lets the app on screen read the clipboard. Nudge gets around it '
                'by noticing copies in the system log, then taking focus for a split second '
                'with an invisible window to read them.',
      items: [
        MenuItem(
          label: '1. Allow "Appear on top"',
          icon: mark(s.overlay),
          onTap: () => unawaited(openOverlaySettings()),
        ),
        MenuItem(
          label: s.logs ? '2. Log permission: granted' : '2. Copy the command for a computer',
          icon: mark(s.logs),
          onTap: s.logs
              ? null
              : () async {
                  await Clipboard.setData(const ClipboardData(text: instantClipboardCommand));
                  if (mounted) {
                    showToast(
                      context,
                      'Copied. With the phone plugged into a computer (USB debugging on), '
                      'run it there once',
                    );
                  }
                },
        ),
        if (s.overlay && s.logs)
          MenuItem(
            label: s.running ? '3. Log access: allowed' : '3. Allow log access',
            icon: mark(s.running),
            onTap: s.running
                ? null
                : () {
                    unawaited(startInstantClipboard());
                    showToast(context, 'Tap "Allow" when Android asks about device logs');
                  },
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

  /// Windows: see [d]'s screen; it opens in a window of its own. Paired
  /// phones share without a question, so nothing to say unless it fails.
  Future<void> _mirror(DeviceInfo d) async {
    final problem = await controller.mirror(d);
    if (problem != null && mounted) {
      showToast(context, "Couldn't show ${d.name}'s screen: $problem", icon: CupertinoIcons.xmark_circle);
    }
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
        // Screen mirroring: a PC sees a phone's screen (its user is asked);
        // a phone shows its own on a PC.
        // While a screen is shared with it (either way): stop instead.
        if (controller.isMirroring(d))
          MenuItem(
            label: 'Stop sharing',
            icon: CupertinoIcons.stop_circle,
            destructive: true,
            onTap: () => controller.stopMirroring(d),
          )
        else if (!isPhone && d.platform == 'android')
          MenuItem(
            label: 'Show its screen',
            icon: CupertinoIcons.device_phone_portrait,
            onTap: () => unawaited(_mirror(d)),
          )
        else if (isPhone && d.platform == 'windows')
          MenuItem(
            label: 'Show my screen on ${d.name}',
            icon: CupertinoIcons.rectangle_on_rectangle,
            onTap: () => unawaited(showScreenOn(d.id)),
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
            label: 'Default device',
            icon: CupertinoIcons.pin,
            toggle: controller.defaultDevice?.id == d.id,
            onToggle: (on) => controller.setDefault(on ? d : null),
          ),
        if (controller.pairedDevices.length > 1)
          MenuItem(
            label: 'Arrange devices',
            icon: CupertinoIcons.arrow_up_arrow_down,
            onTap: _arrange,
          ),
        // Disconnect: gone from here for now, back from the pairing screen
        // with no code. Unpair: forgotten; pairing again needs a code.
        MenuItem(
          label: 'Disconnect',
          icon: CupertinoIcons.link,
          onTap: () {
            controller.disconnect(d);
            showToast(context, '${d.name} disconnected. Reconnect it from Pair.', icon: CupertinoIcons.link);
          },
          beside: MenuItem(
            label: 'Unpair',
            icon: CupertinoIcons.trash,
            destructive: true,
            onTap: () async {
              final ok = await confirm(
                context,
                title: 'Unpair ${d.name}?',
                message: "It's forgotten: you'll need to pair again with a code to send between these devices.",
                action: 'Unpair',
                destructive: true,
              );
              if (ok) controller.unpair(d);
            },
          ),
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
        _moving = true;
        if ((_pages.page ?? 0).round() != target) _pages.jumpToPage(target);
        _moving = false;
        if (keep < 0) setState(() => _focusId = targets.firstOrNull?.id);
        _adoptFocus();
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

    return _FlowScope(
      orb: _orbKey,
      self: _selfKey,
      occluders: _occluders,
      child: Stack(
        children: [
          // Behind everything: the dots between the device and this one.
          Positioned.fill(
            child: _FlowStream(
              // Sending goes up to the device; receiving comes down to this one.
              direction: linked.isEmpty ? null : (linked.first.incoming ? 1 : -1),
            ),
          ),
          Column(
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
                        .min(box.maxWidth * 0.74 - 24, box.maxHeight - _aboveOrb - _underOrb - 8)
                        .clamp(140.0, 330.0);
                    // Marked only when there's more than one device to choose from.
                    final target = controller.pairedDevices.length > 1
                        ? controller.sendTarget
                        : null;
                    final isDefault = target != null && target.id == controller.defaultDevice?.id;
                    return Stack(
                      alignment: Alignment.center,
                      children: [
                        ScrollConfiguration(
                          // Swipe with a mouse too, not just touch.
                          behavior: ScrollConfiguration.of(context).copyWith(
                            dragDevices: PointerDeviceKind.values.toSet(),
                            scrollbars: false,
                          ),
                          child: PageView.builder(
                            controller: _pages,
                            itemCount: targets.length + 1,
                            onPageChanged: (i) {
                              tapFeedback();
                              setState(() => _focusId = i < targets.length ? targets[i].id : null);
                              if (!_moving) _makeCurrent(i);
                            },
                            itemBuilder: (context, i) => _Carousel(
                              pages: _pages,
                              index: i,
                              child: i < targets.length
                                  ? _DevicePage(
                                      device: targets[i],
                                      orbKey: targets[i].id == _focusId ? _orbKey : null,
                                      focused: targets[i].id == _focusId,
                                      dragging: _dragging,
                                      transfers: controller.transfers,
                                      size: orb,
                                      mark: targets[i].isBrowser
                                          ? _Mark.temporary
                                          : targets[i].id != target?.id
                                          ? null
                                          : isDefault
                                          ? _Mark.defaultDevice
                                          : _Mark.current,
                                      onMark: () => targets[i].isBrowser
                                          ? _browserMenu(targets[i])
                                          : _aboutTarget(targets[i]),
                                      onTap: () => _tapDevice(targets[i]),
                                      onMenu: () => targets[i].isBrowser
                                          ? _browserMenu(targets[i])
                                          : _deviceMenu(targets[i]),
                                      onAway: () => _explainAway(targets[i]),
                                      onCancel: controller.cancelWith,
                                    )
                                  : _AddPage(
                                      size: orb,
                                      focused: _focusId == null,
                                      onTap: _openPairing,
                                    ),
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
              // Out of the way while files move with the device on screen.
              _Occludes(
                active: linked.isEmpty,
                child: AnimatedOpacity(
                  opacity: linked.isEmpty ? 1 : 0,
                  duration: const Duration(milliseconds: 250),
                  child: _PageDots(
                    onTap: (i) => _moveTo(i, byUser: true),
                    count: targets.length + 1,
                    index: _focusId == null
                        ? targets.length
                        : math.max(0, targets.indexWhere((d) => d.id == _focusId)),
                  ),
                ),
              ),
              const SizedBox(height: 28),
              _Self(
                bubbleKey: _selfKey,
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
          ),
        ],
      ),
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
    // From the web page: only for now, so no "always accept".
    final browser = widget.controller.deviceById(o.deviceId)?.isBrowser ?? false;
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
          if (browser)
            Container(
              padding: const EdgeInsets.fromLTRB(16, 12, 16, 12),
              decoration: BoxDecoration(
                color: Palette.warn.withValues(alpha: 0.1),
                borderRadius: BorderRadius.circular(18),
                border: Border.all(color: Palette.warn.withValues(alpha: 0.35)),
              ),
              child: Row(
                children: [
                  const Icon(CupertinoIcons.clock, size: 18, color: Palette.warn),
                  const SizedBox(width: 12),
                  Expanded(
                    child: Text(
                      'Temporary connection through the web page: not paired, '
                      'so each send is asked about.',
                      style: Fonts.small.copyWith(color: Palette.text),
                    ),
                  ),
                ],
              ),
            )
          else
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
const _underOrb = 10.0 + 34 + 22 + _cableSlot;

/// Height of what sits above it: the current/default device mark and a gap.
const _aboveOrb = 28.0 + 6;

/// Over a device's orb: where "Send latest" (and opening Nudge) goes, or
/// that it's only a web page, here for now.
bool _isComputer(DeviceInfo d) => const {'windows', 'macos', 'linux'}.contains(d.platform);

/// Room under a device for its cable pill, kept whether or not it shows, so
/// nothing moves (or gets cut off) when a cable comes or goes.
const _cableSlot = 44.0;

/// What a cable between this device and [device] means right now, if
/// anything: connected over it, or plugged in with the setting it needs
/// off (USB tethering on the phone), or a cable that only charges.
enum _CableNote { connected, needsTethering, chargeOnly }

_CableNote? _cableNote(DeviceInfo d, UsbCable here) {
  if (d.cable == 'connected') return _CableNote.connected;
  if (!d.paired) return null;
  // On the phone, its own view of the cable answers at once; the core's
  // follows a moment later.
  final computer = isPhone && _isComputer(d);
  if (d.cable == 'tethering' || (computer && here.couldUse)) return _CableNote.needsTethering;
  if (computer && here.chargeOnly) return _CableNote.chargeOnly;
  return null;
}

/// A cable between two devices, said under the other one: "Connected by
/// cable" while files can use it, or what to switch on when they can't.
/// Files use a cable (any kind that carries data: USB-C, micro-USB, USB-A)
/// once the phone shares a network on it, which only the person can turn
/// on; then Nudge uses it by itself.
class _CablePill extends StatelessWidget {
  const _CablePill({required this.device, required this.note});

  final DeviceInfo device;
  final _CableNote note;

  /// The phone's name, from whichever end this is.
  String get _phone => isPhone ? 'this phone' : device.name;

  void _explain(BuildContext context) {
    final (title, text, items) = switch (note) {
      _CableNote.connected => (
        'Connected by cable',
        device.isBrowser
            ? "This page reached this computer over the phone's cable, so files go that way."
            : 'Files go over the cable whenever it beats Wi-Fi, which is usually by a lot. '
                  'To stop using it, turn off USB tethering on $_phone.',
        [
          if (isPhone)
            MenuItem(
              label: 'Open USB tethering settings',
              icon: CupertinoIcons.settings,
              onTap: () => unawaited(openTetherSettings()),
            ),
        ],
      ),
      _CableNote.needsTethering => (
        'Use the cable every time',
        isPhone
            ? "Files go over the cable once USB tethering is on, often several times faster than "
                  "Wi-Fi. Android turns it off whenever the cable comes out, so set it once to come "
                  'back on by itself: Developer options → Default USB configuration → USB tethering.'
                  '${usbCable.value.developerOptions ? '' : ' Developer options is hidden until you tap Build number 7 times (in About phone → Software information).'}'
                  " While it's on, the computer will likely use this phone's internet instead of its Wi-Fi."
            : 'Files go over the cable once USB tethering is on, but Android turns it off whenever '
                  'the cable comes out. On ${device.name}, set it once to come back on by itself: tap '
                  'the note in Nudge there, or go to Developer options → Default USB configuration → '
                  "USB tethering. While it's on, this computer will likely use the phone's internet "
                  'instead of its Wi-Fi.',
        [
          if (isPhone) ...[
            MenuItem(
              label: usbCable.value.developerOptions
                  ? 'Set it up once (Developer options)'
                  : 'Set it up once (About phone)',
              icon: CupertinoIcons.settings,
              onTap: () => unawaited(openAutoTethering()),
            ),
            MenuItem(
              label: 'Just this time (USB tethering)',
              icon: CupertinoIcons.bolt,
              onTap: () => unawaited(openTetherSettings()),
            ),
          ],
        ],
      ),
      _CableNote.chargeOnly => (
        'This cable only charges',
        "It carries power but no data, so files can't go over it. "
            'Use a cable that carries data: the one that came with the phone usually does, '
            'whether it ends in USB-C, micro-USB or USB-A.',
        <MenuItem>[],
      ),
    };
    showMenuSheet(context, title: title, subtitle: text, items: items);
  }

  @override
  Widget build(BuildContext context) {
    final (text, tone) = switch (note) {
      _CableNote.connected => ('Connected by cable', Palette.good),
      _CableNote.needsTethering => ('Plugged in · turn on USB tethering', Palette.accent),
      _CableNote.chargeOnly => ('This cable only charges', Palette.muted),
    };
    final action = note == _CableNote.needsTethering;
    return Tooltip(
      message: action ? 'How to send over the cable' : 'About the cable',
      child: Pressable(
        onTap: () => _explain(context),
        child: Container(
          height: 32,
          constraints: const BoxConstraints(maxWidth: 320),
          // The same on both sides, with the arrow (when there is one) as
          // wide as the icon, so the words sit in the middle.
          padding: const EdgeInsets.symmetric(horizontal: 14),
          decoration: BoxDecoration(
            borderRadius: BorderRadius.circular(16),
            color: Color.alphaBlend(tone.withValues(alpha: 0.12), Palette.glass),
            border: Border.all(color: tone.withValues(alpha: 0.35)),
          ),
          child: Row(
            mainAxisSize: MainAxisSize.min,
            children: [
              if (note == _CableNote.connected)
                Container(
                  width: 8,
                  height: 8,
                  decoration: BoxDecoration(
                    color: tone,
                    shape: BoxShape.circle,
                    boxShadow: [BoxShadow(color: tone.withValues(alpha: 0.6), blurRadius: 8)],
                  ),
                )
              else
                Icon(
                  note == _CableNote.chargeOnly
                      ? CupertinoIcons.battery_25
                      : CupertinoIcons.bolt_fill,
                  size: 14,
                  color: tone,
                ),
              const SizedBox(width: 8),
              Flexible(
                child: Text(
                  text,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: Fonts.label.copyWith(height: 1.1),
                ),
              ),
              if (action) ...[
                const SizedBox(width: 8),
                Icon(CupertinoIcons.chevron_right, size: 14, color: tone),
              ],
            ],
          ),
        ),
      ),
    );
  }
}

enum _Mark { current, defaultDevice, temporary }

/// "Current device" or "Default device", over that device's orb. Tap to
/// find out what it means, or change it.
class _MarkChip extends StatelessWidget {
  const _MarkChip({super.key, required this.mark, required this.onTap});

  final _Mark mark;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final (label, icon, color) = switch (mark) {
      _Mark.current => ('CURRENT DEVICE', CupertinoIcons.scope, Palette.accent),
      _Mark.defaultDevice => ('DEFAULT DEVICE', CupertinoIcons.pin_fill, Palette.accent),
      _Mark.temporary => ('TEMPORARY', CupertinoIcons.clock, Palette.warn),
    };
    return Tooltip(
      message: switch (mark) {
        _Mark.current => 'Current device',
        _Mark.defaultDevice => 'Default device',
        _Mark.temporary => 'Connected through a web page, until it closes',
      },
      child: Pressable(
        onTap: onTap,
        child: Container(
          height: 28,
          padding: const EdgeInsets.symmetric(horizontal: 12),
          decoration: BoxDecoration(
            borderRadius: BorderRadius.circular(14),
            color: color.withValues(alpha: 0.16),
            border: Border.all(color: color.withValues(alpha: 0.45)),
          ),
          child: Row(
            mainAxisSize: MainAxisSize.min,
            children: [
              Icon(icon, size: 13, color: color),
              const SizedBox(width: 6),
              Text(
                label,
                style: TextStyle(
                  color: color,
                  fontSize: 11,
                  fontWeight: FontWeight.w600,
                  letterSpacing: 1.1,
                ),
              ),
            ],
          ),
        ),
      ),
    );
  }
}

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
    this.orbKey,
    required this.focused,
    required this.dragging,
    required this.transfers,
    required this.size,
    required this.mark,
    required this.onMark,
    required this.onTap,
    required this.onMenu,
    required this.onCancel,
    required this.onAway,
  });

  final DeviceInfo device;

  /// Set on the device on screen, for the transfer stream to find.
  final GlobalKey? orbKey;
  final bool focused;
  final bool dragging;
  final List<Transfer> transfers;
  final double size;

  /// Shown over the orb when this is where "Send latest" goes.
  final _Mark? mark;
  final VoidCallback onMark;
  final VoidCallback onTap;
  final VoidCallback onMenu;

  /// Stop what's moving with this device.
  final void Function(DeviceInfo device) onCancel;

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
    if (last.state == TransferState.failed && last.error == 'cancelled') {
      _show('Cancelled', Palette.text);
    } else if (last.state == TransferState.failed) {
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
      _ => ('Tap to send', null),
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
      onLongPress: d.paired || d.isBrowser ? widget.onMenu : null,
    );

    final mark = widget.mark;
    return Column(
      mainAxisSize: MainAxisSize.min,
      children: [
        // Same height with or without the mark, so nothing jumps.
        SizedBox(
          height: 28,
          child: AnimatedSwitcher(
            duration: const Duration(milliseconds: 250),
            child: mark == null
                ? const SizedBox.shrink()
                : _MarkChip(key: ValueKey(mark), mark: mark, onTap: widget.onMark),
          ),
        ),
        const SizedBox(height: 6),
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
          child: widget.orbKey == null ? orb : KeyedSubtree(key: widget.orbKey, child: orb),
        ),
        const SizedBox(height: 10),
        // The menu for a paired device, centred under it, and whether it
        // takes our files without asking. Same height either way, so
        // nothing jumps.
        SizedBox(
          height: 34,
          child: AnimatedOpacity(
            opacity: widget.focused && (d.paired || d.isBrowser) ? 1 : 0,
            duration: const Duration(milliseconds: 250),
            child: running.any((t) => t.cancellable)
                ? _Occludes(
                    child: Tooltip(
                      message: running.every((t) => t.incoming) ? 'Stop receiving' : 'Stop sending',
                      child: Pressable(
                        onTap: () => widget.onCancel(d),
                        child: Container(
                          height: 34,
                          padding: const EdgeInsets.symmetric(horizontal: 14),
                          decoration: BoxDecoration(
                            borderRadius: BorderRadius.circular(17),
                            color: Palette.glass,
                            border: Border.all(color: Palette.glassBorder),
                          ),
                          child: const Row(
                            mainAxisSize: MainAxisSize.min,
                            children: [
                              Icon(CupertinoIcons.xmark, size: 13, color: Palette.text),
                              SizedBox(width: 6),
                              Text('Cancel', style: TextStyle(color: Palette.text, fontSize: 14)),
                            ],
                          ),
                        ),
                      ),
                    ),
                  )
                : d.paired || d.isBrowser
                ? _Occludes(
                    child: Tooltip(
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
                    ),
                  )
                : null,
          ),
        ),
        SizedBox(
          height: 22,
          child: AnimatedOpacity(
            // Out of the way while files move, so the stream between the two
            // devices runs straight.
            opacity:
                widget.focused &&
                    running.isEmpty &&
                    (d.isBrowser || d.paired && d.acceptsFromUs != null)
                ? 1
                : 0,
            duration: const Duration(milliseconds: 250),
            child: Padding(
              padding: const EdgeInsets.only(top: 6),
              child: _Occludes(
                active: running.isEmpty,
                child: Row(
                  mainAxisSize: MainAxisSize.min,
                  children: [
                    Icon(
                      d.isBrowser
                          ? CupertinoIcons.clock
                          : d.acceptsFromUs == true
                          ? CupertinoIcons.checkmark_shield
                          : CupertinoIcons.hand_raised,
                      size: 13,
                      color: Palette.muted,
                    ),
                    const SizedBox(width: 5),
                    Text(
                      d.isBrowser
                          ? 'Not paired · gone when the page closes'
                          : d.acceptsFromUs == true
                          ? 'Accepts your files automatically'
                          : 'Asks before accepting your files',
                      style: Fonts.small.copyWith(fontSize: 12),
                    ),
                  ],
                ),
              ),
            ),
          ),
        ),
        // A cable between the two: connected over it, or what to switch on.
        SizedBox(
          height: _cableSlot,
          child: ValueListenableBuilder<UsbCable>(
            valueListenable: usbCable,
            builder: (context, here, _) {
              final note = _cableNote(d, here);
              return AnimatedSwitcher(
                duration: const Duration(milliseconds: 250),
                child: note == null || !widget.focused || running.isNotEmpty
                    ? const SizedBox.shrink()
                    : Align(
                        key: ValueKey(note),
                        alignment: Alignment.bottomCenter,
                        child: _Occludes(
                          child: _CablePill(device: d, note: note),
                        ),
                      ),
              );
            },
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
        const SizedBox(height: _aboveOrb),
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

/// The circles at either end of a transfer, and what lies between them,
/// for [_FlowStream] to find on screen.
class _FlowScope extends InheritedWidget {
  const _FlowScope({
    required this.orb,
    required this.self,
    required this.occluders,
    required super.child,
  });

  /// The device on screen's circle, and this device's bubble.
  final GlobalKey orb;
  final GlobalKey self;

  /// Things the stream passes behind (buttons, pills, text).
  final Set<GlobalKey> occluders;

  static _FlowScope? of(BuildContext context) =>
      context.dependOnInheritedWidgetOfExactType<_FlowScope>();

  @override
  bool updateShouldNotify(_FlowScope old) => false;
}

/// Something the transfer stream passes behind: dots fade out as they reach
/// it and back in past it, rather than crossing it.
class _Occludes extends StatefulWidget {
  const _Occludes({this.active = true, required this.child});

  /// False while what it wraps is hidden: the stream goes straight past.
  final bool active;
  final Widget child;

  @override
  State<_Occludes> createState() => _OccludesState();
}

class _OccludesState extends State<_Occludes> {
  final _key = GlobalKey();
  _FlowScope? _scope;

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    _scope?.occluders.remove(_key);
    _scope = _FlowScope.of(context);
    _apply();
  }

  @override
  void didUpdateWidget(_Occludes old) {
    super.didUpdateWidget(old);
    _apply();
  }

  void _apply() {
    if (widget.active) {
      _scope?.occluders.add(_key);
    } else {
      _scope?.occluders.remove(_key);
    }
  }

  @override
  void dispose() {
    _scope?.occluders.remove(_key);
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => KeyedSubtree(key: _key, child: widget.child);
}

/// Dots streaming in a straight line between the device on screen and this
/// one: up while sending ([direction] -1), down while receiving (1). Evenly
/// spaced, at a steady speed whatever the distance, fading in as they leave
/// one circle and out as they reach the other.
class _FlowStream extends StatefulWidget {
  const _FlowStream({required this.direction});

  final int? direction;

  @override
  State<_FlowStream> createState() => _FlowStreamState();
}

class _FlowStreamState extends State<_FlowStream> with TickerProviderStateMixin {
  /// Time since the stream started, driving the dots.
  final _clock = ValueNotifier<Duration>(Duration.zero);
  late final _ticker = createTicker((elapsed) => _clock.value = elapsed);

  /// Fades the whole stream in and out as transfers start and stop.
  late final _shown = AnimationController(vsync: this, duration: const Duration(milliseconds: 300));

  /// The direction while fading out, after the transfer has ended.
  int _direction = -1;

  @override
  void initState() {
    super.initState();
    _shown.addStatusListener((s) {
      if (s == AnimationStatus.dismissed) _ticker.stop();
    });
    _sync();
  }

  @override
  void didUpdateWidget(_FlowStream old) {
    super.didUpdateWidget(old);
    _sync();
  }

  void _sync() {
    final direction = widget.direction;
    if (direction == null) {
      _shown.reverse();
      return;
    }
    _direction = direction;
    if (!_ticker.isActive) _ticker.start();
    _shown.forward();
  }

  @override
  void dispose() {
    _ticker.dispose();
    _shown.dispose();
    _clock.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final scope = _FlowScope.of(context);
    if (scope == null) return const SizedBox.shrink();
    return IgnorePointer(
      child: RepaintBoundary(
        child: CustomPaint(
          size: Size.infinite,
          painter: _StreamPainter(
            clock: _clock,
            shown: _shown,
            direction: () => _direction,
            scope: scope,
            canvasBox: () => context.findRenderObject() as RenderBox?,
          ),
        ),
      ),
    );
  }
}

class _StreamPainter extends CustomPainter {
  _StreamPainter({
    required this.clock,
    required this.shown,
    required this.direction,
    required this.scope,
    required this.canvasBox,
  }) : super(repaint: Listenable.merge([clock, shown]));

  final ValueListenable<Duration> clock;
  final Animation<double> shown;
  final int Function() direction;
  final _FlowScope scope;
  final RenderBox? Function() canvasBox;

  /// Logical pixels per second, and between dots.
  static const _speed = 150.0;
  static const _spacing = 22.0;

  /// Distance over which a dot fades in, out, and around what it passes.
  static const _fade = 16.0;

  /// Where `key`'s widget is, in this canvas's coordinates.
  Rect? _rect(GlobalKey key, RenderBox canvas) {
    final box = key.currentContext?.findRenderObject();
    if (box is! RenderBox || !box.attached || !box.hasSize) return null;
    final topLeft = canvas.globalToLocal(box.localToGlobal(Offset.zero));
    final bottomRight = canvas.globalToLocal(box.localToGlobal(box.size.bottomRight(Offset.zero)));
    return Rect.fromPoints(topLeft, bottomRight);
  }

  @override
  void paint(Canvas canvas, Size size) {
    if (shown.value == 0) return;
    final box = canvasBox();
    if (box == null || !box.hasSize) return;
    final orb = _rect(scope.orb, box);
    final self = _rect(scope.self, box);
    if (orb == null || self == null) return;

    // From just inside the orb's outer ring to just short of the bubble.
    final top = Offset(orb.center.dx, orb.bottom - 4);
    final bottom = Offset(self.center.dx, self.top - 6);
    final length = (bottom - top).distance;
    if (length < _spacing * 2) return;
    final (from, to) = direction() > 0 ? (top, bottom) : (bottom, top);

    // What's on the way, as gaps along the line.
    final gaps = <(double, double)>[];
    for (final key in scope.occluders) {
      final r = _rect(key, box);
      if (r == null || r.left > top.dx || r.right < top.dx) continue;
      final a = ((r.top - top.dy) / (bottom.dy - top.dy)).clamp(0.0, 1.0) * length;
      final b = ((r.bottom - top.dy) / (bottom.dy - top.dy)).clamp(0.0, 1.0) * length;
      if (b > a) gaps.add(direction() > 0 ? (a, b) : (length - b, length - a));
    }

    final seconds = clock.value.inMicroseconds / 1e6;
    final offset = (seconds * _speed) % _spacing;
    final paint = Paint();
    for (var s = offset; s < length; s += _spacing) {
      // Fade near the ends, and around anything it passes behind.
      var alpha = math.min(_ease(s / _fade), _ease((length - s) / _fade));
      for (final (a, b) in gaps) {
        if (s > a - _fade && s < b + _fade) {
          final inside = s >= a && s <= b;
          alpha = inside
              ? 0
              : math.min(alpha, _ease(math.min((a - s).abs(), (s - b).abs()) / _fade));
        }
      }
      if (alpha <= 0) continue;
      paint.color = Palette.text.withValues(alpha: 0.85 * alpha * shown.value);
      canvas.drawCircle(Offset.lerp(from, to, s / length)!, 1.6 + 1.2 * alpha, paint);
    }
  }

  /// 0 → 1, smoothly, for x from 0 to 1.
  static double _ease(double x) {
    final t = x.clamp(0.0, 1.0);
    return t * t * (3 - 2 * t);
  }

  @override
  bool shouldRepaint(_StreamPainter old) => true;
}

/// This device, small, under the big orb. Rings while receiving; pops
/// green when something arrives.
class _Self extends StatefulWidget {
  const _Self({
    this.bubbleKey,
    required this.onTap,
    required this.name,
    required this.progress,
    required this.busy,
    required this.arrivals,
  });

  /// On the bubble itself, for the transfer stream to find.
  final GlobalKey? bubbleKey;
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
                      key: widget.bubbleKey,
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
                      if (t.cancellable) ...[
                        const SizedBox(width: 4),
                        Tooltip(
                          message: t.incoming ? 'Stop receiving' : 'Stop sending',
                          child: CupertinoButton(
                            padding: EdgeInsets.zero,
                            minimumSize: const Size(32, 32),
                            onPressed: () => controller.cancel(t!),
                            child: const Icon(CupertinoIcons.xmark, size: 15, color: Palette.muted),
                          ),
                        ),
                      ],
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
        t.error == 'they declined' || t.cancelled
            ? CupertinoIcons.xmark
            : CupertinoIcons.exclamationmark,
        t.error == 'cancelled' ? Palette.muted : Palette.bad,
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
          if (t.cancellable)
            CupertinoButton(
              padding: const EdgeInsets.symmetric(horizontal: 12),
              onPressed: () => controller.cancel(t),
              child: const Text('Cancel', style: TextStyle(color: Palette.bad, fontSize: 15)),
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
