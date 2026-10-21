# 金标准

`tests/golden.rs` 拿 kwscope 的数逐帧逐触发对这里的数。这些数都是 kws-train 自己的运行时
（`kws_infer`、`kws stream`）写的，不是 kwscope 写的。

| 文件 | 内容 |
| --- | --- |
| `thchs30_clip.wav` | [THCHS-30](https://www.openslr.org/18/)（OpenSLR-18，Apache-2.0，清华大学 CSLT）测试集 `D8_750`–`D8_754` 五句首尾相接，42.4 s，16 kHz 单声道 PCM16 |
| `bundles/stage_one`、`bundles/stage_two` | `random_bundles.py` 写的 bundle：kws-train 一级（heimdal BC-ResNet，40 维 log-mel，宽 3）与二级（4 层 conformer checker）的结构，权重是带种子的随机初始化，没训练过 |
| `triggers_t1_0.553.jsonl`、`triggers_t1_0.603.jsonl` | `kws stream` 在 t1 = 0.553 与 0.603、t2 = 0.455、det.v8 下的输出 |
| `stage_one_feats.npy`、`stage_one_logits.npy` | 按 100 ms 推入时一级每帧的特征与网络原始输出（`make_fixtures.py`） |
| `stage_two_feats_<i>.npy`、`stage_two_spans.json` | 前两个触发、每个前导一段的二级特征、样本区间与成员 logit |

bundle 没学过任何语料，触发本身没有意义：这里只钉 kwscope 和 `kws_infer` 算得一样。
随机初始化的 BC-ResNet 输出在 1e-5 量级、分数全是 0.5，所以它的 BatchNorm 统计量在这段录音上重算过一遍。
阈值都挑在分数的空档里（t1 离最近一帧的分数 > 5e-5，t2 离最近的二级 logit > 2e-3），
两边差在容差之内时判法不会翻；t2 让两份触发里都有收有拒。

## 重做

在 kws-train 的工作区里，`F` 指向本目录（先放好 clip）：

```bash
uv run --frozen --no-sync python $F/random_bundles.py $F
P=$PWD/conf/catalogs/eval_protocols/det.v8.yaml
for t in 0.553 0.603; do
  (cd $F && uv run --project $OLDPWD --frozen --no-sync kws stream thchs30_clip.wav \
    --bundle bundles/stage_one --threshold $t --protocol $P \
    --stage-two bundles/stage_two --t2 0.455 > triggers_t1_$t.jsonl)
done
uv run --frozen --no-sync python $F/make_fixtures.py $F
```

`random_bundles.py` 在同一个 kws-train 提交上逐字节重现这两个 bundle。换了 bundle 就整套重做：
触发、特征和 logit 都绑在这两个 bundle 上，阈值也要重新挑空档。

## 容差

特征在 1e-4 内（两边都是 f64 算、转 f32，只差在求和次序），网络输出在 1e-3 内（同一张图，
不同的 ORT 构建）。触发的整数与布尔字段必须逐字相同，`score` 在 1e-5 内。
