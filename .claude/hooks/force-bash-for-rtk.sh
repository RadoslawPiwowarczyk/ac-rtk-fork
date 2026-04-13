#!/usr/bin/env bash
INPUT=$(cat)
TOOL=$(echo "$INPUT" | jq -r '.tool_name // empty')
case "$TOOL" in
  Read)
    FILE=$(echo "$INPUT" | jq -r '.tool_input.file_path // empty')
    if [ -f "$FILE" ]; then
      SIZE=$(wc -c < "$FILE")
      # Files over 50KB — let built-in Read handle with smart chunking
      if [ "$SIZE" -gt 50000 ]; then
        exit 0
      fi
    fi
    echo "Use cat $FILE via Bash instead of Read — RTK compresses the output." >&2
    exit 2
    ;;
  Grep)
    echo "Use grep -rn via Bash instead of Grep — RTK compresses the output." >&2
    exit 2
    ;;
  Glob)
    echo "Use find or ls via Bash instead of Glob — RTK compresses the output." >&2
    exit 2
    ;;
  *)
    exit 0
    ;;
esac