"""Test local Gandalf scoring and optional ONNX export; does not install a backend.

Build-time dependencies: torch, transformers, safetensors. ONNX export/parity
additionally needs onnx and onnxruntime. No files are downloaded or executed from
the model directory. Reports contain synthetic probe results, not user messages.
"""
import argparse
import hashlib
import json
from pathlib import Path
import platform
import time

ROOT = Path(__file__).resolve().parents[1]
GANDALF_WEIGHTS = "916370da013a428a8ed3e00791049d12b3b3fd5836969d8b0492bc5de372d5c8"


def digest(path):
    with path.open("rb") as source:
        return hashlib.file_digest(source, "sha256").hexdigest()


def head_tail(ids, budget, tail=64):
    if budget < 1:
        raise ValueError("policy fills the model input")
    if len(ids) <= budget:
        return ids
    tail = min(tail, budget // 2)
    return ids[:budget - tail] + (ids[-tail:] if tail else [])


def pair_tokens(tokenizer, first, second):
    # Transformers 5 removed build_inputs_with_special_tokens from its fast
    # tokenizers. Read the tokenizer's pair template using two probe encodings,
    # then replace each non-special run with the actual sequence's token IDs.
    backend = tokenizer.backend_tokenizer
    probes = [backend.encode("a", add_special_tokens=False) for _ in range(2)]
    layout = backend.post_process(*probes, add_special_tokens=True)
    sequences = iter([first, second])
    ids = []
    in_sequence = False
    for token, special in zip(layout.ids, layout.special_tokens_mask):
        if special:
            ids.append(token)
        elif not in_sequence:
            ids.extend(next(sequences))
        in_sequence = not special
    return ids


def encode(tokenizer, policies, message, context=None):
    context = (context or "").strip()
    evidence = f"Message:\n{message}\n\nContext:\n{context}" if context else message
    evidence_ids = tokenizer(evidence, add_special_tokens=False)["input_ids"]
    features = []
    for policy in policies:
        policy_ids = tokenizer(policy, add_special_tokens=False)["input_ids"]
        budget = 512 - tokenizer.num_special_tokens_to_add(pair=True) - len(policy_ids)
        ids = pair_tokens(tokenizer, policy_ids, head_tail(evidence_ids, budget))
        features.append({"input_ids": ids, "attention_mask": [1] * len(ids)})
    return tokenizer.pad(features, padding=True, return_tensors="pt")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--model-path", type=Path, required=True)
    parser.add_argument("--weights-sha256", default=GANDALF_WEIGHTS)
    parser.add_argument("--catalogue", type=Path, default=ROOT / "src/runtime/presets.json")
    parser.add_argument("--report", type=Path, required=True)
    parser.add_argument("--onnx-output", type=Path)
    parser.add_argument("--threads", type=int, default=2)
    args = parser.parse_args()
    if args.threads < 1:
        parser.error("threads must be positive")
    if args.onnx_output and args.onnx_output.exists():
        parser.error("refusing to overwrite an ONNX export")
    weights_hash = digest(args.model_path / "model.safetensors")
    if weights_hash != args.weights_sha256:
        parser.error("model weights do not match the expected SHA-256")
    config = json.loads((args.model_path / "config.json").read_text(encoding="utf-8"))
    if config.get("model_type") != "deberta-v2" or len(config.get("id2label", {})) != 1:
        parser.error("this probe requires Gandalf's single-logit DeBERTa architecture")

    import torch
    import transformers

    torch.set_num_threads(args.threads)
    started = time.perf_counter()
    tokenizer = transformers.AutoTokenizer.from_pretrained(
        args.model_path, local_files_only=True, trust_remote_code=False)
    model = transformers.AutoModelForSequenceClassification.from_pretrained(
        args.model_path, local_files_only=True, trust_remote_code=False,
        use_safetensors=True).cpu().eval()
    load_seconds = time.perf_counter() - started
    presets = json.loads(args.catalogue.read_text(encoding="utf-8"))["presets"]
    policies = [entry["wording"] for entry in presets] + ["Keep launch dates private."]
    encoded = encode(tokenizer, policies, "Thank you for your help.", "Can you help me with this?")

    def forward(inputs):
        with torch.inference_mode():
            return torch.cat([model(**{key: value[i:i + 8] for key, value in inputs.items()}).logits
                              for i in range(0, len(inputs["input_ids"]), 8)], dim=0)

    timings = []
    for _ in range(3):
        started = time.perf_counter()
        logits = forward(encoded)
        timings.append(time.perf_counter() - started)
    scores = logits.sigmoid().flatten().tolist()
    if len(scores) != len(policies) or not all(0 <= score <= 1 for score in scores):
        raise ValueError("invalid policy scores")
    report = {
        "platform": platform.platform(), "weights_sha256": weights_hash,
        "torch": torch.__version__, "transformers": transformers.__version__,
        "threads": args.threads, "device": "cpu", "load_seconds": load_seconds,
        "policy_count": len(policies), "sequence_length": encoded["input_ids"].shape[1],
        "batch_size": 8, "scoring_seconds": timings, "finite_scores": len(scores),
        "preprocessing": {"max_tokens": 512, "tail_tokens": 64, "evidence_format": "target-first"},
        "quality_evaluation": False,
        "reference_probabilities": dict(zip(policies, scores)),
    }
    if args.onnx_output:
        import numpy as np
        import onnxruntime as ort

        class Logits(torch.nn.Module):
            def __init__(self, source):
                super().__init__()
                self.source = source

            def forward(self, input_ids, attention_mask):
                return self.source(input_ids=input_ids, attention_mask=attention_mask).logits

        long_input = encode(tokenizer, policies[:2], "Earlier evidence. " * 600 + "Final evidence.")
        torch.onnx.export(
            Logits(model).eval(), (long_input["input_ids"], long_input["attention_mask"]),
            str(args.onnx_output), input_names=["input_ids", "attention_mask"],
            output_names=["logits"], opset_version=18, dynamo=False,
            dynamic_axes={"input_ids": {0: "batch", 1: "sequence"},
                          "attention_mask": {0: "batch", 1: "sequence"},
                          "logits": {0: "batch"}})
        options = ort.SessionOptions()
        options.intra_op_num_threads = args.threads
        options.inter_op_num_threads = 1
        started = time.perf_counter()
        session = ort.InferenceSession(str(args.onnx_output), sess_options=options,
                                       providers=["CPUExecutionProvider"])
        onnx_load = time.perf_counter() - started

        def run(inputs):
            return np.concatenate([
                session.run(["logits"], {key: value[i:i + 8].numpy() for key, value in inputs.items()})[0]
                for i in range(0, len(inputs["input_ids"]), 8)])

        onnx_timings = []
        for _ in range(3):
            started = time.perf_counter()
            actual = run(encoded)
            onnx_timings.append(time.perf_counter() - started)
        cases = [(encoded, logits.numpy())]
        for count in [1, 2, 8]:
            cases.append(({key: value[:count] for key, value in encoded.items()}, logits[:count].numpy()))
        cases.append((long_input, forward(long_input).numpy()))
        parity_errors = []
        for inputs, reference in cases:
            actual = run(inputs)
            if not np.allclose(actual, reference, rtol=1e-4, atol=1e-5):
                error = float(np.max(np.abs(actual - reference)))
                raise ValueError(f"ONNX export differs from the original model: shape={tuple(inputs['input_ids'].shape)}, max_logit_error={error}")
            parity_errors.append(float(np.max(np.abs(actual - reference))))
        model_policies = [entry["wording"] for entry in presets if entry["tier"] == "model"]
        full_length = encode(tokenizer, model_policies, "Earlier evidence. " * 600 + "Final evidence.")
        full_length_timings = []
        for _ in range(3):
            started = time.perf_counter()
            full_length_scores = run(full_length)
            full_length_timings.append(time.perf_counter() - started)
        if full_length_scores.shape != (len(model_policies), 1) or not np.isfinite(full_length_scores).all():
            raise ValueError("invalid full-length policy scores")
        report["onnx"] = {
            "onnxruntime": ort.__version__, "opset": 18,
            "bytes": args.onnx_output.stat().st_size, "sha256": digest(args.onnx_output),
            "load_seconds": onnx_load, "scoring_seconds": onnx_timings,
            "max_absolute_logit_errors": parity_errors,
            "dynamic_batches_tested": [1, 2, 8, len(policies)],
            "sequence_lengths_tested": [encoded["input_ids"].shape[1], long_input["input_ids"].shape[1]],
            "full_length_model_batch": {
                "policy_count": len(model_policies), "sequence_length": full_length["input_ids"].shape[1],
                "scoring_seconds": full_length_timings,
            },
            "passed": True,
        }
    args.report.parent.mkdir(parents=True, exist_ok=True)
    args.report.write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps(report))


if __name__ == "__main__":
    main()
