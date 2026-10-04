import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'dart:typed_data';

import 'package:crypto/crypto.dart';
import 'package:e2em_local/e2em.dart';
import 'package:e2em_local/src/json.dart' show Json;
import 'package:posix/posix.dart' as posix;
import 'package:test/test.dart';

class FakeRuntime {
  late Directory directory;
  late ServerSocket listener;
  final sockets = <Socket>[];
  Future<Json?> Function(Json)? onCall;
  bool malformed = false;
  bool badProof = false;

  Future<void> start() async {
    directory = Directory.systemTemp.createTempSync('e2em-dart-transport-');
    posix.chmod(directory.path, '0700');
    listener = await ServerSocket.bind(
        InternetAddress('${directory.path}/runtime.sock',
            type: InternetAddressType.unix),
        0);
    posix.chmod('${directory.path}/runtime.sock', '0600');
    listener.listen((socket) {
      sockets.add(socket);
      unawaited(serve(socket).catchError((Object _) {
        socket.destroy();
      }));
    });
  }

  Future<Client> connect() => Client.open(
      socketPath: '${directory.path}/runtime.sock',
      principal: 'daccord',
      secret: 'a' * 64,
      provider: 'fake-provider');

  Future<void> serve(Socket socket) async {
    final input = StreamIterator<Uint8List>(socket);
    var buffer = <int>[];
    Future<Json> read() async {
      while (buffer.length < 4) {
        if (!await input.moveNext()) throw StateError('closed');
        buffer.addAll(input.current);
      }
      final size =
          ByteData.sublistView(Uint8List.fromList(buffer)).getUint32(0);
      while (buffer.length < size + 4) {
        if (!await input.moveNext()) throw StateError('closed');
        buffer.addAll(input.current);
      }
      final value =
          jsonDecode(utf8.decode(buffer.sublist(4, size + 4))) as Json;
      buffer = buffer.sublist(size + 4);
      return value;
    }

    Future<void> send(Json value) async {
      final bytes = utf8.encode(jsonEncode(value));
      final header = ByteData(4)..setUint32(0, bytes.length);
      // Split the header and payload to exercise fragmented socket delivery.
      socket.add(header.buffer.asUint8List().sublist(0, 2));
      await socket.flush();
      await Future<void>.delayed(const Duration(milliseconds: 1));
      socket.add([...header.buffer.asUint8List().sublist(2), ...bytes]);
      await socket.flush();
    }

    try {
      final hello = await read();
      final serverNonce = 'b' * 64;
      final parts = [
        'server',
        'fake-provider',
        'daccord',
        hello['nonce'] as String,
        serverNonce
      ];
      final proof = Hmac(sha256, utf8.encode('a' * 64))
          .convert(utf8.encode('${parts.join('\u0000')}\u0000'))
          .toString();
      await send({
        'provider': 'fake-provider',
        'nonce': serverNonce,
        'proof': badProof ? '0' * 64 : proof
      });
      await read();
      await send({'authenticated': true, 'provider': 'fake-provider'});
      while (true) {
        final call = await read();
        Json? reply;
        final operation = call['operation'] as Json;
        if (operation['op'] == 'capabilities') {
          reply = {
            'kind': 'capabilities',
            'capabilities': {
              'api_version': '0.1',
              'provider': 'fake-provider',
              'runtime': 'test',
              'model': 'none',
              'tokenizer': 'none',
              'registry': 'test',
              'languages': ['en'],
              'detectors': ['pii.email'],
              'presets': <String>[],
              'custom_policies': 'none',
              'profiles': ['personal'],
              'actions': ['allow', 'warn', 'review'],
              'limits': {
                for (final name in [
                  'message_bytes',
                  'request_text_bytes',
                  'context_turns',
                  'policy_rules',
                  'frame_bytes',
                  'result_spans',
                  'deadline_ms'
                ])
                  name: 100
              },
              'backend_ready': true,
              'runtime_state': 'unloaded',
              'max_tokens': null,
            }
          };
        } else if (operation['op'] == 'cancel') {
          reply = {'kind': 'cancelled', 'accepted': true};
        } else if (malformed) {
          reply = {'kind': 'assessment', 'assessment': <String, dynamic>{}};
        } else {
          reply = await onCall?.call(operation);
        }
        if (reply != null)
          await send({'call_id': call['call_id'], 'reply': reply});
      }
    } finally {
      await input.cancel();
      socket.destroy();
    }
  }

  Future<void> close() async {
    for (final socket in sockets) {
      socket.destroy();
    }
    await listener.close();
    directory.deleteSync(recursive: true);
  }
}

void main() {
  group('authenticated Unix transport', () {
    late FakeRuntime runtime;
    setUp(() async {
      runtime = FakeRuntime();
      await runtime.start();
    });
    tearDown(() => runtime.close());

    test('fragmented frames authenticate and nullable capabilities validate',
        () async {
      final client = await runtime.connect();
      expect(client.capabilityManifest['max_tokens'], isNull);
      client.close();
    });
    test('provider proof failure cannot open a client', () async {
      runtime.badProof = true;
      await expectLater(runtime.connect(), throwsA(isA<E2EMError>()));
    });
    test('malformed replies fail pending calls with review', () async {
      final client = await runtime.connect();
      runtime.malformed = true;
      await expectLater(
          client.assess('Hello'),
          throwsA(isA<E2EMError>()
              .having((e) => e.code, 'code', 'INTERNAL_ERROR')
              .having((e) => e.action, 'action', 'review')));
      client.close();
    });
    test('in-flight cancellation sends cancel without allowing a report',
        () async {
      final client = await runtime.connect();
      final token = CancellationToken();
      final response = client.assess('Hello', cancellation: token);
      final checked = expectLater(response,
          throwsA(isA<E2EMError>().having((e) => e.code, 'code', 'CANCELLED')));
      token.cancel();
      await checked;
      expect(await client.capabilities(), isNotEmpty);
      client.close();
    });
    test('deadline expiry remains a typed review failure', () async {
      final client = await runtime.connect();
      await expectLater(
          client.assess('Hello', deadlineMs: 1),
          throwsA(isA<E2EMError>()
              .having((e) => e.code, 'code', 'DEADLINE_EXCEEDED')));
      client.close();
    });
    test('frame bounds and unserializable requests do not break the connection',
        () async {
      final client = await runtime.connect();
      final invalid = messageRequest('Hello')..['invalid'] = Object();
      await expectLater(
          client.assess(invalid),
          throwsA(isA<E2EMError>()
              .having((e) => e.code, 'code', 'INVALID_REQUEST')));
      await expectLater(
          client.assess('x' * 131072),
          throwsA(isA<E2EMError>()
              .having((e) => e.code, 'code', 'INVALID_REQUEST')));
      expect(await client.capabilities(), isNotEmpty);
      client.close();
    });
    test('eight-call bound and closing fail outstanding work', () async {
      final client = await runtime.connect();
      final outstanding = List.generate(8, (i) => client.assess('Message $i'));
      final checked = Future.wait(outstanding.map((future) => expectLater(
          future,
          throwsA(isA<E2EMError>()
              .having((e) => e.code, 'code', 'MODEL_UNAVAILABLE')))));
      await expectLater(
          client.assess('Ninth'),
          throwsA(isA<E2EMError>()
              .having((e) => e.code, 'code', 'RESOURCE_EXHAUSTED')));
      client.close();
      await checked;
    });
  }, skip: Platform.isWindows);
}
