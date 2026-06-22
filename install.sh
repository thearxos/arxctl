#!/usr/bin/env bash
D=$(cd "$(dirname "$0")" && pwd); sudo install -m755 "$D/arxctl" /usr/local/bin/arxctl
