import 'dart:async';
import 'dart:collection';
import 'dart:convert';
import 'dart:typed_data';

import 'package:crypto/crypto.dart';

import 'assessment.dart';
import 'json.dart';
import 'local.dart';
import 'message.dart';
import 'transport.dart';

const _maxFrame = 131072;

/// Cancellation for one or more assessments. Cancelled reports authorize no send.
class CancellationToken {
  bool _cancelled = false;
  final _listeners = <void Function()>{};
  bool get isCancelled => _cancelled;
  void cancel() {
    if (_cancelled) return;
    _cancelled = true;
    for (final listener in _listeners.toList()) {
      listener();
    }
    _listeners.clear();
  }
}

/// An explicitly provisioned local provider for discovery.
class ProviderCandidate {
  const ProviderCandidate(
      {required this.kind,
      required this.socketPath,
      required this.principal,
      required this.secret,
      required this.provider});
  final String kind;
  final String socketPath;
  final String principal;
  final String secret;
  final String provider;
  @override
  String toString() => 'ProviderCandidate(kind: $kind, provider: $provider)';
}

class _Pending {
  _Pending(this.completer, this.timer);
  final Completer<Json> completer;
  final Timer timer;
}

/// Persistent authenticated connection to the installed desktop runtime.
class Client {
  Client._(this._transport, this.provider, this.model) {
    _subscription = _transport.input.listen(_data,
        onError: (Object _) => _fail('MODEL_UNAVAILABLE'),
        onDone: () => _fail('MODEL_UNAVAILABLE'));
  }
  final Transport _transport;
  final String provider;
  final String? model;
  late final StreamSubscription<List<int>> _subscription;
  late final Json capabilityManifest;
  final _pending = <String, _Pending>{};
  final _frames = Queue<Json>();
  final _waiters = Queue<_Pending>();
  Uint8List _buffer = Uint8List(0);
  int _counter = 0;
  bool _closed = false;
  bool _receiving = false;
  Future<void> _writes = Future<void>.value();

  static Future<Client> connect(
      {String app = 'my-app', String? configPath, String? model}) async {
    final settings = connection(app, configPath);
    return open(
        socketPath: settings['socket_path'] as String,
        principal: settings['principal'] as String,
        secret: settings['secret'] as String,
        provider: settings['provider'] as String,
        model: model);
  }

  static Future<Client> open(
      {required String socketPath,
      required String principal,
      required String secret,
      required String provider,
      String? model}) async {
    final transport = await Transport.connect(socketPath);
    final client = Client._(transport, provider, model);
    try {
      final clientNonce = nonce(32);
      client._send({'principal': principal, 'nonce': clientNonce});
      final challenge = await client._frame();
      final serverNonce = challenge['nonce'];
      if (challenge.length != 3 ||
          challenge['provider'] != provider ||
          serverNonce is! String ||
          !RegExp(r'^[a-f0-9]{64}$').hasMatch(serverNonce) ||
          !_equal(
              challenge['proof'],
              _proof(secret, 'server', provider, principal, clientNonce,
                  serverNonce))) {
        throw const E2EMError('MODEL_UNAVAILABLE');
      }
      client._send({
        'proof': _proof(
            secret, 'client', provider, principal, clientNonce, serverNonce)
      });
      final authenticated = await client._frame();
      if (authenticated.length != 2 ||
          authenticated['authenticated'] != true ||
          authenticated['provider'] != provider) {
        throw const E2EMError('MODEL_UNAVAILABLE');
      }
      client._receiving = true;
      while (client._frames.isNotEmpty) {
        client._dispatch(client._frames.removeFirst());
      }
      final caps = await client.capabilities();
      if (caps['api_version'] != '0.1' || caps['backend_ready'] != true) {
        throw const E2EMError('MODEL_UNAVAILABLE');
      }
      client.capabilityManifest = immutable(caps) as Json;
      return client;
    } catch (error) {
      client.close();
      throw error is E2EMError ? error : const E2EMError('MODEL_UNAVAILABLE');
    }
  }

  static Future<Client> discover(
    List<ProviderCandidate> candidates, {
    List<String> requiredDetectors = const [],
    String? pinned,
  }) async {
    const order = {'native': 0, 'project': 1, 'embedded': 2};
    if (candidates.any((c) => !order.containsKey(c.kind)))
      throw const E2EMError('INVALID_REQUEST');
    final sorted = candidates.toList()
      ..sort((a, b) => order[a.kind]!.compareTo(order[b.kind]!));
    for (final candidate in sorted) {
      if (pinned != null && candidate.provider != pinned) continue;
      try {
        final client = await open(
            socketPath: candidate.socketPath,
            principal: candidate.principal,
            secret: candidate.secret,
            provider: candidate.provider);
        final detectors =
            client.capabilityManifest['detectors'] as List<dynamic>;
        if (requiredDetectors.every(detectors.contains)) return client;
        client.close();
      } on E2EMError {
        continue;
      }
    }
    throw const E2EMError('MODEL_UNAVAILABLE');
  }

  void _data(List<int> chunk) {
    if (_closed) return;
    try {
      _buffer = Uint8List.fromList([..._buffer, ...chunk]);
      while (_buffer.length >= 4) {
        final size = ByteData.sublistView(_buffer).getUint32(0);
        if (size == 0 || size > _maxFrame) throw 0;
        if (_buffer.length < size + 4) break;
        final frame =
            jsonDecode(utf8.decode(_buffer.sublist(4, size + 4))) as Json;
        _buffer = Uint8List.fromList(_buffer.sublist(size + 4));
        if (_waiters.isNotEmpty) {
          final waiter = _waiters.removeFirst();
          waiter.timer.cancel();
          waiter.completer.complete(frame);
        } else if (_receiving) {
          _dispatch(frame);
        } else {
          if (_frames.length >= 4) throw 0;
          _frames.add(frame);
        }
      }
    } catch (_) {
      _fail('INTERNAL_ERROR');
    }
  }

  Future<Json> _frame() {
    if (_frames.isNotEmpty) return Future.value(_frames.removeFirst());
    if (_closed) return Future.error(const E2EMError('MODEL_UNAVAILABLE'));
    final completer = Completer<Json>();
    final timer =
        Timer(const Duration(seconds: 5), () => _fail('DEADLINE_EXCEEDED'));
    _waiters.add(_Pending(completer, timer));
    return completer.future;
  }

  void _send(Json value) {
    late List<int> bytes;
    try {
      bytes = utf8.encode(jsonEncode(value));
    } catch (_) {
      throw const E2EMError('INVALID_REQUEST');
    }
    if (bytes.isEmpty || bytes.length > _maxFrame)
      throw const E2EMError('INVALID_REQUEST');
    final prefix = ByteData(4)..setUint32(0, bytes.length);
    final frame = [...prefix.buffer.asUint8List(), ...bytes];
    _writes = _writes.then<void>((_) async {
      if (!_closed) {
        await _transport.write(frame).timeout(const Duration(seconds: 5));
      }
    }).catchError((Object error) => _fail(
        error is TimeoutException ? 'DEADLINE_EXCEEDED' : 'MODEL_UNAVAILABLE'));
  }

  void _dispatch(Json response) {
    if (!valid(response)) {
      _fail('INTERNAL_ERROR');
      return;
    }
    final pending = _pending.remove(response['call_id']);
    if (pending != null) {
      pending.timer.cancel();
      pending.completer.complete(response['reply'] as Json);
    }
  }

  Future<Json> _call(Json operation, String expected,
      {int timeoutMs = 5000}) async {
    if (_closed) throw const E2EMError('MODEL_UNAVAILABLE');
    if (_pending.length >= 8) throw const E2EMError('RESOURCE_EXHAUSTED');
    final id = '${++_counter}';
    final completer = Completer<Json>();
    final timer = Timer(Duration(milliseconds: timeoutMs), () {
      _pending.remove(id);
      completer.completeError(const E2EMError('DEADLINE_EXCEEDED'));
    });
    _pending[id] = _Pending(completer, timer);
    try {
      _send({'call_id': id, 'api_version': '0.1', 'operation': operation});
      final reply = await completer.future;
      if (reply['kind'] == 'error')
        throw E2EMError(reply['error_code'] as String);
      if (reply['kind'] != expected) throw const E2EMError('INTERNAL_ERROR');
      return reply;
    } finally {
      timer.cancel();
      _pending.remove(id);
    }
  }

  Future<Json> capabilities() async =>
      (await _call({'op': 'capabilities'}, 'capabilities'))['capabilities']
          as Json;
  Future<Json> validatePolicy(Json policy) async => (await _call(
          {'op': 'validate_policy', 'policy': policy}, 'policy'))['policy_ref']
      as Json;
  Future<Json> models() => _call({'op': 'models'}, 'models');
  Future<Json> installModel(String source,
          {String name = 'custom', bool autoUpdate = false}) =>
      _call({
        'op': 'install_model',
        'source': source,
        'name': name,
        'auto_update': autoUpdate
      }, 'models', timeoutMs: 900000);
  Future<bool> cancel(String requestId) async => (await _call(
          {'op': 'cancel', 'request_id': requestId}, 'cancelled'))['accepted']
      as bool;

  /// Assess text, or an advanced JSON request with explicit IDs and revisions.
  Future<Assessment> assess(
    Object request, {
    List<String>? context,
    List<String>? policies,
    List<String>? customPolicies,
    String? model,
    int deadlineMs = 15000,
    CancellationToken? cancellation,
  }) async {
    if (cancellation?.isCancelled == true) throw const E2EMError('CANCELLED');
    late Json original;
    try {
      if (request is String) {
        original = messageRequest(request,
            context: context ?? [],
            policies: policies,
            customPolicies: customPolicies ?? [],
            model: model ?? this.model,
            deadlineMs: deadlineMs);
      } else {
        if (context != null || policies != null || customPolicies != null)
          throw 0;
        original = jsonDecode(jsonEncode(request)) as Json;
        if (model != null || this.model != null) {
          final options = (original['options'] as Json?) ?? <String, dynamic>{};
          options.putIfAbsent('model', () => model ?? this.model);
          original['options'] = options;
        }
      }
      if (original['request_id'] is! String ||
          original['message'] is! Json ||
          (original['options']?['deadline_ms'] ?? 15000) is! int) throw 0;
    } catch (_) {
      throw const E2EMError('INVALID_REQUEST');
    }
    final cancelled = Completer<Json>();
    void abort() => cancelled.completeError(const E2EMError('CANCELLED'));
    cancellation?._listeners.add(abort);
    try {
      final response = _call(
          {'op': 'assess', 'request': original}, 'assessment',
          timeoutMs:
              (original['options']?['deadline_ms'] as int? ?? 15000) + 1000);
      final reply = await Future.any(
          [response, if (cancellation != null) cancelled.future]);
      if (cancellation?.isCancelled == true) throw const E2EMError('CANCELLED');
      return validateAssessment(reply['assessment'], original);
    } catch (error) {
      if (!_closed) {
        try {
          await cancel(original['request_id'] as String);
        } catch (_) {/* Best effort. */}
      }
      throw error is E2EMError ? error : const E2EMError('INTERNAL_ERROR');
    } finally {
      cancellation?._listeners.remove(abort);
    }
  }

  void _fail(String code) {
    if (_closed) return;
    _closed = true;
    _transport.close();
    unawaited(_subscription.cancel());
    for (final pending in [..._pending.values, ..._waiters]) {
      pending.timer.cancel();
      pending.completer.completeError(E2EMError(code));
    }
    _pending.clear();
    _waiters.clear();
    _frames.clear();
    _buffer = Uint8List(0);
  }

  /// Idempotently close the connection, failing any outstanding calls.
  void close() => _fail('MODEL_UNAVAILABLE');
}

String _proof(String secret, String role, String provider, String principal,
        String client, String server) =>
    Hmac(sha256, utf8.encode(secret))
        .convert(utf8.encode(
            '$role\u0000$provider\u0000$principal\u0000$client\u0000$server\u0000'))
        .toString();

bool _equal(dynamic received, String expected) {
  if (received is! String || received.length != expected.length) return false;
  var difference = 0;
  for (var i = 0; i < expected.length; i++) {
    difference |= received.codeUnitAt(i) ^ expected.codeUnitAt(i);
  }
  return difference == 0;
}

/// Assess once using installer-managed local app settings.
Future<Assessment> assess(
  String message, {
  String app = 'my-app',
  String? configPath,
  String? model,
  List<String> context = const [],
  List<String>? policies,
  List<String> customPolicies = const [],
  int deadlineMs = 15000,
  CancellationToken? cancellation,
}) async {
  if (cancellation?.isCancelled == true) throw const E2EMError('CANCELLED');
  final client =
      await Client.connect(app: app, configPath: configPath, model: model);
  try {
    return await client.assess(message,
        context: context,
        policies: policies,
        customPolicies: customPolicies,
        deadlineMs: deadlineMs,
        cancellation: cancellation);
  } finally {
    client.close();
  }
}
