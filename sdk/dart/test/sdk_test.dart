import 'dart:convert';
import 'dart:io';

import 'package:e2em_local/e2em.dart';
import 'package:e2em_local/src/assessment.dart' show validateAssessment;
import 'package:e2em_local/src/json.dart' show Json, valid;
import 'package:e2em_local/src/local.dart' show connection;
import 'package:e2em_local/src/windows_pipe.dart' show WindowsPipe;
import 'package:posix/posix.dart' as posix;
import 'package:test/test.dart';

Json report(Json request) => {
      'api_version': '0.1',
      'request_id': request['request_id'],
      'message_id': request['message']['id'],
      'message_revision': request['message']['revision'],
      'status': 'assessed',
      'action': 'allow',
      'versions': {
        'runtime': '0.1',
        'model': 'test',
        'detectors': '1',
        'registry': '1',
        'policy_id': request['policy']['id'],
        'policy_version': request['policy']['version']
      },
      'coverage': {
        'language': 'en',
        'target_complete': true,
        'context_complete': true,
        'dropped_context_ids': <String>[],
        'unevaluated_rules': <String>[]
      },
      'findings': <Json>[],
      'reason_codes': <String>[],
      'duration_ms': 0,
    };

void main() {
  test('message defaults, context, presets and custom-only policy', () {
    final request = messageRequest('Original 🙂 draft');
    final rules = request['policy']['rules'] as List<Json>;
    expect(presets, hasLength(40));
    expect(rules.map((r) => r['category']), presets.map((p) => p['id']));
    expect(request['message']['text'], 'Original 🙂 draft');
    expect(
        rules.firstWhere(
            (r) => r['category'] == 'spam.repeat')['min_context_messages'],
        1);
    final custom = messageRequest('Hello',
        policies: [],
        customPolicies: ['Keep launch dates private.'],
        context: ['Earlier']);
    expect(custom['policy']['rules'], hasLength(1));
    expect(
        custom['policy']['rules'][0]['context_requirement'], 'supplied_window');
    expect(custom['context'][0]['text'], 'Earlier');
    expect(() => presets[0]['directions'].add('other'), throwsUnsupportedError);
  });
  test('UTF-8 spans preserve emoji, BOMs and reject split code points', () {
    expect(utf16Span('🙂 alex@example.test', 5, 22), [3, 20]);
    expect(utf16Span('\ufeff🙂 alex@example.test', 8, 25), [4, 21]);
    for (final span in [
      [1, 5],
      [-1, 2],
      [0, 100],
      [4, 1]
    ]) {
      expect(
          () => utf16Span('🙂 x', span[0], span[1]), throwsA(isA<E2EMError>()));
    }
  });
  test('assessment snapshots are immutable and include context and policy', () {
    final request = messageRequest('Hello', context: ['Earlier']);
    final assessment = validateAssessment(report(request), request);
    expect(assessment.appliesTo(request), isTrue);
    final reordered =
        Map<String, dynamic>.fromEntries(request.entries.toList().reversed);
    expect(assessment.appliesTo(reordered), isTrue);
    request['context'][0]['text'] = 'Edited';
    expect(assessment.appliesTo(request), isFalse);
    expect(() => assessment.request['message']['text'] = 'Edited',
        throwsUnsupportedError);
    expect(assessment.toString(), isNot(contains('Hello')));
    expect(jsonEncode(assessment), isNot(contains('Hello')));
  });
  test('malformed, stale and incomplete assessments never authorize a send',
      () {
    final request = messageRequest('Hello');
    final wire = report(request);
    for (final change in <Json>[
      {'status': 'error'},
      {'status': 'indeterminate'},
      {'api_version': '9'},
      {'message_revision': 'old'},
      {'request_id': 'other'},
      {'duration_ms': -1},
      {
        'versions': {...wire['versions'] as Json, 'policy_version': 'old'}
      },
      {
        'coverage': {...wire['coverage'] as Json, 'target_complete': false}
      },
      {
        'findings': [<String, dynamic>{}]
      },
      {'unexpected': true},
    ]) {
      expect(
          () => validateAssessment({...wire, ...change}, request),
          throwsA(isA<E2EMError>()
              .having((e) => e.code, 'code', 'INTERNAL_ERROR')));
    }
  });
  test('nullable numeric schema constraints do not reject null', () {
    final rule = <String, dynamic>{
      'type': ['integer', 'null'],
      'minimum': 0
    };
    expect(valid(null, rule), isTrue);
    expect(valid(-1, rule), isFalse);
  });
  test(
      'settings reject missing files, malformed app IDs and principal mismatch',
      () {
    expect(() => connection('../invalid', null), throwsA(isA<E2EMError>()));
    expect(() => connection('daccord', '/missing-e2em-settings'),
        throwsA(isA<E2EMError>()));
  });
  test('Unix settings reject broad permissions, symlinks and oversized files',
      () {
    final directory = Directory.systemTemp.createTempSync('e2em-dart-');
    final file = File('${directory.path}/settings.json');
    try {
      file.writeAsStringSync(jsonEncode({
        'socket_path': '/socket',
        'principal': 'daccord',
        'secret': 'a' * 64,
        'provider': 'test-provider'
      }));
      posix.chmod(file.path, '0600');
      expect(connection('daccord', file.path)['principal'], 'daccord');
      expect(() => connection('other', file.path), throwsA(isA<E2EMError>()));
      final link = Link('${directory.path}/link')..createSync(file.path);
      expect(() => connection('daccord', link.path), throwsA(isA<E2EMError>()));
      posix.chmod(file.path, '0644');
      expect(() => connection('daccord', file.path), throwsA(isA<E2EMError>()));
      posix.chmod(file.path, '0600');
      file.writeAsStringSync(' ' * 16385);
      expect(() => connection('daccord', file.path), throwsA(isA<E2EMError>()));
    } finally {
      directory.deleteSync(recursive: true);
    }
  }, skip: Platform.isWindows);
  test('Windows rejects remote and unrelated pipes before loading native APIs',
      () async {
    for (final name in [
      r'\\remote\pipe\e2em-app',
      r'\\.\pipe\other',
      r'\\.\pipe\e2em-',
      r'\\.\pipe\e2em-app/remote'
    ]) {
      await expectLater(WindowsPipe.connect(name), throwsA(isA<E2EMError>()));
    }
  });
  test('pre-cancelled one-shot requests require no connection', () async {
    final cancellation = CancellationToken()
      ..cancel()
      ..cancel();
    await expectLater(assess('Hello', cancellation: cancellation),
        throwsA(isA<E2EMError>().having((e) => e.code, 'code', 'CANCELLED')));
  });
}
