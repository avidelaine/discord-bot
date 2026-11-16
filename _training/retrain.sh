#!/usr/bin/env bash
# Self-training, step 2 (2026-11-16): retrains the wake word model on everything collected so far.
#   1. voice/.venv/Scripts/python _training/label_collected.py   (Windows, GPU free: Whisper labels the near-misses)
#   2. wsl -d Ubuntu -- bash "/mnt/c/Users/alexa/Desktop/Personal Projects/discord-bot/_training/retrain.sh"
#   3. voice/.venv/Scripts/python _training/bench.py models/wakeword/bobby.onnx _training/models/latest/bobby.onnx
# Positives: model 2's set + the "Bobby, <request>" clips + every collected clip whose transcript holds "Bobby"
# (x20: passed the gate, or missed and found by Whisper). Negatives: collected clips without "Bobby" (real hard ones).
set -e
P=~/oww/output/bobby/positive_train
N=~/oww/output/bobby/negative_train
W="/mnt/c/Users/alexa/Desktop/Personal Projects/discord-bot/_training"
rm -f "$P"/CONT_*.wav "$P"/COLL_*.wav "$N"/COLL_*.wav
cp "$W"/cont_clips/*.wav "$P"/
pos=0; neg=0
if [ -d "$W/collected" ]; then
  while IFS= read -r -d '' t; do
    f="${t%.txt}.wav"; b=$(basename "$f" .wav)
    [ -f "$f" ] || continue
    if grep -qiE 'bob+(y|i|ie)\b|moby' "$t"; then
      for k in $(seq 1 20); do cp "$f" "$P/COLL_${b}_${k}.wav"; done; pos=$((pos + 1))
    else
      cp "$f" "$N/COLL_${b}.wav"; neg=$((neg + 1))
    fi
  done < <(find "$W/collected" -name '*.txt' -print0)
fi
echo "collected: $pos with Bobby (x20 in positives), $neg without (hard negatives)"
cd ~/oww
bash train.sh --from augment 2>&1 | tee "$W/retrain.log"
stamp=$(date +%Y%m%d-%H%M)
mkdir -p "$W/models/$stamp" "$W/models/latest"
cp export/bobby.onnx* "$W/models/$stamp/"; cp export/bobby.onnx* "$W/models/latest/"
echo "new model: $W/models/$stamp (also models/latest); compare with bench.py before using it"
