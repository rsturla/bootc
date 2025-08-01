use std assert
use tap.nu

tap begin "composefs integration smoke test"

bootc internals test-composefs

tap ok
