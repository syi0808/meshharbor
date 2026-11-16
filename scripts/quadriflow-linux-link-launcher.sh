#!/usr/bin/env bash
# GNU compiler launcher for the unsigned diagnostic build's sole final executable.
set -euo pipefail
trace=$1
shift
args=("$@")
output=""
for ((i = 1; i + 1 < ${#args[@]}; i++)); do
  if [[ ${args[i]} = -o ]]; then
    [[ -z "$output" ]]
    output=${args[i+1]}
  fi
done
if [[ $output != quadriflow && $output != ./quadriflow ]]; then
  exec "${args[@]}"
fi

# The caller owns a newly made private scratch directory. Never overwrite a
# trace: a second final link would no longer identify one observed invocation.
umask 077
set -o noclobber
"${args[@]}" -Wl,-t,-t > "$trace"
test -s "$trace"
