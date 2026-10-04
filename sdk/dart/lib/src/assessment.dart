import 'dart:convert';

import 'json.dart';

/// A fixed-code failure. Failed assessments always require review.
class E2EMError implements Exception {
  const E2EMError(this.code);
  final String code;
  String get action => 'review';
  @override
  String toString() => 'E2EMError($code)';
}

/// Convert half-open UTF-8 byte offsets to Dart/Flutter UTF-16 offsets.
List<int> utf16Span(String text, int start, int end) {
  final bytes = utf8.encode(text);
  try {
    if (start < 0 || end < start || end > bytes.length)
      throw const FormatException();
    int offset(int byte) {
      final prefix = utf8.decode(bytes.sublist(0, byte));
      // Dart's decoder strips a leading BOM; offsets must include it.
      return prefix.length +
          (byte >= 3 && bytes[0] == 0xef && bytes[1] == 0xbb && bytes[2] == 0xbf
              ? 1
              : 0);
    }

    return [offset(start), offset(end)];
  } catch (_) {
    throw const E2EMError('INVALID_REQUEST');
  }
}

/// An immutable report bound to a complete request snapshot.
class Assessment {
  Assessment._(this.value, this.request, this._snapshot);
  final Json value;
  final Json request;
  final String _snapshot;
  String get status => value['status'] as String;
  String get action => value['action'] as String;
  Json get coverage => value['coverage'] as Json;
  List<Json> get findings => List<Json>.unmodifiable(
      (value['findings'] as List<dynamic>).cast<Json>());
  Map<String, double> get scores => Map.unmodifiable({
        for (final finding in findings)
          if (finding['score'] != null)
            (finding['category'] ?? finding['rule_id']) as String:
                (finding['score'] as num).toDouble(),
      });
  List<String> get unevaluated {
    final rules = (request['policy']?['rules'] as List<dynamic>?) ?? [];
    final names = {
      for (final rule in rules) (rule as Json)['id']: rule['category']
    };
    return List<String>.unmodifiable(
        (coverage['unevaluated_rules'] as List<dynamic>)
            .map((dynamic id) => (names[id] ?? id) as String));
  }

  bool appliesTo(Json current) {
    try {
      return snapshot(current) == _snapshot;
    } catch (_) {
      return false;
    }
  }

  /// Wire report only: original message and custom policy text are omitted.
  Json toJson() => value;
  @override
  String toString() => 'Assessment(status: $status, action: $action)';
}

Assessment validateAssessment(dynamic value, Json request) {
  try {
    if (!valid(value, responseSchema[r'$defs']['Assessment'] as Json)) throw 0;
    final report = value as Json;
    final message = request['message'] as Json;
    if (report['api_version'] != '0.1' ||
        report['request_id'] != request['request_id'] ||
        report['message_id'] != message['id'] ||
        report['message_revision'] != message['revision']) throw 0;
    final status = report['status'];
    final selected = (request['policy'] ?? request['policy_ref']) as Json;
    final versions = report['versions'] as Json;
    if (['assessed', 'indeterminate'].contains(status) &&
        (versions['policy_id'] != selected['id'] ||
            versions['policy_version'] != selected['version'])) throw 0;
    if (status != 'assessed' && report['action'] != 'review') throw 0;
    if (status == 'error' && report['error_code'] is! String) throw 0;
    final coverage = report['coverage'] as Json;
    if (status == 'assessed' &&
        (coverage['target_complete'] != true ||
            coverage['context_complete'] != true ||
            (coverage['unevaluated_rules'] as List<dynamic>).isNotEmpty))
      throw 0;
    final texts = <String, String>{
      message['id'] as String: message['text'] as String
    };
    for (final dynamic turn in (request['context'] as List<dynamic>?) ?? []) {
      texts[turn['id'] as String] = turn['text'] as String;
    }
    for (final dynamic finding in report['findings'] as List<dynamic>) {
      final dynamic score = finding['score'];
      if (score != null &&
          (score is! num || !score.isFinite || score < 0 || score > 1)) throw 0;
      for (final dynamic span in finding['spans'] as List<dynamic>) {
        utf16Span(texts[span['message_id']]!, span['start'] as int,
            span['end'] as int);
      }
    }
    return Assessment._(immutable(report) as Json, immutable(request) as Json,
        snapshot(request));
  } catch (_) {
    throw const E2EMError('INTERNAL_ERROR');
  }
}
