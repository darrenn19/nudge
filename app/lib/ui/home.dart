import 'package:flutter/material.dart';
import 'package:open_filex/open_filex.dart';

import '../controller.dart';
import '../format.dart';
import '../platform.dart';
import '../src/rust/api/nudge.dart';
import 'pair.dart';

class HomeScreen extends StatelessWidget {
  const HomeScreen({super.key, required this.controller});

  final NudgeController controller;

  Future<void> _openPairing(BuildContext context) async {
    final device = await Navigator.of(context)
        .push<DeviceInfo>(MaterialPageRoute(builder: (_) => PairScreen(controller: controller)));
    if (device != null && context.mounted) {
      ScaffoldMessenger.of(context)
          .showSnackBar(SnackBar(content: Text('Paired with ${device.name}')));
    }
  }

  Future<void> _send(BuildContext context, DeviceInfo device) async {
    final paths = await pickFiles();
    controller.send(device, paths);
  }

  @override
  Widget build(BuildContext context) {
    return ListenableBuilder(
      listenable: controller,
      builder: (context, _) {
        final paired = controller.pairedDevices;
        final nearby = controller.nearbyDevices;
        final transfers = controller.transfers;
        final empty = paired.isEmpty && nearby.isEmpty;

        return Scaffold(
          appBar: AppBar(
            title: const Text('Nudge'),
            actions: [
              IconButton(
                tooltip: 'Pair a device',
                icon: const Icon(Icons.qr_code_2_rounded),
                onPressed: () => _openPairing(context),
              ),
              const SizedBox(width: 4),
            ],
          ),
          body: RefreshIndicator(
            onRefresh: () async => controller.refresh(),
            child: Center(
              child: ConstrainedBox(
                constraints: const BoxConstraints(maxWidth: 640),
                child: ListView(
                  physics: const AlwaysScrollableScrollPhysics(),
                  padding: const EdgeInsets.fromLTRB(16, 4, 16, 32),
                  children: [
                    _ThisDevice(controller: controller),
                    if (transfers.isNotEmpty) ...[
                      _SectionHeader(
                        'Transfers',
                        action: transfers.any((t) => t.state != TransferState.running)
                            ? TextButton(
                                onPressed: controller.clearFinished,
                                child: const Text('Clear'),
                              )
                            : null,
                      ),
                      for (final t in transfers.take(10))
                        _TransferTile(transfer: t, controller: controller),
                    ],
                    if (empty)
                      _EmptyState(onPair: () => _openPairing(context))
                    else ...[
                      if (paired.isNotEmpty) ...[
                        const _SectionHeader('My devices'),
                        for (final d in paired)
                          _DeviceTile(
                            device: d,
                            onTap: d.online ? () => _send(context, d) : null,
                            onUnpair: () => _confirmUnpair(context, d),
                          ),
                      ],
                      if (nearby.isNotEmpty) ...[
                        const _SectionHeader('Nearby'),
                        for (final d in nearby)
                          _DeviceTile(device: d, onPair: () => _openPairing(context)),
                      ],
                    ],
                  ],
                ),
              ),
            ),
          ),
        );
      },
    );
  }

  Future<void> _confirmUnpair(BuildContext context, DeviceInfo device) async {
    final ok = await showDialog<bool>(
      context: context,
      builder: (context) => AlertDialog(
        title: Text('Unpair ${device.name}?'),
        content: const Text(
          "You'll need to scan a pairing code again before sending files between these devices.",
        ),
        actions: [
          TextButton(onPressed: () => Navigator.pop(context, false), child: const Text('Cancel')),
          FilledButton(onPressed: () => Navigator.pop(context, true), child: const Text('Unpair')),
        ],
      ),
    );
    if (ok == true) controller.unpair(device);
  }
}

class _ThisDevice extends StatelessWidget {
  const _ThisDevice({required this.controller});

  final NudgeController controller;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final muted = theme.colorScheme.onSurfaceVariant;
    final discovery = controller.app.discoveryRunning();
    return Padding(
      padding: const EdgeInsets.symmetric(vertical: 8),
      child: Row(
        children: [
          Icon(
            discovery ? Icons.wifi_tethering_rounded : Icons.wifi_tethering_off_rounded,
            size: 18,
            color: discovery ? theme.colorScheme.primary : theme.colorScheme.error,
          ),
          const SizedBox(width: 8),
          Expanded(
            child: Text(
              discovery
                  ? 'Visible to your devices as ${controller.deviceName}'
                  : "Can't look for devices on this network",
              style: theme.textTheme.bodyMedium?.copyWith(color: muted),
              overflow: TextOverflow.ellipsis,
            ),
          ),
        ],
      ),
    );
  }
}

class _SectionHeader extends StatelessWidget {
  const _SectionHeader(this.title, {this.action});

  final String title;
  final Widget? action;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return Padding(
      padding: const EdgeInsets.fromLTRB(4, 20, 0, 6),
      child: Row(
        children: [
          Expanded(
            child: Text(
              title,
              style: theme.textTheme.titleSmall?.copyWith(
                color: theme.colorScheme.onSurfaceVariant,
                fontWeight: FontWeight.w600,
              ),
            ),
          ),
          ?action,
        ],
      ),
    );
  }
}

IconData _platformIcon(String platform) => switch (platform) {
  'android' || 'ios' => Icons.smartphone_rounded,
  'windows' || 'linux' || 'macos' => Icons.laptop_windows_rounded,
  _ => Icons.devices_other_rounded,
};

class _DeviceTile extends StatelessWidget {
  const _DeviceTile({required this.device, this.onTap, this.onUnpair, this.onPair});

  final DeviceInfo device;
  final VoidCallback? onTap;
  final VoidCallback? onUnpair;
  final VoidCallback? onPair;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final scheme = theme.colorScheme;
    final status = device.paired
        ? (device.online ? 'Nearby · tap to send' : 'Not nearby')
        : 'Not paired';

    return Card(
      margin: const EdgeInsets.symmetric(vertical: 4),
      child: ListTile(
        contentPadding: const EdgeInsets.fromLTRB(16, 6, 8, 6),
        enabled: !device.paired || device.online,
        onTap: onTap,
        leading: CircleAvatar(
          backgroundColor: device.online ? scheme.primaryContainer : scheme.surfaceContainerHighest,
          foregroundColor: device.online ? scheme.onPrimaryContainer : scheme.onSurfaceVariant,
          child: Icon(_platformIcon(device.platform)),
        ),
        title: Text(device.name, overflow: TextOverflow.ellipsis),
        subtitle: Row(
          children: [
            if (device.paired) ...[
              Container(
                width: 8,
                height: 8,
                decoration: BoxDecoration(
                  shape: BoxShape.circle,
                  color: device.online ? Colors.green : scheme.outline,
                ),
              ),
              const SizedBox(width: 6),
            ],
            Flexible(child: Text(status, overflow: TextOverflow.ellipsis)),
          ],
        ),
        trailing: device.paired
            ? PopupMenuButton<void>(
                tooltip: 'More',
                itemBuilder: (_) => [PopupMenuItem(onTap: onUnpair, child: const Text('Unpair'))],
              )
            : FilledButton.tonal(onPressed: onPair, child: const Text('Pair')),
      ),
    );
  }
}

class _TransferTile extends StatelessWidget {
  const _TransferTile({required this.transfer, required this.controller});

  final Transfer transfer;
  final NudgeController controller;

  @override
  Widget build(BuildContext context) {
    final t = transfer;
    final theme = Theme.of(context);
    final scheme = theme.colorScheme;
    final peer = controller.deviceById(t.deviceId)?.name ?? 'device';

    final String detail;
    switch (t.state) {
      case TransferState.running:
        final parts = [
          '${formatSize(t.bytes)} of ${formatSize(t.size)}',
          if (t.bytesPerSec > 0) formatSpeed(t.bytesPerSec),
          if (t.remaining case final left?) '${formatDuration(left)} left',
        ];
        detail = parts.join(' · ');
      case TransferState.done:
        final took = t.took ?? Duration.zero;
        detail =
            '${t.incoming ? 'From' : 'Sent to'} $peer · ${formatSize(t.size)} in '
            '${formatDuration(took)} (${formatSpeed(t.bytesPerSec)})';
      case TransferState.failed:
        detail = t.error ?? 'Failed';
    }

    return Card(
      margin: const EdgeInsets.symmetric(vertical: 4),
      child: Padding(
        padding: const EdgeInsets.fromLTRB(16, 12, 8, 12),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Row(
              children: [
                Icon(
                  switch (t.state) {
                    TransferState.failed => Icons.error_outline_rounded,
                    TransferState.done => Icons.check_circle_outline_rounded,
                    TransferState.running => t.incoming ? Icons.south_rounded : Icons.north_rounded,
                  },
                  size: 20,
                  color: t.state == TransferState.failed ? scheme.error : scheme.primary,
                ),
                const SizedBox(width: 12),
                Expanded(
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      Text(t.fileName, overflow: TextOverflow.ellipsis),
                      const SizedBox(height: 2),
                      Text(
                        t.state == TransferState.running
                            ? '${t.incoming ? 'From' : 'To'} $peer · $detail'
                            : detail,
                        style: theme.textTheme.bodySmall?.copyWith(
                          color: t.state == TransferState.failed
                              ? scheme.error
                              : scheme.onSurfaceVariant,
                        ),
                        maxLines: 2,
                        overflow: TextOverflow.ellipsis,
                      ),
                    ],
                  ),
                ),
                if (t.state == TransferState.done && t.incoming && t.path != null)
                  TextButton(onPressed: () => OpenFilex.open(t.path!), child: const Text('Open')),
              ],
            ),
            if (t.state == TransferState.running) ...[
              const SizedBox(height: 10),
              Padding(
                padding: const EdgeInsets.only(right: 8),
                child: LinearProgressIndicator(
                  value: t.bytes == 0 ? null : t.fraction,
                  borderRadius: BorderRadius.circular(4),
                ),
              ),
            ],
          ],
        ),
      ),
    );
  }
}

class _EmptyState extends StatelessWidget {
  const _EmptyState({required this.onPair});

  final VoidCallback onPair;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final other = isPhone ? 'computer' : 'phone';
    return Padding(
      padding: const EdgeInsets.only(top: 72),
      child: Column(
        children: [
          Icon(Icons.devices_rounded, size: 56, color: theme.colorScheme.primary),
          const SizedBox(height: 16),
          Text('No devices yet', style: theme.textTheme.titleLarge),
          const SizedBox(height: 8),
          Text(
            'Open Nudge on your $other, then pair the two with a QR code. '
            "You'll only need to do this once.",
            textAlign: TextAlign.center,
            style: theme.textTheme.bodyMedium?.copyWith(color: theme.colorScheme.onSurfaceVariant),
          ),
          const SizedBox(height: 24),
          FilledButton.icon(
            onPressed: onPair,
            icon: const Icon(Icons.qr_code_2_rounded),
            label: const Text('Pair a device'),
          ),
        ],
      ),
    );
  }
}
