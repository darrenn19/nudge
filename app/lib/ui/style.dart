// Nudge's look: a dark, softly lit background, frosted glass, and a calm
// orb at the centre. Shared by every screen on every platform.

import 'dart:async';
import 'dart:io';
import 'dart:math' as math;
import 'dart:ui';

import 'package:flutter/cupertino.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:qr_flutter/qr_flutter.dart';

/// Colours. The app is always dark: the glow needs it.
abstract final class Palette {
  static const top = Color(0xFF1D2036);
  static const bottom = Color(0xFF0A0B13);
  static const text = Color(0xF2FFFFFF);
  static const muted = Color(0x8CFFFFFF);
  static const faint = Color(0x4DFFFFFF);
  static const glass = Color(0x14FFFFFF);
  static const glassBorder = Color(0x1AFFFFFF);
  static const accent = Color(0xFFA5AFFF);
  static const good = Color(0xFF7BE0A6);
  static const bad = Color(0xFFFF8A80);
  static const warn = Color(0xFFFFC46B);
}

const _spring = Curves.easeOutCubic;

/// The clock for slow, ambient motion (the background's drift, an orb's
/// calm breathing). It ticks 15 times a second rather than every frame:
/// motion this slow looks the same, and the window is redrawn a quarter as
/// often (each redraw costs about the same however little moved). One clock,
/// so everything ambient moves in the same frames. It runs only while
/// something listens.
class _AmbientClock extends ChangeNotifier {
  static const _interval = Duration(milliseconds: 66);
  final _watch = Stopwatch()..start();
  Timer? _timer;
  int _listeners = 0;

  double get seconds => _watch.elapsedMicroseconds / 1e6;

  @override
  void addListener(VoidCallback listener) {
    super.addListener(listener);
    if (_listeners++ == 0) _timer = Timer.periodic(_interval, (_) => notifyListeners());
  }

  @override
  void removeListener(VoidCallback listener) {
    super.removeListener(listener);
    if (--_listeners == 0) {
      _timer?.cancel();
      _timer = null;
    }
  }
}

final _ambient = _AmbientClock();

/// Moves a State with the ambient clock while [ambient] is set, and stops
/// while its tickers are muted (the window hidden in the tray).
mixin _AmbientMotion<T extends StatefulWidget> on State<T> {
  bool _wantsAmbient = false;
  bool _listening = false;
  bool _ready = false;

  /// Called on each ambient tick with the clock's time.
  void onAmbientTick(double seconds);

  set ambient(bool on) {
    _wantsAmbient = on;
    if (_ready) _syncAmbient();
  }

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    _ready = true;
    _syncAmbient();
  }

  void _syncAmbient() {
    final on = _wantsAmbient && TickerMode.valuesOf(context).enabled;
    if (on == _listening) return;
    _listening = on;
    on ? _ambient.addListener(_tick) : _ambient.removeListener(_tick);
  }

  void _tick() => onAmbientTick(_ambient.seconds);

  @override
  void dispose() {
    if (_listening) _ambient.removeListener(_tick);
    super.dispose();
  }
}

/// A light tap on phones, where it reads as "pressed".
void tapFeedback() {
  if (Platform.isAndroid || Platform.isIOS) HapticFeedback.selectionClick();
}

/// The background: a dark gradient with slow, drifting light.
class NudgeBackground extends StatefulWidget {
  const NudgeBackground({super.key, required this.child});

  final Widget child;

  @override
  State<NudgeBackground> createState() => _NudgeBackgroundState();
}

class _NudgeBackgroundState extends State<NudgeBackground> with _AmbientMotion {
  /// Where the light is in its 40-second sway, 0 to 1.
  final _drift = ValueNotifier<double>(0);

  @override
  void initState() {
    super.initState();
    ambient = true;
  }

  @override
  void onAmbientTick(double seconds) => _drift.value = seconds % 40 / 40;

  @override
  void dispose() {
    _drift.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    return DecoratedBox(
      decoration: const BoxDecoration(
        gradient: LinearGradient(
          begin: Alignment.topCenter,
          end: Alignment.bottomCenter,
          colors: [Palette.top, Palette.bottom],
        ),
      ),
      child: Stack(
        fit: StackFit.expand,
        children: [
          // All the light is painted once, into one layer a little bigger
          // than the screen, and that layer sways slowly: one cheap move per
          // frame instead of blending several large layers.
          LayoutBuilder(
            builder: (context, box) {
              final size = box.biggest;
              final pad = size.longestSide * 0.08;
              return Stack(
                clipBehavior: Clip.hardEdge,
                children: [
                  Positioned(
                    left: -pad,
                    top: -pad,
                    width: size.width + 2 * pad,
                    height: size.height + 2 * pad,
                    child: AnimatedBuilder(
                      animation: _drift,
                      builder: (context, child) {
                        final a = _drift.value * 2 * math.pi;
                        return Transform.translate(
                          offset: Offset(pad * 0.7 * math.sin(a), pad * 0.5 * math.cos(a * 0.8)),
                          child: child,
                        );
                      },
                      child: const RepaintBoundary(
                        child: CustomPaint(painter: _Lights(), size: Size.infinite),
                      ),
                    ),
                  ),
                ],
              );
            },
          ),
          widget.child,
        ],
      ),
    );
  }
}

typedef _LightSpec = (Color color, double x, double y, double r, double phase);

const _lights = <_LightSpec>[
  (Color(0x405A5FB0), 0.25, 0.30, 0.55, 0.0),
  (Color(0x307A4F8A), 0.80, 0.65, 0.50, 2.1),
  (Color(0x262F6A8C), 0.30, 0.85, 0.45, 4.2),
];

class _Lights extends CustomPainter {
  const _Lights();

  @override
  void paint(Canvas canvas, Size size) {
    for (final (color, x, y, r, _) in _lights) {
      final center = Offset(size.width * x, size.height * y);
      final radius = size.longestSide * r;
      canvas.drawCircle(
        center,
        radius,
        Paint()
          ..shader = RadialGradient(colors: [color, color.withValues(alpha: 0)])
              .createShader(Rect.fromCircle(center: center, radius: radius)),
      );
    }
  }

  @override
  bool shouldRepaint(_Lights old) => false;
}

/// Frosted glass.
class Glass extends StatelessWidget {
  const Glass({
    super.key,
    required this.child,
    this.radius = 20,
    this.padding = EdgeInsets.zero,
    this.blur = 0,
  });

  final Widget child;
  final double radius;
  final EdgeInsetsGeometry padding;

  /// Blur what's behind: costly, so only for things over real content.
  final double blur;

  @override
  Widget build(BuildContext context) {
    final panel = Container(
      padding: padding,
      decoration: BoxDecoration(
        color: Palette.glass,
        borderRadius: BorderRadius.circular(radius),
        border: Border.all(color: Palette.glassBorder),
      ),
      child: child,
    );
    if (blur == 0) return panel;
    return ClipRRect(
      borderRadius: BorderRadius.circular(radius),
      child: BackdropFilter(
        filter: ImageFilter.blur(sigmaX: blur, sigmaY: blur),
        child: panel,
      ),
    );
  }
}

/// Shrinks a little while pressed, like iOS buttons.
class Pressable extends StatefulWidget {
  const Pressable({super.key, required this.child, this.onTap, this.onLongPress});

  final Widget child;
  final VoidCallback? onTap;
  final VoidCallback? onLongPress;

  @override
  State<Pressable> createState() => _PressableState();
}

class _PressableState extends State<Pressable> {
  bool _down = false;

  /// What the current press is made with (finger, mouse, ...).
  PointerDeviceKind? _kind;

  void _set(bool down) {
    if (_down != down) setState(() => _down = down);
  }

  @override
  Widget build(BuildContext context) {
    final enabled = widget.onTap != null || widget.onLongPress != null;
    return MouseRegion(
      cursor: enabled ? SystemMouseCursors.click : MouseCursor.defer,
      child: Listener(
        onPointerDown: (e) => _kind = e.kind,
        child: GestureDetector(
          behavior: HitTestBehavior.opaque,
          onTapDown: enabled ? (_) => _set(true) : null,
          onTapUp: enabled ? (_) => _set(false) : null,
          onTapCancel: () => _set(false),
          onTap: widget.onTap == null
              ? null
              : () {
                  tapFeedback();
                  widget.onTap!();
                },
          // Long-press is a touch gesture. A mouse uses right-click instead:
          // a held mouse button is too easy to trigger by accident (a stray
          // press left over from a file dialog, say).
          onLongPress: widget.onLongPress == null
              ? null
              : () {
                  _set(false);
                  if (_kind == PointerDeviceKind.mouse) return;
                  HapticFeedback.mediumImpact();
                  widget.onLongPress!();
                },
          onSecondaryTap: widget.onLongPress,
          child: AnimatedScale(
            scale: _down ? 0.94 : 1,
            duration: const Duration(milliseconds: 160),
            curve: _spring,
            child: widget.child,
          ),
        ),
      ),
    );
  }
}

/// A round glass icon button.
class GlassIconButton extends StatelessWidget {
  const GlassIconButton({
    super.key,
    required this.icon,
    required this.onTap,
    required this.tooltip,
    this.dot = false,
  });

  final IconData icon;
  final VoidCallback onTap;
  final String tooltip;

  /// A small dot in the corner: something's happening.
  final bool dot;

  @override
  Widget build(BuildContext context) {
    return Tooltip(
      message: tooltip,
      child: Pressable(
        onTap: onTap,
        child: Stack(
          clipBehavior: Clip.none,
          children: [
            Glass(
              radius: 22,
              child: SizedBox.square(
                dimension: 44,
                child: Icon(icon, size: 20, color: Palette.text),
              ),
            ),
            Positioned(
              top: 2,
              right: 2,
              child: AnimatedScale(
                scale: dot ? 1 : 0,
                duration: const Duration(milliseconds: 250),
                curve: Curves.easeOutBack,
                child: Container(
                  width: 9,
                  height: 9,
                  decoration: const BoxDecoration(color: Palette.accent, shape: BoxShape.circle),
                ),
              ),
            ),
          ],
        ),
      ),
    );
  }
}

/// A wide, pill-shaped button.
class PillButton extends StatelessWidget {
  const PillButton({
    super.key,
    required this.label,
    required this.onTap,
    this.icon,
    this.filled = false,
  });

  final String label;
  final VoidCallback? onTap;
  final IconData? icon;
  final bool filled;

  @override
  Widget build(BuildContext context) {
    final content = Padding(
      padding: const EdgeInsets.symmetric(horizontal: 22, vertical: 14),
      child: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          if (icon != null) ...[
            Icon(icon, size: 18, color: filled ? Palette.bottom : Palette.text),
            const SizedBox(width: 8),
          ],
          Text(
            label,
            style: TextStyle(
              fontSize: 15,
              fontWeight: FontWeight.w600,
              color: filled ? Palette.bottom : Palette.text,
            ),
          ),
        ],
      ),
    );
    return Pressable(
      onTap: onTap,
      child: Opacity(
        opacity: onTap == null ? 0.5 : 1,
        child: filled
            ? DecoratedBox(
                decoration: BoxDecoration(
                  color: Palette.text,
                  borderRadius: BorderRadius.circular(28),
                ),
                child: content,
              )
            : Glass(radius: 28, child: content),
      ),
    );
  }
}

/// A screen on the Nudge background with a minimal top bar: a back button
/// and, optionally, a title.
class NudgePage extends StatelessWidget {
  const NudgePage({super.key, required this.child, this.title, this.actions = const []});

  final Widget child;
  final String? title;
  final List<Widget> actions;

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      backgroundColor: Colors.transparent,
      body: RepaintBoundary(
        child: SafeArea(
          child: Column(
            children: [
              Padding(
                padding: const EdgeInsets.fromLTRB(16, 8, 16, 0),
                child: Row(
                  children: [
                    if (Navigator.of(context).canPop())
                      GlassIconButton(
                        icon: CupertinoIcons.chevron_back,
                        tooltip: 'Back',
                        onTap: () => Navigator.of(context).maybePop(),
                      )
                    else
                      const SizedBox(width: 44),
                    Expanded(
                      child: Text(
                        title ?? '',
                        textAlign: TextAlign.center,
                        style: const TextStyle(
                          color: Palette.text,
                          fontSize: 17,
                          fontWeight: FontWeight.w600,
                        ),
                      ),
                    ),
                    if (actions.isEmpty) const SizedBox(width: 44) else ...actions,
                  ],
                ),
              ),
              Expanded(child: child),
            ],
          ),
        ),
      ),
    );
  }
}

/// The orb: concentric soft circles that breathe, with an icon, a word and
/// a smaller line in the middle. [progress] (0–1) draws a ring around the
/// centre; [pulse] makes it breathe faster and brighter; [tint] colours the
/// centre (green when sent, red when failed). Changing [ripple] sends one
/// wave out from it.
///
/// Built for smoothness: the circles are drawn once and the breathing only
/// scales them, so each frame is a cheap transform, not a repaint.
class Orb extends StatefulWidget {
  const Orb({
    super.key,
    required this.size,
    this.icon,
    this.label,
    this.sublabel,
    this.sublabelColor,
    this.progress,
    this.pulse = false,
    this.breathe = true,
    this.tint,
    this.badge,
    this.dim = false,
    this.ripple = 0,
    this.onTap,
    this.onLongPress,
  });

  final double size;
  final IconData? icon;
  final String? label;
  final String? sublabel;
  final Color? sublabelColor;
  final double? progress;
  final bool pulse;

  /// Off for orbs that aren't the focus (neighbouring pages).
  final bool breathe;
  final Color? tint;

  /// Shown in place of [sublabel] (the "not nearby" mark).
  final Widget? badge;

  /// Faded: the device isn't around. Done with colours rather than an
  /// opacity layer, which would redraw the whole orb on every breath.
  final bool dim;
  final int ripple;
  final VoidCallback? onTap;
  final VoidCallback? onLongPress;

  @override
  State<Orb> createState() => _OrbState();
}

class _OrbState extends State<Orb> with TickerProviderStateMixin, _AmbientMotion {
  late final _breath = AnimationController(vsync: this, duration: _period);
  late final _wave = AnimationController(vsync: this, duration: const Duration(milliseconds: 1100));

  Duration get _period =>
      widget.pulse ? const Duration(milliseconds: 1600) : const Duration(seconds: 5);

  /// The ambient clock's time at the last calm breath, to carry on from.
  double? _lastTick;

  @override
  void initState() {
    super.initState();
    _breathe();
  }

  @override
  void didUpdateWidget(Orb old) {
    super.didUpdateWidget(old);
    if (old.pulse != widget.pulse) _breath.duration = _period;
    if (old.pulse != widget.pulse || old.breathe != widget.breathe) _breathe();
    if (old.ripple != widget.ripple) _wave.forward(from: 0);
  }

  /// A pulse is quick, so it moves every frame; a calm breath is slow
  /// enough for the ambient clock.
  void _breathe() {
    final pulsing = widget.breathe && widget.pulse;
    if (pulsing && !_breath.isAnimating) _breath.repeat();
    if (!pulsing && _breath.isAnimating) _breath.stop();
    _lastTick = null;
    ambient = widget.breathe && !widget.pulse;
  }

  @override
  void onAmbientTick(double seconds) {
    final last = _lastTick;
    _lastTick = seconds;
    if (last == null) return;
    _breath.value = (_breath.value + (seconds - last) / 5) % 1;
  }

  @override
  void dispose() {
    _breath.dispose();
    _wave.dispose();
    super.dispose();
  }

  /// A circle that breathes [phase] behind the innermost one.
  Widget _ring(double fraction, double alpha, double phase, {Color? color, bool glow = false}) {
    final d = widget.size * fraction;
    final disc = RepaintBoundary(
      child: AnimatedContainer(
        duration: const Duration(milliseconds: 400),
        width: d,
        height: d,
        decoration: BoxDecoration(
          shape: BoxShape.circle,
          color: (color ?? Colors.white).withValues(alpha: widget.dim ? alpha * 0.55 : alpha),
          boxShadow: glow
              ? [
                  BoxShadow(
                    color: (color ?? Palette.accent).withValues(alpha: 0.35),
                    blurRadius: 40,
                    spreadRadius: 4,
                  ),
                ]
              : null,
        ),
      ),
    );
    return AnimatedBuilder(
      animation: _breath,
      builder: (context, child) {
        final t = _breath.value * 2 * math.pi;
        final depth = widget.pulse ? 0.05 : 0.025;
        return Transform.scale(scale: 1 + depth * math.sin(t - phase), child: child);
      },
      child: disc,
    );
  }

  @override
  Widget build(BuildContext context) {
    final size = widget.size;
    final centre = AnimatedSwitcher(
      duration: const Duration(milliseconds: 250),
      transitionBuilder: (child, a) => FadeTransition(opacity: a, child: child),
      child: Column(
        key: ValueKey('${widget.icon}|${widget.label}|${widget.sublabel}|${widget.badge != null}'),
        mainAxisSize: MainAxisSize.min,
        children: [
          if (widget.icon != null)
            Padding(
              padding: const EdgeInsets.only(bottom: 6),
              child: Icon(
                widget.icon,
                color: widget.dim ? Palette.muted : Palette.text,
                size: size * 0.1,
              ),
            ),
          if (widget.label != null)
            Text(
              widget.label!,
              textAlign: TextAlign.center,
              maxLines: 2,
              overflow: TextOverflow.ellipsis,
              style: TextStyle(
                color: widget.dim ? Palette.muted : Palette.text,
                fontSize: size * 0.068,
                fontWeight: FontWeight.w600,
                height: 1.15,
              ),
            ),
          if (widget.badge != null)
            Padding(padding: const EdgeInsets.only(top: 6), child: widget.badge)
          else if (widget.sublabel != null)
            Padding(
              padding: const EdgeInsets.only(top: 3),
              child: Text(
                widget.sublabel!,
                textAlign: TextAlign.center,
                maxLines: 2,
                overflow: TextOverflow.ellipsis,
                style: TextStyle(
                  color: widget.sublabelColor ?? Palette.muted,
                  fontSize: size * 0.048,
                  fontWeight: FontWeight.w500,
                ),
              ),
            ),
        ],
      ),
    );
    return Pressable(
      onTap: widget.onTap,
      onLongPress: widget.onLongPress,
      child: SizedBox.square(
        dimension: size,
        child: Stack(
          alignment: Alignment.center,
          clipBehavior: Clip.none,
          children: [
            // The arrival wave.
            AnimatedBuilder(
              animation: _wave,
              builder: (context, _) {
                if (!_wave.isAnimating) return const SizedBox.shrink();
                final t = Curves.easeOutCubic.transform(_wave.value);
                return IgnorePointer(
                  child: Transform.scale(
                    scale: 0.9 + 0.5 * t,
                    child: Container(
                      width: size,
                      height: size,
                      decoration: BoxDecoration(
                        shape: BoxShape.circle,
                        border: Border.all(
                          color: Palette.good.withValues(alpha: 0.6 * (1 - t)),
                          width: 2,
                        ),
                      ),
                    ),
                  ),
                );
              },
            ),
            _ring(1, widget.pulse ? 0.09 : 0.06, 1.2),
            _ring(0.72, widget.pulse ? 0.12 : 0.09, 0.6),
            _ring(0.5, widget.tint == null ? 0.2 : 0.32, 0, color: widget.tint, glow: widget.pulse),
            if (widget.progress != null)
              SizedBox.square(
                dimension: size * 0.62,
                child: TweenAnimationBuilder<double>(
                  tween: Tween(end: widget.progress!.clamp(0, 1)),
                  duration: const Duration(milliseconds: 400),
                  curve: _spring,
                  builder: (context, value, _) => CustomPaint(painter: _Ring(value, Palette.text)),
                ),
              ),
            SizedBox(width: size * 0.44, child: centre),
          ],
        ),
      ),
    );
  }
}

/// A progress ring. Null [value] spins.
class ProgressRing extends StatefulWidget {
  const ProgressRing({
    super.key,
    required this.value,
    required this.size,
    this.color = Palette.text,
  });

  final double? value;
  final double size;
  final Color color;

  @override
  State<ProgressRing> createState() => _ProgressRingState();
}

class _ProgressRingState extends State<ProgressRing> with SingleTickerProviderStateMixin {
  late final _spin = AnimationController(vsync: this, duration: const Duration(milliseconds: 1400))
    ..repeat();

  @override
  void dispose() {
    _spin.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final value = widget.value;
    return SizedBox.square(
      dimension: widget.size,
      child: value == null
          ? RotationTransition(
              turns: _spin,
              child: CustomPaint(painter: _Ring(0.25, widget.color)),
            )
          : TweenAnimationBuilder<double>(
              tween: Tween(end: value.clamp(0, 1)),
              duration: const Duration(milliseconds: 400),
              curve: _spring,
              builder: (context, v, _) => CustomPaint(painter: _Ring(v, widget.color)),
            ),
    );
  }
}

class _Ring extends CustomPainter {
  _Ring(this.value, this.color);

  final double value;
  final Color color;

  @override
  void paint(Canvas canvas, Size size) {
    final rect = Offset.zero & size;
    final track = Paint()
      ..style = PaintingStyle.stroke
      ..strokeWidth = 3
      ..color = color.withValues(alpha: 0.12);
    canvas.drawCircle(rect.center, size.width / 2 - 1.5, track);
    canvas.drawArc(
      rect.deflate(1.5),
      -math.pi / 2,
      2 * math.pi * value,
      false,
      Paint()
        ..style = PaintingStyle.stroke
        ..strokeWidth = 3
        ..strokeCap = StrokeCap.round
        ..color = color,
    );
  }

  @override
  bool shouldRepaint(_Ring old) => old.value != value || old.color != color;
}

/// A glass sheet that slides up from the bottom.
Future<T?> showGlassSheet<T>(BuildContext context, WidgetBuilder builder) {
  return showModalBottomSheet<T>(
    context: context,
    isScrollControlled: true,
    backgroundColor: Colors.transparent,
    barrierColor: Colors.black.withValues(alpha: 0.45),
    elevation: 0,
    builder: (context) => Padding(
      padding: const EdgeInsets.fromLTRB(8, 0, 8, 8),
      child: SafeArea(
        top: false,
        child: Center(
          heightFactor: 1,
          child: ConstrainedBox(
            constraints: const BoxConstraints(maxWidth: 520),
            child: Glass(
              radius: 32,
              blur: 40,
              child: ColoredBox(
                color: Palette.top.withValues(alpha: 0.55),
                child: Column(
                  mainAxisSize: MainAxisSize.min,
                  children: [
                    const SizedBox(height: 8),
                    Container(
                      width: 36,
                      height: 5,
                      decoration: BoxDecoration(
                        color: Palette.faint,
                        borderRadius: BorderRadius.circular(3),
                      ),
                    ),
                    Flexible(child: builder(context)),
                  ],
                ),
              ),
            ),
          ),
        ),
      ),
    ),
  );
}

/// A short message that drops in from the top, like an iOS banner.
void showToast(
  BuildContext context,
  String text, {
  IconData icon = CupertinoIcons.checkmark_alt,
  String? action,
  VoidCallback? onAction,
}) {
  final overlay = Overlay.maybeOf(context);
  if (overlay == null) return;
  late OverlayEntry entry;
  entry = OverlayEntry(
    builder: (_) => _Toast(
      text: text,
      icon: icon,
      action: action,
      onAction: onAction,
      onDone: () => entry.remove(),
    ),
  );
  overlay.insert(entry);
}

class _Toast extends StatefulWidget {
  const _Toast({
    required this.text,
    required this.icon,
    required this.onDone,
    this.action,
    this.onAction,
  });

  final String text;
  final IconData icon;
  final String? action;
  final VoidCallback? onAction;
  final VoidCallback onDone;

  @override
  State<_Toast> createState() => _ToastState();
}

class _ToastState extends State<_Toast> with SingleTickerProviderStateMixin {
  late final _show = AnimationController(vsync: this, duration: const Duration(milliseconds: 450));
  Timer? _timer;

  @override
  void initState() {
    super.initState();
    _show.forward();
    _timer = Timer(const Duration(seconds: 4), _hide);
  }

  Future<void> _hide() async {
    _timer?.cancel();
    if (!mounted) return;
    await _show.reverse();
    widget.onDone();
  }

  @override
  void dispose() {
    _timer?.cancel();
    _show.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final curve = CurvedAnimation(
      parent: _show,
      curve: Curves.easeOutBack,
      reverseCurve: Curves.easeIn,
    );
    return Positioned(
      top: MediaQuery.paddingOf(context).top + 8,
      left: 12,
      right: 12,
      child: Center(
        child: FadeTransition(
          opacity: _show,
          child: SlideTransition(
            position: Tween(begin: const Offset(0, -1.2), end: Offset.zero).animate(curve),
            child: ConstrainedBox(
              constraints: const BoxConstraints(maxWidth: 440),
              child: Material(
                type: MaterialType.transparency,
                child: GestureDetector(
                  onVerticalDragEnd: (_) => _hide(),
                  child: Glass(
                    radius: 22,
                    blur: 30,
                    child: ColoredBox(
                      color: Palette.top.withValues(alpha: 0.6),
                      child: Padding(
                        padding: const EdgeInsets.fromLTRB(16, 12, 8, 12),
                        child: Row(
                          children: [
                            Icon(widget.icon, size: 20, color: Palette.text),
                            const SizedBox(width: 12),
                            Expanded(
                              child: Text(
                                widget.text,
                                maxLines: 2,
                                overflow: TextOverflow.ellipsis,
                                style: const TextStyle(color: Palette.text, fontSize: 14),
                              ),
                            ),
                            if (widget.action != null)
                              CupertinoButton(
                                padding: const EdgeInsets.symmetric(horizontal: 12),
                                minimumSize: const Size(0, 32),
                                onPressed: () {
                                  widget.onAction?.call();
                                  _hide();
                                },
                                child: Text(
                                  widget.action!,
                                  style: const TextStyle(
                                    color: Palette.accent,
                                    fontWeight: FontWeight.w600,
                                    fontSize: 14,
                                  ),
                                ),
                              )
                            else
                              const SizedBox(width: 8),
                          ],
                        ),
                      ),
                    ),
                  ),
                ),
              ),
            ),
          ),
        ),
      ),
    );
  }
}

/// Moving between screens: the new one fades in while sliding a little,
/// the old one fades out. The background stays put (it's shared), so only
/// the content moves: cheap, and calm.
Route<T> nudgeRoute<T>(WidgetBuilder builder) => PageRouteBuilder<T>(
  transitionDuration: const Duration(milliseconds: 320),
  reverseTransitionDuration: const Duration(milliseconds: 260),
  pageBuilder: (context, _, _) => builder(context),
  transitionsBuilder: (context, animation, secondary, child) {
    final inCurve = CurvedAnimation(
      parent: animation,
      curve: Curves.easeOutCubic,
      reverseCurve: Curves.easeInCubic,
    );
    final outCurve = CurvedAnimation(parent: secondary, curve: Curves.easeOutCubic);
    return FadeTransition(
      opacity: inCurve,
      child: SlideTransition(
        position: Tween(begin: const Offset(0.06, 0), end: Offset.zero).animate(inCurve),
        child: FadeTransition(opacity: Tween(begin: 1.0, end: 0.0).animate(outCurve), child: child),
      ),
    );
  },
);

/// A yes/no question, in Nudge's glass.
Future<bool> confirm(
  BuildContext context, {
  required String title,
  required String message,
  required String action,
  bool destructive = false,
}) async {
  final ok = await showGeneralDialog<bool>(
    context: context,
    barrierDismissible: true,
    barrierLabel: 'Cancel',
    barrierColor: Colors.black.withValues(alpha: 0.5),
    transitionDuration: const Duration(milliseconds: 220),
    pageBuilder: (context, _, _) => Center(
      child: Padding(
        padding: const EdgeInsets.all(32),
        child: ConstrainedBox(
          constraints: const BoxConstraints(maxWidth: 340),
          child: Material(
            type: MaterialType.transparency,
            child: Glass(
              radius: 28,
              blur: 30,
              child: ColoredBox(
                color: Palette.top.withValues(alpha: 0.7),
                child: Padding(
                  padding: const EdgeInsets.fromLTRB(22, 24, 22, 18),
                  child: Column(
                    mainAxisSize: MainAxisSize.min,
                    children: [
                      Text(
                        title,
                        textAlign: TextAlign.center,
                        style: Fonts.title.copyWith(fontSize: 19),
                      ),
                      const SizedBox(height: 8),
                      Text(message, textAlign: TextAlign.center, style: Fonts.body),
                      const SizedBox(height: 22),
                      Row(
                        children: [
                          Expanded(
                            child: SheetButton(
                              label: 'Cancel',
                              onTap: () => Navigator.pop(context, false),
                            ),
                          ),
                          const SizedBox(width: 10),
                          Expanded(
                            child: SheetButton(
                              label: action,
                              filled: !destructive,
                              color: destructive ? Palette.bad : null,
                              onTap: () => Navigator.pop(context, true),
                            ),
                          ),
                        ],
                      ),
                    ],
                  ),
                ),
              ),
            ),
          ),
        ),
      ),
    ),
    transitionBuilder: (context, a, _, child) => FadeTransition(
      opacity: a,
      child: ScaleTransition(
        scale: Tween(
          begin: 0.94,
          end: 1.0,
        ).animate(CurvedAnimation(parent: a, curve: Curves.easeOutCubic)),
        child: child,
      ),
    ),
  );
  return ok == true;
}

/// A full-width button for sheets and dialogs.
class SheetButton extends StatelessWidget {
  const SheetButton({
    super.key,
    required this.label,
    required this.onTap,
    this.filled = false,
    this.color,
  });

  final String label;
  final VoidCallback? onTap;
  final bool filled;

  /// Text colour (for example red, for something destructive).
  final Color? color;

  @override
  Widget build(BuildContext context) {
    return Pressable(
      onTap: onTap,
      child: Container(
        height: 50,
        alignment: Alignment.center,
        decoration: BoxDecoration(
          color: filled ? Palette.text : Palette.glass,
          borderRadius: BorderRadius.circular(25),
          border: filled ? null : Border.all(color: Palette.glassBorder),
        ),
        child: Text(
          label,
          style: TextStyle(
            color: filled ? Palette.bottom : (color ?? Palette.text),
            fontWeight: FontWeight.w600,
            fontSize: 16,
          ),
        ),
      ),
    );
  }
}

/// One row in a [showMenuSheet].
class MenuItem {
  const MenuItem({
    required this.label,
    required this.icon,
    this.onTap,
    this.destructive = false,
    this.toggle,
    this.onToggle,
  });

  final String label;
  final IconData icon;
  final VoidCallback? onTap;
  final bool destructive;

  /// A switch on the right instead of an action: its current value.
  final bool? toggle;
  final ValueChanged<bool>? onToggle;
}

/// A menu in Nudge's glass: a title, rows of actions, and Cancel.
Future<void> showMenuSheet(
  BuildContext context, {
  required String title,
  String? subtitle,
  required List<MenuItem> items,
}) {
  return showGlassSheet<void>(
    context,
    (context) => _MenuSheet(title: title, subtitle: subtitle, items: items),
  );
}

class _MenuSheet extends StatefulWidget {
  const _MenuSheet({required this.title, required this.subtitle, required this.items});

  final String title;
  final String? subtitle;
  final List<MenuItem> items;

  @override
  State<_MenuSheet> createState() => _MenuSheetState();
}

class _MenuSheetState extends State<_MenuSheet> {
  // Switches flip here at once, before the change comes back from the core.
  late final _values = {
    for (final (i, item) in widget.items.indexed)
      if (item.toggle != null) i: item.toggle!,
  };

  @override
  Widget build(BuildContext context) {
    return Padding(
      padding: const EdgeInsets.fromLTRB(16, 14, 16, 16),
      child: Column(
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Text(
            widget.title,
            textAlign: TextAlign.center,
            style: Fonts.title.copyWith(fontSize: 19),
          ),
          if (widget.subtitle != null) ...[
            const SizedBox(height: 4),
            Text(widget.subtitle!, textAlign: TextAlign.center, style: Fonts.small),
          ],
          const SizedBox(height: 16),
          if (widget.items.isNotEmpty)
            Container(
              decoration: BoxDecoration(
                color: Palette.glass,
                borderRadius: BorderRadius.circular(20),
                border: Border.all(color: Palette.glassBorder),
              ),
              child: Column(
                children: [
                  for (final (i, item) in widget.items.indexed) ...[
                    if (i > 0)
                      const Divider(
                        height: 1,
                        thickness: 1,
                        color: Palette.glassBorder,
                        indent: 52,
                      ),
                    _row(i, item),
                  ],
                ],
              ),
            ),
          const SizedBox(height: 10),
          SheetButton(label: 'Cancel', onTap: () => Navigator.of(context).pop()),
        ],
      ),
    );
  }

  Widget _row(int i, MenuItem item) {
    final color = item.destructive ? Palette.bad : Palette.text;
    final value = _values[i];
    void tap() {
      if (value != null) {
        setState(() => _values[i] = !value);
        item.onToggle?.call(!value);
        return;
      }
      Navigator.of(context).pop();
      item.onTap?.call();
    }

    return Pressable(
      onTap: tap,
      child: Padding(
        padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 14),
        child: Row(
          children: [
            Icon(item.icon, color: color, size: 20),
            const SizedBox(width: 16),
            Expanded(
              child: Text(item.label, style: TextStyle(color: color, fontSize: 16)),
            ),
            if (value != null)
              IgnorePointer(
                child: CupertinoSwitch(
                  value: value,
                  activeTrackColor: Palette.good,
                  onChanged: (_) {},
                ),
              ),
          ],
        ),
      ),
    );
  }
}

/// A QR code on a white card (scanners need dark on light).
class QrCard extends StatelessWidget {
  const QrCard({super.key, required this.child});

  final Widget child;

  @override
  Widget build(BuildContext context) {
    return Container(
      padding: const EdgeInsets.all(18),
      decoration: BoxDecoration(
        color: Colors.white,
        borderRadius: BorderRadius.circular(32),
        boxShadow: [
          BoxShadow(color: Palette.accent.withValues(alpha: 0.25), blurRadius: 60, spreadRadius: 2),
        ],
      ),
      child: child,
    );
  }
}

/// Headline and secondary text styles.
abstract final class Fonts {
  static const title = TextStyle(
    color: Palette.text,
    fontSize: 22,
    fontWeight: FontWeight.w700,
    letterSpacing: -0.3,
  );
  static const body = TextStyle(color: Palette.muted, fontSize: 15, height: 1.35);
  static const small = TextStyle(color: Palette.muted, fontSize: 13);
  static const label = TextStyle(color: Palette.text, fontSize: 13, fontWeight: FontWeight.w500);
}

/// A QR code, drawn once per [data] however often its parent rebuilds.
class CodeQr extends StatefulWidget {
  const CodeQr({super.key, required this.data, this.size = 220});

  final String data;
  final double size;

  @override
  State<CodeQr> createState() => _CodeQrState();
}

class _CodeQrState extends State<CodeQr> {
  late Widget _qr = _build();

  /// Drawing a QR code is the heaviest thing on its screen, so it waits
  /// until the screen has finished sliding in, then fades in.
  bool _ready = false;
  Animation<double>? _route;

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    final route = ModalRoute.of(context)?.animation;
    if (route == _route) return;
    _route?.removeStatusListener(_settled);
    _route = route;
    if (route == null || route.isCompleted) {
      _ready = true;
    } else {
      route.addStatusListener(_settled);
    }
  }

  void _settled(AnimationStatus status) {
    if (status.isCompleted && mounted) setState(() => _ready = true);
  }

  @override
  void dispose() {
    _route?.removeStatusListener(_settled);
    super.dispose();
  }

  @override
  void didUpdateWidget(CodeQr old) {
    super.didUpdateWidget(old);
    if (old.data != widget.data || old.size != widget.size) _qr = _build();
  }

  Widget _build() => RepaintBoundary(
    child: QrImageView(
      data: widget.data,
      size: widget.size,
      backgroundColor: Colors.white,
      errorCorrectionLevel: QrErrorCorrectLevel.M,
      eyeStyle: const QrEyeStyle(eyeShape: QrEyeShape.circle, color: Color(0xFF111318)),
      dataModuleStyle: const QrDataModuleStyle(
        dataModuleShape: QrDataModuleShape.circle,
        color: Color(0xFF111318),
      ),
    ),
  );

  @override
  Widget build(BuildContext context) => SizedBox.square(
    dimension: widget.size,
    child: AnimatedOpacity(
      opacity: _ready ? 1 : 0,
      duration: const Duration(milliseconds: 220),
      child: _ready ? _qr : null,
    ),
  );
}
