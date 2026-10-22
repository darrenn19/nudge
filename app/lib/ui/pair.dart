import 'dart:async';

import 'package:flutter/cupertino.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:mobile_scanner/mobile_scanner.dart';

import '../controller.dart';
import '../platform.dart';
import '../src/rust/api/nudge.dart';
import 'style.dart';

/// How long a code lasts; matches the core's `CODE_LIFETIME`.
const _codeLifetime = Duration(minutes: 5);

enum _Mode { show, scan, paste }

/// Pairs this device with another. Pops with the new [DeviceInfo] on success.
///
/// Opens showing this device's code; the other side can scan it, or this
/// side can switch to scanning theirs.
class PairScreen extends StatefulWidget {
  const PairScreen({super.key, required this.controller});

  final NudgeController controller;

  @override
  State<PairScreen> createState() => _PairScreenState();
}

class _PairScreenState extends State<PairScreen> {
  _Mode _mode = _Mode.show;
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
    return NudgePage(
      title: 'Pair',
      child: Center(
        child: ConstrainedBox(
          constraints: const BoxConstraints(maxWidth: 440),
          child: ListView(
            physics: const BouncingScrollPhysics(),
            padding: const EdgeInsets.fromLTRB(24, 20, 24, 32),
            children: [
              CupertinoTheme(
                data: const CupertinoThemeData(brightness: Brightness.dark),
                child: CupertinoSlidingSegmentedControl<_Mode>(
                  groupValue: _mode,
                  backgroundColor: Palette.glass,
                  thumbColor: const Color(0x33FFFFFF),
                  onValueChanged: (m) {
                    if (m == null) return;
                    tapFeedback();
                    setState(() {
                      _mode = m;
                      _error = null;
                    });
                  },
                  children: {
                    for (final m in modes)
                      m: Padding(
                        padding: const EdgeInsets.symmetric(vertical: 8),
                        child: Text(switch (m) {
                          _Mode.show => 'My code',
                          _Mode.scan => 'Scan',
                          _Mode.paste => 'Enter code',
                        }, style: const TextStyle(color: Palette.text, fontSize: 14)),
                      ),
                  },
                ),
              ),
              const SizedBox(height: 32),
              AnimatedSwitcher(
                duration: const Duration(milliseconds: 220),
                switchInCurve: Curves.easeOut,
                switchOutCurve: Curves.easeIn,
                layoutBuilder: (current, previous) =>
                    Stack(alignment: Alignment.topCenter, children: [...previous, ?current]),
                child: KeyedSubtree(
                  key: ValueKey(_mode),
                  child: switch (_mode) {
                    _Mode.show => _ShowCode(app: widget.controller.app),
                    _Mode.scan => _ScanCode(busy: _busy, onCode: _useCode),
                    _Mode.paste => _PasteCode(busy: _busy, onCode: _useCode),
                  },
                ),
              ),
              AnimatedSize(
                duration: const Duration(milliseconds: 250),
                child: _error == null
                    ? const SizedBox(width: double.infinity)
                    : Padding(
                        padding: const EdgeInsets.only(top: 20),
                        child: Text(
                          _error!,
                          textAlign: TextAlign.center,
                          style: const TextStyle(color: Palette.bad, fontSize: 14),
                        ),
                      ),
              ),
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

  /// Seconds left, for the countdown alone; the QR doesn't rebuild with it.
  final _left = ValueNotifier<int>(_codeLifetime.inSeconds);

  @override
  void initState() {
    super.initState();
    _newCode();
    _tick = Timer.periodic(const Duration(seconds: 1), (_) {
      if (DateTime.now().isAfter(_expires)) setState(_newCode);
      _left.value = _expires.difference(DateTime.now()).inSeconds;
    });
  }

  void _newCode() {
    _code = widget.app.startPairing();
    _expires = DateTime.now().add(_codeLifetime);
    _left.value = _codeLifetime.inSeconds;
  }

  @override
  void dispose() {
    _tick?.cancel();
    _left.dispose();
    // Not showing the code any more, so stop accepting it.
    widget.app.cancelPairing();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final other = isPhone ? 'other device' : 'phone';
    return Column(
      children: [
        QrCard(child: CodeQr(data: _code)),
        const SizedBox(height: 28),
        Text('Scan with your $other', style: Fonts.title, textAlign: TextAlign.center),
        const SizedBox(height: 6),
        ValueListenableBuilder<int>(
          valueListenable: _left,
          builder: (context, left, _) => Text(
            'Works once · ${left ~/ 60}:${(left % 60).toString().padLeft(2, '0')}',
            style: Fonts.small,
          ),
        ),
        const SizedBox(height: 16),
        CupertinoButton(
          onPressed: () async {
            await Clipboard.setData(ClipboardData(text: _code));
            if (context.mounted) showToast(context, 'Code copied');
          },
          child: const Text('Copy as text', style: TextStyle(color: Palette.accent, fontSize: 15)),
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
    autoStart: false,
  );
  bool _live = false;

  @override
  void initState() {
    super.initState();
    Future<void>.delayed(const Duration(milliseconds: 280), () async {
      if (!mounted) return;
      setState(() => _live = true);
      await _scanner.start();
    });
  }

  @override
  void dispose() {
    _scanner.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    return Column(
      children: [
        ClipRRect(
          borderRadius: BorderRadius.circular(32),
          child: AspectRatio(
            aspectRatio: 1,
            child: Stack(
              fit: StackFit.expand,
              children: [
                if (!_live)
                  const ColoredBox(color: Color(0xFF05060A))
                else
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
                      color: Palette.glass,
                      child: Center(
                        child: Padding(
                          padding: const EdgeInsets.all(24),
                          child: Text(
                            error.errorCode == MobileScannerErrorCode.permissionDenied
                                ? 'Allow the camera in Settings to scan, or show your code instead.'
                                : "The camera isn't available.",
                            textAlign: TextAlign.center,
                            style: Fonts.body,
                          ),
                        ),
                      ),
                    ),
                  ),
                // A soft frame to aim with.
                IgnorePointer(
                  child: Center(
                    child: FractionallySizedBox(
                      widthFactor: 0.62,
                      heightFactor: 0.62,
                      child: DecoratedBox(
                        decoration: BoxDecoration(
                          borderRadius: BorderRadius.circular(28),
                          border: Border.all(color: Colors.white.withValues(alpha: 0.7), width: 3),
                        ),
                      ),
                    ),
                  ),
                ),
                IgnorePointer(
                  ignoring: !widget.busy,
                  child: AnimatedOpacity(
                    opacity: widget.busy ? 1 : 0,
                    duration: const Duration(milliseconds: 250),
                    child: const ColoredBox(
                      color: Color(0x99000000),
                      child: Center(
                        child: CupertinoActivityIndicator(color: Palette.text, radius: 14),
                      ),
                    ),
                  ),
                ),
              ],
            ),
          ),
        ),
        const SizedBox(height: 28),
        Text(
          widget.busy ? 'Pairing…' : 'Point at the code on your other device',
          textAlign: TextAlign.center,
          style: Fonts.title.copyWith(fontSize: 18),
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
    return Column(
      children: [
        const Text(
          'On the other device, open My code and tap Copy as text. Paste it here.',
          textAlign: TextAlign.center,
          style: Fonts.body,
        ),
        const SizedBox(height: 20),
        Glass(
          padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 4),
          child: TextField(
            controller: _text,
            minLines: 3,
            maxLines: 5,
            enabled: !widget.busy,
            style: const TextStyle(color: Palette.text, fontSize: 14),
            cursorColor: Palette.accent,
            decoration: const InputDecoration(
              border: InputBorder.none,
              hintText: 'nudge:pair?…',
              hintStyle: TextStyle(color: Palette.faint),
            ),
            onSubmitted: widget.onCode,
          ),
        ),
        const SizedBox(height: 20),
        PillButton(
          label: widget.busy ? 'Pairing…' : 'Pair',
          filled: true,
          onTap: widget.busy ? null : () => widget.onCode(_text.text),
        ),
      ],
    );
  }
}
