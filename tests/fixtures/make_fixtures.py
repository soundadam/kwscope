"""Golden fixtures for kwscope, written by kws-train's own runtime (kws_infer).

Run from a kws-train checkout, with the two bundles and the clip beside this
script:

    uv run --frozen --no-sync python make_fixtures.py <fixtures dir>

It writes, per stage-one frame of ``thchs30_clip.wav`` streamed in 100 ms pushes
the way ``kws stream`` does, the features the network reads
(``stage_one_feats.npy``, float32 (T, dim)) and the network's raw outputs
(``stage_one_logits.npy``, float32 (T, 2K)), and, for one span per lead of the
first triggers, the stage-two features (``stage_two_feats_<i>.npy``) with the
member logits they give. The trigger lists themselves come from ``kws
stream`` (see README.md in this directory).
"""

from __future__ import annotations

import json
import sys
from pathlib import Path

import numpy as np
import soundfile as sf

from kws_infer.bundle import load_bundle, load_stage_two_bundle
from kws_infer.stage_one import StageOne
from kws_infer.detector import DetectorConfig
from kws_infer.stage_two import StageTwo, span

root = Path(sys.argv[1])
audio, rate = sf.read(root / "thchs30_clip.wav", dtype="float32")
bundle = load_bundle(root / "bundles" / "stage_one")
engine = StageOne(bundle, DetectorConfig(threshold=2.0, window=50, lockout=80, warmup=150))
step = round(0.1 * rate)
feats, logits = [], []
for at in range(0, len(audio), step):
    f = engine.frontend.push(audio[at : at + step])
    feats.append(f)
    if len(f):
        out, engine.cache = engine.session.run(
            None, {"feats": f[None].astype(np.float32), "cache_in": engine.cache}
        )
        logits.append(out[0])
np.save(root / "stage_one_feats.npy", np.concatenate(feats).astype(np.float32))
np.save(root / "stage_one_logits.npy", np.concatenate(logits).astype(np.float32))

checker = StageTwo(root / "bundles" / "stage_two", 0.455)
member = checker.meta.members[0]
arrays = checker.bundle.frontends[0]
spans = []
with open(root / "triggers_t1_0.553.jsonl") as fh:
    triggers = [json.loads(line) for line in fh][:2]
i = 0
for trig in triggers:
    for lead in member.leads:
        lo, hi = span(trig["start_sample"], trig["end_sample"], member, lead)
        f = StageTwo.features(member, arrays, audio[lo:hi])
        out = checker.sessions[0].run(["logits"], {"feats": f[None]})[0]
        np.save(root / f"stage_two_feats_{i}.npy", f)
        spans.append({"index": i, "lo": lo, "hi": hi, "logits": out[0].tolist()})
        i += 1
(root / "stage_two_spans.json").write_text(json.dumps(spans, indent=2) + "\n")
