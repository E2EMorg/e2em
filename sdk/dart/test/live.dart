// Runs against a rules-only daemon supplied by scripts/check_dart_sdk.py.
import 'dart:convert';
import 'dart:io';

import 'package:e2em_local/e2em.dart';
import 'package:e2em_local/src/assessment.dart' show validateAssessment;
import 'package:e2em_local/src/json.dart' show Json;
import 'package:test/test.dart';

dynamic fixture(String name) =>
    jsonDecode(File('../../tests/conformance/$name.json').readAsStringSync());
Json clone(Json value) => jsonDecode(jsonEncode(value)) as Json;

void main() {
  final automaticApp = Platform.environment['E2EM_DART_AUTO_APP'];
  if (automaticApp != null) {
    test('automatic app connection requires no interaction', () async {
      final report = await assess('alex@example.test',
          app: automaticApp, policies: ['pii.email']);
      expect(report.status, 'assessed');
      expect(report.action, 'warn');
    });
  }
  test('shared live desktop conformance', () async {
    final settings = Platform.environment['E2EM_DART_CONFIG']!;
    Client? ready;
    for (var i = 0; i < 100; i++) {
      try {
        ready = await Client.connect(app: 'daccord', configPath: settings);
        break;
      } on E2EMError {
        await Future<void>.delayed(const Duration(milliseconds: 50));
      }
    }
    if (ready == null) throw StateError('Dart client could not authenticate');
    final client = ready;
    try {
      final cases = fixture('assessments') as List<dynamic>;
      for (final dynamic entry in cases) {
        final request = entry['request'] as Json;
        final result = await client.assess(request);
        expect(result.status, entry['expected']['status']);
        expect(result.action, entry['expected']['action']);
        expect([
          for (final f in result.findings)
            for (final dynamic s in f['spans'] as List<dynamic>)
              [s['start'], s['end']]
        ], entry['expected']['spans']);
        expect(result.appliesTo(request), isTrue);
        final edited = clone(request)..['message']['revision'] = 'new';
        expect(result.appliesTo(edited), isFalse);
        expect(
            () => validateAssessment(
                {...result.value, 'message_revision': 'old'}, request),
            throwsA(isA<E2EMError>()));
      }
      for (final dynamic entry in fixture('policy-failures') as List<dynamic>) {
        await expectLater(
            client.validatePolicy(entry['policy'] as Json),
            throwsA(isA<E2EMError>()
                .having((e) => e.code, 'code', entry['error_code'])));
      }
      for (final dynamic entry in fixture('policy-reports') as List<dynamic>) {
        final policy = clone(entry['policy'] as Json)..['id'] = entry['name'];
        final reference = await client.validatePolicy(policy);
        final request = clone(cases[0]['request'] as Json)..remove('policy');
        request['policy_ref'] = reference;
        final result = await client.assess(request);
        expect(result.status, entry['expected']['status']);
        expect(result.action, entry['expected']['action']);
        expect(result.coverage['unevaluated_rules'],
            entry['expected']['unevaluated_rules']);
        expect(result.value['reason_codes'], entry['expected']['reason_codes']);
      }
      final defaults = await client.assess('Hello');
      expect(defaults.request['policy']['rules'], hasLength(40));
      expect(defaults.status, 'indeterminate');
      final selected = await client.assess('🙂 alex@example.test',
          policies: ['pii.email'], context: ['Earlier']);
      expect(selected.action, 'warn');
      final custom = await client.assess('Hello',
          policies: [], customPolicies: ['Keep project details private.']);
      expect(custom.unevaluated, ['custom-1']);
      expect(custom.scores, isEmpty);
      expect(
          jsonEncode(custom), isNot(contains('Keep project details private.')));
      final once = await assess('Hello',
          app: 'daccord', configPath: settings, policies: ['pii.email']);
      expect(once.action, 'allow');
      final concurrent = await Future.wait(List.generate(
          4, (i) => client.assess('Message $i', policies: ['pii.email'])));
      expect(concurrent.every((r) => r.action == 'allow'), isTrue);
      expect(await client.cancel('missing'), isFalse);
      final token = CancellationToken()..cancel();
      await expectLater(client.assess('Hello', cancellation: token),
          throwsA(isA<E2EMError>().having((e) => e.code, 'code', 'CANCELLED')));
      final config = jsonDecode(File(settings).readAsStringSync()) as Json;
      await expectLater(
          Client.open(
              socketPath: config['socket_path'] as String,
              principal: 'daccord',
              secret: '0' * 64,
              provider: config['provider'] as String),
          throwsA(isA<E2EMError>()));
      final discovered = await Client.discover([
        ProviderCandidate(
            kind: 'project',
            socketPath: config['socket_path'] as String,
            principal: 'daccord',
            secret: config['secret'] as String,
            provider: config['provider'] as String)
      ], requiredDetectors: [
        'pii.email'
      ]);
      discovered.close();
      client.close();
      client.close();
      await expectLater(client.capabilities(), throwsA(isA<E2EMError>()));
      print(
          'Dart live desktop conformance passed (${cases.length} assessment fixtures)');
    } finally {
      client.close();
    }
  });
}
