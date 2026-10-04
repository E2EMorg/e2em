import 'dart:convert';

import 'data.g.dart';

typedef Json = Map<String, dynamic>;

final Json responseSchema = jsonDecode(responseSchemaJson) as Json;

/// Validate the structural schema emitted by the Rust contract.
bool valid(dynamic value, [Json? rule]) {
  rule ??= responseSchema;
  if (rule.containsKey(r'$ref')) {
    final name = (rule[r'$ref'] as String).split('/').last;
    return valid(value, responseSchema[r'$defs'][name] as Json);
  }
  for (final key in ['anyOf', 'oneOf']) {
    if (rule[key] is List) {
      final count = (rule[key] as List<dynamic>)
          .where((dynamic r) => valid(value, r as Json))
          .length;
      return key == 'oneOf' ? count == 1 : count > 0;
    }
  }
  if (rule.containsKey('const') && value != rule['const']) return false;
  if (rule['enum'] is List &&
      !(rule['enum'] as List<dynamic>).contains(value)) {
    return false;
  }
  final dynamic type = rule['type'];
  if (type is List) {
    return type.any((dynamic t) => valid(value, {...rule!, 'type': t}));
  }
  if (type == 'object') {
    if (value is! Json) return false;
    final required = (rule['required'] as List<dynamic>?) ?? [];
    if (required.any((dynamic k) => !value.containsKey(k))) return false;
    final properties = (rule['properties'] as Json?) ?? {};
    if (rule['additionalProperties'] == false &&
        value.keys.any((k) => !properties.containsKey(k))) return false;
    return value.entries.every((e) =>
        !properties.containsKey(e.key) ||
        valid(e.value, properties[e.key] as Json));
  }
  if (type == 'array') {
    return value is List &&
        value.every((dynamic v) => valid(v, rule!['items'] as Json));
  }
  final matches = switch (type) {
    'string' => value is String,
    'integer' => value is int,
    'number' => value is num && value.isFinite,
    'boolean' => value is bool,
    'null' => value == null,
    _ => false,
  };
  return matches &&
      (value is! num ||
          !rule.containsKey('minimum') ||
          value >= (rule['minimum'] as num));
}

dynamic immutable(dynamic value) {
  if (value is Json) {
    return Map<String, dynamic>.unmodifiable(
        value.map((k, dynamic v) => MapEntry(k, immutable(v))));
  }
  if (value is List) return List<dynamic>.unmodifiable(value.map(immutable));
  return value;
}

// Sort object keys so map insertion order does not affect revision checks.
String snapshot(Json value) {
  dynamic sorted(dynamic item) {
    if (item is Json) {
      final keys = item.keys.toList()..sort();
      return {for (final key in keys) key: sorted(item[key])};
    }
    if (item is List) return item.map(sorted).toList();
    return item;
  }

  return jsonEncode(sorted(value));
}
