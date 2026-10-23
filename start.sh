#!/usr/bin/env sh
set -eu
cd "$(dirname "$0")"
export GT4_MAX_CONCURRENCY="${GT4_MAX_CONCURRENCY:-4}"
export GT4_ORT_THREADS="${GT4_ORT_THREADS:-2}"
if [ -x "./bili_ticket_gt_server" ]; then
  exec ./bili_ticket_gt_server "$@"
fi
exec cargo run --release -- "$@"
