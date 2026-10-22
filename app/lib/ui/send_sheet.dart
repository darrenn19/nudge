import 'dart:io';

import 'package:flutter/material.dart';

import '../controller.dart';
import '../format.dart';
import '../platform.dart';
import '../src/rust/api/nudge.dart';

/// Send [request]'s files, asking which device only when there's a choice:
/// with one paired device nearby they go straight to it.
Future<void> chooseAndSend(
  BuildContext context,
  NudgeController controller,
  FilesToSend request,
) async {
  final files = request.files;
  final nearby = controller.pairedDevices.where((d) => d.online).toList();
  final device = nearby.length == 1
      ? nearby.single
      : await showModalBottomSheet<DeviceInfo>(
          context: context,
          showDragHandle: true,
          isScrollControlled: true,
          builder: (_) => _DevicePicker(controller: controller, files: files),
        );
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
                if (paired.isEmpty)
                  Padding(
                    padding: const EdgeInsets.symmetric(vertical: 24),
                    child: Text(
                      'No paired devices yet. Pair one from the Nudge home screen first.',
                      textAlign: TextAlign.center,
                      style: theme.textTheme.bodyMedium?.copyWith(color: muted),
                    ),
                  ),
                for (final d in paired)
                  Card(
                    margin: const EdgeInsets.symmetric(vertical: 4),
                    child: ListTile(
                      enabled: d.online,
                      leading: Icon(
                        d.platform == 'android' || d.platform == 'ios'
                            ? Icons.smartphone_rounded
                            : Icons.laptop_windows_rounded,
                      ),
                      title: Text(d.name, overflow: TextOverflow.ellipsis),
                      subtitle: Text(d.online ? 'Nearby' : 'Not nearby'),
                      onTap: () => Navigator.of(context).pop(d),
                    ),
                  ),
                if (paired.isNotEmpty && !paired.any((d) => d.online))
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
