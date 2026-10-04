import 'dart:ffi';
import 'dart:io';
import 'dart:convert';

import 'package:ffi/ffi.dart';
import 'package:path/path.dart' as path;
import 'package:posix/posix.dart' as posix;

import 'assessment.dart';
import 'json.dart';

final _diagnostics = <String>{};
void diagnose(String stage, [Object? error, StackTrace? stack]) {
  if (Platform.environment['E2EM_DART_DIAGNOSTICS'] == '1' &&
      _diagnostics.add(stage)) {
    // Stages, error types and call sites contain no credential or message data.
    stderr.writeln('Dart transport: $stage (${error.runtimeType})');
    if (stack != null) stderr.writeln(stack);
  }
}

void checkDesktop() {
  if (!Platform.isLinux && !Platform.isMacOS && !Platform.isWindows) {
    throw const E2EMError('MODEL_UNAVAILABLE');
  }
}

void checkSocket(String socketPath) {
  final parent = posix.lstat(path.dirname(socketPath));
  final socket = posix.lstat(socketPath);
  for (final entry in [parent, socket]) {
    if (entry.uid != posix.getuid() || entry.mode.mode & 0x3f != 0) throw 0;
  }
  if (!parent.mode.isDirectory || !socket.mode.isSocket) throw 0;
}

Json connection(String app, String? configPath) {
  try {
    checkDesktop();
    if (!RegExp(r'^[a-zA-Z0-9_-]{1,64}$').hasMatch(app)) throw 0;
    final config = configPath ??
        (Platform.isWindows
            ? path.join(
                Platform.environment['LOCALAPPDATA']!, 'E2EM', 'app-$app.json')
            : path.join(Platform.environment['HOME']!, '.config', 'e2em',
                'apps', '$app.json'));
    List<int> bytes;
    if (Platform.isWindows) {
      final file = File(config);
      final info = file.statSync();
      if (info.type != FileSystemEntityType.file || info.size > 16384) throw 0;
      final handle = file.openSync();
      try {
        bytes = handle.readSync(16385);
      } finally {
        handle.closeSync();
      }
    } else {
      // Open without following a symlink and inspect the opened inode, not a
      // path that could be replaced between the check and the read.
      final libc = DynamicLibrary.process();
      final open = libc.lookupFunction<Int32 Function(Pointer<Utf8>, Int32),
          int Function(Pointer<Utf8>, int)>('open');
      final nativePath = config.toNativeUtf8();
      late int descriptor;
      try {
        // O_RDONLY | O_NOFOLLOW | O_NONBLOCK (avoid blocking on a FIFO).
        descriptor = open(nativePath, Platform.isMacOS ? 0x104 : 0x20800);
      } finally {
        calloc.free(nativePath);
      }
      if (descriptor < 0) throw 0;
      try {
        final info = posix.stat(
            '${Platform.isLinux ? '/proc/self/fd' : '/dev/fd'}/$descriptor');
        if (!info.mode.isFile ||
            info.uid != posix.getuid() ||
            info.mode.mode & 0x3f != 0 ||
            info.size > 16384) throw 0;
        bytes = [];
        while (bytes.length <= 16384) {
          // posix.read uses signed int8 buffers; normalize to UTF-8 bytes.
          final chunk = posix.read(descriptor, 16385 - bytes.length);
          if (chunk.isEmpty) break;
          bytes.addAll(chunk.map((b) => b & 0xff));
        }
      } finally {
        posix.close(descriptor);
      }
    }
    if (bytes.length > 16384) throw 0;
    final value = jsonDecode(utf8.decode(bytes)) as Json;
    for (final key in ['socket_path', 'principal', 'secret', 'provider']) {
      if (value[key] is! String || (value[key] as String).isEmpty) throw 0;
    }
    if (value['principal'] != app) throw 0;
    return value;
  } catch (error, stack) {
    diagnose('read settings', error, stack);
    throw const E2EMError('MODEL_UNAVAILABLE');
  }
}
