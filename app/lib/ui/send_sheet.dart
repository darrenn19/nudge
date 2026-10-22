import 'dart:io';

import 'package:flutter/material.dart';

import '../controller.dart';
import '../format.dart';
import '../platform.dart';
import '../src/rust/api/nudge.dart';
import 'web_share.dart';

/// Send [request]'s files, asking which device only when there's a choice:
/// with one device nearby (paired, or an open browser page) they go straight
/// to it. With none nearby but one paired device seen earlier (say, a phone
/// with its screen off), they go to that. Files meant for someone without
/// Nudge ([FilesToSend.byQr], or picked that way) go to the QR code screen.
Future<void> chooseAndSend(
  BuildContext context,
  NudgeController controller,
  FilesToSend request,
) async {
  final files = request.files;
  if (request.byQr) return _shareByQr(context, controller, files);
  final nearby = [
    ...controller.pairedDevices.where((d) => d.online),
    ...controller.browserDevices.where((d) => d.online),
  ];
  final reachable = controller.pairedDevices.where((d) => d.reachable).toList();
  final choice = nearby.length == 1
      ? nearby.single
      : nearby.isEmpty && reachable.length == 1
      ? reachable.single
      : await showModalBottomSheet<Object>(
          context: context,
          showDragHandle: true,
          isScrollControlled: true,
          builder: (_) => _DevicePicker(controller: controller, files: files),
        );
  if (choice == _someoneWithoutNudge && context.mounted) {
    return _shareByQr(context, controller, files);
  }
  final device = choice is DeviceInfo ? choice : null;
  if (device == null) {
    await closeUnsent(files);
    return;
  }
  controller.send(device, files);
  if (context.mounted) {
    ScaffoldMessenger.of(context)
        .showSnackBar(SnackBar(content: Text('Sending ${describeFiles(files)} to ${device.name}')));
  }
  // Shared from another app: go back to it. The notification shows progress
  // and keeps Android from stopping the transfer.
  if (request.fromOutside && Platform.isAndroid) {
    await showTransferProgress(title: 'Sending to ${device.name}', text: 'Starting…', progress: -1);
    await leaveApp();
  }
}

/// Picked in the device list: someone without Nudge.
const _someoneWithoutNudge = #someoneWithoutNudge;

/// Show the QR code; the files go to the first person who opens the page.
Future<void> _shareByQr(
  BuildContext context,
  NudgeController controller,
  List<OutgoingFile> files,
) async {
  await Navigator.of(context).push(
    MaterialPageRoute<void>(
      builder: (_) => WebShareScreen(controller: controller, files: files),
    ),
  );
}

/// "photo.jpg" or "3 files".
String describeFiles(List<OutgoingFile> files) =>
    files.length == 1 ? files.single.name : '${files.length} files';

class _DevicePicker extends StatelessWidget {
  const _DevicePicker({required this.controller, required this.files});

  final NudgeController controller;
  final List<OutgoingFile> files;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final muted = theme.colorScheme.onSurfaceVariant;
    final size = files.fold<int>(0, (sum, f) => sum + f.size);
    return SafeArea(
      child: ListenableBuilder(
        listenable: controller,
        builder: (context, _) {
          final paired = controller.pairedDevices;
          final browsers = controller.browserDevices;
          return Padding(
            padding: const EdgeInsets.fromLTRB(16, 0, 16, 16),
            child: Column(
              mainAxisSize: MainAxisSize.min,
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [
                Text('Send ${describeFiles(files)}', style: theme.textTheme.titleLarge),
                const SizedBox(height: 2),
                Text(formatSize(size), style: theme.textTheme.bodyMedium?.copyWith(color: muted)),
                const SizedBox(height: 16),
                if (paired.isEmpty && browsers.isEmpty)
                  Padding(
                    padding: const EdgeInsets.symmetric(vertical: 24),
                    child: Text(
                      'No paired devices yet. Pair one from the Nudge home screen first.',
                      textAlign: TextAlign.center,
                      style: theme.textTheme.bodyMedium?.copyWith(color: muted),
                    ),
                  ),
                for (final d in [...paired, ...browsers])
                  Card(
                    margin: const EdgeInsets.symmetric(vertical: 4),
                    child: ListTile(
                      enabled: d.reachable,
                      leading: Icon(
                        d.isBrowser
                            ? Icons.public_rounded
                            : d.platform == 'android' || d.platform == 'ios'
                            ? Icons.smartphone_rounded
                            : Icons.laptop_windows_rounded,
                      ),
                      title: Text(d.name, overflow: TextOverflow.ellipsis),
                      subtitle: Text(
                        d.isBrowser
                            ? 'In a browser · they accept first'
                            : d.online
                            ? 'Nearby'
                            : d.reachable
                            ? 'Not seen just now · tap to try'
                            : 'Not nearby',
                      ),
                      onTap: () => Navigator.of(context).pop(d),
                    ),
                  ),
                Card(
                  margin: const EdgeInsets.symmetric(vertical: 4),
                  child: ListTile(
                    leading: const Icon(Icons.qr_code_2_rounded),
                    title: const Text('Someone without Nudge'),
                    subtitle: const Text('Show a code to scan with their camera'),
                    onTap: () => Navigator.of(context).pop(_someoneWithoutNudge),
                  ),
                ),
                if (paired.isNotEmpty && browsers.isEmpty && !paired.any((d) => d.reachable))
                  Padding(
                    padding: const EdgeInsets.only(top: 12),
                    child: Text(
                      'Waiting for one of your devices to come nearby…',
                      textAlign: TextAlign.center,
                      style: theme.textTheme.bodySmall?.copyWith(color: muted),
                    ),
                  ),
              ],
            ),
          );
        },
      ),
    );
  }
}
