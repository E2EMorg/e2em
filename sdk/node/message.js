import catalogue from './presets.json' with {type:'json'};
import {randomBytes} from 'node:crypto';

export const presets = Object.freeze(catalogue.presets.map(preset => Object.freeze({...preset, directions: Object.freeze([...preset.directions])})));
const registry = new Map(presets.map(preset => [preset.id, preset]));
const strings = value => typeof value === 'string' ? [value] : value;

export function messageRequest(message, {context = [], policies = presets.map(preset => preset.id), customPolicies = [], model, deadlineMs = 15000} = {}) {
  context = strings(context); policies = strings(policies); customPolicies = strings(customPolicies);
  if (typeof message !== 'string' || [context, policies, customPolicies].some(items => !Array.isArray(items) || items.some(item => typeof item !== 'string'))) throw new TypeError('Expected message text and lists of strings');
  const rules = policies.map((category, index) => {
    const preset = registry.get(category);
    const detected = category === 'pii.email';
    const contextual = context.length || category === 'spam.repeat';
    return {
      id: `preset-${index + 1}`, category, directions: preset ? [...preset.directions] : ['outgoing', 'incoming'],
      match: detected ? 'detected' : 'score', action: 'warn',
      context_requirement: contextual && !detected ? 'supplied_window' : 'target_only',
      ...(!detected ? {review_threshold: 0.4, action_threshold: 0.7, model_thresholds: true, ...(contextual ? {min_context_messages: 1} : {})} : {}),
    };
  });
  rules.push(...customPolicies.map((text, index) => ({
    id: `custom-${index + 1}`, policy_text: text, directions: ['outgoing', 'incoming'], match: 'policy_text', action: 'warn',
    context_requirement: context.length ? 'supplied_window' : 'target_only', ...(context.length ? {min_context_messages: 1} : {}),
  })));
  return {
    api_version: '0.1', request_id: `check-${randomBytes(12).toString('hex')}`, direction: 'outgoing',
    options: {deadline_ms: deadlineMs, ...(model === undefined ? {} : {model})},
    message: {id: 'message', revision: '1', speaker: 'self', text: message},
    context: context.map((text, index) => ({id: `context-${index + 1}`, speaker: 'peer', text})),
    policy: {schema_version: '0.1', id: 'message-check', version: '1', profile: 'personal', rules,
      default_action: 'allow', on_error: 'review', on_indeterminate: 'review', override: 'user_confirm'},
  };
}
