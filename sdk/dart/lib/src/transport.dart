import 'dart:async';
import 'dart:io';
import 'dart:typed_data';

import 'assessment.dart';
import 'local.dart';
import 'windows_pipe.dart';

abstract class Transport {
  Stream<List<int>> get input;
  Future<void> write(List<int> bytes);
  void close();

  static Future<Transport> connect(String socketPath) async {
    try {
      checkDesktop();
      if (Platform.isWindows) return await WindowsPipe.connect(socketPath);
      checkSocket(socketPath);
      final socket = await Socket.connect(
          InternetAddress(socketPath, type: InternetAddressType.unix), 0,
          timeout: const Duration(seconds: 5));
      return _UnixSocket(socket);
    } catch (_) {
      throw const E2EMError('MODEL_UNAVAILABLE');
    }
  }
}

class _UnixSocket implements Transport {
  _UnixSocket(this.socket);
  final Socket socket;
  @override
  Stream<Uint8List> get input => socket;
  @override
  Future<void> write(List<int> bytes) async {
    socket.add(bytes);
    await socket.flush();
  }

  @override
  void close() => socket.destroy();
}
