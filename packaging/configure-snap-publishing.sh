#!/bin/bash
# Run locally once to authorize GitHub Actions to publish Lahza.
set -euo pipefail
umask 077

command -v snapcraft >/dev/null
command -v gh >/dev/null
gh auth status

credential_dir="$(mktemp -d)"
trap 'rm -f "$credential_dir/store-login"; rmdir "$credential_dir"' EXIT
expires="$(date -u -d '+365 days' '+%Y-%m-%dT%H:%M:%SZ')"

snapcraft export-login \
  --snaps=lahza \
  --channels=stable,beta \
  --acls=package_access,package_push,package_update,package_release \
  --expires="$expires" \
  "$credential_dir/store-login"

gh secret set SNAPCRAFT_STORE_CREDENTIALS \
  --repo FarhanAliRaza/lahza < "$credential_dir/store-login"
echo "Snap publishing configured for FarhanAliRaza/lahza until $expires."
echo 'Run this script again before that date to renew the credential.'
