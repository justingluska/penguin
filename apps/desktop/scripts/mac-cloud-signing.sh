#!/usr/bin/env bash
# Sync with iCloud on the Mac (docs/CROSS-DEVICE-STATE.md) needs the app
# signed with iCloud (CloudKit) and push entitlements, which a Developer ID
# app may only carry with its provisioning profile embedded. This takes the
# profile (a Developer ID, "MAC_APP_DIRECT", profile for the app's bundle id)
# and writes, into an output directory:
#   embedded.provisionprofile   the profile, for bundle.macOS.files
#   Penguin.entitlements        entitlements read from the profile itself
#   tauri-macos.json            the Tauri config fragment that uses both
# so no team id, container or bundle id lives in the repo: they all come
# from the profile. A build without a profile signs as before and the app
# reports sync as unavailable.
#
#   mac-cloud-signing.sh <profile file> <bundle id> <output dir>
set -euo pipefail

profile="$1"
bundle_id="$2"
out="$3"
mkdir -p "$out"

plist="$out/profile.plist"
security cms -D -i "$profile" > "$plist"
pb() { /usr/libexec/PlistBuddy -c "Print :$1" "$plist" 2>/dev/null; }

app_id="$(pb Entitlements:com.apple.application-identifier)"
team="$(pb Entitlements:com.apple.developer.team-identifier)"
if [[ -z "$app_id" || -z "$team" ]]; then
  echo "::error::the provisioning profile has no application or team identifier"
  exit 1
fi
if [[ "$app_id" != "$team.$bundle_id" ]]; then
  echo "::error::the provisioning profile is for $app_id, not $team.$bundle_id"
  exit 1
fi
if [[ "$(pb ProvisionsAllDevices)" != "true" ]]; then
  echo "::error::not a Developer ID profile (ProvisionsAllDevices is not set)"
  exit 1
fi
expires="$(pb ExpirationDate)"
echo "profile for $bundle_id, expires $expires"

# Every iCloud container the profile allows (CKContainer.default() is the first).
containers=()
i=0
while c="$(pb "Entitlements:com.apple.developer.icloud-container-identifiers:$i")"; do
  containers+=("$c")
  i=$((i + 1))
done
if [[ ${#containers[@]} -eq 0 ]]; then
  echo "::error::the provisioning profile has no iCloud container; enable iCloud (CloudKit) on the App ID"
  exit 1
fi
if [[ -z "$(pb Entitlements:com.apple.developer.aps-environment)" ]]; then
  echo "::error::the provisioning profile has no push entitlement; enable Push Notifications on the App ID"
  exit 1
fi

ent="$out/Penguin.entitlements"
rm -f "$ent"
pbw() { /usr/libexec/PlistBuddy -c "$1" "$ent" >/dev/null; }
pbw "Add :com.apple.application-identifier string $app_id"
pbw "Add :com.apple.developer.team-identifier string $team"
pbw "Add :com.apple.developer.icloud-container-identifiers array"
for j in "${!containers[@]}"; do
  pbw "Add :com.apple.developer.icloud-container-identifiers:$j string ${containers[$j]}"
done
pbw "Add :com.apple.developer.icloud-services array"
pbw "Add :com.apple.developer.icloud-services:0 string CloudKit"
# Developer ID builds talk to the container's Production environment.
pbw "Add :com.apple.developer.icloud-container-environment string Production"
# CloudKit's silent pushes (CKSyncEngine subscribes by itself).
pbw "Add :com.apple.developer.aps-environment string production"
plutil -lint "$ent"

cp "$profile" "$out/embedded.provisionprofile"
cat > "$out/tauri-macos.json" <<EOF
{"bundle":{"macOS":{"entitlements":"$ent","files":{"embedded.provisionprofile":"$out/embedded.provisionprofile"}}}}
EOF
rm -f "$plist"
echo "entitlements: ${#containers[@]} container(s), team ${#team} characters"
