#!/usr/bin/env bash
# Tauri v2 updater マニフェスト latest.json を生成して stdout に出力する。
# リリース CI（.github/workflows/release.yml）から呼ばれ、生成物は公開 mojiroku-releases の
# Release アセットとして添付する。配信は mojiroku.com Worker の /updater/latest.json プロキシ経由。
#
# 使い方:
#   scripts/make-updater-manifest.sh <VERSION> <SIG_PATH> [NOTES] [SHA] > latest.json
#
#   VERSION   : 例 0.3.0（src-tauri/tauri.conf.json の version と一致させる）
#   SIG_PATH  : tauri build が出力する mojiroku-macos-aarch64.app.tar.gz.sig へのパス。
#               中身は「単一行 base64」（= latest.json の signature にそのまま入る値）。
#   NOTES     : リリースノート（任意・既定空）
#   SHA       : ソースコミット SHA（任意・notes 末尾に付ける運用は呼び出し側で）
#
# 重要:
# - signature は .sig の中身を verbatim で入れる。$(cat) で読むと末尾改行だけ除去され、
#   単一行 base64 はそのまま保たれる（jq --arg が安全に JSON 文字列化する）。
# - url は「版非依存の安定名 + latest 解決」。版を上げてもリンクが壊れない。
#   latest は非 draft・非 prerelease の公開リリースにしか解決しないので、CI は draft→検証→
#   publish(flip) の順で原子的に切替えること。
set -euo pipefail

VERSION="${1:?VERSION required}"
SIG_PATH="${2:?SIG_PATH required}"
NOTES="${3:-}"
# SHA は将来用（現状 notes は呼び出し側で組み立てるためここでは未使用）
SHA="${4:-}"

if [ ! -f "$SIG_PATH" ]; then
  echo "make-updater-manifest: signature file not found: $SIG_PATH" >&2
  exit 1
fi

SIG="$(cat "$SIG_PATH")"            # 単一行 base64。$() が末尾改行を除去
PUB_DATE="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
URL="https://github.com/daichi-hotta/mojiroku-releases/releases/latest/download/mojiroku-macos-aarch64.app.tar.gz"

jq -n \
  --arg v "$VERSION" \
  --arg sig "$SIG" \
  --arg notes "$NOTES" \
  --arg date "$PUB_DATE" \
  --arg url "$URL" \
  '{version: $v, notes: $notes, pub_date: $date, platforms: {"darwin-aarch64": {signature: $sig, url: $url}}}'
