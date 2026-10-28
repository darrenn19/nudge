// The direct link for big transfers: the phone starts a hotspot, the PC
// joins it, and both go back to normal once the transfer is done. The core
// decides when (only when it saves real time, and only if the user allows
// it); this file does each platform's part.

import 'dart:async';
import 'dart:io';

import 'package:flutter/services.dart';

const _channel = MethodChannel('nudge/platform');

/// How to join a hotspot this phone started.
class HotspotDetails {
  const HotspotDetails({required this.ssid, required this.passphrase, required this.wpa3});

  final String ssid;
  final String passphrase;

  /// WPA3 only, no WPA2 fallback.
  final bool wpa3;
}

/// Android: start a local-only hotspot (no internet sharing, just a direct
/// network between the two devices). Null if it couldn't.
Future<HotspotDetails?> startHotspot() async {
  if (!Platform.isAndroid) return null;
  try {
    final info = await _channel.invokeMapMethod<String, Object?>('startHotspot');
    final ssid = info?['ssid'] as String?;
    if (info == null || ssid == null) return null;
    return HotspotDetails(
      ssid: ssid,
      passphrase: info['passphrase'] as String? ?? '',
      wpa3: info['wpa3'] == true,
    );
  } on PlatformException {
    return null;
  }
}

Future<void> stopHotspot() async {
  if (Platform.isAndroid) await _channel.invokeMethod('stopHotspot');
}

// Windows: join with netsh, the same tool Windows' own settings use.

/// The temporary Wi-Fi profile for the phone's hotspot.
const _profile = 'Nudge direct';

/// The network to go back to afterwards.
String? _previous;

/// Join the phone's hotspot. True once connected with an address on it.
Future<bool> joinHotspot({
  required String ssid,
  required String passphrase,
  required bool wpa3,
}) async {
  if (!Platform.isWindows) return false;
  try {
    _previous ??= await _currentSsid();
    final before = await _addresses();
    final file = File('${Directory.systemTemp.path}\\nudge-direct.xml');
    await file.writeAsString(_profileXml(ssid, passphrase, wpa3));
    final added = await Process.run('netsh', [
      'wlan',
      'add',
      'profile',
      'filename=${file.path}',
      'user=current',
    ]);
    unawaited(file.delete().catchError((_) => file));
    if (added.exitCode != 0) return false;
    final connect = await Process.run('netsh', ['wlan', 'connect', 'name=$_profile', 'ssid=$ssid']);
    if (connect.exitCode != 0) {
      await forgetHotspot();
      return false;
    }
    // Connected once there's a new address (DHCP done). Recent Windows
    // versions hide the network name from netsh without location access,
    // so the address is what's checked.
    for (var i = 0; i < 40; i++) {
      await Future<void>.delayed(const Duration(milliseconds: 500));
      final now = await _addresses();
      if (now.any((a) => !before.contains(a))) return true;
    }
    await leaveHotspot();
    return false;
  } on ProcessException {
    return false;
  }
}

/// Back to the network the PC was on before.
Future<void> leaveHotspot() async {
  if (!Platform.isWindows) return;
  final previous = _previous;
  _previous = null;
  await forgetHotspot();
  if (previous == null) return;
  // Right after leaving, the adapter hasn't seen the usual network again
  // yet and netsh says it's "not available": ask Windows to look (listing
  // networks rescans) and try again, for up to ~15 s. Windows only rejoins
  // by itself when the profile is set to connect automatically.
  try {
    for (var i = 0; i < 15; i++) {
      await Process.run('netsh', ['wlan', 'show', 'networks']);
      final r = await Process.run('netsh', ['wlan', 'connect', 'name=$previous']);
      if (r.exitCode == 0) {
        for (var j = 0; j < 6; j++) {
          await Future<void>.delayed(const Duration(milliseconds: 500));
          if (await _currentSsid() == previous) return;
        }
      }
      await Future<void>.delayed(const Duration(seconds: 1));
    }
  } on ProcessException {
    // No netsh: nothing more to do.
  }
}

/// Remove the temporary profile (also at startup, in case Nudge stopped
/// while joined). Removing it disconnects from the hotspot, and Windows
/// goes back to its usual network.
Future<void> forgetHotspot() async {
  if (!Platform.isWindows) return;
  try {
    await Process.run('netsh', ['wlan', 'delete', 'profile', 'name=$_profile']);
  } on ProcessException {
    // Nothing to remove.
  }
}

/// The Wi-Fi network the PC is on now, if netsh will say.
Future<String?> _currentSsid() async {
  final result = await Process.run('netsh', ['wlan', 'show', 'interfaces']);
  for (final line in '${result.stdout}'.split('\n')) {
    final match = RegExp(r'^\s*SSID\s*:\s*(.+?)\s*$').firstMatch(line);
    if (match != null) return match.group(1);
  }
  return null;
}

Future<Set<String>> _addresses() async {
  final interfaces = await NetworkInterface.list(type: InternetAddressType.IPv4);
  return {
    for (final i in interfaces)
      for (final a in i.addresses) a.address,
  };
}

String _profileXml(String ssid, String passphrase, bool wpa3) {
  String esc(String s) => s
      .replaceAll('&', '&amp;')
      .replaceAll('<', '&lt;')
      .replaceAll('>', '&gt;')
      .replaceAll('"', '&quot;')
      .replaceAll("'", '&apos;');
  final auth = wpa3 ? 'WPA3SAE' : 'WPA2PSK';
  return '''<?xml version="1.0"?>
<WLANProfile xmlns="http://www.microsoft.com/networking/WLAN/profile/v1">
  <name>$_profile</name>
  <SSIDConfig><SSID><name>${esc(ssid)}</name></SSID></SSIDConfig>
  <connectionType>ESS</connectionType>
  <connectionMode>manual</connectionMode>
  <MSM>
    <security>
      <authEncryption>
        <authentication>$auth</authentication>
        <encryption>AES</encryption>
        <useOneX>false</useOneX>
      </authEncryption>
      <sharedKey>
        <keyType>passPhrase</keyType>
        <protected>false</protected>
        <keyMaterial>${esc(passphrase)}</keyMaterial>
      </sharedKey>
    </security>
  </MSM>
</WLANProfile>
''';
}
