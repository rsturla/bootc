use std assert
use tap.nu

tap begin "composefs integration smoke test"

bootc internals test-composefs

bootc internals cfs --help
bootc internals cfs oci pull docker://busybox busybox
test -L /sysroot/composefs/streams/refs/busybox

tap ok
