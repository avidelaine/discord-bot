#!/usr/bin/env bash
# Add the French-accented clips and the user's own "Bobby" (100 copies each) to the positive training set,
# then make model 2 less conservative and a bit larger.
set -e
P=~/oww/output/bobby/positive_train
W="/mnt/c/Users/alexa/Desktop/Personal Projects/discord-bot/_training"
cp "$W"/fr_clips/*.wav "$P"/
for f in "$W"/real_clips/*.wav; do
  b=$(basename "$f" .wav)
  for k in $(seq 1 100); do cp "$f" "$P/REAL_${b}_${k}.wav"; done
done
echo "positive_train: $(ls "$P" | wc -l) clips ($(ls "$P" | grep -c '^FR_') French, $(ls "$P" | grep -c '^REAL_') real)"
cd ~/oww
sed -i -e 's/^max_negative_weight: 1500/max_negative_weight: 500/' -e 's/^layer_size: 32/layer_size: 64/' configs/bobby.yaml
grep -E '^(max_negative_weight|layer_size|steps)' configs/bobby.yaml
