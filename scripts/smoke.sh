#!/bin/bash
set -e
echo "Running smoke tests..."
python3 -m pytest tests/smoke/ -v
