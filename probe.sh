echo "  bash version : $BASH_VERSION"
if set -o pipefail 2>/dev/null; then echo "  pipefail     : yes"; else echo "  pipefail     : NO"; fi
echo "  uname        : $(uname -s)"
if cd /c 2>/dev/null; then echo "  /c mount     : yes ($(pwd))"; else echo "  /c mount     : no"; fi
echo ""
echo "  coreutils the corpus needs (18 tools, 113 uses):"
missing=0
for t in sed tar tr grep sort find cut printf wc awk test du chown ln df yes date curl xargs; do
  if command -v "$t" >/dev/null 2>&1; then
    printf "    ok      %s\n" "$t"
  else
    printf "    MISSING %s\n" "$t"
    missing=$((missing+1))
  fi
done
echo ""
echo "  missing count: $missing"
