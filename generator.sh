#!/usr/bin/env bash

set -uo pipefail

BASE="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
WORK="$BASE/error-test-work"
BUILD="$WORK/build"
REPO="$WORK/repo"
ROOTS="$WORK/roots"
LOGS="$BASE/error-logs"

mkdir -p "$BUILD" "$REPO" "$ROOTS" "$LOGS"

# ---------------------------------------
# General utilities
# ---------------------------------------

require_tools() {
    local missing=0

    for tool in pacman makepkg repo-add sudo; do
        if ! command -v "$tool" >/dev/null 2>&1; then
            echo "Missing prerequisite: $tool"
            missing=1
        fi
    done

    return "$missing"
}

record_test() {
    local id="$1"
    local name="$2"
    shift 2

    local logfile="$LOGS/ARCH_$(printf '%03d' "$id").log"
    local code

    echo
    echo "======================================"
    echo "TEST ARCH_$(printf '%03d' "$id")"
    echo "$name"
    echo "======================================"

    if "$@" > "$logfile" 2>&1; then
        code=0
    else
        code=$?
    fi

    cat "$logfile"

    echo
    echo "Exit code: $code"
    echo "$code" > "$LOGS/ARCH_$(printf '%03d' "$id").exit"
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

    if ! (
        cd "$dir" &&
        makepkg --nodeps --noconfirm --cleanbuild -f
    ) > "$LOGS/build-$name.log" 2>&1; then
        echo "Build failed: $name"
        cat "$LOGS/build-$name.log"
        return 1
    fi

    local archive="$dir/$name-$version-1-any.pkg.tar.zst"

    if [[ ! -f "$archive" ]]; then
        echo "Missing archive: $archive"
        return 1
    fi

    cp "$archive" "$REPO/"
}

prepare_packages() {
    echo "Preparing package fixtures..."

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
    ) > "$LOGS/repository.log" 2>&1 || return 1

    mkdir -p "$WORK/old-repo"

    cp "$REPO/test-base-1.0-1-any.pkg.tar.zst" "$WORK/old-repo/"
    cp "$REPO/test-stale-old-1.0-1-any.pkg.tar.zst" "$WORK/old-repo/"

    (
        cd "$WORK/old-repo" &&
        repo-add testrepo.db.tar.gz ./*.pkg.tar.zst
    ) >> "$LOGS/repository.log" 2>&1 || return 1

    echo "Package fixtures ready."
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
        --noconfirm "$@"
}

pacman_test() {
    local id="$1"
    local name="$2"
    local root="$3"
    shift 3

    record_test "$id" "$name" run_pacman "$root" "$@"
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

    pacman_test 6 "Invalid package archive" "$ROOTS/06" \
        -U "$bad"
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
    ) > "$LOGS/build-file-a.log" 2>&1 || return 1

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
    ) > "$LOGS/build-file-b.log" 2>&1 || return 1

    local archive_b="$dir/test-file-b-1.0-1-any.pkg.tar.zst"

    if ! run_pacman "$root" -U "$archive_a" \
        > "$LOGS/ARCH_008-setup.log" 2>&1; then
        cat "$LOGS/ARCH_008-setup.log"
        return 1
    fi

    record_test 8 "File conflict" run_pacman "$root" -U "$archive_b"
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
        -Sy > "$LOGS/ARCH_010-sync.log" 2>&1 || return 1

    sed -i "s|$WORK/old-repo|$REPO|" "$root/pacman.conf"

    pacman_test 10 "Stale repository database" "$root" \
        -S test-stale-new
}

# ---------------------------------------
# Tests 11-20: Linux errors
# ---------------------------------------

# 11. Permission denied

test_11() {
    local dir="$WORK/permission"
    mkdir -p "$dir"

    local file="$dir/protected.txt"
    printf 'Protected test file\n' > "$file"
    chmod 000 "$file"

    record_test 11 "Permission denied" \
        sudo -u nobody cat "$file"

    chmod 600 "$file"
}

# 12. Read-only filesystem

test_12() {
    local dir="$WORK/readonly"
    mkdir -p "$dir"

    sudo mount -t tmpfs -o size=4m tmpfs "$dir" || {
        echo "Could not mount temporary filesystem."
        return 1
    }

    if ! sudo mount -o remount,ro "$dir"; then
        sudo umount "$dir"
        return 1
    fi

    record_test 12 "Read-only filesystem" \
        sudo touch "$dir/test-file"

    sudo mount -o remount,rw "$dir"
    sudo umount "$dir"
}

# 13. Disk full

test_13() {
    local dir="$WORK/disk-full"
    mkdir -p "$dir"

    sudo mount -t tmpfs -o size=1m tmpfs "$dir" || {
        echo "Could not mount temporary filesystem."
        return 1
    }

    record_test 13 "Disk full" \
        sudo dd if=/dev/zero of="$dir/full-file" \
        bs=256K count=16 status=none

    sudo umount "$dir"
}

# 14. Missing directory

test_14() {
    local dir="$WORK/does-not-exist"

    rm -rf "$dir"

    record_test 14 "Missing directory" \
        ls "$dir"
}

# 15. Broken symbolic link

test_15() {
    local dir="$WORK/broken-link"
    mkdir -p "$dir"

    ln -s "$dir/nonexistent-target" "$dir/broken"

    record_test 15 "Broken symbolic link" \
        cat "$dir/broken"
}

# 16. File ownership mismatch

test_16() {
    local dir="$WORK/ownership"
    mkdir -p "$dir"

    local file="$dir/owned-file"
    printf 'Ownership test\n' > "$file"

    sudo chown root:root "$file"
    sudo chmod 600 "$file"

    record_test 16 "File ownership mismatch" \
        sudo -u nobody chown nobody "$file"

}

# 17. DNS resolution failure

test_17() {
    record_test 17 "DNS resolution failure" \
        getent ahosts nonexistent.invalid
}

# 18. Connection refused

test_18() {
    local port

    port="$(python3 -c '
import socket
s = socket.socket()
s.bind(("127.0.0.1", 0))
print(s.getsockname()[1])
s.close()
')"

    record_test 18 "Connection refused" \
        python3 -c "
import socket
s = socket.socket()
s.settimeout(2)
s.connect(('127.0.0.1', $port))
"
}

# 19. Network timeout

test_19() {
    record_test 19 "Network timeout" \
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
    print("TimeoutError: operation timed out")
    raise SystemExit(1)
finally:
    client.close()
    server.close()
    thread.join()
'
}

# 20. Invalid proxy configuration

test_20() {
    if ! command -v curl >/dev/null 2>&1; then
        echo "curl is required for test 20."
        return 1
    fi

    record_test 20 "Invalid proxy configuration" \
        curl --silent --show-error \
        --max-time 3 \
        --proxy http://127.0.0.1:1 \
        http://example.com/
}

# ---------------------------------------
# Tests 21-35: System errors
# ---------------------------------------

# Tests 21-35

test_21() {
    command_that_does_not_exist_12345
}

test_22() {
    ls --invalid-test-option
}

test_23() {
    python3 -c '
import configparser
c = configparser.ConfigParser()
c.read_string("[broken\n")
'
}

test_24() {
    python3 -c '
import ctypes
ctypes.CDLL("lib_missing_test_12345.so")
'
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
except OSError as e:
    print(e)
finally:
    s1.close()
    s2.close()
'
}

test_26() {
    kill -0 99999999
}

test_27() {
    systemctl status nonexistent-test-service.service
}

test_28() {
    python3 -c '
import os
r,w = os.pipe()
os.close(r)
try:
    os.write(w,b"test")
except OSError as e:
    print(e)
    raise SystemExit(1)
finally:
    os.close(w)
'
}

test_29() {
    cat /dev/test-device-that-does-not-exist
}

test_30() {
    bash -c 'ulimit -n 32; exec python3 -c "
files=[]
try:
    while True:
        files.append(open(\"/dev/null\"))
except OSError as e:
    print(e)
    raise SystemExit(1)
finally:
    for f in files:
        f.close()
"'
}

test_31() {
    local file="$WORK/non-executable.sh"
    printf '#!/bin/sh\necho test\n' > "$file"
    chmod 644 "$file"
    "$file"
}

test_32() {
    python3 -c '
import os
try:
    os.read(999999, 10)
except OSError as e:
    print(e)
    raise SystemExit(1)
'
}

test_33() {
    python3 -c '
import os
try:
    os.lseek(-1, 0, os.SEEK_SET)
except OSError as e:
    print(e)
    raise SystemExit(1)
'
}

test_34() {
    python3 -c '
import os
r,w = os.pipe()
os.set_blocking(r, False)
try:
    os.read(r, 10)
except BlockingIOError as e:
    print(e)
    raise SystemExit(1)
finally:
    os.close(r)
    os.close(w)
'
}



test_35() {
    local id=35
    local dir="$WORK/cascade-035"
    local root="$ROOTS/35"
    local log="$LOGS/ARCH_035.log"
    local passed=0
    local failed=0
    local skipped=0

    mkdir -p "$dir" "$root/var/lib/pacman/local" \
        "$root/var/cache/pacman/pkg" "$root/var/log"

    : > "$log"

    # Capture actual command output and verify expected errors.
    stage() {
        local name="$1"
        local pattern="$2"
        shift 2

        local output="$dir/stage.log"

        {
            echo
            echo "========================================"
            echo "STAGE: $name"
            echo "========================================"
            printf 'COMMAND:'
            printf ' %q' "$@"
            echo
        } | tee -a "$log"

        if "$@" > "$output" 2>&1; then
            cat "$output" | tee -a "$log"
            echo "FAIL: Command unexpectedly succeeded." | tee -a "$log"
            ((failed+=1))
        else
            local code=$?
            cat "$output" | tee -a "$log"

            if grep -Eiq "$pattern" "$output"; then
                echo "PASS: Expected error reproduced (exit $code)." | tee -a "$log"
                ((passed+=1))
            else
                echo "FAIL: Expected error signature not found (exit $code)." | tee -a "$log"
                ((failed+=1))
           fi
        fi
    }

    echo "ARCH LINUX CASCADING FAILURE TEST" | tee -a "$log"
    echo "Test started: $(date)" | tee -a "$log"

    # Create an isolated pacman configuration.
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

    # 1. Invalid configuration.
    cat > "$dir/invalid.conf" <<'EOF'
[options
SigLevel = Never
EOF

    stage "Invalid pacman configuration" \
        'config file|syntax|parse|could not be read' \
        sudo pacman --config "$dir/invalid.conf" -Syy

    # 2. Missing repository database.
    cat > "$dir/missing-repo.conf" <<EOF
[options]
Architecture = auto
SigLevel = Never

[missingrepo]
Server = file://$dir/nonexistent-repository
EOF

    stage "Missing repository database" \
        'failed to synchronize|failed retrieving|could not be read|error' \
        sudo pacman --config "$dir/missing-repo.conf" -Sy

    # 3. Corrupted package archive.
    printf 'NOT A VALID PACMAN PACKAGE ARCHIVE' \
        > "$dir/corrupted.pkg.tar.zst"

    stage "Corrupted package archive" \
        'invalid|could not|failed|error|archive' \
        sudo pacman --config "$config" \
        --root "$root" \
        --dbpath "$root/var/lib/pacman" \
        --cachedir "$root/var/cache/pacman/pkg" \
        --logfile "$root/var/log/pacman.log" \
        --noconfirm -U "$dir/corrupted.pkg.tar.zst"

    # 4. Missing dependency.
    stage "Missing package dependency" \
        'unable to satisfy|missing dependencies|requires|could not satisfy' \
        sudo pacman --config "$config" \
        --root "$root" \
        --dbpath "$root/var/lib/pacman" \
        --cachedir "$root/var/cache/pacman/pkg" \
        --logfile "$root/var/log/pacman.log" \
        --noconfirm -U "$REPO/test-missing-1.0-1-any.pkg.tar.zst"

    # 5. Dependency version mismatch.
    stage "Dependency version mismatch" \
        'unable to satisfy|could not satisfy|requires|version' \
        sudo pacman --config "$config" \
        --root "$root" \
        --dbpath "$root/var/lib/pacman" \
        --cachedir "$root/var/cache/pacman/pkg" \
        --logfile "$root/var/log/pacman.log" \
        --noconfirm -S test-needs-new

    # 6. Package conflict.
    stage "Package conflict" \
        'conflicts with|conflicting packages|unable to satisfy' \
        sudo pacman --config "$config" \
        --root "$root" \
        --dbpath "$root/var/lib/pacman" \
        --cachedir "$root/var/cache/pacman/pkg" \
        --logfile "$root/var/log/pacman.log" \
        --noconfirm -U \
        "$REPO/test-conflict-a-1.0-1-any.pkg.tar.zst" \
        "$REPO/test-conflict-b-1.0-1-any.pkg.tar.zst"

    # 7. Database lock.
    touch "$root/var/lib/pacman/db.lck"

    stage "Pacman database lock" \
        'database is locked|unable to lock|db.lck' \
        sudo pacman --config "$config" \
        --root "$root" \
        --dbpath "$root/var/lib/pacman" \
        --cachedir "$root/var/cache/pacman/pkg" \
        --logfile "$root/var/log/pacman.log" \
        --noconfirm -S test-base

    rm -f "$root/var/lib/pacman/db.lck"

    # 8. Repository metadata integrity.
    if [[ -f "$REPO/testrepo.db" || -f "$REPO/testrepo.db.tar.gz" ]]; then
        echo "Repository database exists." | tee -a "$log"
    else
        echo "Repository database missing." | tee -a "$log"
        ((failed+=1))
    fi

    # 9. Dependency cycle.
    # Pacman may accept and order cyclic dependencies rather than
    # reporting a cycle as an error. Do not fabricate an error here.
    echo | tee -a "$log"
    echo "STAGE: Dependency cycle" | tee -a "$log"
    echo "SKIP: Requires a separately validated cycle fixture." | tee -a "$log"
    ((skipped+=1))

    # Final report.
    {
        echo
        echo "========================================"
        echo "FINAL TEST 35 REPORT"
        echo "========================================"
        echo "Passed: $passed"
        echo "Failed: $failed"
        echo "Skipped: $skipped"
        echo "Total: $((passed + failed + skipped))"
        echo "========================================"
        echo "Log: $log"
    } | tee -a "$log"

    echo
    echo "Full diagnostic report: $log"

    [[ "$failed" -eq 0 ]]
}








# ---------------------------------------
# Menu
# ---------------------------------------

main() {
    require_tools || exit 1

    if ! command -v python3 >/dev/null 2>&1; then
        echo "Python 3 is required for network tests."
        exit 1
    fi

    if ! prepare_packages; then
        echo "Package preparation failed."
        exit 1
    fi

    for n in {1..10}; do
        create_root "$(printf '%02d' "$n")"
    done

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
        echo "35. Out of memory simulation"




        echo " 0. Exit"
        echo

        read -r -p "Select test: " choice

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
        read -r -p "Run another test? (y/n): " again

        [[ "$again" =~ ^[Yy]$ ]] || break
    done

    echo
    echo "Results saved in: $LOGS"


}

main



