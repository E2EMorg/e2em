import 'dart:convert';
import 'dart:math';

import 'data.g.dart';
import 'json.dart';

/// Built-in presets, generated from the runtime's catalogue.
final List<Json> presets = List<Json>.unmodifiable(
    ((jsonDecode(presetsJson) as Json)['presets'] as List<dynamic>)
        .map((dynamic value) => immutable(value) as Json));
final _registry = {
  for (final preset in presets) preset['id'] as String: preset
};

String nonce(int length) {
  final random = Random.secure();
  return List.generate(
          length, (_) => random.nextInt(256).toRadixString(16).padLeft(2, '0'))
      .join();
}

/// Build an outgoing request, preserving the original text and context.
Json messageRequest(
  String message, {
  List<String> context = const [],
  List<String>? policies,
  List<String> customPolicies = const [],
  String? model,
  int deadlineMs = 15000,
}) {
  final selected = policies ?? presets.map((p) => p['id'] as String).toList();
  final rules = <Json>[];
  for (final category in selected) {
    final detected = category == 'pii.email';
    final contextual = context.isNotEmpty || category == 'spam.repeat';
    rules.add({
      'id': 'preset-${rules.length + 1}',
      'category': category,
      'directions': List<String>.from(
          (_registry[category]?['directions'] as List<dynamic>?) ??
              ['outgoing', 'incoming']),
      'match': detected ? 'detected' : 'score',
      'action': 'warn',
      'context_requirement':
          contextual && !detected ? 'supplied_window' : 'target_only',
      if (!detected) ...{
        'review_threshold': 0.4,
        'action_threshold': 0.7,
        'model_thresholds': true,
        if (contextual) 'min_context_messages': 1,
      },
    });
  }
  for (var i = 0; i < customPolicies.length; i++) {
    rules.add({
      'id': 'custom-${i + 1}',
      'policy_text': customPolicies[i],
      'directions': ['outgoing', 'incoming'],
      'match': 'policy_text',
      'action': 'warn',
      'context_requirement':
          context.isEmpty ? 'target_only' : 'supplied_window',
      if (context.isNotEmpty) 'min_context_messages': 1,
    });
  }
  return {
    'api_version': '0.1',
    'request_id': 'check-${nonce(12)}',
    'direction': 'outgoing',
    'options': {'deadline_ms': deadlineMs, if (model != null) 'model': model},
    'message': {
      'id': 'message',
      'revision': '1',
      'speaker': 'self',
      'text': message
    },
    'context': [
      for (var i = 0; i < context.length; i++)
        {'id': 'context-${i + 1}', 'speaker': 'peer', 'text': context[i]}
    ],
    'policy': {
      'schema_version': '0.1',
      'id': 'message-check',
      'version': '1',
      'profile': 'personal',
      'rules': rules,
      'default_action': 'allow',
      'on_error': 'review',
      'on_indeterminate': 'review',
      'override': 'user_confirm',
    },
  };
}
