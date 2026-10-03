"""Message-first request builder. JSON policies remain an advanced option."""
import json
import secrets
from pathlib import Path

CATALOGUE = json.loads(Path(__file__).with_name("presets.json").read_text(encoding="utf-8"))
PRESETS = {preset["id"]: preset for preset in CATALOGUE["presets"]}


def request(message, *, context=None, policies=None, custom_policies=None, model=None, deadline_ms=15000):
    if not isinstance(message, str):
        raise ValueError("message must be text")
    if context is None:
        context = []
    elif isinstance(context, str):
        context = [context]
    if not isinstance(context, (list, tuple)) or any(not isinstance(text, str) for text in context):
        raise ValueError("context must be text or a list of prior messages")
    if policies is None:
        policies = list(PRESETS)
    if isinstance(policies, str):
        policies = [policies]
    if custom_policies is None:
        custom_policies = []
    if isinstance(custom_policies, str):
        custom_policies = [custom_policies]
    if any(not isinstance(items, (list, tuple)) or any(not isinstance(item, str) for item in items)
           for items in (policies, custom_policies)):
        raise ValueError("policies must be text or a list of strings")
    rules = []
    for index, category in enumerate(policies):
        preset = PRESETS.get(category, {})
        detected = category == "pii.email"
        contextual = bool(context) or category == "spam.repeat"
        rule = {
            "id": f"preset-{index + 1}", "category": category,
            "directions": list(preset.get("directions", ["outgoing", "incoming"])),
            "match": "detected" if detected else "score",
            "context_requirement": "supplied_window" if contextual and not detected else "target_only",
            "action": "warn",
        }
        if not detected:
            rule.update(review_threshold=0.4, action_threshold=0.7, model_thresholds=True)
            if contextual:
                rule["min_context_messages"] = 1
        rules.append(rule)
    for index, text in enumerate(custom_policies):
        rule = {
            "id": f"custom-{index + 1}", "policy_text": text,
            "directions": ["outgoing", "incoming"], "match": "policy_text",
            "context_requirement": "supplied_window" if context else "target_only", "action": "warn",
        }
        if context:
            rule["min_context_messages"] = 1
        rules.append(rule)
    return {
        "api_version": "0.1", "request_id": "check-" + secrets.token_hex(12), "direction": "outgoing",
        "options": {"deadline_ms": deadline_ms, **({"model": model} if model is not None else {})},
        "message": {"id": "message", "revision": "1", "speaker": "self", "text": message},
        "context": [{"id": f"context-{index + 1}", "speaker": "peer", "text": text}
                    for index, text in enumerate(context)],
        "policy": {"schema_version": "0.1", "id": "message-check", "version": "1", "profile": "personal",
                   "rules": rules, "default_action": "allow", "on_error": "review",
                   "on_indeterminate": "review", "override": "user_confirm"},
    }
