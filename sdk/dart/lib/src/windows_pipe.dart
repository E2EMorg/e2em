import 'dart:async';
import 'dart:ffi';

import 'package:ffi/ffi.dart';

import 'assessment.dart';
import 'transport.dart';

// OVERLAPPED contains a union of two DWORD offsets and a pointer.
final class _Overlapped extends Struct {
  @UintPtr()
  external int internal;
  @UintPtr()
  external int internalHigh;
  @Uint32()
  external int offset;
  @Uint32()
  external int offsetHigh;
  @IntPtr()
  external int event;
}

/// Local Windows named pipes with overlapped IO. No blocking reads occur on
/// Flutter's UI isolate. Handles and buffers stay alive until IO completes.
class WindowsPipe implements Transport {
  WindowsPipe._(this._api, this._handle) {
    unawaited(_receive());
  }
  final _WindowsApi _api;
  final int _handle;
  final _input = StreamController<List<int>>();
  bool _closed = false;
  Future<void> _writes = Future<void>.value();

  static Future<WindowsPipe> connect(String name) async {
    const prefix = r'\\.\pipe\e2em-';
    if (name.length > 200 ||
        !name.startsWith(prefix) ||
        !RegExp(r'^[A-Za-z0-9-]+$').hasMatch(name.substring(prefix.length))) {
      throw const E2EMError('MODEL_UNAVAILABLE');
    }
    final api = _WindowsApi();
    final nativeName = name.toNativeUtf16();
    final watch = Stopwatch()..start();
    try {
      while (true) {
        // GENERIC_READ | GENERIC_WRITE, OPEN_EXISTING, FILE_FLAG_OVERLAPPED,
        // SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION. A pipe server may
        // identify this client but may not impersonate it.
        final handle = api.createFile(
            nativeName, 0xc0000000, 0, nullptr, 3, 0x40110000, 0);
        if (handle != -1) return WindowsPipe._(api, handle);
        if (api.lastError() != 231 || watch.elapsedMilliseconds >= 5000) {
          throw const E2EMError('MODEL_UNAVAILABLE');
        }
        await Future<void>.delayed(const Duration(milliseconds: 10));
      }
    } finally {
      calloc.free(nativeName);
    }
  }

  @override
  Stream<List<int>> get input => _input.stream;

  Future<int> _io(Pointer<Uint8> buffer, int length, bool writing) async {
    if (_closed) throw const E2EMError('MODEL_UNAVAILABLE');
    final overlapped = calloc<_Overlapped>();
    final count = calloc<Uint32>();
    final event = _api.createEvent(nullptr, 1, 0, nullptr);
    if (event == 0) {
      calloc.free(overlapped);
      calloc.free(count);
      throw const E2EMError('MODEL_UNAVAILABLE');
    }
    overlapped.ref.event = event;
    var pending = false;
    try {
      final result = (writing ? _api.writeFile : _api.readFile)(
          _handle, buffer, length, count, overlapped);
      if (result == 0) {
        if (_api.lastError() != 997) throw const E2EMError('MODEL_UNAVAILABLE');
      }
      // GetOverlappedResult supplies the byte count for asynchronous IO,
      // including operations that completed immediately.
      pending = true;
      while (_api.getResult(_handle, overlapped, count, 0) == 0) {
        if (_api.lastError() != 996) {
          pending = false;
          throw const E2EMError('MODEL_UNAVAILABLE');
        }
        await Future<void>.delayed(const Duration(milliseconds: 2));
      }
      pending = false;
      return count.value;
    } finally {
      if (pending) {
        _api.cancel(_handle, overlapped);
        // Cancellation must complete before freeing native IO buffers.
        _api.getResult(_handle, overlapped, count, 1);
      }
      _api.close(event);
      calloc.free(count);
      calloc.free(overlapped);
    }
  }

  Future<void> _receive() async {
    final buffer = calloc<Uint8>(8192);
    try {
      while (!_closed) {
        final count = await _io(buffer, 8192, false);
        if (count == 0) throw const E2EMError('MODEL_UNAVAILABLE');
        _input.add(List<int>.from(buffer.asTypedList(count)));
      }
    } catch (_) {
      if (!_closed) _input.addError(const E2EMError('MODEL_UNAVAILABLE'));
      close();
    } finally {
      calloc.free(buffer);
      // close() cancels IO, but the handle is retained until both operations
      // have finished so GetOverlappedResult can observe their completion.
      await _writes.catchError((Object _) {});
      _api.close(_handle);
      await _input.close();
    }
  }

  @override
  Future<void> write(List<int> bytes) {
    final operation = _writes.then((_) async {
      if (_closed) throw const E2EMError('MODEL_UNAVAILABLE');
      final buffer = calloc<Uint8>(bytes.length);
      buffer.asTypedList(bytes.length).setAll(0, bytes);
      try {
        var offset = 0;
        while (offset < bytes.length) {
          final count = await _io(buffer + offset, bytes.length - offset, true);
          if (count == 0) throw const E2EMError('MODEL_UNAVAILABLE');
          offset += count;
        }
      } finally {
        calloc.free(buffer);
      }
    });
    _writes = operation.catchError((Object _) {});
    return operation;
  }

  @override
  void close() {
    if (_closed) return;
    _closed = true;
    _api.cancel(_handle, nullptr);
  }
}

class _WindowsApi {
  final _dll = DynamicLibrary.open('kernel32.dll');
  late final createFile = _dll.lookupFunction<
      IntPtr Function(Pointer<Utf16>, Uint32, Uint32, Pointer<Void>, Uint32,
          Uint32, IntPtr),
      int Function(Pointer<Utf16>, int, int, Pointer<Void>, int, int,
          int)>('CreateFileW');
  late final createEvent = _dll.lookupFunction<
      IntPtr Function(Pointer<Void>, Int32, Int32, Pointer<Utf16>),
      int Function(Pointer<Void>, int, int, Pointer<Utf16>)>('CreateEventW');
  late final readFile = _dll.lookupFunction<
      Int32 Function(IntPtr, Pointer<Uint8>, Uint32, Pointer<Uint32>,
          Pointer<_Overlapped>),
      int Function(int, Pointer<Uint8>, int, Pointer<Uint32>,
          Pointer<_Overlapped>)>('ReadFile');
  late final writeFile = _dll.lookupFunction<
      Int32 Function(IntPtr, Pointer<Uint8>, Uint32, Pointer<Uint32>,
          Pointer<_Overlapped>),
      int Function(int, Pointer<Uint8>, int, Pointer<Uint32>,
          Pointer<_Overlapped>)>('WriteFile');
  late final getResult = _dll.lookupFunction<
      Int32 Function(IntPtr, Pointer<_Overlapped>, Pointer<Uint32>, Int32),
      int Function(int, Pointer<_Overlapped>, Pointer<Uint32>,
          int)>('GetOverlappedResult');
  late final cancel = _dll.lookupFunction<
      Int32 Function(IntPtr, Pointer<_Overlapped>),
      int Function(int, Pointer<_Overlapped>)>('CancelIoEx');
  late final close = _dll
      .lookupFunction<Int32 Function(IntPtr), int Function(int)>('CloseHandle');
  late final lastError =
      _dll.lookupFunction<Uint32 Function(), int Function()>('GetLastError');
}
