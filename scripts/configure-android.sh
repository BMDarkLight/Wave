#!/usr/bin/env bash
# Turn the Gradle project from `tauri android init` into Wave's Android app.
#
# `tauri android init` only generates a stock Tauri project. Wave adds to it:
#   - its Java sources (ExoPlayer, media-session bridge, SAF, crash reporter,
#     updater)
#   - its resources (launcher icons, the status bar / notification icon,
#     the network security config and its debug-build override)
#   - manifest permissions, the portrait lock, the early crash provider and
#     the provider that shares downloaded updates with the installer
#   - the Media3 / ExoPlayer Gradle dependencies
#   - ProGuard keep rules for everything native code calls over JNI
#
# Without this an APK builds but has no audio engine, so run it after every
# `tauri android init`, locally and in CI (.github/workflows/android.yml).
# Safe to run more than once.
set -euo pipefail

cd "$(dirname "$0")/.."

APP_DIR="src-tauri/gen/android/app"
JAVA_SRC="src-tauri/android-src/java"
JAVA_DEST="$APP_DIR/src/main/java"
RES_SRC="src-tauri/icons/android"
RES_DEST="$APP_DIR/src/main/res"
DEBUG_RES_DEST="$APP_DIR/src/debug/res"
MANIFEST="$APP_DIR/src/main/AndroidManifest.xml"
PROGUARD_SRC="src-tauri/android-src/proguard-wave.pro"

if [[ ! -d "$JAVA_DEST" ]]; then
  echo "::error::Missing $JAVA_DEST (did tauri android init fail?)"
  exit 1
fi
if [[ ! -d "$JAVA_SRC" ]]; then
  echo "::error::Missing $JAVA_SRC"
  exit 1
fi
if [[ ! -f "$MANIFEST" ]]; then
  echo "::error::Missing $MANIFEST"
  exit 1
fi
if [[ ! -d "$RES_SRC" ]]; then
  echo "::error::Missing $RES_SRC: run \`npm run icons\` first"
  exit 1
fi

# JNI / SAF Java sources
cp -R "$JAVA_SRC"/. "$JAVA_DEST"/
echo "Synced Android Java sources from $JAVA_SRC"

# Strip UTF-8 BOMs (javac rejects \ufeff before `package`).
python3 - <<'PY'
from pathlib import Path
root = Path("src-tauri/gen/android/app/src/main/java")
bom = b"\xef\xbb\xbf"
for path in root.rglob("*.java"):
    data = path.read_bytes()
    if data.startswith(bom):
        path.write_bytes(data[len(bom):])
        print(f"Stripped UTF-8 BOM: {path}")
PY

# ProGuard keep rules (JNI entry points)
if [[ -f "$PROGUARD_SRC" ]]; then
  cp "$PROGUARD_SRC" "$APP_DIR/proguard-wave.pro"
  for candidate in "$APP_DIR/proguard-rules.pro" "$APP_DIR/proguard-rules.txt"; do
    if [[ -f "$candidate" ]] && ! grep -q "proguard-wave.pro" "$candidate"; then
      printf '\n# Wave JNI keep rules\n-include proguard-wave.pro\n' >> "$candidate"
      echo "Hooked Wave keep rules into $candidate"
    fi
  done
fi

# Launcher icons
cp -R "$RES_SRC"/. "$RES_DEST"/
echo "Applied Wave launcher icons into $RES_DEST"

# App resources (status bar / media notification icon, network security config)
# Without drawable/ic_notification the media notification falls
# back to the template's ic_launcher_foreground, the Android robot.
# xml/wave_network_security.xml is referenced from <application> below.
cp -R src-tauri/android-src/res/. "$RES_DEST"/
echo "Applied Wave Android resources into $RES_DEST"

# Debug-only resources. The debug source set overrides main, so debug builds
# get a network security config that allows the dev server's plain HTTP.
mkdir -p "$DEBUG_RES_DEST"
cp -R src-tauri/android-src/res-debug/. "$DEBUG_RES_DEST"/
echo "Applied Wave debug resources into $DEBUG_RES_DEST"

# Manifest: permissions + portrait lock
python3 - "$MANIFEST" <<'PY'
from pathlib import Path
import re
import sys

path = Path(sys.argv[1])
text = path.read_text(encoding="utf-8")

permissions = [
    "android.permission.INTERNET",
    # Lets remote music tell mobile data from Wi-Fi.
    "android.permission.ACCESS_NETWORK_STATE",
    "android.permission.WAKE_LOCK",
    "android.permission.FOREGROUND_SERVICE",
    "android.permission.FOREGROUND_SERVICE_MEDIA_PLAYBACK",
    "android.permission.MODIFY_AUDIO_SETTINGS",
    "android.permission.POST_NOTIFICATIONS",
    "android.permission.READ_MEDIA_AUDIO",
    "android.permission.READ_EXTERNAL_STORAGE",
    # Lets the in-app updater open the system installer on a downloaded APK.
    "android.permission.REQUEST_INSTALL_PACKAGES",
]

inserts = []
for perm in permissions:
    marker = f'android:name="{perm}"'
    if marker in text:
        continue
    if perm == "android.permission.READ_EXTERNAL_STORAGE":
        inserts.append(
            f'    <uses-permission android:name="{perm}" android:maxSdkVersion="32" />'
        )
    else:
        inserts.append(f'    <uses-permission android:name="{perm}" />')

if inserts:
    app_idx = text.find("<application")
    if app_idx == -1:
        print("::error::could not find <application> in AndroidManifest.xml", file=sys.stderr)
        sys.exit(1)
    text = text[:app_idx] + "\n".join(inserts) + "\n" + text[app_idx:]
    print(f"Patched {len(inserts)} Android permission(s)")
    for line in inserts:
        print(f"  + {line.strip()}")
else:
    print("Android permissions already present")

orientation = 'android:screenOrientation="portrait"'

def patch_activity(match: re.Match) -> str:
    tag = match.group(0)
    if "android:screenOrientation=" in tag:
        return re.sub(
            r'android:screenOrientation="[^"]*"',
            orientation,
            tag,
            count=1,
        )
    if tag.endswith("/>"):
        return tag[:-2].rstrip() + f"\n            {orientation} />"
    if tag.endswith(">"):
        return tag[:-1].rstrip() + f"\n            {orientation}>"
    return tag

patched, count = re.subn(
    r"<activity\b[^>]*/?>",
    patch_activity,
    text,
    flags=re.IGNORECASE | re.DOTALL,
)
if count == 0:
    print("::error::No <activity> tags found in AndroidManifest.xml", file=sys.stderr)
    sys.exit(1)

path.write_text(patched, encoding="utf-8")
print(f"Locked {count} activity(ies) to portrait")

# Early crash reporter ContentProvider (runs before Activity).
text = path.read_text(encoding="utf-8")
provider_name = "app.bmdarklight.wave.CrashInitProvider"
if provider_name not in text:
    provider_xml = (
        '        <provider\n'
        f'            android:name="{provider_name}"\n'
        '            android:authorities="${applicationId}.wave-crash-init"\n'
        '            android:exported="false"\n'
        '            android:initOrder="100" />\n'
    )
    app_open = re.search(r"<application\b[^>]*>", text, flags=re.IGNORECASE | re.DOTALL)
    if not app_open:
        print("::error::could not find <application> open tag for CrashInitProvider", file=sys.stderr)
        sys.exit(1)
    insert_at = app_open.end()
    text = text[:insert_at] + "\n" + provider_xml + text[insert_at:]
    path.write_text(text, encoding="utf-8")
    print("Registered CrashInitProvider in AndroidManifest.xml")
else:
    print("CrashInitProvider already present in AndroidManifest.xml")

# FileProvider that shares downloaded update APKs with the system installer.
text = path.read_text(encoding="utf-8")
update_provider = "app.bmdarklight.wave.UpdateFileProvider"
if update_provider not in text:
    provider_xml = (
        '        <provider\n'
        f'            android:name="{update_provider}"\n'
        '            android:authorities="${applicationId}.wave-updates"\n'
        '            android:exported="false"\n'
        '            android:grantUriPermissions="true">\n'
        '            <meta-data\n'
        '                android:name="android.support.FILE_PROVIDER_PATHS"\n'
        '                android:resource="@xml/wave_update_paths" />\n'
        '        </provider>\n'
    )
    app_open = re.search(r"<application\b[^>]*>", text, flags=re.IGNORECASE | re.DOTALL)
    if not app_open:
        print("::error::could not find <application> open tag for UpdateFileProvider", file=sys.stderr)
        sys.exit(1)
    insert_at = app_open.end()
    text = text[:insert_at] + "\n" + provider_xml + text[insert_at:]
    path.write_text(text, encoding="utf-8")
    print("Registered UpdateFileProvider in AndroidManifest.xml")
else:
    print("UpdateFileProvider already present in AndroidManifest.xml")

# Network security config: cleartext only to the loopback stream server.
text = path.read_text(encoding="utf-8")
if "android:networkSecurityConfig=" not in text:
    app_open = re.search(r"<application\b[^>]*>", text, flags=re.IGNORECASE | re.DOTALL)
    if not app_open:
        print("::error::could not find <application> open tag for networkSecurityConfig", file=sys.stderr)
        sys.exit(1)
    tag = app_open.group(0)
    attr = 'android:networkSecurityConfig="@xml/wave_network_security"'
    if tag.endswith("/>"):
        patched_tag = tag[:-2].rstrip() + f"\n        {attr} />"
    else:
        patched_tag = tag[:-1].rstrip() + f"\n        {attr}>"
    text = text[:app_open.start()] + patched_tag + text[app_open.end():]
    path.write_text(text, encoding="utf-8")
    print("Set networkSecurityConfig on <application>")
else:
    print("networkSecurityConfig already set in AndroidManifest.xml")
PY

# Media3 / ExoPlayer deps for WaveExoPlayer
python3 - <<'PY'
from pathlib import Path
import re

candidates = list(Path("src-tauri/gen/android").rglob("build.gradle.kts"))
app_gradle = None
for path in candidates:
    text = path.read_text(encoding="utf-8")
    if "dependencies" in text and ("tauri" in text.lower() or path.parts[-2] == "app"):
        # Prefer the app module gradle
        if path.parent.name == "app" or "namespace" in text:
            app_gradle = path
            break
        if app_gradle is None:
            app_gradle = path

if app_gradle is None:
    # Fall back: app/build.gradle.kts
    fallback = Path("src-tauri/gen/android/app/build.gradle.kts")
    if fallback.is_file():
        app_gradle = fallback

if app_gradle is None:
    raise SystemExit("::error::Could not find Android app build.gradle.kts")

text = app_gradle.read_text(encoding="utf-8")
deps = [
    'implementation("androidx.media3:media3-exoplayer:1.5.1")',
    'implementation("androidx.media3:media3-common:1.5.1")',
]
missing = [d for d in deps if d not in text]
if not missing:
    print(f"Media3 deps already present in {app_gradle}")
else:
    # Insert before the closing of the dependencies block
    m = re.search(r"dependencies\s*\{", text)
    if not m:
        raise SystemExit(f"::error::No dependencies {{}} in {app_gradle}")
    # Find matching closing brace for dependencies
    start = m.end()
    depth = 1
    i = start
    while i < len(text) and depth:
        if text[i] == "{":
            depth += 1
        elif text[i] == "}":
            depth -= 1
        i += 1
    insert_at = i - 1
    injection = "\n    // Wave ExoPlayer (content:// playback)\n"
    injection += "\n".join(f"    {d}" for d in missing) + "\n"
    text = text[:insert_at] + injection + text[insert_at:]
    app_gradle.write_text(text, encoding="utf-8")
    print(f"Patched Media3 deps into {app_gradle}")
    for d in missing:
        print(f"  + {d}")
PY
