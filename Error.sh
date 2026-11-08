#!/usr/bin/env bash

set -euo pipefail

LAB="/tmp/pacman-troubleshooting-lab"

mkdir -p "$LAB"/{db/local,db/sync,cache,logs}

echo "Creating Pacman troubleshooting lab..."

# Scenario 1: Stale database lock
touch "$LAB/db/db.lck"

# Scenario 2: Invalid local package metadata
mkdir -p "$LAB/db/local/broken-package-1.0"

cat > "$LAB/db/local/broken-package-1.0/desc" <<'EOF'
%NAME%
broken-package

INVALID_METADATA
EOF

# Scenario 3: Corrupted sync database
printf '\x00\xFF\x00INVALID_DATABASE' \
  > "$LAB/db/sync/core.db"

# Scenario 4: Invalid package archive
printf 'NOT A VALID PACMAN PACKAGE' \
  > "$LAB/cache/broken-package.pkg.tar.zst"

# Scenario 5: Interrupted transaction record
cat > "$LAB/logs/transaction.log" <<'EOF'
[ALPM] transaction started
[ALPM] installing broken-package
[ALPM] transaction interrupted
EOF

echo
echo "Lab successfully created."
echo "Location: $LAB"
echo
echo "Scenarios:"
echo "1. Stale database lock"
echo "2. Invalid package metadata"
echo "3. Corrupted sync database"
echo "4. Invalid package archive"
echo "5. Interrupted transaction log"
echo
echo "Run: ./pacman-lab.sh --cleanup"

sudo pacman --dbpath /tmp/pacman-troubleshooting-lab/db -Sy
pacman --dbpath /tmp/pacman-troubleshooting-lab/db -Q

