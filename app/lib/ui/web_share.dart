import 'dart:async';

import 'package:flutter/cupertino.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import '../controller.dart';
import '../platform.dart';
import '../src/rust/api/nudge.dart';
import 'send_sheet.dart' show describeFiles;
import 'style.dart';

/// Share with someone who doesn't have Nudge: they scan a code, a page opens
/// in their browser, and while it's open they're a device you can send to.
/// They accept each offer, and can send files back.
class WebShareScreen extends StatefulWidget {
  const WebShareScreen({super.key, required this.controller, this.files});

  final NudgeController controller;

  /// Files to offer to the first person who opens the page (shared with
  /// "Share by QR code", or "Someone without Nudge" in the device list).
  final List<OutgoingFile>? files;

  @override
  State<WebShareScreen> createState() => _WebShareScreenState();
}

class _WebShareScreenState extends State<WebShareScreen> {
  NudgeController get controller => widget.controller;
  // Usually already started (see HomeScreen._openWebShare), so the screen
  // opens with its content rather than swapping it in mid-transition.
  late Future<String> _starting = switch (controller.webShareUrl) {
    final url? => SynchronousFuture(url),
    null => controller.startWebShare(),
  };

  /// Waiting for someone to open the page, then sent to them.
  late List<OutgoingFile>? _waiting = widget.files;
  String? _sentTo;

  /// Pages already open don't count: the files are for whoever scans now.
  late final Set<String> _before;

  @override
  void initState() {
    super.initState();
    _before = {for (final b in controller.browserDevices) b.id};
    controller.addListener(_sendWhenSomeoneArrives);
  }

  @override
  void dispose() {
    controller.removeListener(_sendWhenSomeoneArrives);
    // Left before anyone came: let go of the shared files.
    if (_waiting case final files?) closeUnsent(files);
    super.dispose();
  }

  void _sendWhenSomeoneArrives() {
    final files = _waiting;
    if (files == null || !mounted) return;
    for (final b in controller.browserDevices) {
      if (b.online && !_before.contains(b.id)) {
        controller.send(b, files);
        setState(() {
          _waiting = null;
          _sentTo = b.name;
        });
        return;
      }
    }
  }

  void _restart() => setState(() => _starting = controller.startWebShare());

  Future<void> _send(DeviceInfo device) async {
    final files = await pickFiles();
    if (files.isNotEmpty) controller.send(device, files);
  }

  /// Files or the clipboard, to someone on the page.
  void _choose(DeviceInfo device) {
    showMenuSheet(
      context,
      title: '${device.name} · temporary',
      subtitle: 'On the web page until they close it. Not paired.',
      items: [
        MenuItem(
          label: 'Send files',
          icon: CupertinoIcons.paperplane,
          onTap: () => unawaited(_send(device)),
        ),
        MenuItem(
          label: 'Send clipboard',
          icon: CupertinoIcons.doc_on_clipboard,
          onTap: () => unawaited(controller.sendClipboardTo(device)),
        ),
      ],
    );
  }

  @override
  Widget build(BuildContext context) {
    return NudgePage(
      title: 'Without the app',
      child: FutureBuilder<String>(
        future: _starting,
        builder: (context, snapshot) {
          if (snapshot.hasError) {
            return _Message(
              icon: CupertinoIcons.wifi_slash,
              title: "Couldn't start sharing",
              text: '${snapshot.error}',
              action: PillButton(label: 'Try again', filled: true, onTap: _restart),
            );
          }
          if (!snapshot.hasData) {
            return const Center(child: CupertinoActivityIndicator(color: Palette.text));
          }
          return ListenableBuilder(
            listenable: controller,
            builder: (context, _) {
              final url = controller.webShareUrl;
              return AnimatedSwitcher(
                duration: const Duration(milliseconds: 350),
                child: url == null
                    ? _Message(
                        key: const ValueKey('stopped'),
                        icon: CupertinoIcons.pause_circle,
                        title: 'Sharing stopped',
                        text: 'It stops by itself after 10 minutes with nobody connected.',
                        action: PillButton(label: 'Share again', filled: true, onTap: _restart),
                      )
                    : _Sharing(
                        key: const ValueKey('sharing'),
                        url: url,
                        waiting: _waiting,
                        sentTo: _sentTo,
                        browsers: controller.browserDevices,
                        onSend: _choose,
                        onStop: () {
                          controller.stopWebShare();
                          Navigator.of(context).pop();
                        },
                      ),
              );
            },
          );
        },
      ),
    );
  }
}

class _Sharing extends StatelessWidget {
  const _Sharing({
    super.key,
    required this.url,
    required this.waiting,
    required this.sentTo,
    required this.browsers,
    required this.onSend,
    required this.onStop,
  });

  final String url;
  final List<OutgoingFile>? waiting;
  final String? sentTo;
  final List<DeviceInfo> browsers;
  final void Function(DeviceInfo) onSend;
  final VoidCallback onStop;

  @override
  Widget build(BuildContext context) {
    final files = waiting;
    return Center(
      child: ConstrainedBox(
        constraints: const BoxConstraints(maxWidth: 440),
        child: ListView(
          physics: const BouncingScrollPhysics(),
          padding: const EdgeInsets.fromLTRB(24, 20, 24, 32),
          children: [
            AnimatedSize(
              duration: const Duration(milliseconds: 300),
              child: files == null && sentTo == null
                  ? const SizedBox(width: double.infinity)
                  : Padding(
                      padding: const EdgeInsets.only(bottom: 24),
                      child: Glass(
                        padding: const EdgeInsets.fromLTRB(16, 12, 16, 12),
                        child: Row(
                          children: [
                            Icon(
                              sentTo == null
                                  ? CupertinoIcons.hourglass
                                  : CupertinoIcons.checkmark_circle_fill,
                              color: sentTo == null ? Palette.text : Palette.good,
                              size: 22,
                            ),
                            const SizedBox(width: 12),
                            Expanded(
                              child: Text(
                                sentTo == null
                                    ? '${describeFiles(files!)} for whoever scans'
                                    : 'Sent to $sentTo · they accept on their phone',
                                style: const TextStyle(color: Palette.text, fontSize: 14),
                              ),
                            ),
                          ],
                        ),
                      ),
                    ),
            ),
            Center(
              child: QrCard(child: CodeQr(data: url)),
            ),
            const SizedBox(height: 28),
            const Text('Scan with any camera', textAlign: TextAlign.center, style: Fonts.title),
            const SizedBox(height: 6),
            const Text(
              'Same Wi-Fi · nothing to install',
              textAlign: TextAlign.center,
              style: Fonts.small,
            ),
            const SizedBox(height: 4),
            const Text(
              "A temporary connection: they're here while the page is open, never paired.",
              textAlign: TextAlign.center,
              style: Fonts.small,
            ),
            const SizedBox(height: 28),
            AnimatedSize(
              duration: const Duration(milliseconds: 300),
              child: browsers.isEmpty
                  ? const SizedBox(width: double.infinity)
                  : Padding(
                      padding: const EdgeInsets.only(bottom: 28),
                      child: Wrap(
                        alignment: WrapAlignment.center,
                        spacing: 8,
                        runSpacing: 12,
                        children: [
                          for (final b in browsers)
                            Pressable(
                              key: ValueKey(b.id),
                              onTap: () => onSend(b),
                              child: SizedBox(
                                width: 88,
                                child: Column(
                                  children: [
                                    Container(
                                      width: 56,
                                      height: 56,
                                      decoration: BoxDecoration(
                                        shape: BoxShape.circle,
                                        color: Colors.white.withValues(
                                          alpha: b.online ? 0.14 : 0.06,
                                        ),
                                        border: Border.all(color: Palette.glassBorder),
                                      ),
                                      child: const Icon(CupertinoIcons.globe, color: Palette.text),
                                    ),
                                    const SizedBox(height: 6),
                                    Text(
                                      b.name,
                                      maxLines: 1,
                                      overflow: TextOverflow.ellipsis,
                                      style: Fonts.label,
                                    ),
                                    const Text(
                                      'Temporary',
                                      style: TextStyle(color: Palette.warn, fontSize: 11),
                                    ),
                                  ],
                                ),
                              ),
                            ),
                        ],
                      ),
                    ),
            ),
            Row(
              mainAxisAlignment: MainAxisAlignment.center,
              children: [
                PillButton(
                  label: 'Copy link',
                  icon: CupertinoIcons.link,
                  onTap: () async {
                    await Clipboard.setData(ClipboardData(text: url));
                    if (context.mounted) showToast(context, 'Link copied');
                  },
                ),
                const SizedBox(width: 10),
                PillButton(label: 'Stop', icon: CupertinoIcons.stop_fill, onTap: onStop),
              ],
            ),
            const SizedBox(height: 24),
            const Text(
              "Stops after 10 minutes with nobody connected. Not encrypted, so use it on Wi-Fi you trust.",
              textAlign: TextAlign.center,
              style: TextStyle(color: Palette.faint, fontSize: 12, height: 1.4),
            ),
          ],
        ),
      ),
    );
  }
}

class _Message extends StatelessWidget {
  const _Message({
    super.key,
    required this.icon,
    required this.title,
    required this.text,
    required this.action,
  });

  final IconData icon;
  final String title;
  final String text;
  final Widget action;

  @override
  Widget build(BuildContext context) {
    return Center(
      child: Padding(
        padding: const EdgeInsets.all(32),
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            Icon(icon, size: 48, color: Palette.muted),
            const SizedBox(height: 16),
            Text(title, style: Fonts.title, textAlign: TextAlign.center),
            const SizedBox(height: 8),
            Text(text, textAlign: TextAlign.center, style: Fonts.body),
            const SizedBox(height: 24),
            action,
          ],
        ),
      ),
    );
  }
}
