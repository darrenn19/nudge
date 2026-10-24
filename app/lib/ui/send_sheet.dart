import 'dart:io';

import 'package:flutter/cupertino.dart';
import 'package:flutter/material.dart';

import '../controller.dart';
import '../format.dart';
import '../platform.dart';
import '../src/rust/api/nudge.dart';
import 'home.dart' show deviceIcon;
import 'style.dart';
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
  final chosen = request.to == null ? null : controller.deviceById(request.to!);
  final choice = chosen != null && chosen.paired
      ? chosen
      : nearby.length == 1
      ? nearby.single
      : nearby.isEmpty && reachable.length == 1
      ? reachable.single
      : await showGlassSheet<Object>(
          context,
          (_) => _DevicePicker(controller: controller, files: files),
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
    showToast(
      context,
      'Sending ${describeFiles(files)} to ${device.name}',
      icon: CupertinoIcons.arrow_up_circle,
    );
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
  await Navigator.of(context)
      .push(nudgeRoute<void>((_) => WebShareScreen(controller: controller, files: files)));
}

/// "photo.jpg" or "3 files".
String describeFiles(List<OutgoingFile> files) =>
    files.length == 1 ? files.single.name : '${files.length} files';

/// Where to send: your devices as bubbles, like AirDrop.
class _DevicePicker extends StatelessWidget {
  const _DevicePicker({required this.controller, required this.files});

  final NudgeController controller;
  final List<OutgoingFile> files;

  @override
  Widget build(BuildContext context) {
    final size = files.fold<int>(0, (sum, f) => sum + f.size);
    return ListenableBuilder(
      listenable: controller,
      builder: (context, _) {
        final devices = [...controller.pairedDevices, ...controller.browserDevices];
        return Padding(
          padding: const EdgeInsets.fromLTRB(20, 16, 20, 24),
          child: Column(
            mainAxisSize: MainAxisSize.min,
            children: [
              Text(
                describeFiles(files),
                style: Fonts.title,
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
              ),
              const SizedBox(height: 4),
              Text(formatSize(size), style: Fonts.small),
              const SizedBox(height: 24),
              Wrap(
                alignment: WrapAlignment.center,
                spacing: 8,
                runSpacing: 16,
                children: [
                  for (final d in devices)
                    _Choice(
                      icon: deviceIcon(d),
                      label: d.name,
                      note: d.isBrowser ? 'Temporary' : null,
                      dim: !d.reachable,
                      onTap: d.reachable ? () => Navigator.of(context).pop(d) : null,
                    ),
                  _Choice(
                    icon: CupertinoIcons.qrcode,
                    label: 'Without app',
                    onTap: () => Navigator.of(context).pop(_someoneWithoutNudge),
                  ),
                ],
              ),
              if (devices.isNotEmpty && !devices.any((d) => d.reachable)) ...[
                const SizedBox(height: 20),
                const Text('Waiting for your devices to come nearby…', style: Fonts.small),
              ],
            ],
          ),
        );
      },
    );
  }
}

class _Choice extends StatelessWidget {
  const _Choice({required this.icon, required this.label, this.note, this.onTap, this.dim = false});

  final IconData icon;
  final String label;

  /// Under the name: "Temporary" for a web page.
  final String? note;
  final VoidCallback? onTap;
  final bool dim;

  @override
  Widget build(BuildContext context) {
    return Pressable(
      onTap: onTap,
      child: Opacity(
        opacity: dim ? 0.4 : 1,
        child: SizedBox(
          width: 88,
          child: Column(
            children: [
              Container(
                width: 60,
                height: 60,
                decoration: BoxDecoration(
                  shape: BoxShape.circle,
                  color: Colors.white.withValues(alpha: 0.12),
                  border: Border.all(color: Palette.glassBorder),
                ),
                child: Icon(icon, color: Palette.text, size: 24),
              ),
              const SizedBox(height: 8),
              Text(
                label,
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
                textAlign: TextAlign.center,
                style: Fonts.label,
              ),
              if (note != null)
                Text(note!, style: const TextStyle(color: Palette.warn, fontSize: 11)),
            ],
          ),
        ),
      ),
    );
  }
}
