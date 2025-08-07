use std assert
use tap.nu

tap begin "initramfs"

if (not ("/usr/lib/bootc/initramfs-setup" | path exists)) {
    print "No initramfs support"
    exit 0
}

journalctl -b -t bootc-root-setup.service --grep=OK

tap ok
