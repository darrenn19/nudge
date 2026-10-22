import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:qr_flutter/qr_flutter.dart';

import '../controller.dart';
import '../platform.dart';
import '../src/rust/api/nudge.dart';
import 'send_sheet.dart' show describeFiles;

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
  late Future<String> _starting = controller.startWebShare();

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

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      appBar: AppBar(title: const Text('Share without the app')),
      body: FutureBuilder<String>(
        future: _starting,
        builder: (context, snapshot) {
          if (snapshot.hasError) {
            return _Message(
              icon: Icons.wifi_off_rounded,
              title: "Couldn't start sharing",
              text: '${snapshot.error}',
              action: FilledButton(onPressed: _restart, child: const Text('Try again')),
            );
          }
          if (!snapshot.hasData) return const Center(child: CircularProgressIndicator());
          return ListenableBuilder(
            listenable: controller,
            builder: (context, _) {
              final url = controller.webShareUrl;
              if (url == null) {
                return _Message(
                  icon: Icons.public_off_rounded,
                  title: 'Sharing stopped',
                  text: 'It stops by itself after 10 minutes with nobody connected.',
                  action: FilledButton(onPressed: _restart, child: const Text('Share again')),
                );
              }
              return _Sharing(
                url: url,
                waiting: _waiting,
                sentTo: _sentTo,
                browsers: controller.browserDevices,
                onSend: _send,
                onStop: () {
                  controller.stopWebShare();
                  Navigator.of(context).pop();
                },
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
    final theme = Theme.of(context);
    final muted = theme.colorScheme.onSurfaceVariant;
    return Center(
      child: ConstrainedBox(
        constraints: const BoxConstraints(maxWidth: 480),
        child: ListView(
          padding: const EdgeInsets.fromLTRB(24, 16, 24, 32),
          children: [
            if (waiting != null || sentTo != null)
              Card(
                margin: const EdgeInsets.only(bottom: 20),
                color: theme.colorScheme.secondaryContainer,
                child: ListTile(
                  leading: Icon(
                    sentTo == null ? Icons.hourglass_top_rounded : Icons.check_circle_rounded,
                    color: theme.colorScheme.onSecondaryContainer,
                  ),
                  title: Text(
                    sentTo == null ? '${describeFiles(waiting!)} ready to send' : 'Sent to $sentTo',
                  ),
                  subtitle: Text(
                    sentTo == null
                        ? 'They go to the first person who opens the page.'
                        : 'They tap Accept on their phone to download.',
                  ),
                ),
              ),
            Center(
              child: Container(
                padding: const EdgeInsets.all(16),
                decoration: BoxDecoration(
                  // QR codes need dark-on-light to scan, whatever the theme.
                  color: Colors.white,
                  borderRadius: BorderRadius.circular(24),
                ),
                child: QrImageView(
                  data: url,
                  size: 220,
                  backgroundColor: Colors.white,
                  errorCorrectionLevel: QrErrorCorrectLevel.M,
                  eyeStyle: const QrEyeStyle(eyeShape: QrEyeShape.square, color: Color(0xFF111318)),
                  dataModuleStyle: const QrDataModuleStyle(
                    dataModuleShape: QrDataModuleShape.square,
                    color: Color(0xFF111318),
                  ),
                ),
              ),
            ),
            const SizedBox(height: 20),
            Text(
              'Scan with any phone camera',
              textAlign: TextAlign.center,
              style: theme.textTheme.titleMedium,
            ),
            const SizedBox(height: 6),
            Text(
              'They need to be on the same Wi-Fi. Nothing to install: a page opens in '
              'their browser, they accept what you send, and they can send files back.',
              textAlign: TextAlign.center,
              style: theme.textTheme.bodyMedium?.copyWith(color: muted),
            ),
            const SizedBox(height: 8),
            Center(
              child: TextButton.icon(
                onPressed: () async {
                  await Clipboard.setData(ClipboardData(text: url));
                  if (context.mounted) {
                    ScaffoldMessenger.of(context)
                        .showSnackBar(const SnackBar(content: Text('Link copied')));
                  }
                },
                icon: const Icon(Icons.copy_rounded, size: 18),
                label: const Text('Copy link'),
              ),
            ),
            const SizedBox(height: 16),
            Text(
              'Connected',
              style: theme.textTheme.titleSmall?.copyWith(
                color: muted,
                fontWeight: FontWeight.w600,
              ),
            ),
            const SizedBox(height: 6),
            if (browsers.isEmpty)
              Padding(
                padding: const EdgeInsets.symmetric(vertical: 12),
                child: Text(
                  'Nobody yet. They appear here once the page opens.',
                  style: theme.textTheme.bodyMedium?.copyWith(color: muted),
                ),
              ),
            for (final b in browsers)
              Card(
                margin: const EdgeInsets.symmetric(vertical: 4),
                child: ListTile(
                  leading: const CircleAvatar(child: Icon(Icons.public_rounded)),
                  title: Text(b.name),
                  subtitle: Text(b.online ? 'Tap to send files' : 'Page in the background'),
                  trailing: const Icon(Icons.send_rounded),
                  onTap: () => onSend(b),
                ),
              ),
            const SizedBox(height: 20),
            FilledButton.tonal(onPressed: onStop, child: const Text('Stop sharing')),
            const SizedBox(height: 12),
            Text(
              'Stops by itself after 10 minutes with nobody connected. The page '
              "isn't encrypted, so use it on networks you trust.",
              textAlign: TextAlign.center,
              style: theme.textTheme.bodySmall?.copyWith(color: muted),
            ),
          ],
        ),
      ),
    );
  }
}

class _Message extends StatelessWidget {
  const _Message({
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
    final theme = Theme.of(context);
    return Center(
      child: Padding(
        padding: const EdgeInsets.all(32),
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            Icon(icon, size: 48, color: theme.colorScheme.onSurfaceVariant),
            const SizedBox(height: 16),
            Text(title, style: theme.textTheme.titleLarge),
            const SizedBox(height: 8),
            Text(text, textAlign: TextAlign.center),
            const SizedBox(height: 20),
            action,
          ],
        ),
      ),
    );
  }
}
