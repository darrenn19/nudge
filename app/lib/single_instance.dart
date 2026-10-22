// Windows: one Nudge at a time.
//
// Explorer's "Send with Nudge" starts a new process per selected file. The
// first process to start holds a lock file and listens on a loopback port;
// later ones hand their files to it and exit, so files from one selection
// arrive together.

import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'dart:math';

import 'windows_shell.dart';

const _sendFlag = '--send';

/// File paths passed on the command line (`--send <path>...`).
List<String> filesFromArgs(List<String> args) {
  final at = args.indexOf(_sendFlag);
  return at < 0 ? const [] : args.sublist(at + 1).where((a) => a.isNotEmpty).toList();
}

class SingleInstance {
  SingleInstance._(this._lock, this._server, this._token);

  // Held open for the life of the process: the lock is what makes us primary.
  // ignore: unused_field
  final RandomAccessFile _lock;
  final ServerSocket _server;
  final String _token;

  /// Becomes the running instance, or hands [args] to the one already running
  /// and returns null (the caller should exit).
  static Future<SingleInstance?> claim(String dataDir, List<String> args) async {
    await Directory(dataDir).create(recursive: true);
    final lockFile = await File('$dataDir\\instance.lock').open(mode: FileMode.append);
    try {
      await lockFile.lock(FileLock.exclusive);
    } on FileSystemException {
      await lockFile.close();
      // Started at login while Nudge is already running: nothing to hand over.
      if (!args.contains(backgroundFlag)) await _handOver(dataDir, args);
      return null;
    }
    final server = await ServerSocket.bind(InternetAddress.loopbackIPv4, 0);
    final token = _randomToken();
    await File('$dataDir\\instance.json')
        .writeAsString(jsonEncode({'port': server.port, 'token': token}), flush: true);
    return SingleInstance._(lockFile, server, token);
  }

  /// Calls [onFiles] with files from later launches (batched, since Explorer
  /// starts one process per file), and [onShow] when a launch has no files.
  void listen({
    required void Function(List<String> paths) onFiles,
    required void Function() onShow,
  }) {
    final pending = <String>[];
    Timer? batch;
    _server.listen((socket) {
      utf8.decoder.bind(socket).transform(const LineSplitter()).take(1).listen((line) {
        socket.destroy();
        final Map<String, Object?> msg;
        try {
          msg = jsonDecode(line) as Map<String, Object?>;
        } on FormatException {
          return;
        }
        if (msg['token'] != _token) return;
        final files = (msg['files'] as List?)?.whereType<String>().toList() ?? const [];
        if (files.isEmpty) {
          onShow();
          return;
        }
        pending.addAll(files);
        batch?.cancel();
        batch = Timer(const Duration(milliseconds: 400), () {
          onFiles(List.of(pending));
          pending.clear();
        });
      }, onError: (_) => socket.destroy());
    });
  }

  /// Send our files to the running instance. It may still be starting up,
  /// so keep trying for a few seconds.
  static Future<void> _handOver(String dataDir, List<String> args) async {
    final files = filesFromArgs(args);
    final deadline = DateTime.now().add(const Duration(seconds: 10));
    while (DateTime.now().isBefore(deadline)) {
      try {
        final info = jsonDecode(
          await File('$dataDir\\instance.json').readAsString(),
        ) as Map<String, Object?>;
        final socket = await Socket.connect(
          InternetAddress.loopbackIPv4,
          info['port'] as int,
          timeout: const Duration(seconds: 1),
        );
        socket.writeln(jsonEncode({'token': info['token'], 'files': files}));
        await socket.flush();
        await socket.close();
        return;
      } catch (_) {
        await Future<void>.delayed(const Duration(milliseconds: 200));
      }
    }
  }

  static String _randomToken() {
    final random = Random.secure();
    return base64Url.encode(List.generate(18, (_) => random.nextInt(256)));
  }
}
