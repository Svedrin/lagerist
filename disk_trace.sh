#!/bin/bash

set -e
set -u

BASE=/sys/kernel/debug/tracing/instances
INST="$BASE/lagerist"

cleanup() {
    echo 0 > "$INST/tracing_on"
    rmdir "$INST"
}

trap cleanup exit

mkdir -p "$INST"
echo 1 > "$INST/events/block/block_rq_issue/enable"
echo 1 > "$INST/events/block/block_rq_insert/enable"
echo 1 > "$INST/events/block/block_rq_complete/enable"
echo 1 > "$INST/tracing_on"

cat "$INST/trace_pipe"
