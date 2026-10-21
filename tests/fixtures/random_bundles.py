"""The two test bundles: kws-train's architectures with untrained weights.

Stage one is the heimdal BC-ResNet on 40 log-mel bands at width 3, stage two
the 4-layer conformer checker, as in kws-train's baselines, but the weights
are a seeded random init, so nothing learned from any corpus ships here.
Random init alone leaves the BC-ResNet's logits near 1e-5, every score at
0.5, so its BatchNorm statistics are recomputed on ``thchs30_clip.wav``.
Run from a kws-train checkout:

    uv run --frozen --no-sync python random_bundles.py <fixtures dir>
"""

from __future__ import annotations

import json
import shutil
import sys
import tempfile
from pathlib import Path

import soundfile as sf
import torch

from kws_plugins.common.frontend import attach_frontend
from kws_plugins.family.checker.export import export_runs
from kws_plugins.family.checker.family import CheckerFrontendConfig, CheckerModelConfig
from kws_plugins.family.checker.model import build_checker
from kws_plugins.family.heimdal.export import export_run
from kws_plugins.family.heimdal.family import (
    HeimdalFrontendConfig,
    HeimdalModelConfig,
    feature_dim,
)
from kws_plugins.family.heimdal.model import build_heimdal_model
from kws_train.config.run import read_run

KEYWORDS = ["NIHAO_KAKA"]
STAGE_ONE = {
    "family": "heimdal",
    "seed": 1,
    "method": {
        "model": {"keywords": KEYWORDS, "n_keywords": 1, "sub_bands": 10, "width": 3},
        "frontend": {
            "c0_norm_s": 2.0,
            "cepstral": False,
            "f_min": 20.0,
            "hop_length": 160,
            "log_offset": 1e-06,
            "n_fft": 512,
            "n_mels": 40,
            "n_mfcc": 40,
            "sample_rate": 16000,
            "spec_aug": False,
            "win_length": 400,
        },
    },
}
STAGE_TWO = {
    "family": "checker",
    "seed": 2,
    "method": {
        "model": {
            "conv_kernel": 15,
            "dim": 144,
            "dropout": 0.1,
            "ff_dim": 576,
            "heads": 4,
            "keywords": KEYWORDS,
            "kind": "conformer",
            "layers": 4,
            "pooling": "max",
            "subsampling": 4,
        },
        "frontend": {
            "cmn": True,
            "f_min": 20.0,
            "hop_length": 160,
            "log_offset": 1e-06,
            "n_fft": 512,
            "n_mels": 80,
            "sample_rate": 16000,
            "win_length": 400,
        },
        "crop": {"extra_leads_ms": [1700.0], "lead_ms": 1350.0, "start_jitter_ms": 150.0},
    },
}


def run_dir(stage: dict, root: Path) -> Path:
    """A run directory with only the spec sections export reads."""
    root.mkdir(parents=True)
    recipe = {"family": stage["family"], "name": "random-init", "method": stage["method"]}
    (root / "resolved.json").write_text(json.dumps({"recipe": recipe}))
    (root / "corpora.json").write_text(json.dumps({"corpora": []}))
    return root


@torch.no_grad()
def recalibrate(model: torch.nn.Module, clip: Path) -> None:
    audio, _ = sf.read(clip, dtype="float32")
    wave = torch.from_numpy(audio)[None]
    for m in model.modules():
        if isinstance(m, torch.nn.modules.batchnorm._BatchNorm):
            m.reset_running_stats()
            m.momentum = None
    model.train()
    model(wave, n_samples=torch.tensor([wave.shape[1]]))
    model.eval()


def scrub(path: Path, seed: int) -> None:
    """The json says how the weights came about, and names no run or corpus."""
    meta = json.loads(path.read_text())
    source = {"weights": f"random init, torch.manual_seed({seed})"}
    if "members" in meta:
        for m in meta["members"]:
            m["source"] = {"crop": m["source"]["crop"], **source}
    else:
        meta["source"] = source
    path.write_text(json.dumps(meta, indent=2, sort_keys=True) + "\n")


def main() -> None:
    fixtures = Path(sys.argv[1])
    out = fixtures / "bundles"
    shutil.rmtree(out, ignore_errors=True)
    with tempfile.TemporaryDirectory() as tmp:
        one = run_dir(STAGE_ONE, Path(tmp) / "stage_one")
        run = read_run(one)
        frontend = run.section("recipe.method.frontend", HeimdalFrontendConfig)
        config = run.section("recipe.method.model", HeimdalModelConfig)
        torch.manual_seed(STAGE_ONE["seed"])
        model = attach_frontend(
            build_heimdal_model(config, feature_dim(frontend)), frontend, spec_aug=False
        )
        recalibrate(model, fixtures / "thchs30_clip.wav")
        torch.save(model.state_dict(), one / "ema.pt")
        export_run(one, out / "stage_one")
        scrub(out / "stage_one" / "stage_one.json", STAGE_ONE["seed"])

        two = run_dir(STAGE_TWO, Path(tmp) / "stage_two")
        run = read_run(two)
        config = run.section("recipe.method.model", CheckerModelConfig)
        frontend = run.section("recipe.method.frontend", CheckerFrontendConfig)
        torch.manual_seed(STAGE_TWO["seed"])
        checker = build_checker(config, frontend, len(config.keywords))
        torch.save(checker.state_dict(), two / "ema.pt")
        export_runs([two], out / "stage_two")
        scrub(out / "stage_two" / "stage_two.json", STAGE_TWO["seed"])


if __name__ == "__main__":
    main()
