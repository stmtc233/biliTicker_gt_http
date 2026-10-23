@echo off
setlocal
cd /d "%~dp0"
if not defined GT4_MAX_CONCURRENCY set GT4_MAX_CONCURRENCY=4
if not defined GT4_ORT_THREADS set GT4_ORT_THREADS=2
if exist bili_ticket_gt_server.exe (
  bili_ticket_gt_server.exe %*
) else (
  cargo run --release -- %*
)
