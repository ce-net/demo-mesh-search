#!/bin/sh
# Bundle web/app.ts (+ the generated client) into web/app.js. Needs only npx.
set -e
cd "$(dirname "$0")/../web"
npx -y esbuild@0.24.0 app.ts --bundle --format=esm --target=es2020 --outfile=app.js
