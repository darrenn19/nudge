import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:mobile_scanner/mobile_scanner.dart';
import 'package:qr_flutter/qr_flutter.dart';

import '../controller.dart';
import '../platform.dart';
import '../src/rust/api/nudge.dart';

/// How long a code lasts; matches the core's `CODE_LIFETIME`.
const _codeLifetime = Duration(minutes: 5);

enum _Mode { show, scan, paste }

/// Pairs this device with another. Pops with the new [DeviceInfo] on success.
///
/// Phones open on the camera (they usually scan a computer's code);
/// computers open showing their own code. Either side can do either.
class PairScreen extends StatefulWidget {
  const PairScreen({super.key, required this.controller});

  final NudgeController controller;

  @override
  State<PairScreen> createState() => _PairScreenState();
}

class _PairScreenState extends State<PairScreen> {
  late _Mode _mode = isPhone ? _Mode.scan : _Mode.show;
  late final StreamSubscription<AppEvent> _events;
  bool _busy = false;
  String? _error;

  @override
  void initState() {
    super.initState();
    _events = widget.controller.pairingEvents.listen((event) {
      if (event case AppEvent_Paired(:final device)) _done(device);
    });
  }

  @override
  void dispose() {
    _events.cancel();
    widget.controller.app.cancelPairing();
    super.dispose();
  }

  bool _finished = false;

  /// Called by both the `Paired` event and a successful scan, so once only.
  void _done(DeviceInfo device) {
    if (_finished || !mounted) return;
    _finished = true;
    Navigator.of(context).pop(device);
  }

  /// Pair using a code read by the camera or pasted in.
  Future<void> _useCode(String code) async {
    if (_busy) return;
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      final device = await widget.controller.app.pair(code: code.trim());
      _done(device);
    } catch (e) {
      if (mounted) {
        setState(() {
          _busy = false;
          _error = _friendly(e.toString());
        });
      }
    }
  }

  String _friendly(String error) {
    if (error.contains('not a valid pairing code')) {
      return "That isn't a Nudge pairing code.";
    }
    if (error.contains("didn't accept") || error.contains('rejected')) {
      return 'That code has expired or was already used. Show a new one on the other device.';
    }
    if (error.contains("couldn't reach")) {
      return "Couldn't reach the other device. Check both are on the same Wi-Fi.";
    }
    return error;
  }

  @override
  Widget build(BuildContext context) {
    final modes = <_Mode>[_Mode.show, if (isPhone) _Mode.scan else _Mode.paste];
    return Scaffold(
      appBar: AppBar(title: const Text('Pair a device')),
      body: Center(
        child: ConstrainedBox(
          constraints: const BoxConstraints(maxWidth: 480),
          child: ListView(
            padding: const EdgeInsets.fromLTRB(24, 8, 24, 32),
            children: [
              SegmentedButton<_Mode>(
                showSelectedIcon: false,
                segments: [
                  for (final m in modes)
                    ButtonSegment(
                      value: m,
                      label: Text(switch (m) {
                        _Mode.show => 'Show my code',
                        _Mode.scan => 'Scan a code',
                        _Mode.paste => 'Enter a code',
                      }),
                      icon: Icon(switch (m) {
                        _Mode.show => Icons.qr_code_2_rounded,
                        _Mode.scan => Icons.qr_code_scanner_rounded,
                        _Mode.paste => Icons.keyboard_rounded,
                      }),
                    ),
                ],
                selected: {_mode},
                onSelectionChanged: (s) => setState(() {
                  _mode = s.first;
                  _error = null;
                }),
              ),
              const SizedBox(height: 24),
              switch (_mode) {
                _Mode.show => _ShowCode(app: widget.controller.app),
                _Mode.scan => _ScanCode(busy: _busy, onCode: _useCode),
                _Mode.paste => _PasteCode(busy: _busy, onCode: _useCode),
              },
              if (_error != null) ...[
                const SizedBox(height: 16),
                Text(
                  _error!,
                  textAlign: TextAlign.center,
                  style: TextStyle(color: Theme.of(context).colorScheme.error),
                ),
              ],
            ],
          ),
        ),
      ),
    );
  }
}

/// This device's code as a QR, with a countdown. Makes a new code when the
/// old one expires.
class _ShowCode extends StatefulWidget {
  const _ShowCode({required this.app});

  final NudgeApp app;

  @override
  State<_ShowCode> createState() => _ShowCodeState();
}

class _ShowCodeState extends State<_ShowCode> {
  late String _code;
  late DateTime _expires;
  Timer? _tick;

  @override
  void initState() {
    super.initState();
    _newCode();
    _tick = Timer.periodic(const Duration(seconds: 1), (_) {
      if (DateTime.now().isAfter(_expires)) _newCode();
      setState(() {});
    });
  }

  void _newCode() {
    _code = widget.app.startPairing();
    _expires = DateTime.now().add(_codeLifetime);
  }

  @override
  void dispose() {
    _tick?.cancel();
    // Not showing the code any more, so stop accepting it.
    widget.app.cancelPairing();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final left = _expires.difference(DateTime.now());
    final mm = left.inMinutes;
    final ss = (left.inSeconds % 60).toString().padLeft(2, '0');
    final other = isPhone ? 'other device' : 'phone';
    return Column(
      children: [
        Container(
          padding: const EdgeInsets.all(16),
          decoration: BoxDecoration(
            // QR codes need dark-on-light to scan, whatever the theme.
            color: Colors.white,
            borderRadius: BorderRadius.circular(24),
            boxShadow: [
              BoxShadow(
                color: Colors.black.withValues(alpha: 0.08),
                blurRadius: 24,
                offset: const Offset(0, 8),
              ),
            ],
          ),
          child: QrImageView(
            data: _code,
            size: 240,
            backgroundColor: Colors.white,
            errorCorrectionLevel: QrErrorCorrectLevel.M,
            eyeStyle: const QrEyeStyle(eyeShape: QrEyeShape.square, color: Color(0xFF111318)),
            dataModuleStyle: const QrDataModuleStyle(
              dataModuleShape: QrDataModuleShape.square,
              color: Color(0xFF111318),
            ),
          ),
        ),
        const SizedBox(height: 24),
        Text('Scan with Nudge on your $other', style: theme.textTheme.titleMedium),
        const SizedBox(height: 6),
        Text(
          'In Nudge, tap Pair a device, then Scan a code.\nThis code works once · expires in $mm:$ss',
          textAlign: TextAlign.center,
          style: theme.textTheme.bodyMedium?.copyWith(color: theme.colorScheme.onSurfaceVariant),
        ),
        const SizedBox(height: 12),
        TextButton.icon(
          onPressed: () async {
            await Clipboard.setData(ClipboardData(text: _code));
            if (context.mounted) {
              ScaffoldMessenger.of(context)
                  .showSnackBar(const SnackBar(content: Text('Code copied')));
            }
          },
          icon: const Icon(Icons.copy_rounded, size: 18),
          label: const Text('Copy code as text'),
        ),
      ],
    );
  }
}

class _ScanCode extends StatefulWidget {
  const _ScanCode({required this.busy, required this.onCode});

  final bool busy;
  final ValueChanged<String> onCode;

  @override
  State<_ScanCode> createState() => _ScanCodeState();
}

class _ScanCodeState extends State<_ScanCode> {
  final _scanner = MobileScannerController(
    formats: const [BarcodeFormat.qrCode],
    detectionSpeed: DetectionSpeed.noDuplicates,
  );

  @override
  void dispose() {
    _scanner.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return Column(
      children: [
        ClipRRect(
          borderRadius: BorderRadius.circular(24),
          child: AspectRatio(
            aspectRatio: 1,
            child: Stack(
              fit: StackFit.expand,
              children: [
                MobileScanner(
                  controller: _scanner,
                  onDetect: (capture) {
                    for (final code in capture.barcodes) {
                      final text = code.rawValue;
                      if (text != null && text.startsWith('nudge:pair?')) {
                        widget.onCode(text);
                        return;
                      }
                    }
                  },
                  errorBuilder: (context, error) => ColoredBox(
                    color: theme.colorScheme.surfaceContainerHighest,
                    child: Center(
                      child: Padding(
                        padding: const EdgeInsets.all(24),
                        child: Text(
                          error.errorCode == MobileScannerErrorCode.permissionDenied
                              ? 'Nudge needs the camera to scan pairing codes. '
                                    'Allow it in Settings, or show your code instead.'
                              : "The camera isn't available.",
                          textAlign: TextAlign.center,
                        ),
                      ),
                    ),
                  ),
                ),
                if (widget.busy)
                  const ColoredBox(
                    color: Colors.black45,
                    child: Center(child: CircularProgressIndicator()),
                  ),
              ],
            ),
          ),
        ),
        const SizedBox(height: 24),
        Text(
          widget.busy ? 'Pairing…' : 'Point the camera at the code on your other device',
          textAlign: TextAlign.center,
          style: theme.textTheme.titleMedium,
        ),
      ],
    );
  }
}

class _PasteCode extends StatefulWidget {
  const _PasteCode({required this.busy, required this.onCode});

  final bool busy;
  final ValueChanged<String> onCode;

  @override
  State<_PasteCode> createState() => _PasteCodeState();
}

class _PasteCodeState extends State<_PasteCode> {
  final _text = TextEditingController();

  @override
  void dispose() {
    _text.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Text(
          'On the other device, choose Show my code, then Copy code as text, '
          'and paste it here.',
          style: theme.textTheme.bodyMedium?.copyWith(color: theme.colorScheme.onSurfaceVariant),
        ),
        const SizedBox(height: 16),
        TextField(
          controller: _text,
          minLines: 3,
          maxLines: 5,
          enabled: !widget.busy,
          decoration: const InputDecoration(border: OutlineInputBorder(), hintText: 'nudge:pair?…'),
          onSubmitted: widget.onCode,
        ),
        const SizedBox(height: 16),
        FilledButton(
          onPressed: widget.busy ? null : () => widget.onCode(_text.text),
          child: widget.busy
              ? const SizedBox.square(
                  dimension: 18,
                  child: CircularProgressIndicator(strokeWidth: 2),
                )
              : const Text('Pair'),
        ),
      ],
    );
  }
}
