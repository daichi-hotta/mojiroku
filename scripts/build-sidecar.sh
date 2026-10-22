#!/usr/bin/env bash
# mojiroku-llm（ローカル要約 sidecar, ADR-0007）をビルドし、
# Tauri externalBin が要求する triple 付き名で src-tauri/binaries/ に配置する。
# あわせて mojiroku-mcp（ローカル MCP サーバ, ADR-0010）もビルドする。
# `tauri dev` / `tauri build` の前に実行すること（justfile の dev/build から呼ばれる）。
set -euo pipefail

cd "$(dirname "$0")/.."
TRIPLE="$(rustc -vV | sed -n 's/host: //p')"

cargo build --release -p mojiroku-llm
mkdir -p src-tauri/binaries
cp "target/release/mojiroku-llm" "src-tauri/binaries/mojiroku-llm-${TRIPLE}"
chmod +x "src-tauri/binaries/mojiroku-llm-${TRIPLE}"
echo "placed: src-tauri/binaries/mojiroku-llm-${TRIPLE}"

# mojiroku-mcp（ローカル MCP サーバ）も externalBin として .app に同梱する。
#
# アプリ自身は spawn しない（ADR-0010。spawn するのは Claude Desktop / Claude Code）。
# それでも externalBin に置くのは、**署名と公証を llm sidecar と同じ経路に乗せるため**。
# bundle resources に置く案もあるが、Mach-O が hardened runtime で署名される保証が
# 無く、公証で落ちるかどうかを CI で試すまで分からない。externalBin は v0.4.0 から
# 実績がある（release.yml が Contents/MacOS/ の各バイナリの runtime フラグを検証する）。
#
# アプリは spawn しない。webview には shell の許可を一切与えておらず（capabilities/default.json）、
# mojiroku-llm を起動するのは Rust 側のコードだけなので、externalBin に足しても起動経路は増えない。
#
# 利用者は /Applications/mojiroku.app/Contents/MacOS/mojiroku-mcp を MCP 設定の
# command に指定する（docs/mcp.md）。ビルド環境が無くても使える。
cargo build --release -p mojiroku-mcp
cp "target/release/mojiroku-mcp" "src-tauri/binaries/mojiroku-mcp-${TRIPLE}"
chmod +x "src-tauri/binaries/mojiroku-mcp-${TRIPLE}"
echo "placed: src-tauri/binaries/mojiroku-mcp-${TRIPLE}"
