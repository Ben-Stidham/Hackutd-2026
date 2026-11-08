None selected 

Skip to content
Using Gmail with screen readers
Enable desktop notifications for Gmail.
   OK  No thanks
1 of 5,509
Error Generator
Inbox

Jacob Zeman
Attachments
Sat, Nov 7, 10:06 PM (5 hours ago)
 

Jacob Zeman <charredhelfroze@gmail.com>
Attachments
2:22 AM (1 hour ago)
to me



On Sun, Nov 8, 2026 at 4:06 AM Jacob Zeman <charredhelfroze@gmail.com> wrote:


 One attachment
  •  Scanned by Gmail
Compose:
New Message
Recipients
Subject


mkdir -p ~/linux-damage-lab/etc
mkdir -p ~/linux-damage-lab/bin
mkdir -p ~/linux-damage-lab/usr
mkdir -p ~/linux-damage-lab/dev


gen2.sh
100%
#!/usr/bin/env bash

set -uo pipefail

BASE="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
WORK="$BASE/error-test-work"
BUILD="$WORK/build"
REPO="$WORK/repo"
ROOTS="$WORK/roots"

mkdir -p "$BUILD" "$REPO" "$ROOTS"

# ---------------------------------------
# General utilities
# ---------------------------------------

require_tools() {
    for tool in pacman makepkg repo-add sudo; do
        command -v "$tool" >/dev/null 2>&1 || return 1
    done
}

# ---------------------------------------
# Pacman package builder
# ---------------------------------------

build_package() {
    local name="$1"
    local version="${2:-1.0}"
    local dependency="${3:-}"
    local conflict="${4:-}"

    local dir="$BUILD/$name"

    mkdir -p "$dir"

    cat > "$dir/PKGBUILD" <<EOF
pkgname=$name
pkgver=$version
pkgrel=1
pkgdesc="Arch Linux error testing fixture"
arch=('any')
license=('MIT')
depends=()
conflicts=()
EOF

    [[ -z "$dependency" ]] ||
        printf "depends=('%s')\n" "$dependency" >> "$dir/PKGBUILD"

    [[ -z "$conflict" ]] ||
        printf "conflicts=('%s')\n" "$conflict" >> "$dir/PKGBUILD"

    cat >> "$dir/PKGBUILD" <<'EOF'

package() {
    install -d "$pkgdir/usr/share/$pkgname"
    printf 'Test fixture\n' > "$pkgdir/usr/share/$pkgname/info"
}
EOF

    (
        cd "$dir" &&
        makepkg --nodeps --noconfirm --cleanbuild -f
    ) >/dev/null 2>&1 || return 1

    local archive="$dir/$name-$version-1-any.pkg.tar.zst"

    [[ -f "$archive" ]] || return 1

    cp "$archive" "$REPO/"
}

prepare_packages() {
    build_package test-base || return 1
    build_package test-missing 1.0 test-does-not-exist || return 1

    build_package test-versioned-dep || return 1
    build_package test-needs-new 1.0 \
        'test-versioned-dep>=2.0' || return 1

    build_package test-cycle-a 1.0 test-cycle-b || return 1
    build_package test-cycle-b 1.0 test-cycle-a || return 1

    build_package test-conflict-a 1.0 "" test-conflict-b || return 1
    build_package test-conflict-b || return 1

    build_package test-file-a || return 1
    build_package test-file-b || return 1

    build_package test-stale-old || return 1
    build_package test-stale-new || return 1

    (
        cd "$REPO" &&
        repo-add testrepo.db.tar.gz ./*.pkg.tar.zst
    ) >/dev/null 2>&1 || return 1

    mkdir -p "$WORK/old-repo"

    cp "$REPO/test-base-1.0-1-any.pkg.tar.zst" "$WORK/old-repo/"
    cp "$REPO/test-stale-old-1.0-1-any.pkg.tar.zst" "$WORK/old-repo/"

    (
        cd "$WORK/old-repo" &&
        repo-add testrepo.db.tar.gz ./*.pkg.tar.zst
    ) >/dev/null 2>&1 || return 1
}

# ---------------------------------------
# Isolated pacman roots
# ---------------------------------------

create_root() {
    local name="$1"
    local root="$ROOTS/$name"

    mkdir -p \
        "$root/var/lib/pacman/local" \
        "$root/var/cache/pacman/pkg" \
        "$root/var/log"

    cat > "$root/pacman.conf" <<EOF
[options]
RootDir = $root
DBPath = /var/lib/pacman
CacheDir = /var/cache/pacman/pkg
LogFile = /var/log/pacman.log
Architecture = auto
SigLevel = Never
LocalFileSigLevel = Never

[testrepo]
Server = file://$REPO
EOF
}

run_pacman() {
    local root="$1"
    shift

    sudo pacman \
        --config "$root/pacman.conf" \
        --root "$root" \
        --dbpath "$root/var/lib/pacman" \
        --cachedir "$root/var/cache/pacman/pkg" \
        --logfile "$root/var/log/pacman.log" \
        --noconfirm "$@" >/dev/null 2>&1
}

pacman_test() {
    local id="$1"
    local name="$2"
    local root="$3"
    shift 3

    run_pacman "$root" "$@" || true
}

# ---------------------------------------
# Tests 1-10: Pacman
# ---------------------------------------

test_01() {
    pacman_test 1 "Missing dependency" "$ROOTS/01" \
        -U "$REPO/test-missing-1.0-1-any.pkg.tar.zst"
}

test_02() {
    pacman_test 2 "Version mismatch" "$ROOTS/02" \
        -S test-needs-new
}

test_03() {
    pacman_test 3 "Dependency cycle" "$ROOTS/03" \
        -S test-cycle-a
}

test_04() {
    pacman_test 4 "Package conflict" "$ROOTS/04" \
        -U "$REPO/test-conflict-a-1.0-1-any.pkg.tar.zst" \
           "$REPO/test-conflict-b-1.0-1-any.pkg.tar.zst"
}

test_05() {
    pacman_test 5 "Target not found" "$ROOTS/05" \
        -S package-does-not-exist
}

test_06() {
    local bad="$WORK/invalid.pkg.tar.zst"
    printf 'Invalid package archive\n' > "$bad"

    pacman_test 6 "Invalid package archive" "$ROOTS/06" -U "$bad"

    rm -f "$bad"
}

test_07() {
    local root="$ROOTS/07"

    cat > "$root/pacman.conf" <<EOF
[options]
RootDir = $root
DBPath = /var/lib/pacman
Architecture = auto
SigLevel = Required
LocalFileSigLevel = Required
EOF

    pacman_test 7 "Package signature failure" "$root" \
        -U "$REPO/test-base-1.0-1-any.pkg.tar.zst"
}

test_08() {
    local root="$ROOTS/08"
    local dir="$BUILD/file-conflict"

    mkdir -p "$dir"

    cat > "$dir/PKGBUILD" <<'EOF'
pkgname=test-file-a
pkgver=1.0
pkgrel=1
pkgdesc="File conflict fixture A"
arch=('any')
package() {
    install -d "$pkgdir/usr/share/test-shared"
    echo "A" > "$pkgdir/usr/share/test-shared/file"
}
EOF

    (
        cd "$dir" &&
        makepkg --nodeps --noconfirm -f
    ) >/dev/null 2>&1 || return 1

    local archive_a="$dir/test-file-a-1.0-1-any.pkg.tar.zst"

    cat > "$dir/PKGBUILD" <<'EOF'
pkgname=test-file-b
pkgver=1.0
pkgrel=1
pkgdesc="File conflict fixture B"
arch=('any')
package() {
    install -d "$pkgdir/usr/share/test-shared"
    echo "B" > "$pkgdir/usr/share/test-shared/file"
}
EOF

    (
        cd "$dir" &&
        makepkg --nodeps --noconfirm -f
    ) >/dev/null 2>&1 || return 1

    local archive_b="$dir/test-file-b-1.0-1-any.pkg.tar.zst"

    run_pacman "$root" -U "$archive_a" || return 1
    run_pacman "$root" -U "$archive_b" || true
}

test_09() {
    local root="$ROOTS/09"
    local lock="$root/var/lib/pacman/db.lck"

    touch "$lock"

    pacman_test 9 "Pacman database lock" "$root" -S test-base

    rm -f "$lock"
}

test_10() {
    local root="$ROOTS/10"

    cat > "$root/pacman.conf" <<EOF
[options]
RootDir = $root
DBPath = /var/lib/pacman
Architecture = auto
SigLevel = Never

[testrepo]
Server = file://$WORK/old-repo
EOF

    sudo pacman --config "$root/pacman.conf" \
        --root "$root" \
        --dbpath "$root/var/lib/pacman" \
        -Sy >/dev/null 2>&1 || true

    sed -i "s|$WORK/old-repo|$REPO|" "$root/pacman.conf"

    pacman_test 10 "Stale repository database" "$root" \
        -S test-stale-new
}

# ---------------------------------------
# Tests 11-20: Linux errors
# ---------------------------------------

test_11() {
    local dir="$WORK/permission"
    mkdir -p "$dir"

    local file="$dir/protected.txt"

    printf 'Protected test file\n' > "$file"
    chmod 000 "$file"

    sudo -u nobody cat "$file" >/dev/null 2>&1 || true

    chmod 600 "$file"
}

test_12() {
    local dir="$WORK/readonly"
    mkdir -p "$dir"

    sudo mount -t tmpfs -o size=4m tmpfs "$dir" >/dev/null 2>&1 || return 0

    if sudo mount -o remount,ro "$dir" >/dev/null 2>&1; then
        sudo touch "$dir/test-file" >/dev/null 2>&1 || true
    fi

    sudo mount -o remount,rw "$dir" >/dev/null 2>&1 || true
    sudo umount "$dir" >/dev/null 2>&1 || true
}

test_13() {
    local dir="$WORK/disk-full"
    mkdir -p "$dir"

    sudo mount -t tmpfs -o size=1m tmpfs "$dir" >/dev/null 2>&1 || return 0

    sudo dd if=/dev/zero of="$dir/full-file" \
        bs=256K count=16 status=none >/dev/null 2>&1 || true

    sudo umount "$dir" >/dev/null 2>&1 || true
}

test_14() {
    local dir="$WORK/does-not-exist"

    rm -rf "$dir"

    ls "$dir" >/dev/null 2>&1 || true
}

test_15() {
    local dir="$WORK/broken-link"
    mkdir -p "$dir"

    ln -sf "$dir/nonexistent-target" "$dir/broken"

    cat "$dir/broken" >/dev/null 2>&1 || true
}

test_16() {
    local dir="$WORK/ownership"
    mkdir -p "$dir"

    local file="$dir/owned-file"

    printf 'Ownership test\n' > "$file"

    sudo chown root:root "$file" >/dev/null 2>&1 || true
    sudo chmod 600 "$file" >/dev/null 2>&1 || true

    sudo -u nobody chown nobody "$file" >/dev/null 2>&1 || true
}

test_17() {
    getent ahosts nonexistent.invalid >/dev/null 2>&1 || true
}

test_18() {
    local port

    port="$(python3 -c '
import socket
s = socket.socket()
s.bind(("127.0.0.1", 0))
print(s.getsockname()[1])
s.close()
')" 2>/dev/null

    python3 -c "
import socket
s = socket.socket()
s.settimeout(2)
s.connect(('127.0.0.1', $port))
" >/dev/null 2>&1 || true
}

test_19() {
    python3 -c '
import socket, threading, time

server = socket.socket()
server.bind(("127.0.0.1", 0))
server.listen(1)
port = server.getsockname()[1]

def accept_connection():
    conn, _ = server.accept()
    time.sleep(3)
    conn.close()

thread = threading.Thread(target=accept_connection)
thread.start()

client = socket.socket()
client.settimeout(1)

try:
    client.connect(("127.0.0.1", port))
    client.recv(1)
except socket.timeout:
    pass
finally:
    client.close()
    server.close()
    thread.join()
' >/dev/null 2>&1 || true
}

test_20() {
    curl --silent --show-error \
        --max-time 3 \
        --proxy http://127.0.0.1:1 \
        http://example.com/ >/dev/null 2>&1 || true
}

# ---------------------------------------
# Tests 21-34: System errors
# ---------------------------------------

test_21() {
    command_that_does_not_exist_12345 >/dev/null 2>&1 || true
}

test_22() {
    ls --invalid-test-option >/dev/null 2>&1 || true
}

test_23() {
    python3 -c '
import configparser
c = configparser.ConfigParser()
c.read_string("[broken\n")
' >/dev/null 2>&1 || true
}

test_24() {
    python3 -c '
import ctypes
ctypes.CDLL("lib_missing_test_12345.so")
' >/dev/null 2>&1 || true
}

test_25() {
    python3 -c '
import socket
s1 = socket.socket()
s1.bind(("127.0.0.1", 0))
s1.listen()
s2 = socket.socket()
try:
    s2.bind(s1.getsockname())
except OSError:
    pass
finally:
    s1.close()
    s2.close()
' >/dev/null 2>&1 || true
}

test_26() {
    kill -0 99999999 >/dev/null 2>&1 || true
}

test_27() {
    systemctl status nonexistent-test-service.service >/dev/null 2>&1 || true
}

test_28() {
    python3 -c '
import os
r,w = os.pipe()
os.close(r)
try:
    os.write(w,b"test")
except OSError:
    pass
finally:
    os.close(w)
' >/dev/null 2>&1 || true
}

test_29() {
    cat /dev/test-device-that-does-not-exist >/dev/null 2>&1 || true
}

test_30() {
    bash -c 'ulimit -n 32; exec python3 -c "
files=[]
try:
    while True:
        files.append(open(\"/dev/null\"))
except OSError:
    pass
finally:
    for f in files:
        f.close()
"' >/dev/null 2>&1 || true
}

test_31() {
    local file="$WORK/non-executable.sh"

    printf '#!/bin/sh\necho test\n' > "$file"
    chmod 644 "$file"

    "$file" >/dev/null 2>&1 || true

    rm -f "$file"
}

test_32() {
    python3 -c '
import os
try:
    os.read(999999, 10)
except OSError:
    pass
' >/dev/null 2>&1 || true
}

test_33() {
    python3 -c '
import os
try:
    os.lseek(-1, 0, os.SEEK_SET)
except OSError:
    pass
' >/dev/null 2>&1 || true
}

test_34() {
    python3 -c '
import os
r,w = os.pipe()
os.set_blocking(r, False)
try:
    os.read(r, 10)
except BlockingIOError:
    pass
finally:
    os.close(r)
    os.close(w)
' >/dev/null 2>&1 || true
}

# ---------------------------------------
# Test 35: Cascading package failures
# ---------------------------------------

test_35() {
    local dir="$WORK/cascade-035"
    local root="$ROOTS/35"

    mkdir -p "$dir" \
        "$root/var/lib/pacman/local" \
        "$root/var/cache/pacman/pkg" \
        "$root/var/log"

    local config="$root/pacman.conf"

    cat > "$config" <<EOF
[options]
RootDir = $root
DBPath = /var/lib/pacman
CacheDir = /var/cache/pacman/pkg
LogFile = /var/log/pacman.log
Architecture = auto
SigLevel = Never
LocalFileSigLevel = Never

[testrepo]
Server = file://$REPO
EOF

    cat > "$dir/invalid.conf" <<'EOF'
[options
SigLevel = Never
EOF

    sudo pacman --config "$dir/invalid.conf" -Syy \
        >/dev/null 2>&1 || true

    cat > "$dir/missing-repo.conf" <<EOF
[options]
Architecture = auto
SigLevel = Never

[missingrepo]
Server = file://$dir/nonexistent-repository
EOF

    sudo pacman --config "$dir/missing-repo.conf" -Sy \
        >/dev/null 2>&1 || true

    printf 'NOT A VALID PACMAN PACKAGE ARCHIVE' \
        > "$dir/corrupted.pkg.tar.zst"

    run_pacman "$root" -U "$dir/corrupted.pkg.tar.zst" || true

    run_pacman "$root" -U \
        "$REPO/test-missing-1.0-1-any.pkg.tar.zst" || true

    run_pacman "$root" -S test-needs-new || true

    run_pacman "$root" -U \
        "$REPO/test-conflict-a-1.0-1-any.pkg.tar.zst" \
        "$REPO/test-conflict-b-1.0-1-any.pkg.tar.zst" || true

    local lock="$root/var/lib/pacman/db.lck"

    touch "$lock"

    run_pacman "$root" -S test-base || true

    rm -f "$lock"

    # Dependency cycles are not guaranteed to produce an error.
    # Verify repository metadata without printing anything.

    [[ -f "$REPO/testrepo.db" || -f "$REPO/testrepo.db.tar.gz" ]] \
        >/dev/null 2>&1 || true

    rm -f "$dir/corrupted.pkg.tar.zst"
}

# ---------------------------------------
# Menu
# ---------------------------------------

main() {
    require_tools || return 1

    command -v python3 >/dev/null 2>&1 || return 1

    prepare_packages || return 1

    for n in {1..10}; do
        create_root "$(printf '%02d' "$n")"
    done

    create_root 35

    while true; do
        echo
        echo "======================================"
        echo " ARCH LINUX ERROR GENERATOR"
        echo "======================================"
        echo " 1. Missing dependency"
        echo " 2. Version mismatch"
        echo " 3. Dependency cycle"
        echo " 4. Package conflict"
        echo " 5. Target not found"
        echo " 6. Invalid package archive"
        echo " 7. Package signature failure"
        echo " 8. File conflict"
        echo " 9. Pacman database lock"
        echo "10. Stale repository database"
        echo "--------------------------------------"
        echo "11. Permission denied"
        echo "12. Read-only filesystem"
        echo "13. Disk full"
        echo "14. Missing directory"
        echo "15. Broken symbolic link"
        echo "16. File ownership mismatch"
        echo "17. DNS resolution failure"
        echo "18. Connection refused"
        echo "19. Network timeout"
        echo "20. Invalid proxy configuration"
        echo "--------------------------------------"
        echo "21. Command not found"
        echo "22. Invalid command option"
        echo "23. Invalid configuration syntax"
        echo "24. Missing shared library"
        echo "25. Port already in use"
        echo "26. Process not found"
        echo "27. Service not running"
        echo "28. Broken pipe"
        echo "29. Input/output error"
        echo "30. Too many open files"
        echo "31. Invalid permissions"
        echo "32. Read error"
        echo "33. Invalid argument"
        echo "34. Resource temporarily unavailable"
        echo "35. Cascading package failures"
        echo
        echo " 0. Exit"
        echo

        read -r -p "Select test: " choice || break

        case "$choice" in
            1) test_01 ;;
            2) test_02 ;;
            3) test_03 ;;
            4) test_04 ;;
            5) test_05 ;;
            6) test_06 ;;
            7) test_07 ;;
            8) test_08 ;;
            9) test_09 ;;
            10) test_10 ;;
            11) test_11 ;;
            12) test_12 ;;
            13) test_13 ;;
            14) test_14 ;;
            15) test_15 ;;
            16) test_16 ;;
            17) test_17 ;;
            18) test_18 ;;
            19) test_19 ;;
            20) test_20 ;;
            21) test_21 ;;
            22) test_22 ;;
            23) test_23 ;;
            24) test_24 ;;
            25) test_25 ;;
            26) test_26 ;;
            27) test_27 ;;
            28) test_28 ;;
            29) test_29 ;;
            30) test_30 ;;
            31) test_31 ;;
            32) test_32 ;;
            33) test_33 ;;
            34) test_34 ;;
            35) test_35 ;;
            0) break ;;
            *) echo "Invalid selection." ;;
        esac

        echo
        read -r -p "Run another test? (y/n): " again || break

        [[ "$again" =~ ^[Yy]$ ]] || break
    done
}

main
Displaying gen2.sh.
