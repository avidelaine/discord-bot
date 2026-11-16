#!/usr/bin/env bash
# Model 3 (2026-11-16): adds "Bobby" followed right away by the request (cont_clips, 0.08-0.4 s of the request kept
# after the word) and, if any, the real collected sentences whose transcript holds "Bobby" (x20 each), on top of
# model 2's positives (synthetic + French + the owner's 20 recordings). Then re-augments and retrains.
# Run in WSL:  bash "/mnt/c/Users/alexa/Desktop/Personal Projects/discord-bot/_training/inject2.sh"
set -e
P=~/oww/output/bobby/positive_train
W="/mnt/c/Users/alexa/Desktop/Personal Projects/discord-bot/_training"
rm -f "$P"/CONT_*.wav "$P"/COLL_*.wav
cp "$W"/cont_clips/*.wav "$P"/
if [ -d "$W/collected" ]; then
  # the voice service saved them at 16 kHz mono; the transcript (.txt) says whether "Bobby" was in it
  grep -rliE 'bob+(y|i|ie)\b|moby' "$W"/collected --include='*.txt' | while read -r t; do
    f="${t%.txt}.wav"; b=$(basename "$f" .wav)
    for k in $(seq 1 20); do cp "$f" "$P/COLL_${b}_${k}.wav"; done
  done
fi
echo "positive_train: $(ls "$P" | wc -l) clips ($(ls "$P" | grep -c '^CONT_') Bobby+request, $(ls "$P" | grep -c '^COLL_') real collected copies)"
cd ~/oww
mkdir -p "$W/models/v2"; cp -n output/bobby.onnx output/bobby.onnx.data "$W/models/v2/" 2>/dev/null || true  # keep model 2 (the one in use)
# the trainer skips training when output/bobby.onnx exists, and its export step writes under the default config's
# name (export/hey_echo.onnx): drop the old model first and copy the new one from output/ ourselves
rm -f output/bobby.onnx output/bobby.onnx.data
bash train.sh --from augment 2>&1 | tee "$W/train3.log"
test -f output/bobby.onnx || { echo "no model trained"; exit 1; }
mkdir -p "$W/models/v3"; cp output/bobby.onnx output/bobby.onnx.data "$W/models/v3/"
ls -la "$W/models/v3/"
